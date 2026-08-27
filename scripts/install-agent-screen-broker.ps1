param(
    [string]$InstallDirectory = "C:\Users\jeanp\codex-switch-terminal\scripts",
    [string]$KeyDirectory = "C:\Users\jeanp\Documents\Switch-PrepApp\data\screen-bridge-keys",
    [string]$BrowserKeyDirectory = "C:\Users\jeanp\Documents\Switch-PrepApp\data\browser-bridge-keys",
    [string]$AuthorizedKeysPath = (Join-Path $env:ProgramData "ssh\administrators_authorized_keys"),
    [ValidateRange(1, 65535)]
    [int]$WindowsSshPort = 2222
)

$ErrorActionPreference = "Stop"

if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator
)) {
    throw "Relance ce script dans un terminal PowerShell eleve : sshd lit administrators_authorized_keys pour ce compte administrateur."
}

function Invoke-SshKeygen {
    param([Parameter(Mandatory = $true)][string]$KeyFile)

    # Windows PowerShell 5.1 supprime l'argument vide de `-N ""` lors d'un
    # appel natif direct. cmd.exe conserve les deux guillemets attendus par
    # ssh-keygen, comme dans l'installeur navigateur Switch.
    $quotedFile = '"' + $KeyFile.Replace('"', '\"') + '"'
    cmd /d /c "ssh-keygen -q -t ed25519 -N \"\" -C switch-agent-screen-restricted -f $quotedFile"
    if ($LASTEXITCODE -ne 0) {
        throw "La cle SSH ecran n'a pas pu etre creee."
    }
}

function New-AdministratorsAuthorizedKeysAcl {
    $administrators = [Security.Principal.SecurityIdentifier]::new("S-1-5-32-544")
    $system = [Security.Principal.SecurityIdentifier]::new("S-1-5-18")
    $acl = [Security.AccessControl.FileSecurity]::new()
    $acl.SetOwner($administrators)
    $acl.SetAccessRuleProtection($true, $false)
    foreach ($identity in @($administrators, $system)) {
        $rule = [Security.AccessControl.FileSystemAccessRule]::new(
            $identity,
            [Security.AccessControl.FileSystemRights]::FullControl,
            [Security.AccessControl.AccessControlType]::Allow
        )
        [void]$acl.AddAccessRule($rule)
    }
    return $acl
}

$nodePath = (Get-Command node.exe -ErrorAction Stop).Source
$sshPath = (Get-Command ssh.exe -ErrorAction Stop).Source
$sshKeygen = (Get-Command ssh-keygen.exe -ErrorAction Stop).Source
$sshKeyscan = (Get-Command ssh-keyscan.exe -ErrorAction Stop).Source
$brokerPath = Join-Path $InstallDirectory "agent-screen-broker.mjs"
$clientPath = Join-Path $InstallDirectory "agent-screen-client.mjs"
$corePath = Join-Path $InstallDirectory "agent-screen-core.mjs"
$forcedCommandPath = Join-Path $InstallDirectory "agent-screen-ssh-command.cmd"
foreach ($path in @($brokerPath, $clientPath, $corePath, $forcedCommandPath)) {
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Fichier du controle ecran Switch introuvable: $path"
    }
}

# Une cle distincte du pont Windows general ET du navigateur agent : chaque
# capacite exposee aux agents doit avoir sa propre cle a commande forcee.
New-Item -ItemType Directory -Force -Path $KeyDirectory | Out-Null
$privateKeyPath = Join-Path $KeyDirectory "windows_screen_ed25519"
$publicKeyPath = "$privateKeyPath.pub"
if (-not (Test-Path -LiteralPath $privateKeyPath -PathType Leaf)) {
    Invoke-SshKeygen -KeyFile $privateKeyPath
}
if (-not (Test-Path -LiteralPath $publicKeyPath -PathType Leaf)) {
    throw "Cle publique ecran introuvable: $publicKeyPath"
}

