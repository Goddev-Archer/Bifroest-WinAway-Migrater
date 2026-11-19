use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

#[cfg(target_os = "windows")]
use std::process::Command;

use crate::blackbox::PackResult;

pub const UKI_FILENAME: &str = "bifrost-boot.efi";
pub const UKI_RELEASE_URL: &str =
    "https://github.com/Goddev-Archer/Bifroest-WinAway-Migrater/releases/download/v0.1.0-experimental/bifrost-boot.efi";
pub const ROOTFS_FILENAME: &str = "bifrost_rootfs.tar.zst";
pub const ARCH_BOOTSTRAP_URLS: &[&str] = &[
    "https://geo.mirror.pkgbuild.com/iso/latest/archlinux-bootstrap-x86_64.tar.zst",
    "http://geo.mirror.pkgbuild.com/iso/latest/archlinux-bootstrap-x86_64.tar.zst",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DistroRootfsSpec {
    pub distro_label: String,
    pub download_urls: Vec<&'static str>,
    pub use_embedded_asgard: bool,
    pub min_expected_bytes: u64,
}

/// Resolves the rootfs staging strategy for the user's chosen distribution while preserving freedom of choice.
pub fn resolve_distro_rootfs_spec(target_distro: &str) -> DistroRootfsSpec {
    let lc = target_distro.to_lowercase();
    if lc.contains("arch") {
        DistroRootfsSpec {
            distro_label: target_distro.to_string(),
            download_urls: ARCH_BOOTSTRAP_URLS.to_vec(),
            use_embedded_asgard: false,
            min_expected_bytes: 10 * 1024 * 1024, // Arch bootstrap is ~120 MB
        }
    } else if lc.contains("alpine") || lc.contains("asgard") {
        DistroRootfsSpec {
            distro_label: target_distro.to_string(),
            download_urls: Vec::new(),
            use_embedded_asgard: true,
            min_expected_bytes: 0,
        }
    } else {
        // Fedora / Debian / NixOS / Custom: never force Arch; honor custom local bifrost_rootfs.tar.zst or fall back cleanly.
        DistroRootfsSpec {
            distro_label: target_distro.to_string(),
            download_urls: Vec::new(),
            use_embedded_asgard: true,
            min_expected_bytes: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffResult {
    pub stage_dir: PathBuf,
    pub manifest_file: PathBuf,
    pub checksum_file: PathBuf,
    pub efi_batch_script: PathBuf,
    pub ramfs_runner_script: PathBuf,
    pub bcd_commands: Vec<String>,
    pub esp_drive: String,
    pub uki_staged_path: Option<PathBuf>,
    pub rootfs_staged_path: Option<PathBuf>,
    pub rootfs_source: String,
    pub bcd_guid: Option<String>,
    pub bcd_armed: bool,
    pub secure_boot_enabled: bool,
    pub status_log: Vec<String>,
}

/// Parses a BCD `{xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}` UUID from `bcdedit` stdout (locale-agnostic).
pub fn parse_bcd_guid(output: &str) -> Option<String> {
    let bytes = output.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] == b'{' && i + 38 <= bytes.len() && bytes[i + 37] == b'}' {
            let candidate = &output[i + 1..i + 37];
            if is_valid_uuid_inner(candidate) {
                return Some(format!("{{{candidate}}}"));
            }
        }
    }
    None
}

fn is_valid_uuid_inner(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (idx, &b) in bytes.iter().enumerate() {
        if idx == 8 || idx == 13 || idx == 18 || idx == 23 {
            if b != b'-' {
                return false;
            }
        } else if !b.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

/// Checks Windows registry to determine whether UEFI Secure Boot is currently enabled.
pub fn detect_secure_boot() -> bool {
    #[cfg(target_os = "windows")]
    {
        if let Ok(out) = Command::new("reg")
            .args([
                "query",
                r"HKLM\SYSTEM\CurrentControlSet\Control\SecureBoot\State",
                "/v",
                "UEFISecureBootEnabled",
            ])
            .output()
        {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout);
                return text.contains("0x1");
            }
        }
    }
    false
}

/// Selects a free drive letter (preferring `S:` or an already-mounted ESP on `S:`) for `mountvol`.
pub fn find_free_esp_drive() -> String {
    if Path::new("S:\\EFI").exists() || !Path::new("S:\\").exists() {
        return "S:".to_string();
    }
    for letter in ["T:", "U:", "V:", "W:", "R:", "Q:", "P:", "Z:"] {
        let root = format!("{letter}\\");
        if Path::new(&format!("{root}EFI")).exists() || !Path::new(&root).exists() {
            return letter.to_string();
        }
    }
    "S:".to_string()
}

fn is_sandbox_profile(user_profile: &Path) -> bool {
    if cfg!(test) {
        return true;
    }
    let tmp = std::env::temp_dir()
        .to_string_lossy()
        .to_ascii_lowercase()
        .replace('/', "\\");
    let prof = user_profile
        .to_string_lossy()
        .to_ascii_lowercase()
        .replace('/', "\\");
    let tmp_trimmed = tmp.trim_end_matches('\\');
    prof.starts_with(tmp_trimmed) || prof.contains("bifrost_vm_selftest")
}

/// Resolves `bifrost-boot.efi` from local search paths or downloads it into `stage_dir`.
pub fn ensure_uki_binary(stage_dir: &Path) -> io::Result<PathBuf> {
    let dest = stage_dir.join(UKI_FILENAME);
    let mut local_candidates = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        local_candidates.push(cwd.join(UKI_FILENAME));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            local_candidates.push(parent.join(UKI_FILENAME));
        }
    }
    local_candidates.push(dest.clone());

    for cand in local_candidates {
        if let Ok(meta) = fs::metadata(&cand) {
            if meta.is_file() && meta.len() > 0 {
                if cand != dest {
                    fs::copy(&cand, &dest)?;
                }
                return Ok(dest);
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        if is_sandbox_profile(stage_dir) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Sandbox-Modus: UKI Download übersprungen.",
            ));
        }

        let curl_status = Command::new("curl.exe")
            .args([
                "-fL",
                "--retry",
                "2",
                "--connect-timeout",
                "15",
                "-o",
                &dest.to_string_lossy(),
                UKI_RELEASE_URL,
            ])
            .status();

        let curl_ok = curl_status.map(|s| s.success()).unwrap_or(false)
            && fs::metadata(&dest).map(|m| m.len() > 0).unwrap_or(false);

        if !curl_ok {
            let _ = fs::remove_file(&dest);
            let ps_dest = dest.to_string_lossy().replace('\'', "''");
            let ps_cmd = format!(
                "[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12; Invoke-WebRequest -Uri '{UKI_RELEASE_URL}' -OutFile '{ps_dest}' -UseBasicParsing"
            );
            let ps_out = Command::new("powershell")
                .args(["-NoProfile", "-Command", &ps_cmd])
                .output()?;
            if !ps_out.status.success() {
                let _ = fs::remove_file(&dest);
                let stderr = String::from_utf8_lossy(&ps_out.stderr);
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!("UKI Download fehlgeschlagen ({UKI_RELEASE_URL}): {stderr}"),
                ));
            }
        }

        let size = fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
        if size == 0 {
            let _ = fs::remove_file(&dest);
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Heruntergeladene bifrost-boot.efi ist 0 Bytes groß.",
            ));
        }
        return Ok(dest);
    }

    #[cfg(not(target_os = "windows"))]
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("{UKI_FILENAME} nicht lokal vorhanden"),
    ))
}

