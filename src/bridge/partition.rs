use serde::{Deserialize, Serialize};
use std::{
    io,
    path::{Path, PathBuf},
};

#[cfg(target_os = "linux")]
use std::{fs, process::Command};

const SECTOR_SIZE_DEFAULT: u64 = 512;
const MIB: u64 = 1_048_576;
const SECTORS_PER_MIB: u64 = MIB / SECTOR_SIZE_DEFAULT; // 2048

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockDevice {
    pub name: String,
    pub dev_path: PathBuf,
    pub model: String,
    pub total_bytes: u64,
    pub sector_size: u64,
    pub is_virtual_disk: bool,
    pub partitions: Vec<PartitionEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartitionEntry {
    pub dev_path: PathBuf,
    pub part_number: u32,
    pub start_sector: u64,
    pub size_sectors: u64,
    pub size_bytes: u64,
    pub fs_type: String,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShrinkAndPartitionPlan {
    pub target_disk: PathBuf,
    pub is_virtual_disk: bool,
    pub efi_partition: PathBuf,
    pub windows_ntfs_partition: PathBuf,
    pub windows_part_number: u32,
    pub new_linux_partition: PathBuf,
    pub new_linux_part_number: u32,
    pub ntfs_original_bytes: u64,
    pub ntfs_shrunk_bytes: u64,
    pub linux_target_bytes: u64,
    pub linux_start_sector: u64,
    pub linux_size_sectors: u64,
    pub commands_to_run: Vec<String>,
}

/// Formats a kernel block partition device node path (handling `p`-suffix for numeric disk names like `nvme0n1` or `loop0`).
pub fn format_partition_path(disk_dev: &Path, part_num: u32) -> PathBuf {
    let s = disk_dev.to_string_lossy();
    let needs_p = s
        .chars()
        .last()
        .map(|c| c.is_ascii_digit())
        .unwrap_or(false);
    if needs_p {
        PathBuf::from(format!("{s}p{part_num}"))
    } else {
        PathBuf::from(format!("{s}{part_num}"))
    }
}

/// Returns true if the block device name or hardware model string indicates a VM virtual disk.
pub fn is_vm_disk_model(name: &str, model: &str) -> bool {
    if name.starts_with("vd") {
        return true;
    }
    let m = model.to_lowercase();
    m.contains("qemu")
        || m.contains("virtio")
        || m.contains("vbox")
        || m.contains("virtualbox")
        || m.contains("vmware")
        || m.contains("msft virtual")
        || m.contains("hyper-v")
}

/// Discovers physical or virtual block devices on Linux, or constructs a VM VirtIO model on Windows.
pub fn discover_block_devices() -> Vec<BlockDevice> {
    #[cfg(target_os = "linux")]
    {
        let devices = discover_linux_block_devices();
        if !devices.is_empty() {
            return devices;
        }
    }

    vec![build_vm_fallback_device()]
}

fn build_vm_fallback_device() -> BlockDevice {
    let scanned_total = crate::scanner::system::scan_system_disk()
        .map(|d| d.total_bytes)
        .unwrap_or(0);
    // Align to MiB and guarantee at least 100 GiB if host report is unavailable or too small
    let total_bytes = if scanned_total >= 40 * 1024 * MIB {
        (scanned_total / MIB) * MIB
    } else {
        100 * 1024 * MIB
    };

    let efi_start = 2048u64;
    let efi_sectors = 260 * SECTORS_PER_MIB;
    let msr_start = efi_start + efi_sectors;
    let msr_sectors = 16 * SECTORS_PER_MIB;
    let ntfs_start = msr_start + msr_sectors;
    let total_sectors = total_bytes / SECTOR_SIZE_DEFAULT;
    // Leave 2048 sectors at end for backup GPT header
    let ntfs_sectors = total_sectors.saturating_sub(ntfs_start + 2048);

    BlockDevice {
        name: "vda".to_string(),
        dev_path: PathBuf::from("/dev/vda"),
        model: "VirtIO Block Device (VM)".to_string(),
        total_bytes,
        sector_size: SECTOR_SIZE_DEFAULT,
        is_virtual_disk: true,
        partitions: vec![
            PartitionEntry {
                dev_path: PathBuf::from("/dev/vda1"),
                part_number: 1,
                start_sector: efi_start,
                size_sectors: efi_sectors,
                size_bytes: efi_sectors * SECTOR_SIZE_DEFAULT,
                fs_type: "vfat".to_string(),
                label: Some("EFI".to_string()),
            },
            PartitionEntry {
                dev_path: PathBuf::from("/dev/vda2"),
                part_number: 2,
                start_sector: msr_start,
                size_sectors: msr_sectors,
                size_bytes: msr_sectors * SECTOR_SIZE_DEFAULT,
                fs_type: "msr".to_string(),
                label: Some("Microsoft reserved".to_string()),
            },
            PartitionEntry {
                dev_path: PathBuf::from("/dev/vda3"),
                part_number: 3,
                start_sector: ntfs_start,
                size_sectors: ntfs_sectors,
                size_bytes: ntfs_sectors * SECTOR_SIZE_DEFAULT,
                fs_type: "ntfs".to_string(),
                label: Some("Windows".to_string()),
            },
        ],
    }
}

#[cfg(target_os = "linux")]
pub fn discover_linux_block_devices() -> Vec<BlockDevice> {
    let mut devices = Vec::new();
    let Ok(entries) = fs::read_dir("/sys/block") else {
        return devices;
    };

    // Optionally collect fs metadata from lsblk -J
    let lsblk_json = Command::new("lsblk")
        .args(["-J", "-b", "-o", "NAME,PATH,SIZE,FSTYPE,LABEL,MODEL"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| serde_json::from_slice::<serde_json::Value>(&o.stdout).ok());

    for entry in entries.filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("loop")
            || name.starts_with("ram")
            || name.starts_with("sr")
            || name.starts_with("zram")
            || name.starts_with("dm-")
        {
            continue;
        }

        let sys_path = entry.path();
        let size_sectors = fs::read_to_string(sys_path.join("size"))
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0);
        if size_sectors == 0 {
            continue;
        }

        let model = fs::read_to_string(sys_path.join("device/model"))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| find_lsblk_str(&lsblk_json, &name, "model").unwrap_or_else(|| "Block Disk".to_string()));

        let dev_path = PathBuf::from(format!("/dev/{name}"));
        let partitions = read_sysfs_partitions(&sys_path, &name, &dev_path, &lsblk_json);

        devices.push(BlockDevice {
            is_virtual_disk: is_vm_disk_model(&name, &model),
            name,
            dev_path,
            model,
            total_bytes: size_sectors * SECTOR_SIZE_DEFAULT,
            sector_size: SECTOR_SIZE_DEFAULT,
            partitions,
        });
    }

    devices
}

