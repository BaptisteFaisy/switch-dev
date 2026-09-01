param(
  [ValidateSet("base", "small-q5_1", "small", "large-v3-turbo-q5_0")]
  [string]$WhisperModel = "large-v3-turbo-q5_0",
  [string]$SummaryModel = "qwen3:4b-instruct-2507-q4_K_M",
  [string]$VoiceHome = "",
  [switch]$Force,
  [switch]$SkipOllamaPull
)

$ErrorActionPreference = "Stop"

# PSModulePath : remet les modules Windows PowerShell en tete (Get-FileHash introuvable sinon dans le sandbox).
& (Join-Path $PSScriptRoot 'switch-development-ensure-system-module-path.ps1')
$ProgressPreference = "SilentlyContinue"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

if (-not $VoiceHome.Trim()) {
  if (-not $env:APPDATA) {
    throw "APPDATA est introuvable. Passe explicitement -VoiceHome <dossier>."
  }
  $VoiceHome = Join-Path $env:APPDATA "codex-switch-terminal\voice"
}
$VoiceHome = [IO.Path]::GetFullPath($VoiceHome)
$WhisperDir = Join-Path $VoiceHome "whisper"
$ModelsDir = Join-Path $VoiceHome "models"
$DownloadsDir = Join-Path $VoiceHome "downloads"

$modelCatalog = @{
  "base" = @{
    File = "ggml-base.bin"
    Sha1 = "465707469ff3a37a2b9b8d8f89f2f99de7299dac"
  }
  "small-q5_1" = @{
    File = "ggml-small-q5_1.bin"
    Sha1 = "6fe57ddcfdd1c6b07cdcc73aaf620810ce5fc771"
  }
  "small" = @{
    File = "ggml-small.bin"
    Sha1 = "55356645c2b361a969dfd0ef2c5a50d530afd8d5"
  }
  "large-v3-turbo-q5_0" = @{
    File = "ggml-large-v3-turbo-q5_0.bin"
    Sha1 = "e050f7970618a659205450ad97eb95a18d69c9ee"
  }
}

function Download-File {
  param(
    [Parameter(Mandatory = $true)][string]$Uri,
    [Parameter(Mandatory = $true)][string]$Destination
  )
  $partial = "$Destination.partial"
  if (Test-Path -LiteralPath $partial) {
    Remove-Item -LiteralPath $partial -Force
  }
  $curl = Get-Command curl.exe -ErrorAction SilentlyContinue
  if ($curl) {
    & $curl.Source -L --fail --retry 3 --output $partial $Uri
    if ($LASTEXITCODE -ne 0) {
      throw "Telechargement impossible : $Uri"
    }
  }
  else {
    Invoke-WebRequest -UseBasicParsing -Uri $Uri -OutFile $partial
  }
  Move-Item -LiteralPath $partial -Destination $Destination -Force
}

function Find-WhisperCli {
  if (-not (Test-Path -LiteralPath $WhisperDir)) { return $null }
  return Get-ChildItem -LiteralPath $WhisperDir -Recurse -File -Filter "whisper-cli.exe" |
    Select-Object -First 1
}