/// Resolves or downloads the target distribution's rootfs archive into `stage_dir/bifrost_rootfs.tar.zst`.
/// Honors user-supplied custom rootfs archives first, downloads official bootstrap images when chosen (e.g. Arch Linux),
/// and cleanly falls back to the embedded Asgard base rootfs without ever forcing an unselected distro.
pub fn ensure_distro_rootfs<F>(
    user_profile: &Path,
    stage_dir: &Path,
    target_distro: &str,
    log: &mut Vec<String>,
    mut status_cb: F,
) -> (Option<PathBuf>, String)
where
    F: FnMut(&str),
{
    let spec = resolve_distro_rootfs_spec(target_distro);
    let dest = stage_dir.join(ROOTFS_FILENAME);
    let marker = stage_dir.join("bifrost_rootfs.source");
    let expected_marker = format!("auto:{}", spec.distro_label);

    // Clear stale auto-downloaded rootfs if the user switched to a different target distro
    if let Ok(prev_marker) = fs::read_to_string(&marker) {
        if prev_marker.trim().starts_with("auto:") && prev_marker.trim() != expected_marker {
            let _ = fs::remove_file(&dest);
            let _ = fs::remove_file(&marker);
            log.push(format!(
                "Altes automatisches Rootfs ({}) entfernt, um neue Distro-Wahl ({target_distro}) zu respektieren.",
                prev_marker.trim()
            ));
        }
    }

    // 1. Always honor an explicitly user-provided local bifrost_rootfs.tar.zst (maximum freedom of choice)
    let mut local_candidates = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        local_candidates.push(cwd.join(ROOTFS_FILENAME));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            local_candidates.push(parent.join(ROOTFS_FILENAME));
        }
    }
    local_candidates.push(user_profile.join(ROOTFS_FILENAME));
    if !marker.exists() {
        local_candidates.push(dest.clone());
    }

    for cand in local_candidates {
        if let Ok(meta) = fs::metadata(&cand) {
            if meta.is_file() && meta.len() > 0 {
                if cand != dest {
                    status_cb(&format!("Kopiere lokales Rootfs ({})...", cand.display()));
                    if let Err(e) = fs::copy(&cand, &dest) {
                        log.push(format!("[WARN] Kopieren des lokalen Rootfs fehlgeschlagen: {e}"));
                        continue;
                    }
                }
                let _ = fs::remove_file(&marker);
                let sz_mb = meta.len() as f64 / (1024.0 * 1024.0);
                let desc = format!("Benutzerdefiniertes lokales Rootfs ({sz_mb:.1} MB)");
                log.push(format!("Rootfs bereit: {} [{desc}]", dest.display()));
                return (Some(dest), desc);
            }
        }
    }

    // 2. If distro uses embedded Asgard rootfs (e.g. Alpine Minimal or distro without direct bootstrap URL)
    if spec.use_embedded_asgard || spec.download_urls.is_empty() {
        let desc = format!(
            "Eingebettetes Asgard-Basis-Rootfs (apk + Auto-DHCP, fuer {target_distro})"
        );
        log.push(format!("Rootfs-Modus: {desc}"));
        return (None, desc);
    }

    // 3. Reuse already auto-downloaded rootfs for the same distro if valid
    if let Ok(meta) = fs::metadata(&dest) {
        if meta.is_file() && meta.len() >= spec.min_expected_bytes {
            let sz_mb = meta.len() as f64 / (1024.0 * 1024.0);
            let desc = format!("Offizielles {target_distro} Bootstrap-Rootfs (gecached, {sz_mb:.1} MB)");
            log.push(format!("Rootfs wiederverwendet: {} [{desc}]", dest.display()));
            return (Some(dest), desc);
        }
    }

    // 4. Download official distro bootstrap image on non-sandbox Windows hosts
    if is_sandbox_profile(user_profile) {
        let desc = format!("Sandbox-Modus: {target_distro} Rootfs-Download uebersprungen");
        log.push(desc.clone());
        return (None, desc);
    }

    #[cfg(target_os = "windows")]
    {
        for url in &spec.download_urls {
            status_cb(&format!("Lade {target_distro} Bootstrap-Rootfs ({url})..."));
            let _ = fs::remove_file(&dest);

            let curl_status = Command::new("curl.exe")
                .args([
                    "-fL",
                    "--retry",
                    "2",
                    "--connect-timeout",
                    "20",
                    "-o",
                    &dest.to_string_lossy(),
                    url,
                ])
                .status();

            let mut ok = curl_status.map(|s| s.success()).unwrap_or(false)
                && fs::metadata(&dest)
                    .map(|m| m.len() >= spec.min_expected_bytes)
                    .unwrap_or(false);

            if !ok {
                let _ = fs::remove_file(&dest);
                let ps_dest = dest.to_string_lossy().replace('\'', "''");
                let ps_cmd = format!(
                    "[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12; Invoke-WebRequest -Uri '{url}' -OutFile '{ps_dest}' -UseBasicParsing"
                );
                if let Ok(ps_out) = Command::new("powershell")
                    .args(["-NoProfile", "-Command", &ps_cmd])
                    .output()
                {
                    ok = ps_out.status.success()
                        && fs::metadata(&dest)
                            .map(|m| m.len() >= spec.min_expected_bytes)
                            .unwrap_or(false);
                }
            }

            if ok {
                let sz_mb = fs::metadata(&dest).map(|m| m.len()).unwrap_or(0) as f64 / (1024.0 * 1024.0);
                let _ = fs::write(&marker, format!("{expected_marker}\n"));
                let desc = format!("Offizielles {target_distro} Bootstrap-Rootfs ({sz_mb:.1} MB, pacman bereit)");
                log.push(format!("Rootfs heruntergeladen: {} [{desc}]", dest.display()));
                return (Some(dest), desc);
            }
            let _ = fs::remove_file(&dest);
        }

        let desc = format!(
            "Offline-Fallback: Eingebettetes Asgard-Basis-Rootfs ({target_distro}-Download nicht erreichbar)"
        );
        log.push(format!("[WARN] {desc}"));
        (None, desc)
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = &mut status_cb;
        let desc = format!("Eingebettetes Asgard-Basis-Rootfs ({target_distro})");
        log.push(desc.clone());
        (None, desc)
    }
}

