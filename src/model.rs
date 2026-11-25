use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CandidateCategory {
    Documents,
    GitRepositories,
    Credentials,
    ApplicationConfig,
    SystemExcluded,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateItem {
    pub id: String,
    pub path: PathBuf,
    pub display_name: String,
    pub description: String,
    pub total_bytes: u64,
    pub size_human: String,
    pub category: CandidateCategory,
    pub selected: bool,
    pub item_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitRepoInfo {
    pub path: PathBuf,
    pub name: String,
    pub current_branch: String,
    pub is_dirty: bool,
    pub uncommitted_files_count: usize,
    #[serde(default = "default_true")]
    pub selected: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredSoftware {
    pub name: String,
    pub id: String,
    pub source: String,
    pub flatpak_id: Option<String>,
    pub native_pkg: Option<String>,
    pub will_reinstall: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DesktopEnvironment {
    #[default]
    CliOnly,
    Xfce4,
    KdePlasma,
    Gnome,
    Hyprland,
}

impl DesktopEnvironment {
    pub const ALL: &'static [DesktopEnvironment] = &[
        DesktopEnvironment::CliOnly,
        DesktopEnvironment::Xfce4,
        DesktopEnvironment::KdePlasma,
        DesktopEnvironment::Gnome,
        DesktopEnvironment::Hyprland,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            DesktopEnvironment::CliOnly => "CLI Only (Pure Konsole / Minimal)",
            DesktopEnvironment::Xfce4 => "XFCE4 (Leicht & Stabil)",
            DesktopEnvironment::KdePlasma => "KDE Plasma (Moderner Voll-Desktop)",
            DesktopEnvironment::Gnome => "GNOME (Klassisch)",
            DesktopEnvironment::Hyprland => "Hyprland (Wayland Tiling WM)",
        }
    }

    pub fn slug(&self) -> &'static str {
        match self {
            DesktopEnvironment::CliOnly => "cli",
            DesktopEnvironment::Xfce4 => "xfce4",
            DesktopEnvironment::KdePlasma => "plasma",
            DesktopEnvironment::Gnome => "gnome",
            DesktopEnvironment::Hyprland => "hyprland",
        }
    }

    pub fn from_slug(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "cli" | "none" | "clionly" => Some(DesktopEnvironment::CliOnly),
            "xfce" | "xfce4" => Some(DesktopEnvironment::Xfce4),
            "kde" | "plasma" | "kdeplasma" => Some(DesktopEnvironment::KdePlasma),
            "gnome" => Some(DesktopEnvironment::Gnome),
            "hyprland" => Some(DesktopEnvironment::Hyprland),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LinuxMigrationMode {
    #[default]
    BtrfsSubvolumeHop,
    RamfsFullTransit,
}

impl LinuxMigrationMode {
    pub fn label(&self) -> &'static str {
        match self {
            LinuxMigrationMode::BtrfsSubvolumeHop => {
                "Btrfs Subvolume-Hop (Zero-Copy @home, Instant-Wechsel)"
            }
            LinuxMigrationMode::RamfsFullTransit => {
                "RAMFS Full-Transit (bifrost-boot.efi, Wipe/Dateisystem-Wechsel)"
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum RollbackPolicy {
    #[default]
    KeepRollbackSubvolume,
    DirectPurgeOldRoot,
}

impl RollbackPolicy {
    pub fn label(&self) -> &'static str {
        match self {
            RollbackPolicy::KeepRollbackSubvolume => {
                "Rollback behalten (@root_old + Boot-Eintrag, loeschbar via --cleanup)"
            }
            RollbackPolicy::DirectPurgeOldRoot => {
                "Direct Purge (Altes @root nach Boot-Verifikation entfernen)"
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskScanInfo {
    pub drive_letter: String,
    pub mount_point: String,
    pub file_system: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub is_bitlocker_active: bool,
    pub bitlocker_status: String,
    #[serde(default)]
    pub is_btrfs_subvol_capable: bool,
    #[serde(default)]
    pub root_subvol_name: Option<String>,
    #[serde(default)]
    pub is_luks_active: bool,
    #[serde(default)]
    pub luks_mapper_name: Option<String>,
    #[serde(default)]
    pub luks_underlying_uuid: Option<String>,
    #[serde(default)]
    pub esp_mount_point: Option<String>,
    #[serde(default)]
    pub source_distro_pretty: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanResult {
    pub user_profile_dir: PathBuf,
    pub disk_info: Option<DiskScanInfo>,
    pub candidates: Vec<CandidateItem>,
    pub git_repos: Vec<GitRepoInfo>,
    pub software: Vec<DiscoveredSoftware>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationManifest {
    pub schema_version: String,
    pub created_at_utc: u64,
    pub source_os: String,
    pub target_distro: String,
    pub target_filesystem: String,
    #[serde(default = "default_true")]
    pub clean_install: bool,
    pub user_profile: PathBuf,
    pub target_username: String,
    pub selected_software: Vec<DiscoveredSoftware>,
    pub git_repositories: Vec<GitRepoInfo>,
    pub archived_categories: Vec<String>,
    pub total_uncompressed_bytes: u64,
    pub total_files: usize,
    pub archive_sha256: Option<String>,
    #[serde(default)]
    pub desktop_env: Option<DesktopEnvironment>,
    #[serde(default)]
    pub linux_migration_mode: Option<LinuxMigrationMode>,
    #[serde(default)]
    pub rollback_policy: Option<RollbackPolicy>,
    #[serde(default)]
    pub source_uid: Option<u32>,
    #[serde(default)]
    pub source_gid: Option<u32>,
    #[serde(default)]
    pub luks_uuid: Option<String>,
}

pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.1} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}
