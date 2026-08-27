# Surveille la bande passante du Switch local (SSD, Windows) afin de repérer
# une saturation (ex. connexion limitée à 10 Mbit/s) quand la flotte d'agents
# autonomes ou les chats tournent.
#
# Modes :
#   - Get-NetAdapterStatistics  (défaut / -Once) : relevé ponctuel du débit
#     descendant/montant en Mo/s sur l'adaptateur par défaut.
#   - -Continu  : boucle toutes les -Seconds secondes jusqu'à touche
#     (équivaut à nload/iftop côté VPS).
#
# Usage :
#   powershell -File scripts/bandwidth-monitor.ps1 [-Once|-Continu] [-Seconds 10]
#
# Règle de base (10 Mbit/s = 1,25 Mo/s) :
#   - API LLM active ~0,5-2 Mo/s par tour ; si le débit soutenu approche
#     1,25 Mo/s en continu, réduisez la cadence/un lancement simultané
#     (voir CST_AUTONOMOUS_AGENT_STAGGER_MS).

[CmdletBinding()]
param(
  [switch]$Once,
  [switch]$Continu,
  [ValidateRange(1, 3600)]
  [int]$Seconds = 10
)

$ErrorActionPreference = "Stop"

function Get-DefaultInterface {
  try {
    if (-not $Once -and -not $Continu) {
      # interactive : on choisit l'interface avec le plus de trafic cumulé
      return Get-NetAdapterStatistics | Sort-Object ReceivedBytes -Descending | Select-Object -First 1
    }
  } catch { }
  $adapter = Get-NetAdapter -Physical | Where-Object Status -eq 'Up' | Select-Object -First 1
  if (-not $adapter) { throw "Aucune interface reseau active." }
  return Get-NetAdapterStatistics -Name $adapter.Name
}

function Format-MoPerSec {
  param([double]$Bytes, [int]$Window)
  return "{0:N2} Mo/s" -f ($Bytes / 1048576 / $Window)
}

$iface = Get-DefaultInterface

if ($Continu) {
  Write-Host "Surveillance continue de $($iface.Name) toutes les ${Seconds}s (Ctrl+C pour quitter)."
  while ($true) {
    $rx0 = $iface.ReceivedBytes; $tx0 = $iface.SentBytes
    Start-Sleep -Seconds $Seconds
    $iface = Get-NetAdapterStatistics -Name $iface.Name
    $rx1 = $iface.ReceivedBytes; $tx1 = $iface.SentBytes
    $down = $rx1 - $rx0; $up = $tx1 - $tx0
    $when = Get-Date -Format 'HH:mm:ss'
    Write-Host ("{0}  {1}  Descent:{2}  Mont:{3}" -f $when, $Seconds, (Format-MoPerSec $down $Seconds), (Format-MoPerSec $up $Seconds))
    Start-Sleep 0
  }
}

# Mode ponctuel (défaut / -Once)
$rx0 = $iface.ReceivedBytes; $tx0 = $iface.SentBytes
Start-Sleep -Seconds $Seconds
$iface = Get-NetAdapterStatistics -Name $iface.Name
$rx1 = $iface.ReceivedBytes; $tx1 = $iface.SentBytes
$down = $rx1 - $rx0; $up = $tx1 - $tx0

Write-Host ""
Write-Host ("Interface  : {0}  ({1})" -f $iface.Name, $iface.LinkSpeed)
Write-Host ("Fenetre    : {0}s" -f $Seconds)
Write-Host ("Descendant : {0}  (max 10 Mbit/s = 1,25 Mo/s)" -f (Format-MoPerSec $down $Seconds))
Write-Host ("Montant    : {0}" -f (Format-MoPerSec $up $Seconds))
Write-Host ""
Write-Host "Interpretation :"
Write-Host "  - debit soutenu proche de 1,25 Mo/s  -> connexion saturee."
Write-Host "  - sinon, laissez la flotte tourner (CPU/RAM OK)."