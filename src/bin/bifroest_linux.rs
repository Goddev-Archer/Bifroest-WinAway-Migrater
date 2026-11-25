use std::{
    fs, io,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

use bifroest::{
    blackbox::create_blackbox_archive_with_mode,
    bridge::{
        distro::resolve_tier1_distro,
        handoff::{ensure_distro_rootfs, stage_vm_handoff_with_progress},
        linux_hop::{
            cleanup_rollback_subvolumes, execute_btrfs_subvolume_hop, plan_btrfs_subvolume_hop,
            SubvolumeHopSummary,
        },
    },
    model::{
        format_bytes, CandidateCategory, CandidateItem, DesktopEnvironment, DiscoveredSoftware,
        DiskScanInfo, GitRepoInfo, LinuxMigrationMode, MigrationManifest, RollbackPolicy,
        ScanResult,
    },
    scanner::{get_user_profile_dir, Scanner},
};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame, Terminal,
};

const LOGO_TXT: &str = include_str!("../../logo.txt");
const C_BORDER: Color = Color::DarkGray;
const C_BORDER_FOCUS: Color = Color::Cyan;
const C_TEXT: Color = Color::Gray;
const C_TEXT_BRIGHT: Color = Color::White;
const C_MUTED: Color = Color::DarkGray;
const C_ACCENT: Color = Color::Cyan;
const C_SUCCESS: Color = Color::LightGreen;
const C_WARN_BORDER: Color = Color::LightRed;
const C_WARN_TITLE: Color = Color::LightYellow;

#[derive(Clone, Copy, PartialEq, Eq)]
enum LinuxPhase {
    Gate = 0,
    Scanning = 1,
    Diagnostics = 2,
    Payload = 3,
    Blueprint = 4,
    Flightplan = 5,
    Executing = 6,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LinuxBlueprintFocus {
    Distro,
    Desktop,
    MigrationMode,
    RollbackPolicy,
    SoftwareList,
}

#[derive(Clone)]
struct ScanStatus {
    message: String,
    step: usize,
    total: usize,
}

struct LinuxWizardApp {
    should_quit: bool,
    request_sudo_reexec: bool,
    phase: LinuxPhase,
    gate_button_idx: usize,
    spinner_tick: usize,

    scan_status: Arc<Mutex<ScanStatus>>,
    scan_slot: Arc<Mutex<Option<ScanResult>>>,
    is_scanning: Arc<AtomicBool>,

    user_profile: PathBuf,
    disk_info: Option<DiskScanInfo>,
    candidates: Vec<CandidateItem>,
    git_repos: Vec<GitRepoInfo>,
    software: Vec<DiscoveredSoftware>,

    selected_payload_idx: usize,

    blueprint_focus: LinuxBlueprintFocus,
    distros: Vec<&'static str>,
    selected_distro: usize,
    selected_desktop: usize,
    migration_mode: LinuxMigrationMode,
    rollback_policy: RollbackPolicy,
    selected_sw_idx: usize,

    is_running_exec: Arc<AtomicBool>,
    exec_logs: Arc<Mutex<Vec<String>>>,
    hop_summary: Arc<Mutex<Option<SubvolumeHopSummary>>>,
    exec_error: Arc<Mutex<Option<String>>>,
}

impl LinuxWizardApp {
    fn new() -> Self {
        Self {
            should_quit: false,
            request_sudo_reexec: false,
            phase: LinuxPhase::Gate,
            gate_button_idx: 0,
            spinner_tick: 0,
            scan_status: Arc::new(Mutex::new(ScanStatus {
                message: "Bereit zum Linux-System-Scan...".to_string(),
                step: 0,
                total: 4,
            })),
            scan_slot: Arc::new(Mutex::new(None)),
            is_scanning: Arc::new(AtomicBool::new(false)),
            user_profile: get_user_profile_dir(),
            disk_info: None,
            candidates: Vec::new(),
            git_repos: Vec::new(),
            software: Vec::new(),
            selected_payload_idx: 0,
            blueprint_focus: LinuxBlueprintFocus::Distro,
            distros: vec![
                "Arch Linux",
                "Ubuntu 26.04 LTS",
                "Debian 12",
                "Fedora 41",
                "Alpine Linux (Asgard Minimal)",
                "NixOS",
            ],
            selected_distro: 0,
            selected_desktop: 0,
            migration_mode: LinuxMigrationMode::BtrfsSubvolumeHop,
            rollback_policy: RollbackPolicy::KeepRollbackSubvolume,
            selected_sw_idx: 0,
            is_running_exec: Arc::new(AtomicBool::new(false)),
            exec_logs: Arc::new(Mutex::new(Vec::new())),
            hop_summary: Arc::new(Mutex::new(None)),
            exec_error: Arc::new(Mutex::new(None)),
        }
    }

