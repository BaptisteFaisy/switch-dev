param(
  [int]$Port = 18080,
  [string]$TargetDir = "",
  [string]$SourceDir = "",
  [switch]$SkipBuild,
  [ValidateRange(1, 60)]
  [int]$LockTimeoutSec = 10,
  [ValidateRange(1, 720)]
  [int]$StaleAssetRetentionHours = 72
)

# Publication frontend multi-agents sur le noeud local, sans drain ni restart.
# Vite produit des assets hashes : on les copie d'abord puis on remplace
# index.html atomiquement en dernier. Les anciens chunks hashes restent servis
# pendant une courte periode afin que les onglets deja ouverts puissent finir
# leurs imports dynamiques. Le mutex ne couvre que cette publication, jamais le
# build.

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$Root = Split-Path -Parent $ScriptDir
$DefaultSourceDir = Join-Path $Root "dist"
$LocalUrl = "http://127.0.0.1:$Port"

if (-not $SourceDir) {
  $SourceDir = $DefaultSourceDir
}
elseif (-not $SkipBuild) {
  throw "-SourceDir exige -SkipBuild afin de ne jamais publier un build different de la source demandee."
}

if (-not $TargetDir) {
  $NodeHome = Join-Path $env:LOCALAPPDATA "codex-switch-terminal-node"
  $TargetDir = Join-Path $NodeHome "current\dist"
}

function Copy-TreeEntry {
  param(
    [Parameter(Mandatory = $true)][string]$Source,
    [Parameter(Mandatory = $true)][string]$Destination
  )

  $item = Get-Item -LiteralPath $Source -Force
  if (-not $item.PSIsContainer) {
    $parent = Split-Path -Parent $Destination
    New-Item -ItemType Directory -Force -Path $parent | Out-Null
    Copy-Item -LiteralPath $Source -Destination $Destination -Force
    return
  }

  New-Item -ItemType Directory -Force -Path $Destination | Out-Null
  foreach ($child in Get-ChildItem -LiteralPath $Source -Force) {
    Copy-TreeEntry -Source $child.FullName -Destination (Join-Path $Destination $child.Name)
  }
}

function Get-TreeRelativePath {
  param(
    [Parameter(Mandatory = $true)][string]$Root,
    [Parameter(Mandatory = $true)][string]$FullName
  )

  $normalizedRoot = [IO.Path]::GetFullPath($Root).TrimEnd(
    [IO.Path]::DirectorySeparatorChar,
    [IO.Path]::AltDirectorySeparatorChar
  )
  $normalizedName = [IO.Path]::GetFullPath($FullName)
  $prefix = $normalizedRoot + [IO.Path]::DirectorySeparatorChar
  if (-not $normalizedName.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Chemin hors du dist pendant le nettoyage: $normalizedName"
  }
  return $normalizedName.Substring($prefix.Length)
}

function Remove-StaleTreeEntries {
  param(
    [Parameter(Mandatory = $true)][string]$SourceRoot,
    [Parameter(Mandatory = $true)][string]$TargetRoot,
    [Parameter(Mandatory = $true)][int]$RetainAssetHours
  )

  $sourceEntries = @{}
  foreach ($entry in Get-ChildItem -LiteralPath $SourceRoot -Recurse -Force) {
    $relative = Get-TreeRelativePath -Root $SourceRoot -FullName $entry.FullName
    $sourceEntries[$relative] = $true
  }

  $removed = 0
  $assetPrefix = "assets$([IO.Path]::DirectorySeparatorChar)"
  $assetCutoff = [DateTime]::UtcNow.AddHours(-$RetainAssetHours)
  $targetEntries = @(
    Get-ChildItem -LiteralPath $TargetRoot -Recurse -Force |
      Sort-Object { $_.FullName.Length } -Descending
  )
  foreach ($entry in $targetEntries) {
    $relative = Get-TreeRelativePath -Root $TargetRoot -FullName $entry.FullName
    if ($sourceEntries.ContainsKey($relative)) { continue }
    if (
      -not $entry.PSIsContainer -and
      $relative.StartsWith($assetPrefix, [StringComparison]::OrdinalIgnoreCase) -and
      $entry.LastWriteTimeUtc -ge $assetCutoff
    ) {
      continue
    }
    if (Test-Path -LiteralPath $entry.FullName) {
      Remove-Item -LiteralPath $entry.FullName -Recurse -Force
      $removed++
    }
  }
  return $removed
}

function Test-ServerReady {
  try {
    $health = Invoke-RestMethod -Uri "$LocalUrl/healthz" -TimeoutSec 3
    return $health.ok -eq $true
  }
  catch {
    return $false
  }
}

# Le travail long reste volontairement hors mutex.
if (-not $SkipBuild) {
  Push-Location $Root
  try {
    npm run build:frontend
    if ($LASTEXITCODE -ne 0) { throw "Le build frontend a echoue." }
  }
  finally {
    Pop-Location
  }
}

