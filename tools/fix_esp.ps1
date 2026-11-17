# Requires -RunAsAdministrator
$ErrorActionPreference = "Stop"

Write-Host "==========================================================" -ForegroundColor Cyan
Write-Host "  Bifrost ESP Space Optimizer & Finalizer" -ForegroundColor Cyan
Write-Host "==========================================================" -ForegroundColor Cyan

& "$PSScriptRoot\patch_vm.ps1"
