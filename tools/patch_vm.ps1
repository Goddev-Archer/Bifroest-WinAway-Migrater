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

$ukiSize = (Get-Item $ukiSource).Length
Write-Host "[1/5] Pruefe Quell-UKI ($([math]::Round($ukiSize / 1MB, 2)) MB)..." -ForegroundColor Yellow
if ($ukiSize -lt 20MB) {
    Write-Host "[FEHLER] UKI ist unvollstaendig!" -ForegroundColor Red
    pause
    exit 1
}
Write-Host "      [OK] UKI ist startklar!" -ForegroundColor Green

# 1. Mount VHDX
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

Write-Host "[4/5] Bereinige alte Dateien & kopiere neues UKI..." -ForegroundColor Yellow

# Clean up redundant old files to prevent ESP overflow (96 MB limit)
if (Test-Path "S:\EFI\Bifrost\bifrost-boot.efi") {
    Remove-Item -Path "S:\EFI\Bifrost\bifrost-boot.efi" -Force
}
if (Test-Path "S:\EFI\Boot\bootx64.efi") {
    Remove-Item -Path "S:\EFI\Boot\bootx64.efi" -Force
}

$targetMs = "S:\EFI\Microsoft\Boot"
New-Item -ItemType Directory -Force -Path $targetMs | Out-Null

if (Test-Path "$targetMs\bootmgfw.efi") {
    Remove-Item -Path "$targetMs\bootmgfw.efi" -Force
}

# Copy primary bootloader (bootmgfw.efi)
Copy-Item -Path $ukiSource -Destination "$targetMs\bootmgfw.efi" -Force
$copiedMs = (Get-Item "$targetMs\bootmgfw.efi").Length
Write-Host "      [OK] $targetMs\bootmgfw.efi geschrieben ($([math]::Round($copiedMs / 1MB, 2)) MB)" -ForegroundColor Green

# Check remaining space; if enough, also write fallback bootx64.efi
$freeMB = (Get-Volume -DriveLetter S).SizeRemaining / 1MB
if ($freeMB -gt ($ukiSize / 1MB + 5)) {
    $targetBoot = "S:\EFI\Boot"
    New-Item -ItemType Directory -Force -Path $targetBoot | Out-Null
    Copy-Item -Path $ukiSource -Destination "$targetBoot\bootx64.efi" -Force
    Write-Host "      [OK] S:\EFI\Boot\bootx64.efi geschrieben ($([math]::Round((Get-Item "$targetBoot\bootx64.efi").Length / 1MB, 2)) MB)" -ForegroundColor Green
} else {
    Write-Host "      [INFO] Platzsparmodus aktiv: Primaerer Bootloader bootmgfw.efi ist scharf ($([math]::Round($freeMB, 1)) MB frei auf ESP)." -ForegroundColor Green
}

Write-Host "[5/5] Trenne Laufwerksbuchstabe und VHDX sauber..." -ForegroundColor Yellow
Get-Partition | Where-Object { $_.DriveLetter -eq 'S' } | Remove-PartitionAccessPath -AccessPath "S:\" -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1
Dismount-DiskImage -ImagePath $vhdxPath -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2

Write-Host ""
Write-Host "==========================================================" -ForegroundColor Green
Write-Host "  ERFOLG! Das neue UKI mit Live-Logging ist auf der VHDX!" -ForegroundColor Green
Write-Host "  Starte die VM 'DerTestGerät' jetzt in Hyper-V!" -ForegroundColor Green
Write-Host "==========================================================" -ForegroundColor Green
Write-Host "Druecke eine beliebige Taste zum Beenden..."
$null = $Host.UI.RawUI.ReadKey("NoEcho,IncludeKeyDown")
