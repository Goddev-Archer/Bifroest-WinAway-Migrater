# Requires -RunAsAdministrator
$ErrorActionPreference = "Stop"

Write-Host "==========================================================" -ForegroundColor Cyan
Write-Host "  Bifrost VM EFI Patcher (PowerShell Direct Engine)" -ForegroundColor Cyan
Write-Host "==========================================================" -ForegroundColor Cyan

# 0. Check elevation
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
    Write-Host "[FEHLER] Dieses Skript muss als Administrator ausgefuehrt werden!" -ForegroundColor Red
    Write-Host "Bitte mit Rechtsklick -> 'Als Administrator ausfuehren' starten." -ForegroundColor Yellow
    pause
    exit 1
}

$vhdxPath = "C:\ProgramData\Microsoft\Windows\Virtual Hard Disks\Memory-Killer.vhdx"
$repoRoot = "C:\dev\Project Movement"
$ukiSource = Join-Path $repoRoot "target\bifrost-boot.efi"

if (-not (Test-Path $vhdxPath)) {
    Write-Host "[ERROR] VHDX nicht gefunden: $vhdxPath" -ForegroundColor Red
    pause
    exit 1
}

if (-not (Test-Path $ukiSource)) {
    Write-Host "[ERROR] UKI nicht gefunden: $ukiSource" -ForegroundColor Red
    pause
    exit 1
}

Write-Host "[1/5] Pruefe Quell-UKI (target\bifrost-boot.efi)..." -ForegroundColor Yellow
$ukiBytes = [System.IO.File]::ReadAllBytes($ukiSource)
$ukiText = [System.Text.Encoding]::ASCII.GetString($ukiBytes)
if ($ukiText.Contains("keep_bootcon") -and (-not $ukiText.Contains("nomodeset"))) {
    Write-Host "      [OK] Quell-UKI verifiziert: 'keep_bootcon' ist AKTIV, 'nomodeset' ist ENTFERNT!" -ForegroundColor Green
} else {
    Write-Host "      [FEHLER] Quell-UKI enthaelt nicht die richtigen Kernel-Parameter!" -ForegroundColor Red
    pause
    exit 1
}

# 1. Check if S: is already mounted
$targetLetter = "S"
if (-not (Test-Path "S:\EFI")) {
    $existing = Get-DiskImage -ImagePath $vhdxPath -ErrorAction SilentlyContinue
    if ($existing -and $existing.Attached) {
        Dismount-DiskImage -ImagePath $vhdxPath -ErrorAction SilentlyContinue
        Start-Sleep -Seconds 2
    }

    Write-Host "[2/5] Binde virtuelle Festplatte ein..." -ForegroundColor Yellow
    Mount-DiskImage -ImagePath $vhdxPath -PassThru | Out-Null
    Start-Sleep -Seconds 3

    $disk = Get-DiskImage -ImagePath $vhdxPath | Get-Disk
    if (-not $disk) {
        Write-Host "[ERROR] Konnte gemountete Disk nicht finden!" -ForegroundColor Red
        Dismount-DiskImage -ImagePath $vhdxPath -ErrorAction SilentlyContinue
        pause
        exit 1
    }

    Write-Host "      Disk gefunden: Nummer $($disk.Number)" -ForegroundColor Green

    Write-Host "[3/5] Suche EFI-Systempartition..." -ForegroundColor Yellow
    $partitions = Get-Partition -DiskNumber $disk.Number
    $espPart = $partitions | Where-Object { 
        $_.GptType -eq "{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}" -or $_.Type -eq "System" -or ($_.Size -ge 80MB -and $_.Size -le 200MB)
    } | Select-Object -First 1

    if (-not $espPart) {
        Write-Host "[ERROR] Keine EFI-Systempartition gefunden!" -ForegroundColor Red
        Dismount-DiskImage -ImagePath $vhdxPath -ErrorAction SilentlyContinue
        pause
        exit 1
    }

    Set-Partition -DiskNumber $espPart.DiskNumber -PartitionNumber $espPart.PartitionNumber -NewDriveLetter $targetLetter
    Start-Sleep -Seconds 2
} else {
    Write-Host "[2/5] EFI-Partition bereits unter S: eingehaengt!" -ForegroundColor Green
}

Write-Host "[4/5] Kopiere UKI auf die EFI-Partition (Platzsparend fuer 96 MB ESP)..." -ForegroundColor Yellow

# Clean up redundant Bifrost copy to ensure we never run out of space on 96MB ESP
if (Test-Path "S:\EFI\Bifrost\bifrost-boot.efi") {
    Remove-Item -Path "S:\EFI\Bifrost\bifrost-boot.efi" -Force
}

$targetBoot = "S:\EFI\Boot"
$targetMs = "S:\EFI\Microsoft\Boot"

New-Item -ItemType Directory -Force -Path $targetBoot | Out-Null
New-Item -ItemType Directory -Force -Path $targetMs | Out-Null

# Remove old 0-byte or broken bootmgfw.efi
if (Test-Path "$targetMs\bootmgfw.efi") {
    $curSize = (Get-Item "$targetMs\bootmgfw.efi").Length
    if ($curSize -lt 10MB) {
        Remove-Item -Path "$targetMs\bootmgfw.efi" -Force
    }
}

# Copy to bootmgfw.efi (Windows Boot Manager replacement)
Copy-Item -Path $ukiSource -Destination "$targetMs\bootmgfw.efi" -Force
$copiedMs = (Get-Item "$targetMs\bootmgfw.efi").Length
Write-Host "      [OK] $targetMs\bootmgfw.efi geschrieben ($([math]::Round($copiedMs / 1MB, 2)) MB)" -ForegroundColor Green

# Copy to bootx64.efi (UEFI Fallback Bootloader)
Copy-Item -Path $ukiSource -Destination "$targetBoot\bootx64.efi" -Force
$copiedBoot = (Get-Item "$targetBoot\bootx64.efi").Length
Write-Host "      [OK] $targetBoot\bootx64.efi geschrieben ($([math]::Round($copiedBoot / 1MB, 2)) MB)" -ForegroundColor Green

$freeSpace = (Get-Volume -DriveLetter S).SizeRemaining / 1MB
Write-Host "      Freier Speicherplatz auf ESP: $([math]::Round($freeSpace, 2)) MB" -ForegroundColor Green

Write-Host "[5/5] Trenne Laufwerksbuchstabe und VHDX sauber..." -ForegroundColor Yellow
Get-Partition | Where-Object { $_.DriveLetter -eq 'S' } | Remove-PartitionAccessPath -AccessPath "S:\" -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1
Dismount-DiskImage -ImagePath $vhdxPath -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2

Write-Host ""
Write-Host "==========================================================" -ForegroundColor Green
Write-Host "  ERFOLG! Das neue UKI mit keep_bootcon wurde auf die VHDX geschrieben!" -ForegroundColor Green
Write-Host "  Du kannst die VM 'DerTestGerät' jetzt in Hyper-V starten!" -ForegroundColor Green
Write-Host "==========================================================" -ForegroundColor Green
Write-Host "Druecke eine beliebige Taste zum Beenden..."
$null = $Host.UI.RawUI.ReadKey("NoEcho,IncludeKeyDown")