#[cfg(target_os = "linux")]
fn read_sysfs_partitions(
    sys_disk_path: &Path,
    disk_name: &str,
    disk_dev: &Path,
    lsblk_json: &Option<serde_json::Value>,
) -> Vec<PartitionEntry> {
    let mut parts = Vec::new();
    let Ok(entries) = fs::read_dir(sys_disk_path) else {
        return parts;
    };

    for entry in entries.filter_map(|e| e.ok()) {
        let pname = entry.file_name().to_string_lossy().into_owned();
        if !pname.starts_with(disk_name) {
            continue;
        }
        let ppath = entry.path();
        if !ppath.join("partition").exists() {
            continue;
        }
        let part_number = fs::read_to_string(ppath.join("partition"))
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(0);
        if part_number == 0 {
            continue;
        }
        let start_sector = fs::read_to_string(ppath.join("start"))
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0);
        let size_sectors = fs::read_to_string(ppath.join("size"))
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0);

        let mut fs_type = find_lsblk_str(lsblk_json, &pname, "fstype").unwrap_or_default();
        let part_dev_path = format_partition_path(disk_dev, part_number);
        if fs_type.is_empty() {
            if let Ok(out) = Command::new("blkid")
                .args(["-s", "TYPE", "-o", "value", &part_dev_path.to_string_lossy()])
                .output()
            {
                if out.status.success() {
                    fs_type = String::from_utf8_lossy(&out.stdout).trim().to_string();
                }
            }
        }
        let label = find_lsblk_str(lsblk_json, &pname, "label");

        parts.push(PartitionEntry {
            dev_path: part_dev_path,
            part_number,
            start_sector,
            size_sectors,
            size_bytes: size_sectors * SECTOR_SIZE_DEFAULT,
            fs_type,
            label,
        });
    }

    parts.sort_by_key(|p| p.part_number);
    parts
}