    fn start_scan(&self) {
        if self.is_scanning.load(Ordering::SeqCst) {
            return;
        }
        self.is_scanning.store(true, Ordering::SeqCst);
        let status_arc = Arc::clone(&self.scan_status);
        let slot_arc = Arc::clone(&self.scan_slot);
        let flag = Arc::clone(&self.is_scanning);

        thread::spawn(move || {
            let res = Scanner::scan_with_progress(|msg, step, total| {
                if let Ok(mut lock) = status_arc.lock() {
                    lock.message = msg.to_string();
                    lock.step = step;
                    lock.total = total;
                }
            });
            if let Ok(mut slot) = slot_arc.lock() {
                *slot = Some(res);
            }
            flag.store(false, Ordering::SeqCst);
        });
    }

    fn poll_scan(&mut self) {
        if self.phase == LinuxPhase::Scanning && !self.is_scanning.load(Ordering::SeqCst) {
            if let Ok(mut slot) = self.scan_slot.lock() {
                if let Some(mut scan) = slot.take() {
                    self.user_profile = scan.user_profile_dir;
                    self.disk_info = scan.disk_info;
                    self.git_repos = scan.git_repos;
                    self.software = scan.software;

                    if let Some(disk) = &self.disk_info {
                        if !disk.is_btrfs_subvol_capable {
                            self.migration_mode = LinuxMigrationMode::RamfsFullTransit;
                        }
                    }

                    let git_count = self.git_repos.len();
                    scan.candidates.push(CandidateItem {
                        id: "git_bundle".to_string(),
                        path: PathBuf::from("Git Repositories"),
                        display_name: format!("Git Repositories ({git_count} Repos)"),
                        description: "Alle erkannten Git-Repositories".to_string(),
                        total_bytes: 50 * 1024 * 1024,
                        size_human: format!("~ 50 MB ({git_count} Repos)"),
                        category: CandidateCategory::GitRepositories,
                        selected: true,
                        item_count: git_count,
                    });
                    self.candidates = scan.candidates;
                    self.phase = LinuxPhase::Diagnostics;
                }
            }
        }
    }

    fn start_execution(&self, dry_run: bool) {
        if self.is_running_exec.load(Ordering::SeqCst) {
            return;
        }
        self.is_running_exec.store(true, Ordering::SeqCst);
        if let Ok(mut logs) = self.exec_logs.lock() {
            logs.clear();
            logs.push(if dry_run {
                "[Dry-Run] Starte Linux-Migrations-Simulation...".to_string()
            } else {
                "[Live] Starte Linux-zu-Linux Migration...".to_string()
            });
        }
        if let Ok(mut err) = self.exec_error.lock() {
            *err = None;
        }

        let target_distro = self.distros[self.selected_distro].to_string();
        let desktop_env = DesktopEnvironment::ALL[self.selected_desktop];
        let mode = self.migration_mode;
        let rollback = self.rollback_policy;
        let user_profile = self.user_profile.clone();
        let candidates = self.candidates.clone();
        let git_repos = self.git_repos.clone();
        let software = self.software.clone();
        let disk_info = self.disk_info.clone();

        let logs_arc = Arc::clone(&self.exec_logs);
        let summary_arc = Arc::clone(&self.hop_summary);
        let err_arc = Arc::clone(&self.exec_error);
        let flag = Arc::clone(&self.is_running_exec);

        thread::spawn(move || {
            let res = run_linux_migration(
                &user_profile,
                &target_distro,
                desktop_env,
                mode,
                rollback,
                disk_info.as_ref(),
                &candidates,
                &git_repos,
                &software,
                dry_run,
                |msg| {
                    if let Ok(mut l) = logs_arc.lock() {
                        l.push(msg.to_string());
                    }
                },
            );
            match res {
                Ok(summary_opt) => {
                    if let Ok(mut s) = summary_arc.lock() {
                        *s = summary_opt;
                    }
                    if let Ok(mut l) = logs_arc.lock() {
                        l.push("[✓] Pipeline erfolgreich abgeschlossen!".to_string());
                    }
                }
                Err(e) => {
                    if let Ok(mut err) = err_arc.lock() {
                        *err = Some(e.to_string());
                    }
                }
            }
            flag.store(false, Ordering::SeqCst);
        });
    }
}

fn is_effective_root() -> bool {
    #[cfg(target_os = "linux")]
    {
        if let Ok(out) = Command::new("id").arg("-u").output() {
            return String::from_utf8_lossy(&out.stdout).trim() == "0";
        }
        false
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

fn current_uid_gid() -> (u32, u32) {
    #[cfg(target_os = "linux")]
    {
        if let Ok(sudo_uid) = std::env::var("SUDO_UID") {
            let uid = sudo_uid.trim().parse::<u32>().unwrap_or(1000);
            let gid = std::env::var("SUDO_GID")
                .ok()
                .and_then(|g| g.trim().parse::<u32>().ok())
                .unwrap_or(uid);
            return (uid, gid);
        }
        let uid = Command::new("id")
            .arg("-u")
            .output()
            .ok()
            .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u32>().ok())
            .unwrap_or(1000);
        let gid = Command::new("id")
            .arg("-g")
            .output()
            .ok()
            .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u32>().ok())
            .unwrap_or(uid);
        return (if uid == 0 { 1000 } else { uid }, if gid == 0 { 1000 } else { gid });
    }
    #[cfg(not(target_os = "linux"))]
    (1000, 1000)
}

#[allow(clippy::too_many_arguments)]
fn run_linux_migration<F>(
    user_profile: &Path,
    target_distro: &str,
    desktop_env: DesktopEnvironment,
    mode: LinuxMigrationMode,
    rollback_policy: RollbackPolicy,
    disk_info: Option<&DiskScanInfo>,
    candidates: &[CandidateItem],
    git_repos: &[GitRepoInfo],
    software: &[DiscoveredSoftware],
    dry_run: bool,
    mut log_cb: F,
) -> io::Result<Option<SubvolumeHopSummary>>
where
    F: FnMut(&str),
{
    let username = user_profile
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty() && n != "root")
        .unwrap_or_else(|| "linuxuser".to_string());
    let (uid, gid) = current_uid_gid();
    let luks_uuid = disk_info.and_then(|d| d.luks_underlying_uuid.clone());
    let current_subvol = disk_info
        .and_then(|d| d.root_subvol_name.clone())
        .unwrap_or_else(|| "@root".to_string());
    let esp_mount = disk_info
        .and_then(|d| d.esp_mount_point.clone())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/boot/efi"));

