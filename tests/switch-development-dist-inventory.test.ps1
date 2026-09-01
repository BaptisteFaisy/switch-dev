[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$modulePath = Join-Path $repoRoot "scripts\switch-development-dist-inventory.psm1"
Import-Module -Name $modulePath -Force -Scope Local

function Assert-True {
  param(
    [Parameter(Mandatory = $true)][bool]$Condition,
    [Parameter(Mandatory = $true)][string]$Message
  )
  if (-not $Condition) {
    throw $Message
  }
}

function Copy-DistFixture {
  param(
    [Parameter(Mandatory = $true)][string]$Source,
    [Parameter(Mandatory = $true)][string]$Destination
  )
  Copy-Item -LiteralPath $Source -Destination $Destination -Recurse
}

$systemTempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
$testRoot = Join-Path $systemTempRoot ("switch-development-dist-inventory-" + [Guid]::NewGuid().ToString("N"))
$testRootFull = [IO.Path]::GetFullPath($testRoot)
if (-not $testRootFull.StartsWith($systemTempRoot, [StringComparison]::OrdinalIgnoreCase)) {
  throw "Racine temporaire de test hors du dossier temporaire systeme: $testRootFull"
}

try {
  $sourceDist = Join-Path $testRootFull "source-dist"
  New-Item -ItemType Directory -Path (Join-Path $sourceDist "assets\lazy") -Force | Out-Null
  Set-Content -LiteralPath (Join-Path $sourceDist "index.html") -Value "<html>fixture</html>" -Encoding UTF8
  Set-Content -LiteralPath (Join-Path $sourceDist "service-worker.js") -Value "self.fixture = true;" -Encoding UTF8
  Set-Content -LiteralPath (Join-Path $sourceDist "assets\index-fixture.js") -Value "export const entry = true;" -Encoding UTF8
  Set-Content -LiteralPath (Join-Path $sourceDist "assets\lazy\panel.js") -Value "export const panel = 1;" -Encoding UTF8
  Set-Content -LiteralPath (Join-Path $sourceDist "assets\lazy\panel.css") -Value ".panel { color: blue; }" -Encoding UTF8

  $sourceInventory = Get-SwitchDevelopmentDistInventory -DistRoot $sourceDist
  $paths = @($sourceInventory.files | ForEach-Object { [string]$_.path })
  [string[]]$expectedPaths = @($paths)
  [Array]::Sort($expectedPaths, [StringComparer]::Ordinal)
  Assert-True -Condition (($paths -join "`n") -ceq ($expectedPaths -join "`n")) `
    -Message "L'inventaire dist n'est pas trie selon l'ordre ordinal des chemins."

  $unchangedDist = Join-Path $testRootFull "unchanged-dist"
  Copy-DistFixture -Source $sourceDist -Destination $unchangedDist
  $unchangedInventory = Get-SwitchDevelopmentDistInventory -DistRoot $unchangedDist
  Assert-True -Condition (Test-SwitchDevelopmentDistInventoryEqual `
    -Left $sourceInventory -Right $unchangedInventory) `
    -Message "Une copie dist identique doit conserver le meme manifeste."

  $modifiedDist = Join-Path $testRootFull "modified-dist"
  Copy-DistFixture -Source $sourceDist -Destination $modifiedDist
  Set-Content -LiteralPath (Join-Path $modifiedDist "assets\lazy\panel.js") `
    -Value "export const panel = 2;" -Encoding UTF8
  $modifiedInventory = Get-SwitchDevelopmentDistInventory -DistRoot $modifiedDist
  Assert-True -Condition (-not (Test-SwitchDevelopmentDistInventoryEqual `
    -Left $sourceInventory -Right $modifiedInventory)) `
    -Message "La modification d'un chunk lazy doit changer le manifeste dist."

  $deletedDist = Join-Path $testRootFull "deleted-dist"
  Copy-DistFixture -Source $sourceDist -Destination $deletedDist
  $deletedPath = Join-Path $deletedDist "assets\lazy\panel.css"
  if (-not ([IO.Path]::GetFullPath($deletedPath)).StartsWith($testRootFull + '\', [StringComparison]::OrdinalIgnoreCase)) {
    throw "Chemin de suppression de fixture hors racine temporaire: $deletedPath"
  }
  Remove-Item -LiteralPath $deletedPath -Force
  $deletedInventory = Get-SwitchDevelopmentDistInventory -DistRoot $deletedDist
  Assert-True -Condition (-not (Test-SwitchDevelopmentDistInventoryEqual `
    -Left $sourceInventory -Right $deletedInventory)) `
    -Message "La suppression d'un CSS lazy doit changer le manifeste dist."

  $addedDist = Join-Path $testRootFull "added-dist"
  Copy-DistFixture -Source $sourceDist -Destination $addedDist
  Set-Content -LiteralPath (Join-Path $addedDist "assets\lazy\extra.css") `
    -Value ".extra { display: grid; }" -Encoding UTF8
  $addedInventory = Get-SwitchDevelopmentDistInventory -DistRoot $addedDist
  Assert-True -Condition (-not (Test-SwitchDevelopmentDistInventoryEqual `
    -Left $sourceInventory -Right $addedInventory)) `
    -Message "L'ajout d'un CSS lazy doit changer le manifeste dist."

  Write-Output "SWITCH_DEVELOPMENT_DIST_INVENTORY_TESTS=OK"
}
finally {
  if (Test-Path -LiteralPath $testRootFull) {
    if (-not $testRootFull.StartsWith($systemTempRoot, [StringComparison]::OrdinalIgnoreCase)) {
      throw "Nettoyage refuse hors du dossier temporaire systeme: $testRootFull"
    }
    Remove-Item -LiteralPath $testRootFull -Recurse -Force
  }
}
