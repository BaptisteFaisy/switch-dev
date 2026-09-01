[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [ValidatePattern('^[A-Za-z0-9._-]+@[A-Za-z0-9._:-]+$')]
  [string]$SshTarget,
  [string]$IdentityFile = (Join-Path $env:USERPROFILE ".ssh\id_ed25519"),
  [string]$KnownHostsFile = "",
  [ValidateRange(1, 65535)]
  [int]$SshPort = 22,
  [ValidateRange(1, 65535)]
  [int]$RemotePort = 8080,
  [ValidateRange(1, 65535)]
  [int]$LocalPort = 18080,
  [string]$PublicBaseUrl = "",
  [ValidatePattern('^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$')]
  [string]$NodeId = "vps",
  [string]$NodeLabel = "VPS",
  [ValidateRange(1, 1024)]
  [int]$Capacity = 1,
  [ValidatePattern('^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$')]
  [string]$WorkspaceName = "codex-switch-terminal",
  [string]$Image = "",
  [string]$RegistryUsername = "",
  [string]$RegistryTokenFile = "",
  [ValidatePattern('^[A-Za-z0-9][A-Za-z0-9._/-]{0,199}$')]
  [string]$TranscriptionModel = "Systran/faster-whisper-small",
  [string]$TranscriptionUrl = "",
  [ValidateSet("auto", "gpu", "cpu")]
  [string]$TranscriptionAccelerator = "auto",
  [string]$SpeachesImage = "ghcr.io/speaches-ai/speaches:latest-cuda-12.6.3",
  [ValidatePattern('^[A-Za-z0-9._-]+$')]
  [string]$WslDistribution = "Ubuntu",
  [switch]$GpuTranscription,
  [switch]$SkipAccountSeed,
  [switch]$NoWorkspaceSeed,
  [switch]$AcceptNewHostKey,
  [switch]$Connect
)

$ErrorActionPreference = "Stop"

# PSModulePath : remet les modules Windows PowerShell en tete (Get-FileHash introuvable sinon dans le sandbox).
& (Join-Path $PSScriptRoot 'switch-development-ensure-system-module-path.ps1')

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$Root = Split-Path -Parent $ScriptDir
$DataDir = Join-Path $env:APPDATA "codex-switch-terminal-server"
$LocalEnvFile = Join-Path $DataDir "server.local.env.ps1"
$SettingsFile = Join-Path $DataDir "settings.json"
$CodexHomes = Join-Path $DataDir "codex-homes"
$AutonomousAgentsFile = Join-Path $DataDir "autonomous-agents.json"
$ProfileDir = Join-Path $env:APPDATA "codex-switch-terminal\vps"
$ProfilePath = Join-Path $ProfileDir "$NodeId.json"

function Assert-OneLine {
  param([string]$Name, [AllowEmptyString()][string]$Value)
  if ($Value.Contains("`r") -or $Value.Contains("`n")) {
    throw "$Name doit tenir sur une seule ligne."
  }
}

function New-RandomToken {
  $bytes = New-Object byte[] 48
  $generator = [Security.Cryptography.RandomNumberGenerator]::Create()
  try {
    $generator.GetBytes($bytes)
  }
  finally {
    $generator.Dispose()
  }
  return [Convert]::ToBase64String($bytes).TrimEnd('=').Replace('+', '-').Replace('/', '_')
}

function Protect-Secret {
  param([string]$Value)
  $secure = ConvertTo-SecureString -String $Value -AsPlainText -Force
  return ConvertFrom-SecureString -SecureString $secure
}

