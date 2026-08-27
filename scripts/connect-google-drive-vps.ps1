[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [ValidatePattern('^[A-Za-z0-9._-]+@[A-Za-z0-9._:-]+$')]
  [string]$SshTarget,

  [Parameter(Mandatory = $true)]
  [string]$IdentityFile,

  [string]$RemoteName = "cst-google-drive",
  [string]$RemoteConfig = "/etc/rclone/cst-google-drive.conf",
  [string]$RcloneVersion = "1.74.4",
  [string]$ClientCredentialsFile
)

$ErrorActionPreference = "Stop"
$resolvedIdentity = (Resolve-Path -LiteralPath $IdentityFile).Path
$temporaryRoot = Join-Path $env:TEMP ("cst-rclone-auth-" + [guid]::NewGuid().ToString("N"))
$expectedTemporaryParent = [IO.Path]::GetFullPath($env:TEMP).TrimEnd("\") + "\"

try {
  New-Item -ItemType Directory -Path $temporaryRoot | Out-Null

  $archivePath = Join-Path $temporaryRoot "rclone.zip"
  $downloadUrl = "https://downloads.rclone.org/v$RcloneVersion/rclone-v$RcloneVersion-windows-amd64.zip"
  Invoke-WebRequest -Uri $downloadUrl -OutFile $archivePath
  Expand-Archive -LiteralPath $archivePath -DestinationPath $temporaryRoot

  $rclone = Get-ChildItem -LiteralPath $temporaryRoot -Recurse -Filter "rclone.exe" |
    Select-Object -First 1
  if (-not $rclone) {
    throw "rclone.exe est introuvable dans l'archive officielle."
  }

  $localConfig = Join-Path $temporaryRoot "cst-google-drive.conf"
  $authorizeOutput = Join-Path $temporaryRoot "authorize.out"
  $authorizeError = Join-Path $temporaryRoot "authorize.err"
  $configArguments = @(
    "config", "create", $RemoteName, "drive",
    "scope=drive", "config_is_local=true",
    "--config", $localConfig, "--no-output"
  )
  if ($ClientCredentialsFile) {
    $resolvedCredentials = (Resolve-Path -LiteralPath $ClientCredentialsFile).Path
    $credentials = Get-Content -LiteralPath $resolvedCredentials -Raw | ConvertFrom-Json
    if (
      $credentials.client_id -notmatch '^[0-9]+-[A-Za-z0-9]+\.apps\.googleusercontent\.com$' -or
      [string]::IsNullOrWhiteSpace([string]$credentials.client_secret)
    ) {
      throw "Le fichier d'identifiants OAuth est invalide."
    }
    $configArguments = @(
      "config", "create", $RemoteName, "drive",
      "scope=drive", "config_is_local=true",
      "client_id=$($credentials.client_id)",
      "client_secret=$($credentials.client_secret)",
      "--config", $localConfig, "--no-output"
    )
  }
  $authorizeProcess = Start-Process `
    -FilePath $rclone.FullName `
    -ArgumentList $configArguments `
    -RedirectStandardOutput $authorizeOutput `
    -RedirectStandardError $authorizeError `
    -WindowStyle Hidden `
    -Wait `
    -PassThru
  if ($authorizeProcess.ExitCode -ne 0) {
    throw "L'autorisation Google a ete interrompue (code $($authorizeProcess.ExitCode))."
  }

  if (-not (Test-Path -LiteralPath $localConfig)) {
    throw "Le fichier de configuration OAuth Google n'a pas ete cree."
  }

  $remoteTemporaryConfig = "/tmp/cst-google-drive-$([guid]::NewGuid().ToString('N')).conf"
  & scp -q -i $resolvedIdentity -o BatchMode=yes $localConfig "${SshTarget}:$remoteTemporaryConfig"
  if ($LASTEXITCODE -ne 0) {
    throw "Le transfert securise de la configuration a echoue."
  }

  $remoteInstall = "sudo -n install -d -m 0700 /etc/rclone && " +
    "sudo -n install -o root -g root -m 0600 $remoteTemporaryConfig $RemoteConfig && " +
    "rm -f $remoteTemporaryConfig"
  & ssh -i $resolvedIdentity -o BatchMode=yes $SshTarget $remoteInstall
  if ($LASTEXITCODE -ne 0) {
    throw "L'installation distante de la configuration a echoue."
  }

  Write-Host "Autorisation Google Drive installee sur le VPS." -ForegroundColor Green
}
finally {
  if (Test-Path -LiteralPath $temporaryRoot) {
    $resolvedTemporaryRoot = [IO.Path]::GetFullPath($temporaryRoot)
    $temporaryLeaf = Split-Path $resolvedTemporaryRoot -Leaf
    if (
      $resolvedTemporaryRoot.StartsWith($expectedTemporaryParent, [StringComparison]::OrdinalIgnoreCase) -and
      $temporaryLeaf.StartsWith("cst-rclone-auth-", [StringComparison]::Ordinal)
    ) {
      Remove-Item -LiteralPath $resolvedTemporaryRoot -Recurse -Force
    }
  }
}
