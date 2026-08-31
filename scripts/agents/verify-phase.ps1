#requires -Version 7.0

[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [ValidateSet("0", "A")]
  [string]$Phase,

  [ValidateRange(0, 9007199254740991)]
  [long]$Seed = 42,

  [ValidateRange(1, 10)]
  [int]$Scale = 10,

  [string]$ArtifactsRoot = "",

  [switch]$FoundationOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repositoryRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..\..")).Path
$verifier = Join-Path $repositoryRoot "sidecar\mass-subagents\scripts\verify-phase.mjs"
if ($env:SWITCH_ENV -ne "development") {
  throw "SWITCH_ENV=development est obligatoire."
}
if (-not (Test-Path -LiteralPath $verifier -PathType Leaf)) {
  throw "Verificateur mass-subagents absent : $verifier"
}

$arguments = @($verifier, "--phase", $Phase, "--seed", [string]$Seed, "--scale", [string]$Scale)
if (-not [string]::IsNullOrWhiteSpace($ArtifactsRoot)) {
  $arguments += @("--artifacts-root", $ArtifactsRoot)
}
if ($FoundationOnly) {
  $arguments += "--foundation-only"
}

& (Get-Command node -ErrorAction Stop).Source @arguments
exit $LASTEXITCODE
