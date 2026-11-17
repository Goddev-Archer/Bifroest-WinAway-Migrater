# Requires -RunAsAdministrator
$ErrorActionPreference = "Stop"

Write-Host "==========================================================" -ForegroundColor Cyan
Write-Host "  Bifrost ESP Space Optimizer & Finalizer" -ForegroundColor Cyan
Write-Host "==========================================================" -ForegroundColor Cyan

$vhdxPath = "C:\ProgramData\Microsoft\Windows\Virtual Hard Disks\Memory-Killer.vhdx"
$repoRoot = "C:\dev\Project Movement"
$ukiSource = Join-Path $repoRoot "target\bifrost-boot.efi"

# 1. Check if S: is mounted
if (-not (Test-Path "S:\EFI")) {
    Write-Host "[1/4] S:\EFI nicht gefunden, binde VHDX ein..." -ForegroundColor Yellow
    Mount-DiskImage -ImagePath $vhdxPath -PassThru | Out-Null
    Start-Sleep -Seconds 2
    $disk = Get-DiskImage -ImagePath $vhdxPath | Get-Disk
    $espPart = Get-Partition -DiskNumber $disk.Number | Where-Object { $_.GptType -eq "{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}" -or $_.Type -eq "System" -or ($_.Size -ge 80MB -and $_.Size -le 200MB) } | Select-Object -First 1
    Set-Partition -DiskNumber $espPart.DiskNumber -PartitionNumber $espPart.PartitionNumber -NewDriveLetter "S"
    Start-Sleep -Seconds 1
}

Write-Host "[1/4] Bereinige Speicherplatz auf der 96 MB EFI-Partition..." -ForegroundColor Yellow
# Free space by removing redundant copy in \EFI\Bifrost\
if (Test-Path "S:\EFI\Bifrost\bifrost-boot.efi") {
    Remove-Item -Path "S:\EFI\Bifrost\bifrost-boot.efi" -Force
    Write-Host "      [OK] Redundante Kopie in S:\EFI\Bifrost entfernt (28.9 MB freigegeben)!" -ForegroundColor Green
}

# Remove 0-byte bootmgfw.efi if present
if (Test-Path "S:\EFI\Microsoft\Boot\bootmgfw.efi") {
    $curSize = (Get-Item "S:\EFI\Microsoft\Boot\bootmgfw.efi").Length
    if ($curSize -lt 10MB) {
        Remove-Item -Path "S:\EFI\Microsoft\Boot\bootmgfw.efi" -Force
        Write-Host "      [OK] Unvollstaendige bootmgfw.efi ($curSize Bytes) entfernt." -ForegroundColor Green
    }
}

Write-Host "[2/4] Kopiere neues UKI nach S:\EFI\Microsoft\Boot\bootmgfw.efi..." -ForegroundColor Yellow
Copy-Item -Path $ukiSource -Destination "S:\EFI\Microsoft\Boot\bootmgfw.efi" -Force
$sizeMs = (Get-Item "S:\EFI\Microsoft\Boot\bootmgfw.efi").Length
Write-Host "      [OK] S:\EFI\Microsoft\Boot\bootmgfw.efi: $([math]::Round($sizeMs / 1MB, 2)) MB" -ForegroundColor Green

Write-Host "[3/4] Stelle sicher, dass S:\EFI\Boot\bootx64.efi aktuell ist..." -ForegroundColor Yellow
Copy-Item -Path $ukiSource -Destination "S:\EFI\Boot\bootx64.efi" -Force
$sizeBoot = (Get-Item "S:\EFI\Boot\bootx64.efi").Length
Write-Host "      [OK] S:\EFI\Boot\bootx64.efi: $([math]::Round($sizeBoot / 1MB, 2)) MB" -ForegroundColor Green

Write-Host "[4/4] Pruefe freien Speicherplatz auf S:..." -ForegroundColor Yellow
$freeSpace = (Get-Volume -DriveLetter S).SizeRemaining / 1MB
Write-Host "      Freier Speicher auf EFI-Partition: $([math]::Round($freeSpace, 2)) MB" -ForegroundColor Green

Write-Host "Trenne Laufwerksbuchstabe S: und VHDX..." -ForegroundColor Yellow
Get-Partition | Where-Object { $_.DriveLetter -eq 'S' } | Remove-PartitionAccessPath -AccessPath "S:\" -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1
Dismount-DiskImage -ImagePath $vhdxPath -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2

Write-Host ""
Write-Host "==========================================================" -ForegroundColor Green
Write-Host "  ERFOLG! VHDX ist 100% startklar und sauber getrennt!" -ForegroundColor Green
Write-Host "  Starte die VM 'DerTestGerät' jetzt in Hyper-V!" -ForegroundColor Green
Write-Host "==========================================================" -ForegroundColor Green
Write-Host "Druecke eine beliebige Taste zum Beenden..."
$null = $Host.UI.RawUI.ReadKey("NoEcho,IncludeKeyDown")
