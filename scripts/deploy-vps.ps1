[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [ValidatePattern('^[A-Za-z0-9._-]+@[A-Za-z0-9._:-]+$')]
  [string]$SshTarget,
  [string]$IdentityFile = "",
  [string]$KnownHostsFile = "",
  [ValidateRange(1, 65535)]
  [int]$SshPort = 22,
  [ValidateRange(1, 65535)]
  [int]$RemotePort = 8080,
  [ValidateRange(1, 65535)]
  [int]$LocalPort = 8080,
  [ValidatePattern('^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$')]
  [string]$NodeId = "vps",
  [string]$NodeLabel = "VPS",
  [ValidateRange(1, 1024)]
  [int]$Capacity = 1,
  [string]$AdminTokenFile = "",
  [switch]$SkipAccountSeed,
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
$ProfileDir = Join-Path $env:APPDATA "codex-switch-terminal\vps"
$ProfilePath = Join-Path $ProfileDir "$NodeId.json"

function Assert-OneLine {
  param([string]$Name, [AllowEmptyString()][string]$Value)
  if ($Value.Contains("`r") -or $Value.Contains("`n")) {
    throw "$Name doit tenir sur une seule ligne."
  }
}

function Quote-SystemdEnvironment {
  param([AllowEmptyString()][string]$Value)
  Assert-OneLine -Name "Valeur de configuration" -Value $Value
  return '"' + $Value.Replace('\', '\\').Replace('"', '\"') + '"'
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

function Get-LineSha256 {
  param([string]$Value)
  $sha = [Security.Cryptography.SHA256]::Create()
  try {
    $bytes = [Text.Encoding]::UTF8.GetBytes($Value + "`n")
    return ([BitConverter]::ToString($sha.ComputeHash($bytes))).Replace('-', '').ToLowerInvariant()
  }
  finally {
    $sha.Dispose()
  }
}

Assert-OneLine -Name "NodeLabel" -Value $NodeLabel

foreach ($command in @("tar", "ssh", "scp", "npm", "git")) {
  if (-not (Get-Command $command -ErrorAction SilentlyContinue)) {
    throw "Commande requise introuvable: $command"
  }
}

$resolvedIdentity = ""
if ($IdentityFile.Trim()) {
  $resolvedIdentity = (Resolve-Path -LiteralPath $IdentityFile).Path
}
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
    throw "Le port distant d'un noeud existant ne peut pas etre change pendant une mise a jour SSH."
  }
}

$localAdminToken = ""
$localGitPat = ""
if (Test-Path -LiteralPath $LocalEnvFile) {
  $previousAdminToken = $env:CST_ADMIN_TOKEN
  $previousGitPat = $env:CST_GIT_PAT
  try {
    . $LocalEnvFile
    $localAdminToken = [string]$env:CST_ADMIN_TOKEN
    $localGitPat = [string]$env:CST_GIT_PAT
  }
  finally {
    $env:CST_ADMIN_TOKEN = $previousAdminToken
    $env:CST_GIT_PAT = $previousGitPat
  }
}

if ($existingProfile -and [string]$existingProfile.tokenProtected) {
  $adminToken = Unprotect-Secret ([string]$existingProfile.tokenProtected)
}
elseif ($AdminTokenFile.Trim()) {
  $adminToken = (Get-Content -LiteralPath (Resolve-Path -LiteralPath $AdminTokenFile).Path -Raw).Trim()
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

$bindLine = 'CST_BIND=' + (Quote-SystemdEnvironment "127.0.0.1:$RemotePort")
$tokenLine = 'CST_ADMIN_TOKEN=' + (Quote-SystemdEnvironment $adminToken)
$bindLineHash = Get-LineSha256 $bindLine
$tokenLineHash = Get-LineSha256 $tokenLine
$publicBaseUrl = "http://127.0.0.1:$RemotePort"

$tempRoot = Join-Path $env:TEMP ("cst-vps-" + [guid]::NewGuid().ToString("N"))
$sourceArchive = Join-Path $tempRoot "cst-source.tar.gz"
$dataArchive = Join-Path $tempRoot "cst-data.tar.gz"
$remoteEnv = Join-Path $tempRoot "codex-switch-terminal.env"
$remoteId = [guid]::NewGuid().ToString("N")
$remoteDir = "/tmp/cst-deploy-$remoteId"
$remoteCreated = $false

$sshArgs = @(
  "-p", [string]$SshPort,
  "-o", "BatchMode=yes",
  "-o", "ServerAliveInterval=30",
  "-o", "ServerAliveCountMax=10"
)
$scpArgs = @(
  "-P", [string]$SshPort,
  "-o", "BatchMode=yes",
  "-o", "ServerAliveInterval=30",
  "-o", "ServerAliveCountMax=10"
)
if ($resolvedIdentity) {
  $sshArgs += @("-i", $resolvedIdentity)
  $scpArgs += @("-i", $resolvedIdentity)
}
if ($resolvedKnownHostsFile) {
  $sshArgs += @(
    "-o", "UserKnownHostsFile=$resolvedKnownHostsFile",
    "-o", "StrictHostKeyChecking=yes"
  )
  $scpArgs += @(
    "-o", "UserKnownHostsFile=$resolvedKnownHostsFile",
    "-o", "StrictHostKeyChecking=yes"
  )
}
if ($AcceptNewHostKey) {
  $sshArgs += @("-o", "StrictHostKeyChecking=accept-new")
  $scpArgs += @("-o", "StrictHostKeyChecking=accept-new")
}

New-Item -ItemType Directory -Force -Path $tempRoot | Out-Null
try {
  Write-Host "Verification de l'acces SSH et des privileges..." -ForegroundColor Cyan
  & ssh @sshArgs $SshTarget 'if [ "$(id -u)" -eq 0 ]; then exit 0; fi; command -v sudo >/dev/null 2>&1 && sudo -n true'
  if ($LASTEXITCODE -ne 0) {
    throw "Le VPS doit etre joignable par cle SSH avec root, ou avec un utilisateur autorise a executer sudo sans mot de passe."
  }

  & ssh @sshArgs $SshTarget "install -d -m 0700 '$remoteDir'"
  if ($LASTEXITCODE -ne 0) { throw "Impossible de preparer le dossier temporaire distant." }
  $remoteCreated = $true

  Push-Location $Root
  try {
    npm run build:frontend
    if ($LASTEXITCODE -ne 0) { throw "Le build frontend a echoue." }

    & tar @(
      "--exclude=src-tauri/target*",
      "-czf", $sourceArchive,
      "dist", "src-tauri", "rust-toolchain.toml"
    )
    if ($LASTEXITCODE -ne 0) { throw "La creation de l'archive source a echoue." }
    $sourceDigest = (Get-FileHash -LiteralPath $sourceArchive -Algorithm SHA256).Hash.ToLowerInvariant()
  }
  finally {
    Pop-Location
  }

  $hasDataArchive = $false
  if (-not $SkipAccountSeed) {
    if ((Test-Path -LiteralPath $SettingsFile) -and (Test-Path -LiteralPath $CodexHomes)) {
      Push-Location $DataDir
      try {
        & tar -czf $dataArchive "settings.json" "codex-homes"
        if ($LASTEXITCODE -ne 0) { throw "La sauvegarde initiale des comptes a echoue." }
        $hasDataArchive = $true
      }
      finally {
        Pop-Location
      }
    }
    else {
      Write-Warning "Aucun compte local pret dans $DataDir; le VPS demarrera sans compte."
    }
  }

  $environmentLines = @(
    $bindLine
    'CST_DATA_DIR="/srv/cst"'
    'CST_WORKSPACES_ROOT="/srv/cst/workspaces"'
    'CST_STATIC_DIR="/opt/codex-switch-terminal/current/dist"'
    'CST_PUBLIC_BASE_URL=' + (Quote-SystemdEnvironment $publicBaseUrl)
    'CST_ALLOWED_ORIGINS=' + (Quote-SystemdEnvironment $publicBaseUrl)
    $tokenLine
    'CST_GIT_PAT=' + (Quote-SystemdEnvironment $localGitPat)
    'CST_NODE_ID=' + (Quote-SystemdEnvironment $NodeId)
    'CST_NODE_LABEL=' + (Quote-SystemdEnvironment $NodeLabel)
    'CST_NODE_CAPACITY=' + (Quote-SystemdEnvironment ([string]$Capacity))
  )
  foreach ($name in @(
    'CST_ALLOW_REGISTRATION'
    'CST_AUTH_SECURE_COOKIE'
    'CST_GOOGLE_CLIENT_ID'
    'CST_GOOGLE_CLIENT_SECRET'
    'CST_GOOGLE_REDIRECT_URI'
    'CST_MICROSOFT_CLIENT_ID'
    'CST_MICROSOFT_CLIENT_SECRET'
    'CST_MICROSOFT_TENANT_ID'
    'CST_MICROSOFT_REDIRECT_URI'
    'CST_MICROSOFT_SCOPES'
    'CST_REVOLUT_PUBLIC_KEY'
    'CST_REVOLUT_SECRET_KEY'
    'CST_REVOLUT_CURRENCY'
    'CST_REVOLUT_SANDBOX'
    'CST_REVOLUT_MERCHANT_BASE_URL'
    'CST_REVOLUT_RETURN_URL'
    'CST_REVOLUT_WEBHOOK_URL'
    'CST_VOICE_TRANSCRIPTION_MODE'
    'CST_VOICE_TRANSCRIPTION_URL'
    'CST_VOICE_TRANSCRIPTION_MODEL'
    'CST_VOICE_TRANSCRIPTION_API_KEY'
    'CST_VOICE_TRANSCRIPTION_ACCELERATOR'
    'CST_VOICE_OLLAMA_URL'
    'CST_VOICE_OLLAMA_MODEL'
    'CST_VOICE_OLLAMA_API_KEY'
    'CST_VOICE_ALLOW_INSECURE_REMOTE'
  )) {
    $value = [Environment]::GetEnvironmentVariable($name)
    if ($null -ne $value -and $value.Trim()) {
      $environmentLines += $name + '=' + (Quote-SystemdEnvironment $value.Trim())
    }
  }
  [IO.File]::WriteAllText(
    $remoteEnv,
    (($environmentLines -join "`n") + "`n"),
    (New-Object Text.UTF8Encoding($false))
  )

  $transferFiles = @(
    $sourceArchive,
    $remoteEnv,
    (Join-Path $Root "deploy\install-vps-node.sh"),
    (Join-Path $Root "deploy\update-node.sh"),
    (Join-Path $Root "deploy\codex-switch-terminal.service")
  )
  if ($hasDataArchive) { $transferFiles += $dataArchive }

  Write-Host "Transfert chiffre vers $SshTarget..." -ForegroundColor Cyan
  & scp @scpArgs @transferFiles "${SshTarget}:$remoteDir/"
  if ($LASTEXITCODE -ne 0) { throw "Le transfert SCP a echoue." }

  $commit = "archive-$($sourceDigest.Substring(0, 12))"
  $rev = & git -C $Root rev-parse --short HEAD 2>$null
  if ($LASTEXITCODE -eq 0 -and $rev) {
    $commit = ([string]$rev).Trim()
    $worktreeChanges = @(& git -C $Root status --porcelain --untracked-files=normal 2>$null)
    if ($LASTEXITCODE -eq 0 -and $worktreeChanges.Count -gt 0) {
      $commit = "$commit-dirty-$($sourceDigest.Substring(0, 12))"
    }
  }
  if ($commit -notmatch '^[A-Za-z0-9._-]+$') { $commit = "unknown" }

  $remoteDataPath = if ($hasDataArchive) { "$remoteDir/cst-data.tar.gz" } else { "$remoteDir/no-data.tar.gz" }
  $remoteCommand = @'
set -eu
as_root() {
  if [ "$(id -u)" -eq 0 ]; then
    "$@"
  else
    sudo -n "$@"
  fi
}
if [ -f /opt/codex-switch-terminal/.installed ] && systemctl list-unit-files 2>/dev/null | grep -q '^codex-switch-terminal.service' && [ -x /opt/codex-switch-terminal/current/cst-server ]; then
  current_token_hash="$(as_root grep -m1 '^CST_ADMIN_TOKEN=' /etc/codex-switch-terminal.env | sha256sum | awk '{print $1}')"
  current_bind_hash="$(as_root grep -m1 '^CST_BIND=' /etc/codex-switch-terminal.env | sha256sum | awk '{print $1}')"
  [ "$current_token_hash" = "__TOKEN_HASH__" ] || exit 42
  [ "$current_bind_hash" = "__BIND_HASH__" ] || exit 43
  as_root install -m 0640 -o root -g cst '__REMOTE_DIR__/codex-switch-terminal.env' /etc/codex-switch-terminal.env
  as_root install -m 0644 '__REMOTE_DIR__/codex-switch-terminal.service' /etc/systemd/system/codex-switch-terminal.service
  as_root systemctl daemon-reload
  as_root bash '__REMOTE_DIR__/update-node.sh' --source '__REMOTE_DIR__/cst-source.tar.gz' --commit '__COMMIT__'
else
  as_root bash '__REMOTE_DIR__/install-vps-node.sh' --source '__REMOTE_DIR__/cst-source.tar.gz' --data '__DATA_PATH__' --env '__REMOTE_DIR__/codex-switch-terminal.env' --service '__REMOTE_DIR__/codex-switch-terminal.service' --commit '__COMMIT__'
fi
'@
  $remoteCommand = $remoteCommand.Replace('__TOKEN_HASH__', $tokenLineHash)
  $remoteCommand = $remoteCommand.Replace('__BIND_HASH__', $bindLineHash)
  $remoteCommand = $remoteCommand.Replace('__REMOTE_DIR__', $remoteDir)
  $remoteCommand = $remoteCommand.Replace('__DATA_PATH__', $remoteDataPath)
  $remoteCommand = $remoteCommand.Replace('__COMMIT__', $commit)

  Write-Host "Installation du runtime de chats sur le VPS..." -ForegroundColor Cyan
  & ssh @sshArgs $SshTarget $remoteCommand
  $deployExitCode = $LASTEXITCODE
  if ($deployExitCode -eq 42) {
    throw "Le token du profil local ne correspond pas au noeud deja installe; rotation refusee pendant une mise a jour."
  }
  if ($deployExitCode -eq 43) {
    throw "Le port d'ecoute du noeud deja installe differe; sa modification automatique est refusee pendant une mise a jour."
  }
  if ($deployExitCode -ne 0) { throw "Le deploiement VPS a echoue (code $deployExitCode)." }

  & ssh @sshArgs $SshTarget "curl -fsS --max-time 5 'http://127.0.0.1:$RemotePort/healthz' >/dev/null"
  if ($LASTEXITCODE -ne 0) {
    Write-Warning "La sonde a echoue; derniers journaux du service distant:"
    & ssh @sshArgs $SshTarget 'if [ "$(id -u)" -eq 0 ]; then journalctl -u codex-switch-terminal -n 80 --no-pager; else sudo -n journalctl -u codex-switch-terminal -n 80 --no-pager; fi' 2>$null
    throw "Le service distant est installe mais sa sonde de sante echoue. Les journaux ci-dessus indiquent la cause."
  }

  New-Item -ItemType Directory -Force -Path $ProfileDir | Out-Null
  $profile = [ordered]@{
    version = 1
    id = $NodeId
    label = $NodeLabel
    sshTarget = $SshTarget
    sshPort = $SshPort
    identityFile = $resolvedIdentity
    knownHostsFile = $resolvedKnownHostsFile
    remotePort = $RemotePort
    defaultLocalPort = $LocalPort
    tokenProtected = (Protect-Secret $adminToken)
  }
  $profile | ConvertTo-Json | Set-Content -LiteralPath $ProfilePath -Encoding UTF8

  Write-Host ""
  Write-Host "Noeud VPS operationnel et prive." -ForegroundColor Green
  Write-Host "Profil      : $NodeId"
  Write-Host "Runtime     : $SshTarget (127.0.0.1:$RemotePort)"
  Write-Host "Connexion   : npm run connect:vps -- -Profile $NodeId"

}
finally {
  if ($remoteCreated) {
    & ssh @sshArgs $SshTarget "rm -f '$remoteDir/'* && rmdir '$remoteDir'" 2>$null | Out-Null
  }
  if (Test-Path -LiteralPath $tempRoot) {
    Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
  }
}

if ($Connect) {
  $connectArgs = @{ Profile = $NodeId; LocalPort = $LocalPort }
  if ($resolvedKnownHostsFile) { $connectArgs.KnownHostsFile = $resolvedKnownHostsFile }
  & (Join-Path $ScriptDir "connect-vps.ps1") @connectArgs
}