/// Builds active Windows commands to mount ESP, stage `bifrost-boot.efi`, and deploy to the UEFI boot path.
pub fn generate_bcdedit_commands(esp_drive: &str) -> Vec<String> {
    let clean_drive = esp_drive.trim_end_matches(['\\', '/']);
    vec![
        "setlocal EnableDelayedExpansion".to_string(),
        format!("if not exist {clean_drive}\\EFI mountvol {clean_drive} /S"),
        format!("if not exist {clean_drive}\\EFI\\Bifrost mkdir {clean_drive}\\EFI\\Bifrost"),
        format!("if not exist {clean_drive}\\EFI\\Boot mkdir {clean_drive}\\EFI\\Boot"),
        format!("copy /Y \"%~dp0{UKI_FILENAME}\" {clean_drive}\\EFI\\Bifrost\\{UKI_FILENAME} || exit /b 1"),
        format!("if not exist {clean_drive}\\EFI\\Bifrost\\{UKI_FILENAME} exit /b 1"),
        // Backup original bootmgfw.efi if not already backed up
        format!("if exist {clean_drive}\\EFI\\Microsoft\\Boot\\bootmgfw.efi if not exist {clean_drive}\\EFI\\Microsoft\\Boot\\bootmgfw.efi.winbak copy /Y {clean_drive}\\EFI\\Microsoft\\Boot\\bootmgfw.efi {clean_drive}\\EFI\\Microsoft\\Boot\\bootmgfw.efi.winbak"),
        // Backup original bootx64.efi if present
        format!("if exist {clean_drive}\\EFI\\Boot\\bootx64.efi if not exist {clean_drive}\\EFI\\Boot\\bootx64.efi.winbak copy /Y {clean_drive}\\EFI\\Boot\\bootx64.efi {clean_drive}\\EFI\\Boot\\bootx64.efi.winbak"),
        // Deploy UKI to primary bootmgfw.efi (UEFI Firmware executes this directly, bypassing Windows Boot Manager checks)
        format!("copy /Y \"%~dp0{UKI_FILENAME}\" {clean_drive}\\EFI\\Microsoft\\Boot\\bootmgfw.efi"),
        // Deploy UKI to fallback bootx64.efi
        format!("copy /Y \"%~dp0{UKI_FILENAME}\" {clean_drive}\\EFI\\Boot\\bootx64.efi"),
        // Clear any stale one-time bootsequence so bootmgfw doesn't reject custom EFI images with 0xc000007b
        "bcdedit /deletevalue {bootmgr} bootsequence 2>nul".to_string(),
        "bcdedit /deletevalue {fwbootmgr} bootsequence 2>nul".to_string(),
        // Also register an explicit BCD / fwbootmgr entry as secondary safety
        "set \"BIFROST_GUID=\"".to_string(),
        "for /f \"tokens=2 delims={}\" %%g in ('bcdedit /copy {bootmgr} /d \"Bifrost Linux Bridge (RAMFS)\" ^| findstr \"{\"') do set \"BIFROST_GUID={%%g}\"".to_string(),
        format!("if defined BIFROST_GUID bcdedit /set !BIFROST_GUID! device partition={clean_drive}"),
        format!("if defined BIFROST_GUID bcdedit /set !BIFROST_GUID! path \\EFI\\Bifrost\\{UKI_FILENAME}"),
        "if defined BIFROST_GUID bcdedit /set {fwbootmgr} displayorder !BIFROST_GUID! /addfirst 2>nul".to_string(),
    ]
}

