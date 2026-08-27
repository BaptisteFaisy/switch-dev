# Serveur Switch local/SSD = developpement uniquement.
# La version stable/production est hebergee sur le VPS Microsoft Azure.
param(
  [switch]$Build
)

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$Root = Split-Path -Parent $ScriptDir
$DataDir = Join-Path $env:APPDATA "codex-switch-terminal-server"
$EnvFile = Join-Path $DataDir "server.local.env.ps1"
$StaticDir = Join-Path $Root "dist"
$ServerExe = Join-Path $Root "src-tauri\target\release\cst-server.exe"

function Assert-CheckoutIsNotStale {
  # Un autre checkout peut avancer refs/remotes/origin/main meme si ce dossier
  # n'a pas encore ete synchronise.
  # Refuser un ancetre connu evite qu'un simple restart republie une vieille UI.
  if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
    return
  }

  $head = [string](& git -C $Root rev-parse --verify HEAD 2>$null)
  $originMain = [string](& git -C $Root rev-parse --verify refs/remotes/origin/main 2>$null)
  $head = $head.Trim()
  $originMain = $originMain.Trim()
  if (-not $head -or -not $originMain -or $head -eq $originMain) {
    return
  }

  & git -C $Root merge-base --is-ancestor $head $originMain 2>$null
  if ($LASTEXITCODE -eq 0) {
    $shortHead = $head.Substring(0, [Math]::Min(8, $head.Length))
    $shortOrigin = $originMain.Substring(0, [Math]::Min(8, $originMain.Length))
    throw @"
Demarrage refuse : le checkout local $shortHead est en retard sur origin/main $shortOrigin.
Ce script n'a ni arrete ni modifie le serveur existant.
Preserve d'abord tes changements locaux, puis synchronise avec :
  git pull --ff-only origin main
"@
  }
}

function Test-TreeNewerThan {
  param(
    [string[]]$Paths,
    [datetime]$Timestamp
  )

  foreach ($path in $Paths) {
    if (-not (Test-Path $path)) {
      continue
    }

    $item = Get-Item $path
    if (-not $item.PSIsContainer) {
      if ($item.LastWriteTimeUtc -gt $Timestamp.ToUniversalTime()) {
        return $true
      }
      continue
    }

    $newer = Get-ChildItem -Path $path -Recurse -File |
      Where-Object { $_.LastWriteTimeUtc -gt $Timestamp.ToUniversalTime() } |
      Select-Object -First 1
    if ($newer) {
      return $true
    }
  }

  return $false
}

function Test-NeedsBuild {
  param(
    [string]$Output,
    [string[]]$Inputs
  )

  if (-not (Test-Path $Output)) {
    return $true
  }

  $outputTime = (Get-Item $Output).LastWriteTimeUtc
  return Test-TreeNewerThan -Paths $Inputs -Timestamp $outputTime
}

Assert-CheckoutIsNotStale

New-Item -ItemType Directory -Force -Path $DataDir | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $DataDir "codex-homes") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $DataDir "workspaces") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $DataDir "logs") | Out-Null

if (-not (Test-Path $EnvFile)) {
  $chars = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789".ToCharArray()
  $token = -join (1..56 | ForEach-Object { $chars | Get-Random })
  @"
`$env:CST_ADMIN_TOKEN = "$token"
`$env:CST_GIT_PAT = ""
"@ | Set-Content -Path $EnvFile -Encoding UTF8
}

. $EnvFile

$ip = Get-NetIPAddress -AddressFamily IPv4 -ErrorAction SilentlyContinue |
  Where-Object {
    $_.IPAddress -ne "127.0.0.1" -and
    $_.IPAddress -notlike "169.254.*" -and
    $_.AddressState -eq "Preferred"
  } |
  Sort-Object InterfaceMetric |
  Select-Object -First 1 -ExpandProperty IPAddress

if (-not $ip) {
  $ip = "127.0.0.1"
}

