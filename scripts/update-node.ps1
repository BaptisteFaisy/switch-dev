param(
  [int]$Port = 8080,
  [string]$TaskName = "Codex Switch Terminal Node",
  # Delai TOTAL pour trouver un instant sans terminal. Le noeud reste ouvert
  # aux agents pendant cette attente ; le drain n'est active qu'a la bascule.
  [int]$DrainTimeoutSec = 300,
  # Un noeud charge peut repondre lentement alors que ses chats continuent.
  # La sonde reste fail-closed : si ce delai expire, la MAJ est annulee.
  [ValidateRange(3, 60)]
  [int]$ProbeTimeoutSec = 10,
  [ValidateRange(5, 60)]
  [int]$DrainLeaseSec = 20,
  [ValidateRange(1, 60)]
  [int]$StopTimeoutSec = 10,
  [int]$VerifyTimeoutSec = 60,
  [switch]$Force,
  [switch]$SkipBuild,
  [string]$BuiltExePath = "",
  [string]$BuiltDistPath = "",
  # --- Mode release (Phase 2 : artefacts CI signes) ---
  [string]$ReleaseTag = "",
  [string]$Repo = "BaptisteFaisy/Software-multi-account",
  [string]$Asset = "cst-server-windows-x86_64.zip",
  [string]$MinisignPubKey = "",
  [switch]$AllowUnsigned
)

# Mise a jour SURE d'un noeud Windows.
#
# DEUX modes pour peupler releases\<v> :
#   - build   (defaut)       : compile sur l'hote (ou -SkipBuild pour reutiliser
#                              un binaire deja compile).
#   - release (-ReleaseTag)  : TELECHARGE l'artefact signe de la GitHub Release,
#                              verifie SHA-256 + signature minisign (fail-closed),
#                              puis l'installe.
#
# Sequence commune ensuite : self-check `--version` -> installe dans
# releases\<version-commit> -> attente NON BLOQUANTE activeTerminals==0 ->
# courte lease de drain -> stop tache -> bascule de la jonction
# 'current' -> (re)demarrage tache -> verification "vraiment revenu" -> rollback
# de la jonction si echec.
#
# Layout : %LOCALAPPDATA%\codex-switch-terminal-node\releases\<v>\{cst-server.exe,dist}
#          + jonction 'current'. Donnees separees dans %APPDATA%\codex-switch-terminal-server.

$ErrorActionPreference = "Stop"

# PSModulePath : remet les modules Windows PowerShell en tete (Get-FileHash introuvable sinon dans le sandbox).
& (Join-Path $PSScriptRoot 'switch-development-ensure-system-module-path.ps1')
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
$IsReleaseMode = [bool]$ReleaseTag

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$Root = Split-Path -Parent $ScriptDir
$DataDir = Join-Path $env:APPDATA "codex-switch-terminal-server"
$EnvFile = Join-Path $DataDir "server.local.env.ps1"
$NodeHome = Join-Path $env:LOCALAPPDATA "codex-switch-terminal-node"
$ReleasesDir = Join-Path $NodeHome "releases"
$CurrentLink = Join-Path $NodeHome "current"
$StartNode = Join-Path $ScriptDir "start-node.ps1"

if (-not (Test-Path $EnvFile)) { throw "Config introuvable: $EnvFile (lance setup-pc-node.ps1 d'abord)." }
. $EnvFile
if (-not $env:CST_ADMIN_TOKEN) { throw "CST_ADMIN_TOKEN absent de $EnvFile." }

$base = "http://127.0.0.1:$Port"
$authHeaders = @{ Authorization = "Bearer $env:CST_ADMIN_TOKEN" }

function Get-Healthz {
  try {
    return Invoke-RestMethod -Uri "$base/healthz" -TimeoutSec $ProbeTimeoutSec
  }
  catch {
    return $null
  }
}

