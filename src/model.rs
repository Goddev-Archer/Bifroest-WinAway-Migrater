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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskScanInfo {
    pub drive_letter: String,
    pub mount_point: String,
    pub file_system: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub is_bitlocker_active: bool,
    pub bitlocker_status: String,
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
    pub user_profile: PathBuf,
    pub target_username: String,
    pub selected_software: Vec<DiscoveredSoftware>,
    pub git_repositories: Vec<GitRepoInfo>,
    pub archived_categories: Vec<String>,
    pub total_uncompressed_bytes: u64,
    pub total_files: usize,
    pub archive_sha256: Option<String>,
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
