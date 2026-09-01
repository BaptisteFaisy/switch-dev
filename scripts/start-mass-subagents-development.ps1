#requires -Version 7.0

[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [string]$DeploymentRoot,

  [switch]$EnableSwitchReadonly,

  [ValidateSet(18084)]
  [int]$Port = 18084
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# PSModulePath : remet les modules Windows PowerShell en tete (Get-FileHash introuvable sinon dans le sandbox).
& (Join-Path $PSScriptRoot 'switch-development-ensure-system-module-path.ps1')

function Get-MassSubagentsInventory {
  param([Parameter(Mandatory = $true)][string]$Root)

  $resolvedRoot = [IO.Path]::GetFullPath($Root)
  $records = @(
    Get-ChildItem -LiteralPath $resolvedRoot -File -Recurse |
      Where-Object { $_.Name -ne ".switch-mass-subagents-release.json" } |
      ForEach-Object {
        $relative = [IO.Path]::GetRelativePath($resolvedRoot, $_.FullName).Replace("\", "/")
        [pscustomobject]@{
          path = $relative
          length = $_.Length
          sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $_.FullName).Hash.ToLowerInvariant()
        }
      } |
      Sort-Object path
  )
  $payload = ($records | ForEach-Object { "$($_.path)`t$($_.length)`t$($_.sha256)" }) -join "`n"
  $algorithm = [Security.Cryptography.SHA256]::Create()
  try {
    $treeHash = [Convert]::ToHexString(
      $algorithm.ComputeHash([Text.Encoding]::UTF8.GetBytes($payload))
    ).ToLowerInvariant()
  } finally {
    $algorithm.Dispose()
  }
  return [pscustomobject]@{ files = $records; treeSha256 = $treeHash }
}

function Set-PrivateFileSystemAcl {
  param(
    [Parameter(Mandatory = $true)][string]$LiteralPath,
    [Parameter(Mandatory = $true)][bool]$Directory
  )

  $identity = [Security.Principal.WindowsIdentity]::GetCurrent().User
  if ($Directory) {
    $acl = [Security.AccessControl.DirectorySecurity]::new()
    $inheritance = [Security.AccessControl.InheritanceFlags]"ContainerInherit, ObjectInherit"
  } else {
    $acl = [Security.AccessControl.FileSecurity]::new()
    $inheritance = [Security.AccessControl.InheritanceFlags]::None
  }
  $acl.SetOwner($identity)
  $acl.SetAccessRuleProtection($true, $false)
  $rule = [Security.AccessControl.FileSystemAccessRule]::new(
    $identity,
    [Security.AccessControl.FileSystemRights]::FullControl,
    $inheritance,
    [Security.AccessControl.PropagationFlags]::None,
    [Security.AccessControl.AccessControlType]::Allow
  )
  [void]$acl.AddAccessRule($rule)
  Set-Acl -LiteralPath $LiteralPath -AclObject $acl
}

function Set-ReadOnlyReleaseAcl {
  param(
    [Parameter(Mandatory = $true)][string]$LiteralPath,
    [Parameter(Mandatory = $true)][bool]$Directory
  )

  $identity = [Security.Principal.WindowsIdentity]::GetCurrent().User
  if ($Directory) {
    $acl = [Security.AccessControl.DirectorySecurity]::new()
    $inheritance = [Security.AccessControl.InheritanceFlags]"ContainerInherit, ObjectInherit"
  } else {
    $acl = [Security.AccessControl.FileSecurity]::new()
    $inheritance = [Security.AccessControl.InheritanceFlags]::None
  }
  $acl.SetOwner($identity)
  $acl.SetAccessRuleProtection($true, $false)
  $rule = [Security.AccessControl.FileSystemAccessRule]::new(
    $identity,
    [Security.AccessControl.FileSystemRights]"ReadAndExecute, Synchronize",
    $inheritance,
    [Security.AccessControl.PropagationFlags]::None,
    [Security.AccessControl.AccessControlType]::Allow
  )
  [void]$acl.AddAccessRule($rule)
  Set-Acl -LiteralPath $LiteralPath -AclObject $acl
}

function Restore-OriginalEnvironment {
  param([Parameter(Mandatory = $true)][hashtable]$Snapshot)

  foreach ($item in @(Get-ChildItem Env:)) {
    if (-not $Snapshot.ContainsKey($item.Name)) {
      Remove-Item -LiteralPath ("Env:" + $item.Name) -ErrorAction SilentlyContinue
    }
  }
  foreach ($name in $Snapshot.Keys) {
    Set-Item -LiteralPath ("Env:" + $name) -Value $Snapshot[$name]
  }
}

function Assert-TrustedSourceState {
  param(
    [Parameter(Mandatory = $true)][string]$AppRoot,
    [Parameter(Mandatory = $true)][string[]]$Paths
  )

  $git = (Get-Command git -ErrorAction Stop).Source
  $head = @(& $git -C $AppRoot rev-parse --verify "HEAD^{commit}" 2>$null)
  if ($LASTEXITCODE -ne 0 -or $head.Count -ne 1 -or $head[0] -notmatch "^[0-9a-f]{40}$") {
    throw "Le commit Git source mass-subagents ne peut pas etre verifie."
  }
  $status = @(& $git -C $AppRoot status --porcelain=v1 --untracked-files=all -- @Paths)
  if ($LASTEXITCODE -ne 0) {
    throw "L'etat Git du paquet mass-subagents ne peut pas etre verifie."
  }
  if ($status.Count -ne 0) {
    throw "Le paquet mass-subagents ou ses lanceurs differe de HEAD; commit dedie obligatoire avant lancement."
  }
}

function Get-EnvironmentDefault {
  param(
    [Parameter(Mandatory = $true)][string]$Name,
    [Parameter(Mandatory = $true)][string]$Fallback
  )

  $value = [Environment]::GetEnvironmentVariable($Name, "Process")
  if ([string]::IsNullOrWhiteSpace($value)) { return $Fallback }
  return $value
}

$originalEnvironment = @{}
foreach ($item in @(Get-ChildItem Env:)) {
  $originalEnvironment[$item.Name] = $item.Value
}

$deploymentRootPath = [IO.Path]::GetFullPath($DeploymentRoot)
$appRoot = Join-Path $deploymentRootPath "app"
$sourceRoot = Join-Path $deploymentRootPath "app\sidecar\mass-subagents"
$configurationPath = Join-Path $deploymentRootPath "config\server.local.env.ps1"
$verificationScript = Join-Path $deploymentRootPath "app\scripts\verify-mass-subagents-development.ps1"
$mutex = [Threading.Mutex]::new(
  $false,
  ("Local\SwitchDevelopmentMassSubagents-" + $Port)
)
$mutexHeld = $false
$deployMutex = [Threading.Mutex]::new($false, "Local\SwitchDevelopmentDeploy-18082")
$deployMutexHeld = $false
$process = $null
$sidecarToken = $null
$pidPath = $null

try {
  try {
    $mutexHeld = $mutex.WaitOne([TimeSpan]::FromSeconds(30))
  } catch [Threading.AbandonedMutexException] {
    $mutexHeld = $true
  }
  if (-not $mutexHeld) {
    throw "Un autre lancement mass-subagents Switch developpement est deja en cours."
  }

  foreach ($requiredPath in @(
    (Join-Path $sourceRoot "package.json"),
    (Join-Path $sourceRoot "src\cli.mjs"),
    (Join-Path $sourceRoot "src\store.mjs"),
    (Join-Path $sourceRoot "contracts\switch-http.v2.json"),
    (Join-Path $sourceRoot "agent_roles\scout\v1\manifest.json"),
    $verificationScript
  )) {
    if (-not (Test-Path -LiteralPath $requiredPath -PathType Leaf)) {
      throw "Paquet mass-subagents incomplet : $requiredPath est absent."
    }
  }
  Assert-TrustedSourceState -AppRoot $appRoot -Paths @(
    "sidecar/mass-subagents",
    "scripts/start-mass-subagents-development.ps1",
    "scripts/stop-mass-subagents-development.ps1",
    "scripts/verify-mass-subagents-development.ps1"
  )

  $node = Get-Command node -ErrorAction Stop
  $nodeVersion = [version](& $node.Source -p "process.versions.node")
  if ($nodeVersion.Major -lt 22) {
    throw "Node.js 22 ou plus recent est obligatoire pour mass-subagents."
  }
  if (-not $env:LOCALAPPDATA) {
    throw "LOCALAPPDATA est indisponible : le runtime interne ne peut pas etre prepare."
  }

  $runtimeRoot = Join-Path $env:LOCALAPPDATA "SwitchDevelopmentMassSubagents"
  $releasesPath = Join-Path $runtimeRoot "releases"
  $logsPath = Join-Path $runtimeRoot "logs"
  $runtimeRunPath = Join-Path $runtimeRoot "run"
  $dataPath = Join-Path $runtimeRoot "data"
  $tokenPath = Join-Path $runtimeRunPath "mass-subagents-admin.token"

  New-Item -ItemType Directory -Path $runtimeRoot -Force | Out-Null
  Set-PrivateFileSystemAcl -LiteralPath $runtimeRoot -Directory $true
  New-Item -ItemType Directory -Path @(
    $releasesPath,
    $logsPath,
    $runtimeRunPath,
    $dataPath
  ) -Force | Out-Null
  foreach ($privateDirectory in @($releasesPath, $logsPath, $runtimeRunPath, $dataPath)) {
    Set-PrivateFileSystemAcl -LiteralPath $privateDirectory -Directory $true
  }

  $sourceInventory = Get-MassSubagentsInventory -Root $sourceRoot
  $releaseId = "msa-" + $sourceInventory.treeSha256.Substring(0, 24)
  $releasePath = Join-Path $releasesPath $releaseId
  $healthUri = "http://127.0.0.1:$Port/healthz"

  $listener = Get-NetTCPConnection -State Listen -LocalPort $Port -ErrorAction SilentlyContinue |
    Select-Object -First 1
  if ($listener) {
    try {
      $health = Invoke-RestMethod -Uri $healthUri -TimeoutSec 3
      if (
        $health.ok -eq $true -and
        $health.releaseId -eq $releaseId -and
        $health.realChatDispatchEnabled -eq [bool]$EnableSwitchReadonly
      ) {
        Write-Output "mass-subagents Switch developpement est deja disponible sur 127.0.0.1:$Port."
        return
      }
    } catch {
      # Le port occupe n'est jamais tue aveuglement.
    }
    throw "Le port $Port est occupe par une autre release ou un service non sain."
  }

  & $verificationScript -DeploymentRoot $deploymentRootPath
  if ($LASTEXITCODE -ne 0) {
    throw "La release mass-subagents n'a pas franchi sa verification locale."
  }

  try {
    $deployMutexHeld = $deployMutex.WaitOne([TimeSpan]::FromSeconds(30))
  } catch [Threading.AbandonedMutexException] {
    $deployMutexHeld = $true
  }
  if (-not $deployMutexHeld) {
    throw "Une mise en cache Switch developpement concurrente est en cours."
  }
  $sourceAfterVerification = Get-MassSubagentsInventory -Root $sourceRoot
  if ($sourceAfterVerification.treeSha256 -ne $sourceInventory.treeSha256) {
    throw "Le paquet mass-subagents a change pendant sa verification."
  }

  if (-not (Test-Path -LiteralPath (Join-Path $releasePath "src\cli.mjs") -PathType Leaf)) {
    $stagePath = Join-Path $releasesPath ("stage-" + [Guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Path $stagePath | Out-Null
    try {
      Get-ChildItem -LiteralPath $sourceRoot -Force | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination $stagePath -Recurse
      }
      $stagedInventory = Get-MassSubagentsInventory -Root $stagePath
      $sourceAfterCopy = Get-MassSubagentsInventory -Root $sourceRoot
      if (
        $stagedInventory.treeSha256 -ne $sourceInventory.treeSha256 -or
        $sourceAfterCopy.treeSha256 -ne $sourceInventory.treeSha256
      ) {
        throw "Le paquet mass-subagents a change pendant sa mise en cache."
      }
      $manifest = [ordered]@{
        schema = "switch-mass-subagents/development-release/v1"
        releaseId = $releaseId
        treeSha256 = $sourceInventory.treeSha256
        fileCount = $sourceInventory.files.Count
        createdAt = [DateTime]::UtcNow.ToString("o")
      } | ConvertTo-Json -Depth 4
      [IO.File]::WriteAllText(
        (Join-Path $stagePath ".switch-mass-subagents-release.json"),
        $manifest + "`n",
        [Text.UTF8Encoding]::new($false)
      )
      Move-Item -LiteralPath $stagePath -Destination $releasePath
    } catch {
      if (Test-Path -LiteralPath $stagePath) {
        Remove-Item -LiteralPath $stagePath -Recurse -Force
      }
      throw
    }
  }

  $runtimeInventory = Get-MassSubagentsInventory -Root $releasePath
  $manifestPath = Join-Path $releasePath ".switch-mass-subagents-release.json"
  if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
    throw "Le manifeste de release mass-subagents est absent."
  }
  $releaseManifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json
  if (
    $runtimeInventory.treeSha256 -ne $sourceInventory.treeSha256 -or
    $releaseManifest.schema -ne "switch-mass-subagents/development-release/v1" -or
    $releaseManifest.releaseId -ne $releaseId -or
    $releaseManifest.treeSha256 -ne $sourceInventory.treeSha256 -or
    [int]$releaseManifest.fileCount -ne $sourceInventory.files.Count
  ) {
    throw "La release mass-subagents en cache ne correspond pas au paquet source."
  }
  Get-ChildItem -LiteralPath $releasePath -File -Recurse -Force | ForEach-Object {
    $_.IsReadOnly = $true
    Set-ReadOnlyReleaseAcl -LiteralPath $_.FullName -Directory $false
  }
  Get-ChildItem -LiteralPath $releasePath -Directory -Recurse -Force |
    Sort-Object { $_.FullName.Length } -Descending |
    ForEach-Object { Set-ReadOnlyReleaseAcl -LiteralPath $_.FullName -Directory $true }
  Set-ReadOnlyReleaseAcl -LiteralPath $releasePath -Directory $true
  $deployMutex.ReleaseMutex()
  $deployMutexHeld = $false

  if (-not (Test-Path -LiteralPath $tokenPath -PathType Leaf)) {
    $tokenBytes = [byte[]]::new(32)
    [Security.Cryptography.RandomNumberGenerator]::Fill($tokenBytes)
    $tokenPayload = [Text.UTF8Encoding]::new($false).GetBytes(
      [Convert]::ToHexString($tokenBytes).ToLowerInvariant() + "`n"
    )
    $stream = [IO.FileStream]::new(
      $tokenPath,
      [IO.FileMode]::CreateNew,
      [IO.FileAccess]::Write,
      [IO.FileShare]::None
    )
    try {
      $stream.Write($tokenPayload)
      $stream.Flush($true)
    } finally {
      $stream.Dispose()
    }
  }
  Set-PrivateFileSystemAcl -LiteralPath $tokenPath -Directory $false
  $sidecarToken = (Get-Content -Raw -LiteralPath $tokenPath).Trim()
  if ($sidecarToken -cnotmatch "^[a-f0-9]{64}$") {
    throw "Le token administrateur mass-subagents local est invalide."
  }

  $switchToken = $null
  if ($EnableSwitchReadonly) {
    if (-not (Test-Path -LiteralPath $configurationPath -PathType Leaf)) {
      throw "Configuration Switch developpement absente : $configurationPath"
    }
    $configurationText = [IO.File]::ReadAllText($configurationPath)
    $tokenMatch = [regex]::Match(
      $configurationText,
      '(?m)^\s*\$env:CST_ADMIN_TOKEN\s*=\s*(?<quote>[''"])(?<token>[A-Fa-f0-9]{64})\k<quote>\s*$'
    )
    if (-not $tokenMatch.Success) {
      throw "CST_ADMIN_TOKEN n'est pas un littéral hexadecimal valide dans la configuration Switch developpement."
    }
    $switchToken = $tokenMatch.Groups["token"].Value
  }

  $capacity = Get-EnvironmentDefault -Name "MASS_SUBAGENTS_SWITCH_CAPACITY" -Fallback "2"
  $tickMilliseconds = Get-EnvironmentDefault -Name "MASS_SUBAGENTS_TICK_MS" -Fallback "1000"
  $maxRealAgents = Get-EnvironmentDefault -Name "MASS_SUBAGENTS_MAX_REAL_AGENTS" -Fallback "32"
  $maxStoredRuns = Get-EnvironmentDefault -Name "MASS_SUBAGENTS_MAX_STORED_RUNS" -Fallback "100"
  $maxArtifactBytes = Get-EnvironmentDefault -Name "MASS_SUBAGENTS_MAX_ARTIFACT_BYTES" -Fallback "4194304"

  $childEnvironment = @{
    SWITCH_ENV = "development"
    MASS_SUBAGENTS_ADMIN_TOKEN = $sidecarToken
    MASS_SUBAGENTS_BIND = "127.0.0.1"
    MASS_SUBAGENTS_PORT = [string]$Port
    MASS_SUBAGENTS_DATA_DIR = $dataPath
    MASS_SUBAGENTS_RELEASE_ID = $releaseId
    MASS_SUBAGENTS_SWITCH_CAPACITY = $capacity
    MASS_SUBAGENTS_TICK_MS = $tickMilliseconds
    MASS_SUBAGENTS_MAX_REAL_AGENTS = $maxRealAgents
    MASS_SUBAGENTS_MAX_STORED_RUNS = $maxStoredRuns
    MASS_SUBAGENTS_MAX_ARTIFACT_BYTES = $maxArtifactBytes
    MASS_SUBAGENTS_SWITCH_READONLY_ENABLED = $EnableSwitchReadonly.IsPresent.ToString().ToLowerInvariant()
  }
  if ($EnableSwitchReadonly) {
    $childEnvironment["SWITCH_BASE_URL"] = "http://127.0.0.1:18082"
    $childEnvironment["SWITCH_ADMIN_TOKEN"] = $switchToken
  }

  $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
  $stdoutPath = Join-Path $logsPath "mass-subagents-$stamp.stdout.log"
  $stderrPath = Join-Path $logsPath "mass-subagents-$stamp.stderr.log"
  $baseEnvironmentNames = @(
    "ComSpec",
    "NUMBER_OF_PROCESSORS",
    "OS",
    "PATH",
    "PATHEXT",
    "PROCESSOR_ARCHITECTURE",
    "SystemDrive",
    "SystemRoot",
    "TEMP",
    "TMP",
    "WINDIR"
  )
  foreach ($item in @(Get-ChildItem Env:)) {
    if ($baseEnvironmentNames -notcontains $item.Name) {
      Remove-Item -LiteralPath ("Env:" + $item.Name) -ErrorAction SilentlyContinue
    }
  }
  try {
    $process = Start-Process -FilePath $node.Source `
      -ArgumentList @("src/cli.mjs", "serve") `
      -WorkingDirectory $releasePath `
      -WindowStyle Hidden `
      -Environment $childEnvironment `
      -RedirectStandardOutput $stdoutPath `
      -RedirectStandardError $stderrPath `
      -PassThru
  } finally {
    Restore-OriginalEnvironment -Snapshot $originalEnvironment
  }

  $pidPath = Join-Path $runtimeRunPath "mass-subagents.pid"
  [IO.File]::WriteAllText(
    $pidPath,
    "$($process.Id)`n",
    [Text.UTF8Encoding]::new($false)
  )

  $deadline = [DateTime]::UtcNow.AddSeconds(30)
  do {
    if ($process.HasExited) {
      throw "mass-subagents a quitte pendant son demarrage. Consulte $stderrPath"
    }
    try {
      $health = Invoke-RestMethod -Uri $healthUri -TimeoutSec 2
      if (
        $health.ok -eq $true -and
        $health.releaseId -eq $releaseId -and
        $health.realChatDispatchEnabled -eq [bool]$EnableSwitchReadonly
      ) {
        Write-Output "mass-subagents Switch developpement disponible sur 127.0.0.1:$Port (release $releaseId)."
        return
      }
    } catch {
      Start-Sleep -Milliseconds 250
    }
  } while ([DateTime]::UtcNow -lt $deadline)
  throw "mass-subagents n'est pas devenu sain dans le delai imparti. Consulte $stderrPath"
} catch {
  if ($process -and -not $process.HasExited -and $sidecarToken) {
    try {
      $shutdownResponse = Invoke-RestMethod `
        -Method Post `
        -Uri "http://127.0.0.1:$Port/v1/admin/shutdown" `
        -Headers @{ Authorization = "Bearer $sidecarToken" } `
        -TimeoutSec 5
      if ($shutdownResponse.ok -eq $true) {
        [void]$process.WaitForExit(10000)
      }
    } catch {
      Write-Warning "Le processus lance n'a pas repondu a l'arret authentifie; aucun arret force n'a ete tente."
    }
  }
  if ($process -and $process.HasExited -and $pidPath -and (Test-Path -LiteralPath $pidPath)) {
    $recordedPid = (Get-Content -Raw -LiteralPath $pidPath).Trim()
    if ($recordedPid -eq [string]$process.Id) {
      Remove-Item -LiteralPath $pidPath -Force
    }
  }
  throw
} finally {
  Restore-OriginalEnvironment -Snapshot $originalEnvironment
  if ($deployMutexHeld) { $deployMutex.ReleaseMutex() }
  $deployMutex.Dispose()
  if ($mutexHeld) { $mutex.ReleaseMutex() }
  $mutex.Dispose()
}
