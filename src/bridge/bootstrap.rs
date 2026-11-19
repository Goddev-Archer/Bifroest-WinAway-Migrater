use std::{
    fs::{self, File},
    io::{self, BufReader},
    path::{Component, Path, PathBuf},
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
            let is_symlink = dest
                .symlink_metadata()
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false);
            if !is_symlink {
                fs::create_dir_all(&dest)?;
                #[cfg(unix)]
                if let Ok(mode) = entry.header().mode() {
                    let _ = fs::set_permissions(&dest, fs::Permissions::from_mode(mode & 0o7777));
                }
            }
        } else if entry_type.is_symlink() {
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            if let Ok(Some(link_target)) = entry.link_name() {
                if let Ok(meta) = dest.symlink_metadata() {
                    if meta.is_dir() && !meta.file_type().is_symlink() {
                        let _ = fs::remove_dir(&dest);
                    } else {
                        let _ = fs::remove_file(&dest);
                    }
                }
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
                let _ = fs::set_permissions(&dest, fs::Permissions::from_mode(mode & 0o7777));
            }
        }

        files_count += 1;
        if files_count % 100 == 0 {
            progress_cb(
                &format!("Extracting Linux rootfs ({files_count} files): {clean_str}"),
                1,
                total_steps,
            );
        }
    }

    #[cfg(unix)]
    {
        let tmp_dir = target_root.join("tmp");
        let _ = fs::create_dir_all(&tmp_dir);
        let _ = fs::set_permissions(&tmp_dir, fs::Permissions::from_mode(0o1777));
    }

    Ok(files_count)
}

/// Appends standard Linux device/system groups missing from `/etc/group` without touching existing entries.
fn ensure_standard_groups(etc_dir: &Path) -> io::Result<()> {
    let group_path = etc_dir.join("group");
    let existing = fs::read_to_string(&group_path).unwrap_or_else(|_| "root:x:0:\n".to_string());
    let mut updated = existing.clone();
    if !updated.ends_with('\n') && !updated.is_empty() {
        updated.push('\n');
    }

    let defaults = [
        ("root", "root:x:0:"),
        ("tty", "tty:x:5:"),
        ("disk", "disk:x:6:"),
        ("lp", "lp:x:7:"),
        ("kmem", "kmem:x:9:"),
        ("wheel", "wheel:x:10:root"),
        ("audio", "audio:x:18:"),
        ("cdrom", "cdrom:x:19:"),
        ("dialout", "dialout:x:20:"),
        ("video", "video:x:27:"),
        ("input", "input:x:101:"),
        ("users", "users:x:100:"),
    ];

    for (name, line) in defaults {
        let prefix = format!("{name}:");
        if !updated.lines().any(|l| l.starts_with(&prefix)) {
            updated.push_str(line);
            updated.push('\n');
        }
    }

    fs::write(&group_path, updated)
}

/// Configures active Arch Linux geo-mirrors in `/etc/pacman.d/mirrorlist` if a real Arch rootfs is present.
pub fn configure_arch_mirrors_if_present(target_root: &Path) -> io::Result<bool> {
    let mirrorlist = target_root.join("etc/pacman.d/mirrorlist");
    if !mirrorlist.exists() {
        return Ok(false);
    }
    let existing = fs::read_to_string(&mirrorlist).unwrap_or_default();
    let has_active_server = existing
        .lines()
        .map(str::trim)
        .any(|l| l.starts_with("Server =") || l.starts_with("Server="));
    if has_active_server {
        return Ok(true);
    }

    let header = "\
## Enabled by Bifrost Zero-BIOS Transit Engine
Server = https://geo.mirror.pkgbuild.com/$repo/os/$arch
Server = http://geo.mirror.pkgbuild.com/$repo/os/$arch

";
    fs::write(&mirrorlist, format!("{header}{existing}"))?;
    Ok(true)
}

/// Writes an accurate `/etc/os-release` ONLY if the extracted distro did not ship its own genuine identity.
fn ensure_os_release(etc_dir: &Path, target_distro: &str) -> io::Result<()> {
    let os_release_path = etc_dir.join("os-release");
    if let Ok(existing) = fs::read_to_string(&os_release_path) {
        let trimmed = existing.trim();
        // Preserve genuine distro os-release (e.g. extracted Arch Linux, Fedora, Debian, NixOS)
        if !trimmed.is_empty() && !trimmed.contains("Bifrost Asgard") {
            return Ok(());
        }
    }

    let lc = target_distro.to_lowercase();
    let (id, id_like) = if lc.contains("arch") {
        ("arch", "archlinux")
    } else if lc.contains("fedora") {
        ("fedora", "rhel")
    } else if lc.contains("debian") {
        ("debian", "debian")
    } else if lc.contains("nixos") {
        ("nixos", "nixos")
    } else {
        ("alpine", "alpine")
    };

    let content = format!(
        "NAME=\"{target_distro}\"\n\
         PRETTY_NAME=\"{target_distro} (Bifrost Asgard)\"\n\
         ID={id}\n\
         ID_LIKE={id_like}\n\
         HOME_URL=\"https://github.com/Goddev-Archer/Bifroest-WinAway-Migrater\"\n"
    );
    fs::write(&os_release_path, content)
}

