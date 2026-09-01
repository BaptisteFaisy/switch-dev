[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [ValidateScript({ Test-Path -LiteralPath $_ -PathType Leaf })]
  [string]$ServerPath,

  [ValidateRange(2, 3600)]
  [int]$DurationSeconds = 30,

  [ValidateRange(250, 60000)]
  [int]$SampleIntervalMilliseconds = 1000,

  [ValidateRange(250, 60000)]
  [int]$ProcessTreeRefreshMilliseconds = 4000,

  [ValidateNotNullOrEmpty()]
  [string]$Label = "idle-root-isolated-$([DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ'))",

  [string]$StaticDir = ""
)

# Lance le binaire fourni dans un environnement local jetable, sans compte,
# client HTTP, terminal ni tour Codex, puis reutilise le sampler commun. Aucun
# serveur deja actif et aucun terminal utilisateur ne sont modifies.

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# PSModulePath : remet les modules Windows PowerShell en tete (Get-FileHash introuvable sinon dans le sandbox).
& (Join-Path $PSScriptRoot 'switch-development-ensure-system-module-path.ps1')

$projectRoot = Split-Path -Parent $PSScriptRoot
$resolvedServerPath = (Resolve-Path -LiteralPath $ServerPath).Path
$StaticDir = if ([string]::IsNullOrWhiteSpace($StaticDir)) {
  Join-Path $projectRoot "dist"
} else {
  $StaticDir
}
if (-not (Test-Path -LiteralPath $StaticDir -PathType Container)) {
  throw "Le dossier statique est introuvable: $StaticDir"
}
$resolvedStaticDir = (Resolve-Path -LiteralPath $StaticDir).Path
$tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$tempDir = Join-Path $tempRoot ("cst-idle-baseline-" + [Guid]::NewGuid().ToString("N"))
$stdoutPath = Join-Path $tempDir "server.stdout.log"
$stderrPath = Join-Path $tempDir "server.stderr.log"
$server = $null
$environmentNames = @(
  "CST_ACCOUNTS_DIR",
  "CST_ADMIN_TOKEN",
  "CST_BIND",
  "CST_DATA_DIR",
  "CST_GIT_PAT",
  "CST_NODE_CAPACITY",
  "CST_NODE_ID",
  "CST_NODE_LABEL",
  "CST_PUBLIC_BASE_URL",
  "CST_STATIC_DIR",
  "CST_WORKSPACES_ROOT"
)
$previousEnvironment = @{}
foreach ($name in $environmentNames) {
  $previousEnvironment[$name] = [Environment]::GetEnvironmentVariable(
    $name,
    [EnvironmentVariableTarget]::Process
  )
}

function Set-ProcessEnvironment {
  param(
    [Parameter(Mandatory = $true)]
    [string]$Name,

    [AllowNull()]
    [string]$Value
  )

  [Environment]::SetEnvironmentVariable(
    $Name,
    $Value,
    [EnvironmentVariableTarget]::Process
  )
}

function Get-FreeLoopbackPort {
  $listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
  try {
    $listener.Start()
    return [int]$listener.LocalEndpoint.Port
  }
  finally {
    $listener.Stop()
  }
}

function Wait-ServerReady {
  param(
    [Parameter(Mandatory = $true)]
    [int]$ProcessId,

    [Parameter(Mandatory = $true)]
    [string]$OutputPath,

    [Parameter(Mandatory = $true)]
    [string]$ErrorPath
  )

  $deadline = [DateTime]::UtcNow.AddSeconds(15)
  while ([DateTime]::UtcNow -lt $deadline) {
    $liveProcess = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
    if (-not $liveProcess) {
      $stderr = Get-Content -Raw -LiteralPath $ErrorPath -ErrorAction SilentlyContinue
      throw "Le serveur isole a quitte avant la mesure: $stderr"
    }
    if (Test-Path -LiteralPath $OutputPath) {
      $stdout = [string](Get-Content -Raw -LiteralPath $OutputPath)
      if ($stdout -like "*listening on*") {
        return
      }
    }
    Start-Sleep -Milliseconds 100
  }

  throw "Le serveur isole n'a pas annonce son ecoute dans les 15 secondes."
}

