param(
  [Parameter(Mandatory = $true)]
  [ValidatePattern('^[A-Za-z0-9._-]+@[A-Za-z0-9._:-]+$')]
  [string]$SshTarget,
  [string]$IdentityFile = "",
  [string]$OracleHostName = "oracle-free-cst",
  [string]$OracleUrl = "",
  [int]$Capacity = 1,
  [switch]$SkipTailscaleLogin
)

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$Root = Split-Path -Parent $ScriptDir
# Commit a embarquer dans le binaire distant (pas de .git transfere sur Oracle).
$commit = ""
$rev = & git -C $Root rev-parse --short HEAD 2>$null
if ($LASTEXITCODE -eq 0 -and $rev) { $commit = ([string]$rev).Trim() }
$DataDir = Join-Path $env:APPDATA "codex-switch-terminal-server"
$LocalEnvFile = Join-Path $DataDir "server.local.env.ps1"
$SettingsFile = Join-Path $DataDir "settings.json"
$CodexHomes = Join-Path $DataDir "codex-homes"
$Tailscale = "C:\Program Files\Tailscale\tailscale.exe"

function Quote-SystemdEnvironment {
  param([AllowEmptyString()][string]$Value)
  return '"' + $Value.Replace('\', '\\').Replace('"', '\"') + '"'
}

foreach ($command in @("tar", "ssh", "scp")) {
  if (-not (Get-Command $command -ErrorAction SilentlyContinue)) {
    throw "Commande requise introuvable: $command"
  }
}
if (-not (Test-Path $LocalEnvFile)) {
  throw "Configuration locale introuvable: $LocalEnvFile"
}
if (-not (Test-Path $SettingsFile) -or -not (Test-Path $CodexHomes)) {
  throw "Les comptes locaux ne sont pas prets dans $DataDir"
}

. $LocalEnvFile
if (-not $env:CST_ADMIN_TOKEN) {
  throw "CST_ADMIN_TOKEN est absent de la configuration locale."
}

if (-not $OracleUrl.Trim()) {
  if (-not (Test-Path $Tailscale)) {
    throw "OracleUrl est requis lorsque Tailscale n'est pas installe localement."
  }
  $tailStatus = (& $Tailscale status --json | ConvertFrom-Json)
  $suffix = ([string]$tailStatus.MagicDNSSuffix).Trim().TrimEnd(".")
  if (-not $suffix) {
    throw "MagicDNS est indisponible; indique OracleUrl explicitement."
  }
  $OracleUrl = "https://$OracleHostName.$suffix"
}
$OracleUrl = $OracleUrl.Trim().TrimEnd("/")

$tempRoot = Join-Path $env:TEMP ("cst-oracle-" + [guid]::NewGuid().ToString("N"))
$sourceArchive = Join-Path $tempRoot "cst-source.tar.gz"
$dataArchive = Join-Path $tempRoot "cst-data.tar.gz"
$remoteEnv = Join-Path $tempRoot "codex-switch-terminal.env"

$sshArgs = @()
if ($IdentityFile.Trim()) {
  $resolvedIdentity = (Resolve-Path $IdentityFile).Path
  $sshArgs += @("-i", $resolvedIdentity)
}

New-Item -ItemType Directory -Force -Path $tempRoot | Out-Null
try {
  Push-Location $Root
  try {
    npm run build:frontend
    if ($LASTEXITCODE -ne 0) { throw "Le build frontend a echoue." }

    & tar @(
      "--exclude=src-tauri/target",
      "--exclude=src-tauri/target-alt",
      "-czf", $sourceArchive,
      "dist", "src-tauri", "rust-toolchain.toml"
    )
    if ($LASTEXITCODE -ne 0) { throw "La creation de l'archive source a echoue." }
  }
  finally {
    Pop-Location
  }

  Push-Location $DataDir
  try {
    & tar -czf $dataArchive "settings.json" "codex-homes"
    if ($LASTEXITCODE -ne 0) { throw "La sauvegarde des comptes a echoue." }
  }
  finally {
    Pop-Location
  }

  $environmentLines = @(
    'CST_BIND="127.0.0.1:8080"'
    'CST_DATA_DIR="/srv/cst"'
    'CST_STATIC_DIR="/opt/codex-switch-terminal/current/dist"'
    'CST_PUBLIC_BASE_URL=' + (Quote-SystemdEnvironment $OracleUrl)
    'CST_ALLOWED_ORIGINS=' + (Quote-SystemdEnvironment $OracleUrl)
    'CST_ADMIN_TOKEN=' + (Quote-SystemdEnvironment $env:CST_ADMIN_TOKEN)
    'CST_GIT_PAT=' + (Quote-SystemdEnvironment $env:CST_GIT_PAT)
    'CST_NODE_ID="oracle-free"'
    'CST_NODE_LABEL="Oracle Free"'
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
  )) {
    $value = [Environment]::GetEnvironmentVariable($name)
    if ($null -ne $value -and $value.Trim()) {
      $environmentLines += $name + '=' + (Quote-SystemdEnvironment $value.Trim())
    }
  }
  $environmentLines | Set-Content -Path $remoteEnv -Encoding ASCII

  Write-Host "Transfert chiffre vers $SshTarget..." -ForegroundColor Cyan
  & scp @sshArgs `
    $sourceArchive `
    $dataArchive `
    $remoteEnv `
    (Join-Path $Root "deploy\install-oracle-node.sh") `
    (Join-Path $Root "deploy\update-node.sh") `
    (Join-Path $Root "deploy\codex-switch-terminal.service") `
    "${SshTarget}:/tmp/"
  if ($LASTEXITCODE -ne 0) { throw "Le transfert SCP a echoue." }

  # Premier deploiement -> install complet ; deploiements suivants -> mise a jour
  # sure (drain -> bascule atomique -> verif -> rollback) qui preserve les
  # donnees distantes.
  $remoteCmd = "if systemctl list-unit-files 2>/dev/null | grep -q '^codex-switch-terminal.service' && [ -x /opt/codex-switch-terminal/current/cst-server ]; then sudo bash /tmp/update-node.sh --source /tmp/cst-source.tar.gz --commit '$commit'; else sudo bash /tmp/install-oracle-node.sh --commit '$commit'; fi"
  & ssh @sshArgs $SshTarget $remoteCmd
  if ($LASTEXITCODE -ne 0) { throw "Le deploiement Oracle a echoue." }

  if (-not $SkipTailscaleLogin) {
    Write-Host "Autorise maintenant le noeud Oracle dans la page Tailscale affichee." -ForegroundColor Yellow
    & ssh @sshArgs $SshTarget "sudo tailscale up --hostname=$OracleHostName"
    if ($LASTEXITCODE -ne 0) { throw "La connexion Tailscale Oracle a echoue." }
  }

  & ssh @sshArgs $SshTarget "sudo tailscale serve --bg http://127.0.0.1:8080 && sudo systemctl restart codex-switch-terminal.service"
  if ($LASTEXITCODE -ne 0) { throw "La publication Tailscale Serve Oracle a echoue." }

  Write-Host "" 
  Write-Host "Noeud Oracle operationnel." -ForegroundColor Green
  Write-Host "URL         : $OracleUrl"
  Write-Host "Config app  : Oracle Free|$OracleUrl||20"
}
finally {
  Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
}