function Get-ActiveWorkloadCount {
  param([object]$Health)
  if (-not $Health) { return 0 }

  $activeTerminals = [int]$Health.activeTerminals
  try {
    $chatTurns = Invoke-RestMethod -Uri "$base/api/chat/turns/active" `
      -Headers $authHeaders -TimeoutSec $ProbeTimeoutSec -ErrorAction Stop
  }
  catch {
    throw "Impossible de verifier les tours de chat actifs ; mise a jour annulee sans redemarrage. $($_.Exception.Message)"
  }
  if ($chatTurns -isnot [Array]) {
    throw "Reponse invalide pour les tours de chat actifs ; mise a jour annulee sans redemarrage."
  }
  $activeChatTurns = $chatTurns.Count
  return $activeTerminals + $activeChatTurns
}

function Set-DrainState {
  param([bool]$Draining)
  $body = @{
    draining = $Draining
    ttlSeconds = if ($Draining) { $DrainLeaseSec } else { 0 }
  } | ConvertTo-Json -Compress
  Invoke-RestMethod -Uri "$base/api/admin/drain" -Method Post -Headers $authHeaders `
    -ContentType "application/json" -Body $body -TimeoutSec 5 | Out-Null
}

function Set-Junction {
  param([string]$Link, [string]$Target)
  # rmdir sur une jonction retire UNIQUEMENT le point de reparse (jamais la
  # cible), contrairement a Remove-Item qui peut suivre le lien. mklink /J ne
  # requiert pas de droits admin ni le Developer Mode.
  if (Test-Path $Link) { & cmd /c rmdir "$Link" | Out-Null }
  & cmd /c mklink /J "$Link" "$Target" | Out-Null
  if ($LASTEXITCODE -ne 0) { throw "mklink /J a echoue: $Link -> $Target" }
}

function Test-ReleaseUpdateInProgress {
  param([Parameter(Mandatory = $true)][string]$ReleasePath)

  $marker = Join-Path $ReleasePath ".update-in-progress"
  if (-not (Test-Path -LiteralPath $marker -PathType Leaf)) { return $false }

  try {
    $parts = ([string](Get-Content -LiteralPath $marker -Raw)).Trim() -split '\|'
    if ($parts.Count -ne 2) { throw "Marqueur incomplet" }
    $ownerPid = [int]$parts[0]
    $ownerStartTicks = [long]$parts[1]
    $owner = Get-Process -Id $ownerPid -ErrorAction Stop
    if ($owner.StartTime.ToUniversalTime().Ticks -eq $ownerStartTicks) {
      return $true
    }
  }
  catch {
    # Processus termine ou marqueur incomplet : cette release est orpheline.
  }

  Remove-Item -LiteralPath $marker -Force -ErrorAction SilentlyContinue
  return $false
}

function Remove-ObsoleteLocalReleases {
  param([Parameter(Mandatory = $true)][string]$Keep)

  if (-not (Test-Path -LiteralPath $ReleasesDir -PathType Container)) { return }
  $keepPath = [IO.Path]::GetFullPath($Keep).TrimEnd(
    [IO.Path]::DirectorySeparatorChar,
    [IO.Path]::AltDirectorySeparatorChar
  )
  $removed = 0

  foreach ($candidate in Get-ChildItem -LiteralPath $ReleasesDir -Directory -Force) {
    $candidatePath = [IO.Path]::GetFullPath($candidate.FullName).TrimEnd(
      [IO.Path]::DirectorySeparatorChar,
      [IO.Path]::AltDirectorySeparatorChar
    )
    if ([string]::Equals($candidatePath, $keepPath, [StringComparison]::OrdinalIgnoreCase)) {
      continue
    }
    if (Test-ReleaseUpdateInProgress -ReleasePath $candidatePath) {
      Write-Host "Release conservee car une mise a jour l'utilise: $($candidate.Name)" -ForegroundColor DarkGray
      continue
    }
    try {
      Remove-Item -LiteralPath $candidatePath -Recurse -Force -ErrorAction Stop
      $removed++
    }
    catch {
      Write-Warning "Ancienne release impossible a supprimer ($candidatePath): $($_.Exception.Message)"
    }
  }

  Write-Host "Nettoyage local : $removed ancienne(s) release(s) web/app supprimee(s)." -ForegroundColor Green
}

function Register-NodeTask {
  $userId = [Security.Principal.WindowsIdentity]::GetCurrent().Name
  # Le script peut etre lance depuis pwsh.exe (runtime Codex), dont PSHOME ne
  # contient pas powershell.exe. La tache persistante utilise Windows PowerShell.
  $powershell = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"
  if (-not (Test-Path -LiteralPath $powershell -PathType Leaf)) {
    throw "PowerShell Windows introuvable: $powershell"
  }
  $arguments = "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$StartNode`" -Port $Port"
  $action = New-ScheduledTaskAction -Execute $powershell -Argument $arguments
  $logonTrigger = New-ScheduledTaskTrigger -AtLogOn -User $userId
  $watchdogTrigger = New-ScheduledTaskTrigger `
    -Once `
    -At (Get-Date).AddMinutes(5) `
    -RepetitionInterval (New-TimeSpan -Minutes 5)
  $trigger = @($logonTrigger, $watchdogTrigger)
  $settings = New-ScheduledTaskSettingsSet `
    -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable -DontStopOnIdleEnd `
    -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) `
    -MultipleInstances IgnoreNew `
    -ExecutionTimeLimit ([TimeSpan]::Zero)
  $principal = New-ScheduledTaskPrincipal -UserId $userId -LogonType Interactive -RunLevel Limited
  Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger `
    -Settings $settings -Principal $principal `
    -Description "Noeud Codex Switch Terminal (release active via 'current')" -Force | Out-Null
}

function Stop-NodeProcesses {
  # Stop-ScheduledTask arrete parfois uniquement le PowerShell parent et laisse
  # cst-server.exe orphelin. Ne cibler que les executables installes sous le
  # NodeHome evite de toucher un autre serveur portable lance par l'utilisateur.
  $trustedRoot = [IO.Path]::GetFullPath($NodeHome).TrimEnd(
    [IO.Path]::DirectorySeparatorChar,
    [IO.Path]::AltDirectorySeparatorChar
  ) + [IO.Path]::DirectorySeparatorChar
  $stopped = 0
  foreach ($process in Get-CimInstance Win32_Process -Filter "Name = 'cst-server.exe'") {
    $executable = [string]$process.ExecutablePath
    if (-not $executable) { continue }
    $resolved = [IO.Path]::GetFullPath($executable)
    if (-not $resolved.StartsWith($trustedRoot, [StringComparison]::OrdinalIgnoreCase)) {
      continue
    }
    Stop-Process -Id ([int]$process.ProcessId) -Force -ErrorAction Stop
    $stopped++
  }
  if ($stopped -gt 0) {
    Write-Host "$stopped processus cst-server orphelin(s) arrete(s)." -ForegroundColor DarkGray
  }
}

function Test-NodeBack {
  param([string]$WantVersion, [string]$WantCommit)
  $deadline = (Get-Date).AddSeconds($VerifyTimeoutSec)
  while ($true) {
    $h = Get-Healthz
    if ($h -and $h.version -eq $WantVersion -and $h.commit -eq $WantCommit `
        -and $h.ready -eq $true -and $h.draining -eq $false) {
      # Sonde d'acceptation : compte bidon -> 400/500 sur un noeud sain, 503 si
      # draine, 401 si token invalide. On accepte tout sauf 503/401/echec.
      $code = 0
      try {
        Invoke-WebRequest -Uri "$base/api/terminals" -Method Post -Headers $authHeaders `
          -ContentType "application/json" -TimeoutSec 8 -UseBasicParsing `
          -Body '{"accountId":"__cst_update_probe__","repoUrl":"","cols":80,"rows":24}' | Out-Null
        $code = 200
      } catch {
        if ($_.Exception.Response) { $code = [int]$_.Exception.Response.StatusCode.value__ }
      }
      if ($code -ne 503 -and $code -ne 401 -and $code -ne 0) { return $true }
    }
    if ((Get-Date) -ge $deadline) { return $false }
    Start-Sleep -Seconds 2
  }
}

