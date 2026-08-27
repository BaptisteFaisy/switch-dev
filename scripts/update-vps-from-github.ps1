[CmdletBinding()]
param(
  [ValidatePattern('^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$')]
  [string]$Profile = "azure",
  [string]$Repository = "",
  [ValidatePattern('^[A-Za-z0-9][A-Za-z0-9._/-]{0,199}$')]
  [string]$Ref = "main",
  [ValidatePattern('^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$')]
  [string]$WslDistribution = "Ubuntu",
  [switch]$CheckOnly,
  [switch]$ReplaceDirtyDeployment,
  [switch]$Connect
)

$ErrorActionPreference = "Stop"
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$Root = Split-Path -Parent $ScriptDir
$ProfilePath = Join-Path $env:APPDATA "codex-switch-terminal\vps\$Profile.json"

function Invoke-Capture {
  param(
    [Parameter(Mandatory = $true)][string]$Command,
    [Parameter(Mandatory = $true)][string[]]$Arguments,
    [switch]$AllowFailure
  )
  $previous = $ErrorActionPreference
  try {
    $ErrorActionPreference = "Continue"
    $output = @(& $Command @Arguments 2>&1)
    $exitCode = $LASTEXITCODE
  }
  finally {
    $ErrorActionPreference = $previous
  }
  if ($exitCode -ne 0 -and -not $AllowFailure) {
    throw (($output | ForEach-Object { [string]$_ }) -join [Environment]::NewLine)
  }
  return [pscustomobject]@{
    ExitCode = $exitCode
    Lines = @($output | ForEach-Object { [string]$_ })
  }
}

function Resolve-GitHubRepository {
  param([string]$Value)
  $candidate = $Value.Trim()
  if (-not $candidate) {
    $remote = Invoke-Capture -Command "git" -Arguments @("-C", $Root, "remote", "get-url", "origin")
    $candidate = ($remote.Lines -join "`n").Trim()
  }

  $candidate = $candidate -replace '^git@github\.com:', ''
  $candidate = $candidate -replace '^ssh://git@github\.com/', ''
  $candidate = $candidate -replace '^https://github\.com/', ''
  $candidate = $candidate.Trim().Trim('/') -replace '\.git$', ''
  if ($candidate -notmatch '^[A-Za-z0-9](?:[A-Za-z0-9.-]{0,38})/[A-Za-z0-9._-]{1,100}$') {
    throw "Le depot doit etre un depot GitHub sous la forme proprietaire/depot."
  }
  return $candidate
}

function Resolve-GitHubRevision {
  param(
    [Parameter(Mandatory = $true)][string]$CloneUrl,
    [Parameter(Mandatory = $true)][string]$RequestedRef
  )
  if ($RequestedRef.Contains("..") -or $RequestedRef.Contains("//") -or $RequestedRef.EndsWith("/")) {
    throw "La reference GitHub est invalide."
  }

  foreach ($remoteRef in @("refs/heads/$RequestedRef", "refs/tags/$RequestedRef^{}", "refs/tags/$RequestedRef")) {
    $lookup = Invoke-Capture -Command "git" -Arguments @(
      "ls-remote", "--exit-code", $CloneUrl, $remoteRef
    ) -AllowFailure
    if ($lookup.ExitCode -ne 0) { continue }
    $line = $lookup.Lines | Where-Object { $_ -match '^[0-9a-fA-F]{40}\s' } | Select-Object -First 1
    if ($line) { return ($line -split '\s+')[0].ToLowerInvariant() }
  }
  throw "La branche ou le tag GitHub '$RequestedRef' est introuvable."
}

function Invoke-SshJson {
  param(
    [Parameter(Mandatory = $true)][string[]]$SshArguments,
    [Parameter(Mandatory = $true)][string]$Target,
    [Parameter(Mandatory = $true)][int]$Port
  )
  $result = Invoke-Capture -Command "ssh" -Arguments (
    $SshArguments + @($Target, "curl -fsS --max-time 10 http://127.0.0.1:$Port/healthz")
  )
  try {
    return ($result.Lines -join "`n") | ConvertFrom-Json
  }
  catch {
    throw "La sonde du VPS n'a pas renvoye un JSON valide."
  }
}

function Assert-HealthyNode {
  param(
    [Parameter(Mandatory = $true)][object]$Health,
    [Parameter(Mandatory = $true)][string]$Stage
  )
  if ($Health.ok -ne $true -or $Health.ready -ne $true -or $Health.draining -eq $true) {
    throw "La sonde du VPS n'est pas saine $Stage."
  }
}

