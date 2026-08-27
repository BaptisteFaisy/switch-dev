[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$canonicalSourceRoot = $env:CST_CANONICAL_SOURCE_ROOT
$manifestPath = $env:CST_BUILD_SOURCE_MANIFEST
$snapshotRoot = if ([string]::IsNullOrWhiteSpace($env:CST_BUILD_SNAPSHOT_ROOT)) {
    Split-Path -Parent $PSScriptRoot
} else {
    $env:CST_BUILD_SNAPSHOT_ROOT
}
$target = if ([string]::IsNullOrWhiteSpace($env:CST_BUILD_TARGET)) {
    'SwitchDevelopment'
} else {
    $env:CST_BUILD_TARGET
}

if ([string]::IsNullOrWhiteSpace($canonicalSourceRoot)) {
    throw 'Build refuse: CST_CANONICAL_SOURCE_ROOT doit designer la source vivante inventorisee.'
}
if ([string]::IsNullOrWhiteSpace($manifestPath)) {
    throw 'Build refuse: CST_BUILD_SOURCE_MANIFEST doit designer le manifeste cree apres la fusion revue.'
}
if ($target -notin @('SwitchDevelopment', 'AzureVps')) {
    throw "Build refuse: CST_BUILD_TARGET invalide ($target)."
}

$guardPath = Join-Path $PSScriptRoot 'switch-build-source-guard.ps1'
if (-not (Test-Path -LiteralPath $guardPath -PathType Leaf)) {
    throw "Build refuse: garde absente ($guardPath)."
}

$guardParameters = @{
    Mode = 'Verify'
    CanonicalSourceRoot = $canonicalSourceRoot
    BuildSnapshotRoot = $snapshotRoot
    ManifestPath = $manifestPath
    Target = $target
}
if ($target -eq 'AzureVps') {
    if ($env:CST_VPS_DEPLOYMENT_AUTHORIZATION -ne
        'I_EXPLICITLY_AUTHORIZE_AZURE_VPS_DEPLOYMENT') {
        throw 'Build VPS refuse: autorisation utilisateur courante et non ambigue absente.'
    }
    $guardParameters.UserAuthorizedVpsDeployment = $true
}

& $guardPath @guardParameters
Write-Output 'NPM_PREBUILD_SOURCE_GUARD=OK'
