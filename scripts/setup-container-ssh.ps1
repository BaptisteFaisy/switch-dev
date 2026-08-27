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
  # 2222 (le port 22 de l'hote est occupe par le sshd WSL, wslrelay.exe).
  [ValidateRange(1, 65535)]
  [int]$WindowsSshPort = 2222
)

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
if (-not $RepoRoot.Trim()) { $RepoRoot = Split-Path -Parent $ScriptDir }
if (-not $WindowsUser.Trim()) { $WindowsUser = $env:USERNAME }

$SshDir = Join-Path $env:USERPROFILE ".ssh"
$WindowsAuthorizedKeys = Join-Path $SshDir "authorized_keys"
$WindowsPublicKey = Join-Path $SshDir "id_ed25519.pub"
# Cle de retour du conteneur : la partie privee reste dans le volume persistant
# .cst-data/ssh (monte sur /srv/cst/ssh), la partie publique va sur le poste.
$DataDir = Join-Path $RepoRoot ".cst-data"
$ContainerSshDir = Join-Path $DataDir "ssh"
$ContainerAuthorizedKeys = Join-Path $ContainerSshDir "authorized_keys"
$ContainerBackKey = Join-Path $ContainerSshDir "id_back"

# Les cles sont de l'ASCII pur : un BOM UTF-8 casserait le premier champ lu par
# sshd. Le contenu est toujours reecrit en ASCII.

# PowerShell 5.1 avale les arguments vides en invoquant une commande native :
# `ssh-keygen -N ""` devient « ssh-keygen: Too many arguments ». On passe par
# cmd /c avec une ligne de commande construite a la main : `""` y traverse
# intact et ssh-keygen recoit bien une passphrase vide.
function Invoke-SshKeygen {
  param([Parameter(Mandatory = $true)][string]$KeyFile)
  $quotedFile = '"' + $KeyFile.Replace('"', '\"') + '"'
  cmd /d /c "ssh-keygen -q -t ed25519 -N \"\" -f $quotedFile"
  if ($LASTEXITCODE -ne 0) {
    throw "ssh-keygen a echoue (code $LASTEXITCODE) pour $KeyFile."
  }
}

function Add-KeyLine {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$PublicKey,
    [Parameter(Mandatory = $true)][string]$Detail
  )
  $existing = @()
  if (Test-Path -LiteralPath $Path) {
    $existing = @(Get-Content -LiteralPath $Path -ErrorAction SilentlyContinue)
  }
  if ($existing -contains $PublicKey) {
    Write-Host "$Detail : deja present, rien a faire."
    return
  }
  Add-Content -LiteralPath $Path -Value $PublicKey -Encoding ascii
  Write-Host "$Detail : ajoutee -> $Path"
}

# 1) Cle publique du poste (sens poste -> conteneur). Generee si absente.
if (-not (Test-Path -LiteralPath $WindowsPublicKey)) {
  if (-not (Test-Path -LiteralPath $SshDir)) {
    New-Item -ItemType Directory -Path $SshDir | Out-Null
  }
  Write-Host "Aucune cle locale : generation de $WindowsPublicKey ..."
  Invoke-SshKeygen -KeyFile (Join-Path $SshDir "id_ed25519")
}
$windowsKey = (Get-Content -LiteralPath $WindowsPublicKey -Raw).Trim()

# 2) authorized_keys du conteneur (sens poste -> conteneur).
if (-not (Test-Path -LiteralPath $ContainerSshDir)) {
  New-Item -ItemType Directory -Path $ContainerSshDir | Out-Null
}
Add-KeyLine `
  -Path $ContainerAuthorizedKeys `
  -PublicKey $windowsKey `
  -Detail "Cle du poste pour le conteneur"

# 3) Cle de retour du conteneur (sens conteneur -> poste).
if (-not (Test-Path -LiteralPath "$ContainerBackKey.pub")) {
  Write-Host "Cle de retour manquante : generation de $ContainerBackKey ..."
  Invoke-SshKeygen -KeyFile $ContainerBackKey
}
$backPublicKey = (Get-Content -LiteralPath "$ContainerBackKey.pub" -Raw).Trim()

# 4) authorized_keys du poste Windows (sens conteneur -> poste).
if (-not (Test-Path -LiteralPath $SshDir)) {
  New-Item -ItemType Directory -Path $SshDir | Out-Null
}
Add-KeyLine `
  -Path $WindowsAuthorizedKeys `
  -PublicKey $backPublicKey `
  -Detail "Cle de retour du conteneur pour le poste"

# OpenSSH Windows refuse une authorized_keys atteignable par d'autres comptes
# ("Bad owner or permissions") : on retire l'heritage et on ne garde que
# l'utilisateur et SYSTEM.
icacls $WindowsAuthorizedKeys /inheritance:r /grant:r "$env:USERNAME:F" "SYSTEM:F" | Out-Null

# 5) Verifications cote poste.
$sshd = Get-Service sshd -ErrorAction SilentlyContinue
if (-not $sshd) {
  Write-Warning "OpenSSH Server Windows n'est pas installe. Installe-le:"
  Write-Warning "  Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0"
}
elseif ($sshd.Status -ne "Running") {
  Write-Warning "Le service sshd Windows existe mais est arrete:"
  Write-Warning "  Start-Service sshd"
}
else {
  Write-Host "sshd Windows actif sur le port $WindowsSshPort - OK."
if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
  Write-Host "Compte non administrateur: la cle est bien lue dans $WindowsAuthorizedKeys."
}
else {
  Write-Warning "Compte administrateur: OpenSSH Windows ignore ~/.ssh/authorized_keys."
  Write-Warning "Copie la cle de retour dans C:\ProgramData\ssh\administrators_authorized_keys (admin requis)."
}
}

# 6) Rappel du lancement unique (cles + rebuild + up).
Write-Host ""
Write-Host "Pour tout lancer d'un coup (cles incluses, idempotent):"
Write-Host "  npm run container:start"
Write-Host ""
Write-Host "Connexions disponibles une fois le conteneur relance:"
Write-Host "  poste -> conteneur : ssh -p $ContainerSshPort cst@127.0.0.1"
Write-Host "  conteneur -> poste : ssh local   (dans le conteneur, compte $WindowsUser)"