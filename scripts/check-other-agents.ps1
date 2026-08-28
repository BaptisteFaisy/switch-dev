# check-other-agents.ps1
# ======================
# Obligation collaborative sur le noeud de dev partage : avant de deplacer
# son propre build/release dans app et de redemarrer, tout agent DOIT prendre
# connaissance de l'activite des autres chats et inclure leurs changements.
#
# Ce script inventorie de facon objective :
#   1. les commits recents du clone app (auteur/date/titre),
#   2. les changements non commites (arbres de travail chauds),
#   3. les builds deployes recents (alertes s'ils different de HEAD),
#   4. les fichiers cibles que l'agent s'apprete a remplacer et qui ont ete
#      modifies par d'autres (= risque d'ecrasement).
#
# Sortie : 0 = aucune concurrence a risque ; 2 = concurrence a prendre en compte
# (avec -Strict, le script refusera et l'agent doit traiter l'activite avant de
# deployer, sur autorisation explicite de l'utilisateur).
#
# Exemples :
#   check-other-agents.ps1
#   check-other-agents.ps1 -Strict
#   check-other-agents.ps1 -Strict -OverlapPaths src/main.ts,src-tauri/src/gmail.rs

[CmdletBinding()]
param(
  [switch]$Strict,
  [string[]]$OverlapPaths
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$appRoot = Split-Path -Parent $scriptRoot
$runtimeReleasesPath = Join-Path $env:LOCALAPPDATA "SwitchDevelopmentRuntime\releases"

function Get-RelativeTime {
  param([datetime]$T)
  if ($null -eq $T -or $T -eq [datetime]::MinValue) { return "inconnu" }
  $span = (Get-Date) - $T
  if ($span.TotalMinutes -lt 1) { return "moins d'une minute" }
  if ($span.TotalMinutes -lt 60) { return "{0} min" -f [int]$span.TotalMinutes }
  if ($span.TotalHours -lt 24) { return "{0} h {1} min" -f [int]$span.TotalHours, [int]$span.Minutes }
  return "{0} jour(s)" -f [int]$span.TotalDays
}

Write-Host ""
Write-Host "====================================================================="
Write-Host "  VERIFICATION DES AUTRES CHATS (obligatoire avant tout deploiement)"
Write-Host "  noeud : $appRoot"
Write-Host "====================================================================="

# --- 1. Commits recents -----------------------------------------------------
Write-Host ""
Write-Host "[1/4] Commits git recents du clone app (activite des autres chats) :"
$recent = git -C $appRoot log --since="24 hours ago" --pretty=format:"  %h %ad %an -> %s" `
  --date=format:"%H:%M:%S" 2>$null | Select-Object -First 10
if ($recent) { $recent | ForEach-Object { Write-Host $_ } } else { Write-Host "  (aucun commit dans les 24 dernieres heures)" }

$headRaw = git -C $appRoot log -1 --format="%at %s" 2>$null
$headAgo = "inconnu"
if ($headRaw) {
  $epoch = [long]($headRaw.Split(' ')[0])
  $headAgo = Get-RelativeTime -T ([DateTimeOffset]::FromUnixTimeSeconds($epoch).LocalDateTime)
}
Write-Host "  -> HEAD = $(git -C $appRoot rev-parse --short HEAD) du $headAgo"

# --- 2. Working tree sale ---------------------------------------------------
Write-Host ""
Write-Host "[2/4] Changements non commites (quelqu'un est evidemment en train de travailler) :"
$dirty = git -C $appRoot status --porcelain 2>$null |
  Where-Object { $_ -notmatch '\.backup-ssd' }
$dirtyList = @($dirty)
if ($dirtyList.Count -gt 0) { $dirtyList | ForEach-Object { Write-Host "  $_" } } else { Write-Host "  (arbre de travail propre)" }

# --- 3. Builds deployes recents ---------------------------------------------
Write-Host ""
Write-Host "[3/4] Builds deployes recents (qui peuvent etre d'autres chats) :"
$hasReleases = (Test-Path -LiteralPath $runtimeReleasesPath)
$recentReleases = @()
if ($hasReleases) {
  $cutoff = (Get-Date).AddHours(-12)
  $recentReleases = @(Get-ChildItem -LiteralPath $runtimeReleasesPath -Directory |
    Where-Object { $_.LastWriteTime -ge $cutoff } | Sort-Object LastWriteTime -Descending)
}
if ($recentReleases.Count -gt 0) {
  $recentReleases | Select-Object -First 6 | ForEach-Object {
    Write-Host ("  {0}  (il y a {1})" -f $_.Name, (Get-RelativeTime -T $_.LastWriteTime))
  }
  if ($recentReleases.Count -gt 6) {
    Write-Host ("  ... +{0} autre(s) build(s) dans les 12 dernieres heures" -f ($recentReleases.Count - 6))
  }
} else {
  Write-Host "  (aucun build deploye reparametre dans les 12 dernieres heures)"
}

# --- 4. Chevauchement de fichiers -------------------------------------------
$overlapHits = @()
if ($OverlapPaths) {
  Write-Host ""
  Write-Host "[4/4] Fichiers cibles modifies par d'autres chats :"
  foreach ($p in $OverlapPaths) {
    $status = git -C $appRoot status --porcelain -- $p 2>$null
    if ($status) {
      $overlapHits += $p
      Write-Host ("  OTAGE RISQUE: {0}" -f $p)
    } else {
      Write-Host ("  ok: {0}" -f $p)
    }
  }
}

# --- Synthese ---------------------------------------------------------------
Write-Host ""
Write-Host "====================================================================="
Write-Host "  SYNTHESE"
Write-Host "====================================================================="
$warnings = @()
$blockers = @()
if ($dirtyList.Count -gt 0) {
  $message = "Des fichiers sont en cours de modification (non commites, {0} entrees)." -f $dirtyList.Count
  $warnings += $message
  $blockers += $message
}
if ($headAgo -match "min|heure") {
  $warnings += "Des commits ont ete faits il y a moins d'une heure ; ils sont deja inclus dans HEAD."
}
if ($overlapHits.Count -gt 0) {
  $message = "TU S'APPRETES A REMPLACER DES FICHIERS QU'UN AUTRE CHAT A MODIFIES: {0}" -f ($overlapHits -join ", ")
  $warnings += $message
  $blockers += $message
}
if ($recentReleases.Count -gt 0) {
  $warnings += "Des builds ont ete deployes recemment ; assure-toi de deployer au-dessus d'eux (pas de les retirer)."
}

if ($warnings.Count -eq 0) {
  Write-Host "  Aucune concurrence active detectee. Rappel : verifie toujours manuellement"
  Write-Host "  l'etat avant d'ecraser app\ (commits, releases)."
  exit 0
}
foreach ($w in $warnings) {
  if ($w -like "TU S'APPRETES*") { Write-Host "  [!!] $w" } else { Write-Host ("  [!] {0}" -f $w) }
}

if ($Strict -and $blockers.Count -gt 0) {
  Write-Host ""
  Write-Host "  MODE STRICT : la concurrence detectee doit etre prise en compte avant de"
  Write-Host "  deployer. Integre les changements des autres chats (verifie leurs commits),"
  Write-Host "  ou demande la confirmation explicite de l'utilisateur pour passer outre."
  exit 2
}
if ($Strict) {
  Write-Host ""
  Write-Host "  MODE STRICT : aucun changement concurrent non integre ne bloque la bascule."
  exit 0
}
Write-Host ""
Write-Host "  (mode normal) Lance ce script avec -Strict pour exiger la prise en compte."
exit 0
