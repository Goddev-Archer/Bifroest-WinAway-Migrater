use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

use crate::blackbox::PackResult;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffResult {
    pub stage_dir: PathBuf,
    pub manifest_file: PathBuf,
    pub checksum_file: PathBuf,
    pub efi_batch_script: PathBuf,
    pub ramfs_runner_script: PathBuf,
    pub bcd_commands: Vec<String>,
}

/// Builds the Windows `bcdedit` command sequence required to schedule a one-time boot into the RAMFS kernel.
pub fn generate_bcdedit_commands(esp_drive: &str) -> Vec<String> {
    let clean_drive = esp_drive.trim_end_matches(['\\', '/']);
    vec![
        format!("mountvol {clean_drive} /S"),
        format!("mkdir {clean_drive}\\EFI\\Bifrost 2>nul"),
        format!("copy /Y vmlinuz-bifrost.efi {clean_drive}\\EFI\\Bifrost\\vmlinuz-bifrost.efi"),
        format!("copy /Y initramfs-bifrost.img {clean_drive}\\EFI\\Bifrost\\initramfs-bifrost.img"),
        "bcdedit /copy {bootmgr} /d \"Bifrost Linux Bridge (RAMFS)\"".to_string(),
        "rem Set the returned {GUID} below to arm the one-time boot sequence:".to_string(),
        "rem bcdedit /set {NEW_GUID} path \\EFI\\Bifrost\\vmlinuz-bifrost.efi".to_string(),
        "rem bcdedit /bootsequence {NEW_GUID}".to_string(),
        "rem shutdown /r /t 5".to_string(),
    ]
}

/// Stages manifest, checksum verification, Windows EFI handoff script, and RAMFS runner for VM testing.
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

    let bcd_commands = generate_bcdedit_commands("S:");

    let efi_batch_script = stage_dir.join("stage_efi_boot.bat");
    let mut bat = String::from("@echo off\r\necho [Bifrost] Staging Linux RAMFS in EFI System Partition...\r\n\r\n");
    for cmd in &bcd_commands {
        bat.push_str(cmd);
        bat.push_str("\r\n");
    }
    bat.push_str("\r\necho [Bifrost] Handoff-Skript abgeschlossen. Fuehre Reboot ins RAMFS durch.\r\n");
    fs::write(&efi_batch_script, bat)?;

    let ramfs_runner_script = stage_dir.join("run_ramfs_migration.sh");
    let runner_sh = "\
#!/bin/sh
set -e

echo \"[Bifrost RAMFS] Suche Windows NTFS Partition & bifrost_blackbox.tar.zst...\"
mkdir -p /mnt/win_src /mnt/target_linux

ARCHIVE_PATH=\"\"
for part in /dev/vda3 /dev/sda3 /dev/nvme0n1p3; do
  if [ -b \"$part\" ]; then
    if ntfs-3g -o ro \"$part\" /mnt/win_src 2>/dev/null; then
      FOUND=$(find /mnt/win_src/Users -maxdepth 3 -name bifrost_blackbox.tar.zst 2>/dev/null | head -n 1)
      if [ -n \"$FOUND\" ]; then
        ARCHIVE_PATH=\"$FOUND\"
        break
      fi
      umount /mnt/win_src || true
    fi
  fi
done

if [ -z \"$ARCHIVE_PATH\" ]; then
  echo \"[ERROR] bifrost_blackbox.tar.zst nicht gefunden!\"
  exit 1
fi

echo \"[Bifrost RAMFS] Gefunden: $ARCHIVE_PATH\"
exec /bin/bifroest --ramfs --archive \"$ARCHIVE_PATH\" --target-root /mnt/target_linux
";
    fs::write(&ramfs_runner_script, runner_sh)?;

    Ok(HandoffResult {
        stage_dir,
        manifest_file,
        checksum_file,
        efi_batch_script,
        ramfs_runner_script,
        bcd_commands,
    })
}
