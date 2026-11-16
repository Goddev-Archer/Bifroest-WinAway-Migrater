pub mod git;
pub mod software;
pub mod system;

use std::{
    env,
    fs,
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

use crate::model::{
    format_bytes, CandidateCategory, CandidateItem, ScanResult,
};

pub struct Scanner;

impl Scanner {
    pub fn scan_all() -> ScanResult {
        Self::scan_with_progress(|_, _, _| {})
    }

    pub fn scan_with_progress<F>(mut progress_cb: F) -> ScanResult
    where
        F: FnMut(&str, usize, usize),
    {
        let user_profile = get_user_profile_dir();

        // 1. System & BitLocker Scan
        progress_cb("Prüfe NVMe-Partitionen & BitLocker-Status...", 1, 4);
        let disk_info = system::scan_system_disk();

        // 2. Dateisystem-Kandidaten (Dokumente, Keys, VS Code)
        progress_cb("Berechne Größen von Dokumenten, SSH & VS Code...", 2, 4);
        let candidates = scan_filesystem_candidates(&user_profile);

        // 3. Git Repositories in bekannten Entwickler-Verzeichnissen
        progress_cb("Suche Git-Repositories in Entwickler-Pfaden...", 3, 4);
        let mut dev_roots = vec![
            user_profile.join("Projects"),
            user_profile.join("source").join("repos"),
            user_profile.join("dev"),
            user_profile.join("workspace"),
        ];
        #[cfg(target_os = "windows")]
        dev_roots.push(PathBuf::from("C:\\dev"));
        let git_repos = git::scan_git_repos(&dev_roots, 3);

        // 4. Installierte Software via Winget & Registry
        progress_cb("Lese Winget-Software-Datenbank aus...", 4, 4);
        let software = software::scan_installed_software(&user_profile);

        ScanResult {
            user_profile_dir: user_profile,
            disk_info,
            candidates,
            git_repos,
            software,
        }
    }
}

fn get_user_profile_dir() -> PathBuf {
    if let Ok(profile) = env::var("USERPROFILE") {
        return PathBuf::from(profile);
    }
    if let Ok(home) = env::var("HOME") {
        return PathBuf::from(home);
    }
    PathBuf::from("C:\\Users\\Default")
}

fn scan_filesystem_candidates(user_profile: &Path) -> Vec<CandidateItem> {
    let mut list = Vec::new();

    // Documents
    let docs = user_profile.join("Documents");
    if docs.exists() {
        let (bytes, count) = calculate_dir_size(&docs, 4);
        list.push(CandidateItem {
            id: "docs".to_string(),
            path: docs.clone(),
            display_name: docs.to_string_lossy().into_owned(),
            description: "Dokumente & persönliche Arbeitsdateien".to_string(),
            total_bytes: bytes,
            size_human: format_bytes(bytes),
            category: CandidateCategory::Documents,
            selected: true,
            item_count: count,
        });
    }

    // SSH Keys
    let ssh = user_profile.join(".ssh");
    if ssh.exists() {
        let (bytes, count) = calculate_dir_size(&ssh, 2);
        list.push(CandidateItem {
            id: "ssh".to_string(),
            path: ssh.clone(),
            display_name: ssh.to_string_lossy().into_owned(),
            description: "SSH-Schlüssel (id_ed25519, id_rsa, config)".to_string(),
            total_bytes: bytes,
            size_human: format_bytes(bytes),
            category: CandidateCategory::Credentials,
            selected: true,
            item_count: count,
        });
    }

    // GPG Keys
    let gnupg = user_profile.join(".gnupg");
    if gnupg.exists() {
        let (bytes, count) = calculate_dir_size(&gnupg, 2);
        list.push(CandidateItem {
            id: "gnupg".to_string(),
            path: gnupg.clone(),
            display_name: gnupg.to_string_lossy().into_owned(),
            description: "GPG-Keyrings & Vertrauensdatenbank".to_string(),
            total_bytes: bytes,
            size_human: format_bytes(bytes),
            category: CandidateCategory::Credentials,
            selected: true,
            item_count: count,
        });
    }

    // VS Code User Config (Windows AppData oder Linux ~/.config)
    let vscode_win = user_profile.join("AppData").join("Roaming").join("Code").join("User");
    let vscode_linux = user_profile.join(".config").join("Code").join("User");
    let vscode_target = if vscode_win.exists() {
        Some((vscode_win, "%APPDATA%\\Code\\User"))
    } else if vscode_linux.exists() {
        Some((vscode_linux, "~/.config/Code/User"))
    } else {
        None
    };

    if let Some((vscode, display)) = vscode_target {
        let (bytes, count) = calculate_dir_size(&vscode, 3);
        list.push(CandidateItem {
            id: "vscode".to_string(),
            path: vscode,
            display_name: display.to_string(),
            description: "VS Code settings.json, keybindings, snippets".to_string(),
            total_bytes: bytes,
            size_human: format_bytes(bytes),
            category: CandidateCategory::ApplicationConfig,
            selected: true,
            item_count: count,
        });
    }

    // Downloads (Standardmäßig abgewählt)
    let downloads = user_profile.join("Downloads");
    if downloads.exists() {
        let (bytes, count) = calculate_dir_size(&downloads, 2);
        list.push(CandidateItem {
            id: "downloads".to_string(),
            path: downloads.clone(),
            display_name: downloads.to_string_lossy().into_owned(),
            description: "Temporäre Downloads (Empfohlen: Ausschließen)".to_string(),
            total_bytes: bytes,
            size_human: format_bytes(bytes),
            category: CandidateCategory::SystemExcluded,
            selected: false,
            item_count: count,
        });
    }

    // AppData Local (Standardmäßig abgewählt)
    let appdata_local = user_profile.join("AppData").join("Local");
    if appdata_local.exists() {
        list.push(CandidateItem {
            id: "appdata_local".to_string(),
            path: appdata_local.clone(),
            display_name: "%LOCALAPPDATA% (Windows Temp/Caches)".to_string(),
            description: "Temporäre Caches & Logs (Ausschließen)".to_string(),
            total_bytes: 0,
            size_human: "Cache (Excluded)".to_string(),
            category: CandidateCategory::SystemExcluded,
            selected: false,
            item_count: 0,
        });
    }

    list
}

fn calculate_dir_size(dir: &Path, max_depth: usize) -> (u64, usize) {
    let mut total_bytes = 0;
    let mut file_count = 0;

    let it = WalkDir::new(dir).max_depth(max_depth).into_iter();
    for entry in it.filter_map(|e| e.ok()) {
        if entry.file_type().is_file() {
            if let Ok(meta) = fs::metadata(entry.path()) {
                total_bytes += meta.len();
                file_count += 1;
            }
        }
    }

    (total_bytes, file_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scanner_real_environment() {
        let result = Scanner::scan_all();
        println!("User Profile: {:?}", result.user_profile_dir);
        if let Some(disk) = &result.disk_info {
            println!("Disk: {} ({}), Total: {} GB, Avail: {} GB, BitLocker: {}", 
                disk.mount_point, disk.file_system,
                disk.total_bytes / (1024*1024*1024),
                disk.available_bytes / (1024*1024*1024),
                disk.bitlocker_status);
        }
        println!("Found {} candidates:", result.candidates.len());
        for c in &result.candidates {
            println!(" - {} ({})", c.display_name, c.size_human);
        }
        println!("Found {} Git Repos:", result.git_repos.len());
        for g in &result.git_repos {
            println!(" - {} [branch: {}] dirty={}", g.name, g.current_branch, g.is_dirty);
        }
        println!("Found {} Software packages:", result.software.len());
        for s in &result.software {
            println!(" - {} (flatpak: {:?})", s.name, s.flatpak_id);
        }
    }
}
