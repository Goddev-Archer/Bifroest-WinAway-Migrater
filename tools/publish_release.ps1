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

# Prüfe bestehendes Release
try {
    $existing = Invoke-RestMethod -Uri "https://api.github.com/repos/$repo/releases/tags/v0.1.0-experimental" -Headers $headers -Method Get
    Write-Host "Bestehendes Release gefunden (ID: $($existing.id)). Lösche altes Release..." -ForegroundColor Yellow
    Invoke-RestMethod -Uri "https://api.github.com/repos/$repo/releases/$($existing.id)" -Headers $headers -Method Delete
} catch {
    Write-Host "Kein bestehendes Release vorhanden." -ForegroundColor Gray
}

# Release Payload
$releaseNotes = @"
# ⚠️ BIFRÖST v0.1.0 (Definitiv-Nicht-Final-Und-Experimentell)

> **Status:** Vorabversion / Experimenteller Test-Build für Virtuelle Maschinen (VMs).
> **Wichtig:** Noch nicht für Produktivsysteme geeignet!

### Enthaltene Features:
- 🚀 **100% Rust-basiertes Single-File Binary** (``bifroest.exe``, nur 1.78 MB)
- 🖥️ **BIOS/MemTest86-Ästhetik & TUI** mit geführten Phasen und CP437-Doppelrahmen
- 🛡️ **Zero-Data-Loss Architektur:** Zerstörungsfreies NTFS-Shrink-Planungsmodul
- 📦 **Blackbox-Packaging:** Zstandard-komprimiertes Archiv inkl. ``bifrost_manifest.json`` und SHA-256 Hash
- 🔄 **VM-Handoff:** Automatische Erzeugung von ``stage_efi_boot.bat`` und RAMFS-Runner
- 🧪 **Eingebauter Self-Test:** Startbar via ``bifroest.exe --self-test-vm``
"@

$payload = @{
    tag_name = "v0.1.0-experimental"
    target_commitish = "master"
    name = "v0.1.0-experimental (Definitiv-Nicht-Final-Und-Experimentell)"
    body = $releaseNotes
    draft = $false
    prerelease = $true
} | ConvertTo-Json -Depth 5

Write-Host "Erstelle neues Release auf GitHub..." -ForegroundColor Cyan
$newRelease = Invoke-RestMethod -Uri "https://api.github.com/repos/$repo/releases" -Headers $headers -Method Post -Body $payload -ContentType "application/json; charset=utf-8"
Write-Host "Release erfolgreich erstellt! ID: $($newRelease.id), URL: $($newRelease.html_url)" -ForegroundColor Green

# Asset bifroest.exe hochladen
$binaryPath = "target\release\bifroest.exe"
if (-not (Test-Path $binaryPath)) {
    throw "Binary nicht gefunden: $binaryPath"
}

$fileBytes = [System.IO.File]::ReadAllBytes((Resolve-Path $binaryPath))
$uploadUri = "https://uploads.github.com/repos/$repo/releases/$($newRelease.id)/assets?name=bifroest.exe"

$uploadHeaders = @{
    "Authorization" = "Bearer $token"
    "Accept" = "application/vnd.github+json"
    "User-Agent" = "Bifroest-Release-Agent"
    "Content-Type" = "application/octet-stream"
}

Write-Host "Lade $binaryPath ($($fileBytes.Length) Bytes) nach GitHub hoch..." -ForegroundColor Cyan
$asset = Invoke-RestMethod -Uri $uploadUri -Headers $uploadHeaders -Method Post -Body $fileBytes
Write-Host "Asset erfolgreich hochgeladen!" -ForegroundColor Green
Write-Host "Download-URL: $($asset.browser_download_url)" -ForegroundColor Yellow
