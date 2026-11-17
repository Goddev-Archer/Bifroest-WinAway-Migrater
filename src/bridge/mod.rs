pub mod bootstrap;
pub mod fs;
pub mod handoff;
pub mod inject;
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

/// Runs the complete Linux RAMFS migration pipeline (discovery, NTFS shrink, Btrfs setup, Blackbox injection, bootloader & post-install).
pub fn execute_full_bridge_pipeline<F>(
    archive_path: &Path,
    expected_sha256: Option<&str>,
    target_root: &Path,
    requested_linux_bytes: u64,
    dry_run: bool,
    mut step_cb: F,
) -> io::Result<BridgeExecutionSummary>
where
    F: FnMut(&str, usize, usize),
{
    const TOTAL_STEPS: usize = 5;
    const GIB: u64 = 1024 * 1024 * 1024;
    const MIB: u64 = 1024 * 1024;

    // Step 1/5: Discover disks & compute non-destructive ShrinkAndPartitionPlan
    step_cb("Ermittle Block-Devices & berechne Partitionsplan...", 1, TOTAL_STEPS);
    let devices = discover_block_devices();
    let disk = devices
        .into_iter()
        .find(|d| d.is_virtual_disk)
        .or_else(|| discover_block_devices().into_iter().next())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Kein Ziel-Laufwerk gefunden"))?;

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

    // Safely clamp requested Linux partition size and minimum remaining NTFS space
    let min_ntfs_remaining = if let Some(needed) = min_needed_by_files {
        // Keep actual files + 512 MiB safety buffer, leaving at least 4 GiB for Linux if possible
        (needed + 512 * MIB).max(12 * GIB).min(ntfs_max.saturating_sub(4 * GIB))
    } else if ntfs_max > 24 * GIB {
        8 * GIB
    } else {
        // For small VM disks (~20 GB), keep at least 12-14 GB for Windows files
        (ntfs_max * 6 / 10).max(12 * GIB).min(ntfs_max.saturating_sub(4 * GIB))
    };

    let max_allocatable = ntfs_max.saturating_sub(min_ntfs_remaining + 64 * MIB);
    let effective_linux_bytes = target_req.min(max_allocatable).max(MIB);

    let plan = calculate_shrink_plan(&disk, effective_linux_bytes, min_ntfs_remaining)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

    // Step 2/5: Execute partition plan (ntfsresize + parted)
    step_cb(
        &format!(
            "Führe NTFS-Shrink & GPT-Anpassung auf {} durch...",
            plan.target_disk.display()
        ),
        2,
        TOTAL_STEPS,
    );
    execute_partition_plan(&plan, dry_run, |cmd_msg, idx, total| {
        let _ = (idx, total);
        step_cb(cmd_msg, 2, TOTAL_STEPS);
    })?;

    // Step 3/5: Format & create Btrfs subvolume layout + /etc/fstab
    step_cb("Erstelle Btrfs Subvolumes (@root, @home, @blackbox) & fstab...", 3, TOTAL_STEPS);
    std::fs::create_dir_all(target_root)?;
    setup_target_filesystems(
        target_root,
        &plan.new_linux_partition,
        &plan.efi_partition,
        "Btrfs (@root, @home, @blackbox, zstd)",
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

    // Step 5/5: Deploy systemd-boot & Flatpak first-boot provisioning service
    step_cb("Deploye systemd-boot & First-Boot Software-Service...", 5, TOTAL_STEPS);
    deploy_base_system(
        target_root,
        None,
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
        assert!(plan.commands_to_run[0].contains("ntfsresize --force --no-action"));
    }

    #[test]
    fn test_posix_permission_rules() {
        assert_eq!(determine_posix_mode(".ssh", true), 0o700);
        assert_eq!(determine_posix_mode(".ssh/id_ed25519", false), 0o600);
        assert_eq!(determine_posix_mode(".ssh/id_ed25519.pub", false), 0o644);
        assert_eq!(determine_posix_mode(".ssh/config", false), 0o644);
        assert_eq!(determine_posix_mode(".gnupg/trustdb.gpg", false), 0o600);
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
        let repo_dir = win_prof.join("dev").join("DemoApp");
        fs::create_dir_all(&ssh_dir).unwrap();
        fs::create_dir_all(&docs_dir).unwrap();
        fs::create_dir_all(repo_dir.join(".git")).unwrap();

        fs::write(ssh_dir.join("id_ed25519"), "PRIVATE-KEY-CONTENT").unwrap();
        fs::write(ssh_dir.join("id_ed25519.pub"), "ssh-ed25519 AAAAC3Nza test").unwrap();
        fs::write(docs_dir.join("report.txt"), "Migration report content").unwrap();
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
        assert!(has_ssh_dir_700, "Expected 0o700 fix recorded for .ssh");
        assert!(has_key_600, "Expected 0o600 fix recorded for id_ed25519");

        let _ = fs::remove_dir_all(&base_tmp);
    }
}
