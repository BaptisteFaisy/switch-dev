$ErrorActionPreference = "Stop"

$isAdmin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
  [Security.Principal.WindowsBuiltInRole]::Administrator
)

if (-not $isAdmin) {
  throw "Lance ce script dans PowerShell en administrateur."
}

$ruleName = "Codex Switch Terminal SaaS Local 18080"
$existing = Get-NetFirewallRule -DisplayName $ruleName -ErrorAction SilentlyContinue

if (-not $existing) {
  New-NetFirewallRule `
    -DisplayName $ruleName `
    -Direction Inbound `
    -Action Allow `
    -Protocol TCP `
    -LocalPort 18080 `
    -Profile Private | Out-Null
}

Write-Host "Regle pare-feu active pour le port TCP 18080." -ForegroundColor Green