#[cfg(target_os = "windows")]
fn run_bcdedit(args: &[&str]) -> io::Result<String> {
    let out = Command::new("bcdedit").args(args).output()?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if !out.status.success() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("bcdedit {:?} fehlgeschlagen: {} {}", args, stdout.trim(), stderr.trim()),
        ));
    }
    Ok(stdout)
}

#[cfg(target_os = "windows")]
fn arm_windows_efi_handoff(
    stage_dir: &Path,
    esp_drive: &str,
    efi_batch_script: &Path,
    log: &mut Vec<String>,
) -> (Option<PathBuf>, Option<String>, bool) {
    let uki_local = match ensure_uki_binary(stage_dir) {
        Ok(p) => {
            let sz = fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            log.push(format!("UKI bereit: {} ({:.2} MB)", p.display(), sz as f64 / (1024.0 * 1024.0)));
            p
        }
        Err(e) => {
            log.push(format!("[WARN] UKI nicht verfügbar: {e}"));
            return (None, None, false);
        }
    };

    let clean_drive = esp_drive.trim_end_matches(['\\', '/']);
    let esp_root = format!("{clean_drive}\\");
    let esp_efi_dir = PathBuf::from(&esp_root).join("EFI");

    let mut needs_elevation = false;
    if !esp_efi_dir.exists() {
        match Command::new("mountvol").args([clean_drive, "/S"]).output() {
            Ok(out) if out.status.success() => {
                log.push(format!("ESP nach {clean_drive} gemountet."));
            }
            Ok(out) => {
                let err = String::from_utf8_lossy(&out.stderr);
                let out_s = String::from_utf8_lossy(&out.stdout);
                log.push(format!("mountvol {clean_drive} /S benötigt Admin ({} {})", out_s.trim(), err.trim()));
                needs_elevation = true;
            }
            Err(e) => {
                log.push(format!("mountvol Aufruf fehlgeschlagen: {e}"));
                needs_elevation = true;
            }
        }
    } else {
        log.push(format!("ESP bereits unter {clean_drive} verfügbar."));
    }

    let esp_target = PathBuf::from(&esp_root)
        .join("EFI")
        .join("Bifrost")
        .join(UKI_FILENAME);

    if !needs_elevation {
        if let Some(parent) = esp_target.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                log.push(format!("mkdir ESP fehlgeschlagen: {e}"));
                needs_elevation = true;
            }
        }
    }

    if !needs_elevation {
        match fs::copy(&uki_local, &esp_target) {
            Ok(bytes) if bytes > 0 => {
                let exists_ok = fs::metadata(&esp_target).map(|m| m.len() > 0).unwrap_or(false);
                if !exists_ok {
                    log.push("[ERROR] Verifikation von bifrost-boot.efi auf ESP fehlgeschlagen.".to_string());
                    return (Some(uki_local), None, false);
                }
                log.push(format!("UKI auf ESP kopiert & verifiziert: {} ({bytes} Bytes)", esp_target.display()));
            }
            Ok(_) => {
                log.push("[ERROR] Kopierte bifrost-boot.efi auf ESP ist 0 Bytes.".to_string());
                return (Some(uki_local), None, false);
            }
            Err(e) => {
                log.push(format!("Kopie auf ESP fehlgeschlagen: {e}"));
                needs_elevation = true;
            }
        }
    }

    if !needs_elevation {
        // Backup original bootmgfw.efi if present
        let ms_boot_dir = PathBuf::from(&esp_root).join("EFI").join("Microsoft").join("Boot");
        let orig_bootmgfw = ms_boot_dir.join("bootmgfw.efi");
        let backup_bootmgfw = ms_boot_dir.join("bootmgfw.efi.winbak");
        if orig_bootmgfw.exists() && !backup_bootmgfw.exists() {
            if let Err(e) = fs::copy(&orig_bootmgfw, &backup_bootmgfw) {
                log.push(format!("[WARN] Backup von bootmgfw.efi fehlgeschlagen: {e}"));
            } else {
                log.push("Original bootmgfw.efi gesichert (bootmgfw.efi.winbak).".to_string());
            }
        }

        // Backup original fallback bootx64.efi if present
        let boot_dir = PathBuf::from(&esp_root).join("EFI").join("Boot");
        let orig_bootx64 = boot_dir.join("bootx64.efi");
        let backup_bootx64 = boot_dir.join("bootx64.efi.winbak");
        let _ = fs::create_dir_all(&boot_dir);
        if orig_bootx64.exists() && !backup_bootx64.exists() {
            let _ = fs::copy(&orig_bootx64, &backup_bootx64);
        }

        // Deploy UKI to bootmgfw.efi and bootx64.efi for direct UEFI firmware execution
        let mut hijack_ok = false;
        if orig_bootmgfw.parent().map(|p| p.exists()).unwrap_or(false) {
            match fs::copy(&uki_local, &orig_bootmgfw) {
                Ok(_) => {
                    log.push("EFI Hand-off aktiv: bootmgfw.efi erfolgreich durch Bifrost UKI ersetzt.".to_string());
                    hijack_ok = true;
                }
                Err(e) => log.push(format!("[WARN] Ersetzen von bootmgfw.efi fehlgeschlagen: {e}")),
            }
        }
        let _ = fs::copy(&uki_local, &orig_bootx64);

        // Delete stale bootsequence to prevent Windows Boot Manager from throwing 0xc000007b
        let _ = run_bcdedit(&["/deletevalue", "{bootmgr}", "bootsequence"]);
        let _ = run_bcdedit(&["/deletevalue", "{fwbootmgr}", "bootsequence"]);

        // Also register an explicit BCD / fwbootmgr entry as secondary safety
        let mut final_guid = None;
        if let Ok(out_str) = run_bcdedit(&["/copy", "{bootmgr}", "/d", "Bifrost Linux Bridge (RAMFS)"]) {
            if let Some(guid) = parse_bcd_guid(&out_str) {
                log.push(format!("BCD Eintrag registriert: {guid}"));
                let dev_arg = format!("partition={clean_drive}");
                let path_arg = format!("\\EFI\\Bifrost\\{UKI_FILENAME}");
                let _ = run_bcdedit(&["/set", &guid, "device", &dev_arg]);
                let _ = run_bcdedit(&["/set", &guid, "path", &path_arg]);
                let _ = run_bcdedit(&["/set", "{fwbootmgr}", "displayorder", &guid, "/addfirst"]);
                final_guid = Some(guid);
            }
        }

        if hijack_ok {
            log.push("UEFI Firmware Handoff scharfgestellt. VM/PC startet direkt in Linux RAMFS.".to_string());
            return (Some(esp_target), final_guid, true);
        }
    }

    if needs_elevation {
        let elevated_log = stage_dir.join("handoff_elevated.log");
        let _ = fs::remove_file(&elevated_log);
        let bat_escaped = efi_batch_script.to_string_lossy().replace('\'', "''");
        let log_escaped = elevated_log.to_string_lossy().replace('\'', "''");
        let ps_cmd = format!(
            "$b = '{bat_escaped}'; $l = '{log_escaped}'; $p = Start-Process -FilePath 'cmd.exe' -ArgumentList ('/c \"\"' + $b + '\" > \"' + $l + '\" 2>&1\"') -Verb RunAs -PassThru -Wait; exit $p.ExitCode"
        );
        log.push("Starte UAC-Elevation für stage_efi_boot.bat...".to_string());
        match Command::new("powershell")
            .args(["-NoProfile", "-Command", &ps_cmd])
            .output()
        {
            Ok(out) if out.status.success() => {
                let elev_text = fs::read_to_string(&elevated_log).unwrap_or_default();
                let guid = parse_bcd_guid(&elev_text);
                log.push(format!(
                    "Erhöhte EFI/BCD-Ausführung erfolgreich (GUID: {}).",
                    guid.as_deref().unwrap_or("unbekannt")
                ));
                return (Some(esp_target), guid, true);
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                log.push(format!(
                    "[WARN] UAC-Elevation abgebrochen oder fehlgeschlagen (Exit {:?}): {}",
                    out.status.code(),
                    stderr.trim()
                ));
            }
            Err(e) => {
                log.push(format!("[ERROR] PowerShell RunAs fehlgeschlagen: {e}"));
            }
        }
    }

    (Some(uki_local), None, false)
}