[void](New-Item -ItemType Directory -Path $tempDir)
try {
  $port = Get-FreeLoopbackPort
  $adminToken = [Guid]::NewGuid().ToString("N")
  Set-ProcessEnvironment -Name "CST_ACCOUNTS_DIR" -Value $null
  Set-ProcessEnvironment -Name "CST_ADMIN_TOKEN" -Value $adminToken
  Set-ProcessEnvironment -Name "CST_BIND" -Value "127.0.0.1:$port"
  Set-ProcessEnvironment -Name "CST_DATA_DIR" -Value $tempDir
  Set-ProcessEnvironment -Name "CST_GIT_PAT" -Value $null
  Set-ProcessEnvironment -Name "CST_NODE_CAPACITY" -Value "1"
  Set-ProcessEnvironment -Name "CST_NODE_ID" -Value "idle-baseline"
  Set-ProcessEnvironment -Name "CST_NODE_LABEL" -Value "Idle baseline"
  Set-ProcessEnvironment -Name "CST_PUBLIC_BASE_URL" -Value "http://127.0.0.1:$port"
  Set-ProcessEnvironment -Name "CST_STATIC_DIR" -Value $resolvedStaticDir
  Set-ProcessEnvironment -Name "CST_WORKSPACES_ROOT" -Value $projectRoot

  $server = Start-Process `
    -FilePath $resolvedServerPath `
    -PassThru `
    -WindowStyle Hidden `
    -RedirectStandardOutput $stdoutPath `
    -RedirectStandardError $stderrPath
  if (-not $server) {
    throw "Le lancement du serveur isole n'a retourne aucun processus."
  }
  Wait-ServerReady `
    -ProcessId $server.Id `
    -OutputPath $stdoutPath `
    -ErrorPath $stderrPath

  $establishedConnections = @(
    Get-NetTCPConnection `
      -OwningProcess $server.Id `
      -State Established `
      -ErrorAction SilentlyContinue
  ).Count
  if ($establishedConnections -ne 0) {
    throw "Le scenario isole a deja $establishedConnections connexion(s) cliente(s)."
  }

  $captureText = & "powershell.exe" @(
    "-NoProfile",
    "-ExecutionPolicy",
    "Bypass",
    "-File",
    (Join-Path $PSScriptRoot "measure-server-resources.ps1"),
    "-RootProcessId",
    $server.Id,
    "-DurationSeconds",
    $DurationSeconds,
    "-SampleIntervalMilliseconds",
    $SampleIntervalMilliseconds,
    "-ProcessTreeRefreshMilliseconds",
    $ProcessTreeRefreshMilliseconds,
    "-Label",
    $Label
  ) | Out-String
  if ($LASTEXITCODE -ne 0) {
    throw "La mesure idle a echoue (code $LASTEXITCODE)."
  }
  $capture = $captureText | ConvertFrom-Json

  # La liveness est verifiee apres la fenetre pour que cette requete ne pollue
  # pas la CPU mesuree et qu'aucune connexion cliente ne reste ouverte au debut.
  $health = Invoke-RestMethod `
    -Uri "http://127.0.0.1:$port/healthz" `
    -Method Get `
    -TimeoutSec 5
  if ($health.ok -ne $true -or $health.ready -ne $true) {
    throw "Le serveur isole ne repond pas comme un noeud pret apres la mesure."
  }

  $capture | Add-Member -NotePropertyName "idleScenario" -NotePropertyValue ([ordered]@{
    isolated = $true
    establishedClientConnectionsAtStart = $establishedConnections
    serverSha256 = (Get-FileHash -LiteralPath $resolvedServerPath -Algorithm SHA256).Hash
    health = [ordered]@{
      ok = [bool]$health.ok
      ready = [bool]$health.ready
      activeTerminals = [int]$health.activeTerminals
      version = [string]$health.version
      commit = [string]$health.commit
    }
  })
  $capture | ConvertTo-Json -Depth 12
}
finally {
  if ($server) {
    $server.Refresh()
    if (-not $server.HasExited) {
      Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue
      [void]$server.WaitForExit(5000)
    }
  }

  foreach ($name in $environmentNames) {
    Set-ProcessEnvironment -Name $name -Value $previousEnvironment[$name]
  }

  $resolvedTempDir = [IO.Path]::GetFullPath($tempDir)
  if (
    $resolvedTempDir.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -and
    (Split-Path -Leaf $resolvedTempDir).StartsWith("cst-idle-baseline-")
  ) {
    Remove-Item -LiteralPath $resolvedTempDir -Recurse -Force -ErrorAction SilentlyContinue
  }
}