$publicKey = (Get-Content -LiteralPath $publicKeyPath -Raw).Trim()
if ($publicKey -notmatch '^ssh-ed25519\s+[A-Za-z0-9+/=]+(?:\s+.*)?$') {
    throw "Cle publique ecran invalide."
}
$keyParts = $publicKey -split '\s+'
$escapedForcedCommand = $forcedCommandPath.Replace('\', '\\')
$restrictedLine = 'restrict,command="' + $escapedForcedCommand + '" ' + $keyParts[0] + ' ' + $keyParts[1] + ' switch-agent-screen-restricted'

$authorizedKeysDirectory = Split-Path -Parent $AuthorizedKeysPath
New-Item -ItemType Directory -Force -Path $authorizedKeysDirectory | Out-Null
$existingAcl = if (Test-Path -LiteralPath $AuthorizedKeysPath -PathType Leaf) {
    Get-Acl -LiteralPath $AuthorizedKeysPath
} else {
    New-AdministratorsAuthorizedKeysAcl
}
$existingLines = if (Test-Path -LiteralPath $AuthorizedKeysPath -PathType Leaf) {
    @(Get-Content -LiteralPath $AuthorizedKeysPath | Where-Object { $_ -notmatch 'switch-agent-screen-restricted\s*$' })
} else {
    @()
}
$updatedLines = @($existingLines) + @($restrictedLine)
$temporaryPath = "$AuthorizedKeysPath.switch-screen.$PID.tmp"
[IO.File]::WriteAllText($temporaryPath, (($updatedLines -join "`n") + "`n"), [Text.UTF8Encoding]::new($false))
Set-Acl -LiteralPath $temporaryPath -AclObject $existingAcl
Move-Item -LiteralPath $temporaryPath -Destination $AuthorizedKeysPath -Force
Set-Acl -LiteralPath $AuthorizedKeysPath -AclObject $existingAcl

# Le conteneur ne monte qu'UN seul windows_known_hosts (partage avec le
# navigateur agent). Si celui du navigateur existe deja, on le complete ; sinon
# on cree le notre et c'est lui qu'il faudra monter.
$knownHostsPath = Join-Path $KeyDirectory "windows_known_hosts"
$sharedKnownHosts = Join-Path $BrowserKeyDirectory "windows_known_hosts"
if (Test-Path -LiteralPath $sharedKnownHosts -PathType Leaf) {
    $knownHostsPath = $sharedKnownHosts
}

$scanOutput = @(& $sshKeyscan -T 5 -p $WindowsSshPort 127.0.0.1 2>$null)
$scannedKeys = @()
foreach ($line in $scanOutput) {
    if ($line -match '^\S+\s+(ssh-ed25519|ecdsa-sha2-nistp256|rsa-sha2-512|rsa-sha2-256|ssh-rsa)\s+([A-Za-z0-9+/=]+)(?:\s+.*)?$') {
        $scannedKeys += [pscustomobject]@{ Type = $Matches[1]; Key = $Matches[2] }
    }
}
$scannedKeys = @($scannedKeys | Sort-Object Type, Key -Unique)
if ($scannedKeys.Count -eq 0) {
    throw "Impossible de lire la cle hote du sshd Windows sur 127.0.0.1:$WindowsSshPort."
}
$preservedKnownHosts = if (Test-Path -LiteralPath $knownHostsPath -PathType Leaf) {
    @(Get-Content -LiteralPath $knownHostsPath | Where-Object {
        $_ -notmatch '^(windows-pc|\[windows-pc\]:\d+)\s+'
    })
} else {
    @()
}
$pinnedLines = @()
foreach ($key in $scannedKeys) {
    $pinnedLines += "windows-pc $($key.Type) $($key.Key)"
    $pinnedLines += "[windows-pc]:$WindowsSshPort $($key.Type) $($key.Key)"
}
$knownHostsTemporary = "$knownHostsPath.switch-screen.$PID.tmp"
[IO.File]::WriteAllText(
    $knownHostsTemporary,
    ((@($preservedKnownHosts) + @($pinnedLines) -join "`n") + "`n"),
    [Text.UTF8Encoding]::new($false)
)
Move-Item -LiteralPath $knownHostsTemporary -Destination $knownHostsPath -Force

$startupDirectory = [Environment]::GetFolderPath("Startup")
$shortcutPath = Join-Path $startupDirectory "Codex Switch Agent Screen Broker.lnk"
$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut($shortcutPath)
$shortcut.TargetPath = $nodePath
$brokerLogPath = Join-Path $KeyDirectory "agent-screen-broker.log"
$brokerArguments = '"' + $brokerPath + '" --log-file "' + $brokerLogPath + '"'
$shortcut.Arguments = $brokerArguments
$shortcut.WorkingDirectory = (Split-Path -Parent $InstallDirectory)
$shortcut.WindowStyle = 7
$shortcut.Description = "Controle l'ecran du poste Windows pour les agents Switch"
$shortcut.Save()

Get-CimInstance Win32_Process -Filter "Name = 'node.exe'" |
    Where-Object { $_.CommandLine -and $_.CommandLine.Contains($brokerPath) } |
    ForEach-Object { Invoke-CimMethod -InputObject $_ -MethodName Terminate | Out-Null }

Start-Process -FilePath $nodePath `
    -ArgumentList $brokerArguments `
    -WorkingDirectory (Split-Path -Parent $InstallDirectory) `
    -WindowStyle Hidden

$pipeReady = $false
$deadline = (Get-Date).AddSeconds(12)
do {
    $pipe = [IO.Pipes.NamedPipeClientStream]::new(
        ".",
        "CodexSwitchAgentScreen",
        [IO.Pipes.PipeDirection]::InOut,
        [IO.Pipes.PipeOptions]::None
    )
    try {
        $pipe.Connect(500)
        $pipeReady = $pipe.IsConnected
    } catch {
        Start-Sleep -Milliseconds 250
    } finally {
        $pipe.Dispose()
    }
} while (-not $pipeReady -and (Get-Date) -lt $deadline)
if (-not $pipeReady) {
    $logTail = if (Test-Path -LiteralPath $brokerLogPath -PathType Leaf) {
        (Get-Content -LiteralPath $brokerLogPath -Tail 20) -join "`n"
    } else {
        "aucun journal produit"
    }
    throw "Le broker ecran n'a pas ouvert son pipe. Journal: $logTail"
}