function Write-UpdateStatus {
  param(
    [string]$Status,
    [string]$RepositoryName,
    [string]$Revision,
    [string]$CurrentRevision
  )
  Write-Output "CST_GITHUB_UPDATE_STATUS=$Status"
  Write-Output "CST_GITHUB_REPOSITORY=$RepositoryName"
  Write-Output "CST_GITHUB_TARGET_COMMIT=$Revision"
  Write-Output "CST_GITHUB_CURRENT_COMMIT=$CurrentRevision"
}

foreach ($command in @("git", "ssh")) {
  if (-not (Get-Command $command -ErrorAction SilentlyContinue)) {
    throw "Commande requise introuvable: $command"
  }
}
if (-not (Test-Path -LiteralPath $ProfilePath)) {
  throw "Profil VPS introuvable: $Profile"
}

$profileData = Get-Content -Raw -LiteralPath $ProfilePath | ConvertFrom-Json
$sshTarget = [string]$profileData.sshTarget
$sshPort = [int]$profileData.sshPort
$remotePort = [int]$profileData.remotePort
$localPort = [int]$profileData.defaultLocalPort
$identityFile = [string]$profileData.identityFile
$knownHostsFile = [string]$profileData.knownHostsFile
$nodeId = [string]$profileData.id
$nodeLabel = [string]$profileData.label
$workspacePath = [string]$profileData.workspace

if ($sshTarget -notmatch '^[A-Za-z0-9._-]+@[A-Za-z0-9._:-]+$') {
  throw "La cible SSH du profil est invalide."
}
if ($sshPort -lt 1 -or $sshPort -gt 65535 -or $remotePort -lt 1 -or $remotePort -gt 65535) {
  throw "Le profil contient un port invalide."
}
if (-not (Test-Path -LiteralPath $identityFile)) {
  throw "Cle SSH introuvable: $identityFile"
}

$resolvedIdentity = (Resolve-Path -LiteralPath $identityFile).Path
$sshArgs = @(
  "-p", [string]$sshPort,
  "-i", $resolvedIdentity,
  "-o", "BatchMode=yes",
  "-o", "ConnectTimeout=20",
  "-o", "ServerAliveInterval=30",
  "-o", "ServerAliveCountMax=4"
)
if ($knownHostsFile.Trim()) {
  $resolvedKnownHosts = (Resolve-Path -LiteralPath $knownHostsFile).Path
  $sshArgs += @(
    "-o", "UserKnownHostsFile=$resolvedKnownHosts",
    "-o", "StrictHostKeyChecking=yes"
  )
}
else {
  $resolvedKnownHosts = ""
  $sshArgs += @("-o", "StrictHostKeyChecking=accept-new")
}

$repositoryName = Resolve-GitHubRepository -Value $Repository
$cloneUrl = "https://github.com/$repositoryName.git"
$targetRevision = Resolve-GitHubRevision -CloneUrl $cloneUrl -RequestedRef $Ref
$beforeHealth = Invoke-SshJson -SshArguments $sshArgs -Target $sshTarget -Port $remotePort
$currentRevision = [string]$beforeHealth.commit
$capacity = [int]$beforeHealth.capacity
if ($capacity -lt 1) { $capacity = [int]$profileData.capacity }
if ($capacity -lt 1) { $capacity = 1 }

$currentBaseRevision = ""
if ($currentRevision -match '^([0-9a-fA-F]{7,40})(?:-|$)') {
  $currentBaseRevision = $Matches[1].ToLowerInvariant()
}
$sameRevision = $currentBaseRevision -and $targetRevision.StartsWith($currentBaseRevision)
$dirtyDeployment = $currentRevision -match '-dirty-'
$status = if ($sameRevision -and -not $dirtyDeployment) {
  "up_to_date"
}
elseif ($dirtyDeployment) {
  "blocked_unpublished_changes"
}
else {
  "update_available"
}

if ($CheckOnly) {
  Write-UpdateStatus -Status $status -RepositoryName $repositoryName -Revision $targetRevision -CurrentRevision $currentRevision
  return
}
if ($status -eq "up_to_date") {
  Write-Host "Le VPS utilise deja la revision GitHub $targetRevision." -ForegroundColor Green
  return
}
if ($dirtyDeployment -and -not $ReplaceDirtyDeployment) {
  throw "Le VPS utilise une image construite avec des changements non publies ($currentRevision). Pousse d'abord ces changements sur GitHub, verifie-les, puis relance une seule fois avec -ReplaceDirtyDeployment."
}

