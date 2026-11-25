use std::{io, path::Path};

#[cfg(target_os = "linux")]
use std::process::Command;

/// Splits a Linux partition path (e.g. `/dev/nvme0n1p1`, `/dev/sda1`) into `(disk_dev, part_num)`.
pub fn split_partition_disk_and_num(part_dev: &str) -> (String, u32) {
    let trimmed = part_dev.trim();
    let digits_start = trimmed
        .rfind(|c: char| !c.is_ascii_digit())
        .map(|idx| idx + 1)
        .unwrap_or(trimmed.len());
    if digits_start == trimmed.len() {
        return (trimmed.to_string(), 1);
    }
    let part_num = trimmed[digits_start..].parse::<u32>().unwrap_or(1);
    let prefix = &trimmed[..digits_start];
    let disk = if prefix.ends_with('p')
        && prefix[..prefix.len() - 1]
            .chars()
            .last()
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false)
    {
        &prefix[..prefix.len() - 1]
    } else {
        prefix
    };
    (disk.to_string(), part_num)
}

/// Extracts the 4-digit hex UEFI BootNum (e.g. `"000A"`) matching `label` from `efibootmgr -v` stdout.
pub fn parse_efibootmgr_bootnum(output: &str, label: &str) -> Option<String> {
    let needle = label.trim().to_ascii_lowercase();
    for raw in output.lines() {
        let line = raw.trim();
        if !line.starts_with("Boot") || line.len() < 8 {
            continue;
        }
        let code = &line[4..8];
        if code.chars().all(|c| c.is_ascii_hexdigit())
            && line.to_ascii_lowercase().contains(&needle)
        {
            return Some(code.to_ascii_uppercase());
        }
    }
    None
}

/// Generates `systemd-boot` entries for the new target subvolume and optional rollback subvolume.
pub fn generate_systemd_boot_hop_entries(
    distro_name: &str,
    slug: &str,
    root_uuid: &str,
    new_subvol: &str,
    old_subvol: Option<&str>,
    luks_uuid: Option<&str>,
) -> (String, Option<String>) {
    let root_arg = if let Some(luks) = luks_uuid {
        format!("root=/dev/mapper/cryptroot rd.luks.uuid={luks} cryptdevice=UUID={luks}:cryptroot")
    } else {
        format!("root=UUID={root_uuid}")
    };

    let new_entry = format!(
        "\
title   Bifrost {distro_name}
linux   /EFI/BifrostLinux/{slug}/vmlinuz
initrd  /EFI/BifrostLinux/{slug}/initramfs.img
options {root_arg} rootflags=subvol={new_subvol} rw quiet
"
    );

    let rollback_entry = old_subvol.map(|old| {
        format!(
            "\
title   Bifrost Rollback ({old})
linux   /EFI/BifrostLinux/rollback/vmlinuz
initrd  /EFI/BifrostLinux/rollback/initramfs.img
options {root_arg} rootflags=subvol={old} rw
"
        )
    });

    (new_entry, rollback_entry)
}

/// Builds deterministic `efibootmgr` shell command strings for logging and dry-run previews.
pub fn generate_efibootmgr_commands(
    esp_disk: &str,
    esp_part_num: u32,
    efi_loader_path: &str,
    label: &str,
    boot_num: Option<&str>,
) -> Vec<String> {
    let win_loader = efi_loader_path.replace('/', "\\");
    let mut cmds = vec![format!(
        "efibootmgr --create --disk {esp_disk} --part {esp_part_num} --loader '{win_loader}' --label '{label}'"
    )];
    if let Some(num) = boot_num {
        cmds.push(format!("efibootmgr --bootnext {num}"));
    } else {
        cmds.push("efibootmgr --bootnext <NEW_BOOTNUM>".to_string());
    }
    cmds
}

/// Writes `loader.conf` + `bifroest-new.conf` (and optional `bifroest-rollback.conf`) onto `esp_mount/loader/`.
pub fn write_systemd_boot_hop_files(
    esp_mount: &Path,
    new_entry: &str,
    rollback_entry: Option<&str>,
    dry_run: bool,
) -> io::Result<()> {
    if dry_run {
        return Ok(());
    }
    let loader_dir = esp_mount.join("loader");
    let entries_dir = loader_dir.join("entries");
    std::fs::create_dir_all(&entries_dir)?;

    let loader_conf = "\
default bifroest-new.conf
timeout 5
console-mode max
editor no
";
    std::fs::write(loader_dir.join("loader.conf"), loader_conf)?;
    std::fs::write(entries_dir.join("bifroest-new.conf"), new_entry)?;

    if let Some(rb) = rollback_entry {
        std::fs::write(entries_dir.join("bifroest-rollback.conf"), rb)?;
    }
    Ok(())
}