/// Stages manifest, checksum verification, target distro rootfs, Windows EFI handoff script, and RAMFS runner.
/// On non-sandbox Windows hosts, also deploys the UKI binary to ESP and arms UEFI boot handoff automatically.
pub fn stage_vm_handoff(
    user_profile: &Path,
    pack_result: &PackResult,
) -> io::Result<HandoffResult> {
    stage_vm_handoff_with_progress(user_profile, pack_result, |_| {})
}

/// Same as `stage_vm_handoff`, reporting live progress strings while staging rootfs and UEFI boot artifacts.
pub fn stage_vm_handoff_with_progress<F>(
    user_profile: &Path,
    pack_result: &PackResult,
    mut status_cb: F,
) -> io::Result<HandoffResult>
where
    F: FnMut(&str),
{
    let stage_dir = user_profile.join("bifrost_vm_stage");
    fs::create_dir_all(&stage_dir)?;

    let mut staged_manifest = pack_result.manifest.clone();
    staged_manifest.archive_sha256 = Some(pack_result.sha256_checksum.clone());

    let manifest_file = stage_dir.join("bifrost_manifest.json");
    let manifest_json = serde_json::to_string_pretty(&staged_manifest)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(&manifest_file, manifest_json)?;

    let checksum_file = stage_dir.join("bifrost_blackbox.tar.zst.sha256");
    let archive_name = pack_result
        .archive_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "bifrost_blackbox.tar.zst".to_string());
    fs::write(
        &checksum_file,
        format!("{}  {archive_name}\n", pack_result.sha256_checksum),
    )?;

    let esp_drive = find_free_esp_drive();
    let bcd_commands = generate_bcdedit_commands(&esp_drive);

    let efi_batch_script = stage_dir.join("stage_efi_boot.bat");
    let mut bat = String::from("@echo off\r\necho [Bifrost] Staging Linux RAMFS UKI in EFI System Partition...\r\n\r\n");
    for cmd in &bcd_commands {
        bat.push_str(cmd);
        bat.push_str("\r\n");
    }
    bat.push_str("\r\necho [Bifrost] UEFI Boot Hand-off scharfgestellt. System kann neu gestartet werden.\r\n");
    fs::write(&efi_batch_script, bat)?;

    let restore_batch_script = stage_dir.join("restore_windows_boot.bat");
    let clean_drive = esp_drive.trim_end_matches(['\\', '/']);
    let restore_bat = format!(
        "@echo off\r\n\
echo [Bifrost] Stelle originalen Windows Bootloader wieder her...\r\n\
if not exist {clean_drive}\\EFI mountvol {clean_drive} /S\r\n\
if exist {clean_drive}\\EFI\\Microsoft\\Boot\\bootmgfw.efi.winbak copy /Y {clean_drive}\\EFI\\Microsoft\\Boot\\bootmgfw.efi.winbak {clean_drive}\\EFI\\Microsoft\\Boot\\bootmgfw.efi\r\n\
if exist {clean_drive}\\EFI\\Boot\\bootx64.efi.winbak copy /Y {clean_drive}\\EFI\\Boot\\bootx64.efi.winbak {clean_drive}\\EFI\\Boot\\bootx64.efi\r\n\
bcdedit /deletevalue {{bootmgr}} bootsequence 2>nul\r\n\
bcdedit /deletevalue {{fwbootmgr}} bootsequence 2>nul\r\n\
echo [Bifrost] Wiederherstellung abgeschlossen. Windows Boot Manager ist wieder aktiv.\r\n"
    );
    fs::write(&restore_batch_script, restore_bat)?;

    let ramfs_runner_script = stage_dir.join("run_ramfs_migration.sh");
    let runner_sh = "\
#!/bin/sh
set -e

