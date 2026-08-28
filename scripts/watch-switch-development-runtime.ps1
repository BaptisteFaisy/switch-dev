[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [string]$DeploymentRoot
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$deploymentRoot = [IO.Path]::GetFullPath($DeploymentRoot)
$launcher = Join-Path $deploymentRoot "app\scripts\start-switch-development-runtime.ps1"
if (-not $env:LOCALAPPDATA) {
  throw "LOCALAPPDATA est indisponible : le watchdog ne peut pas journaliser son etat."
}
$watchLogPath = Join-Path $env:LOCALAPPDATA "SwitchDevelopmentRuntime\logs\watchdog.log"
New-Item -ItemType Directory -Path (Split-Path -Parent $watchLogPath) -Force | Out-Null

function Write-WatchEvent {
  param([string]$Message)
  $line = "{0:O} {1}" -f (Get-Date), $Message
  Add-Content -LiteralPath $watchLogPath -Value $line -Encoding UTF8
}

while ($true) {
  try {
    & $launcher -DeploymentRoot $deploymentRoot
  } catch {
    Write-WatchEvent ("demarrage impossible: " + $_.Exception.Message)
    Start-Sleep -Seconds 10
    continue
  }

  $listener = Get-NetTCPConnection `
    -State Listen `
    -LocalPort 18082 `
    -ErrorAction SilentlyContinue |
    Select-Object -First 1

  if ($null -eq $listener) {
    Write-WatchEvent "serveur absent apres le lancement; nouvelle tentative"
    Start-Sleep -Seconds 3
    continue
  }

  $serverPid = [int]$listener.OwningProcess
  Write-WatchEvent "surveillance du serveur PID $serverPid"
  try {
    Wait-Process -Id $serverPid -ErrorAction Stop
  } catch {
    # Le processus peut disparaitre entre la decouverte et Wait-Process.
  }
  Write-WatchEvent "serveur PID $serverPid termine; relance dans 3 s"
  Start-Sleep -Seconds 3
}
