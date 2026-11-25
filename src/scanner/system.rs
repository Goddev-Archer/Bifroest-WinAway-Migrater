#[cfg(any(target_os = "windows", target_os = "linux"))]
use std::process::Command;
use sysinfo::Disks;

use crate::model::DiskScanInfo;

/// Parses `PRETTY_NAME` (falling back to `NAME`) from Linux `/etc/os-release`.
pub fn parse_os_release_pretty(content: &str) -> Option<String> {
    let mut name_fallback = None;
    for raw in content.lines() {
        let line = raw.trim();
        if let Some(val) = line.strip_prefix("PRETTY_NAME=") {
            let clean = val.trim().trim_matches('"').trim_matches('\'').to_string();
            if !clean.is_empty() {
                return Some(clean);
            }
        } else if let Some(val) = line.strip_prefix("NAME=") {
            let clean = val.trim().trim_matches('"').trim_matches('\'').to_string();
            if !clean.is_empty() {
                name_fallback = Some(clean);
            }
        }
    }
    name_fallback
}

/// Parses `findmnt -n -o FSTYPE,OPTIONS /` output into `(is_btrfs, optional_subvol_name)`.
pub fn parse_findmnt_btrfs_subvol(findmnt_out: &str) -> (bool, Option<String>) {
    let trimmed = findmnt_out.trim();
    if trimmed.is_empty() {
        return (false, None);
    }
    let mut parts = trimmed.split_whitespace();
    let fstype = parts.next().unwrap_or_default();
    if !fstype.eq_ignore_ascii_case("btrfs") {
        return (false, None);
    }
    let opts = parts.next().unwrap_or_default();
    let subvol = opts
        .split(',')
        .find_map(|opt| opt.strip_prefix("subvol="))
        .map(|s| s.trim_start_matches('/').to_string())
        .filter(|s| !s.is_empty());
    (true, subvol)
}

fn scan_lsblk_luks_node(
    node: &serde_json::Value,
    parent_luks_uuid: Option<&str>,
) -> Option<(String, Option<String>)> {
    let fstype = node.get("fstype").and_then(|v| v.as_str()).unwrap_or("");
    let dev_type = node.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let name = node.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let uuid = node.get("uuid").and_then(|v| v.as_str()).map(|s| s.to_string());

    if dev_type == "crypt" {
        return Some((name.to_string(), parent_luks_uuid.map(|s| s.to_string()).or(uuid)));
    }

    let next_parent_uuid = if fstype.eq_ignore_ascii_case("crypto_LUKS") {
        uuid.as_deref().or(parent_luks_uuid)
    } else {
        parent_luks_uuid
    };

    if let Some(children) = node.get("children").and_then(|c| c.as_array()) {
        for child in children {
            if let Some(hit) = scan_lsblk_luks_node(child, next_parent_uuid) {
                return Some(hit);
            }
        }
    }
    None
}

/// Parses `/etc/crypttab` and `lsblk -J` output to detect LUKS2/`dm-crypt` containers.
pub fn parse_luks_status(crypttab: &str, lsblk_json: &str) -> (bool, Option<String>, Option<String>) {
    let mut crypttab_mapper = None;
    let mut crypttab_uuid = None;
    for raw in crypttab.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() >= 2 {
            crypttab_mapper = Some(cols[0].to_string());
            if let Some(uuid) = cols[1].strip_prefix("UUID=") {
                crypttab_uuid = Some(uuid.to_string());
            } else if !cols[1].is_empty() {
                crypttab_uuid = Some(cols[1].to_string());
            }
            break;
        }
    }

    if let Ok(val) = serde_json::from_str::<serde_json::Value>(lsblk_json) {
        if let Some(devs) = val.get("blockdevices").and_then(|d| d.as_array()) {
            for dev in devs {
                if let Some((mapper, uuid)) = scan_lsblk_luks_node(dev, None) {
                    return (
                        true,
                        if mapper.is_empty() { crypttab_mapper } else { Some(mapper) },
                        uuid.or(crypttab_uuid),
                    );
                }
            }
        }
    }

    if crypttab_mapper.is_some() {
        return (true, crypttab_mapper, crypttab_uuid);
    }
    (false, None, None)
}

#[cfg(target_os = "linux")]
fn detect_linux_esp_mount() -> Option<String> {
    for cand in ["/boot/efi", "/efi", "/boot"] {
        let p = std::path::Path::new(cand);
        if p.join("EFI").exists() {
            return Some(cand.to_string());
        }
    }
    None
}