/// Generates `/usr/local/sbin/bifrost-init` (safe PID-1 respawn init for bootstrap rootfs images without systemd).
fn generate_bifrost_init_script() -> String {
    "\
#!/bin/sh
export PATH=/usr/local/sbin:/usr/local/bin:/usr/bin:/bin:/sbin:/usr/sbin
export TERM=linux

mount -o remount,rw / 2>/dev/null || true
mkdir -p /dev/pts /dev/shm /run /tmp
chmod 1777 /tmp 2>/dev/null || true
mount -t devpts devpts /dev/pts 2>/dev/null || true
mount -t tmpfs tmpfs /dev/shm 2>/dev/null || true

if [ -f /etc/hostname ]; then
  HN=$(cat /etc/hostname 2>/dev/null | head -n 1)
  [ -n \"$HN\" ] && hostname \"$HN\" 2>/dev/null || true
fi

SHELL_BIN=\"/bin/sh\"
if [ -x /usr/bin/bash ]; then
  SHELL_BIN=\"/usr/bin/bash\"
elif [ -x /bin/bash ]; then
  SHELL_BIN=\"/bin/bash\"
fi

trap 'reboot -f 2>/dev/null || echo b > /proc/sysrq-trigger' INT
trap 'poweroff -f 2>/dev/null || echo o > /proc/sysrq-trigger' PWR TERM HUP

while true; do
  if [ -x /usr/local/bin/busybox ] && [ -c /dev/tty1 ]; then
    /usr/local/bin/busybox setsid /usr/local/bin/busybox cttyhack \"$SHELL_BIN\" -l </dev/tty1 >/dev/tty1 2>&1
  elif [ -c /dev/tty0 ]; then
    \"$SHELL_BIN\" -l </dev/tty0 >/dev/tty0 2>&1
  else
    \"$SHELL_BIN\" -l
  fi
  sleep 1
done
"
    .to_string()
}

/// Generates optional `/usr/local/bin/bifrost-desktop` helper so users freely choose CLI or any DE using their native package manager.
pub fn generate_desktop_helper_script() -> String {
    "\
#!/bin/sh
set -e

echo \"\"
echo \"======================================================================\"
echo \"  BIFROST OPTIONAL DESKTOP & ENVIRONMENT CHOOSER\"
echo \"  Entscheidungsfreiheit: CLI behalten oder Wunsch-Desktop installieren\"
echo \"======================================================================\"

if command -v pacman >/dev/null 2>&1; then
  PM=\"pacman\"
  echo \"  Erkannter Paketmanager: pacman (Arch Linux)\"
elif command -v apk >/dev/null 2>&1; then
  PM=\"apk\"
  echo \"  Erkannter Paketmanager: apk (Alpine / Asgard Minimal)\"
else
  echo \"  Kein unterstuetzter Paketmanager (pacman/apk) gefunden.\"
  exit 1
fi

echo \"\"
echo \"  [1] XFCE4        (Leichtgewichtig, stabil auf Hyper-V & Bare-Metal)\"
echo \"  [2] KDE Plasma   (Moderner Voll-Desktop)\"
echo \"  [3] GNOME        (Klassischer Linux-Desktop)\"
echo \"  [4] Hyprland     (Wayland Tiling Window Manager)\"
echo \"  [0] Abbrechen    (Reine CLI behalten)\"
echo \"\"
printf \"Auswahl [0-4]: \"
read -r CHOICE

case \"$CHOICE\" in
  0|\"\")
    echo \"[Bifrost] Abgebrochen - reine CLI bleibt unveraendert.\"
    exit 0
    ;;
esac

mkdir -p /dev/pts /dev/shm /run/dbus /var/run/dbus
mount -t devpts devpts /dev/pts 2>/dev/null || true
mount -t tmpfs tmpfs /dev/shm 2>/dev/null || true

if [ \"$PM\" = \"pacman\" ]; then
  if [ ! -d /etc/pacman.d/gnupg/private-keys-v1.d ]; then
    echo \"[Bifrost] Initialisiere Arch Linux pacman-Schluesselbund...\"
    pacman-key --init
    pacman-key --populate archlinux
  fi
  pacman -Sy --noconfirm
  case \"$CHOICE\" in
    1)
      pacman -S --noconfirm xorg-server xorg-xinit xf86-video-fbdev xf86-input-libinput xfce4 xfce4-goodies ttf-dejavu dbus
      echo \"[OK] XFCE4 installiert! Starten mit: startx /usr/bin/startxfce4\"
      ;;
    2)
      pacman -S --noconfirm xorg-server xorg-xinit plasma-meta konsole dolphin ttf-dejavu dbus
      echo \"[OK] KDE Plasma installiert! Starten mit: startx /usr/bin/startplasma-x11\"
      ;;
    3)
      pacman -S --noconfirm gnome gnome-terminal gdm ttf-dejavu dbus
      echo \"[OK] GNOME installiert!\"
      ;;
    4)
      pacman -S --noconfirm hyprland kitty waybar wofi ttf-dejavu seatd
      echo \"[OK] Hyprland installiert! Starten mit: Hyprland\"
      ;;
    *)
      echo \"Ungueltige Auswahl.\"
      exit 1
      ;;
  esac
