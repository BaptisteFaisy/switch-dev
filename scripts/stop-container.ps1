[CmdletBinding()]
param(
  # Racine du depot (par defaut : parent du dossier scripts/).
  [string]$RepoRoot = ""
)

# Arret du conteneur local lance par start-container.ps1. `stop` conserve le
# conteneur (et son volume .cst-data) ; `docker compose down` le supprimerait.

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
if (-not $RepoRoot.Trim()) { $RepoRoot = Split-Path -Parent $ScriptDir }

Push-Location $RepoRoot
try {
  docker compose stop
  if ($LASTEXITCODE -ne 0) {
    throw "docker compose stop a echoue (code $LASTEXITCODE)."
  }
}
finally {
  Pop-Location
}

Write-Host "Conteneur arrete. Pour le relancer : npm run container:start"
