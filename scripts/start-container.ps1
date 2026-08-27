[CmdletBinding()]
param(
  # Racine du depot (par defaut : parent du dossier scripts/).
  [string]$RepoRoot = "",
  # Compte Windows expose en SSH au conteneur (par defaut : utilisateur courant).
  [string]$WindowsUser = "",
  # Port publie par compose pour le sshd du conteneur (poste -> conteneur).
  # 2224 : 22 = sshd WSL, 2222 = serveur OpenSSH Windows, 2223 = sshd Duello (WSL).
  [ValidateRange(1, 65535)]
  [int]$ContainerSshPort = 2224,
  # Port du serveur SSH Windows vers lequel se connecte le conteneur.
  [ValidateRange(1, 65535)]
  [int]$WindowsSshPort = 2222,
  # Port HTTP publie par compose (sonde de sante du launcher).
  [ValidateRange(1, 65535)]
  [int]$HostPort = 18080,
  # Ne pas reconstruire l'image : `docker compose up -d` seulement.
  [switch]$SkipBuild
)

# ---------------------------------------------------------------------------
# Lanceur UNIQUE du conteneur local. Il enchaîne, sans etape manuelle :
#   1. l'echange de cles SSH bidirectionnel (setup-container-ssh.ps1, idempotent),
#   2. le demarrage du serveur OpenSSH Windows si necessaire,
#   3. `docker compose up -d` avec CST_DATA_PATH pointe sur .cst-data (le
#      bind-mount ou vivent deja les cles), puis la sonde de sante.
# Le pont SSH (poste -> conteneur via sshd, conteneur -> poste via `ssh local`)
# est alors actif a chaque lancement, cles incluses.
# ---------------------------------------------------------------------------

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
if (-not $RepoRoot.Trim()) { $RepoRoot = Split-Path -Parent $ScriptDir }
if (-not $WindowsUser.Trim()) { $WindowsUser = $env:USERNAME }

$SetupScript = Join-Path $ScriptDir "setup-container-ssh.ps1"

# 1) Echange de cles (idempotent). Genere la paire id_back dans .cst-data/ssh,
#    enregistre la cle publique du poste dans le authorized_keys du conteneur et
#    la cle de retour dans celui du poste.
& $SetupScript `
  -RepoRoot $RepoRoot `
  -WindowsUser $WindowsUser `
  -ContainerSshPort $ContainerSshPort `
  -WindowsSshPort $WindowsSshPort

# 2) Serveur OpenSSH Windows : l'ouvrir automatiquement (demarrage + mode
#    automatique) pour que le pont conteneur -> poste soit toujours joignable.
#    Demarrer un service exige les droits administrateur : sans elevation on
#    relance un helper en UAC uniquement quand c'est necessaire.
$sshd = Get-Service sshd -ErrorAction SilentlyContinue
if (-not $sshd) {
  Write-Warning "OpenSSH Server Windows n'est pas installe:"
  Write-Warning "  Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0"
  Write-Warning "Le pont conteneur -> poste restera indisponible tant qu'il n'est pas installe."
}
else {
  $isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
  $helper = Join-Path $ScriptDir "ensure-windows-sshd.ps1"
  $needAdmin = ($sshd.Status -ne "Running") -or ($sshd.StartType -ne "Automatic")

  if ($needAdmin -and -not $isAdmin) {
    Write-Host "sshd Windows arrete ou non automatique : demande d'elevation (UAC)..."
    $ps = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"
    try {
      Start-Process -FilePath $ps -Verb RunAs -Wait `
        -ArgumentList "-NoProfile -ExecutionPolicy Bypass -File `"$helper`" -WindowsSshPort $WindowsSshPort"
    }
    catch {
      Write-Warning "Elevation annulee ou impossible : $($_.Exception.Message)"
    }
  }
  elseif ($needAdmin) {
    & $helper -WindowsSshPort $WindowsSshPort
  }

  $sshd.Refresh()
  if ($sshd.Status -eq "Running") {
    Write-Host "sshd Windows actif sur le port $WindowsSshPort - OK."
  }
  else {
    Write-Warning "sshd Windows indisponible : le pont conteneur -> poste restera muet."
  }
}

# 3) Lancement du conteneur. Les cles SSH sont deja dans .cst-data/ssh (ecrites
#    par setup-container-ssh.ps1) et compose les bind-monte sur /srv/cst/ssh ;
#    les donnees de sessions restent, elles, dans le volume nomme cst-data.
$env:CST_SSH_LOCAL_USER = $WindowsUser
$env:CST_SSH_LOCAL_PORT = [string]$WindowsSshPort
$env:CST_SSH_PORT = [string]$ContainerSshPort

Push-Location $RepoRoot
try {
  if ($SkipBuild) {
    Write-Host "Demarrage du conteneur (sans rebuild)..."
    docker compose up -d
  }
  else {
    Write-Host "Build puis demarrage du conteneur..."
    docker compose up -d --build
  }
  if ($LASTEXITCODE -ne 0) {
    throw "docker compose up a echoue (code $LASTEXITCODE)."
  }
}
finally {
  Pop-Location
}

# 4) Sonde de sante HTTP, en laissant la main des que le service repond.
$healthy = $false
for ($attempt = 0; $attempt -lt 60; $attempt++) {
  try {
    & curl.exe -fsS "http://127.0.0.1:$HostPort/healthz" | Out-Null
    if ($LASTEXITCODE -eq 0) { $healthy = $true; break }
  }
  catch { }
  Start-Sleep -Seconds 2
}

Write-Host ""
if ($healthy) {
  Write-Host "Conteneur sain sur http://127.0.0.1:$HostPort - OK." -ForegroundColor Green
}
else {
  Write-Warning "Le conteneur demarre mais la sonde /healthz n'a pas repondu. Consulte: docker compose logs"
}
Write-Host ""
Write-Host "Pont SSH bidirectionnel actif :"
Write-Host "  poste -> conteneur : ssh -p $ContainerSshPort cst@127.0.0.1"
Write-Host "  conteneur -> poste : ssh local   (dans le conteneur, compte $WindowsUser)"