function Install-CublasRuntime {
  param(
    [Parameter(Mandatory = $true)][string]$Destination
  )

  $requiredDlls = @("cublas64_11.dll", "cublasLt64_11.dll")
  $missingDlls = $requiredDlls | Where-Object {
    -not (Test-Path -LiteralPath (Join-Path $Destination $_))
  }
  if (-not $missingDlls -and -not $Force) { return }

  Write-Host "Installation du runtime NVIDIA cuBLAS 11 pour Whisper..." -ForegroundColor Cyan
  $manifestUri = "https://developer.download.nvidia.com/compute/cuda/redist/redistrib_11.8.0.json"
  $manifest = Invoke-RestMethod -Uri $manifestUri
  $package = $manifest.libcublas.'windows-x86_64'
  if (-not $package.relative_path -or -not $package.sha256) {
    throw "Le manifeste NVIDIA CUDA 11.8 ne contient pas le runtime cuBLAS Windows x64."
  }

  $archiveName = Split-Path -Leaf $package.relative_path
  $archive = Join-Path $DownloadsDir $archiveName
  if (-not (Test-Path -LiteralPath $archive) -or $Force) {
    $sizeMb = [Math]::Round(([double]$package.size / 1MB), 0)
    Write-Host "Telechargement de $archiveName ($sizeMb Mo)..." -ForegroundColor Cyan
    Download-File `
      -Uri "https://developer.download.nvidia.com/compute/cuda/redist/$($package.relative_path)" `
      -Destination $archive
  }

  $archiveHash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
  if ($archiveHash -ne $package.sha256.ToLowerInvariant()) {
    throw "Somme SHA256 invalide pour $archive."
  }

  $extractDir = [IO.Path]::GetFullPath((Join-Path $DownloadsDir "nvidia-cublas-11.8"))
  $downloadsRoot = [IO.Path]::GetFullPath("$DownloadsDir$([IO.Path]::DirectorySeparatorChar)")
  if (-not $extractDir.StartsWith($downloadsRoot, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Dossier temporaire cuBLAS hors du dossier de telechargement : $extractDir"
  }
  if (Test-Path -LiteralPath $extractDir) {
    Remove-Item -LiteralPath $extractDir -Recurse -Force
  }
  Expand-Archive -LiteralPath $archive -DestinationPath $extractDir -Force

  foreach ($dllName in $requiredDlls) {
    $source = Get-ChildItem -LiteralPath $extractDir -Recurse -File -Filter $dllName |
      Select-Object -First 1
    if (-not $source) {
      throw "$dllName est absent de l'archive NVIDIA $archiveName."
    }
    Copy-Item -LiteralPath $source.FullName -Destination (Join-Path $Destination $dllName) -Force
  }

  Remove-Item -LiteralPath $extractDir -Recurse -Force
  Remove-Item -LiteralPath $archive -Force
}

$ollama = Get-Command ollama.exe -ErrorAction SilentlyContinue
if (-not $ollama) {
  $ollama = Get-Command ollama -ErrorAction SilentlyContinue
}
if (-not $ollama) {
  throw @"
Ollama est introuvable.
Installe Ollama pour Windows depuis https://ollama.com/download/windows,
rouvre PowerShell, puis relance ce script.
"@
}

Write-Host "Verification du GPU NVIDIA..." -ForegroundColor Cyan
$nvidiaSmi = Get-Command nvidia-smi.exe -ErrorAction SilentlyContinue
if ($nvidiaSmi) {
  & $nvidiaSmi.Source --query-gpu=name,memory.total,driver_version --format=csv,noheader
}
else {
  Write-Warning "nvidia-smi est introuvable. Mets le pilote NVIDIA a jour avant d'utiliser Whisper CUDA."
}

New-Item -ItemType Directory -Force -Path $WhisperDir, $ModelsDir, $DownloadsDir | Out-Null

$whisperCli = Find-WhisperCli
if (-not $whisperCli -or $Force) {
  Write-Host "Recherche de la derniere version officielle de whisper.cpp..." -ForegroundColor Cyan
  $headers = @{ "User-Agent" = "Codex-Switch-Terminal-voice-setup" }
  $release = Invoke-RestMethod `
    -Headers $headers `
    -Uri "https://api.github.com/repos/ggml-org/whisper.cpp/releases/latest"
  $asset = $release.assets |
    Where-Object { $_.name -eq "whisper-cublas-11.8.0-bin-x64.zip" } |
    Select-Object -First 1
  if (-not $asset) {
    $asset = $release.assets |
      Where-Object { $_.name -match '^whisper-cublas-.*-bin-x64\.zip$' } |
      Sort-Object size |
      Select-Object -First 1
  }
  if (-not $asset) {
    throw "La release whisper.cpp $($release.tag_name) ne contient aucun binaire CUDA Windows x64."
  }

  $archive = Join-Path $DownloadsDir $asset.name
  if (-not (Test-Path -LiteralPath $archive) -or $Force) {
    $sizeMb = [Math]::Round(([double]$asset.size / 1MB), 0)
    Write-Host "Telechargement de $($asset.name) ($sizeMb Mo)..." -ForegroundColor Cyan
    Download-File -Uri $asset.browser_download_url -Destination $archive
  }
  Write-Host "Installation de whisper.cpp CUDA..." -ForegroundColor Cyan
  Expand-Archive -LiteralPath $archive -DestinationPath $WhisperDir -Force
  $whisperCli = Find-WhisperCli
  if (-not $whisperCli) {
    throw "whisper-cli.exe est absent apres extraction de $archive."
  }
  Remove-Item -LiteralPath $archive -Force
}
Write-Host "Whisper : $($whisperCli.FullName)" -ForegroundColor Green
Install-CublasRuntime -Destination $whisperCli.DirectoryName

$modelInfo = $modelCatalog[$WhisperModel]
$modelPath = Join-Path $ModelsDir $modelInfo.File
$modelValid = $false
if (Test-Path -LiteralPath $modelPath) {
  $modelHash = (Get-FileHash -LiteralPath $modelPath -Algorithm SHA1).Hash.ToLowerInvariant()
  $modelValid = $modelHash -eq $modelInfo.Sha1
  if (-not $modelValid -and -not $Force) {
    throw "Le fichier $modelPath existe mais sa somme SHA1 est incorrecte. Relance avec -Force."
  }
}
if (-not $modelValid -or $Force) {
  $modelUrl = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/$($modelInfo.File)?download=true"
  Write-Host "Telechargement du modele Whisper $WhisperModel..." -ForegroundColor Cyan
  Download-File -Uri $modelUrl -Destination $modelPath
  $modelHash = (Get-FileHash -LiteralPath $modelPath -Algorithm SHA1).Hash.ToLowerInvariant()
  if ($modelHash -ne $modelInfo.Sha1) {
    throw "Somme SHA1 invalide pour $modelPath. Attendu: $($modelInfo.Sha1), recu: $modelHash"
  }
}
Write-Host "Modele STT : $modelPath" -ForegroundColor Green

if (-not $SkipOllamaPull) {
  Write-Host "Telechargement du petit modele de resume $SummaryModel..." -ForegroundColor Cyan
  & $ollama.Source pull $SummaryModel
  if ($LASTEXITCODE -ne 0) {
    throw "ollama pull $SummaryModel a echoue. Verifie qu'Ollama est lance."
  }
}

$voiceConfig = [ordered]@{
  transcriptionMode = "local"
  transcriptionAccelerator = "gpu"
  whisperModel = $modelInfo.File
  ollamaModel = $SummaryModel
  ollamaUrl = "http://127.0.0.1:11434"
} | ConvertTo-Json
$utf8WithoutBom = [Text.UTF8Encoding]::new($false)
[IO.File]::WriteAllText((Join-Path $VoiceHome "config.json"), "$voiceConfig`n", $utf8WithoutBom)

Write-Host ""
Write-Host "Saisie vocale locale prete." -ForegroundColor Green
Write-Host "Dossier       : $VoiceHome"
Write-Host "Transcription : $WhisperModel (whisper.cpp CUDA)"
Write-Host "Resume        : $SummaryModel (Ollama)"
Write-Host ""
Write-Host "Redemarre Codex Switch Terminal, ouvre un chat, puis clique sur le micro."
Write-Host "Pour utiliser un modele de resume plus leger :"
Write-Host "  ollama pull qwen3:1.7b-q4_K_M"
Write-Host '  $env:CST_VOICE_OLLAMA_MODEL = "qwen3:1.7b-q4_K_M"'