$tempRoot = Join-Path $env:TEMP ("cst-github-update-" + [guid]::NewGuid().ToString("N"))
$sourceRoot = Join-Path $tempRoot "source"
New-Item -ItemType Directory -Path $tempRoot -Force | Out-Null
try {
  Write-Host "Recuperation de $repositoryName@$targetRevision depuis GitHub..." -ForegroundColor Cyan
  Invoke-Capture -Command "git" -Arguments @(
    "clone", "--quiet", "--no-checkout", $cloneUrl, $sourceRoot
  ) | Out-Null
  Invoke-Capture -Command "git" -Arguments @(
    "-C", $sourceRoot, "fetch", "--quiet", "--depth", "1", "origin", $targetRevision
  ) | Out-Null
  Invoke-Capture -Command "git" -Arguments @(
    "-C", $sourceRoot, "checkout", "--quiet", "--detach", $targetRevision
  ) | Out-Null
  $checkedOut = Invoke-Capture -Command "git" -Arguments @("-C", $sourceRoot, "rev-parse", "HEAD")
  if (($checkedOut.Lines -join "").Trim().ToLowerInvariant() -ne $targetRevision) {
    throw "La revision clonee ne correspond pas a la revision resolue."
  }

  $deployScript = Join-Path $sourceRoot "scripts\deploy-vps-ansible.ps1"
  foreach ($required in @("Dockerfile", "deploy\ansible\playbook.yml", "scripts\deploy-vps-ansible.ps1")) {
    if (-not (Test-Path -LiteralPath (Join-Path $sourceRoot $required))) {
      throw "La revision GitHub ne contient pas le deploiement portable requis: $required"
    }
  }

  $workspaceName = ($workspacePath.TrimEnd('/') -split '/')[-1]
  if ($workspaceName -notmatch '^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$') {
    throw "Le nom du workspace du profil est invalide."
  }
  $deployArgs = @{
    SshTarget = $sshTarget
    SshPort = $sshPort
    IdentityFile = $resolvedIdentity
    RemotePort = $remotePort
    LocalPort = $localPort
    NodeId = $nodeId
    NodeLabel = $nodeLabel
    Capacity = $capacity
    WorkspaceName = $workspaceName
    WslDistribution = $WslDistribution
    SkipAccountSeed = $true
    NoWorkspaceSeed = $true
  }
  if ($resolvedKnownHosts) {
    $deployArgs.KnownHostsFile = $resolvedKnownHosts
  }
  else {
    $deployArgs.AcceptNewHostKey = $true
  }
  if ($Connect) { $deployArgs.Connect = $true }

  Write-Host "Mise a jour du profil '$Profile' depuis GitHub..." -ForegroundColor Cyan
  & $deployScript @deployArgs
  if ($LASTEXITCODE -ne 0) {
    throw "Le deploiement GitHub a echoue (code $LASTEXITCODE)."
  }

  $afterHealth = Invoke-SshJson -SshArguments $sshArgs -Target $sshTarget -Port $remotePort
  Assert-HealthyNode -Health $afterHealth -Stage "apres la mise a jour"
  $deployedRevision = [string]$afterHealth.commit
  if (-not $targetRevision.StartsWith(($deployedRevision -split '-')[0])) {
    throw "Le VPS est sain, mais sa revision $deployedRevision ne correspond pas a GitHub $targetRevision."
  }

  $updatedProfile = Get-Content -Raw -LiteralPath $ProfilePath | ConvertFrom-Json
  $updatedProfile | Add-Member -NotePropertyName capacity -NotePropertyValue $capacity -Force
  $updatedProfile | Add-Member -NotePropertyName source -NotePropertyValue ([ordered]@{
    type = "github"
    repository = $repositoryName
    ref = $Ref
    commit = $targetRevision
  }) -Force
  $updatedProfile | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $ProfilePath -Encoding UTF8

  Write-Host "VPS mis a jour depuis GitHub et sonde de sante validee." -ForegroundColor Green
  Write-UpdateStatus -Status "updated" -RepositoryName $repositoryName -Revision $targetRevision -CurrentRevision $deployedRevision
}
finally {
  $tempBase = [IO.Path]::GetFullPath($env:TEMP).TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
  $resolvedTempRoot = [IO.Path]::GetFullPath($tempRoot)
  if (
    $resolvedTempRoot.StartsWith($tempBase, [StringComparison]::OrdinalIgnoreCase) -and
    (Split-Path -Leaf $resolvedTempRoot).StartsWith("cst-github-update-", [StringComparison]::Ordinal)
  ) {
    Remove-Item -LiteralPath $resolvedTempRoot -Recurse -Force -ErrorAction SilentlyContinue
  }
}
