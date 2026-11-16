use std::{
    fs::{self, File},
    io::{self, BufReader},
    path::Path,
};
use tar::Archive;
use zstd::Decoder;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[cfg(target_os = "linux")]
use std::process::Command;

use crate::model::MigrationManifest;

/// Generates `systemd-boot` `loader.conf` and boot entry configuration for the migrated Linux target.
pub fn generate_systemd_boot_configs(
    distro_name: &str,
    root_uuid: &str,
    target_fs: &str,
) -> (String, String) {
    let loader_conf = "\
default bifroest-linux.conf
timeout 3
console-mode max
editor no
"
    .to_string();

    let subvol_opt = if target_fs.to_lowercase().contains("btrfs") {
        " rootflags=subvol=@root"
    } else {
        ""
    };

    let entry_conf = format!(
        "\
title   Bifrost {distro_name}
linux   /vmlinuz-linux
initrd  /initramfs-linux.img
options root=UUID={root_uuid}{subvol_opt} rw quiet
"
    );

    (loader_conf, entry_conf)
}

/// Generates the systemd one-shot unit and shell script for first-boot Flatpak and native app installation.
pub fn generate_postinstall_service(manifest: &MigrationManifest) -> (String, String) {
    let service_unit = "\
[Unit]
Description=Bifrost First-Boot Software Provisioning
After=network-online.target
Wants=network-online.target
ConditionPathExists=!/var/lib/bifrost/.postinstall_done

[Service]
Type=oneshot
ExecStart=/usr/local/bin/bifrost-postinstall.sh
RemainAfterExit=yes

[Install]
WantedBy=multi-user.target
"
    .to_string();

    let distro_lc = manifest.target_distro.to_lowercase();
    let mut script = String::from("#!/bin/sh\nset -e\n\n");
    script.push_str("echo \"[Bifrost] Starte First-Boot Software-Provisioning...\"\n");
    script.push_str(
        "flatpak remote-add --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo || true\n\n",
    );

    for sw in manifest.selected_software.iter().filter(|s| s.will_reinstall) {
        if let Some(fp_id) = &sw.flatpak_id {
            script.push_str(&format!(
                "flatpak install -y --noninteractive flathub {fp_id} || true\n"
            ));
        } else if let Some(pkg) = &sw.native_pkg {
            let native_cmd = if distro_lc.contains("debian") || distro_lc.contains("ubuntu") {
                format!("apt-get install -y {pkg}")
            } else if distro_lc.contains("fedora") {
                format!("dnf install -y {pkg}")
            } else {
                format!("pacman -S --noconfirm {pkg}")
            };
            script.push_str(&format!("{native_cmd} || true\n"));
        }
    }

    script.push_str("\nmkdir -p /var/lib/bifrost\n");
    script.push_str("touch /var/lib/bifrost/.postinstall_done\n");
    script.push_str("echo \"[Bifrost] Provisioning abgeschlossen.\"\n");

    (service_unit, script)
}

/// Deploys optional base rootfs, writes `systemd-boot` entries, hostname, and first-boot provisioning service.
pub fn deploy_base_system<F>(
    target_root: &Path,
    base_rootfs_archive: Option<&Path>,
    manifest: &MigrationManifest,
    root_uuid: &str,
    dry_run: bool,
    mut progress_cb: F,
) -> io::Result<()>
where
    F: FnMut(&str, usize, usize),
{
    let total_steps = 4;

    if let Some(rootfs) = base_rootfs_archive {
        if rootfs.exists() && !dry_run {
            progress_cb(&format!("Entpacke Basis-Rootfs {:?}...", rootfs), 1, total_steps);
            let f = File::open(rootfs)?;
            let decoder = Decoder::new(BufReader::new(f))?;
            let mut tar = Archive::new(decoder);
            tar.unpack(target_root)?;
        } else {
            progress_cb("Überspringe Base-Rootfs Entpacken (Dry-Run/Sandbox)...", 1, total_steps);
        }
    } else {
        progress_cb("Bereite Bootloader- & Systemkonfiguration vor...", 1, total_steps);
    }

    progress_cb("Schreibe systemd-boot Konfiguration...", 2, total_steps);
    let (loader_conf, entry_conf) = generate_systemd_boot_configs(
        &manifest.target_distro,
        root_uuid,
        &manifest.target_filesystem,
    );

    let efi_loader_dir = target_root.join("boot/efi/loader");
    let efi_entries_dir = efi_loader_dir.join("entries");
    fs::create_dir_all(&efi_entries_dir)?;
    fs::write(efi_loader_dir.join("loader.conf"), loader_conf)?;
    fs::write(efi_entries_dir.join("bifroest-linux.conf"), entry_conf)?;

    #[cfg(target_os = "linux")]
    if !dry_run {
        let efi_path = target_root.join("boot/efi");
        let _ = Command::new("bootctl")
            .args(["--path", &efi_path.to_string_lossy(), "install"])
            .output();
    }

    progress_cb("Installiere First-Boot Flatpak-Service...", 3, total_steps);
    let (service_unit, postinstall_script) = generate_postinstall_service(manifest);

    let systemd_dir = target_root.join("etc/systemd/system");
    let bin_dir = target_root.join("usr/local/bin");
    fs::create_dir_all(&systemd_dir)?;
    fs::create_dir_all(&bin_dir)?;

    fs::write(systemd_dir.join("bifrost-postinstall.service"), service_unit)?;
    let script_path = bin_dir.join("bifrost-postinstall.sh");
    fs::write(&script_path, postinstall_script)?;

    #[cfg(unix)]
    {
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755))?;
    }

    progress_cb("Setze Ziel-Hostname (/etc/hostname)...", 4, total_steps);
    let etc_dir = target_root.join("etc");
    fs::create_dir_all(&etc_dir)?;
    fs::write(etc_dir.join("hostname"), "bifroest-linux\n")?;

    Ok(())
}
