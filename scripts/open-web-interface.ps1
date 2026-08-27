param(
  [switch]$NoBrowser
)

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$Root = Split-Path -Parent $ScriptDir
$DataDir = Join-Path $env:APPDATA "codex-switch-terminal-server"
$EnvFile = Join-Path $DataDir "server.local.env.ps1"
$PidFile = Join-Path $DataDir "server.pid"
$ServerScript = Join-Path $ScriptDir "start-local-server.ps1"
$Port = 18080
$LocalUrl = "http://127.0.0.1:$Port"

function Get-LanIp {
  $ip = Get-NetIPAddress -AddressFamily IPv4 -ErrorAction SilentlyContinue |
    Where-Object {
      $_.IPAddress -ne "127.0.0.1" -and
      $_.IPAddress -notlike "169.254.*" -and
      $_.AddressState -eq "Preferred"
    } |
    Sort-Object InterfaceMetric |
    Select-Object -First 1 -ExpandProperty IPAddress

  if ($ip) { return $ip }
  return "127.0.0.1"
}

function Test-ServerReady {
  try {
    $response = Invoke-WebRequest -UseBasicParsing -Uri "$LocalUrl/healthz" -TimeoutSec 2
    return $response.StatusCode -eq 200
  } catch {
    return $false
  }
}

function Get-LocalToken {
  if (-not (Test-Path $EnvFile)) {
    return $null
  }

  $tokenLine = Get-Content $EnvFile |
    Where-Object { $_ -match "CST_ADMIN_TOKEN" } |
    Select-Object -First 1
  if (-not $tokenLine) {
    return $null
  }

  $match = [regex]::Match($tokenLine, '["'']([^"'']+)["'']')
  if (-not $match.Success) {
    return $null
  }

  return $match.Groups[1].Value
}

New-Item -ItemType Directory -Force -Path $DataDir | Out-Null

if (-not (Test-ServerReady)) {
  $process = Start-Process powershell `
    -ArgumentList @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $ServerScript) `
    -WindowStyle Minimized `
    -PassThru
  $process.Id | Set-Content -Path $PidFile -Encoding ASCII

  $ready = $false
  for ($i = 0; $i -lt 60; $i++) {
    Start-Sleep -Milliseconds 500
    if (Test-ServerReady) {
      $ready = $true
      break
    }
  }

  if (-not $ready) {
    throw "Le serveur local n'a pas repondu sur $LocalUrl."
  }
}

$token = Get-LocalToken
if ($token) {
  Set-Clipboard -Value $token
}

$lanUrl = "http://$(Get-LanIp):$Port"
if (-not $NoBrowser) {
  Start-Process $LocalUrl
}

Write-Host ""
Write-Host "Site web Codex Switch Terminal ouvert." -ForegroundColor Cyan
Write-Host "URL locale : $LocalUrl"
Write-Host "URL reseau : $lanUrl"
if ($token) {
  Write-Host "Token admin copie dans le presse-papiers." -ForegroundColor Green
} else {
  Write-Host "Token introuvable. Regarde $EnvFile." -ForegroundColor Yellow
}
Write-Host ""
Write-Host 'Pour arreter le serveur: & ".\Lancer Codex Switch Terminal.cmd" stop'
