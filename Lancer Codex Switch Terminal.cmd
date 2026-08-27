@echo off
setlocal EnableExtensions

rem Switch sur SSD/T7 ou pc-fixe = developpement uniquement.
rem La version stable/production est hebergee sur le VPS Microsoft Azure.
set "APP_DIR=%~dp0"
rem Toutes les copies du projet partagent les memes comptes et discussions.
set "CST_DATA_DIR=%APPDATA%\codex-switch-terminal-server"
set "ROOT_EXE=%APP_DIR%Codex Switch Terminal.exe"
set "CLOUD_EXE=%APP_DIR%Codex Switch Terminal Cloud.exe"
set "RELEASE_EXE=%APP_DIR%src-tauri\target\release\codex-switch-terminal.exe"
set "DEBUG_EXE=%APP_DIR%src-tauri\target\debug\codex-switch-terminal.exe"

cd /d "%APP_DIR%"
title Codex Switch Terminal

if "%~1"=="" goto :run_prepapp
call :dispatch "%~1"
exit /b %ERRORLEVEL%

:menu
cls
echo ========================================================
echo                CODEX SWITCH TERMINAL
echo ========================================================
echo.
echo   1. Lancer l'application locale
echo   2. Lancer l'application Cloud
echo   3. Ouvrir le site web local
echo   4. Demarrer le serveur SaaS local
echo   5. Arreter le serveur SaaS local
echo   6. Autoriser le serveur dans le pare-feu
echo   0. Quitter
echo.
choice /c 1234560 /n /m "Choisis une action [1-6, 0] : "

if errorlevel 7 goto :done
if errorlevel 6 goto :menu_firewall
if errorlevel 5 goto :menu_stop
if errorlevel 4 goto :menu_server
if errorlevel 3 goto :menu_web
if errorlevel 2 goto :menu_cloud
if errorlevel 1 goto :menu_app
goto :menu

:menu_app
call :run_app
if errorlevel 1 pause
goto :done

:menu_cloud
call :run_cloud
if errorlevel 1 pause
goto :done

:menu_web
call :run_web
call :wait_for_menu
goto :menu

:menu_server
call :run_server
call :wait_for_menu
goto :menu

:menu_stop
call :run_stop
call :wait_for_menu
goto :menu

:menu_firewall
call :run_firewall
call :wait_for_menu
goto :menu

:dispatch
set "ACTION=%~1"
if /i "%ACTION%"=="app" goto :run_app
if /i "%ACTION%"=="local" goto :run_app
if /i "%ACTION%"=="cloud" goto :run_cloud
if /i "%ACTION%"=="web" goto :run_web
if /i "%ACTION%"=="site" goto :run_web
if /i "%ACTION%"=="server" goto :run_server
if /i "%ACTION%"=="serveur" goto :run_server
if /i "%ACTION%"=="stop" goto :run_stop
if /i "%ACTION%"=="arreter" goto :run_stop
if /i "%ACTION%"=="firewall" goto :run_firewall
if /i "%ACTION%"=="pare-feu" goto :run_firewall
if /i "%ACTION%"=="help" goto :show_help
if /i "%ACTION%"=="--help" goto :show_help
if /i "%ACTION%"=="/?" goto :show_help

echo Action inconnue : %ACTION%
echo.
call :show_help
exit /b 1

:show_help
echo Utilisation :
echo   "%~nx0" [app^|cloud^|web^|server^|stop^|firewall]
echo.
echo Sans argument, le lanceur ouvre Switch PrepApp sur le PC.
exit /b 0

:run_prepapp
powershell -NoProfile -ExecutionPolicy Bypass -File "%USERPROFILE%\Documents\Switch-PrepApp\Start-Switch-PrepApp.ps1"
exit /b %ERRORLEVEL%

:run_app
if exist "%RELEASE_EXE%" (
  start "Codex Switch Terminal" /D "%APP_DIR%" "%RELEASE_EXE%"
  exit /b 0
)

if exist "%ROOT_EXE%" (
  start "Codex Switch Terminal" /D "%APP_DIR%" "%ROOT_EXE%"
  exit /b 0
)

if exist "%DEBUG_EXE%" (
  start "Codex Switch Terminal" /D "%APP_DIR%" "%DEBUG_EXE%"
  exit /b 0
)

echo Aucun executable compile trouve.
echo Lancement en mode developpement...
call npm.cmd run dev
exit /b %ERRORLEVEL%

:run_cloud
set "CST_CLIENT_REMOTE=1"
set "CST_CLIENT_BASE_URL=http://127.0.0.1:18080"

if exist "%RELEASE_EXE%" (
  start "Codex Switch Terminal Cloud" /D "%APP_DIR%" "%RELEASE_EXE%"
  exit /b 0
)

if exist "%ROOT_EXE%" (
  start "Codex Switch Terminal Cloud" /D "%APP_DIR%" "%ROOT_EXE%"
  exit /b 0
)

if exist "%CLOUD_EXE%" (
  start "Codex Switch Terminal Cloud" /D "%APP_DIR%" "%CLOUD_EXE%"
  exit /b 0
)

if exist "%DEBUG_EXE%" (
  start "Codex Switch Terminal Cloud" /D "%APP_DIR%" "%DEBUG_EXE%"
  exit /b 0
)

echo Aucun executable de l'application trouve.
exit /b 1

:run_web
powershell -NoProfile -ExecutionPolicy Bypass -File "%APP_DIR%scripts\open-web-interface.ps1"
exit /b %ERRORLEVEL%

:run_server
powershell -NoProfile -ExecutionPolicy Bypass -File "%APP_DIR%scripts\start-local-server.ps1"
exit /b %ERRORLEVEL%

:run_stop
powershell -NoProfile -ExecutionPolicy Bypass -File "%APP_DIR%scripts\stop-local-server.ps1"
exit /b %ERRORLEVEL%

:run_firewall
powershell -NoProfile -ExecutionPolicy Bypass -Command "Start-Process PowerShell -Verb RunAs -ArgumentList '-NoProfile -ExecutionPolicy Bypass -File ""%APP_DIR%scripts\allow-local-server-firewall.ps1""'"
if errorlevel 1 exit /b %ERRORLEVEL%
echo La demande administrateur a ete ouverte.
exit /b 0

:wait_for_menu
echo.
echo Appuie sur une touche pour revenir au menu...
pause >nul
exit /b 0

:done
exit /b 0
