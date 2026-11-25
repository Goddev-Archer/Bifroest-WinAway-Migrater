use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{self, BufReader, Read},
    path::{Component, Path, PathBuf},
};
use tar::Archive;
use zstd::Decoder;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[cfg(target_os = "linux")]
use std::process::Command;

use crate::model::MigrationManifest;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InjectionReport {
    pub manifest: MigrationManifest,
    pub files_restored: usize,
    pub bytes_restored: u64,
    pub sha256_verified: bool,
    pub computed_sha256: String,
    pub permission_fixes: Vec<(PathBuf, u32)>,
    pub target_home_dir: PathBuf,
}

/// Computes the strict POSIX mode (`0o700`, `0o600`, `0o755`, `0o644`) for restored user files and credential folders.
pub fn determine_posix_mode(rel_path_in_home: &str, is_dir: bool) -> u32 {
    let mut norm = rel_path_in_home.replace('\\', "/");
    while norm.starts_with("./") {
        norm = norm[2..].to_string();
    }
    let norm = norm.trim_start_matches('/').trim_end_matches('/');

    if is_dir {
        if norm == ".ssh"
            || norm.starts_with(".ssh/")
            || norm == ".gnupg"
            || norm.starts_with(".gnupg/")
        {
            return 0o700;
        }
        return 0o755;
    }

    if norm.starts_with(".ssh/") {
        let file_name = norm.rsplit('/').next().unwrap_or(norm);
        if file_name.ends_with(".pub") || file_name == "config" || file_name == "known_hosts" {
            return 0o644;
        }
        return 0o600;
    }

    if norm.starts_with(".gnupg/")
        || norm.ends_with(".nmconnection")
        || norm.contains("NetworkManager/system-connections/")
    {
        return 0o600;
    }

    if norm.ends_with(".sh") || norm.starts_with(".local/bin/") {
        return 0o755;
    }

    0o644
}

/// Calculates SHA-256 of the Blackbox archive and compares against an optional expected hex checksum.
pub fn verify_archive_sha256(
    archive_path: &Path,
    expected_sha256: Option<&str>,
) -> io::Result<(bool, String)> {
    let file = File::open(archive_path)?;
    let mut reader = BufReader::new(file);
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
    let computed: String = hash.iter().map(|b| format!("{:02x}", b)).collect();

    let verified = match expected_sha256 {
        Some(expected) => computed.eq_ignore_ascii_case(expected.trim()),
        None => true,
    };

    Ok((verified, computed))
}