# --- 1. Peupler la release : mode build OU mode release (download + verif) ---
$distSrc = $null
$builtExe = $null
$releaseDir = $null
$releaseMarker = $null

if ($IsReleaseMode) {
  if (-not $MinisignPubKey) { $MinisignPubKey = $env:CST_MINISIGN_PUBKEY }
  $dl = Join-Path ([System.IO.Path]::GetTempPath()) ("cst-dl-" + [System.IO.Path]::GetRandomFileName())
  $stage = Join-Path ([System.IO.Path]::GetTempPath()) ("cst-stage-" + [System.IO.Path]::GetRandomFileName())
  New-Item -ItemType Directory -Force -Path $dl, $stage | Out-Null

  $baseUrl = "https://github.com/$Repo/releases/download/$ReleaseTag"
  $zipPath = Join-Path $dl $Asset
  Write-Host "Telechargement de $Asset depuis $Repo@$ReleaseTag" -ForegroundColor Cyan
  foreach ($suffix in @("", ".sha256", ".minisig")) {
    Invoke-WebRequest -Uri "$baseUrl/$Asset$suffix" -OutFile "$zipPath$suffix" -UseBasicParsing -TimeoutSec 180
  }

  # 1a. Empreinte SHA-256 (fail-closed). Le .sha256 contient "<hash>  <basename>".
  $expected = (((Get-Content "$zipPath.sha256" -TotalCount 1) -split '\s+')[0]).Trim().ToLower()
  $actual = (Get-FileHash -Algorithm SHA256 $zipPath).Hash.ToLower()
  if ($expected -ne $actual) { throw "SHA-256 invalide: attendu $expected, obtenu $actual." }
  Write-Host "SHA-256 verifiee." -ForegroundColor Green

  # 1b. Signature minisign (fail-closed sauf -AllowUnsigned explicite).
  if ($AllowUnsigned) {
    Write-Warning "Verification de signature IGNOREE (-AllowUnsigned)."
  } else {
    if (-not $MinisignPubKey -or $MinisignPubKey -like "RWQPLACEHOLDER*") {
      throw "Cle publique minisign non configuree (-MinisignPubKey / CST_MINISIGN_PUBKEY ; voir deploy/PHASE2-UPDATES.md)."
    }
    if (-not (Get-Command minisign -ErrorAction SilentlyContinue)) {
      throw "minisign introuvable sur le PATH (installe-le, ou -AllowUnsigned). Voir deploy/PHASE2-UPDATES.md."
    }
    & minisign -Vm $zipPath -x "$zipPath.minisig" -P $MinisignPubKey
    if ($LASTEXITCODE -ne 0) { throw "Signature minisign INVALIDE pour $Asset." }
    Write-Host "Signature minisign verifiee." -ForegroundColor Green
  }

  # 1c. Extraction (l'archive contient cst-server.exe + dist\).
  Expand-Archive -Path $zipPath -DestinationPath $stage -Force
  $builtExe = Join-Path $stage "cst-server.exe"
  $distSrc = Join-Path $stage "dist"
  if (-not (Test-Path $builtExe)) { throw "Archive invalide: cst-server.exe introuvable." }
  if (-not (Test-Path $distSrc)) { throw "Archive invalide: dist\ introuvable." }
} else {
  if (-not $SkipBuild) {
    Push-Location $Root
    try {
      $rev = & git -C $Root rev-parse --short HEAD 2>$null
      if ($LASTEXITCODE -eq 0 -and $rev) { $env:CST_GIT_COMMIT = ([string]$rev).Trim() }
      npm run build:frontend; if ($LASTEXITCODE -ne 0) { throw "Le build frontend a echoue." }
      npm run build:server;   if ($LASTEXITCODE -ne 0) { throw "Le build serveur a echoue." }
    } finally { Pop-Location }
  }
  $builtExe = if ($BuiltExePath) {
    [IO.Path]::GetFullPath($BuiltExePath)
  } else {
    Join-Path $Root "src-tauri\target\release\cst-server.exe"
  }
  if (-not (Test-Path $builtExe)) { throw "Binaire introuvable: $builtExe (retire -SkipBuild ?)." }
  $distSrc = if ($BuiltDistPath) {
    [IO.Path]::GetFullPath($BuiltDistPath)
  } else {
    Join-Path $Root "dist"
  }
  if (-not (Test-Path $distSrc -PathType Container)) {
    throw "Frontend introuvable: $distSrc (retire -SkipBuild ?)."
  }
}

