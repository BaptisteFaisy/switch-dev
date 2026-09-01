Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# Remet les dossiers modules Windows PowerShell en tete de PSModulePath (le
# stub sandbox Freebuff/Codex masque sinon le vrai Microsoft.PowerShell.Utility
# et fait perdre Get-FileHash en PowerShell 5.1). Logique partagee :
# scripts/switch-development-ensure-system-module-path.ps1
& (Join-Path $PSScriptRoot 'switch-development-ensure-system-module-path.ps1')

function Get-TextSha256 {
  param([Parameter(Mandatory = $true)][AllowEmptyString()][string]$Text)

  $encoding = New-Object Text.UTF8Encoding($false)
  $algorithm = [Security.Cryptography.SHA256]::Create()
  try {
    $bytes = $encoding.GetBytes($Text)
    return ([BitConverter]::ToString($algorithm.ComputeHash($bytes))).Replace('-', '')
  }
  finally {
    $algorithm.Dispose()
  }
}

function Test-InventoryProperty {
  param(
    [Parameter(Mandatory = $true)]$Value,
    [Parameter(Mandatory = $true)][string]$Name
  )

  return $null -ne $Value.PSObject.Properties[$Name]
}

function Get-SwitchDevelopmentDistInventory {
  [CmdletBinding()]
  param([Parameter(Mandatory = $true)][string]$DistRoot)

  $rootItem = Get-Item -LiteralPath $DistRoot -Force -ErrorAction Stop
  if (-not $rootItem.PSIsContainer) {
    throw "Le chemin dist n'est pas un dossier: $DistRoot"
  }
  if (($rootItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
    throw "Le dossier dist ne peut pas etre un lien/reparse point: $($rootItem.FullName)"
  }

  $rootFull = [IO.Path]::GetFullPath($rootItem.FullName).TrimEnd('\', '/')
  $rootPrefix = $rootFull + [IO.Path]::DirectorySeparatorChar
  $filesByPath = New-Object 'Collections.Generic.Dictionary[string,string]' ([StringComparer]::Ordinal)
  $directories = New-Object 'Collections.Generic.Stack[IO.DirectoryInfo]'
  $directories.Push($rootItem)

  while ($directories.Count -gt 0) {
    $directory = $directories.Pop()
    foreach ($item in Get-ChildItem -LiteralPath $directory.FullName -Force -ErrorAction Stop) {
      if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Lien/reparse point refuse dans dist: $($item.FullName)"
      }
      if ($item.PSIsContainer) {
        $directories.Push($item)
        continue
      }

      $fullPath = [IO.Path]::GetFullPath($item.FullName)
      if (-not $fullPath.StartsWith($rootPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Fichier dist hors racine refuse: $fullPath"
      }
      $relativePath = $fullPath.Substring($rootPrefix.Length).Replace('\', '/')
      if ($filesByPath.ContainsKey($relativePath)) {
        throw "Chemin dist duplique apres normalisation: $relativePath"
      }
      $filesByPath.Add($relativePath, $fullPath)
    }
  }

  [string[]]$relativePaths = @($filesByPath.Keys)
  [Array]::Sort($relativePaths, [StringComparer]::Ordinal)
  $entries = New-Object 'Collections.Generic.List[object]'
  $treeLines = New-Object 'Collections.Generic.List[string]'

  foreach ($relativePath in $relativePaths) {
    $path = $filesByPath[$relativePath]
    $item = Get-Item -LiteralPath $path -Force -ErrorAction Stop
    $length = [int64]$item.Length
    $sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash.ToUpperInvariant()
    $entry = [ordered]@{
      path = $relativePath
      length = $length
      sha256 = $sha256
    }
    $entries.Add([pscustomobject]$entry)
    $lengthText = $length.ToString([Globalization.CultureInfo]::InvariantCulture)
    $treeLines.Add("$relativePath`0$lengthText`0$sha256")
  }

  $treeText = ($treeLines.ToArray()) -join "`n"
  return [pscustomobject][ordered]@{
    treeSha256 = Get-TextSha256 -Text $treeText
    fileCount = $entries.Count
    files = $entries.ToArray()
  }
}

function Test-SwitchDevelopmentDistInventoryEqual {
  [CmdletBinding()]
  param(
    [Parameter(Mandatory = $true)]$Left,
    [Parameter(Mandatory = $true)]$Right
  )

  foreach ($property in @('treeSha256', 'fileCount', 'files')) {
    if (-not (Test-InventoryProperty -Value $Left -Name $property) -or
        -not (Test-InventoryProperty -Value $Right -Name $property)) {
      return $false
    }
  }
  if ([string]$Left.treeSha256 -ne [string]$Right.treeSha256 -or
      [int]$Left.fileCount -ne [int]$Right.fileCount) {
    return $false
  }

  $leftFiles = @($Left.files)
  $rightFiles = @($Right.files)
  if ($leftFiles.Count -ne $rightFiles.Count -or $leftFiles.Count -ne [int]$Left.fileCount) {
    return $false
  }
  for ($index = 0; $index -lt $leftFiles.Count; $index += 1) {
    $leftEntry = $leftFiles[$index]
    $rightEntry = $rightFiles[$index]
    if ([string]$leftEntry.path -cne [string]$rightEntry.path -or
        [int64]$leftEntry.length -ne [int64]$rightEntry.length -or
        [string]$leftEntry.sha256 -ne [string]$rightEntry.sha256) {
      return $false
    }
  }
  return $true
}

Export-ModuleMember -Function @(
  'Get-SwitchDevelopmentDistInventory',
  'Test-SwitchDevelopmentDistInventoryEqual'
)
