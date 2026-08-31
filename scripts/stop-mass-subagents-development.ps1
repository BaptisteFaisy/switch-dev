#requires -Version 7.0

[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [string]$DeploymentRoot,

  [ValidateSet(18084)]
  [int]$Port = 18084
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$deploymentRootPath = [IO.Path]::GetFullPath($DeploymentRoot)
if (-not (Test-Path -LiteralPath (Join-Path $deploymentRootPath "app") -PathType Container)) {
  throw "Racine Switch developpement invalide : $deploymentRootPath"
}
if (-not $env:LOCALAPPDATA) {
  throw "LOCALAPPDATA est indisponible."
}
$runtimeRoot = Join-Path $env:LOCALAPPDATA "SwitchDevelopmentMassSubagents"
$tokenPath = Join-Path $runtimeRoot "run\mass-subagents-admin.token"
$pidPath = Join-Path $runtimeRoot "run\mass-subagents.pid"
$listener = Get-NetTCPConnection -State Listen -LocalPort $Port -ErrorAction SilentlyContinue |
  Select-Object -First 1
if (-not $listener) {
  if (Test-Path -LiteralPath $pidPath -PathType Leaf) {
    Remove-Item -LiteralPath $pidPath -Force
  }
  Write-Output "mass-subagents Switch developpement est deja arrete."
  return
}
if (-not (Test-Path -LiteralPath $tokenPath -PathType Leaf)) {
  throw "Token local absent : refus d'arreter un processus non authentifie sur le port $Port."
}
$token = (Get-Content -Raw -LiteralPath $tokenPath).Trim()
if ($token -cnotmatch "^[a-f0-9]{64}$") {
  throw "Token local invalide : refus d'arreter le processus sur le port $Port."
}
$headers = @{ Authorization = "Bearer $token" }
$response = Invoke-RestMethod `
  -Method Post `
  -Uri "http://127.0.0.1:$Port/v1/admin/shutdown" `
  -Headers $headers `
  -TimeoutSec 5
if ($response.ok -ne $true) {
  throw "La demande d'arret gracieux mass-subagents a ete refusee."
}
$deadline = [DateTime]::UtcNow.AddSeconds(30)
do {
  Start-Sleep -Milliseconds 250
  $listener = Get-NetTCPConnection -State Listen -LocalPort $Port -ErrorAction SilentlyContinue |
    Select-Object -First 1
} while ($listener -and [DateTime]::UtcNow -lt $deadline)
if ($listener) {
  throw "mass-subagents n'a pas termine son arret gracieux dans le delai imparti."
}
if (Test-Path -LiteralPath $pidPath -PathType Leaf) {
  Remove-Item -LiteralPath $pidPath -Force
}
Write-Output "mass-subagents Switch developpement est arrete proprement."
