#[cfg(target_os = "windows")]
use std::process::Command;
use sysinfo::Disks;

use crate::model::DiskScanInfo;

pub fn scan_system_disk() -> Option<DiskScanInfo> {
    let disks = Disks::new_with_refreshed_list().ok()?;

    // Iteriere über die Disks (d ist &Disk)
    let primary_disk = disks.iter().find(|d| {
        let mount = d.mount_point().to_string_lossy();
        mount.starts_with("C:") || mount == "/"
    }).or_else(|| disks.iter().next());

    let (mount, fs_name, total, avail) = if let Some(d) = primary_disk {
        (
            d.mount_point().to_string_lossy().into_owned(),
            d.file_system().to_string_lossy().into_owned(),
            d.total_space(),
            d.available_space(),
        )
    } else {
        ("C:\\".to_string(), "NTFS".to_string(), 0, 0)
    };

    let (is_bitlocker_active, bitlocker_status) = check_bitlocker_status(&mount);

    Some(DiskScanInfo {
        drive_letter: "C:".to_string(),
        mount_point: mount,
        file_system: fs_name,
        total_bytes: total,
        available_bytes: avail,
        is_bitlocker_active,
        bitlocker_status,
    })
}

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