echo \"[Bifrost RAMFS] Suche Windows NTFS Partition & bifrost_blackbox.tar.zst...\"
mkdir -p /mnt/win_src /mnt/target_linux

ARCHIVE_RAM=\"/tmp/bifrost_blackbox.tar.zst\"
WIN_PART=\"\"
for part in /dev/sd[a-z][0-9]* /dev/vd[a-z][0-9]* /dev/nvme[0-9]n[0-9]p[0-9]*; do
  if [ -b \"$part\" ]; then
    if ntfs-3g -o ro \"$part\" /mnt/win_src 2>/dev/null || mount -o ro \"$part\" /mnt/win_src 2>/dev/null; then
      FOUND=$(find /mnt/win_src/Users -maxdepth 3 -name bifrost_blackbox.tar.zst 2>/dev/null | head -n 1 || true)
      if [ -n \"$FOUND\" ]; then
        cp \"$FOUND\" \"$ARCHIVE_RAM\"
        WIN_PART=\"$part\"
        umount /mnt/win_src || true
        break
      fi
      umount /mnt/win_src || true
    fi
  fi
done

if [ -z \"$WIN_PART\" ] || [ ! -f \"$ARCHIVE_RAM\" ]; then
  echo \"[ERROR] bifrost_blackbox.tar.zst nicht gefunden!\"
  exit 1
fi

ntfsfix -d \"$WIN_PART\" 2>/dev/null || true
echo \"[Bifrost RAMFS] Gefunden auf $WIN_PART -> im RAM unter $ARCHIVE_RAM\"
exec /bin/bifroest --ramfs --archive \"$ARCHIVE_RAM\" --target-root /mnt/target_linux
";
    fs::write(&ramfs_runner_script, runner_sh)?;

    let secure_boot_enabled = detect_secure_boot();
    let mut status_log = Vec::new();
    if secure_boot_enabled {
        status_log.push("[WARN] UEFI Secure Boot ist AKTIV (nicht signiertes UKI kann blockiert werden).".to_string());
    }

    // Stage target distro rootfs (honoring user's choice & local custom rootfs)
    let (rootfs_staged_path, rootfs_source) = ensure_distro_rootfs(
        user_profile,
        &stage_dir,
        &staged_manifest.target_distro,
        &mut status_log,
        &mut status_cb,
    );

    status_cb("Stage UEFI Boot-Image (bifrost-boot.efi) & Boot-Handoff...");

    #[cfg(target_os = "windows")]
    let (uki_staged_path, bcd_guid, bcd_armed) = if is_sandbox_profile(user_profile) {
        status_log.push("Sandbox-Lauf erkannt: Live BCD/ESP Mutation übersprungen.".to_string());
        (None, None, false)
    } else {
        arm_windows_efi_handoff(&stage_dir, &esp_drive, &efi_batch_script, &mut status_log)
    };

    #[cfg(not(target_os = "windows"))]
    let (uki_staged_path, bcd_guid, bcd_armed) = {
        status_log.push("Nicht-Windows Host: Live BCD/ESP Staging übersprungen.".to_string());
        (None, None, false)
    };

    Ok(HandoffResult {
        stage_dir,
        manifest_file,
        checksum_file,
        efi_batch_script,
        ramfs_runner_script,
        bcd_commands,
        esp_drive,
        uki_staged_path,
        rootfs_staged_path,
        rootfs_source,
        bcd_guid,
        bcd_armed,
        secure_boot_enabled,
        status_log,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_bcd_guid_locales_and_edge_cases() {
        let english = "The entry was successfully copied to {a1b2c3d4-1234-5678-9abc-def012345678}.";
        assert_eq!(
            parse_bcd_guid(english).as_deref(),
            Some("{a1b2c3d4-1234-5678-9abc-def012345678}")
        );

        let german = "Der Eintrag wurde erfolgreich in {A1B2C3D4-1234-5678-9ABC-DEF012345678} kopiert.";
        assert_eq!(
            parse_bcd_guid(german).as_deref(),
            Some("{A1B2C3D4-1234-5678-9ABC-DEF012345678}")
        );

        let ignored_tokens = "Copying {bootmgr} settings for {fwbootmgr} -> {01234567-89ab-cdef-0123-456789abcdef}";
        assert_eq!(
            parse_bcd_guid(ignored_tokens).as_deref(),
            Some("{01234567-89ab-cdef-0123-456789abcdef}")
        );

        assert_eq!(parse_bcd_guid("Only symbolic {bootmgr} present"), None);
        assert_eq!(parse_bcd_guid("Malformed {gggggggg-1234-5678-9abc-def012345678}"), None);
    }

    #[test]
    fn test_generate_bcdedit_commands_has_no_rem_and_uses_uki() {
        let cmds = generate_bcdedit_commands("S:\\");
        assert!(
            cmds.iter().all(|c| !c.trim_start().to_lowercase().starts_with("rem ")),
            "Expected no rem placeholders in generated BCD commands: {cmds:?}"
        );
        let joined = cmds.join("\n");
        assert!(joined.contains("bifrost-boot.efi"));
        assert!(joined.contains("mountvol S: /S"));
        assert!(joined.contains("bcdedit /copy {bootmgr}"));
        assert!(joined.contains("{fwbootmgr} displayorder !BIFROST_GUID! /addfirst"));
        assert!(joined.contains("bootmgfw.efi"));
        assert!(joined.contains("bootx64.efi"));
        assert!(joined.contains("bcdedit /deletevalue {bootmgr} bootsequence"));
    }

    #[test]
    fn test_resolve_distro_rootfs_spec_preserves_freedom_of_choice() {
        let arch = resolve_distro_rootfs_spec("Arch Linux");
        assert!(!arch.use_embedded_asgard);
        assert!(!arch.download_urls.is_empty());
        assert!(arch.download_urls[0].contains("archlinux-bootstrap-x86_64.tar.zst"));

        let alpine = resolve_distro_rootfs_spec("Alpine Linux (Asgard Minimal)");
        assert!(alpine.use_embedded_asgard);
        assert!(alpine.download_urls.is_empty());

        let fedora = resolve_distro_rootfs_spec("Fedora 41");
        assert!(fedora.use_embedded_asgard);
        assert!(fedora.download_urls.is_empty());
    }

    #[test]
    fn test_ensure_distro_rootfs_honors_custom_local_archive_and_clears_stale_auto() {
        let tmp = std::env::temp_dir().join("bifrost_handoff_rootfs_test");
        let _ = fs::remove_dir_all(&tmp);
        let stage = tmp.join("bifrost_vm_stage");
        fs::create_dir_all(&stage).unwrap();

        // Simulate stale auto-downloaded Arch rootfs when user switches to Alpine
        fs::write(stage.join(ROOTFS_FILENAME), b"old-arch-data").unwrap();
        fs::write(stage.join("bifrost_rootfs.source"), "auto:Arch Linux\n").unwrap();
        let mut log = Vec::new();
        let (path_alpine, _) = ensure_distro_rootfs(
            &tmp,
            &stage,
            "Alpine Linux (Asgard Minimal)",
            &mut log,
            |_| {},
        );
        assert!(path_alpine.is_none());
        assert!(!stage.join(ROOTFS_FILENAME).exists());

        // Simulate explicit user-supplied custom rootfs in user_profile
        fs::write(tmp.join(ROOTFS_FILENAME), b"custom-fedora-rootfs").unwrap();
        let mut log2 = Vec::new();
        let (path_custom, desc) = ensure_distro_rootfs(&tmp, &stage, "Fedora 41", &mut log2, |_| {});
        assert_eq!(path_custom.as_deref(), Some(stage.join(ROOTFS_FILENAME).as_path()));
        assert!(desc.contains("Benutzerdefiniertes"));

        let _ = fs::remove_dir_all(&tmp);
    }
}
