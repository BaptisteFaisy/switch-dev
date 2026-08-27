param(
  [string]$NodeId = "pc-fixe",
  [string]$NodeLabel = "PC fixe",
  [int]$Capacity = [Math]::Max(1, [Environment]::ProcessorCount - 2),
  [int]$Port = 18080,
  [string]$OracleUrl = "",
  [string]$TaskName = "Codex Switch Terminal Node",
  [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$Root = Split-Path -Parent $ScriptDir
$DataDir = Join-Path $env:APPDATA "codex-switch-terminal-server"
$EnvFile = Join-Path $DataDir "server.local.env.ps1"
$StartScript = Join-Path $ScriptDir "start-local-server.ps1"
$StopScript = Join-Path $ScriptDir "stop-local-server.ps1"
$Tailscale = "C:\Program Files\Tailscale\tailscale.exe"

function Quote-PowerShellLiteral {
  param([AllowEmptyString()][string]$Value)
  return "'" + $Value.Replace("'", "''") + "'"
}

if (-not (Test-Path $Tailscale)) {
  throw "Tailscale est introuvable. Installe-le puis connecte ce PC au tailnet."
}

$status = (& $Tailscale status --json | ConvertFrom-Json)
if ($status.BackendState -ne "Running") {
  $message = "Tailscale n'est pas connecte. Lance: tailscale up"
  if ($status.AuthURL) {
    $message += "`nAutorisation: $($status.AuthURL)"
  }
  throw $message
}

$dnsName = ([string]$status.Self.DNSName).Trim().TrimEnd(".")
if (-not $dnsName) {
  throw "MagicDNS ne fournit pas de nom pour ce PC."
}
$publicUrl = "https://$dnsName"

New-Item -ItemType Directory -Force -Path $DataDir | Out-Null
if (Test-Path $EnvFile) {
  . $EnvFile
}
if (-not $env:CST_ADMIN_TOKEN) {
  $chars = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789".ToCharArray()
  $env:CST_ADMIN_TOKEN = -join (1..64 | ForEach-Object { $chars | Get-Random })
}
if ($null -eq $env:CST_GIT_PAT) {
  $env:CST_GIT_PAT = ""
}
if (-not $env:CST_CODEX_WINDOWS_SANDBOX) {
  $env:CST_CODEX_WINDOWS_SANDBOX = "unelevated"
}

$allowedOrigins = @($publicUrl)
if ($OracleUrl.Trim()) {
  $allowedOrigins += $OracleUrl.Trim().TrimEnd("/")
}

@(
  '$env:CST_ADMIN_TOKEN = ' + (Quote-PowerShellLiteral $env:CST_ADMIN_TOKEN)
  '$env:CST_GIT_PAT = ' + (Quote-PowerShellLiteral $env:CST_GIT_PAT)
  '$env:CST_CODEX_WINDOWS_SANDBOX = ' + (Quote-PowerShellLiteral $env:CST_CODEX_WINDOWS_SANDBOX)
  '$env:CST_BIND = ' + (Quote-PowerShellLiteral "127.0.0.1:$Port")
  '$env:CST_PUBLIC_BASE_URL = ' + (Quote-PowerShellLiteral $publicUrl)
  '$env:CST_ALLOWED_ORIGINS = ' + (Quote-PowerShellLiteral ($allowedOrigins -join ","))
  '$env:CST_NODE_ID = ' + (Quote-PowerShellLiteral $NodeId)
  '$env:CST_NODE_LABEL = ' + (Quote-PowerShellLiteral $NodeLabel)
  '$env:CST_NODE_CAPACITY = ' + (Quote-PowerShellLiteral ([string]$Capacity))
) | Set-Content -Path $EnvFile -Encoding UTF8

if (-not $SkipBuild) {
  & $StopScript | Out-Null
  Push-Location $Root
  try {
    npm run build:frontend
    if ($LASTEXITCODE -ne 0) { throw "Le build frontend a echoue." }
    npm run build:server
    if ($LASTEXITCODE -ne 0) { throw "Le build serveur a echoue." }
  }
  finally {
    Pop-Location
  }
}

Write-Host "Configuration HTTPS Tailscale Serve..." -ForegroundColor Cyan
& $Tailscale serve --bg --yes "http://127.0.0.1:$Port"
if ($LASTEXITCODE -ne 0) {
  throw "Tailscale Serve n'a pas pu etre configure."
}

$userId = [Security.Principal.WindowsIdentity]::GetCurrent().Name
# Le setup peut etre lance depuis pwsh.exe (runtime Codex), dont PSHOME ne
# contient pas powershell.exe. La tache persistante utilise Windows PowerShell.
$powershell = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"
if (-not (Test-Path -LiteralPath $powershell -PathType Leaf)) {
  throw "PowerShell Windows introuvable: $powershell"
}
$arguments = "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$StartScript`""
$action = New-ScheduledTaskAction -Execute $powershell -Argument $arguments
$logonTrigger = New-ScheduledTaskTrigger -AtLogOn -User $userId
# Le declencheur periodique joue le role de watchdog : MultipleInstances=IgnoreNew
# laisse le noeud actif tranquille et le relance sous cinq minutes s'il s'arrete.
$watchdogTrigger = New-ScheduledTaskTrigger `
  -Once `
  -At (Get-Date).AddMinutes(5) `
  -RepetitionInterval (New-TimeSpan -Minutes 5)
$trigger = @($logonTrigger, $watchdogTrigger)
$settings = New-ScheduledTaskSettingsSet `
  -AllowStartIfOnBatteries `
  -DontStopIfGoingOnBatteries `
  -StartWhenAvailable `
  -RestartCount 999 `
  -RestartInterval (New-TimeSpan -Minutes 1) `
  -ExecutionTimeLimit ([TimeSpan]::Zero)
$principal = New-ScheduledTaskPrincipal -UserId $userId -LogonType Interactive -RunLevel Limited

Register-ScheduledTask `
  -TaskName $TaskName `
  -Action $action `
  -Trigger $trigger `
  -Settings $settings `
  -Principal $principal `
  -Description "Worker Codex Switch Terminal prive via Tailscale" `
  -Force | Out-Null

Start-ScheduledTask -TaskName $TaskName

$headers = @{ Authorization = "Bearer $env:CST_ADMIN_TOKEN" }
$health = $null
for ($attempt = 0; $attempt -lt 30; $attempt++) {
  Start-Sleep -Seconds 1
  try {
    $health = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/health" -Headers $headers -TimeoutSec 2
    break
  }
  catch {
    if ($attempt -eq 29) { throw }
  }
}

Write-Host "" 
Write-Host "Noeud PC operationnel." -ForegroundColor Green
Write-Host "URL privee : $publicUrl"
Write-Host "Noeud       : $($health.nodeLabel)"
Write-Host "Capacite    : $($health.capacity)"
Write-Host "Tache       : $TaskName"
Write-Host "Config app  : $NodeLabel|$publicUrl||0"