$sourceIndex = Join-Path $SourceDir "index.html"
if (-not (Test-Path -LiteralPath $sourceIndex -PathType Leaf)) {
  throw "Build frontend introuvable: $sourceIndex"
}
if (-not (Test-Path -LiteralPath $TargetDir -PathType Container)) {
  throw "Dist active introuvable: $TargetDir. Installe/demarre d'abord le noeud local."
}
if (-not (Test-ServerReady)) {
  throw "Le serveur $LocalUrl ne repond pas ; publication annulee sans modifier le dist actif."
}

$SourceDir = [IO.Path]::GetFullPath($SourceDir)
$TargetDir = [IO.Path]::GetFullPath($TargetDir)
if ($SourceDir.TrimEnd('\') -eq $TargetDir.TrimEnd('\')) {
  Write-Host "Le build est deja le dist actif de $LocalUrl ; aucune copie necessaire." -ForegroundColor Green
  exit 0
}

$mutex = [Threading.Mutex]::new($false, "Local\CST-Deploy-$Port")
$lockHeld = $false
$timer = [Diagnostics.Stopwatch]::new()
$targetIndex = Join-Path $TargetDir "index.html"
$nonce = "$PID-$([Guid]::NewGuid().ToString('N'))"
$tempIndex = Join-Path $TargetDir ".index-$nonce.tmp"
$backupIndex = Join-Path $TargetDir ".index-$nonce.bak"
$staleCount = 0

try {
  try {
    $lockHeld = $mutex.WaitOne([TimeSpan]::FromSeconds($LockTimeoutSec))
  }
  catch [Threading.AbandonedMutexException] {
    # Le publisher precedent est mort : Windows nous transfere le mutex.
    $lockHeld = $true
  }
  if (-not $lockHeld) {
    throw "Un autre agent publie deja le frontend ; reessaie dans quelques secondes."
  }
  $timer.Start()

  # Tous les fichiers pointes par le nouvel index sont presents avant que cet
  # index devienne visible.
  foreach ($entry in Get-ChildItem -LiteralPath $SourceDir -Force) {
    if ($entry.Name -eq "index.html") { continue }
    Copy-TreeEntry -Source $entry.FullName -Destination (Join-Path $TargetDir $entry.Name)
  }

  Copy-Item -LiteralPath $sourceIndex -Destination $tempIndex -Force
  $index = Get-Content -LiteralPath $tempIndex -Raw
  $references = [regex]::Matches($index, '(?:src|href)="([^"#?]+)"')
  foreach ($match in $references) {
    $reference = $match.Groups[1].Value
    if ($reference -match '^(?:[a-z]+:|//|data:)') { continue }
    $relative = $reference.TrimStart('/').Replace('/', [IO.Path]::DirectorySeparatorChar)
    if ($relative -and -not (Test-Path -LiteralPath (Join-Path $TargetDir $relative))) {
      throw "Publication incomplete : $reference reference par index.html est absent."
    }
  }

  if (Test-Path -LiteralPath $targetIndex) {
    # File.Replace repose sur ReplaceFileW sous Windows : l'ancien ou le nouvel
    # index est visible, jamais un fichier partiellement copie.
    [IO.File]::Replace($tempIndex, $targetIndex, $backupIndex, $true)
    Remove-Item -LiteralPath $backupIndex -Force -ErrorAction SilentlyContinue
  }
  else {
    [IO.File]::Move($tempIndex, $targetIndex)
  }

  # Les fichiers ordinaires obsoletes partent immediatement. Les chunks hashes
  # recents gardent une fenetre de grace pour les pages ouvertes avant la
  # bascule ; le garde frontend recharge ensuite les onglets plus anciens.
  $staleCount = Remove-StaleTreeEntries `
    -SourceRoot $SourceDir `
    -TargetRoot $TargetDir `
    -RetainAssetHours $StaleAssetRetentionHours
}
finally {
  $timer.Stop()
  Remove-Item -LiteralPath $tempIndex -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $backupIndex -Force -ErrorAction SilentlyContinue
  if ($lockHeld) { $mutex.ReleaseMutex() }
  $mutex.Dispose()
}

# Verification apres liberation du mutex : elle ne rallonge pas la fenetre
# exclusive et ne bloque jamais les autres agents.
$served = Invoke-WebRequest -UseBasicParsing -Uri "$LocalUrl/?cst-publish=$nonce" -TimeoutSec 5
if ($served.StatusCode -ne 200) {
  throw "Frontend publie mais verification HTTP en echec ($($served.StatusCode))."
}

Write-Host (
  "Frontend publie sur {0} sans drain ni restart (mutex {1:N0} ms, {2} ancien(s) fichier(s) supprime(s))." -f `
    $LocalUrl, $timer.Elapsed.TotalMilliseconds, $staleCount
) -ForegroundColor Green