    let stage_dir = if dry_run {
        std::env::temp_dir().join("bifrost_linux_dryrun_stage")
    } else {
        user_profile.join("bifrost_linux_stage")
    };
    fs::create_dir_all(&stage_dir)?;

    let mut stage_logs = Vec::new();
    let (rootfs_path, rootfs_desc) = ensure_distro_rootfs(
        user_profile,
        &stage_dir,
        target_distro,
        &mut stage_logs,
        |m| log_cb(m),
    );
    log_cb(&format!("Rootfs: {rootfs_desc}"));

    let spec = resolve_tier1_distro(target_distro);
    let manifest = MigrationManifest {
        schema_version: "1.0.0".to_string(),
        created_at_utc: 0,
        source_os: disk_info
            .and_then(|d| d.source_distro_pretty.clone())
            .unwrap_or_else(|| "Linux".to_string()),
        target_distro: target_distro.to_string(),
        target_filesystem: "Btrfs (@root, @home, @blackbox, zstd)".to_string(),
        clean_install: rollback_policy == RollbackPolicy::DirectPurgeOldRoot,
        user_profile: user_profile.to_path_buf(),
        target_username: username.clone(),
        selected_software: software.iter().filter(|s| s.will_reinstall).cloned().collect(),
        git_repositories: git_repos.iter().filter(|r| r.selected).cloned().collect(),
        archived_categories: vec![format!("Tier1: {}", spec.slug)],
        total_uncompressed_bytes: 0,
        total_files: 0,
        archive_sha256: None,
        desktop_env: Some(desktop_env),
        linux_migration_mode: Some(mode),
        rollback_policy: Some(rollback_policy),
        source_uid: Some(uid),
        source_gid: Some(gid),
        luks_uuid: luks_uuid.clone(),
    };

    if mode == LinuxMigrationMode::BtrfsSubvolumeHop {
        let btrfs_dev = PathBuf::from("/dev/sda2");
        let esp_dev = PathBuf::from("/dev/sda1");
        let plan = plan_btrfs_subvolume_hop(
            &btrfs_dev,
            &esp_dev,
            &esp_mount,
            "LINUX-ROOT-UUID",
            "LINUX-ESP-UUID",
            &current_subvol,
            target_distro,
            desktop_env,
            rollback_policy,
            luks_uuid.as_deref(),
            &username,
            uid,
            gid,
        )?;
        log_cb(&format!(
            "Subvolume-Hop Plan: {} -> {} (Rollback: {})",
            plan.current_root_subvol,
            plan.new_root_subvol,
            plan.rollback_policy.label()
        ));
        let summary = execute_btrfs_subvolume_hop(
            &plan,
            rootfs_path.as_deref(),
            &manifest,
            dry_run,
            |msg, step, total| log_cb(&format!("[{step}/{total}] {msg}")),
        )?;
        for line in &summary.efi_handoff_logs {
            log_cb(line);
        }
        return Ok(Some(summary));
    }

