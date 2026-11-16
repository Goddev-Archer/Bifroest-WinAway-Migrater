param(
    [switch]$LaunchQemu,
    [string]$DiskImage = "bifrost_test_disk.qcow2",
    [string]$Initramfs = "initramfs-bifrost.cpio.gz",
    [string]$Kernel = "vmlinuz-bifrost"
)

$ErrorActionPreference = "Stop"

Write-Host "=== [1/2] Starte Bifröst End-to-End VM Self-Test ===" -ForegroundColor Cyan
cargo run -- --self-test-vm
if ($LASTEXITCODE -ne 0) {
    throw "Bifröst VM Self-Test fehlgeschlagen (Exit-Code $LASTEXITCODE)."
}

Write-Host "`n=== [2/2] Prüfe VM / Cross-Compile Infrastruktur ===" -ForegroundColor Cyan
$qemu = Get-Command "qemu-system-x86_64" -ErrorAction SilentlyContinue
$docker = Get-Command "docker" -ErrorAction SilentlyContinue
$wsl = Get-Command "wsl" -ErrorAction SilentlyContinue

if ($qemu) {
    Write-Host "[OK] QEMU gefunden: $($qemu.Source)" -ForegroundColor Green
    $qemuImg = Get-Command "qemu-img" -ErrorAction SilentlyContinue
    if (-not (Test-Path $DiskImage) -and $qemuImg) {
        Write-Host "Erstelle 60 GiB VirtIO Test-Disk ($DiskImage)..." -ForegroundColor Yellow
        & $qemuImg.Source create -f qcow2 $DiskImage 60G | Out-Null
    }

    if ($LaunchQemu -and (Test-Path $Kernel) -and (Test-Path $Initramfs)) {
        Write-Host "Starte QEMU VM im UEFI/VirtIO Test-Modus..." -ForegroundColor Green
        & $qemu.Source `
            -m 4096 `
            -smp 2 `
            -drive "file=$DiskImage,if=virtio,format=qcow2" `
            -kernel $Kernel `
            -initrd $Initramfs `
            -append "console=ttyS0 TERM=linux" `
            -nographic
    } else {
        Write-Host "Hinweis: Starte QEMU manuell mit Kernel '$Kernel' & Initramfs '$Initramfs':" -ForegroundColor Gray
        Write-Host "  qemu-system-x86_64 -m 4096 -smp 2 -drive file=$DiskImage,if=virtio,format=qcow2 -kernel $Kernel -initrd $Initramfs"
    }
} else {
    Write-Host "[INFO] qemu-system-x86_64 nicht im PATH gefunden." -ForegroundColor Yellow
    if ($wsl) {
        Write-Host "[OK] WSL2 ist verfügbar - Linux musl-Build & Initramfs-Erstellung via WSL:" -ForegroundColor Green
        Write-Host "  wsl -d archlinux -- bash -c 'cargo build --release --target x86_64-unknown-linux-musl && sh tools/make_initramfs.sh'"
    }
    if ($docker) {
        Write-Host "[OK] Docker ist verfügbar - Statischer musl-Container via Dockerfile.musl:" -ForegroundColor Green
        Write-Host "  docker build -f Dockerfile.musl -t bifroest-ramfs ."
    }
    Write-Host "Führe unter Windows das TUI direkt aus und drücke im letzten Schritt [B] für die Live-VM-Bridge Simulation:" -ForegroundColor Cyan
    Write-Host "  cargo run"
}
