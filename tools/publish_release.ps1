$ErrorActionPreference = "Stop"

$creds = echo "protocol=https`nhost=github.com" | git credential fill
$tokenLine = ($creds | Select-String "password=")
if (-not $tokenLine) {
    throw "GitHub Token konnte nicht aus git credential helper geladen werden."
}
$token = $tokenLine.Line.Substring(9).Trim()

$headers = @{
    "Authorization" = "Bearer $token"
    "Accept" = "application/vnd.github+json"
    "User-Agent" = "Bifroest-Release-Agent"
}

$repo = "Goddev-Archer/Bifroest-WinAway-Migrater"
$tag = "v0.1.0-experimental"

$binaryPath = "target\release\bifroest.exe"
if (-not (Test-Path $binaryPath)) {
    throw "Binary nicht gefunden: $binaryPath"
}
$binarySizeMB = [math]::Round(((Get-Item $binaryPath).Length / 1MB), 2)

$releaseNotes = @"
# ⚠️ BIFRÖST v0.1.0 (Definitiv-Nicht-Final-Und-Experimentell)

> **Status:** Vorabversion / Experimenteller Test-Build für Virtuelle Maschinen (VMs).
> **Wichtig:** Noch nicht für Produktivsysteme geeignet!

### Enthaltene Features & Assets:
- 🚀 **100% Rust-basiertes Single-File Binary** (``bifroest.exe``, $binarySizeMB MB, statisches CRT – keine VCRUNTIME140.dll nötig)
- 🧬 **Unified Kernel Image (UKI)** (``bifrost-boot.efi`` – Alpine Linux LTS Kernel + musl RAMFS + Cmdline ``bifrost_auto=1``)
- ⚡ **Zero-BIOS Handoff:** Automatisches ESP-Staging (``S:\EFI\Bifrost\bifrost-boot.efi``) & ``bcdedit`` Bootsequence + ``[R]`` Direkt-Reboot
- 🖥️ **BIOS/MemTest86-Ästhetik & TUI** mit geführten Phasen und CP437-Doppelrahmen
- 🛡️ **Zero-Data-Loss Architektur:** Zerstörungsfreies NTFS-Shrink-Planungsmodul (``ntfsresize`` + ``parted``) & Btrfs Subvolumes
- 📦 **Blackbox-Packaging:** Zstandard-komprimiertes Archiv inkl. ``bifrost_manifest.json`` und SHA-256 Hash
- 🧪 **Eingebauter Self-Test:** Startbar via ``bifroest.exe --self-test-vm``
"@

$payloadObj = @{
    tag_name = $tag
    target_commitish = "master"
    name = "v0.1.0-experimental (Definitiv-Nicht-Final-Und-Experimentell)"
    body = $releaseNotes
    draft = $false
    prerelease = $true
}
$payload = $payloadObj | ConvertTo-Json -Depth 5

$release = $null
try {
    $existing = Invoke-RestMethod -Uri "https://api.github.com/repos/$repo/releases/tags/$tag" -Headers $headers -Method Get
    Write-Host "Bestehendes Release gefunden (ID: $($existing.id)). Aktualisiere Metadaten (Assets bleiben erhalten)..." -ForegroundColor Yellow
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$repo/releases/$($existing.id)" -Headers $headers -Method Patch -Body $payload -ContentType "application/json; charset=utf-8"

    foreach ($asset in $release.assets) {
        if ($asset.name -eq "bifroest.exe") {
            Write-Host "Entferne altes Asset 'bifroest.exe' (ID: $($asset.id))..." -ForegroundColor Gray
            Invoke-RestMethod -Uri "https://api.github.com/repos/$repo/releases/assets/$($asset.id)" -Headers $headers -Method Delete
        }
    }
} catch {
    Write-Host "Erstelle neues Release '$tag' auf GitHub..." -ForegroundColor Cyan
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$repo/releases" -Headers $headers -Method Post -Body $payload -ContentType "application/json; charset=utf-8"
}

Write-Host "Release bereit! ID: $($release.id), URL: $($release.html_url)" -ForegroundColor Green

$uploadHeaders = @{
    "Authorization" = "Bearer $token"
    "Accept" = "application/vnd.github+json"
    "User-Agent" = "Bifroest-Release-Agent"
    "Content-Type" = "application/octet-stream"
}

$fileBytes = [System.IO.File]::ReadAllBytes((Resolve-Path $binaryPath))
$uploadUri = "https://uploads.github.com/repos/$repo/releases/$($release.id)/assets?name=bifroest.exe"
Write-Host "Lade $binaryPath ($($fileBytes.Length) Bytes) nach GitHub hoch..." -ForegroundColor Cyan
$uploadedExe = Invoke-RestMethod -Uri $uploadUri -Headers $uploadHeaders -Method Post -Body $fileBytes
Write-Host "Asset 'bifroest.exe' erfolgreich hochgeladen: $($uploadedExe.browser_download_url)" -ForegroundColor Green

# Prüfe alle Assets im Release (inkl. bifrost-boot.efi vom UKI Workflow)
$updatedRelease = Invoke-RestMethod -Uri "https://api.github.com/repos/$repo/releases/$($release.id)" -Headers $headers -Method Get
Write-Host "`nAktuelle Release Assets für ${tag}:" -ForegroundColor Cyan
foreach ($a in $updatedRelease.assets) {
    $szMB = [math]::Round(($a.size / 1MB), 2)
    Write-Host " - $($a.name) ($szMB MB) -> $($a.browser_download_url)" -ForegroundColor White
}
$hasUki = ($updatedRelease.assets | Where-Object { $_.name -eq "bifrost-boot.efi" }).Count -gt 0
if ($hasUki) {
    Write-Host "`n[OK] UKI 'bifrost-boot.efi' ist im Release vorhanden!" -ForegroundColor Green
} else {
    Write-Host "`n[INFO] 'bifrost-boot.efi' noch nicht im Release (wird durch GitHub Actions Workflow build-uki.yml hochgeladen)." -ForegroundColor Yellow
}