# Loopback par defaut : un bind sur 0.0.0.0 declenche la fenetre "Pare-feu
# Windows" (admin) a chaque demarrage. Pour rendre l'interface accessible depuis
# un autre appareil du reseau (telephone/tablette), definir explicitement
# CST_BIND=0.0.0.0:18080 (la fenetre pare-feu n'apparait alors qu'une seule fois,
# ou lancer scripts/allow-local-server-firewall.ps1 en admin une bonne fois).
$env:CST_BIND = if ($env:CST_BIND) { $env:CST_BIND } else { "127.0.0.1:18080" }
$bindHost = ($env:CST_BIND -split ":")[0]
$bindPort = ($env:CST_BIND -split ":")[-1]
if (-not $bindPort) {
  $bindPort = "18080"
}
# Expose sur le LAN uniquement si le bind ecoute sur toutes les interfaces.
$isLanBind = ($bindHost -eq "0.0.0.0" -or $bindHost -eq "::")
$displayHost = if ($isLanBind) { $ip } else { "127.0.0.1" }
$env:CST_DATA_DIR = $DataDir
$env:CST_STATIC_DIR = $StaticDir
if (-not $env:CST_CODEX_WINDOWS_SANDBOX) {
  $env:CST_CODEX_WINDOWS_SANDBOX = "unelevated"
}
if (-not $env:CST_PUBLIC_BASE_URL) {
  $env:CST_PUBLIC_BASE_URL = "http://${displayHost}:$bindPort"
}
if (-not $env:CST_ALLOWED_ORIGINS) {
  $env:CST_ALLOWED_ORIGINS = $env:CST_PUBLIC_BASE_URL
}
$env:CST_NODE_ID = if ($env:CST_NODE_ID) { $env:CST_NODE_ID } else { "pc-local" }
$env:CST_NODE_LABEL = if ($env:CST_NODE_LABEL) { $env:CST_NODE_LABEL } else { "PC local" }
$env:CST_NODE_CAPACITY = if ($env:CST_NODE_CAPACITY) { $env:CST_NODE_CAPACITY } else { [Math]::Max(1, [Environment]::ProcessorCount - 1).ToString() }

Push-Location $Root
try {
  $frontendOutput = Join-Path $StaticDir "index.html"
  $frontendInputs = @(
    (Join-Path $Root "src"),
    (Join-Path $Root "index.html"),
    (Join-Path $Root "package.json"),
    (Join-Path $Root "package-lock.json"),
    (Join-Path $Root "tsconfig.json"),
    (Join-Path $Root "vite.config.ts")
  )
  if ($Build -or (Test-NeedsBuild -Output $frontendOutput -Inputs $frontendInputs)) {
    npm run build:frontend
  }

  $serverInputs = @(
    (Join-Path $Root "src-tauri\src"),
    (Join-Path $Root "src-tauri\Cargo.toml"),
    (Join-Path $Root "src-tauri\Cargo.lock")
  )
  if ($Build -or (Test-NeedsBuild -Output $ServerExe -Inputs $serverInputs)) {
    npm run build:server
  }

  Write-Host ""
  Write-Host "Codex Switch Terminal - serveur local PC" -ForegroundColor Cyan
  Write-Host "Interface web : $env:CST_PUBLIC_BASE_URL"
  Write-Host "Token admin   : $env:CST_ADMIN_TOKEN"
  Write-Host "Donnees       : $env:CST_DATA_DIR"
  Write-Host "Noeud         : $env:CST_NODE_LABEL (capacite $env:CST_NODE_CAPACITY)"
  Write-Host "Config token  : $EnvFile"
  Write-Host ""
  if ($isLanBind) {
    Write-Host "Depuis un autre appareil du meme reseau, ouvre l'URL ci-dessus." -ForegroundColor Yellow
    Write-Host "Si ca bloque, lance scripts/allow-local-server-firewall.ps1 en admin." -ForegroundColor Yellow
  } else {
    Write-Host "Serveur en local uniquement (127.0.0.1) : aucune fenetre pare-feu." -ForegroundColor Green
    Write-Host "Pour l'ouvrir aux autres appareils du reseau, relance avec :" -ForegroundColor Yellow
    Write-Host '  $env:CST_BIND = "0.0.0.0:18080"' -ForegroundColor Yellow
  }
  Write-Host ""

  & $ServerExe
}
finally {
  Pop-Location
}
