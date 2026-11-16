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

/// Builds active Windows commands to mount ESP, stage `bifrost-boot.efi`, and arm `bcdedit`.
pub fn generate_bcdedit_commands(esp_drive: &str) -> Vec<String> {
    let clean_drive = esp_drive.trim_end_matches(['\\', '/']);
    vec![
        "setlocal EnableDelayedExpansion".to_string(),
        format!("if not exist {clean_drive}\\EFI mountvol {clean_drive} /S"),
        format!("if not exist {clean_drive}\\EFI\\Bifrost mkdir {clean_drive}\\EFI\\Bifrost"),
        format!("copy /Y \"%~dp0{UKI_FILENAME}\" {clean_drive}\\EFI\\Bifrost\\{UKI_FILENAME} || exit /b 1"),
        format!("if not exist {clean_drive}\\EFI\\Bifrost\\{UKI_FILENAME} exit /b 1"),
        "set \"BIFROST_GUID=\"".to_string(),
        "for /f \"tokens=2 delims={}\" %%g in ('bcdedit /copy {bootmgr} /d \"Bifrost Linux Bridge (RAMFS)\" ^| findstr \"{\"') do set \"BIFROST_GUID={%%g}\"".to_string(),
        "if not defined BIFROST_GUID exit /b 1".to_string(),
        format!("bcdedit /set !BIFROST_GUID! device partition={clean_drive}"),
        format!("bcdedit /set !BIFROST_GUID! path \\EFI\\Bifrost\\{UKI_FILENAME} || exit /b 1"),
        "bcdedit /set {fwbootmgr} displayorder !BIFROST_GUID! /addfirst".to_string(),
        "bcdedit /bootsequence !BIFROST_GUID!".to_string(),
        "bcdedit /set {fwbootmgr} bootsequence !BIFROST_GUID! 2>nul".to_string(),
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
        match run_bcdedit(&["/copy", "{bootmgr}", "/d", "Bifrost Linux Bridge (RAMFS)"]) {
            Ok(out_str) => {
                if let Some(guid) = parse_bcd_guid(&out_str) {
                    log.push(format!("BCD Eintrag erstellt: {guid}"));
                    let dev_arg = format!("partition={clean_drive}");
                    let path_arg = format!("\\EFI\\Bifrost\\{UKI_FILENAME}");

                    if let Err(e) = run_bcdedit(&["/set", &guid, "device", &dev_arg]) {
                        log.push(format!("[WARN] {e}"));
                    }
                    if let Err(e) = run_bcdedit(&["/set", &guid, "path", &path_arg]) {
                        log.push(format!("[ERROR] {e}"));
                        return (Some(esp_target), Some(guid), false);
                    }

                    let fw_order_ok = run_bcdedit(&["/set", "{fwbootmgr}", "displayorder", &guid, "/addfirst"]).is_ok();
                    let fw_seq_ok = run_bcdedit(&["/set", "{fwbootmgr}", "bootsequence", &guid]).is_ok();
                    let bm_seq_ok = run_bcdedit(&["/bootsequence", &guid]).is_ok();

                    if fw_order_ok || fw_seq_ok || bm_seq_ok {
                        log.push(format!(
                            "One-Time Boot scharfgestellt (fw_order={fw_order_ok}, fw_seq={fw_seq_ok}, bm_seq={bm_seq_ok})."
                        ));
                        return (Some(esp_target), Some(guid), true);
                    }
                    log.push("[ERROR] Keiner der bcdedit Boot-Trigger war erfolgreich.".to_string());
                    return (Some(esp_target), Some(guid), false);
                }
                log.push(format!("[ERROR] Konnte GUID nicht aus bcdedit parsen: {}", out_str.trim()));
                return (Some(esp_target), None, false);
            }
            Err(e) => {
                log.push(format!("bcdedit benötigt Admin-Rechte: {e}"));
                needs_elevation = true;
            }
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

/// Stages manifest, checksum verification, Windows EFI handoff script, and RAMFS runner for VM testing.
/// On non-sandbox Windows hosts, also deploys the UKI binary to ESP and arms `bcdedit` automatically.
pub fn stage_vm_handoff(
    user_profile: &Path,
    pack_result: &PackResult,
) -> io::Result<HandoffResult> {
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
    bat.push_str("\r\necho [Bifrost] BCD Entry !BIFROST_GUID! aktiviert. System kann neu gestartet werden.\r\n");
    fs::write(&efi_batch_script, bat)?;

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
        assert!(joined.contains("bcdedit /bootsequence !BIFROST_GUID!"));
    }
}
