pub mod bootstrap;
pub mod distro;
pub mod fs;
pub mod handoff;
pub mod inject;
pub mod linux_efi;
pub mod linux_hop;
pub mod partition;
pub mod ramfs_cinema;

use std::{
    io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use bootstrap::deploy_base_system;
use fs::setup_target_filesystems;
use inject::{inject_blackbox_archive, InjectionReport};
pub use partition::calculate_clean_install_plan;
use partition::{
    calculate_shrink_plan, discover_block_devices, execute_partition_plan, ShrinkAndPartitionPlan,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeExecutionSummary {
    pub plan: ShrinkAndPartitionPlan,
    pub injection: InjectionReport,
    pub fstab_path: PathBuf,
    pub bootloader_entry_path: PathBuf,
    pub postinstall_script_path: PathBuf,
    pub target_root: PathBuf,
}

/// Runs the complete Linux RAMFS migration pipeline using the archive manifest's migration strategy.
pub fn execute_full_bridge_pipeline<F>(
    archive_path: &Path,
    expected_sha256: Option<&str>,
    target_root: &Path,
    requested_linux_bytes: u64,
    dry_run: bool,
    step_cb: F,
) -> io::Result<BridgeExecutionSummary>
where
    F: FnMut(&str, usize, usize),
{
    execute_full_bridge_pipeline_with_mode(
        archive_path,
        expected_sha256,
        target_root,
        requested_linux_bytes,
        None,
        dry_run,
        step_cb,
    )
}

/// Runs the complete Linux RAMFS migration pipeline with an optional Clean-Install vs Dual-Boot strategy override.
pub fn execute_full_bridge_pipeline_with_mode<F>(
    archive_path: &Path,
    expected_sha256: Option<&str>,
    target_root: &Path,
    requested_linux_bytes: u64,
    clean_install_override: Option<bool>,
    dry_run: bool,
    mut step_cb: F,
) -> io::Result<BridgeExecutionSummary>
where
    F: FnMut(&str, usize, usize),
{
    const TOTAL_STEPS: usize = 5;
    const GIB: u64 = 1024 * 1024 * 1024;
    const MIB: u64 = 1024 * 1024;

    let archive_manifest = inject::read_manifest_from_archive(archive_path).ok();
    let clean_install = clean_install_override.unwrap_or_else(|| {
        archive_manifest
            .as_ref()
            .map(|m| m.clean_install)
            .unwrap_or(true)
    });
    let target_filesystem = archive_manifest
        .as_ref()
        .map(|m| m.target_filesystem.clone())
        .filter(|fs| !fs.trim().is_empty())
        .unwrap_or_else(|| "Btrfs (@root, @home, @blackbox, zstd)".to_string());

    // Step 1/5: Discover disks & compute Clean-Install or Dual-Boot PartitionPlan
    step_cb("Ermittle Block-Devices & berechne Partitionsplan...", 1, TOTAL_STEPS);
    let devices = discover_block_devices();
    let disk = devices
        .into_iter()
        .find(|d| d.is_virtual_disk)
        .or_else(|| discover_block_devices().into_iter().next())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Kein Ziel-Laufwerk gefunden"))?;

    let plan = if clean_install {
        calculate_clean_install_plan(&disk)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?
    } else {
        let ntfs_part_opt = disk
            .partitions
            .iter()
            .find(|p| p.fs_type.eq_ignore_ascii_case("ntfs"))
            .or_else(|| disk.partitions.iter().max_by_key(|p| p.size_bytes));

        let ntfs_max = ntfs_part_opt.map(|p| p.size_bytes).unwrap_or(disk.total_bytes);

        let target_req = if requested_linux_bytes > 0 {
            requested_linux_bytes
        } else {
            32 * GIB
        };

        let min_needed_by_files = if let Some(ntfs_part) = ntfs_part_opt {
            crate::bridge::partition::query_ntfs_minimum_size(&ntfs_part.dev_path)
        } else {
            None
        };

        // Never clamp below needed + 512 MiB so ntfsresize succeeds safely
        let min_ntfs_remaining = if let Some(needed) = min_needed_by_files {
            (needed + 512 * MIB).min(ntfs_max.saturating_sub(512 * MIB))
        } else if ntfs_max > 24 * GIB {
            12 * GIB
        } else {
            (ntfs_max * 75 / 100).min(ntfs_max.saturating_sub(1024 * MIB))
        };

        let max_allocatable = ntfs_max.saturating_sub(min_ntfs_remaining + 64 * MIB);
        let effective_linux_bytes = target_req.min(max_allocatable).max(MIB);

        calculate_shrink_plan(&disk, effective_linux_bytes, min_ntfs_remaining)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?
    };

    // Step 2/5: Execute partition plan (Clean Install wipe or Dual-Boot NTFS shrink)
    let phase2_msg = if clean_install {
        format!(
            "Clean Install: Ersetze Windows-Partition & erstelle Btrfs-Ziel auf {}...",
            plan.target_disk.display()
        )
    } else {
        format!(
            "Dual-Boot: Führe NTFS-Shrink & GPT-Anpassung auf {} durch...",
            plan.target_disk.display()
        )
    };
    step_cb(&phase2_msg, 2, TOTAL_STEPS);
    execute_partition_plan(&plan, dry_run, |cmd_msg, idx, total| {
        let _ = (idx, total);
        step_cb(cmd_msg, 2, TOTAL_STEPS);
    })?;

    // Step 3/5: Format & create target filesystem layout + /etc/fstab (honoring user's choice)
    step_cb(
        &format!("Erstelle Dateisystem-Layout ({target_filesystem}) & fstab..."),
        3,
        TOTAL_STEPS,
    );
    std::fs::create_dir_all(target_root)?;
    setup_target_filesystems(
        target_root,
        &plan.new_linux_partition,
        &plan.efi_partition,
        &target_filesystem,
        dry_run,
        |fs_msg, _i, _t| {
            step_cb(fs_msg, 3, TOTAL_STEPS);
        },
    )?;

    // Step 4/5: Verify SHA-256 & inject Blackbox archive with POSIX permissions
    step_cb("Verifiziere SHA-256 & injiziere Blackbox-Daten nach /home...", 4, TOTAL_STEPS);
    let injection = inject_blackbox_archive(
        archive_path,
        target_root,
        expected_sha256,
        1000,
        1000,
        |inj_msg, _count| {
            step_cb(inj_msg, 4, TOTAL_STEPS);
        },
    )?;

    if !dry_run {
        let blackbox_backup_dir = target_root.join("var/lib/bifrost");
        let _ = std::fs::create_dir_all(&blackbox_backup_dir);
        let _ = std::fs::copy(
            archive_path,
            blackbox_backup_dir.join("bifrost_blackbox.tar.zst"),
        );
    }

    // Step 5/5: Extract real Linux rootfs & deploy systemd-boot + first-boot service
    step_cb("Entpacke Linux-Rootfs & deploye systemd-boot...", 5, TOTAL_STEPS);
    let rootfs_opt = [
        Path::new("/tmp/bifrost_rootfs.tar.zst"),
        Path::new("/tmp/bifrost_rootfs.tar.gz"),
        Path::new("/tmp/bifrost_rootfs.tar.xz"),
        Path::new("/usr/share/bifrost/base-rootfs.tar.zst"),
    ]
    .into_iter()
    .find(|p| p.exists())
    .map(|p| p.to_path_buf());

    deploy_base_system(
        target_root,
        rootfs_opt.as_deref(),
        &injection.manifest,
        "BIFROEST-ROOT-UUID",
        dry_run,
        |boot_msg, _i, _t| {
            step_cb(boot_msg, 5, TOTAL_STEPS);
        },
    )?;

    Ok(BridgeExecutionSummary {
        plan,
        injection,
        fstab_path: target_root.join("etc/fstab"),
        bootloader_entry_path: target_root.join("boot/efi/loader/entries/bifroest-linux.conf"),
        postinstall_script_path: target_root.join("usr/local/bin/bifrost-postinstall.sh"),
        target_root: target_root.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use crate::blackbox::create_blackbox_archive;
    use crate::bridge::inject::determine_posix_mode;
    use crate::bridge::partition::{format_partition_path, BlockDevice, PartitionEntry};
    use crate::model::{CandidateCategory, CandidateItem, DiscoveredSoftware, GitRepoInfo};

    #[test]
    fn test_partition_path_formatting_and_shrink_math() {
        assert_eq!(
            format_partition_path(Path::new("/dev/vda"), 4),
            PathBuf::from("/dev/vda4")
        );
        assert_eq!(
            format_partition_path(Path::new("/dev/nvme0n1"), 4),
            PathBuf::from("/dev/nvme0n1p4")
        );
        assert_eq!(
            format_partition_path(Path::new("/dev/loop0"), 2),
            PathBuf::from("/dev/loop0p2")
        );

        const MIB: u64 = 1_048_576;
        const GIB: u64 = 1024 * MIB;
        let sectors_per_mib = 2048u64;

        let disk = BlockDevice {
            name: "nvme0n1".to_string(),
            dev_path: PathBuf::from("/dev/nvme0n1"),
            model: "QEMU NVMe Ctrl".to_string(),
            total_bytes: 100 * GIB,
            sector_size: 512,
            is_virtual_disk: true,
            partitions: vec![
                PartitionEntry {
                    dev_path: PathBuf::from("/dev/nvme0n1p1"),
                    part_number: 1,
                    start_sector: 2048,
                    size_sectors: 260 * sectors_per_mib,
                    size_bytes: 260 * MIB,
                    fs_type: "vfat".to_string(),
                    label: Some("EFI".to_string()),
                },
                PartitionEntry {
                    dev_path: PathBuf::from("/dev/nvme0n1p3"),
                    part_number: 3,
                    start_sector: 262 * sectors_per_mib,
                    size_sectors: 90 * 1024 * sectors_per_mib,
                    size_bytes: 90 * GIB,
                    fs_type: "ntfs".to_string(),
                    label: Some("Windows".to_string()),
                },
            ],
        };

        let plan = calculate_shrink_plan(&disk, 40 * GIB, 10 * GIB).expect("Shrink plan failed");
        assert_eq!(plan.new_linux_partition, PathBuf::from("/dev/nvme0n1p4"));
        assert_eq!(plan.new_linux_part_number, 4);
        assert_eq!(plan.ntfs_shrunk_bytes, 50 * GIB);
        assert_eq!(plan.linux_start_sector % sectors_per_mib, 0);
        assert_eq!(plan.linux_target_bytes, 40 * GIB);
        assert!(plan.commands_to_run[0].contains("ntfsresize --force --force"));

        let clean_plan = calculate_clean_install_plan(&disk).expect("Clean install plan failed");
        assert_eq!(clean_plan.new_linux_partition, PathBuf::from("/dev/nvme0n1p3"));
        assert_eq!(clean_plan.new_linux_part_number, 3);
        assert_eq!(clean_plan.ntfs_shrunk_bytes, 0);
        assert!(clean_plan.commands_to_run.iter().any(|c| c.contains("parted -s /dev/nvme0n1 rm 3")));
        assert!(clean_plan.commands_to_run.iter().any(|c| c.contains("mkpart BIFROEST_LINUX btrfs") && c.contains("100%")));
        assert!(!clean_plan.commands_to_run.iter().any(|c| c.contains("ntfsresize")));
    }

    #[test]
    fn test_posix_permission_rules() {
        assert_eq!(determine_posix_mode(".ssh", true), 0o700);
        assert_eq!(determine_posix_mode(".ssh/id_ed25519", false), 0o600);
        assert_eq!(determine_posix_mode(".ssh/id_ed25519.pub", false), 0o644);
        assert_eq!(determine_posix_mode(".ssh/config", false), 0o644);
        assert_eq!(determine_posix_mode(".gnupg/trustdb.gpg", false), 0o600);
        assert_eq!(determine_posix_mode(".config/gh", true), 0o700);
        assert_eq!(determine_posix_mode(".config/gh/hosts.yml", false), 0o600);
        assert_eq!(determine_posix_mode(".git-credentials", false), 0o600);
        assert_eq!(determine_posix_mode(".gitconfig", false), 0o644);
        assert_eq!(determine_posix_mode("Documents/notes.txt", false), 0o644);
    }

    #[test]
    fn test_full_vm_bridge_end_to_end() {
        let base_tmp = std::env::temp_dir().join("bifrost_vm_bridge_test");
        let _ = fs::remove_dir_all(&base_tmp);
        let win_prof = base_tmp.join("Users").join("tester");
        let target_root = base_tmp.join("target_root");

        let ssh_dir = win_prof.join(".ssh");
        let docs_dir = win_prof.join("Documents");
        let gh_dir = win_prof.join("AppData").join("Roaming").join("GitHub CLI");
        let gitconfig_file = win_prof.join(".gitconfig");
        let git_creds_file = win_prof.join(".git-credentials");
        let repo_dir = win_prof.join("dev").join("DemoApp");
        fs::create_dir_all(&ssh_dir).unwrap();
        fs::create_dir_all(&docs_dir).unwrap();
        fs::create_dir_all(&gh_dir).unwrap();
        fs::create_dir_all(repo_dir.join(".git")).unwrap();

        fs::write(ssh_dir.join("id_ed25519"), "PRIVATE-KEY-CONTENT").unwrap();
        fs::write(ssh_dir.join("id_ed25519.pub"), "ssh-ed25519 AAAAC3Nza test").unwrap();
        fs::write(docs_dir.join("report.txt"), "Migration report content").unwrap();
        fs::write(gh_dir.join("hosts.yml"), "github.com:\n  user: demo\n").unwrap();
        fs::write(&gitconfig_file, "[user]\n\tname = Demo\n").unwrap();
        fs::write(&git_creds_file, "https://demo:token@github.com\n").unwrap();
        fs::write(repo_dir.join("README.md"), "# DemoApp\n").unwrap();

        let candidates = vec![
            CandidateItem {
                id: "ssh".to_string(),
                path: ssh_dir,
                display_name: ".ssh".to_string(),
                description: "SSH keys".to_string(),
                total_bytes: 50,
                size_human: "50 B".to_string(),
                category: CandidateCategory::Credentials,
                selected: true,
                item_count: 2,
            },
            CandidateItem {
                id: "docs".to_string(),
                path: docs_dir,
                display_name: "Documents".to_string(),
                description: "Docs".to_string(),
                total_bytes: 24,
                size_human: "24 B".to_string(),
                category: CandidateCategory::Documents,
                selected: true,
                item_count: 1,
            },
            CandidateItem {
                id: "gitconfig".to_string(),
                path: gitconfig_file,
                display_name: "~/.gitconfig".to_string(),
                description: "Git config".to_string(),
                total_bytes: 20,
                size_human: "20 B".to_string(),
                category: CandidateCategory::ApplicationConfig,
                selected: true,
                item_count: 1,
            },
            CandidateItem {
                id: "git_credentials".to_string(),
                path: git_creds_file,
                display_name: "~/.git-credentials".to_string(),
                description: "Git creds".to_string(),
                total_bytes: 32,
                size_human: "32 B".to_string(),
                category: CandidateCategory::Credentials,
                selected: true,
                item_count: 1,
            },
            CandidateItem {
                id: "gh_cli".to_string(),
                path: gh_dir,
                display_name: "%APPDATA%\\GitHub CLI".to_string(),
                description: "GitHub CLI".to_string(),
                total_bytes: 26,
                size_human: "26 B".to_string(),
                category: CandidateCategory::Credentials,
                selected: true,
                item_count: 1,
            },
        ];

        let git_repos = vec![GitRepoInfo {
            path: repo_dir,
            name: "DemoApp".to_string(),
            current_branch: "main".to_string(),
            is_dirty: true,
            uncommitted_files_count: 1,
            selected: true,
        }];

        let software = vec![DiscoveredSoftware {
            name: "Visual Studio Code".to_string(),
            id: "Microsoft.VisualStudioCode".to_string(),
            source: "Winget".to_string(),
            flatpak_id: Some("com.visualstudio.code".to_string()),
            native_pkg: Some("code".to_string()),
            will_reinstall: true,
        }];

        let archive_path = base_tmp.join("bifrost_blackbox.tar.zst");
        let pack_res = create_blackbox_archive(
            &archive_path,
            &win_prof,
            &candidates,
            &git_repos,
            &software,
            "Arch Linux",
            "Btrfs (@root, @home, @blackbox, zstd)",
            |_| {},
        )
        .expect("Blackbox packing failed");

        let summary = execute_full_bridge_pipeline(
            &archive_path,
            Some(&pack_res.sha256_checksum),
            &target_root,
            32 * 1024 * 1024 * 1024,
            true,
            |_, _, _| {},
        )
        .expect("Bridge pipeline execution failed");

        assert!(summary.injection.sha256_verified);
        assert!(summary.fstab_path.exists());
        assert!(target_root.join("boot/efi/loader/loader.conf").exists());
        assert!(summary.bootloader_entry_path.exists());
        assert!(summary.postinstall_script_path.exists());
        assert!(target_root.join("home/tester/.ssh/id_ed25519").exists());
        assert!(target_root.join("home/tester/.gitconfig").exists());
        assert!(target_root.join("home/tester/.git-credentials").exists());
        assert!(target_root.join("home/tester/.config/gh/hosts.yml").exists());
        assert!(target_root.join("home/tester/Projects/DemoApp/README.md").exists());

        let has_ssh_dir_700 = summary
            .injection
            .permission_fixes
            .iter()
            .any(|(p, m)| p.ends_with(".ssh") && *m == 0o700);
        let has_key_600 = summary
            .injection
            .permission_fixes
            .iter()
            .any(|(p, m)| p.ends_with("id_ed25519") && *m == 0o600);
        let has_gh_600 = summary
            .injection
            .permission_fixes
            .iter()
            .any(|(p, m)| p.ends_with("hosts.yml") && *m == 0o600);
        let has_git_creds_600 = summary
            .injection
            .permission_fixes
            .iter()
            .any(|(p, m)| p.ends_with(".git-credentials") && *m == 0o600);
        assert!(has_ssh_dir_700, "Expected 0o700 fix recorded for .ssh");
        assert!(has_key_600, "Expected 0o600 fix recorded for id_ed25519");
        assert!(has_gh_600, "Expected 0o600 fix recorded for gh hosts.yml");
        assert!(has_git_creds_600, "Expected 0o600 fix recorded for .git-credentials");

        let _ = fs::remove_dir_all(&base_tmp);
    }
}
