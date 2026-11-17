@echo off
setlocal
cd /d "%~dp0"
title [Bifrost] UKI-Update auf virtueller Festplatte
echo ===================================================================
echo [Bifrost] Starte UKI-Patcher mit Administrator-Rechten...
echo ===================================================================

powershell.exe -NoProfile -ExecutionPolicy Bypass -Command "Start-Process powershell.exe -ArgumentList '-NoProfile -ExecutionPolicy Bypass -File \"%~dp0patch_vm.ps1\"' -Verb RunAs"