# --- 2. Self-check version ---
$verLine = [string](& $builtExe --version)
if ($verLine -notmatch '^cst-server\s+(\S+)\s+\(([^)]+)\)$') {
  throw "Version/commit illisibles via 'cst-server --version': $verLine"
}
$version = $Matches[1]
$commit = $Matches[2]
if ($IsReleaseMode -and ($ReleaseTag.TrimStart('v') -ne $version)) {
  throw "Incoherence: tag $ReleaseTag mais binaire en version $version."
}
Write-Host "Nouvelle release : $verLine" -ForegroundColor Cyan

# --- 3. Installer la release ---
# La version Cargo reste souvent stable pendant le developpement. Le commit fait
# donc partie du dossier afin de ne jamais reecrire l'executable actuellement
# charge/verrouille par Windows. Une seconde publication du meme commit recoit
# un suffixe unique au lieu de modifier la release active.
$safeCommit = $commit -replace '[^A-Za-z0-9._-]', '-'
$releaseId = "$version-$safeCommit"
$releaseDir = Join-Path $ReleasesDir $releaseId
if (Test-Path $releaseDir) {
  $releaseId = "$releaseId-$([DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds())-$PID"
  $releaseDir = Join-Path $ReleasesDir $releaseId
}
New-Item -ItemType Directory -Force -Path $releaseDir | Out-Null
$releaseMarker = Join-Path $releaseDir ".update-in-progress"
$thisProcess = Get-Process -Id $PID
"$PID|$($thisProcess.StartTime.ToUniversalTime().Ticks)" |
  Set-Content -LiteralPath $releaseMarker -Encoding ASCII -NoNewline
