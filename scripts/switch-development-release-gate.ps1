[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [ValidateSet(
    "Initialize",
    "Status",
    "AuditPackage",
    "AuditActive",
    "PrepareCandidate",
    "RecordRuntime"
  )]
  [string]$Operation,

  [string]$DeploymentRoot,
  [string]$CandidateRoot,
  [string]$RuntimeReleasePath,

  [ValidateRange(202001010000, 999912312359)]
  [long]$Generation,

  [ValidateSet("frontend", "backend", "mixed")]
  [string]$ChangeKind,

  [switch]$ConfirmAllConcurrentChangesMerged
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$appRoot = Split-Path -Parent $scriptRoot
if ([string]::IsNullOrWhiteSpace($DeploymentRoot)) {
  $DeploymentRoot = Split-Path -Parent $appRoot
}
$deploymentRootFull = [IO.Path]::GetFullPath($DeploymentRoot).TrimEnd('\')
$expectedAppRoot = [IO.Path]::GetFullPath((Join-Path $deploymentRootFull "app")).TrimEnd('\')
if (-not $appRoot.Equals($expectedAppRoot, [StringComparison]::OrdinalIgnoreCase)) {
  throw "SWITCH_DEVELOPMENT_RELEASE_GATE_REFUSED reason=unexpected_app_root:$appRoot"
}
if ([string]::IsNullOrWhiteSpace($CandidateRoot)) {
  $CandidateRoot = $appRoot
}
$candidateRootFull = [IO.Path]::GetFullPath($CandidateRoot).TrimEnd('\')
$guardRoot = Join-Path $deploymentRootFull ".guard\release-gate"
$policyPath = Join-Path $scriptRoot "switch-development-release-policy.json"
$statePath = Join-Path $guardRoot "state.json"
$candidateManifestPath = Join-Path $guardRoot "candidate.json"
$runtimeRoot = Join-Path $env:LOCALAPPDATA "SwitchDevelopmentRuntime"
$runtimeReleasesRoot = Join-Path $runtimeRoot "releases"
$baseUrl = "http://127.0.0.1:18082"

function Refuse {
  param([Parameter(Mandatory = $true)][string]$Reason)
  throw "SWITCH_DEVELOPMENT_RELEASE_GATE_REFUSED reason=$Reason"
}

function Get-Sha256 {
  param([Parameter(Mandatory = $true)][string]$Path)
  if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
    Refuse -Reason "required_file_missing:$Path"
  }
  return (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToUpperInvariant()
}

function Write-JsonAtomic {
  param(
    [Parameter(Mandatory = $true)]$Value,
    [Parameter(Mandatory = $true)][string]$Path
  )
  $directory = Split-Path -Parent $Path
  New-Item -ItemType Directory -Path $directory -Force | Out-Null
  $temporary = "$Path.next-$PID-$([Guid]::NewGuid().ToString('N'))"
  $utf8NoBom = New-Object Text.UTF8Encoding($false)
  try {
    $json = $Value | ConvertTo-Json -Depth 12
    [IO.File]::WriteAllText($temporary, $json + [Environment]::NewLine, $utf8NoBom)
    if (Test-Path -LiteralPath $Path -PathType Leaf) {
      [IO.File]::Replace($temporary, $Path, $null)
    } else {
      Move-Item -LiteralPath $temporary -Destination $Path
    }
  } finally {
    if (Test-Path -LiteralPath $temporary) {
      Remove-Item -LiteralPath $temporary -Force
    }
  }
}

function Read-Json {
  param([Parameter(Mandatory = $true)][string]$Path)
  if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
    Refuse -Reason "json_missing:$Path"
  }
  try {
    return Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
  } catch {
    Refuse -Reason "json_invalid:$Path"
  }
}

function Get-Policy {
  $policy = Read-Json -Path $policyPath
  if ([int]$policy.schemaVersion -ne 1) {
    Refuse -Reason "policy_schema_invalid"
  }
  return $policy
}

function Get-State {
  $state = Read-Json -Path $statePath
  if ([int]$state.schemaVersion -ne 1) {
    Refuse -Reason "state_schema_invalid"
  }
  return $state
}

function Get-GitHead {
  $output = @(& git -C $appRoot rev-parse HEAD 2>&1)
  if ($LASTEXITCODE -ne 0 -or $output.Count -ne 1) {
    Refuse -Reason "git_head_unavailable"
  }
  return ([string]$output[0]).Trim().ToLowerInvariant()
}

function Assert-GitDescendant {
  param(
    [Parameter(Mandatory = $true)][string]$Ancestor,
    [Parameter(Mandatory = $true)][string]$Descendant
  )
  & git -C $appRoot merge-base --is-ancestor $Ancestor $Descendant 2>$null
  if ($LASTEXITCODE -ne 0) {
    Refuse -Reason "source_commit_not_descendant:$Descendant"
  }
}

function Assert-SourceProofs {
  param([Parameter(Mandatory = $true)]$Policy)
  foreach ($proof in $Policy.requiredSourceProofs) {
    $relative = ([string]$proof.path).Replace('/', '\')
    $path = Join-Path $appRoot $relative
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
      Refuse -Reason "required_source_file_missing:$($proof.id)"
    }
    $text = Get-Content -LiteralPath $path -Raw
    if ($text.IndexOf([string]$proof.text, [StringComparison]::Ordinal) -lt 0) {
      Refuse -Reason "required_source_proof_missing:$($proof.id)"
    }
  }
}

function Get-BuildIdFromIndex {
  param([Parameter(Mandatory = $true)][string]$IndexText)
  $match = [regex]::Match(
    $IndexText,
    '<meta\s+name=["'']cst-build-id["'']\s+content=["'']([^"'']+)["'']',
    [Text.RegularExpressions.RegexOptions]::IgnoreCase
  )
  if (-not $match.Success) {
    Refuse -Reason "build_id_missing"
  }
  return $match.Groups[1].Value
}

function Get-EntryAssetFromIndex {
  param([Parameter(Mandatory = $true)][string]$IndexText)
  $matches = [regex]::Matches(
    $IndexText,
    '["'']/?(assets/index-[^/"'']+\.js)["'']',
    [Text.RegularExpressions.RegexOptions]::IgnoreCase
  )
  $assets = @($matches | ForEach-Object { $_.Groups[1].Value } | Sort-Object -Unique)
  if ($assets.Count -ne 1) {
    Refuse -Reason "single_entry_asset_not_found"
  }
  return [string]$assets[0]
}

function Get-PackageRecord {
  param([Parameter(Mandatory = $true)][string]$Root)
  $rootFull = [IO.Path]::GetFullPath($Root).TrimEnd('\')
  $serverPath = Join-Path $rootFull "cst-server.exe"
  $indexPath = Join-Path $rootFull "dist\index.html"
  $workerPath = Join-Path $rootFull "dist\service-worker.js"
  foreach ($path in @($serverPath, $indexPath, $workerPath)) {
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
      Refuse -Reason "package_incomplete:$path"
    }
  }
  $indexText = Get-Content -LiteralPath $indexPath -Raw
  $entryAsset = Get-EntryAssetFromIndex -IndexText $indexText
  $entryPath = Join-Path (Join-Path $rootFull "dist") $entryAsset.Replace('/', '\')
  if (-not (Test-Path -LiteralPath $entryPath -PathType Leaf)) {
    Refuse -Reason "entry_asset_missing:$entryAsset"
  }
  $serverSha = Get-Sha256 -Path $serverPath
  $indexSha = Get-Sha256 -Path $indexPath
  $versionOutput = @(& $serverPath --version 2>&1)
  if ($LASTEXITCODE -ne 0 -or $versionOutput.Count -ne 1) {
    Refuse -Reason "server_version_unavailable"
  }
  return [ordered]@{
    releaseId = $serverSha.Substring(0, 16) + "-" + $indexSha.Substring(0, 16)
    buildId = Get-BuildIdFromIndex -IndexText $indexText
    serverSha256 = $serverSha
    indexSha256 = $indexSha
    serviceWorkerSha256 = Get-Sha256 -Path $workerPath
    entryAsset = $entryAsset
    entrySha256 = Get-Sha256 -Path $entryPath
    serverVersion = ([string]$versionOutput[0]).Trim()
  }
}

function Test-PackageEqual {
  param(
    [Parameter(Mandatory = $true)]$Left,
    [Parameter(Mandatory = $true)]$Right
  )
  foreach ($field in @(
    "releaseId",
    "buildId",
    "serverSha256",
    "indexSha256",
    "serviceWorkerSha256",
    "entryAsset",
    "entrySha256",
    "serverVersion"
  )) {
    if ([string]$Left.$field -ne [string]$Right.$field) {
      return $false
    }
  }
  return $true
}

function Assert-PackageEqual {
  param(
    [Parameter(Mandatory = $true)]$Expected,
    [Parameter(Mandatory = $true)]$Actual,
    [Parameter(Mandatory = $true)][string]$Reason
  )
  if (-not (Test-PackageEqual -Left $Expected -Right $Actual)) {
    Refuse -Reason $Reason
  }
}

function Get-PolicySha256 {
  return Get-Sha256 -Path $policyPath
}

function Assert-CandidateManifest {
  param(
    [Parameter(Mandatory = $true)]$Manifest,
    [Parameter(Mandatory = $true)]$State,
    [Parameter(Mandatory = $true)]$Package,
    [Parameter(Mandatory = $true)][string]$Head
  )
  if ([int]$Manifest.schemaVersion -ne 1) {
    Refuse -Reason "candidate_schema_invalid"
  }
  $generationValue = [long]$Manifest.generation
  if ($generationValue -le [long]$State.acceptedGeneration) {
    Refuse -Reason "candidate_generation_not_newer"
  }
  if ([long]$Manifest.parentGeneration -ne [long]$State.acceptedGeneration) {
    Refuse -Reason "candidate_parent_generation_mismatch"
  }
  if ([string]$Manifest.parentReleaseId -ne [string]$State.acceptedPackage.releaseId) {
    Refuse -Reason "candidate_parent_release_mismatch"
  }
  if ([string]$Manifest.sourceCommit -ne $Head) {
    Refuse -Reason "candidate_source_commit_mismatch"
  }
  Assert-GitDescendant -Ancestor ([string]$State.acceptedSourceCommit) -Descendant $Head
  if ([string]$Manifest.policySha256 -ne (Get-PolicySha256)) {
    Refuse -Reason "candidate_policy_mismatch"
  }
  if ([string]$Manifest.changeKind -notin @("frontend", "backend", "mixed")) {
    Refuse -Reason "candidate_change_kind_invalid"
  }
  Assert-PackageEqual -Expected $Manifest.package -Actual $Package -Reason "candidate_package_hash_mismatch"

  $serverChanged = [string]$Package.serverSha256 -ne [string]$State.acceptedPackage.serverSha256
  $indexChanged = [string]$Package.indexSha256 -ne [string]$State.acceptedPackage.indexSha256
  switch ([string]$Manifest.changeKind) {
    "frontend" {
      if ($serverChanged -or -not $indexChanged) {
        Refuse -Reason "frontend_candidate_change_set_invalid"
      }
    }
    "backend" {
      if (-not $serverChanged -or $indexChanged) {
        Refuse -Reason "backend_candidate_change_set_invalid"
      }
    }
    "mixed" {
      if (-not $serverChanged -or -not $indexChanged) {
        Refuse -Reason "mixed_candidate_change_set_invalid"
      }
    }
  }
  if ($indexChanged -and [string]$Package.buildId -eq [string]$State.acceptedPackage.buildId) {
    Refuse -Reason "frontend_build_id_not_advanced"
  }
  if ($indexChanged) {
    $generationSuffix = $generationValue.ToString([Globalization.CultureInfo]::InvariantCulture)
    if (-not ([string]$Package.buildId).EndsWith($generationSuffix, [StringComparison]::Ordinal)) {
      Refuse -Reason "frontend_build_id_must_end_with_generation:$generationSuffix"
    }
  }
  if ($serverChanged) {
    $serverCommitMatch = [regex]::Match([string]$Package.serverVersion, '\(([^)]+)\)$')
    if (-not $serverCommitMatch.Success -or
        $serverCommitMatch.Groups[1].Value.ToLowerInvariant() -ne $Head) {
      Refuse -Reason "candidate_server_not_built_from_source_commit"
    }
  }
}

function Assert-PackageAcceptedOrCandidate {
  param(
    [Parameter(Mandatory = $true)]$State,
    [Parameter(Mandatory = $true)]$Package,
    [Parameter(Mandatory = $true)][string]$Head
  )
  Assert-GitDescendant -Ancestor ([string]$State.acceptedSourceCommit) -Descendant $Head
  if (Test-PackageEqual -Left $State.acceptedPackage -Right $Package) {
    return "accepted"
  }
  $manifest = Read-Json -Path $candidateManifestPath
  Assert-CandidateManifest -Manifest $manifest -State $State -Package $Package -Head $Head
  return "candidate"
}

function Get-ActiveProcess {
  $listener = Get-NetTCPConnection -State Listen -LocalPort 18082 -ErrorAction SilentlyContinue |
    Select-Object -First 1
  if (-not $listener) {
    Refuse -Reason "development_runtime_not_listening"
  }
  $process = Get-CimInstance Win32_Process -Filter "ProcessId=$([int]$listener.OwningProcess)"
  if (-not $process -or -not [string]$process.ExecutablePath) {
    Refuse -Reason "development_runtime_process_unknown"
  }
  return $process
}

function Get-ValidatedRuntimeReleasePath {
  param([Parameter(Mandatory = $true)][string]$Path)
  $full = [IO.Path]::GetFullPath($Path).TrimEnd('\')
  $rootPrefix = [IO.Path]::GetFullPath($runtimeReleasesRoot).TrimEnd('\') + '\'
  if (-not $full.StartsWith($rootPrefix, [StringComparison]::OrdinalIgnoreCase) -or
      (Split-Path -Parent $full) -ne $rootPrefix.TrimEnd('\')) {
    Refuse -Reason "runtime_release_outside_cache:$full"
  }
  if (-not (Test-Path -LiteralPath $full -PathType Container)) {
    Refuse -Reason "runtime_release_missing:$full"
  }
  return $full
}

function Get-ServedIndexSha256 {
  $temporary = Join-Path $env:TEMP ("switch-release-gate-index-" + [Guid]::NewGuid().ToString("N") + ".html")
  try {
    Invoke-WebRequest -UseBasicParsing -Uri "$baseUrl/" -OutFile $temporary -TimeoutSec 5
    return Get-Sha256 -Path $temporary
  } finally {
    if (Test-Path -LiteralPath $temporary) {
      Remove-Item -LiteralPath $temporary -Force
    }
  }
}

function Assert-ActiveRuntime {
  param(
    [Parameter(Mandatory = $true)]$ExpectedPackage,
    [Parameter(Mandatory = $true)][string]$ExpectedRuntimePath
  )
  $runtimePath = Get-ValidatedRuntimeReleasePath -Path $ExpectedRuntimePath
  $runtimePackage = Get-PackageRecord -Root $runtimePath
  Assert-PackageEqual -Expected $ExpectedPackage -Actual $runtimePackage -Reason "runtime_package_mismatch"
  $process = Get-ActiveProcess
  $expectedExecutable = [IO.Path]::GetFullPath((Join-Path $runtimePath "cst-server.exe"))
  $actualExecutable = [IO.Path]::GetFullPath([string]$process.ExecutablePath)
  if (-not $expectedExecutable.Equals($actualExecutable, [StringComparison]::OrdinalIgnoreCase)) {
    Refuse -Reason "active_runtime_release_mismatch:$actualExecutable"
  }
  try {
    $health = Invoke-RestMethod -Uri "$baseUrl/healthz" -TimeoutSec 5
  } catch {
    Refuse -Reason "active_health_unavailable"
  }
  if ($health.ok -ne $true -or $health.ready -ne $true -or $health.draining -ne $false) {
    Refuse -Reason "active_runtime_not_ready"
  }
  $versionMatch = [regex]::Match([string]$ExpectedPackage.serverVersion, '\(([^)]+)\)$')
  if (-not $versionMatch.Success -or [string]$health.commit -ne $versionMatch.Groups[1].Value) {
    Refuse -Reason "active_backend_commit_mismatch"
  }
  if ((Get-ServedIndexSha256) -ne [string]$ExpectedPackage.indexSha256) {
    Refuse -Reason "served_index_hash_mismatch"
  }
}

function Get-BlockingGitChanges {
  $status = @(& git -C $appRoot status --porcelain=v1 --untracked-files=all 2>&1)
  if ($LASTEXITCODE -ne 0) {
    Refuse -Reason "git_status_unavailable"
  }
  return @($status | Where-Object {
    $line = [string]$_
    if ($line -match '^\?\? \.backup-ssd-root/') { return $false }
    return -not [string]::IsNullOrWhiteSpace($line)
  })
}

$gateMutex = [Threading.Mutex]::new($false, "Local\SwitchDevelopmentReleaseGate-18082")
$gateLockHeld = $false
try {
  try {
    $gateLockHeld = $gateMutex.WaitOne([TimeSpan]::FromSeconds(30))
  } catch [Threading.AbandonedMutexException] {
    $gateLockHeld = $true
  }
  if (-not $gateLockHeld) {
    Refuse -Reason "gate_lock_busy"
  }

  $policy = Get-Policy
  Assert-SourceProofs -Policy $policy
  $head = Get-GitHead

  switch ($Operation) {
    "Initialize" {
      if (Test-Path -LiteralPath $statePath) {
        Refuse -Reason "state_already_initialized"
      }
      if ($head -ne ([string]$policy.baselineSourceCommit).ToLowerInvariant()) {
        Refuse -Reason "baseline_source_commit_mismatch:$head"
      }
      $package = Get-PackageRecord -Root $candidateRootFull
      Assert-PackageEqual -Expected $policy.baselinePackage -Actual $package -Reason "baseline_package_mismatch"
      $initialRuntimePath = Join-Path $runtimeReleasesRoot ([string]$package.releaseId)
      Assert-ActiveRuntime -ExpectedPackage $package -ExpectedRuntimePath $initialRuntimePath
      $state = [ordered]@{
        schemaVersion = 1
        acceptedGeneration = [long]$policy.minimumGeneration
        acceptedSourceCommit = $head
        acceptedRuntimeRelease = $initialRuntimePath
        acceptedPackage = $package
        recordedAtUtc = [DateTime]::UtcNow.ToString("o")
      }
      Write-JsonAtomic -Value $state -Path $statePath
      Write-Output "SWITCH_DEVELOPMENT_RELEASE_GATE_OK operation=Initialize generation=$($state.acceptedGeneration) release=$($package.releaseId)"
    }
    "Status" {
      $state = Get-State
      [pscustomobject]@{
        acceptedGeneration = [long]$state.acceptedGeneration
        acceptedSourceCommit = [string]$state.acceptedSourceCommit
        acceptedReleaseId = [string]$state.acceptedPackage.releaseId
        acceptedBuildId = [string]$state.acceptedPackage.buildId
        acceptedRuntimeRelease = [string]$state.acceptedRuntimeRelease
        candidatePending = Test-Path -LiteralPath $candidateManifestPath
        policySha256 = Get-PolicySha256
      }
    }
    "AuditPackage" {
      $state = Get-State
      $package = Get-PackageRecord -Root $candidateRootFull
      $kind = Assert-PackageAcceptedOrCandidate -State $state -Package $package -Head $head
      Write-Output "SWITCH_DEVELOPMENT_RELEASE_GATE_OK operation=AuditPackage kind=$kind release=$($package.releaseId) build=$($package.buildId)"
    }
    "AuditActive" {
      $state = Get-State
      Assert-ActiveRuntime -ExpectedPackage $state.acceptedPackage -ExpectedRuntimePath ([string]$state.acceptedRuntimeRelease)
      Write-Output "SWITCH_DEVELOPMENT_RELEASE_GATE_OK operation=AuditActive generation=$($state.acceptedGeneration) release=$($state.acceptedPackage.releaseId)"
    }
    "PrepareCandidate" {
      if (-not $ConfirmAllConcurrentChangesMerged) {
        Refuse -Reason "concurrent_changes_not_confirmed"
      }
      $blocking = @(Get-BlockingGitChanges)
      if ($blocking.Count -gt 0) {
        Refuse -Reason ("git_worktree_not_clean:" + (($blocking | Select-Object -First 5) -join "|"))
      }
      $state = Get-State
      Assert-GitDescendant -Ancestor ([string]$state.acceptedSourceCommit) -Descendant $head
      if ($Generation -le [long]$state.acceptedGeneration) {
        Refuse -Reason "candidate_generation_not_newer"
      }
      if ([string]::IsNullOrWhiteSpace($ChangeKind)) {
        Refuse -Reason "candidate_change_kind_required"
      }
      $package = Get-PackageRecord -Root $candidateRootFull
      if (Test-PackageEqual -Left $state.acceptedPackage -Right $package) {
        Refuse -Reason "candidate_artifact_unchanged"
      }
      $manifest = [ordered]@{
        schemaVersion = 1
        generation = [long]$Generation
        parentGeneration = [long]$state.acceptedGeneration
        parentReleaseId = [string]$state.acceptedPackage.releaseId
        sourceCommit = $head
        changeKind = $ChangeKind
        policySha256 = Get-PolicySha256
        package = $package
        preparedAtUtc = [DateTime]::UtcNow.ToString("o")
      }
      Assert-CandidateManifest -Manifest ([pscustomobject]$manifest) -State $state -Package ([pscustomobject]$package) -Head $head
      Write-JsonAtomic -Value $manifest -Path $candidateManifestPath
      Write-Output "SWITCH_DEVELOPMENT_RELEASE_GATE_OK operation=PrepareCandidate generation=$Generation release=$($package.releaseId)"
    }
    "RecordRuntime" {
      if ([string]::IsNullOrWhiteSpace($RuntimeReleasePath)) {
        Refuse -Reason "runtime_release_path_required"
      }
      $runtimePath = Get-ValidatedRuntimeReleasePath -Path $RuntimeReleasePath
      $state = Get-State
      $package = Get-PackageRecord -Root $runtimePath
      if (Test-PackageEqual -Left $state.acceptedPackage -Right $package) {
        Assert-ActiveRuntime -ExpectedPackage $state.acceptedPackage -ExpectedRuntimePath ([string]$state.acceptedRuntimeRelease)
        Write-Output "SWITCH_DEVELOPMENT_RELEASE_GATE_OK operation=RecordRuntime kind=already-accepted release=$($package.releaseId)"
        break
      }
      $manifest = Read-Json -Path $candidateManifestPath
      Assert-CandidateManifest -Manifest $manifest -State $state -Package $package -Head $head
      Assert-ActiveRuntime -ExpectedPackage $package -ExpectedRuntimePath $runtimePath
      $previousStatePath = Join-Path $guardRoot "state.previous.json"
      Copy-Item -LiteralPath $statePath -Destination $previousStatePath -Force
      $nextState = [ordered]@{
        schemaVersion = 1
        acceptedGeneration = [long]$manifest.generation
        acceptedSourceCommit = $head
        acceptedRuntimeRelease = $runtimePath
        acceptedPackage = $package
        recordedAtUtc = [DateTime]::UtcNow.ToString("o")
      }
      Write-JsonAtomic -Value $nextState -Path $statePath
      $acceptedManifestPath = Join-Path $guardRoot ("accepted-" + [string]$manifest.generation + ".json")
      Move-Item -LiteralPath $candidateManifestPath -Destination $acceptedManifestPath -Force
      Write-Output "SWITCH_DEVELOPMENT_RELEASE_GATE_OK operation=RecordRuntime generation=$($manifest.generation) release=$($package.releaseId)"
    }
  }
} finally {
  if ($gateLockHeld) {
    $gateMutex.ReleaseMutex()
  }
  $gateMutex.Dispose()
}
