[CmdletBinding()]
param(
  [ValidateRange(10, 86400)]
  [int]$WaitTimeoutSeconds = 1800,
  [ValidateRange(5, 300)]
  [int]$DrainLeaseSeconds = 30,
  [switch]$Force
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$appRoot = Split-Path -Parent $scriptRoot
$deploymentRoot = Split-Path -Parent $appRoot
$configPath = Join-Path $deploymentRoot "config\server.local.env.ps1"
$launcher = Join-Path $scriptRoot "start-switch-development-runtime.ps1"
$baseUrl = "http://127.0.0.1:18082"

if (-not (Test-Path -LiteralPath $configPath -PathType Leaf)) {
  throw "Configuration Switch developpement introuvable : $configPath"
}
. $configPath
if (-not $env:CST_ADMIN_TOKEN) {
  throw "CST_ADMIN_TOKEN est absent de la configuration locale."
}

$authHeaders = @{ Authorization = "Bearer $env:CST_ADMIN_TOKEN" }
$restartMutex = [Threading.Mutex]::new($false, "Local\SwitchDevelopmentRestart-18082")
$restartLockHeld = $false
$drainArmed = $false

function Get-DevelopmentHealth {
  try {
    return Invoke-RestMethod -Uri "$baseUrl/healthz" -TimeoutSec 3 -ErrorAction Stop
  } catch {
    return $null
  }
}

function Get-ActiveWorkloadCount {
  param($Health)
  if ($null -eq $Health) { return 0 }
  return [int]$Health.activeTerminals + [int]$Health.activeChatTurns
}

function Set-DevelopmentDrain {
  param([bool]$Draining)
  $body = @{
    draining = $Draining
    ttlSeconds = if ($Draining) { $DrainLeaseSeconds } else { 0 }
  } | ConvertTo-Json -Compress
  Invoke-RestMethod -Uri "$baseUrl/api/admin/drain" -Method Post -Headers $authHeaders `
    -ContentType "application/json" -Body $body -TimeoutSec 5 | Out-Null
}

function Assert-ExpectedServerProcess {
  param($Process)
  $executablePath = [string]$Process.ExecutablePath
  if (-not $executablePath) {
    throw "Le processus qui ecoute sur 18082 n'expose pas son executable ; arret refuse."
  }
  $resolved = [IO.Path]::GetFullPath($executablePath)
  $allowedRoots = @(
    ([IO.Path]::GetFullPath((Join-Path $deploymentRoot "app")).TrimEnd('\') + '\'),
    ([IO.Path]::GetFullPath((Join-Path $env:LOCALAPPDATA "SwitchDevelopmentRuntime")).TrimEnd('\') + '\')
  )
  $allowed = $allowedRoots | Where-Object {
    $resolved.StartsWith($_, [StringComparison]::OrdinalIgnoreCase)
  } | Select-Object -First 1
  if (-not $allowed) {
    throw "Le processus sur 18082 ne vient pas du runtime Switch developpement ; arret refuse."
  }
}

try {
  try {
    $restartLockHeld = $restartMutex.WaitOne(0)
  } catch [Threading.AbandonedMutexException] {
    $restartLockHeld = $true
  }
  if (-not $restartLockHeld) {
    throw "Un autre redemarrage Switch developpement est deja en cours."
  }

  $deadline = (Get-Date).AddSeconds($WaitTimeoutSeconds)
  while ($true) {
    $health = Get-DevelopmentHealth
    if ($null -eq $health) {
      & $launcher -DeploymentRoot $deploymentRoot
      return
    }

    $active = Get-ActiveWorkloadCount -Health $health
    if ($active -eq 0 -or ($Force -and (Get-Date) -ge $deadline)) {
      break
    }
    if ((Get-Date) -ge $deadline) {
      throw "Timeout : $active chat(s) ou terminal(aux) encore actifs. Redemarrage annule sans interruption."
    }
    Write-Output "Switch developpement occupe ($active session(s)) : attente de la fin avant redemarrage..."
    Start-Sleep -Seconds 1
  }

  Set-DevelopmentDrain -Draining $true
  $drainArmed = $true
  Start-Sleep -Milliseconds 300
  $health = Get-DevelopmentHealth
  $active = Get-ActiveWorkloadCount -Health $health
  if ($active -gt 0 -and -not $Force) {
    Set-DevelopmentDrain -Draining $false
    $drainArmed = $false
    throw "Une session a demarre pendant la bascule. Redemarrage annule ; aucun chat n'a ete interrompu."
  }

  $listener = Get-NetTCPConnection -State Listen -LocalPort 18082 -ErrorAction SilentlyContinue |
    Select-Object -First 1
  if ($listener) {
    $serverProcess = Get-CimInstance Win32_Process -Filter "ProcessId=$([int]$listener.OwningProcess)"
    Assert-ExpectedServerProcess -Process $serverProcess
    Stop-Process -Id ([int]$serverProcess.ProcessId) -Force -ErrorAction Stop
  }

  $stopDeadline = (Get-Date).AddSeconds(15)
  while (
    (Get-NetTCPConnection -State Listen -LocalPort 18082 -ErrorAction SilentlyContinue) -and
    (Get-Date) -lt $stopDeadline
  ) {
    Start-Sleep -Milliseconds 200
  }
  if (Get-NetTCPConnection -State Listen -LocalPort 18082 -ErrorAction SilentlyContinue) {
    throw "Le serveur Switch developpement ne s'est pas arrete dans le delai prevu."
  }

  $drainArmed = $false
  & $launcher -DeploymentRoot $deploymentRoot
  $health = Get-DevelopmentHealth
  if ($null -eq $health -or $health.ok -ne $true) {
    throw "Switch developpement n'est pas revenu apres le redemarrage."
  }
  Write-Output "Switch developpement redemarre sans interrompre de session active."
} finally {
  if ($drainArmed) {
    try { Set-DevelopmentDrain -Draining $false } catch { }
  }
  if ($restartLockHeld) {
    $restartMutex.ReleaseMutex()
  }
  $restartMutex.Dispose()
}