/// Verifies integrity and restores Blackbox `.tar.zst` data into `target_root`, enforcing POSIX credential modes.
pub fn inject_blackbox_archive<F>(
    archive_path: &Path,
    target_root: &Path,
    expected_sha256: Option<&str>,
    target_uid: u32,
    target_gid: u32,
    mut progress_cb: F,
) -> io::Result<InjectionReport>
where
    F: FnMut(&str, usize),
{
    progress_cb("Prüfe SHA-256 Integrität der Blackbox...", 0);
    let (sha256_verified, computed_sha256) = verify_archive_sha256(archive_path, expected_sha256)?;
    if expected_sha256.is_some() && !sha256_verified {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("SHA-256 mismatch für Blackbox-Archiv (berechnet: {computed_sha256})"),
        ));
    }

    // Pass 1: Read bifrost_manifest.json first so target_username is known up front
    let mut manifest = read_manifest_from_archive(archive_path)?;
    if manifest.target_username.trim().is_empty() {
        manifest.target_username = "linuxuser".to_string();
    }

    let target_home_dir = target_root.join("home").join(&manifest.target_username);
    fs::create_dir_all(&target_home_dir)?;

    let mut files_restored = 0usize;
    let mut bytes_restored = 0u64;
    let mut permission_fixes: Vec<(PathBuf, u32)> = Vec::new();
    let mut seen_dir_fixes: HashSet<PathBuf> = HashSet::new();

    // Pass 2: Extract entries safely and enforce POSIX permissions
    let f = File::open(archive_path)?;
    let decoder = Decoder::new(BufReader::new(f))?;
    let mut tar = Archive::new(decoder);

    for entry_res in tar.entries()? {
        let mut entry = entry_res?;
        let raw_path = entry.path()?.to_path_buf();
        let Some(clean_rel) = sanitize_archive_path(&raw_path) else {
            continue;
        };

        let clean_str = clean_rel.to_string_lossy().replace('\\', "/");

        if clean_str == "bifrost_manifest.json" {
            let dest_manifest = target_root.join("var/lib/bifrost/bifrost_manifest.json");
            if let Some(parent) = dest_manifest.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut out = File::create(&dest_manifest)?;
            let copied = io::copy(&mut entry, &mut out)?;
            files_restored += 1;
            bytes_restored += copied;
            continue;
        }

        if let Some(rel_in_home) = clean_str.strip_prefix("home/") {
            if rel_in_home.is_empty() {
                continue;
            }
            let dest_path = target_home_dir.join(rel_in_home);

            if entry.header().entry_type().is_dir() {
                fs::create_dir_all(&dest_path)?;
                let mode = determine_posix_mode(rel_in_home, true);
                apply_posix_mode(&dest_path, mode)?;
                if mode == 0o700 && seen_dir_fixes.insert(dest_path.clone()) {
                    permission_fixes.push((dest_path, mode));
                }
                continue;
            }

            if !entry.header().entry_type().is_file() {
                continue;
            }

            // Ensure parent dirs exist and fix credential directory modes (.ssh, .gnupg)
            ensure_parent_dirs_with_modes(
                &target_home_dir,
                Path::new(rel_in_home),
                &mut seen_dir_fixes,
                &mut permission_fixes,
            )?;

            let mut out = File::create(&dest_path)?;
            let copied = io::copy(&mut entry, &mut out)?;
            files_restored += 1;
            bytes_restored += copied;

            let file_mode = determine_posix_mode(rel_in_home, false);
            apply_posix_mode(&dest_path, file_mode)?;
            if file_mode == 0o600 || file_mode == 0o700 {
                permission_fixes.push((dest_path, file_mode));
            }

            progress_cb(&format!("Restored: {rel_in_home}"), files_restored);
        } else if let Some(rel_nm) = clean_str.strip_prefix("etc/NetworkManager/system-connections/") {
            if rel_nm.is_empty() || !entry.header().entry_type().is_file() {
                continue;
            }
            let nm_dir = target_root.join("etc/NetworkManager/system-connections");
            fs::create_dir_all(&nm_dir)?;
            apply_posix_mode(&nm_dir, 0o755)?;
            let dest_nm = nm_dir.join(rel_nm);
            if let Some(parent) = dest_nm.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut out = File::create(&dest_nm)?;
            let copied = io::copy(&mut entry, &mut out)?;
            files_restored += 1;
            bytes_restored += copied;
            apply_posix_mode(&dest_nm, 0o600)?;
            permission_fixes.push((dest_nm, 0o600));
            progress_cb(&format!("Restored WiFi: {rel_nm}"), files_restored);
        }
    }

    #[cfg(target_os = "linux")]
    if target_uid > 0 {
        let ownership = format!("{target_uid}:{target_gid}");
        let _ = Command::new("chown")
            .args(["-R", &ownership, &target_home_dir.to_string_lossy()])
            .output();
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (target_uid, target_gid);

    Ok(InjectionReport {
        manifest,
        files_restored,
        bytes_restored,
        sha256_verified,
        computed_sha256,
        permission_fixes,
        target_home_dir,
    })
}

pub fn read_manifest_from_archive(archive_path: &Path) -> io::Result<MigrationManifest> {
    let f = File::open(archive_path)?;
    let decoder = Decoder::new(BufReader::new(f))?;
    let mut tar = Archive::new(decoder);

    for entry_res in tar.entries()? {
        let mut entry = entry_res?;
        let path_str = entry.path()?.to_string_lossy().replace('\\', "/");
        if path_str == "bifrost_manifest.json" {
            let mut json_buf = Vec::new();
            entry.read_to_end(&mut json_buf)?;
            let manifest: MigrationManifest = serde_json::from_slice(&json_buf)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            return Ok(manifest);
        }
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "bifrost_manifest.json nicht im Blackbox-Archiv gefunden",
    ))
}

fn sanitize_archive_path(raw: &Path) -> Option<PathBuf> {
    let mut clean = PathBuf::new();
    for comp in raw.components() {
        if let Component::Normal(part) = comp {
            clean.push(part);
        }
    }
    if clean.as_os_str().is_empty() {
        None
    } else {
        Some(clean)
    }
}

fn ensure_parent_dirs_with_modes(
    target_home_dir: &Path,
    rel_file_in_home: &Path,
    seen_dir_fixes: &mut HashSet<PathBuf>,
    permission_fixes: &mut Vec<(PathBuf, u32)>,
) -> io::Result<()> {
    let Some(rel_parent) = rel_file_in_home.parent() else {
        return Ok(());
    };

    let mut cur_rel = PathBuf::new();
    for comp in rel_parent.components() {
        if let Component::Normal(part) = comp {
            cur_rel.push(part);
            let full_dir = target_home_dir.join(&cur_rel);
            fs::create_dir_all(&full_dir)?;

            let rel_str = cur_rel.to_string_lossy().replace('\\', "/");
            let mode = determine_posix_mode(&rel_str, true);
            apply_posix_mode(&full_dir, mode)?;
            if mode == 0o700 && seen_dir_fixes.insert(full_dir.clone()) {
                permission_fixes.push((full_dir, mode));
            }
        }
    }

    Ok(())
}

fn apply_posix_mode(path: &Path, mode: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        let perms = fs::Permissions::from_mode(mode);
        fs::set_permissions(path, perms)?;
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
    Ok(())
}