    // Mode B: RAMFS Full-Transit (pack Blackbox + stage handoff for bifrost-boot.efi)
    let archive_dest = stage_dir.join("bifrost_blackbox.tar.zst");
    log_cb("Packe Blackbox-Archiv fuer RAMFS Full-Transit...");
    let pack_res = create_blackbox_archive_with_mode(
        &archive_dest,
        user_profile,
        candidates,
        git_repos,
        software,
        target_distro,
        "Btrfs (@root, @home, @blackbox, zstd)",
        rollback_policy == RollbackPolicy::DirectPurgeOldRoot,
        |p| {
            if p.files_packed % 50 == 1 {
                log_cb(&format!("Packe ({}): {}", p.files_packed, p.current_file));
            }
        },
    )?;
    let handoff = stage_vm_handoff_with_progress(user_profile, &pack_res, |m| log_cb(m))?;
    log_cb(&format!(
        "RAMFS Transit Stage bereit unter {} ({} Dateien)",
        handoff.stage_dir.display(),
        pack_res.total_files
    ));
    Ok(None)
}

fn handle_cli_args() -> io::Result<Option<i32>> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "Bifrost Linux Migration Engine\n\
             Usage:\n\
               bifroest-linux                             Start interactive TUI wizard\n\
               bifroest-linux --cleanup [--dry-run]       Remove @root_old rollback subvolumes & boot entries\n\
               bifroest-linux --headless-hop --distro <NAME> [--desktop <cli|xfce4|plasma|gnome|hyprland>] [--mode <hop|ramfs>] [--rollback <keep|purge>] [--dry-run]"
        );
        return Ok(Some(0));
    }

    let dry_run = args.iter().any(|a| a == "--dry-run");

    if args.iter().any(|a| a == "--cleanup") {
        println!("[Bifrost Cleanup] Suche inaktive Rollback-Subvolumes & Boot-Eintraege...");
        let (top_mount, entries_dir, active_subvol) = if dry_run {
            let tmp = std::env::temp_dir().join("bifrost_cleanup_dryrun");
            let _ = fs::remove_dir_all(&tmp);
            let top = tmp.join("btrfs_top");
            let entries = tmp.join("loader/entries");
            fs::create_dir_all(top.join("@root_arch"))?;
            fs::create_dir_all(top.join("@root_old"))?;
            fs::create_dir_all(&entries)?;
            fs::write(
                entries.join("bifroest-rollback.conf"),
                "title Bifrost Rollback (@root_old)\noptions root=UUID=1 rootflags=subvol=@root_old rw\n",
            )?;
            (top, entries, "@root_arch".to_string())
        } else {
            let scan = bifroest::scanner::system::scan_system_disk();
            let active = scan
                .as_ref()
                .and_then(|d| d.root_subvol_name.clone())
                .unwrap_or_else(|| "@root".to_string());
            let esp = scan
                .as_ref()
                .and_then(|d| d.esp_mount_point.clone())
                .unwrap_or_else(|| "/boot/efi".to_string());
            (
                PathBuf::from("/run/bifrost_btrfs_top"),
                PathBuf::from(esp).join("loader/entries"),
                active,
            )
        };

        let actions = cleanup_rollback_subvolumes(&top_mount, &entries_dir, &active_subvol, dry_run)?;
        if actions.is_empty() {
            println!("Keine inaktiven Rollback-Reste gefunden.");
        } else {
            for act in actions {
                println!("  - {act}");
            }
        }
        return Ok(Some(0));
    }

    if args.iter().any(|a| a == "--headless-hop") {
        let arg_val = |flag: &str| -> Option<String> {
            args.iter()
                .position(|a| a == flag)
                .and_then(|i| args.get(i + 1))
                .cloned()
        };
        let target_distro = arg_val("--distro").unwrap_or_else(|| "Arch Linux".to_string());
        let desktop_env = arg_val("--desktop")
            .and_then(|s| DesktopEnvironment::from_slug(&s))
            .unwrap_or(DesktopEnvironment::CliOnly);
        let mode = match arg_val("--mode").as_deref() {
            Some("ramfs") => LinuxMigrationMode::RamfsFullTransit,
            _ => LinuxMigrationMode::BtrfsSubvolumeHop,
        };
        let rollback = match arg_val("--rollback").as_deref() {
            Some("purge") => RollbackPolicy::DirectPurgeOldRoot,
            _ => RollbackPolicy::KeepRollbackSubvolume,
        };

        let user_profile = get_user_profile_dir();
        let disk_info = bifroest::scanner::system::scan_system_disk();
        let summary = run_linux_migration(
            &user_profile,
            &target_distro,
            desktop_env,
            mode,
            rollback,
            disk_info.as_ref(),
            &[],
            &[],
            &[],
            dry_run,
            |msg| println!("[Bifrost] {msg}"),
        )?;
        if let Some(s) = summary {
            println!(
                "[OK] Target Subvolume: {} | Desktop: {} | Rollback: {}",
                s.plan.new_root_subvol,
                s.plan.desktop_env.label(),
                s.plan.rollback_policy.label()
            );
        }
        return Ok(Some(0));
    }

    Ok(None)
}