try {
  Copy-Item $builtExe (Join-Path $releaseDir "cst-server.exe") -Force
  $releaseDist = Join-Path $releaseDir "dist"
  if (Test-Path $releaseDist) { Remove-Item $releaseDist -Recurse -Force }
  Copy-Item $distSrc $releaseDist -Recurse -Force
}
catch {
  Remove-Item -LiteralPath $releaseDir -Recurse -Force -ErrorAction SilentlyContinue
  $releaseMarker = $null
  throw
}

# Le mutex ne couvre ni le build ni l'attente d'inactivite. Il serialise
# seulement la bascule de quelques secondes entre deux agents deployeurs.
$updateMutex = [Threading.Mutex]::new($false, "Local\CST-Deploy-$Port")
$updateLockHeld = $false
$drainArmed = $false
$deadline = (Get-Date).AddSeconds($DrainTimeoutSec)
$prevTarget = $null
$previousHealth = $null

try {
  # --- 4. Attente NON BLOQUANTE puis acquisition de la courte fenetre ---
  while (-not $updateLockHeld) {
    $h = Get-Healthz
    $running = $null -ne $h
    $active = Get-ActiveWorkloadCount -Health $h
    $available = -not $h -or ($h.draining -eq $false -and $active -eq 0)

    if ($available -or ((Get-Date) -ge $deadline -and $Force)) {
      try {
        $updateLockHeld = $updateMutex.WaitOne(0)
      }
      catch [Threading.AbandonedMutexException] {
        # Le processus precedent est mort : Windows nous transfere le mutex.
        $updateLockHeld = $true
      }
      if (-not $updateLockHeld) {
        throw "Une autre mise a jour effectue deja la bascule de 8080 ; reessaie apres sa verification."
      }

      # Ferme la course entre la sonde precedente et la prise du mutex. Si un
      # terminal vient de demarrer, on rend immediatement le mutex et on attend
      # sans drainer le noeud.
      $h = Get-Healthz
      $running = $null -ne $h
      $active = Get-ActiveWorkloadCount -Health $h
      if ($h -and ($h.draining -eq $true -or ($active -gt 0 -and -not $Force))) {
        $updateMutex.ReleaseMutex()
        $updateLockHeld = $false
      }
      else {
        break
      }
    }

    if ((Get-Date) -ge $deadline) {
      throw "Timeout: $active session(s) actives. MAJ abandonnee sans avoir draine ni bloque le noeud."
    }
    Write-Host "Noeud occupe ($active session(s)) : attente sans drain..." -ForegroundColor DarkGray
    Start-Sleep -Seconds 1
  }

  # Capture la vraie release precedente sous le mutex, apres l'attente : un
  # autre agent a pu deployer pendant notre build ou pendant la phase idle.
  $previousHealth = Get-Healthz
  if (Test-Path $CurrentLink) {
    $item = Get-Item $CurrentLink -Force
    if ($item.Target) { $prevTarget = [string](@($item.Target)[0]) }
  }

  # --- 5. Lease de drain courte, uniquement pendant la bascule ---
  if ($running) {
    Write-Host "Fenetre de bascule : drain borne a ${DrainLeaseSec}s..." -ForegroundColor Yellow
    Set-DrainState -Draining $true
    $drainArmed = $true

    # Une creation deja acceptee juste avant le drain peut apparaitre ici. Sans
    # -Force, on annule aussitot la fenetre plutot que bloquer les autres agents.
    Start-Sleep -Milliseconds 250
    $h = Get-Healthz
    $active = Get-ActiveWorkloadCount -Health $h
    if ($active -gt 0 -and -not $Force) {
      Set-DrainState -Draining $false
      $drainArmed = $false
      throw "Course de bascule: $active session(s) viennent de demarrer. Noeud rouvert immediatement ; retente."
    }
  }

  # --- 6. Stop bref de la tache ---
  $taskExists = [bool](Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue)
  if ($taskExists) {
    Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    $stopDeadline = (Get-Date).AddSeconds($StopTimeoutSec)
    while ((Get-Healthz) -and (Get-Date) -lt $stopDeadline) {
      # Une tache configuree avec RestartCount peut laisser apparaitre un
      # second enfant juste apres le premier Stop-Process. Retenter pendant la
      # fenetre bornee ferme cette course sans toucher aux serveurs portables.
      Stop-NodeProcesses
      Start-Sleep -Milliseconds 100
    }
    if (Get-Healthz) {
      throw "Le serveur ne s'est pas arrete en ${StopTimeoutSec}s ; bascule annulee."
    }
  }

  # --- 7. Bascule de la jonction 'current' ---
  Write-Host "Bascule current -> releases\$releaseId" -ForegroundColor Cyan
  Set-Junction -Link $CurrentLink -Target $releaseDir

  # --- 8. (Re)creer + demarrer la tache ---
  Register-NodeTask
  Start-ScheduledTask -TaskName $TaskName
  # Le nouveau processus repart non draine (etat uniquement en memoire).
  $drainArmed = $false

  # --- 9. Verification exacte version + commit ---
  if (Test-NodeBack -WantVersion $version -WantCommit $commit) {
    Remove-Item -LiteralPath $releaseMarker -Force -ErrorAction SilentlyContinue
    $releaseMarker = $null
    Remove-ObsoleteLocalReleases -Keep $releaseDir
    Write-Host "OK : noeud en $version ($commit), pret et non draine." -ForegroundColor Green
    exit 0
  }

  # --- 10. Rollback ---
  Write-Warning "ECHEC de la verification en $version ($commit)."
  if ($prevTarget -and (Test-Path $prevTarget) -and ($prevTarget -ne $releaseDir)) {
    Write-Host "Rollback -> $prevTarget" -ForegroundColor Yellow
    Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    $stopDeadline = (Get-Date).AddSeconds($StopTimeoutSec)
    while ((Get-Healthz) -and (Get-Date) -lt $stopDeadline) {
      Stop-NodeProcesses
      Start-Sleep -Milliseconds 100
    }
    Set-Junction -Link $CurrentLink -Target $prevTarget
    Start-ScheduledTask -TaskName $TaskName
    $prevVersion = if ($previousHealth) { [string]$previousHealth.version } else { "" }
    $prevCommit = if ($previousHealth) { [string]$previousHealth.commit } else { "" }
    if ($prevVersion -and $prevCommit -and
        (Test-NodeBack -WantVersion $prevVersion -WantCommit $prevCommit)) {
      Write-Host "Rollback OK : noeud restaure en $prevVersion ($prevCommit)." -ForegroundColor Green
    } else {
      Write-Error "ALERTE : le rollback n'a pas restaure un etat sain, intervention manuelle requise."
    }
  } else {
    Write-Error "ALERTE : pas de release precedente valide pour le rollback."
  }
  exit 1
}
finally {
  if ($releaseMarker) {
    Remove-Item -LiteralPath $releaseMarker -Force -ErrorAction SilentlyContinue
    if ($releaseDir -and (Test-Path -LiteralPath $releaseDir -PathType Container)) {
      $activeRelease = $null
      if (Test-Path -LiteralPath $CurrentLink) {
        $currentItem = Get-Item -LiteralPath $CurrentLink -Force
        if ($currentItem.Target) {
          $activeRelease = [string](@($currentItem.Target)[0])
          if (-not [IO.Path]::IsPathRooted($activeRelease)) {
            $activeRelease = Join-Path (Split-Path -Parent $CurrentLink) $activeRelease
          }
          $activeRelease = [IO.Path]::GetFullPath($activeRelease)
        }
      }
      if (-not $activeRelease -or -not [string]::Equals(
        $activeRelease,
        [IO.Path]::GetFullPath($releaseDir),
        [StringComparison]::OrdinalIgnoreCase
      )) {
        Remove-Item -LiteralPath $releaseDir -Recurse -Force -ErrorAction SilentlyContinue
      }
    }
  }
  if ($drainArmed) {
    try {
      Set-DrainState -Draining $false
      Write-Warning "Mise a jour interrompue : noeud remis hors drain."
    }
    catch {
      Write-Warning "La lease de drain expirera seule sous ${DrainLeaseSec}s : $($_.Exception.Message)"
    }
  }
  if ($updateLockHeld) {
    $updateMutex.ReleaseMutex()
  }
  $updateMutex.Dispose()
}
