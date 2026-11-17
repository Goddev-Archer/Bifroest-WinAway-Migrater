use std::{
    fs::{self, File},
    io::{self, BufReader},
    path::{Component, Path, PathBuf},
    thread,
    time::Duration,
};
use tar::Archive;
use zstd::Decoder;

#[cfg(unix)]
use std::os::unix::fs::{symlink, PermissionsExt};

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

/// Strips optional `root.x86_64/` or `./` prefix and normalizes archive entry paths safely.
fn sanitize_rootfs_rel_path(raw: &Path) -> Option<PathBuf> {
    let mut parts: Vec<String> = Vec::new();
    for comp in raw.components() {
        if let Component::Normal(s) = comp {
            parts.push(s.to_string_lossy().into_owned());
        }
    }
    if parts.first().map(|s| s == "root.x86_64").unwrap_or(false) {
        parts.remove(0);
    }
    if parts.is_empty() {
        return None;
    }
    let mut out = PathBuf::new();
    for p in parts {
        out.push(p);
    }
    Some(out)
}

/// Unpacks a compressed `.tar.zst` rootfs archive into `target_root` while preserving Btrfs `/etc/fstab`.
fn extract_rootfs_archive<F>(
    rootfs: &Path,
    target_root: &Path,
    total_steps: usize,
    progress_cb: &mut F,
) -> io::Result<usize>
where
    F: FnMut(&str, usize, usize),
{
    let f = File::open(rootfs)?;
    let decoder = Decoder::new(BufReader::new(f))?;
    let mut tar = Archive::new(decoder);
    let mut files_count = 0usize;

    for entry_res in tar.entries()? {
        let mut entry = entry_res?;
        let raw_path = entry.path()?.to_path_buf();
        let Some(clean_rel) = sanitize_rootfs_rel_path(&raw_path) else {
            continue;
        };
        let clean_str = clean_rel.to_string_lossy().replace('\\', "/");
        if clean_str == "etc/fstab" {
            continue;
        }

        let dest = target_root.join(&clean_rel);
        let entry_type = entry.header().entry_type();

        if entry_type.is_dir() {
            fs::create_dir_all(&dest)?;
        } else if entry_type.is_symlink() {
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            if let Ok(Some(link_target)) = entry.link_name() {
                let _ = fs::remove_file(&dest);
                #[cfg(unix)]
                {
                    let _ = symlink(&link_target, &dest);
                }
                #[cfg(not(unix))]
                {
                    let _ = link_target;
                }
            }
        } else if entry_type.is_hard_link() {
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            if let Ok(Some(link_name)) = entry.link_name() {
                if let Some(clean_link) = sanitize_rootfs_rel_path(&link_name) {
                    let link_target = target_root.join(clean_link);
                    if link_target.exists() {
                        let _ = fs::remove_file(&dest);
                        let _ = fs::hard_link(&link_target, &dest);
                    }
                }
            }
        } else if entry_type.is_file() {
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            let _ = fs::remove_file(&dest);
            let mut out = File::create(&dest)?;
            io::copy(&mut entry, &mut out)?;
            #[cfg(unix)]
            if let Ok(mode) = entry.header().mode() {
                let _ = fs::set_permissions(&dest, fs::Permissions::from_mode(mode));
            }
        }

        files_count += 1;
        if files_count % 25 == 0 {
            progress_cb(
                &format!("Extracting Linux rootfs ({files_count} files): {clean_str}"),
                1,
                total_steps,
            );
            thread::sleep(Duration::from_millis(2));
        }
    }

    Ok(files_count)
}