fn render_wizard(f: &mut Frame, app: &LinuxWizardApp) {
    let outer = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(10),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(outer);

    let logo_lines: Vec<Line> = LOGO_TXT
        .lines()
        .take(8)
        .map(|l| Line::from(Span::styled(l, Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD))))
        .collect();
    f.render_widget(
        Paragraph::new(logo_lines)
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Double)
                    .border_style(Style::default().fg(C_BORDER_FOCUS))
                    .title(" BIFRÖST FOR LINUX — ZERO-BIOS LINUX-TO-LINUX MIGRATION ENGINE "),
            ),
        chunks[0],
    );

    let phase_title = match app.phase {
        LinuxPhase::Gate => " Phase 1: Safe Linux Migration Gate ",
        LinuxPhase::Scanning => " Scanning Linux Environment... ",
        LinuxPhase::Diagnostics => " Phase 2: Linux, Btrfs & LUKS2 Diagnostics ",
        LinuxPhase::Payload => " Phase 3: Payload, XDG & Credentials ",
        LinuxPhase::Blueprint => " Phase 4: Target Distro, Desktop & Rollback Blueprint ",
        LinuxPhase::Flightplan => " Phase 5: Flightplan Confirmation ",
        LinuxPhase::Executing => " Phase 6: Live Migration Execution ",
    };
    let main_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(C_BORDER))
        .title(Span::styled(
            phase_title,
            Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD),
        ));

    let mut lines: Vec<Line> = Vec::new();
    match app.phase {
        LinuxPhase::Gate => {
            lines.push(Line::from(Span::styled(
                "Bifröst for Linux uebertraegt deine Daten, XDG-Configs, WLAN & Apps nahtlos zur Ziel-Distro.",
                Style::default().fg(C_TEXT_BRIGHT).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(" • Modus A (Btrfs Subvolume-Hop): @home bleibt Zero-Copy erhalten + Rollback-Eintrag."));
            lines.push(Line::from(" • Modus B (RAMFS Full-Transit): Voll-Umzug ueber bifrost-boot.efi bei Ext4/Wipe."));
            lines.push(Line::from(""));
            let btn_a = if app.gate_button_idx == 0 { " [▶ Scan starten] " } else { "   Scan starten   " };
            let btn_b = if app.gate_button_idx == 1 { " [▶ Beenden] " } else { "   Beenden   " };
            lines.push(Line::from(vec![
                Span::styled(btn_a, Style::default().fg(Color::Black).bg(if app.gate_button_idx == 0 { C_SUCCESS } else { C_MUTED }).add_modifier(Modifier::BOLD)),
                Span::raw("   "),
                Span::styled(btn_b, Style::default().fg(Color::Black).bg(if app.gate_button_idx == 1 { C_WARN_BORDER } else { C_MUTED })),
            ]));
        }
        LinuxPhase::Scanning => {
            let spinner = ["|", "/", "-", "\\"][app.spinner_tick % 4];
            if let Ok(st) = app.scan_status.lock() {
                lines.push(Line::from(format!("[{spinner}] Schritt {}/{}: {}", st.step, st.total, st.message)));
            }
        }
        LinuxPhase::Diagnostics => {
            if let Some(d) = &app.disk_info {
                lines.push(Line::from(format!(
                    "Quell-OS: {} | FS: {} | Btrfs-Hop faehig: {}",
                    d.source_distro_pretty.as_deref().unwrap_or("Linux"),
                    d.file_system,
                    if d.is_btrfs_subvol_capable { "JA (Zero-Copy @home)" } else { "Nein (RAMFS Full-Transit)" }
                )));
                lines.push(Line::from(format!(
                    "Root-Subvolume: {} | Verschluesselung: {}",
                    d.root_subvol_name.as_deref().unwrap_or("-"),
                    d.bitlocker_status
                )));
                lines.push(Line::from(format!(
                    "ESP Mount: {} | Verfuegbar: {}",
                    d.esp_mount_point.as_deref().unwrap_or("nicht erkannt"),
                    format_bytes(d.available_bytes)
                )));
            }
            lines.push(Line::from(format!(
                "Home: {} | Git-Repos: {} | Apps/Flatpaks: {}",
                app.user_profile.display(),
                app.git_repos.len(),
                app.software.len()
            )));
        }
        LinuxPhase::Payload => {
            lines.push(Line::from("Ausgewaehlte Nutzdaten, XDG-Dotfiles & Credentials (Space zum Umschalten):"));
            lines.push(Line::from(""));
            for (idx, item) in app.candidates.iter().enumerate() {
                let mark = if item.selected { "[x]" } else { "[ ]" };
                let ptr = if idx == app.selected_payload_idx { "▶ " } else { "  " };
                lines.push(Line::from(Span::styled(
                    format!("{ptr}{mark} {:<36} ({})", item.display_name, item.size_human),
                    Style::default().fg(if idx == app.selected_payload_idx { C_TEXT_BRIGHT } else { C_TEXT }),
                )));
            }
        }
        LinuxPhase::Blueprint => {
            let f_distro = app.blueprint_focus == LinuxBlueprintFocus::Distro;
            let f_de = app.blueprint_focus == LinuxBlueprintFocus::Desktop;
            let f_mode = app.blueprint_focus == LinuxBlueprintFocus::MigrationMode;
            let f_rb = app.blueprint_focus == LinuxBlueprintFocus::RollbackPolicy;

            lines.push(Line::from(format!(
                "{} Ziel-Distro:     ◀ {} ▶",
                if f_distro { "▶" } else { " " },
                app.distros[app.selected_distro]
            )));
            lines.push(Line::from(format!(
                "{} Desktop:         ◀ {} ▶",
                if f_de { "▶" } else { " " },
                DesktopEnvironment::ALL[app.selected_desktop].label()
            )));
            lines.push(Line::from(format!(
                "{} Migrations-Weg:  ◀ {} ▶",
                if f_mode { "▶" } else { " " },
                app.migration_mode.label()
            )));
            lines.push(Line::from(format!(
                "{} Rollback-Modus:  ◀ {} ▶",
                if f_rb { "▶" } else { " " },
                app.rollback_policy.label()
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(format!(
                "Erkannte Pakete/Flatpaks zur Re-Installation: {} ausgewaehlt (Space zum Umschalten):",
                app.software.iter().filter(|s| s.will_reinstall).count()
            )));
            let in_sw = app.blueprint_focus == LinuxBlueprintFocus::SoftwareList;
            let sw_offset = app.selected_sw_idx.saturating_sub(3);
            for (rel_i, sw) in app.software.iter().skip(sw_offset).take(4).enumerate() {
                let idx = sw_offset + rel_i;
                let ptr = if in_sw && idx == app.selected_sw_idx { "▶ " } else { "  " };
                let mark = if sw.will_reinstall { "[x]" } else { "[ ]" };
                lines.push(Line::from(format!("{ptr}{mark} {} ({})", sw.name, sw.source)));
            }
        }
        LinuxPhase::Flightplan => {
            lines.push(Line::from(Span::styled(
                "FLIGHTPLAN SUMMARY:",
                Style::default().fg(C_WARN_TITLE).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(format!(" • Ziel-Distro:    {}", app.distros[app.selected_distro])));
            lines.push(Line::from(format!(" • Desktop:        {}", DesktopEnvironment::ALL[app.selected_desktop].label())));
            lines.push(Line::from(format!(" • Verfahren:      {}", app.migration_mode.label())));
            lines.push(Line::from(format!(" • Rollback:       {}", app.rollback_policy.label())));
            lines.push(Line::from(format!(
                " • Root-Rechte:    {}",
                if is_effective_root() { "Aktiv (root)" } else { "Nicht root (sudo/Dry-Run in Phase 6)" }
            )));
        }
        LinuxPhase::Executing => {
            if !is_effective_root() && !app.is_running_exec.load(Ordering::SeqCst) && app.exec_logs.lock().map(|l| l.is_empty()).unwrap_or(true) {
                lines.push(Line::from(Span::styled(
                    "Fuer Live-Btrfs/efibootmgr-Mutationen werden Root-Rechte benoetigt.",
                    Style::default().fg(C_WARN_TITLE).add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::from(" [S] Mit sudo -E neu starten  |  [D] Dry-Run Sandbox Simulation starten"));
            }
            if let Ok(logs) = app.exec_logs.lock() {
                for l in logs.iter().rev().take(12).collect::<Vec<_>>().into_iter().rev() {
                    lines.push(Line::from(l.clone()));
                }
            }
            if let Ok(err) = app.exec_error.lock() {
                if let Some(msg) = err.as_ref() {
                    lines.push(Line::from(Span::styled(format!("FEHLER: {msg}"), Style::default().fg(C_WARN_BORDER))));
                }
            }
        }
    }

    f.render_widget(Paragraph::new(lines).block(main_block), chunks[1]);

    let footer_hint = match app.phase {
        LinuxPhase::Gate => "[←/→] Auswaehlen | [Enter] Starten | [Q] Beenden",
        LinuxPhase::Blueprint => "[↑/↓] Fokus | [←/→] Aendern | [Enter] Weiter zum Flugplan | [Esc] Zurueck",
        LinuxPhase::Executing => "[S] sudo -E | [D] Dry-Run | [Q] Beenden",
        _ => "[Enter] Weiter | [Esc] Zurueck | [Q] Beenden",
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(footer_hint, Style::default().fg(C_MUTED))))
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(C_BORDER))),
        chunks[2],
    );
}

fn main() -> io::Result<()> {
    if let Some(code) = handle_cli_args()? {
        std::process::exit(code);
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = LinuxWizardApp::new();

    while !app.should_quit {
        app.spinner_tick = app.spinner_tick.wrapping_add(1);
        app.poll_scan();
        terminal.draw(|f| render_wizard(f, &app))?;

        if event::poll(Duration::from_millis(80))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Char('Q') => app.should_quit = true,
                    _ => match app.phase {
                        LinuxPhase::Gate => match key.code {
                            KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                                app.gate_button_idx = 1 - app.gate_button_idx;
                            }
                            KeyCode::Enter => {
                                if app.gate_button_idx == 0 {
                                    app.phase = LinuxPhase::Scanning;
                                    app.start_scan();
                                } else {
                                    app.should_quit = true;
                                }
                            }
                            _ => {}
                        },
                        LinuxPhase::Scanning => {}
                        LinuxPhase::Diagnostics => match key.code {
                            KeyCode::Enter => app.phase = LinuxPhase::Payload,
                            KeyCode::Esc => app.phase = LinuxPhase::Gate,
                            _ => {}
                        },
                        LinuxPhase::Payload => match key.code {
                            KeyCode::Up => {
                                app.selected_payload_idx = app.selected_payload_idx.saturating_sub(1);
                            }
                            KeyCode::Down => {
                                if app.selected_payload_idx + 1 < app.candidates.len() {
                                    app.selected_payload_idx += 1;
                                }
                            }
                            KeyCode::Char(' ') => {
                                if let Some(c) = app.candidates.get_mut(app.selected_payload_idx) {
                                    c.selected = !c.selected;
                                }
                            }
                            KeyCode::Enter => app.phase = LinuxPhase::Blueprint,
                            KeyCode::Esc => app.phase = LinuxPhase::Diagnostics,
                            _ => {}
                        },
                        LinuxPhase::Blueprint => match key.code {
                            KeyCode::Up => {
                                app.blueprint_focus = match app.blueprint_focus {
                                    LinuxBlueprintFocus::Distro => LinuxBlueprintFocus::Distro,
                                    LinuxBlueprintFocus::Desktop => LinuxBlueprintFocus::Distro,
                                    LinuxBlueprintFocus::MigrationMode => LinuxBlueprintFocus::Desktop,
                                    LinuxBlueprintFocus::RollbackPolicy => LinuxBlueprintFocus::MigrationMode,
                                    LinuxBlueprintFocus::SoftwareList => {
                                        if app.selected_sw_idx > 0 {
                                            app.selected_sw_idx -= 1;
                                            LinuxBlueprintFocus::SoftwareList
                                        } else {
                                            LinuxBlueprintFocus::RollbackPolicy
                                        }
                                    }
                                };
                            }
                            KeyCode::Down => {
                                app.blueprint_focus = match app.blueprint_focus {
                                    LinuxBlueprintFocus::Distro => LinuxBlueprintFocus::Desktop,
                                    LinuxBlueprintFocus::Desktop => LinuxBlueprintFocus::MigrationMode,
                                    LinuxBlueprintFocus::MigrationMode => LinuxBlueprintFocus::RollbackPolicy,
                                    LinuxBlueprintFocus::RollbackPolicy => LinuxBlueprintFocus::SoftwareList,
                                    LinuxBlueprintFocus::SoftwareList => {
                                        if app.selected_sw_idx + 1 < app.software.len() {
                                            app.selected_sw_idx += 1;
                                        }
                                        LinuxBlueprintFocus::SoftwareList
                                    }
                                };
                            }
                            KeyCode::Char(' ') => {
                                if app.blueprint_focus == LinuxBlueprintFocus::SoftwareList {
                                    if let Some(sw) = app.software.get_mut(app.selected_sw_idx) {
                                        sw.will_reinstall = !sw.will_reinstall;
                                    }
                                }
                            }
                            KeyCode::Left => match app.blueprint_focus {
                                LinuxBlueprintFocus::Distro => {
                                    app.selected_distro = if app.selected_distro == 0 {
                                        app.distros.len() - 1
                                    } else {
                                        app.selected_distro - 1
                                    };
                                }
                                LinuxBlueprintFocus::Desktop => {
                                    app.selected_desktop = if app.selected_desktop == 0 {
                                        DesktopEnvironment::ALL.len() - 1
                                    } else {
                                        app.selected_desktop - 1
                                    };
                                }
                                LinuxBlueprintFocus::MigrationMode => {
                                    app.migration_mode = match app.migration_mode {
                                        LinuxMigrationMode::BtrfsSubvolumeHop => LinuxMigrationMode::RamfsFullTransit,
                                        LinuxMigrationMode::RamfsFullTransit => LinuxMigrationMode::BtrfsSubvolumeHop,
                                    };
                                }
                                LinuxBlueprintFocus::RollbackPolicy => {
                                    app.rollback_policy = match app.rollback_policy {
                                        RollbackPolicy::KeepRollbackSubvolume => RollbackPolicy::DirectPurgeOldRoot,
                                        RollbackPolicy::DirectPurgeOldRoot => RollbackPolicy::KeepRollbackSubvolume,
                                    };
                                }
                                LinuxBlueprintFocus::SoftwareList => {}
                            },
                            KeyCode::Right => match app.blueprint_focus {
                                LinuxBlueprintFocus::Distro => {
                                    app.selected_distro = (app.selected_distro + 1) % app.distros.len();
                                }
                                LinuxBlueprintFocus::Desktop => {
                                    app.selected_desktop = (app.selected_desktop + 1) % DesktopEnvironment::ALL.len();
                                }
                                LinuxBlueprintFocus::MigrationMode => {
                                    app.migration_mode = match app.migration_mode {
                                        LinuxMigrationMode::BtrfsSubvolumeHop => LinuxMigrationMode::RamfsFullTransit,
                                        LinuxMigrationMode::RamfsFullTransit => LinuxMigrationMode::BtrfsSubvolumeHop,
                                    };
                                }
                                LinuxBlueprintFocus::RollbackPolicy => {
                                    app.rollback_policy = match app.rollback_policy {
                                        RollbackPolicy::KeepRollbackSubvolume => RollbackPolicy::DirectPurgeOldRoot,
                                        RollbackPolicy::DirectPurgeOldRoot => RollbackPolicy::KeepRollbackSubvolume,
                                    };
                                }
                                LinuxBlueprintFocus::SoftwareList => {}
                            },
                            KeyCode::Enter => app.phase = LinuxPhase::Flightplan,
                            KeyCode::Esc => app.phase = LinuxPhase::Payload,
                            _ => {}
                        },
                        LinuxPhase::Flightplan => match key.code {
                            KeyCode::Enter => {
                                app.phase = LinuxPhase::Executing;
                                if is_effective_root() {
                                    app.start_execution(false);
                                }
                            }
                            KeyCode::Esc => app.phase = LinuxPhase::Blueprint,
                            _ => {}
                        },
                        LinuxPhase::Executing => match key.code {
                            KeyCode::Char('d') | KeyCode::Char('D') => app.start_execution(true),
                            KeyCode::Char('s') | KeyCode::Char('S') => {
                                if !is_effective_root() {
                                    app.request_sudo_reexec = true;
                                    app.should_quit = true;
                                }
                            }
                            _ => {}
                        },
                    },
                }
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    if app.request_sudo_reexec {
        if let Ok(exe) = std::env::current_exe() {
            let status = Command::new("sudo")
                .args(["-E", &exe.to_string_lossy()])
                .status()?;
            std::process::exit(status.code().unwrap_or(0));
        }
    }

    Ok(())
}