else
  apk update
  case \"$CHOICE\" in
    1)
      apk add --force-overwrite xorg-server xinit eudev xf86-video-fbdev xf86-input-libinput dbus dbus-x11 xfce4 xfce4-terminal font-dejavu
      udevd --daemon 2>/dev/null || true
      udevadm trigger 2>/dev/null || true
      dbus-uuidgen --ensure 2>/dev/null || true
      dbus-daemon --system 2>/dev/null || true
      echo \"[OK] XFCE4 installiert! Starten mit: startx /usr/bin/startxfce4\"
      ;;
    2)
      apk add --force-overwrite xorg-server xinit eudev xf86-video-fbdev xf86-input-libinput dbus dbus-x11 plasma-desktop konsole font-dejavu
      echo \"[OK] KDE Plasma installiert!\"
      ;;
    3)
      apk add --force-overwrite xorg-server xinit eudev dbus dbus-x11 gnome-shell gnome-terminal font-dejavu
      echo \"[OK] GNOME installiert!\"
      ;;
    4)
      apk add --force-overwrite sway foot eudev dbus font-dejavu
      echo \"[OK] Sway (Wayland Tiling) installiert!\"
      ;;
    *)
      echo \"Ungueltige Auswahl.\"
      exit 1
      ;;
  esac
