use std::{
    fs::{self, File},
    io::{self, BufReader, Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use sha2::{Digest, Sha256};
use tar::Builder;
use walkdir::WalkDir;
use zstd::Encoder;

use crate::model::{
    CandidateCategory, CandidateItem, DiscoveredSoftware, GitRepoInfo, MigrationManifest,
};

pub struct PackProgress {
    pub current_file: String,
    pub files_packed: usize,
    pub bytes_processed: u64,
}

pub struct PackResult {
    pub archive_path: PathBuf,
    pub total_files: usize,
    pub uncompressed_bytes: u64,
    pub compressed_bytes: u64,
    pub sha256_checksum: String,
    pub manifest: MigrationManifest,
}

pub fn create_blackbox_archive<F>(
    dest_path: &Path,
    user_profile: &Path,
    candidates: &[CandidateItem],
    git_repos: &[GitRepoInfo],
    software: &[DiscoveredSoftware],
    target_distro: &str,
    target_filesystem: &str,
    progress_cb: F,
) -> io::Result<PackResult>
where
    F: FnMut(PackProgress),
{
    create_blackbox_archive_with_mode(
        dest_path,
        user_profile,
        candidates,
        git_repos,
        software,
        target_distro,
        target_filesystem,
        true,
        progress_cb,
    )
}

pub fn create_blackbox_archive_with_mode<F>(
    dest_path: &Path,
    user_profile: &Path,
    candidates: &[CandidateItem],
    git_repos: &[GitRepoInfo],
    software: &[DiscoveredSoftware],
    target_distro: &str,
    target_filesystem: &str,
    clean_install: bool,
    mut progress_cb: F,
) -> io::Result<PackResult>
where
    F: FnMut(PackProgress),
{
    if let Some(parent) = dest_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let file = File::create(dest_path)?;
    // Zstandard Compression Level 3 (Sehr schnell & hohe Kompressionsrate)
    let encoder = Encoder::new(file, 3)?;
    let mut tar = Builder::new(encoder);

    let mut total_files = 0;
    let mut uncompressed_bytes = 0;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let target_username = user_profile
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "user".to_string());

    // 1. Packe die ausgewählten Kandidaten (Dokumente, Keys, VS Code)
    let mut categories_packed = Vec::new();

    for candidate in candidates {
        if !candidate.selected || !candidate.path.exists() {
            continue;
        }

        let category_name = format!("{:?}", candidate.category);
        if !categories_packed.contains(&category_name) {
            categories_packed.push(category_name);
        }

        // Bestimme das relative Zielverzeichnis in der Blackbox (XDG & Linux konform!)
        let archive_base_prefix = if candidate.id == "xdg_config" {
            "home/.config".to_string()
        } else if candidate.id == "flatpak_data" {
            "home/.var/app".to_string()
        } else if candidate.id == "nm_wifi" {
            "etc/NetworkManager/system-connections".to_string()
        } else if candidate.id == "gh_cli" {
            "home/.config/gh".to_string()
        } else if candidate.id == "gitconfig" {
            "home/.gitconfig".to_string()
        } else if candidate.id == "git_credentials" {
            "home/.git-credentials".to_string()
        } else if candidate.id == "vscode" {
            // Pre-Mapping von Windows %APPDATA%\Code\User nach ~/.config/Code/User!
            "home/.config/Code/User".to_string()
        } else {
            match candidate.category {
                CandidateCategory::Credentials => {
                    let folder = candidate.path.file_name().unwrap_or_default().to_string_lossy();
                    format!("home/{folder}")
                }
                CandidateCategory::ApplicationConfig => {
                    let folder = candidate.path.file_name().unwrap_or_default().to_string_lossy();
                    format!("home/.config/{folder}")
                }
                CandidateCategory::Documents => "home/Documents".to_string(),
                _ => {
                    let folder = candidate.path.file_name().unwrap_or_default().to_string_lossy();
                    format!("home/{folder}")
                }
            }
        };

        pack_directory(
            &mut tar,
            &candidate.path,
            &archive_base_prefix,
            &mut total_files,
            &mut uncompressed_bytes,
            &mut progress_cb,
        )?;
    }

    // 2. Packe die ausgewählten Git Repositories
    for repo in git_repos {
        if !repo.selected || !repo.path.exists() {
            continue;
        }

        let repo_archive_prefix = format!("home/Projects/{}", repo.name);
        pack_directory(
            &mut tar,
            &repo.path,
            &repo_archive_prefix,
            &mut total_files,
            &mut uncompressed_bytes,
            &mut progress_cb,
        )?;
    }

    // 3. Erstelle das Manifest und schreibe es als erste Metadatei ins Archiv
    let selected_software: Vec<DiscoveredSoftware> = software
        .iter()
        .filter(|s| s.will_reinstall)
        .cloned()
        .collect();

    let selected_repos: Vec<GitRepoInfo> = git_repos
        .iter()
        .filter(|r| r.selected)
        .cloned()
        .collect();

    let source_os = if cfg!(target_os = "linux") {
        "Linux".to_string()
    } else {
        "Windows 11".to_string()
    };

    let manifest = MigrationManifest {
        schema_version: "1.0.0".to_string(),
        created_at_utc: now,
        source_os,
        target_distro: target_distro.to_string(),
        target_filesystem: target_filesystem.to_string(),
        clean_install,
        user_profile: user_profile.to_path_buf(),
        target_username,
        selected_software,
        git_repositories: selected_repos,
        archived_categories: categories_packed,
        total_uncompressed_bytes: uncompressed_bytes,
        total_files,
        archive_sha256: None, // Wird nach dem Schließen berechnet
        desktop_env: None,
        linux_migration_mode: None,
        rollback_policy: None,
        source_uid: None,
        source_gid: None,
        luks_uuid: None,
    };

    let manifest_json = serde_json::to_vec_pretty(&manifest)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    let mut header = tar::Header::new_gnu();
    header.set_size(manifest_json.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    tar.append_data(&mut header, "bifrost_manifest.json", &manifest_json[..])?;

    // Tar abschließen und Zstd-Stream flushen
    let zstd_encoder = tar.into_inner()?;
    let mut file = zstd_encoder.finish()?;
    file.flush()?;

    let compressed_bytes = fs::metadata(dest_path)?.len();

    // 4. Berechne SHA-256 Prüfsumme des fertigen Blackbox-Archivs
    let checksum = calculate_file_sha256(dest_path)?;

    Ok(PackResult {
        archive_path: dest_path.to_path_buf(),
        total_files,
        uncompressed_bytes,
        compressed_bytes,
        sha256_checksum: checksum,
        manifest,
    })
}

fn pack_directory<W: Write, F>(
    tar: &mut Builder<W>,
    src_dir: &Path,
    archive_prefix: &str,
    total_files: &mut usize,
    total_bytes: &mut u64,
    progress_cb: &mut F,
) -> io::Result<()>
where
    F: FnMut(PackProgress),
{
    let walker = WalkDir::new(src_dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            // Ignoriere typische Riesen-Cache-Ordner in Projekten & XDG Configs
            if entry.file_type().is_dir() {
                if name == "node_modules"
                    || name == "target"
                    || name == ".cargo"
                    || name == ".cache"
                    || name == "Cache"
                    || name == "CachedData"
                    || name == "GPUCache"
                    || name == "Code Cache"
                {
                    return false;
                }
            }
            true
        });

    for entry in walker.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_file() {
            if let Ok(rel) = path.strip_prefix(src_dir) {
                let archive_path = if rel.as_os_str().is_empty() {
                    archive_prefix.to_string()
                } else {
                    format!("{}/{}", archive_prefix, rel.to_string_lossy().replace('\\', "/"))
                };

                if let Ok(mut f) = File::open(path) {
                    let size = f.metadata()?.len();
                    *total_files += 1;
                    *total_bytes += size;

                    progress_cb(PackProgress {
                        current_file: archive_path.clone(),
                        files_packed: *total_files,
                        bytes_processed: *total_bytes,
                    });

                    let mut header = tar::Header::new_gnu();
                    header.set_size(size);
                    // Standard POSIX Permissions für User-Dateien
                    header.set_mode(0o644);
                    header.set_cksum();

                    let _ = tar.append_data(&mut header, &archive_path, &mut f);
                }
            }
        }
    }

    Ok(())
}