/// Deploys optional base rootfs, writes `systemd-boot` entries, Asgard shell environment, hostname, and first-boot provisioning service.
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
    let fstab_path = target_root.join("etc/fstab");
    let saved_fstab = fs::read(&fstab_path).ok();

    if let Some(rootfs) = base_rootfs_archive {
        if rootfs.exists() && !dry_run {
            progress_cb(&format!("Entpacke Basis-Rootfs {:?}...", rootfs), 1, total_steps);
            let count = extract_rootfs_archive(rootfs, target_root, total_steps, &mut progress_cb)?;
            progress_cb(
                &format!("Linux rootfs entpackt ({count} Eintraege)."),
                1,
                total_steps,
            );
        } else {
            progress_cb("Überspringe Base-Rootfs Entpacken (Dry-Run/Sandbox)...", 1, total_steps);
        }
    } else {
        progress_cb("Bereite Bootloader- & Systemkonfiguration vor...", 1, total_steps);
    }

    if let Some(fstab_bytes) = saved_fstab {
        if let Some(parent) = fstab_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&fstab_path, fstab_bytes)?;
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

    progress_cb("Konfiguriere Asgard System (/etc/hostname, /etc/os-release, Welcome Shell)...", 4, total_steps);
    let etc_dir = target_root.join("etc");
    let profile_d_dir = etc_dir.join("profile.d");
    fs::create_dir_all(&profile_d_dir)?;

    fs::write(etc_dir.join("hostname"), "asgard-linux\n")?;
    fs::write(
        etc_dir.join("os-release"),
        "NAME=\"Arch Linux\"\nPRETTY_NAME=\"Arch Linux (Bifrost Asgard)\"\nID=arch\nID_LIKE=archlinux\nHOME_URL=\"https://github.com/Goddev-Archer/Bifroest-WinAway-Migrater\"\n",
    )?;
    fs::write(
        etc_dir.join("inittab"),
        "::sysinit:/bin/mount -a\n::once:/bin/mdev -s\ntty1::respawn:/bin/sh -l\n::ctrlaltdel:/sbin/reboot\n::shutdown:/bin/umount -a -r\n",
    )?;

    let username = if manifest.target_username.trim().is_empty() {
        "linuxuser"
    } else {
        manifest.target_username.trim()
    };
    let motd_text = format!(
        "\n======================================================================\n\
           WELCOME TO ASGARD LINUX  ({distro})\n\
           Migrated by Bifrost Zero-BIOS Transit Engine\n\
         ======================================================================\n\
           Hostname:       asgard-linux\n\
           Filesystem:     {fs}\n\
           Restored Home:  /home/{username}\n\
           Blackbox Vault: /var/lib/bifrost\n\
         ======================================================================\n\n",
        distro = manifest.target_distro,
        fs = manifest.target_filesystem,
    );
    fs::write(etc_dir.join("motd"), &motd_text)?;

    let welcome_sh = format!(
        "#!/bin/sh\n\
         clear 2>/dev/null || true\n\
         cat /etc/motd 2>/dev/null || true\n\
         echo \"[Btrfs Mounts & Subvolumes]\"\n\
         mount | grep -E 'btrfs|vfat' || df -h /\n\
         echo \"\"\n\
         echo \"[Migrated Files in /home/{username}]\"\n\
         ls -la /home/{username} 2>/dev/null || true\n\
         echo \"\"\n\
         export HOME=/home/{username}\n\
         cd /home/{username} 2>/dev/null || cd /\n"
    );
    let welcome_path = profile_d_dir.join("asgard_welcome.sh");
    fs::write(&welcome_path, welcome_sh)?;
    #[cfg(unix)]
    {
        fs::set_permissions(&welcome_path, fs::Permissions::from_mode(0o755))?;
    }

    let etc_profile = etc_dir.join("profile");
    let existing_profile = fs::read_to_string(&etc_profile).unwrap_or_default();
    if !existing_profile.contains("asgard_welcome.sh") {
        let mut updated = existing_profile;
        updated.push_str("\nfor f in /etc/profile.d/*.sh; do [ -r \"$f\" ] && . \"$f\"; done\n");
        fs::write(&etc_profile, updated)?;
    }

    let marker = format!(
        "distro={}\nfilesystem={}\ntimestamp={}\nuser={}\n",
        manifest.target_distro, manifest.target_filesystem, manifest.created_at_utc, username
    );
    fs::write(target_root.join(".bifrost_installed"), marker)?;

    Ok(())
}
