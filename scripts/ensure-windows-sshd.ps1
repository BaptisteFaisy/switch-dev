[CmdletBinding()]
param(
  # Port du serveur OpenSSH Windows attendu par le pont conteneur -> poste.
  # Simple controle d'information : on ne reecrit pas la config sshd ici.
  [ValidateRange(1, 65535)]
  [int]$WindowsSshPort = 2222
)

# Helper eleve (UAC) : garantit que le serveur OpenSSH Windows est installe,
# demarre et configure en demarrage automatique. Idempotent. Appele par
# start-container.ps1 uniquement quand sshd est arrete ou non automatique.

$ErrorActionPreference = "Stop"

$sshd = Get-Service sshd -ErrorAction SilentlyContinue
if (-not $sshd) {
  Write-Host "OpenSSH Server Windows n'est pas installe."
  Write-Host "  Installer : Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0"
  Write-Host "  (necessite une connexion, un redemarrage puis de reconfigurer le port 2222)"
  exit 1
}

if ($sshd.StartType -ne "Automatic") {
  Set-Service sshd -StartupType Automatic
  Write-Host "sshd Windows : demarrage automatique active."
}

if ($sshd.Status -ne "Running") {
  Start-Service sshd
  Write-Host "sshd Windows : service demarre."
}

$sshd.Refresh()
if ($sshd.Status -eq "Running") {
  Write-Host "sshd Windows actif sur le port $WindowsSshPort (auto au demarrage)."
  exit 0
}

Write-Host "sshd Windows n'a pas pu etre demarre."
exit 1