$probePayload = '{"kind":"agent-screen","sessionId":"switch-installer-probe-1234","action":"health"}'
$sshArguments = @(
    "-T",
    "-o", "BatchMode=yes",
    "-o", "IdentitiesOnly=yes",
    "-o", "ClearAllForwardings=yes",
    "-o", "ForwardAgent=no",
    "-o", "StrictHostKeyChecking=yes",
    "-o", "HostKeyAlias=windows-pc",
    "-o", "UserKnownHostsFile=$knownHostsPath",
    "-o", "ConnectTimeout=5",
    "-i", $privateKeyPath,
    "-p", $WindowsSshPort.ToString(),
    "$env:USERNAME@127.0.0.1",
    "switch-agent-screen"
)
$probeOutput = @($probePayload | & $sshPath @sshArguments 2>$null)
$probeExitCode = $LASTEXITCODE
$probe = try {
    (($probeOutput -join "`n") | ConvertFrom-Json)
} catch {
    $null
}
if ($probeExitCode -ne 0 -or -not $probe -or $probe.ok -ne $true -or $probe.ready -ne $true) {
    throw "Le smoke test SSH force -> client -> pipe ecran a echoue."
}

Write-Output "Broker ecran Switch installe."
Write-Output "Cle privee a monter dans le conteneur: $privateKeyPath"
Write-Output "Destination conteneur: /srv/cst/ssh-keys/windows_screen_ed25519"
Write-Output "Known hosts a monter dans le conteneur: $knownHostsPath"
Write-Output "Destination conteneur: /srv/cst/ssh-keys/windows_known_hosts"
Write-Output "Smoke SSH et pipe: OK"