function Unprotect-Secret {
  param([string]$Value)
  $secure = ConvertTo-SecureString -String $Value
  $pointer = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($secure)
  try {
    return [Runtime.InteropServices.Marshal]::PtrToStringBSTR($pointer)
  }
  finally {
    [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($pointer)
  }
}

function Write-Utf8NoBom {
  param([string]$Path, [string]$Content)
  [IO.File]::WriteAllText($Path, $Content, (New-Object Text.UTF8Encoding($false)))
}

function Convert-AutonomousAgentStoreForVps {
  param(
    [Parameter(Mandatory = $true)][string]$Source,
    [Parameter(Mandatory = $true)][string]$Destination,
    [Parameter(Mandatory = $true)][string]$LocalWorkspace,
    [Parameter(Mandatory = $true)][string]$RemoteWorkspace
  )

  $store = Get-Content -LiteralPath $Source -Raw | ConvertFrom-Json
  $agents = @($store.agents)
  $localRoot = [IO.Path]::GetFullPath($LocalWorkspace).TrimEnd([char[]]@('\', '/'))
  $outsideWorkspace = 0

  foreach ($agent in $agents) {
    $projectDir = [string]$agent.projectDir
    if (-not $projectDir.Trim()) { continue }

    try {
      $normalized = [IO.Path]::GetFullPath($projectDir).TrimEnd([char[]]@('\', '/'))
    }
    catch {
      $outsideWorkspace++
      continue
    }

    if ($normalized.Equals($localRoot, [StringComparison]::OrdinalIgnoreCase)) {
      $agent.projectDir = $RemoteWorkspace
      continue
    }

    $localPrefix = "$localRoot$([IO.Path]::DirectorySeparatorChar)"
    if ($normalized.StartsWith($localPrefix, [StringComparison]::OrdinalIgnoreCase)) {
      $relative = $normalized.Substring($localPrefix.Length).Replace('\', '/')
      $agent.projectDir = "$($RemoteWorkspace.TrimEnd('/'))/$relative"
      continue
    }

    $outsideWorkspace++
  }

  if ($outsideWorkspace -gt 0) {
    throw "$outsideWorkspace agent(s) autonome(s) utilisent un projet hors du workspace transfere. Deplace-les dans $LocalWorkspace avant le deploiement."
  }

  Write-Utf8NoBom -Path $Destination -Content (($store | ConvertTo-Json -Depth 100) + "`n")
}

function Convert-SettingsForVps {
  param(
    [Parameter(Mandatory = $true)][string]$Source,
    [Parameter(Mandatory = $true)][string]$Destination,
    [Parameter(Mandatory = $true)][string]$LocalWorkspace,
    [Parameter(Mandatory = $true)][string]$RemoteWorkspace,
    [Parameter(Mandatory = $true)][string]$RemoteWorkspaceLabel
  )

  $settings = Get-Content -LiteralPath $Source -Raw | ConvertFrom-Json
  $settings.shell = "/bin/bash"
  $localRoot = [IO.Path]::GetFullPath($LocalWorkspace).TrimEnd([char[]]@('\', '/'))
  $sourceWorkspace = @($settings.workspaces | Where-Object {
    $path = [string]$_.path
    if (-not $path.Trim()) { return $false }
    try {
      return [IO.Path]::GetFullPath($path).TrimEnd([char[]]@('\', '/')).Equals(
        $localRoot,
        [StringComparison]::OrdinalIgnoreCase
      )
    }
    catch {
      return $false
    }
  } | Select-Object -First 1)

  if ($sourceWorkspace.Count -eq 1) {
    $portableWorkspace = $sourceWorkspace[0]
    $portableWorkspace.id = $RemoteWorkspace
    $portableWorkspace.path = $RemoteWorkspace
  }
  else {
    $portableWorkspace = [pscustomobject][ordered]@{
      id = $RemoteWorkspace
      label = $RemoteWorkspaceLabel
      path = $RemoteWorkspace
      memory = ""
    }
  }
  $settings.workspaces = @($portableWorkspace)
  $settings.closedWorkspaceIds = @()

  Write-Utf8NoBom -Path $Destination -Content (($settings | ConvertTo-Json -Depth 100) + "`n")
}

function Invoke-WslText {
  param([Parameter(Mandatory = $true)][string[]]$Arguments)
  $output = @(& wsl.exe -d $WslDistribution -e @Arguments 2>&1)
  if ($LASTEXITCODE -ne 0) {
    throw "Commande WSL en echec: $($output -join [Environment]::NewLine)"
  }
  return (($output | ForEach-Object { [string]$_ }) -join "`n").Trim()
}

function Convert-ToWslPath {
  param([Parameter(Mandatory = $true)][string]$WindowsPath)
  return Invoke-WslText -Arguments @("wslpath", "-a", "-u", $WindowsPath)
}

function Quote-SshOptionPath {
  param([Parameter(Mandatory = $true)][string]$Value)
  if ($Value.Contains("'")) {
    throw "Les chemins contenant une apostrophe ne sont pas pris en charge par le transport Ansible."
  }
  return "'$Value'"
}

Assert-OneLine -Name "NodeLabel" -Value $NodeLabel
Assert-OneLine -Name "Image" -Value $Image
Assert-OneLine -Name "TranscriptionModel" -Value $TranscriptionModel
Assert-OneLine -Name "TranscriptionUrl" -Value $TranscriptionUrl
Assert-OneLine -Name "SpeachesImage" -Value $SpeachesImage

foreach ($command in @("wsl.exe", "ssh", "tar", "git")) {
  if (-not (Get-Command $command -ErrorAction SilentlyContinue)) {
    throw "Commande requise introuvable: $command"
  }
}

$resolvedIdentity = (Resolve-Path -LiteralPath $IdentityFile).Path
$resolvedKnownHostsFile = ""
if ($KnownHostsFile.Trim()) {
  $resolvedKnownHostsFile = (Resolve-Path -LiteralPath $KnownHostsFile).Path
}
if ($resolvedKnownHostsFile -and $AcceptNewHostKey) {
  throw "-KnownHostsFile et -AcceptNewHostKey sont mutuellement exclusifs."
}

$existingProfile = $null
if (Test-Path -LiteralPath $ProfilePath) {
  $existingProfile = Get-Content -LiteralPath $ProfilePath -Raw | ConvertFrom-Json
  if ([string]$existingProfile.sshTarget -ne $SshTarget -or [int]$existingProfile.sshPort -ne $SshPort) {
    throw "Le profil '$NodeId' appartient deja a $($existingProfile.sshTarget). Choisis un autre NodeId."
  }
  if ([int]$existingProfile.remotePort -ne $RemotePort) {
    throw "Le port distant d'un noeud existant ne peut pas etre change pendant une mise a jour."
  }
}

if (-not $PSBoundParameters.ContainsKey("GpuTranscription") -and $existingProfile) {
  $GpuTranscription = [bool]$existingProfile.gpuTranscription
}
if (
  -not $PSBoundParameters.ContainsKey("TranscriptionModel") -and
  $existingProfile -and
  [string]$existingProfile.transcriptionModel
) {
  $TranscriptionModel = [string]$existingProfile.transcriptionModel
}
if (
  -not $PSBoundParameters.ContainsKey("TranscriptionUrl") -and
  $existingProfile -and
  [string]$existingProfile.transcriptionUrl
) {
  $TranscriptionUrl = [string]$existingProfile.transcriptionUrl
}
if (
  -not $PSBoundParameters.ContainsKey("TranscriptionAccelerator") -and
  $existingProfile -and
  [string]$existingProfile.transcriptionAccelerator
) {
  $TranscriptionAccelerator = [string]$existingProfile.transcriptionAccelerator
}
if (
  -not $PSBoundParameters.ContainsKey("SpeachesImage") -and
  $existingProfile -and
  [string]$existingProfile.speachesImage
) {
  $SpeachesImage = [string]$existingProfile.speachesImage
}
Assert-OneLine -Name "TranscriptionModel" -Value $TranscriptionModel
Assert-OneLine -Name "TranscriptionUrl" -Value $TranscriptionUrl
Assert-OneLine -Name "SpeachesImage" -Value $SpeachesImage

if ($TranscriptionUrl.Trim()) {
  try {
    $transcriptionUri = [Uri]$TranscriptionUrl.Trim()
  }
  catch {
    throw "TranscriptionUrl doit etre une URL HTTPS absolue."
  }
  if (
    -not $transcriptionUri.IsAbsoluteUri -or
    $transcriptionUri.Scheme -ne "https" -or
    $transcriptionUri.UserInfo -or
    $transcriptionUri.Fragment
  ) {
    throw "TranscriptionUrl doit etre une URL HTTPS absolue, sans identifiants ni fragment."
  }
  $TranscriptionUrl = $transcriptionUri.AbsoluteUri
}

if (-not $PublicBaseUrl.Trim() -and $existingProfile -and [string]$existingProfile.publicBaseUrl) {
  $PublicBaseUrl = [string]$existingProfile.publicBaseUrl
}
if (-not $PublicBaseUrl.Trim()) {
  $PublicBaseUrl = "http://127.0.0.1:$RemotePort"
}
Assert-OneLine -Name "PublicBaseUrl" -Value $PublicBaseUrl
try {
  $publicBaseUri = [Uri]$PublicBaseUrl.Trim()
}
catch {
  throw "PublicBaseUrl doit etre une origine HTTP(S) absolue."
}
if (
  -not $publicBaseUri.IsAbsoluteUri -or
  $publicBaseUri.Scheme -notin @("http", "https") -or
  $publicBaseUri.UserInfo -or
  ($publicBaseUri.AbsolutePath -and $publicBaseUri.AbsolutePath -ne "/") -or
  $publicBaseUri.Query -or
  $publicBaseUri.Fragment
) {
  throw "PublicBaseUrl doit etre une origine HTTP(S) absolue, sans chemin, identifiants, requete ni fragment."
}
$PublicBaseUrl = $publicBaseUri.GetLeftPart([UriPartial]::Authority)

$localAdminToken = ""
$localGitPat = ""
$forwardedEnvironment = [ordered]@{}
$forwardNames = @(
  "CST_ALLOW_REGISTRATION",
  "CST_AUTH_SECURE_COOKIE",
  "CST_GOOGLE_CLIENT_ID",
  "CST_GOOGLE_CLIENT_SECRET",
  "CST_GOOGLE_REDIRECT_URI",
  "CST_MICROSOFT_CLIENT_ID",
  "CST_MICROSOFT_CLIENT_SECRET",
  "CST_MICROSOFT_TENANT_ID",
  "CST_MICROSOFT_REDIRECT_URI",
  "CST_MICROSOFT_SCOPES",
  "CST_REVOLUT_PUBLIC_KEY",
  "CST_REVOLUT_SECRET_KEY",
  "CST_REVOLUT_CURRENCY",
  "CST_REVOLUT_SANDBOX",
  "CST_REVOLUT_MERCHANT_BASE_URL",
  "CST_REVOLUT_RETURN_URL",
  "CST_REVOLUT_WEBHOOK_URL",
  "CST_FAL_KEY",
  "CST_VOICE_TRANSCRIPTION_MODE",
  "CST_VOICE_TRANSCRIPTION_URL",
  "CST_VOICE_TRANSCRIPTION_MODEL",
  "CST_VOICE_TRANSCRIPTION_API_KEY",
  "CST_VOICE_TRANSCRIPTION_ACCELERATOR",
  "CST_VOICE_OLLAMA_URL",
  "CST_VOICE_OLLAMA_MODEL",
  "CST_VOICE_OLLAMA_API_KEY",
  "CST_VOICE_OLLAMA_HOST_HEADER",
  "CST_VOICE_ALLOW_INSECURE_REMOTE"
)
if (Test-Path -LiteralPath $LocalEnvFile) {
  $previousValues = @{}
  foreach ($name in @("CST_ADMIN_TOKEN", "CST_GIT_PAT") + $forwardNames) {
    $previousValues[$name] = [Environment]::GetEnvironmentVariable($name)
  }
  try {
    . $LocalEnvFile
    $localAdminToken = [string]$env:CST_ADMIN_TOKEN
    $localGitPat = [string]$env:CST_GIT_PAT
    foreach ($name in $forwardNames) {
      $value = [Environment]::GetEnvironmentVariable($name)
      if ($null -ne $value -and $value.Trim()) {
        Assert-OneLine -Name $name -Value $value
        $forwardedEnvironment[$name] = $value.Trim()
      }
    }
  }
  finally {
    foreach ($name in $previousValues.Keys) {
      [Environment]::SetEnvironmentVariable($name, $previousValues[$name])
    }
  }
}

if ($GpuTranscription) {
  foreach ($name in @(
    "CST_VOICE_TRANSCRIPTION_MODE",
    "CST_VOICE_TRANSCRIPTION_URL",
    "CST_VOICE_TRANSCRIPTION_MODEL",
    "CST_VOICE_TRANSCRIPTION_API_KEY",
    "CST_VOICE_TRANSCRIPTION_ACCELERATOR",
    "CST_VOICE_ALLOW_INSECURE_REMOTE"
  )) {
    $forwardedEnvironment.Remove($name)
  }
}
elseif ($TranscriptionUrl.Trim()) {
  $forwardedEnvironment["CST_VOICE_TRANSCRIPTION_MODE"] = "remote"
  $forwardedEnvironment["CST_VOICE_TRANSCRIPTION_URL"] = $TranscriptionUrl.Trim()
  $forwardedEnvironment["CST_VOICE_TRANSCRIPTION_MODEL"] = $TranscriptionModel.Trim()
  $forwardedEnvironment["CST_VOICE_TRANSCRIPTION_ACCELERATOR"] = $TranscriptionAccelerator
  $forwardedEnvironment.Remove("CST_VOICE_TRANSCRIPTION_API_KEY")
  $forwardedEnvironment.Remove("CST_VOICE_ALLOW_INSECURE_REMOTE")
}

if ($existingProfile -and [string]$existingProfile.tokenProtected) {
  $adminToken = Unprotect-Secret ([string]$existingProfile.tokenProtected)
}
elseif ($localAdminToken.Trim()) {
  $adminToken = $localAdminToken.Trim()
}
else {
  $adminToken = New-RandomToken
}
if ($adminToken.Length -lt 32) {
  throw "Le token administrateur doit contenir au moins 32 caracteres."
}
Assert-OneLine -Name "Token administrateur" -Value $adminToken
Assert-OneLine -Name "CST_GIT_PAT" -Value $localGitPat

$registryToken = ""
if ($RegistryTokenFile.Trim()) {
  $registryToken = (Get-Content -LiteralPath (Resolve-Path -LiteralPath $RegistryTokenFile).Path -Raw).Trim()
  Assert-OneLine -Name "Token du registre" -Value $registryToken
  if (-not $RegistryUsername.Trim()) {
    throw "-RegistryUsername est requis avec -RegistryTokenFile."
  }
}
$registry = ""
if ($Image.Trim()) {
  $firstImagePart = ($Image.Trim() -split '/', 2)[0]
  $registry = if ($firstImagePart.Contains('.') -or $firstImagePart.Contains(':')) {
    $firstImagePart
  }
  else {
    "docker.io"
  }
}

$sshArgs = @(
  "-p", [string]$SshPort,
  "-o", "BatchMode=yes",
  "-o", "ConnectTimeout=20",
  "-o", "ServerAliveInterval=30",
  "-o", "ServerAliveCountMax=4",
  "-i", $resolvedIdentity
)
if ($resolvedKnownHostsFile) {
  $sshArgs += @(
    "-o", "UserKnownHostsFile=$resolvedKnownHostsFile",
    "-o", "StrictHostKeyChecking=yes"
  )
}
elseif ($AcceptNewHostKey) {
  $sshArgs += @("-o", "StrictHostKeyChecking=accept-new")
}

Write-Host "Verification SSH et sudo sur $SshTarget..." -ForegroundColor Cyan
& ssh @sshArgs $SshTarget 'if [ "$(id -u)" -eq 0 ]; then command -v python3 >/dev/null; else command -v python3 >/dev/null && sudo -n true; fi'
if ($LASTEXITCODE -ne 0) {
  throw "Le VPS doit etre joignable par cle SSH, disposer de Python 3 et autoriser root ou sudo sans mot de passe."
}

$tempRoot = Join-Path $env:TEMP ("cst-ansible-" + [guid]::NewGuid().ToString("N"))
$buildArchive = Join-Path $tempRoot "build-context.tar.gz"
$workspaceArchive = Join-Path $tempRoot "workspace-seed.tar.gz"
$dataArchive = Join-Path $tempRoot "data-seed.tar.gz"
$inventoryFile = Join-Path $tempRoot "inventory.json"
$varsFile = Join-Path $tempRoot "vars.json"

New-Item -ItemType Directory -Force -Path $tempRoot | Out-Null
try {
  Push-Location $Root
  try {
    $commonExcludes = @(
      "--exclude=src-tauri/target",
      "--exclude=src-tauri/target-alt",
      "--exclude=src-tauri/gen",
      "--exclude=**/node_modules",
      "--exclude=**/dist",
      "--exclude=**/*.exe",
      "--exclude=**/*.apk",
      "--exclude=server.local.env.ps1",
      "--exclude=**/server.local.env.ps1"
    )
    $buildEntries = @(
      "Dockerfile",
      ".dockerignore",
      "package.json",
      "package-lock.json",
      "index.html",
      "tsconfig.json",
      "vite.config.ts",
      "rust-toolchain.toml",
      "scripts",
      "src",
      "public",
      "src-tauri",
      "deploy/docker-entrypoint.sh"
    )
    & tar @commonExcludes -czf $buildArchive @buildEntries
    if ($LASTEXITCODE -ne 0) { throw "La creation du contexte Docker a echoue." }

    if (-not $NoWorkspaceSeed) {
      $workspaceEntries = @(
        "package.json",
        "package-lock.json",
        "index.html",
        "tsconfig.json",
        "vite.config.ts",
        "rust-toolchain.toml",
        "README.md",
        "Dockerfile",
        "compose.yaml",
        ".dockerignore",
        ".github",
        "android",
        "ios",
        "config",
        "deploy",
        "docs",
        "public",
        "scripts",
        "src",
        "src-tauri",
        "tests"
      )
      & tar @commonExcludes "--exclude=config/oracle-vps-pool.json" -czf $workspaceArchive @workspaceEntries
      if ($LASTEXITCODE -ne 0) { throw "La creation du workspace initial a echoue." }
    }
  }
  finally {
    Pop-Location
  }

  $seedAccounts = $false
  if (-not $SkipAccountSeed -and (Test-Path -LiteralPath $SettingsFile) -and (Test-Path -LiteralPath $CodexHomes)) {
    # Les sessions, credentials, memoires, skills et bases d'etat sont
    # conserves. Les journaux diagnostics, caches et sandboxes Windows sont
    # regenerables et peuvent peser plusieurs gigaoctets pour aucun benefice
    # sur le runtime Linux.
    $dataSeedExcludes = @(
      "--exclude=codex-homes/*/logs_*.sqlite*",
      "--exclude=codex-homes/*/log",
      "--exclude=codex-homes/*/cache",
      "--exclude=codex-homes/*/tmp",
      "--exclude=codex-homes/*/.tmp",
      "--exclude=codex-homes/*/.sandbox",
      "--exclude=codex-homes/*/.sandbox-bin",
      "--exclude=codex-homes/*/.sandbox-secrets",
      "--exclude=codex-homes/*/sandbox.*.log"
    )
    $dataSeedArguments = @("-czf", $dataArchive) + $dataSeedExcludes + @(
      "-C", $DataDir,
      "codex-homes"
    )
    $portableSettings = Join-Path $tempRoot "settings.json"
    Convert-SettingsForVps `
      -Source $SettingsFile `
      -Destination $portableSettings `
      -LocalWorkspace $Root `
      -RemoteWorkspace "/srv/cst/workspaces/$WorkspaceName" `
      -RemoteWorkspaceLabel $WorkspaceName
    $dataSeedArguments += @(
      "-C", $tempRoot,
      "settings.json"
    )
    if (Test-Path -LiteralPath $AutonomousAgentsFile) {
      $portableAgentStore = Join-Path $tempRoot "autonomous-agents.json"
      Convert-AutonomousAgentStoreForVps `
        -Source $AutonomousAgentsFile `
        -Destination $portableAgentStore `
        -LocalWorkspace $Root `
        -RemoteWorkspace "/srv/cst/workspaces/$WorkspaceName"
      $dataSeedArguments += @(
        "-C", $tempRoot,
        "autonomous-agents.json"
      )
    }
    & tar @dataSeedArguments
    if ($LASTEXITCODE -ne 0) { throw "La sauvegarde initiale des comptes et agents a echoue." }
    $seedAccounts = $true
  }
  elseif (-not $SkipAccountSeed) {
    Write-Warning "Aucun compte local pret dans $DataDir; le VPS demarrera sans seed de compte."
  }

  $sourceDigest = (Get-FileHash -LiteralPath $buildArchive -Algorithm SHA256).Hash.ToLowerInvariant()
  $release = $sourceDigest.Substring(0, 16)
  $commit = "archive-$($sourceDigest.Substring(0, 12))"
  $rev = & git -C $Root rev-parse --short HEAD 2>$null
  if ($LASTEXITCODE -eq 0 -and $rev) {
    $commit = ([string]$rev).Trim()
    $worktreeChanges = @(& git -C $Root status --porcelain --untracked-files=normal 2>$null)
    if ($LASTEXITCODE -eq 0 -and $worktreeChanges.Count -gt 0) {
      $commit = "$commit-dirty-$($sourceDigest.Substring(0, 12))"
    }
  }
  if ($commit -notmatch '^[A-Za-z0-9._-]+$') { $commit = "unknown-$release" }

  $targetParts = $SshTarget.Split('@', 2)
  $wslRoot = Convert-ToWslPath -WindowsPath $Root
  $wslInventory = Convert-ToWslPath -WindowsPath $inventoryFile
  $wslVars = Convert-ToWslPath -WindowsPath $varsFile
  $wslIdentity = Convert-ToWslPath -WindowsPath $resolvedIdentity
  $wslBuildArchive = Convert-ToWslPath -WindowsPath $buildArchive
  $wslWorkspaceArchive = if (-not $NoWorkspaceSeed) {
    Convert-ToWslPath -WindowsPath $workspaceArchive
  }
  else {
    $wslBuildArchive
  }
  $wslDataArchive = if ($seedAccounts) {
    Convert-ToWslPath -WindowsPath $dataArchive
  }
  else {
    $wslBuildArchive
  }

  $sshCommonArgs = @(
    "-o BatchMode=yes",
    "-o ServerAliveInterval=30",
    "-o ServerAliveCountMax=4"
  )
  if ($resolvedKnownHostsFile) {
    $wslKnownHosts = Convert-ToWslPath -WindowsPath $resolvedKnownHostsFile
    $sshCommonArgs += "-o UserKnownHostsFile=$(Quote-SshOptionPath $wslKnownHosts)"
    $sshCommonArgs += "-o StrictHostKeyChecking=yes"
  }
  elseif ($AcceptNewHostKey) {
    $sshCommonArgs += "-o StrictHostKeyChecking=accept-new"
  }

  $inventory = [ordered]@{
    all = [ordered]@{
      children = [ordered]@{
        cst_nodes = [ordered]@{
          hosts = [ordered]@{
            cst_target = [ordered]@{
              ansible_host = $targetParts[1]
              ansible_user = $targetParts[0]
              ansible_port = $SshPort
              ansible_python_interpreter = "/usr/bin/python3"
              ansible_ssh_common_args = ($sshCommonArgs -join " ")
            }
          }
        }
      }
    }
  }
  Write-Utf8NoBom -Path $inventoryFile -Content (($inventory | ConvertTo-Json -Depth 12) + "`n")

  $variables = [ordered]@{
    cst_node_id = $NodeId
    cst_node_label = $NodeLabel
    cst_capacity = $Capacity
    cst_remote_port = $RemotePort
    cst_public_base_url = $PublicBaseUrl
    cst_gpu_transcription = [bool]$GpuTranscription
    cst_transcription_model = $TranscriptionModel.Trim()
    cst_speaches_image = $SpeachesImage.Trim()
    cst_release = $release
    cst_commit = $commit
    cst_image = $Image.Trim()
    cst_build_on_vps = -not [bool]$Image.Trim()
    cst_build_context_archive = $wslBuildArchive
    cst_seed_accounts = $seedAccounts
    cst_data_seed_archive = $wslDataArchive
    cst_seed_workspace = -not $NoWorkspaceSeed
    cst_workspace_seed_archive = $wslWorkspaceArchive
    cst_workspace_name = $WorkspaceName
    cst_admin_token = $adminToken
    cst_git_pat = $localGitPat
    cst_extra_env = $forwardedEnvironment
    cst_registry = $registry
    cst_registry_username = $RegistryUsername.Trim()
    cst_registry_token = $registryToken
  }
  Write-Utf8NoBom -Path $varsFile -Content (($variables | ConvertTo-Json -Depth 12) + "`n")

  $wslRunner = "$wslRoot/scripts/run-ansible-deploy.sh"
  Write-Host "Deploiement Ansible + Docker Compose vers $SshTarget..." -ForegroundColor Cyan
  $previousErrorAction = $ErrorActionPreference
  try {
    $ErrorActionPreference = "Continue"
    & wsl.exe -d $WslDistribution -e bash $wslRunner `
      --root $wslRoot `
      --inventory $wslInventory `
      --vars $wslVars `
      --identity $wslIdentity
    $deployExit = $LASTEXITCODE
  }
  finally {
    $ErrorActionPreference = $previousErrorAction
  }
  if ($deployExit -ne 0) {
    throw "Le deploiement Ansible + Docker Compose a echoue (code $deployExit)."
  }

  New-Item -ItemType Directory -Force -Path $ProfileDir | Out-Null
  $profile = [ordered]@{
    version = 3
    id = $NodeId
    label = $NodeLabel
    sshTarget = $SshTarget
    sshPort = $SshPort
    identityFile = $resolvedIdentity
    knownHostsFile = $resolvedKnownHostsFile
    remotePort = $RemotePort
    publicBaseUrl = $PublicBaseUrl
    defaultLocalPort = $LocalPort
    capacity = $Capacity
    gpuTranscription = [bool]$GpuTranscription
    transcriptionModel = $TranscriptionModel.Trim()
    transcriptionUrl = $TranscriptionUrl.Trim()
    transcriptionAccelerator = $TranscriptionAccelerator
    speachesImage = $SpeachesImage.Trim()
    tokenProtected = (Protect-Secret $adminToken)
    deploymentMode = "ansible-compose"
    image = if ($Image.Trim()) { $Image.Trim() } else { "codex-switch-terminal:$release" }
    workspace = "/srv/cst/workspaces/$WorkspaceName"
  }
  $profile | ConvertTo-Json | Set-Content -LiteralPath $ProfilePath -Encoding UTF8

  Write-Host ""
  Write-Host "Noeud portable operationnel." -ForegroundColor Green
  Write-Host "Profil      : $NodeId"
  Write-Host "Runtime     : $SshTarget (127.0.0.1:$RemotePort)"
  Write-Host "Workspace   : /srv/cst/workspaces/$WorkspaceName"
  Write-Host "Transcrire  : $(if ($GpuTranscription) { "GPU du VPS · $($TranscriptionModel.Trim())" } elseif ($TranscriptionUrl.Trim()) { "$TranscriptionAccelerator distant · $($TranscriptionModel.Trim())" } else { "configuration externe" })"
  Write-Host "Connexion   : npm run connect:vps -- -Profile $NodeId"
}
finally {
  if (Test-Path -LiteralPath $tempRoot) {
    Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
  }
}

if ($Connect) {
  & (Join-Path $ScriptDir "connect-vps.ps1") -Profile $NodeId -LocalPort $LocalPort
}
