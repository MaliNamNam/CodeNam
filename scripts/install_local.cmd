@echo off
REM Double-click or run: scripts\install_local.cmd
REM Installs this repo as CodeNam so you can run: CodeNam
setlocal
cd /d "%~dp0\.."
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install_local.ps1" %*
exit /b %ERRORLEVEL%
