@echo off
rem ============================================================
rem  DSH Control Panel maintenance tool - launcher (pure ASCII)
rem  Prefers PowerShell 7 (pwsh, then 7-preview), falls back to
rem  Windows PowerShell 5.1.
rem ============================================================
setlocal
cd /d "%~dp0"

rem Check that the main script exists next to this launcher
if not exist "%~dp0maintain.ps1" (
    echo [ERROR] maintain.ps1 not found next to maintain.bat.
    echo         Please keep maintain.bat and maintain.ps1 in the same folder.
    exit /b 1
)

rem Detect PowerShell 7: PATH first, then install paths (7, 7-preview), then 5.1
set "PWSH="
where pwsh.exe >nul 2>&1 && set "PWSH=pwsh.exe"
if not defined PWSH if exist "%ProgramFiles%\PowerShell\7\pwsh.exe" set "PWSH=%ProgramFiles%\PowerShell\7\pwsh.exe"
if not defined PWSH if exist "%ProgramFiles%\PowerShell\7-preview\pwsh.exe" set "PWSH=%ProgramFiles%\PowerShell\7-preview\pwsh.exe"
if not defined PWSH set "PWSH=powershell.exe"

"%PWSH%" -NoProfile -ExecutionPolicy Bypass -Sta -WindowStyle Hidden -File "%~dp0maintain.ps1" %*
exit /b %ERRORLEVEL%
