param(
    [string]$InstallDirectory = "C:\Users\jeanp\codex-switch-terminal\scripts"
)

$ErrorActionPreference = "Stop"

$nodePath = (Get-Command node.exe -ErrorAction Stop).Source
$brokerPath = Join-Path $InstallDirectory "account-browser-broker.mjs"
if (-not (Test-Path -LiteralPath $brokerPath -PathType Leaf)) {
    throw "Lanceur navigateur Switch introuvable: $brokerPath"
}

$startupDirectory = [Environment]::GetFolderPath("Startup")
$shortcutPath = Join-Path $startupDirectory "Codex Switch Browser Broker.lnk"
$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut($shortcutPath)
$shortcut.TargetPath = $nodePath
$shortcut.Arguments = '"' + $brokerPath + '"'
$shortcut.WorkingDirectory = Split-Path -Parent $InstallDirectory
$shortcut.WindowStyle = 7
$shortcut.Description = "Ouvre les connexions Freebuff dans le proxy du compte Switch"
$shortcut.Save()

Get-CimInstance Win32_Process -Filter "Name = 'node.exe'" |
    Where-Object { $_.CommandLine -and $_.CommandLine.Contains($brokerPath) } |
    ForEach-Object { Invoke-CimMethod -InputObject $_ -MethodName Terminate | Out-Null }

Start-Process -FilePath $nodePath `
    -ArgumentList @($brokerPath) `
    -WorkingDirectory (Split-Path -Parent $InstallDirectory) `
    -WindowStyle Hidden