pub fn scan_system_disk() -> Option<DiskScanInfo> {
    let disks = Disks::new_with_refreshed_list().ok()?;

    // Iteriere über die Disks (d ist &Disk)
    let primary_disk = disks
        .iter()
        .find(|d| {
            let mount = d.mount_point().to_string_lossy();
            mount.starts_with("C:") || mount == "/"
        })
        .or_else(|| disks.iter().next());

    let (mount, fs_name, total, avail) = if let Some(d) = primary_disk {
        (
            d.mount_point().to_string_lossy().into_owned(),
            d.file_system().to_string_lossy().into_owned(),
            d.total_space(),
            d.available_space(),
        )
    } else if cfg!(target_os = "linux") {
        ("/".to_string(), "btrfs".to_string(), 0, 0)
    } else {
        ("C:\\".to_string(), "NTFS".to_string(), 0, 0)
    };

    #[cfg(target_os = "linux")]
    {
        let os_pretty = std::fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|c| parse_os_release_pretty(&c));

        let findmnt_out = Command::new("findmnt")
            .args(["-n", "-o", "FSTYPE,OPTIONS", "/"])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        let (is_btrfs_subvol_capable, root_subvol_name) = parse_findmnt_btrfs_subvol(&findmnt_out);

        let crypttab = std::fs::read_to_string("/etc/crypttab").unwrap_or_default();
        let lsblk_json = Command::new("lsblk")
            .args(["-J", "-o", "NAME,TYPE,FSTYPE,UUID,MOUNTPOINT"])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        let (is_luks_active, luks_mapper_name, luks_underlying_uuid) =
            parse_luks_status(&crypttab, &lsblk_json);

        let encryption_status = if is_luks_active {
            format!(
                "LUKS2 Aktiv ({})",
                luks_mapper_name.as_deref().unwrap_or("cryptroot")
            )
        } else {
            "Unverschlüsselt / Aus".to_string()
        };

        let esp_mount_point = detect_linux_esp_mount();

        return Some(DiskScanInfo {
            drive_letter: "/".to_string(),
            mount_point: mount,
            file_system: fs_name,
            total_bytes: total,
            available_bytes: avail,
            is_bitlocker_active: false,
            bitlocker_status: encryption_status,
            is_btrfs_subvol_capable,
            root_subvol_name,
            is_luks_active,
            luks_mapper_name,
            luks_underlying_uuid,
            esp_mount_point,
            source_distro_pretty: os_pretty,
        });
    }

    #[cfg(not(target_os = "linux"))]
    {
        let (is_bitlocker_active, bitlocker_status) = check_bitlocker_status(&mount);

        Some(DiskScanInfo {
            drive_letter: "C:".to_string(),
            mount_point: mount,
            file_system: fs_name,
            total_bytes: total,
            available_bytes: avail,
            is_bitlocker_active,
            bitlocker_status,
            is_btrfs_subvol_capable: false,
            root_subvol_name: None,
            is_luks_active: false,
            luks_mapper_name: None,
            luks_underlying_uuid: None,
            esp_mount_point: None,
            source_distro_pretty: None,
        })
    }
}

#[cfg(not(target_os = "linux"))]
fn check_bitlocker_status(mount_point: &str) -> (bool, String) {
    #[cfg(target_os = "windows")]
    {
        let target = if mount_point.len() >= 2 {
            &mount_point[..2]
        } else {
            "C:"
        };

        let output = Command::new("manage-bde.exe")
            .args(["-status", target])
            .output();

        if let Ok(out) = output {
            let stdout = String::from_utf8_lossy(&out.stdout);
            if stdout.contains("Schutz ein") || stdout.contains("Protection On") {
                return (true, "Verschlüsselt (Protection ON)".to_string());
            } else if stdout.contains("Schutz aus") || stdout.contains("Protection Off") {
                return (false, "Deaktiviert (Protection OFF)".to_string());
            } else if stdout.contains("Vollständig entschlüsselt") || stdout.contains("Fully Decrypted") {
                return (false, "Entschlüsselt (Decrypted)".to_string());
            }
        }

        let ps_cmd = format!("(Get-BitLockerVolume -MountPoint '{}').ProtectionStatus", target);
        let ps_output = Command::new("powershell")
            .args(["-NoProfile", "-Command", &ps_cmd])
            .output();

        if let Ok(out) = ps_output {
            let res = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if res == "1" || res.eq_ignore_ascii_case("On") {
                return (true, "Aktiv (PowerShell: Protection ON)".to_string());
            } else if res == "0" || res.eq_ignore_ascii_case("Off") {
                return (false, "Deaktiviert (PowerShell: Protection OFF)".to_string());
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    let _ = mount_point;

    (false, "Unverschlüsselt / Aus".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_linux_system_parsers() {
        let osr = "NAME=\"Ubuntu\"\nPRETTY_NAME=\"Ubuntu 26.04.1 LTS\"\nID=ubuntu\n";
        assert_eq!(
            parse_os_release_pretty(osr).as_deref(),
            Some("Ubuntu 26.04.1 LTS")
        );

        let (btrfs_ok, subvol) =
            parse_findmnt_btrfs_subvol("btrfs rw,noatime,compress=zstd:1,subvol=/@root\n");
        assert!(btrfs_ok);
        assert_eq!(subvol.as_deref(), Some("@root"));

        let (ext4_ok, ext4_sub) = parse_findmnt_btrfs_subvol("ext4 rw,relatime\n");
        assert!(!ext4_ok);
        assert!(ext4_sub.is_none());

        let lsblk = r#"{
            "blockdevices": [
                {
                    "name": "nvme0n1",
                    "type": "disk",
                    "children": [
                        {
                            "name": "nvme0n1p2",
                            "type": "part",
                            "fstype": "crypto_LUKS",
                            "uuid": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
                            "children": [
                                {
                                    "name": "cryptroot",
                                    "type": "crypt",
                                    "fstype": "btrfs",
                                    "mountpoint": "/"
                                }
                            ]
                        }
                    ]
                }
            ]
        }"#;
        let crypttab = "cryptroot UUID=aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee none luks,discard\n";
        let (is_luks, mapper, uuid) = parse_luks_status(crypttab, lsblk);
        assert!(is_luks);
        assert_eq!(mapper.as_deref(), Some("cryptroot"));
        assert_eq!(
            uuid.as_deref(),
            Some("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee")
        );
    }
}