fi
"
    .to_string()
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

    progress_cb("Installiere First-Boot Service & optionale Helfer...", 3, total_steps);
    let (service_unit, postinstall_script) = generate_postinstall_service(manifest);

    let systemd_dir = target_root.join("etc/systemd/system");
    let bin_dir = target_root.join("usr/local/bin");
    let sbin_dir = target_root.join("usr/local/sbin");
    fs::create_dir_all(&systemd_dir)?;
    fs::create_dir_all(&bin_dir)?;
    fs::create_dir_all(&sbin_dir)?;

    fs::write(systemd_dir.join("bifrost-postinstall.service"), service_unit)?;
    let script_path = bin_dir.join("bifrost-postinstall.sh");
    fs::write(&script_path, postinstall_script)?;

    let init_helper_path = sbin_dir.join("bifrost-init");
    fs::write(&init_helper_path, generate_bifrost_init_script())?;

    let desktop_helper_path = bin_dir.join("bifrost-desktop");
    fs::write(&desktop_helper_path, generate_desktop_helper_script())?;

    #[cfg(unix)]
    {
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755))?;
        fs::set_permissions(&init_helper_path, fs::Permissions::from_mode(0o755))?;
        fs::set_permissions(&desktop_helper_path, fs::Permissions::from_mode(0o755))?;
    }

    progress_cb("Konfiguriere Zielsystem (/etc/hostname, Mirrors, Gruppen, Welcome Shell)...", 4, total_steps);
    let etc_dir = target_root.join("etc");
    let profile_d_dir = etc_dir.join("profile.d");
    fs::create_dir_all(&profile_d_dir)?;

    // Ensure /var/run -> ../run symlink so D-Bus & system sockets work seamlessly
    let var_dir = target_root.join("var");
    let _ = fs::create_dir_all(&var_dir);
    let _ = fs::create_dir_all(target_root.join("run"));
    #[cfg(unix)]
    {
        let var_run = var_dir.join("run");
        if var_run.symlink_metadata().is_err() {
            let _ = symlink("../run", &var_run);
        }
    }

    fs::write(etc_dir.join("hostname"), "asgard-linux\n")?;
    ensure_os_release(&etc_dir, &manifest.target_distro)?;
    ensure_standard_groups(&etc_dir)?;
    let _ = configure_arch_mirrors_if_present(target_root);

    // Only write BusyBox inittab when no systemd init is installed
    let has_systemd = target_root.join("usr/lib/systemd/systemd").exists()
        || target_root.join("lib/systemd/systemd").exists();
    if !has_systemd {
        fs::write(
            etc_dir.join("inittab"),
            "::sysinit:/bin/mount -o remount,rw /\n\
             ::sysinit:/bin/mkdir -p /dev/pts /dev/shm /run\n\
             ::sysinit:/bin/mount -t devpts devpts /dev/pts\n\
             ::sysinit:/bin/mount -t tmpfs tmpfs /dev/shm\n\
             ::sysinit:/bin/mount -a\n\
             ::once:/bin/mdev -s\n\
             tty1::respawn:/bin/sh -l\n\
             ::ctrlaltdel:/sbin/reboot\n\
             ::shutdown:/bin/umount -a -r\n",
        )?;
    }

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
         if [ -n \"$BIFROST_WELCOME_SHOWN\" ]; then return 0 2>/dev/null || exit 0; fi\n\
         export BIFROST_WELCOME_SHOWN=1\n\
         clear 2>/dev/null || true\n\
         cat /etc/motd 2>/dev/null || true\n\
         if [ -f /etc/os-release ]; then\n\
           OS_PRETTY=$(grep '^PRETTY_NAME=' /etc/os-release | head -n 1 | cut -d= -f2- | tr -d '\"')\n\
           [ -n \"$OS_PRETTY\" ] && echo \"[Active OS]  $OS_PRETTY\"\n\
         fi\n\
         NET_IP=$(ip -4 -o addr show scope global 2>/dev/null | awk '{{print $2\": \"$4}}' | tr '\\n' ' ')\n\
         if [ -n \"$NET_IP\" ]; then\n\
           echo \"[Network]    $NET_IP\"\n\
         else\n\
           echo \"[Network]    Offline / Kein IPv4-Lease (manuell: udhcpc -i eth0)\"\n\
         fi\n\
         if command -v pacman >/dev/null 2>&1; then\n\
           echo \"[Packages]   pacman bereit (Erststart: pacman-key --init && pacman-key --populate archlinux)\"\n\
         elif command -v apk >/dev/null 2>&1; then\n\
           echo \"[Packages]   apk bereit (z. B. apk update && apk add ...)\"\n\
         fi\n\
         echo \"[Optional]   Desktop installieren? -> bifrost-desktop\"\n\
         echo \"\"\n\
         echo \"[Mounts & Subvolumes]\"\n\
         mount | grep -E 'btrfs|ext4|vfat' || df -h /\n\
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
    if !existing_profile.contains("/etc/profile.d/") && !existing_profile.contains("asgard_welcome.sh") {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_configure_arch_mirrors_and_preserve_os_release() {
        let tmp = std::env::temp_dir().join("bifrost_bootstrap_test_arch");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("etc/pacman.d")).unwrap();
        fs::write(
            tmp.join("etc/pacman.d/mirrorlist"),
            "## Commented default mirrorlist\n#Server = https://example.org/$repo/os/$arch\n",
        )
        .unwrap();
        fs::write(
            tmp.join("etc/os-release"),
            "NAME=\"Arch Linux\"\nPRETTY_NAME=\"Arch Linux\"\nID=arch\n",
        )
        .unwrap();

        assert!(configure_arch_mirrors_if_present(&tmp).unwrap());
        let ml = fs::read_to_string(tmp.join("etc/pacman.d/mirrorlist")).unwrap();
        assert!(ml.contains("Server = https://geo.mirror.pkgbuild.com/$repo/os/$arch"));

        // Must preserve genuine extracted os-release instead of overwriting with fallback
        ensure_os_release(&tmp.join("etc"), "Fedora 41").unwrap();
        let osr = fs::read_to_string(tmp.join("etc/os-release")).unwrap();
        assert_eq!(osr, "NAME=\"Arch Linux\"\nPRETTY_NAME=\"Arch Linux\"\nID=arch\n");

        ensure_standard_groups(&tmp.join("etc")).unwrap();
        let grp = fs::read_to_string(tmp.join("etc/group")).unwrap();
        assert!(grp.contains("video:x:27:"));
        assert!(grp.contains("input:x:101:"));
        assert!(grp.contains("tty:x:5:"));

        let helper = generate_desktop_helper_script();
        assert!(helper.contains("pacman"));
        assert!(helper.contains("apk"));
        assert!(helper.contains("Abbrechen"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_fallback_os_release_respects_selected_distro() {
        let tmp = std::env::temp_dir().join("bifrost_bootstrap_test_fedora");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("etc")).unwrap();

        ensure_os_release(&tmp.join("etc"), "Fedora 41").unwrap();
        let osr = fs::read_to_string(tmp.join("etc/os-release")).unwrap();
        assert!(osr.contains("NAME=\"Fedora 41\""));
        assert!(osr.contains("ID=fedora"));
        assert!(!osr.contains("Arch Linux"));

        let _ = fs::remove_dir_all(&tmp);
    }
}