fn calculate_file_sha256(path: &Path) -> io::Result<String> {
    let f = File::open(path)?;
    let mut reader = BufReader::new(f);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];

    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }

    let hash = hasher.finalize();
    let hex_str: String = hash.iter().map(|b| format!("{:02x}", b)).collect();
    Ok(hex_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tar::Archive;
    use zstd::Decoder;

    #[test]
    fn test_blackbox_pack_and_inspect() {
        let temp_dir = std::env::temp_dir().join("bifrost_test_staging");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        // Erstelle Dummy-Dateien
        let dummy_docs = temp_dir.join("test_docs");
        fs::create_dir_all(&dummy_docs).unwrap();
        fs::write(dummy_docs.join("resume.pdf"), "Fake PDF Content 123456").unwrap();
        fs::write(dummy_docs.join("notes.txt"), "Important project notes").unwrap();

        let candidates = vec![CandidateItem {
            id: "docs".to_string(),
            path: dummy_docs.clone(),
            display_name: "Test Documents".to_string(),
            description: "Docs".to_string(),
            total_bytes: 40,
            size_human: "40 B".to_string(),
            category: CandidateCategory::Documents,
            selected: true,
            item_count: 2,
        }];

        let repos = vec![];
        let software = vec![];

        let archive_target = temp_dir.join("blackbox.tar.zst");

        let result = create_blackbox_archive(
            &archive_target,
            &dummy_docs,
            &candidates,
            &repos,
            &software,
            "Arch Linux",
            "btrfs",
            |p| {
                println!("Packed file: {} (total: {})", p.current_file, p.files_packed);
            },
        ).expect("Packaging failed");

        assert!(archive_target.exists());
        assert!(result.total_files >= 2);
        assert!(!result.sha256_checksum.is_empty());
        println!("Archive created successfully: {:?}", archive_target);
        println!("SHA256: {}", result.sha256_checksum);

        // Dekomprimiere und verifiziere den Inhalt
        let archive_bytes = fs::read(&archive_target).unwrap();
        let decoder = Decoder::new(Cursor::new(archive_bytes)).unwrap();
        let mut tar_archive = Archive::new(decoder);

        let mut found_manifest = false;
        let mut found_resume = false;

        for entry in tar_archive.entries().unwrap() {
            let entry = entry.unwrap();
            let path = entry.path().unwrap().to_string_lossy().into_owned();
            println!("Entry inside Blackbox: {}", path);
            if path == "bifrost_manifest.json" {
                found_manifest = true;
            }
            if path.contains("resume.pdf") {
                found_resume = true;
            }
        }

        assert!(found_manifest, "Manifest missing in Blackbox!");
        assert!(found_resume, "resume.pdf missing in Blackbox!");

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }
}