/// Creates/updates a UEFI Boot entry via `efibootmgr` and arms `--bootnext` for Zero-BIOS reboot.
pub fn arm_linux_efibootmgr_handoff(
    esp_mount: &Path,
    disk_dev: &str,
    part_num: u32,
    efi_loader_rel: &str,
    label: &str,
    dry_run: bool,
) -> io::Result<Vec<String>> {
    if dry_run {
        return Ok(generate_efibootmgr_commands(
            disk_dev,
            part_num,
            efi_loader_rel,
            label,
            Some("000B"),
        ));
    }

    #[cfg(target_os = "linux")]
    {
        let mut logs = Vec::new();
        let win_loader = efi_loader_rel.replace('/', "\\");

        let existing = Command::new("efibootmgr")
            .arg("-v")
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();

        let mut boot_num = parse_efibootmgr_bootnum(&existing, label);
        if boot_num.is_none() {
            let part_str = part_num.to_string();
            let create_out = Command::new("efibootmgr")
                .args([
                    "--create",
                    "--disk",
                    disk_dev,
                    "--part",
                    &part_str,
                    "--loader",
                    &win_loader,
                    "--label",
                    label,
                ])
                .output()?;
            let out_str = String::from_utf8_lossy(&create_out.stdout).into_owned();
            if !create_out.status.success() {
                let err = String::from_utf8_lossy(&create_out.stderr);
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!("efibootmgr --create fehlgeschlagen: {err}"),
                ));
            }
            logs.push(format!("efibootmgr --create {label} ({win_loader}) ausgefuehrt."));
            boot_num = parse_efibootmgr_bootnum(&out_str, label);
        }

        if let Some(num) = boot_num {
            let next_out = Command::new("efibootmgr")
                .args(["--bootnext", &num])
                .output()?;
            if !next_out.status.success() {
                let err = String::from_utf8_lossy(&next_out.stderr);
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!("efibootmgr --bootnext {num} fehlgeschlagen: {err}"),
                ));
            }
            logs.push(format!("UEFI BootNext scharfgestellt auf Boot{num} ({label})."));
        } else {
            logs.push(format!(
                "[WARN] BootNum fuer {label} nicht geparst; nutze Standard-Bootorder von {}",
                esp_mount.display()
            ));
        }

        return Ok(logs);
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = esp_mount;
        Ok(generate_efibootmgr_commands(
            disk_dev,
            part_num,
            efi_loader_rel,
            label,
            Some("000B"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_efibootmgr_parsing_and_systemd_boot_entries() {
        let sample = "\
BootCurrent: 0002
Timeout: 1 seconds
BootOrder: 0002,0001
Boot0001* Windows Boot Manager\tHD(1,GPT,1111)/File(\\EFI\\Microsoft\\Boot\\bootmgfw.efi)
Boot000A* Bifrost Linux\tHD(1,GPT,1111)/File(\\EFI\\systemd\\systemd-bootx64.efi)
";
        assert_eq!(
            parse_efibootmgr_bootnum(sample, "Bifrost Linux").as_deref(),
            Some("000A")
        );
        assert_eq!(parse_efibootmgr_bootnum(sample, "NonExistent"), None);

        assert_eq!(
            split_partition_disk_and_num("/dev/nvme0n1p1"),
            ("/dev/nvme0n1".to_string(), 1)
        );
        assert_eq!(
            split_partition_disk_and_num("/dev/sda2"),
            ("/dev/sda".to_string(), 2)
        );

        let (new_conf, rb_conf) = generate_systemd_boot_hop_entries(
            "Arch Linux",
            "arch",
            "root-1234",
            "@root_arch",
            Some("@root"),
            Some("luks-9999"),
        );
        assert!(new_conf.contains("title   Bifrost Arch Linux"));
        assert!(new_conf.contains("linux   /EFI/BifrostLinux/arch/vmlinuz"));
        assert!(new_conf.contains("rd.luks.uuid=luks-9999"));
        assert!(new_conf.contains("rootflags=subvol=@root_arch"));

        let rb = rb_conf.expect("Expected rollback entry");
        assert!(rb.contains("rootflags=subvol=@root"));
        assert!(rb.contains("cryptdevice=UUID=luks-9999:cryptroot"));

        let cmds = generate_efibootmgr_commands(
            "/dev/nvme0n1",
            1,
            "/EFI/Bifrost/bifrost-boot.efi",
            "Bifrost RAMFS",
            Some("000C"),
        );
        assert!(cmds[0].contains("\\EFI\\Bifrost\\bifrost-boot.efi"));
        assert_eq!(cmds[1], "efibootmgr --bootnext 000C");
    }
}
