[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [string]$DeploymentRoot
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$deploymentRoot = [IO.Path]::GetFullPath($DeploymentRoot)
$configPath = Join-Path $deploymentRoot "config\server.local.env.ps1"
$sourceServerPath = Join-Path $deploymentRoot "app\cst-server.exe"
$sourceStaticPath = Join-Path $deploymentRoot "app\dist"
$releaseGate = Join-Path $deploymentRoot "app\scripts\switch-development-release-gate.ps1"
$distInventoryModule = Join-Path $deploymentRoot "app\scripts\switch-development-dist-inventory.psm1"
$dataPath = Join-Path $deploymentRoot "data"
$deploymentRunPath = Join-Path $deploymentRoot "run"
$launchMutex = [Threading.Mutex]::new($false, "Local\SwitchDevelopmentLaunch-18082")
$launchLockHeld = $false

try {
  try {
    $launchLockHeld = $launchMutex.WaitOne([TimeSpan]::FromSeconds(30))
  } catch [Threading.AbandonedMutexException] {
    $launchLockHeld = $true
  }
  if (-not $launchLockHeld) {
    throw "Un autre lancement Switch developpement est deja en cours."
  }

foreach ($requiredPath in @(
  $configPath,
  $releaseGate,
  $distInventoryModule,
  $sourceServerPath,
  (Join-Path $sourceStaticPath "index.html")
)) {
  if (-not (Test-Path -LiteralPath $requiredPath -PathType Leaf)) {
    throw "Deploiement Switch developpement incomplet : $requiredPath est absent. Aucun backup ancien ne sera charge."
  }
}

Import-Module -Name $distInventoryModule -Force -Scope Local

. $configPath
if (-not $env:CST_ADMIN_TOKEN) {
  throw "CST_ADMIN_TOKEN est absent de la configuration locale."
}
if (-not $env:LOCALAPPDATA) {
  throw "LOCALAPPDATA est indisponible : le runtime interne ne peut pas etre prepare."
}

# Le Samsung T7 a deja subi des reprises d'E/S Windows (event 153), capables de
# tuer un executable mappe depuis E:. Le code et les fichiers statiques tournent
# donc depuis le disque interne. Les donnees et workspaces restent ceux de la
# version de developpement sur E:, sans aucune copie vers la production.
$runtimeRoot = Join-Path $env:LOCALAPPDATA "SwitchDevelopmentRuntime"
$runtimeReleasesPath = Join-Path $runtimeRoot "releases"
$runtimeLogPath = Join-Path $runtimeRoot "logs"
$runtimeRunPath = Join-Path $runtimeRoot "run"
New-Item -ItemType Directory -Path @(
  $runtimeReleasesPath,
  $runtimeLogPath,
  $runtimeRunPath,
  $deploymentRunPath
) -Force | Out-Null

# Verrou partage de deploiement : couvre la fenetre ou ce launcher lit le
# paquet (binaire + dist) depuis app\ et le copie dans le cache interne. Un
# autre agent est alors susceptible de remplacer son paquet dans app\ ; ce
# verrou empeche de demarrer un ensemble executable/frontend incoherent.
# Le redemarrage (restart-switch-development.ps1) tient ce meme mutex pendant
# tout son cycle, et l'arret du serveur se fait sous la protection du port 18082
# deja verifiee plus haut, donc il n'y a ni interblocage ni course sur app\.
$deployMutex = [Threading.Mutex]::new($false, "Local\SwitchDevelopmentDeploy-18082")
$deployLockHeld = $false
try {
  try {
    $deployLockHeld = $deployMutex.WaitOne([TimeSpan]::FromSeconds(60))
  } catch [Threading.AbandonedMutexException] {
    $deployLockHeld = $true
  }
  if (-not $deployLockHeld) {
    throw "Un autre deploiement Switch developpement est en cours ; demarrage annule, aucun paquet incoherent servi."
  }

# Verrou monotone : le paquet app\ doit etre soit la release acceptee, soit un
# candidat manifeste descendant de celle-ci. Le repli silencieux vers app.bak-*
# est volontairement interdit.
& $releaseGate -Operation AuditPackage -DeploymentRoot $deploymentRoot -CandidateRoot (Join-Path $deploymentRoot "app")

$sourceServerHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $sourceServerPath).Hash
$sourceDistInventory = Get-SwitchDevelopmentDistInventory -DistRoot $sourceStaticPath
$releaseId = $sourceServerHash.Substring(0, 16) + "-" + $sourceDistInventory.treeSha256.Substring(0, 16)
$runtimeReleasePath = Join-Path $runtimeReleasesPath $releaseId
$runtimeServerPath = Join-Path $runtimeReleasePath "cst-server.exe"
$runtimeStaticPath = Join-Path $runtimeReleasePath "dist"
$runtimeIndexPath = Join-Path $runtimeStaticPath "index.html"

if (
  -not (Test-Path -LiteralPath $runtimeServerPath -PathType Leaf) -or
  -not (Test-Path -LiteralPath $runtimeIndexPath -PathType Leaf)
) {
  $stagePath = Join-Path $runtimeReleasesPath ("stage-" + [Guid]::NewGuid().ToString("N"))
  New-Item -ItemType Directory -Path $stagePath -Force | Out-Null
  Copy-Item -LiteralPath $sourceServerPath -Destination (Join-Path $stagePath "cst-server.exe")
  Copy-Item -LiteralPath $sourceStaticPath -Destination $stagePath -Recurse

  $stagedServerPath = Join-Path $stagePath "cst-server.exe"
  $stagedServerHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $stagedServerPath).Hash
  $stagedDistInventory = Get-SwitchDevelopmentDistInventory -DistRoot (Join-Path $stagePath "dist")
  $sourceServerHashAfterCopy = (Get-FileHash -Algorithm SHA256 -LiteralPath $sourceServerPath).Hash
  $sourceDistInventoryAfterCopy = Get-SwitchDevelopmentDistInventory -DistRoot $sourceStaticPath

  if (
    $stagedServerHash -ne $sourceServerHash -or
    $sourceServerHashAfterCopy -ne $sourceServerHash -or
    -not (Test-SwitchDevelopmentDistInventoryEqual -Left $sourceDistInventory -Right $stagedDistInventory) -or
    -not (Test-SwitchDevelopmentDistInventoryEqual -Left $sourceDistInventory -Right $sourceDistInventoryAfterCopy)
  ) {
    throw "Le paquet Switch a change pendant sa mise en cache ; demarrage annule sans toucher au serveur existant."
  }

  if (-not (Test-Path -LiteralPath $runtimeReleasePath)) {
    Move-Item -LiteralPath $stagePath -Destination $runtimeReleasePath
  }
  $runtimeServerPath = Join-Path $runtimeReleasePath "cst-server.exe"
  $runtimeStaticPath = Join-Path $runtimeReleasePath "dist"
}

$runtimeIndexPath = Join-Path $runtimeStaticPath "index.html"
foreach ($runtimeRequiredPath in @($runtimeServerPath, $runtimeIndexPath)) {
  if (-not (Test-Path -LiteralPath $runtimeRequiredPath -PathType Leaf)) {
    throw "Runtime Switch developpement incomplet : $runtimeRequiredPath est absent."
  }
}
if (
  (Get-FileHash -Algorithm SHA256 -LiteralPath $runtimeServerPath).Hash -ne $sourceServerHash -or
  -not (Test-SwitchDevelopmentDistInventoryEqual `
    -Left $sourceDistInventory `
    -Right (Get-SwitchDevelopmentDistInventory -DistRoot $runtimeStaticPath))
) {
  throw "Runtime Switch developpement incoherent ; demarrage annule."
}
& $releaseGate -Operation AuditPackage -DeploymentRoot $deploymentRoot -CandidateRoot $runtimeReleasePath

$env:CST_BIND = "127.0.0.1:18082"
$env:CST_DATA_DIR = $dataPath
$env:CST_STATIC_DIR = $runtimeStaticPath
$env:CST_PUBLIC_BASE_URL = "https://pc-fixe-cst.tail3a8bdf.ts.net:10000"
$env:CST_ALLOWED_ORIGINS = $env:CST_PUBLIC_BASE_URL
$env:CST_DEVICE_EMBEDDED_CONNECTOR = "1"

$listener = Get-NetTCPConnection -State Listen -LocalPort 18082 -ErrorAction SilentlyContinue |
  Select-Object -First 1
if ($listener) {
  try {
    $health = Invoke-WebRequest -UseBasicParsing -Uri "http://127.0.0.1:18082/healthz" -TimeoutSec 3
    if ($health.StatusCode -eq 200) {
      & $releaseGate -Operation AuditActive -DeploymentRoot $deploymentRoot
      Write-Output "Switch developpement est deja disponible sur 127.0.0.1:18082."
      return
    }
  } catch {
    # Le port est occupe sans reponse Switch saine : ne jamais tuer aveuglement
    # le processus d'un autre chantier.
  }
  throw "Le port 18082 est deja occupe par un autre processus."
}

$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$stdoutPath = Join-Path $runtimeLogPath "cst-server-$stamp.stdout.log"
$stderrPath = Join-Path $runtimeLogPath "cst-server-$stamp.stderr.log"
$process = Start-Process -FilePath $runtimeServerPath `
  -WorkingDirectory $runtimeReleasePath `
  -WindowStyle Hidden `
  -RedirectStandardOutput $stdoutPath `
  -RedirectStandardError $stderrPath `
  -PassThru
$process.Id | Set-Content -LiteralPath (Join-Path $runtimeRunPath "cst-server.pid") -Encoding ascii
$process.Id | Set-Content -LiteralPath (Join-Path $deploymentRunPath "cst-server.pid") -Encoding ascii

for ($attempt = 0; $attempt -lt 40; $attempt += 1) {
  if ($process.HasExited) {
    $errorTail = if (Test-Path -LiteralPath $stderrPath) {
      (Get-Content -LiteralPath $stderrPath -Tail 20) -join "`n"
    } else {
      "aucun journal d'erreur"
    }
    throw "Switch developpement s'est arrete au demarrage : $errorTail"
  }
  try {
    $health = Invoke-WebRequest -UseBasicParsing -Uri "http://127.0.0.1:18082/healthz" -TimeoutSec 2
    if ($health.StatusCode -eq 200) {
      & $releaseGate -Operation RecordRuntime -DeploymentRoot $deploymentRoot -RuntimeReleasePath $runtimeReleasePath
      Write-Output "Switch developpement demarre (PID $($process.Id)) depuis le runtime interne $runtimeReleasePath."
      return
    }
  } catch {
    Start-Sleep -Milliseconds 250
  }
}

throw "Switch developpement n'a pas repondu a /healthz dans le delai prevu."
} finally {
  if ($deployLockHeld) {
    $deployMutex.ReleaseMutex()
  }
  $deployMutex.Dispose()
}
} finally {
  if ($launchLockHeld) {
    $launchMutex.ReleaseMutex()
  }
  $launchMutex.Dispose()
}