#[cfg(target_os = "linux")]
fn find_lsblk_str(json: &Option<serde_json::Value>, name: &str, field: &str) -> Option<String> {
    let root = json.as_ref()?;
    let arr = root.get("blockdevices")?.as_array()?;
    for dev in arr {
        if dev.get("name").and_then(|v| v.as_str()) == Some(name) {
            return dev.get(field).and_then(|v| v.as_str()).map(|s| s.trim().to_string());
        }
        if let Some(children) = dev.get("children").and_then(|v| v.as_array()) {
            for child in children {
                if child.get("name").and_then(|v| v.as_str()) == Some(name) {
                    return child.get(field).and_then(|v| v.as_str()).map(|s| s.trim().to_string());
                }
            }
        }
    }
    None
}

/// Calculates sector-aligned shrink bounds and Linux partition geometry without modifying disk state.
pub fn calculate_shrink_plan(
    disk: &BlockDevice,
    requested_linux_bytes: u64,
    min_ntfs_remaining_bytes: u64,
) -> Result<ShrinkAndPartitionPlan, String> {
    let sector_size = if disk.sector_size > 0 {
        disk.sector_size
    } else {
        SECTOR_SIZE_DEFAULT
    };

    let efi_part = disk
        .partitions
        .iter()
        .find(|p| p.fs_type.eq_ignore_ascii_case("vfat") || p.part_number == 1)
        .ok_or_else(|| "Keine EFI-Systempartition gefunden.".to_string())?;

    let ntfs_part = disk
        .partitions
        .iter()
        .filter(|p| p.part_number != efi_part.part_number)
        .find(|p| p.fs_type.eq_ignore_ascii_case("ntfs"))
        .or_else(|| disk.partitions.iter().max_by_key(|p| p.size_bytes))
        .ok_or_else(|| "Keine Windows NTFS-Partition zum Verkleinern gefunden.".to_string())?;

    let aligned_linux_bytes = (requested_linux_bytes / MIB) * MIB;
    if aligned_linux_bytes < MIB {
        return Err("Zielgröße für Linux muss mindestens 1 MiB betragen.".to_string());
    }

    if ntfs_part.size_bytes <= aligned_linux_bytes.saturating_add(min_ntfs_remaining_bytes) {
        return Err(format!(
            "NTFS-Partition ({} MiB) ist zu klein für {} MiB Linux + {} MiB Windows-Minimum.",
            ntfs_part.size_bytes / MIB,
            aligned_linux_bytes / MIB,
            min_ntfs_remaining_bytes / MIB
        ));
    }

    let raw_shrunk_bytes = ntfs_part.size_bytes - aligned_linux_bytes;
    let ntfs_shrunk_bytes = (raw_shrunk_bytes / MIB) * MIB;
    let shrunk_sectors = ntfs_shrunk_bytes / sector_size;

    let ntfs_end_sector = ntfs_part.start_sector + shrunk_sectors;
    // Align Linux partition start to 1 MiB sector boundary
    let sectors_per_mib = MIB / sector_size;
    let linux_start_sector = ((ntfs_end_sector + sectors_per_mib - 1) / sectors_per_mib) * sectors_per_mib;

    let orig_ntfs_end = ntfs_part.start_sector + ntfs_part.size_sectors;
    if linux_start_sector >= orig_ntfs_end {
        return Err("Nicht genügend Sektoren nach 1-MiB-Ausrichtung verfügbar.".to_string());
    }
    let linux_size_sectors = ((orig_ntfs_end - linux_start_sector) / sectors_per_mib) * sectors_per_mib;
    let linux_target_bytes = linux_size_sectors * sector_size;

    let max_part_num = disk
        .partitions
        .iter()
        .map(|p| p.part_number)
        .max()
        .unwrap_or(ntfs_part.part_number);
    let new_linux_part_number = max_part_num + 1;
    let new_linux_partition = format_partition_path(&disk.dev_path, new_linux_part_number);

    let win_end_mib = (ntfs_part.start_sector * sector_size + ntfs_shrunk_bytes) / MIB;
    let linux_start_mib = (linux_start_sector * sector_size) / MIB;
    let linux_end_mib = ((linux_start_sector + linux_size_sectors) * sector_size) / MIB;

    let disk_str = disk.dev_path.to_string_lossy();
    let win_str = ntfs_part.dev_path.to_string_lossy();

    let commands_to_run = vec![
        format!("yes | ntfsresize --force --force --no-progress-bar --size {ntfs_shrunk_bytes} {win_str}"),
        format!(
            "printf 'Yes\\n' | parted ---pretend-input-tty {disk_str} resizepart {} {win_end_mib}MiB",
            ntfs_part.part_number
        ),
        format!(
            "parted -s {disk_str} mkpart BIFROEST_LINUX btrfs {linux_start_mib}MiB {linux_end_mib}MiB"
        ),
        format!("partprobe {disk_str}"),
    ];

    Ok(ShrinkAndPartitionPlan {
        target_disk: disk.dev_path.clone(),
        is_virtual_disk: disk.is_virtual_disk,
        efi_partition: efi_part.dev_path.clone(),
        windows_ntfs_partition: ntfs_part.dev_path.clone(),
        windows_part_number: ntfs_part.part_number,
        new_linux_partition,
        new_linux_part_number,
        ntfs_original_bytes: ntfs_part.size_bytes,
        ntfs_shrunk_bytes,
        linux_target_bytes,
        linux_start_sector,
        linux_size_sectors,
        commands_to_run,
    })
}

