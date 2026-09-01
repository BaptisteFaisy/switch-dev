[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('Capture', 'Verify', 'Report')]
    [string]$Mode,

    [Parameter(Mandatory = $true)]
    [string]$CanonicalSourceRoot,

    [Parameter(Mandatory = $true)]
    [string]$BuildSnapshotRoot,

    [string]$ManifestPath,

    [ValidateSet('SwitchDevelopment', 'AzureVps')]
    [string]$Target = 'SwitchDevelopment',

    [switch]$ConfirmAllConcurrentChangesMerged,
    [switch]$UserAuthorizedVpsDeployment
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# PSModulePath : remet les modules Windows PowerShell en tete (Get-FileHash introuvable sinon dans le sandbox).
& (Join-Path $PSScriptRoot 'switch-development-ensure-system-module-path.ps1')

$excludedDirectoryNames = @(
    '.git',
    '.freebuff',
    '.codex-proof',
    '.cst-data',
    '.codex',
    '.claude',
    '.agents',
    '.kombai',
    '.gradle',
    'build',
    'node_modules',
    'target',
    'dist',
    'coverage',
    '.turbo',
    'clawsweeper',
    'clawsweeper-state',
    'preview-phone'
)

function Resolve-DirectoryPath {
    param([Parameter(Mandatory = $true)][string]$Path)

    $resolved = Resolve-Path -LiteralPath $Path -ErrorAction Stop
    $item = Get-Item -LiteralPath $resolved.Path -Force -ErrorAction Stop
    if (-not $item.PSIsContainer) {
        throw "Le chemin n'est pas un dossier: $Path"
    }
    return [IO.Path]::GetFullPath($item.FullName).TrimEnd('\')
}

function Get-TextSha256 {
    param([Parameter(Mandatory = $true)][AllowEmptyString()][string]$Text)

    $encoding = New-Object Text.UTF8Encoding($false)
    $algorithm = [Security.Cryptography.SHA256]::Create()
    try {
        $bytes = $encoding.GetBytes($Text)
        return ([BitConverter]::ToString($algorithm.ComputeHash($bytes))).Replace('-', '')
    } finally {
        $algorithm.Dispose()
    }
}

function Test-IsExcludedRelativePath {
    param([Parameter(Mandatory = $true)][string]$RelativePath)

    $parts = $RelativePath.Replace('\', '/').Split('/')
    foreach ($part in $parts) {
        if ($excludedDirectoryNames -contains $part) {
            return $true
        }
    }
    $fileName = $parts[-1].ToLowerInvariant()
    if ($fileName -in @('.mcp.json', 'server.local.env.ps1')) {
        return $true
    }
    if ($fileName -eq '.env' -or $fileName.EndsWith('.env')) {
        return $true
    }
    if ($fileName -match '\.env\.' -and
        $fileName -notmatch '\.env\.(example|sample|template)$') {
        return $true
    }
    if ([IO.Path]::GetExtension($fileName) -in @('.exe', '.apk', '.msi', '.pdb')) {
        return $true
    }
    return $false
}

function Get-GitMetadata {
    param([Parameter(Mandatory = $true)][string]$Root)

    if (-not (Test-Path -LiteralPath (Join-Path $Root '.git'))) {
        return [ordered]@{
            isGit = $false
            head = $null
            statusSha256 = $null
            statusLineCount = 0
        }
    }

    $headOutput = @(& git -C $Root rev-parse HEAD 2>&1)
    if ($LASTEXITCODE -ne 0) {
        throw "Impossible de lire HEAD dans $Root : $($headOutput -join [Environment]::NewLine)"
    }
    $statusOutput = @(& git -C $Root status --porcelain=v1 --untracked-files=all 2>&1)
    if ($LASTEXITCODE -ne 0) {
        throw "Impossible d'inventorier la worktree $Root : $($statusOutput -join [Environment]::NewLine)"
    }
    $filteredStatusOutput = @($statusOutput | Where-Object {
        $line = [string]$_
        $path = if ($line.Length -gt 3) { $line.Substring(3) } else { $line }
        if ($path.Contains(' -> ')) {
            $path = $path.Substring($path.LastIndexOf(' -> ') + 4)
        }
        $path = $path.Trim('"')
        -not (Test-IsExcludedRelativePath -RelativePath $path)
    })
    $statusText = ($filteredStatusOutput | ForEach-Object { [string]$_ }) -join "`n"
    return [ordered]@{
        isGit = $true
        head = ([string]$headOutput[0]).Trim()
        statusSha256 = Get-TextSha256 -Text $statusText
        statusLineCount = $filteredStatusOutput.Count
    }
}

function Get-SourceInventory {
    param([Parameter(Mandatory = $true)][string]$Root)

    $files = New-Object Collections.Generic.List[object]
    $directories = New-Object 'Collections.Generic.Stack[IO.DirectoryInfo]'
    $directories.Push((Get-Item -LiteralPath $Root -Force))
    while ($directories.Count -gt 0) {
        $directory = $directories.Pop()
        foreach ($item in Get-ChildItem -LiteralPath $directory.FullName -Force -ErrorAction Stop) {
            $relativePath = $item.FullName.Substring($Root.Length).TrimStart('\', '/').Replace('\', '/')
            if (Test-IsExcludedRelativePath -RelativePath $relativePath) {
                continue
            }
            if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "Lien/reparse point refuse dans l'inventaire: $($item.FullName)"
            }
            if ($item.PSIsContainer) {
                $directories.Push($item)
                continue
            }
            $files.Add([ordered]@{
                path = $relativePath
                length = [int64]$item.Length
                sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $item.FullName).Hash
            })
        }
    }

    $orderedFiles = @($files | Sort-Object { $_.path })
    $treeText = ($orderedFiles | ForEach-Object {
        "$($_.path)`0$($_.length)`0$($_.sha256)"
    }) -join "`n"
    $gitMetadata = Get-GitMetadata -Root $Root

    return [ordered]@{
        root = $Root
        treeSha256 = Get-TextSha256 -Text $treeText
        fileCount = $orderedFiles.Count
        files = $orderedFiles
        git = $gitMetadata
    }
}

function Assert-StableInventory {
    param(
        [Parameter(Mandatory = $true)]$Before,
        [Parameter(Mandatory = $true)]$After,
        [Parameter(Mandatory = $true)][string]$Label
    )

    if ($Before.treeSha256 -ne $After.treeSha256 -or
        $Before.git.head -ne $After.git.head -or
        $Before.git.statusSha256 -ne $After.git.statusSha256) {
        throw "$Label a change pendant son inventaire. Arret fail-closed; refaire la fusion et la capture."
    }
}

function Get-InventoryDifferences {
    param(
        [Parameter(Mandatory = $true)]$Expected,
        [Parameter(Mandatory = $true)]$Actual
    )

    $expectedByPath = @{}
    foreach ($entry in $Expected.files) {
        $expectedByPath[[string]$entry.path] = $entry
    }
    $actualByPath = @{}
    foreach ($entry in $Actual.files) {
        $actualByPath[[string]$entry.path] = $entry
    }

    $differences = New-Object Collections.Generic.List[string]
    foreach ($path in @($expectedByPath.Keys | Sort-Object)) {
        if (-not $actualByPath.ContainsKey($path)) {
            $differences.Add("supprime: $path")
        } elseif ($expectedByPath[$path].sha256 -ne $actualByPath[$path].sha256 -or
            [int64]$expectedByPath[$path].length -ne [int64]$actualByPath[$path].length) {
            $differences.Add("modifie: $path")
        }
    }
    foreach ($path in @($actualByPath.Keys | Sort-Object)) {
        if (-not $expectedByPath.ContainsKey($path)) {
            $differences.Add("ajoute: $path")
        }
    }
    return @($differences | Select-Object -First 30)
}

function Assert-MatchesManifestInventory {
    param(
        [Parameter(Mandatory = $true)]$Expected,
        [Parameter(Mandatory = $true)]$Actual,
        [Parameter(Mandatory = $true)][string]$Label
    )

    $matches = $Expected.treeSha256 -eq $Actual.treeSha256 -and
        $Expected.git.head -eq $Actual.git.head -and
        $Expected.git.statusSha256 -eq $Actual.git.statusSha256
    if ($matches) {
        return
    }

    $details = @(Get-InventoryDifferences -Expected $Expected -Actual $Actual)
    $detailText = if ($details.Count -gt 0) {
        [Environment]::NewLine + ($details -join [Environment]::NewLine)
    } else {
        [Environment]::NewLine + 'HEAD ou statut Git divergent.'
    }
    throw "$Label a derive depuis la fusion acceptee.$detailText`nBuild interdit; conserver puis fusionner ces changements dans un snapshot neuf."
}

$canonicalRoot = Resolve-DirectoryPath -Path $CanonicalSourceRoot
$snapshotRoot = Resolve-DirectoryPath -Path $BuildSnapshotRoot
if ($canonicalRoot -eq $snapshotRoot) {
    throw 'La source canonique et le snapshot de build doivent etre deux dossiers distincts.'
}
if ($Target -eq 'AzureVps' -and -not $UserAuthorizedVpsDeployment) {
    throw 'Cible VPS refusee: fournir une autorisation utilisateur courante, explicite et non ambigue.'
}

if ($Mode -eq 'Report') {
    $canonicalInventory = Get-SourceInventory -Root $canonicalRoot
    $snapshotInventory = Get-SourceInventory -Root $snapshotRoot
    [pscustomobject]@{
        target = $Target
        canonicalTreeSha256 = $canonicalInventory.treeSha256
        canonicalFiles = $canonicalInventory.fileCount
        canonicalGitChanges = $canonicalInventory.git.statusLineCount
        snapshotTreeSha256 = $snapshotInventory.treeSha256
        snapshotFiles = $snapshotInventory.fileCount
        snapshotGitChanges = $snapshotInventory.git.statusLineCount
    }
    return
}

if ([string]::IsNullOrWhiteSpace($ManifestPath)) {
    throw 'ManifestPath est obligatoire en mode Capture ou Verify.'
}
$manifestFullPath = [IO.Path]::GetFullPath($ManifestPath)
$manifestDirectory = Split-Path -Parent $manifestFullPath
if (-not (Test-Path -LiteralPath $manifestDirectory -PathType Container)) {
    throw "Le dossier du manifeste n'existe pas: $manifestDirectory"
}
foreach ($root in @($canonicalRoot, $snapshotRoot)) {
    $rootPrefix = $root.TrimEnd('\') + '\'
    if ($manifestFullPath.StartsWith($rootPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Le manifeste doit etre stocke hors de la source et du snapshot afin de ne pas modifier leur hash.'
    }
}

if ($Mode -eq 'Capture') {
    if (-not $ConfirmAllConcurrentChangesMerged) {
        throw 'Capture refusee: confirmer explicitement que toutes les modifications concurrentes ont ete fusionnees.'
    }
    $canonicalBefore = Get-SourceInventory -Root $canonicalRoot
    $snapshotBefore = Get-SourceInventory -Root $snapshotRoot
    $canonicalAfter = Get-SourceInventory -Root $canonicalRoot
    $snapshotAfter = Get-SourceInventory -Root $snapshotRoot
    Assert-StableInventory -Before $canonicalBefore -After $canonicalAfter -Label 'La source canonique'
    Assert-StableInventory -Before $snapshotBefore -After $snapshotAfter -Label 'Le snapshot de build'
    if ($canonicalAfter.treeSha256 -ne $snapshotAfter.treeSha256 -or
        $canonicalAfter.fileCount -ne $snapshotAfter.fileCount) {
        $details = @(Get-InventoryDifferences -Expected $canonicalAfter -Actual $snapshotAfter)
        $detailText = if ($details.Count -gt 0) {
            [Environment]::NewLine + ($details -join [Environment]::NewLine)
        } else {
            ''
        }
        throw "Capture refusee: le snapshot n'est pas une copie exacte de la source canonique.$detailText"
    }

    $manifest = [ordered]@{
        schemaVersion = 1
        capturedAtUtc = [DateTime]::UtcNow.ToString('o')
        target = $Target
        vpsDeploymentExplicitlyAuthorized = [bool]$UserAuthorizedVpsDeployment
        concurrentChangesMerged = $true
        canonical = $canonicalAfter
        snapshot = $snapshotAfter
    }
    $json = $manifest | ConvertTo-Json -Depth 8
    $temporaryPath = "$manifestFullPath.tmp-$PID-$([Guid]::NewGuid().ToString('N'))"
    $utf8NoBom = New-Object Text.UTF8Encoding($false)
    try {
        [IO.File]::WriteAllText($temporaryPath, $json + [Environment]::NewLine, $utf8NoBom)
        Move-Item -LiteralPath $temporaryPath -Destination $manifestFullPath -Force
    } finally {
        if (Test-Path -LiteralPath $temporaryPath) {
            Remove-Item -LiteralPath $temporaryPath -Force
        }
    }
    Write-Output "Manifest capture: $manifestFullPath"
    Write-Output "Canonical: $($canonicalAfter.treeSha256)"
    Write-Output "Snapshot:  $($snapshotAfter.treeSha256)"
    return
}

if (-not (Test-Path -LiteralPath $manifestFullPath -PathType Leaf)) {
    throw "Manifeste absent: $manifestFullPath"
}
$manifest = Get-Content -LiteralPath $manifestFullPath -Raw | ConvertFrom-Json
if ([int]$manifest.schemaVersion -ne 1 -or $manifest.concurrentChangesMerged -ne $true) {
    throw 'Manifeste invalide ou sans attestation de fusion des changements concurrents.'
}
if ([string]$manifest.target -ne $Target) {
    throw "Cible du manifeste incoherente: $($manifest.target) au lieu de $Target."
}
if ($Target -eq 'AzureVps' -and $manifest.vpsDeploymentExplicitlyAuthorized -ne $true) {
    throw 'Le manifeste ne contient aucune autorisation explicite de deploiement VPS.'
}
if ([string]$manifest.canonical.root -ne $canonicalRoot -or
    [string]$manifest.snapshot.root -ne $snapshotRoot) {
    throw 'Les chemins source/snapshot ne correspondent pas au manifeste.'
}

$canonicalCurrent = Get-SourceInventory -Root $canonicalRoot
$snapshotCurrent = Get-SourceInventory -Root $snapshotRoot
Assert-MatchesManifestInventory -Expected $manifest.canonical -Actual $canonicalCurrent -Label 'La source canonique'
Assert-MatchesManifestInventory -Expected $manifest.snapshot -Actual $snapshotCurrent -Label 'Le snapshot de build'
Write-Output 'PREBUILD_SOURCE_GUARD=OK'
Write-Output "Canonical: $($canonicalCurrent.treeSha256)"
Write-Output "Snapshot:  $($snapshotCurrent.treeSha256)"
