# Remet les dossiers modules Windows PowerShell en tete de PSModulePath
# (processus uniquement, idempotent). Certains sandbox (Freebuff/Codex)
# placent avant eux un stub Microsoft.PowerShell.Utility (`.psd1` sans
# `.psm1`) : la resolution de module prefere le premier dossier gagnant, et
# Get-FileHash (defini dans ce `.psm1` en PowerShell 5.1) devient introuvable.
# On retire les chemins systeme de leur position puis on les remet en tete ;
# sur une machine saine, l'ordre ne change pas.
$systemModulePaths = @(
  (Join-Path $env:ProgramFiles 'WindowsPowerShell\Modules'),
  (Join-Path $env:WINDIR 'System32\WindowsPowerShell\v1.0\Modules')
) | Where-Object { $_ }
$currentModulePaths = @($env:PSModulePath -split ';' | Where-Object { $_ })
$otherModulePaths = @($currentModulePaths | Where-Object { $systemModulePaths -notcontains $_ })
$env:PSModulePath = (($systemModulePaths + $otherModulePaths) -join ';')