/// Executes or dry-runs the computed shrink & partitioning command sequence.
pub fn execute_partition_plan<F>(
    plan: &ShrinkAndPartitionPlan,
    dry_run: bool,
    mut progress_cb: F,
) -> io::Result<()>
where
    F: FnMut(&str, usize, usize),
{
    #[cfg(target_os = "linux")]
    if !dry_run {
        let win_str = plan.windows_ntfs_partition.to_string_lossy();
        let _ = Command::new("umount").arg(win_str.as_ref()).output();
        let _ = Command::new("ntfsfix").args(["-d", win_str.as_ref()]).output();
    }

    let total = plan.commands_to_run.len();
    for (idx, cmd) in plan.commands_to_run.iter().enumerate() {
        let prefix = if dry_run || !cfg!(target_os = "linux") {
            "[DRY-RUN]"
        } else {
            "[EXEC]"
        };
        progress_cb(&format!("{prefix} {cmd}"), idx + 1, total);

        #[cfg(target_os = "linux")]
        if !dry_run {
            let output = Command::new("sh").args(["-c", cmd]).output()?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let detail = if !stderr.is_empty() && !stdout.is_empty() {
                    format!("{stderr} | {stdout}")
                } else if !stderr.is_empty() {
                    stderr
                } else if !stdout.is_empty() {
                    stdout
                } else {
                    format!("exit status: {}", output.status)
                };
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!("Command failed ({cmd}): {detail}"),
                ));
            }
        }
    }

    Ok(())
}

/// Queries ntfsresize --info --force to discover the absolute minimum safe partition size for an NTFS volume.
pub fn query_ntfs_minimum_size(part_dev: &Path) -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let out = Command::new("ntfsresize")
            .args(["--info", "--force", &part_dev.to_string_lossy()])
            .output()
            .ok()?;
        let text = format!(
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        for line in text.lines() {
            if line.contains("might resize at") || line.contains("minimum size") {
                let tokens: Vec<&str> = line.split_whitespace().collect();
                for (idx, w) in tokens.iter().enumerate() {
                    if idx + 1 < tokens.len() && tokens[idx + 1].eq_ignore_ascii_case("bytes") {
                        if let Ok(bytes) = w.replace(',', "").parse::<u64>() {
                            return Some(bytes);
                        }
                    }
                }
            }
        }
    }
    let _ = part_dev;
    None
}
