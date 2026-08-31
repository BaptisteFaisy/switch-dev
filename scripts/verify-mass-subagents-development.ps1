#requires -Version 7.0

[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [string]$DeploymentRoot,

  [switch]$IncludeTenThousandSmoke
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$deploymentRootPath = [IO.Path]::GetFullPath($DeploymentRoot)
$sidecarRoot = Join-Path $deploymentRootPath "app\sidecar\mass-subagents"
if (-not (Test-Path -LiteralPath (Join-Path $sidecarRoot "package.json") -PathType Leaf)) {
  throw "Paquet mass-subagents absent : $sidecarRoot"
}
$nodeVersion = [version](& (Get-Command node -ErrorAction Stop).Source -p "process.versions.node")
if ($nodeVersion.Major -lt 22) {
  throw "Node.js 22 ou plus recent est obligatoire."
}
& npm --prefix $sidecarRoot run check
if ($LASTEXITCODE -ne 0) { throw "La verification syntaxique mass-subagents a echoue." }
& npm --prefix $sidecarRoot test
if ($LASTEXITCODE -ne 0) { throw "Les tests mass-subagents ont echoue." }
if ($IncludeTenThousandSmoke) {
  $previousSwitchEnvironment = $env:SWITCH_ENV
  $previousRealDispatch = $env:MASS_SUBAGENTS_REAL_ENABLED
  try {
    $env:SWITCH_ENV = "development"
    $env:MASS_SUBAGENTS_REAL_ENABLED = "false"
    & npm --prefix $sidecarRoot run smoke:10000 -- --repeats 1
    if ($LASTEXITCODE -ne 0) { throw "Le smoke test mass-subagents 10000 a echoue." }
  } finally {
    if ($null -eq $previousSwitchEnvironment) {
      Remove-Item Env:SWITCH_ENV -ErrorAction SilentlyContinue
    } else {
      $env:SWITCH_ENV = $previousSwitchEnvironment
    }
    if ($null -eq $previousRealDispatch) {
      Remove-Item Env:MASS_SUBAGENTS_REAL_ENABLED -ErrorAction SilentlyContinue
    } else {
      $env:MASS_SUBAGENTS_REAL_ENABLED = $previousRealDispatch
    }
  }
}
Write-Output "Verification mass-subagents Switch developpement reussie."
