use crate::model::DesktopEnvironment;

pub const ARCH_BOOTSTRAP_URLS: &[&str] = &[
    "https://geo.mirror.pkgbuild.com/iso/latest/archlinux-bootstrap-x86_64.tar.zst",
    "http://geo.mirror.pkgbuild.com/iso/latest/archlinux-bootstrap-x86_64.tar.zst",
];

pub const UBUNTU_2604_BASE_URLS: &[&str] = &[
    "https://cdimage.ubuntu.com/ubuntu-base/releases/26.04/release/ubuntu-base-26.04.1-base-amd64.tar.gz",
    "https://cdimage.ubuntu.com/ubuntu-base/releases/26.04/release/ubuntu-base-26.04-base-amd64.tar.gz",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DistroFamily {
    Arch,
    Ubuntu,
    Debian,
    Fedora,
    Alpine,
    NixOs,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tier1DistroSpec {
    pub family: DistroFamily,
    pub distro_label: String,
    pub slug: &'static str,
    pub archive_filename: &'static str,
    pub download_urls: Vec<&'static str>,
    pub use_embedded_asgard: bool,
    pub min_expected_bytes: u64,
    pub native_pm: &'static str,
    pub kernel_and_core_pkgs: Vec<&'static str>,
}

/// Resolves distribution metadata, rootfs archive preferences, native package manager, and core kernel packages.
pub fn resolve_tier1_distro(target_distro: &str) -> Tier1DistroSpec {
    let lc = target_distro.to_lowercase();
    if lc.contains("arch") {
        Tier1DistroSpec {
            family: DistroFamily::Arch,
            distro_label: target_distro.to_string(),
            slug: "arch",
            archive_filename: "bifrost_rootfs.tar.zst",
            download_urls: ARCH_BOOTSTRAP_URLS.to_vec(),
            use_embedded_asgard: false,
            min_expected_bytes: 10 * 1024 * 1024,
            native_pm: "pacman",
            kernel_and_core_pkgs: vec![
                "base",
                "linux",
                "linux-firmware",
                "btrfs-progs",
                "networkmanager",
                "sudo",
            ],
        }
    } else if lc.contains("ubuntu") {
        Tier1DistroSpec {
            family: DistroFamily::Ubuntu,
            distro_label: target_distro.to_string(),
            slug: "ubuntu2604",
            archive_filename: "bifrost_rootfs.tar.gz",
            download_urls: UBUNTU_2604_BASE_URLS.to_vec(),
            use_embedded_asgard: false,
            min_expected_bytes: 10 * 1024 * 1024,
            native_pm: "apt",
            kernel_and_core_pkgs: vec![
                "linux-image-generic",
                "initramfs-tools",
                "systemd-sysv",
                "btrfs-progs",
                "network-manager",
                "sudo",
            ],
        }
    } else if lc.contains("debian") {
        Tier1DistroSpec {
            family: DistroFamily::Debian,
            distro_label: target_distro.to_string(),
            slug: "debian12",
            archive_filename: "bifrost_rootfs.tar.xz",
            download_urls: Vec::new(),
            use_embedded_asgard: true,
            min_expected_bytes: 0,
            native_pm: "apt",
            kernel_and_core_pkgs: vec![
                "linux-image-amd64",
                "initramfs-tools",
                "systemd-sysv",
                "btrfs-progs",
                "network-manager",
                "sudo",
            ],
        }
    } else if lc.contains("fedora") {
        Tier1DistroSpec {
            family: DistroFamily::Fedora,
            distro_label: target_distro.to_string(),
            slug: "fedora41",
            archive_filename: "bifrost_rootfs.tar.xz",
            download_urls: Vec::new(),
            use_embedded_asgard: true,
            min_expected_bytes: 0,
            native_pm: "dnf",
            kernel_and_core_pkgs: vec![
                "kernel",
                "dracut",
                "systemd",
                "btrfs-progs",
                "NetworkManager",
                "sudo",
            ],
        }
    } else if lc.contains("alpine") || lc.contains("asgard") {
        Tier1DistroSpec {
            family: DistroFamily::Alpine,
            distro_label: target_distro.to_string(),
            slug: "alpine",
            archive_filename: "bifrost_rootfs.tar.zst",
            download_urls: Vec::new(),
            use_embedded_asgard: true,
            min_expected_bytes: 0,
            native_pm: "apk",
            kernel_and_core_pkgs: vec!["linux-virt", "btrfs-progs", "networkmanager", "sudo"],
        }
    } else if lc.contains("nix") {
        Tier1DistroSpec {
            family: DistroFamily::NixOs,
            distro_label: target_distro.to_string(),
            slug: "nixos",
            archive_filename: "bifrost_rootfs.tar.xz",
            download_urls: Vec::new(),
            use_embedded_asgard: true,
            min_expected_bytes: 0,
            native_pm: "nix",
            kernel_and_core_pkgs: Vec::new(),
        }
    } else {
        Tier1DistroSpec {
            family: DistroFamily::Custom,
            distro_label: target_distro.to_string(),
            slug: "custom",
            archive_filename: "bifrost_rootfs.tar.zst",
            download_urls: Vec::new(),
            use_embedded_asgard: true,
            min_expected_bytes: 0,
            native_pm: "apk",
            kernel_and_core_pkgs: vec!["btrfs-progs"],
        }
    }
}

/// Returns `(packages, optional_display_manager_service)` for a `(DistroFamily, DesktopEnvironment)` pair.
pub fn desktop_packages_for(
    family: DistroFamily,
    de: DesktopEnvironment,
) -> (Vec<&'static str>, Option<&'static str>) {
    if de == DesktopEnvironment::CliOnly {
        return (Vec::new(), None);
    }

    match (family, de) {
        (DistroFamily::Arch, DesktopEnvironment::Xfce4) => (
            vec![
                "xorg-server",
                "xf86-video-fbdev",
                "xf86-input-libinput",
                "xfce4",
                "xfce4-goodies",
                "lightdm",
                "lightdm-gtk-greeter",
                "ttf-dejavu",
            ],
            Some("lightdm"),
        ),
        (DistroFamily::Arch, DesktopEnvironment::KdePlasma) => (
            vec![
                "xorg-server",
                "plasma-meta",
                "konsole",
                "dolphin",
                "sddm",
                "ttf-dejavu",
            ],
            Some("sddm"),
        ),
        (DistroFamily::Arch, DesktopEnvironment::Gnome) => (
            vec!["gnome", "gnome-terminal", "gdm", "ttf-dejavu"],
            Some("gdm"),
        ),
        (DistroFamily::Arch, DesktopEnvironment::Hyprland) => (
            vec!["hyprland", "kitty", "waybar", "wofi", "seatd", "ttf-dejavu"],
            None,
        ),

        (DistroFamily::Ubuntu | DistroFamily::Debian, DesktopEnvironment::Xfce4) => (
            vec![
                "xorg",
                "xfce4",
                "xfce4-terminal",
                "lightdm",
                "lightdm-gtk-greeter",
                "dbus-x11",
                "fonts-dejavu",
            ],
            Some("lightdm"),
        ),
        (DistroFamily::Ubuntu | DistroFamily::Debian, DesktopEnvironment::KdePlasma) => (
            vec!["kde-plasma-desktop", "konsole", "sddm", "dbus-x11"],
            Some("sddm"),
        ),
        (DistroFamily::Ubuntu | DistroFamily::Debian, DesktopEnvironment::Gnome) => {
            let pkg = if family == DistroFamily::Ubuntu {
                "ubuntu-desktop-minimal"
            } else {
                "task-gnome-desktop"
            };
            (vec![pkg, "gnome-terminal", "gdm3"], Some("gdm3"))
        }
        (DistroFamily::Ubuntu | DistroFamily::Debian, DesktopEnvironment::Hyprland) => (
            vec!["sway", "foot", "waybar", "wofi", "dbus-x11", "fonts-dejavu"],
            None,
        ),

        (DistroFamily::Fedora, DesktopEnvironment::Xfce4) => (
            vec!["@xfce-desktop-environment", "lightdm", "dejavu-sans-fonts"],
            Some("lightdm"),
        ),
        (DistroFamily::Fedora, DesktopEnvironment::KdePlasma) => (
            vec!["@kde-desktop-environment", "sddm", "konsole"],
            Some("sddm"),
        ),
        (DistroFamily::Fedora, DesktopEnvironment::Gnome) => (
            vec!["@workstation-product-environment", "gdm"],
            Some("gdm"),
        ),
        (DistroFamily::Fedora, DesktopEnvironment::Hyprland) => (
            vec!["hyprland", "kitty", "waybar", "wofi", "dejavu-sans-fonts"],
            None,
        ),

        (DistroFamily::Alpine, DesktopEnvironment::Xfce4) => (
            vec![
                "xorg-server",
                "xinit",
                "eudev",
                "xf86-video-fbdev",
                "xf86-input-libinput",
                "dbus",
                "dbus-x11",
                "xfce4",
                "xfce4-terminal",
                "font-dejavu",
            ],
            None,
        ),
        (DistroFamily::Alpine, DesktopEnvironment::KdePlasma) => (
            vec![
                "xorg-server",
                "xinit",
                "eudev",
                "xf86-video-fbdev",
                "xf86-input-libinput",
                "dbus",
                "dbus-x11",
                "plasma-desktop",
                "konsole",
                "font-dejavu",
            ],
            None,
        ),
        (DistroFamily::Alpine, DesktopEnvironment::Gnome) => (
            vec![
                "xorg-server",
                "xinit",
                "eudev",
                "dbus",
                "dbus-x11",
                "gnome-shell",
                "gnome-terminal",
                "font-dejavu",
            ],
            None,
        ),
        (DistroFamily::Alpine, DesktopEnvironment::Hyprland) => (
            vec!["sway", "foot", "eudev", "dbus", "font-dejavu"],
            None,
        ),

        _ => (Vec::new(), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tier1_distro_specs_and_desktop_matrices() {
        let arch = resolve_tier1_distro("Arch Linux");
        assert_eq!(arch.family, DistroFamily::Arch);
        assert_eq!(arch.slug, "arch");
        assert_eq!(arch.native_pm, "pacman");
        assert!(arch.kernel_and_core_pkgs.contains(&"linux"));

        let ubuntu = resolve_tier1_distro("Ubuntu 26.04 LTS");
        assert_eq!(ubuntu.family, DistroFamily::Ubuntu);
        assert_eq!(ubuntu.slug, "ubuntu2604");
        assert_eq!(ubuntu.archive_filename, "bifrost_rootfs.tar.gz");
        assert_eq!(ubuntu.native_pm, "apt");
        assert!(ubuntu.kernel_and_core_pkgs.contains(&"linux-image-generic"));

        let debian = resolve_tier1_distro("Debian 12");
        assert_eq!(debian.family, DistroFamily::Debian);
        assert_eq!(debian.archive_filename, "bifrost_rootfs.tar.xz");
        assert_eq!(debian.native_pm, "apt");

        let fedora = resolve_tier1_distro("Fedora 41");
        assert_eq!(fedora.family, DistroFamily::Fedora);
        assert_eq!(fedora.archive_filename, "bifrost_rootfs.tar.xz");
        assert_eq!(fedora.native_pm, "dnf");

        let alpine = resolve_tier1_distro("Alpine Linux (Asgard Minimal)");
        assert_eq!(alpine.family, DistroFamily::Alpine);
        assert_eq!(alpine.native_pm, "apk");
        assert!(alpine.use_embedded_asgard);

        let (cli_pkgs, cli_dm) = desktop_packages_for(DistroFamily::Arch, DesktopEnvironment::CliOnly);
        assert!(cli_pkgs.is_empty());
        assert!(cli_dm.is_none());

        let (kde_pkgs, kde_dm) = desktop_packages_for(DistroFamily::Arch, DesktopEnvironment::KdePlasma);
        assert!(kde_pkgs.contains(&"plasma-meta"));
        assert_eq!(kde_dm, Some("sddm"));

        let (ub_gnome, ub_dm) = desktop_packages_for(DistroFamily::Ubuntu, DesktopEnvironment::Gnome);
        assert!(ub_gnome.contains(&"ubuntu-desktop-minimal"));
        assert_eq!(ub_dm, Some("gdm3"));
    }
}
