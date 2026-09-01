<#
.SYNOPSIS
  Construit l'APK Android (coque WebView) de Codex Switch Terminal.

.DESCRIPTION
  Detecte le JDK fourni par Android Studio (jbr) et le SDK Android, puis lance
  le wrapper Gradle du dossier android/. Copie l'APK produit a la racine du repo
  et sur le Bureau.

.PARAMETER Release
  Construit un APK release (non signe) au lieu du debug. Par defaut : debug
  (signe automatiquement avec la cle debug, donc installable directement).

.EXAMPLE
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-android-apk.ps1
#>
param(
  [switch]$Release
)

$ErrorActionPreference = "Stop"

# PSModulePath : remet les modules Windows PowerShell en tete (Get-FileHash introuvable sinon dans le sandbox).
& (Join-Path $PSScriptRoot 'switch-development-ensure-system-module-path.ps1')

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$Root = Split-Path -Parent $ScriptDir
$AndroidDir = Join-Path $Root "android"

# Refuse de produire un APK si les versions ou les tests frontend divergent.
Push-Location $Root
try {
  npm run verify:quick
  if ($LASTEXITCODE -ne 0) { throw "La verification avant build Android a echoue." }
}
finally {
  Pop-Location
}

# --- JDK (fourni par Android Studio) ---
$JbrCandidates = @(
  "C:\Program Files\Android\Android Studio\jbr",
  "C:\Program Files\Android\Android Studio1\jbr",
  (Join-Path $env:LOCALAPPDATA "Programs\Android Studio\jbr")
)
$Jbr = $JbrCandidates | Where-Object { Test-Path (Join-Path $_ "bin\java.exe") } | Select-Object -First 1
if (-not $Jbr) {
  if ($env:JAVA_HOME -and (Test-Path (Join-Path $env:JAVA_HOME "bin\java.exe"))) {
    $Jbr = $env:JAVA_HOME
  } else {
    throw "JDK introuvable. Installe Android Studio (fournit un JDK dans jbr) ou definis JAVA_HOME."
  }
}

# --- SDK Android ---
$Sdk = if ($env:ANDROID_HOME) { $env:ANDROID_HOME }
       elseif ($env:ANDROID_SDK_ROOT) { $env:ANDROID_SDK_ROOT }
       else { Join-Path $env:LOCALAPPDATA "Android\Sdk" }
if (-not (Test-Path $Sdk)) {
  throw "SDK Android introuvable ($Sdk). Installe-le via Android Studio ou definis ANDROID_HOME."
}

$env:JAVA_HOME = $Jbr
$env:ANDROID_HOME = $Sdk
$env:ANDROID_SDK_ROOT = $Sdk

& node (Join-Path $ScriptDir "clean-build-artifacts.mjs") android
if ($LASTEXITCODE -ne 0) { throw "Le nettoyage des anciens builds Android a echoue." }

$DesktopDir = [Environment]::GetFolderPath("Desktop")
foreach ($oldApk in @(
  (Join-Path $DesktopDir "CodexTerminal-debug.apk"),
  (Join-Path $DesktopDir "CodexTerminal-release.apk")
)) {
  Remove-Item -LiteralPath $oldApk -Force -ErrorAction SilentlyContinue
}

Write-Host "JDK  : $Jbr"       -ForegroundColor Cyan
Write-Host "SDK  : $Sdk"       -ForegroundColor Cyan
Write-Host "Proj : $AndroidDir" -ForegroundColor Cyan

$Tasks = if ($Release) {
  @("lintRelease", "assembleRelease")
} else {
  @("lintDebug", "assembleDebug")
}

Push-Location $AndroidDir
try {
  & (Join-Path $AndroidDir "gradlew.bat") @Tasks --console=plain
  if ($LASTEXITCODE -ne 0) { throw "Echec du build Gradle ($($Tasks -join ', '))." }
}
finally {
  Pop-Location
}

$Variant = if ($Release) { "release" } else { "debug" }
$ApkName = if ($Release) { "app-release-unsigned.apk" } else { "app-debug.apk" }
$ApkPath = Join-Path $AndroidDir "app\build\outputs\apk\$Variant\$ApkName"
if (-not (Test-Path $ApkPath)) {
  # Fallback : premier APK trouve dans le dossier de sortie.
  $ApkPath = Get-ChildItem -Path (Join-Path $AndroidDir "app\build\outputs\apk\$Variant") -Filter *.apk -ErrorAction SilentlyContinue |
    Select-Object -First 1 -ExpandProperty FullName
}
if (-not $ApkPath -or -not (Test-Path $ApkPath)) {
  throw "APK introuvable apres le build."
}

# Un APK debug doit etre signe et structurellement valide avant d'etre copie.
if (-not $Release) {
  $ApkSigner = Get-ChildItem -Path (Join-Path $Sdk "build-tools") -Directory -ErrorAction SilentlyContinue |
    Sort-Object { try { [version]$_.Name } catch { [version]"0.0" } } -Descending |
    ForEach-Object { Join-Path $_.FullName "apksigner.bat" } |
    Where-Object { Test-Path $_ } |
    Select-Object -First 1
  if (-not $ApkSigner) {
    throw "apksigner introuvable dans le SDK Android."
  }
  & $ApkSigner verify --verbose --print-certs $ApkPath
  if ($LASTEXITCODE -ne 0) { throw "La signature de l'APK debug est invalide." }
}

$Dest1 = Join-Path $Root "CodexTerminal-$Variant.apk"
$Dest2 = Join-Path $DesktopDir "CodexTerminal-$Variant.apk"
Copy-Item $ApkPath $Dest1 -Force
Copy-Item $ApkPath $Dest2 -Force
$Hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $Dest1).Hash
$SizeMb = [Math]::Round((Get-Item -LiteralPath $Dest1).Length / 1MB, 2)

Write-Host ""
Write-Host "APK genere :" -ForegroundColor Green
Write-Host "  $ApkPath"
Write-Host "  -> $Dest1"
Write-Host "  -> $Dest2"
Write-Host "  Taille : $SizeMb Mo"
Write-Host "  SHA-256: $Hash"
Write-Host ""
Write-Host "Installer par USB (debogage USB active sur le tel) :" -ForegroundColor Yellow
Write-Host "  `"$Sdk\platform-tools\adb.exe`" install -r `"$Dest1`""
