@echo off
setlocal
cd /d "%~dp0"
title [Bifrost] EFI-Speicherplatz Optimierer
echo ===================================================================
echo [Bifrost] Starte EFI-Finalisierer mit Administrator-Rechten...
echo ===================================================================

powershell.exe -NoProfile -ExecutionPolicy Bypass -Command "Start-Process powershell.exe -ArgumentList '-NoProfile -ExecutionPolicy Bypass -File \"%~dp0fix_esp.ps1\"' -Verb RunAs"
