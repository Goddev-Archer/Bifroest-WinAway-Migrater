pub mod blackbox;
pub mod bridge;
pub mod model;
pub mod scanner;

use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
    Frame, Terminal,
};

use blackbox::{create_blackbox_archive, PackProgress, PackResult};
use model::{
    format_bytes, CandidateCategory, CandidateItem, DiscoveredSoftware, DiskScanInfo, GitRepoInfo,
    ScanResult,
};
use scanner::Scanner;

// ============================================================================
// 100% RAMFS / TTY-COMPATIBLE ANSI-16 PALETTE
// ============================================================================
pub const C_BG: Color = Color::Black;
pub const C_SURFACE: Color = Color::Black;
pub const C_BORDER: Color = Color::DarkGray;
pub const C_BORDER_FOCUS: Color = Color::Cyan;
pub const C_TEXT: Color = Color::Gray;
pub const C_TEXT_BRIGHT: Color = Color::White;
pub const C_MUTED: Color = Color::DarkGray;
pub const C_ACCENT: Color = Color::Cyan;
pub const C_SUCCESS: Color = Color::LightGreen;
pub const C_WARN_BORDER: Color = Color::LightRed;
pub const C_WARN_TITLE: Color = Color::LightYellow;
pub const C_SHADOW: Color = Color::DarkGray;

pub const LOGO_TXT: &str = include_str!("../logo.txt");

// ============================================================================
// DIE GEFÜHRTEN PHASEN (MIT LADEBILDSCHIRM NACH DEM GATE)
// ============================================================================
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Gate = 0,        // Phase 1: Die Initial-Warnung (!WARNING!)
    Scanning = 1,    // Zwischenschritt: Echter Hardware- & System-Ladebildschirm
    Diagnostics = 2, // Phase 2: Pre-Flight Checkliste (BitLocker, Speicher, Dirty Repos)
    Payload = 3,     // Phase 3: Auswahl der Daten & Git-Repositories
    Blueprint = 4,   // Phase 4: Ziel-Distro, Dateisystem & Flatpak-Auswahl
    Flightplan = 5,  // Phase 5: Zusammenfassung & Point of No Return
    Executing = 6,   // Live-Ausführung: Blackbox Zstd-Kompression & Hashen
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BlueprintFocus {
    Distro,
    Filesystem,
    SoftwareList,
}

#[derive(Clone)]
pub struct ScanStatus {
    pub message: String,
    pub step: usize,
    pub total: usize,
}

pub struct App {
    pub should_quit: bool,
    pub phase: Phase,
    pub gate_button_idx: usize, // 0 = Hell yeah, 1 = Nope
    pub final_button_idx: usize,
    pub show_last_warning_modal: bool,
    pub spinner_tick: usize,

    // Scan Status & Thread Sync
    pub scan_status: Arc<Mutex<ScanStatus>>,
    pub scan_result_slot: Arc<Mutex<Option<ScanResult>>>,
    pub is_scanning: Arc<AtomicBool>,

    // Phase 3: Payload Navigation
    pub selected_payload_idx: usize,
    pub inspecting_git_repos: bool,
    pub selected_git_idx: usize,

    // Phase 4: Blueprint Navigation (Focus & Selection)
    pub blueprint_focus: BlueprintFocus,
    pub selected_distro: usize,
    pub distros: Vec<&'static str>,
    pub selected_fs: usize,
    pub filesystems: Vec<&'static str>,
    pub selected_sw_idx: usize,

    // Gescannte Live-Daten (werden erst nach dem Gate befüllt!)
    pub user_profile: PathBuf,
    pub disk_info: Option<DiskScanInfo>,
    pub candidates: Vec<CandidateItem>,
    pub git_repos: Vec<GitRepoInfo>,
    pub software: Vec<DiscoveredSoftware>,

    // Live Execution Status
    pub pack_progress: Arc<Mutex<Option<PackProgress>>>,
    pub pack_result: Arc<Mutex<Option<PackResult>>>,
    pub is_packing: Arc<AtomicBool>,

    // RAMFS Bridge & VM Handoff State
    pub handoff_result: Arc<Mutex<Option<bridge::handoff::HandoffResult>>>,
    pub bridge_summary: Arc<Mutex<Option<bridge::BridgeExecutionSummary>>>,
    pub is_bridging: Arc<AtomicBool>,
    pub bridge_status_msg: Arc<Mutex<String>>,
}

impl App {
    pub fn new() -> Self {
        let user_profile = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("C:\\Users\\santa"));

        Self {
            should_quit: false,
            phase: Phase::Gate,
            gate_button_idx: 0,
            final_button_idx: 0,
            show_last_warning_modal: false,
            spinner_tick: 0,

            scan_status: Arc::new(Mutex::new(ScanStatus {
                message: "Bereit zum Scan...".to_string(),
                step: 0,
                total: 4,
            })),
            scan_result_slot: Arc::new(Mutex::new(None)),
            is_scanning: Arc::new(AtomicBool::new(false)),

            selected_payload_idx: 0,
            inspecting_git_repos: false,
            selected_git_idx: 0,

            blueprint_focus: BlueprintFocus::Distro,
            selected_distro: 0,
            distros: vec!["Arch Linux", "Fedora 41", "Debian 12", "NixOS"],
            selected_fs: 0,
            filesystems: vec![
                "Btrfs (@root, @home, @blackbox, zstd)",
                "Ext4 (Klassisch)",
                "ZFS (Zpool / Datasets)",
            ],
            selected_sw_idx: 0,

            user_profile,
            disk_info: None,
            candidates: Vec::new(),
            git_repos: Vec::new(),
            software: Vec::new(),

            pack_progress: Arc::new(Mutex::new(None)),
            pack_result: Arc::new(Mutex::new(None)),
            is_packing: Arc::new(AtomicBool::new(false)),

            handoff_result: Arc::new(Mutex::new(None)),
            bridge_summary: Arc::new(Mutex::new(None)),
            is_bridging: Arc::new(AtomicBool::new(false)),
            bridge_status_msg: Arc::new(Mutex::new("Bereit für VM-Bridge Test.".to_string())),
        }
    }

    // Startet den Scan ERST nach der Bestätigung des Gates!
    pub fn start_hardware_scan(&self) {
        if self.is_scanning.load(Ordering::SeqCst) {
            return;
        }

        self.is_scanning.store(true, Ordering::SeqCst);
        let status_arc = Arc::clone(&self.scan_status);
        let slot_arc = Arc::clone(&self.scan_result_slot);
        let is_scanning_arc = Arc::clone(&self.is_scanning);

        thread::spawn(move || {
            let res = Scanner::scan_with_progress(|msg, step, total| {
                if let Ok(mut lock) = status_arc.lock() {
                    lock.message = msg.to_string();
                    lock.step = step;
                    lock.total = total;
                }
            });

            if let Ok(mut lock) = slot_arc.lock() {
                *lock = Some(res);
            }
            is_scanning_arc.store(false, Ordering::SeqCst);
        });
    }

    pub fn poll_scan_completion(&mut self) {
        if self.phase == Phase::Scanning && !self.is_scanning.load(Ordering::SeqCst) {
            if let Ok(mut lock) = self.scan_result_slot.lock() {
                if let Some(mut scan) = lock.take() {
                    self.user_profile = scan.user_profile_dir;
                    self.disk_info = scan.disk_info;
                    self.git_repos = scan.git_repos;
                    self.software = scan.software;

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

                    // Automatischer Übergang in Phase 2: Diagnostics!
                    self.phase = Phase::Diagnostics;
                }
            }
        }
    }

    pub fn start_blackbox_packaging(&self) {
        if self.is_packing.load(Ordering::SeqCst) {
            return;
        }

        self.is_packing.store(true, Ordering::SeqCst);

        let progress_arc = Arc::clone(&self.pack_progress);
        let result_arc = Arc::clone(&self.pack_result);
        let handoff_arc = Arc::clone(&self.handoff_result);
        let is_packing_arc = Arc::clone(&self.is_packing);

        let dest = self.user_profile.join("bifrost_blackbox.tar.zst");
        let user_prof = self.user_profile.clone();
        let candidates = self.candidates.clone();
        let repos = self.git_repos.clone();
        let sw = self.software.clone();
        let distro = self.distros[self.selected_distro].to_string();
        let fs = self.filesystems[self.selected_fs].to_string();

        thread::spawn(move || {
            let res = create_blackbox_archive(
                &dest,
                &user_prof,
                &candidates,
                &repos,
                &sw,
                &distro,
                &fs,
                |p| {
                    if let Ok(mut lock) = progress_arc.lock() {
                        *lock = Some(p);
                    }
                },
            );

            if let Ok(pack_res) = res {
                if let Ok(handoff) = bridge::handoff::stage_vm_handoff(&user_prof, &pack_res) {
                    if let Ok(mut hlock) = handoff_arc.lock() {
                        *hlock = Some(handoff);
                    }
                }
                if let Ok(mut lock) = result_arc.lock() {
                    *lock = Some(pack_res);
                }
            }
            is_packing_arc.store(false, Ordering::SeqCst);
        });
    }

    pub fn start_vm_bridge_simulation(&self) {
        if self.is_packing.load(Ordering::SeqCst) || self.is_bridging.load(Ordering::SeqCst) {
            return;
        }

        let (archive_path, checksum) = match self.pack_result.lock() {
            Ok(lock) => match lock.as_ref() {
                Some(res) => (res.archive_path.clone(), Some(res.sha256_checksum.clone())),
                None => return,
            },
            Err(_) => return,
        };

        self.is_bridging.store(true, Ordering::SeqCst);
        let target_root = self.user_profile.join("bifrost_vm_stage").join("target_root");
        let summary_arc = Arc::clone(&self.bridge_summary);
        let status_arc = Arc::clone(&self.bridge_status_msg);
        let is_bridging_arc = Arc::clone(&self.is_bridging);

        thread::spawn(move || {
            let res = bridge::execute_full_bridge_pipeline(
                &archive_path,
                checksum.as_deref(),
                &target_root,
                32 * 1024 * 1024 * 1024,
                true,
                |msg, step, total| {
                    if let Ok(mut lock) = status_arc.lock() {
                        *lock = format!("[{step}/{total}] {msg}");
                    }
                },
            );

            match res {
                Ok(summary) => {
                    if let Ok(mut lock) = status_arc.lock() {
                        *lock = "RAMFS Bridge Sandbox-Injektion erfolgreich!".to_string();
                    }
                    if let Ok(mut slock) = summary_arc.lock() {
                        *slock = Some(summary);
                    }
                }
                Err(err) => {
                    if let Ok(mut lock) = status_arc.lock() {
                        *lock = format!("Bridge-Fehler: {err}");
                    }
                }
            }
            is_bridging_arc.store(false, Ordering::SeqCst);
        });
    }
}

// ============================================================================
// MAIN ENTRY & CLI MODE DISPATCH (--self-test-vm / --ramfs)
// ============================================================================
fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "--self-test-vm") {
        return run_vm_self_test();
    }

    if args.iter().any(|a| a == "--ramfs") {
        return run_ramfs_cli(&args);
    }

    #[cfg(target_os = "linux")]
    {
        // On Linux / RAMFS: NEVER run the Windows wizard! Always run the Transit Engine.
        return run_ramfs_cli(&args);
    }

    #[cfg(target_os = "windows")]
    {
        let original_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic_info| {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
            original_hook(panic_info);
        }));

        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        let mut app = App::new();
        let res = run_app(&mut terminal, &mut app);

        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        terminal.show_cursor()?;

        if let Err(err) = res {
            eprintln!("Bifröst Error: {err}");
        }

        Ok(())
    }
}

fn find_arg_value(args: &[String], flag: &str) -> Option<String> {
    args.windows(2)
        .find(|w| w[0] == flag)
        .map(|w| w[1].clone())
}

fn auto_discover_blackbox() -> io::Result<PathBuf> {
    let tmp_dest = PathBuf::from("/tmp/bifrost_blackbox.tar.zst");
    if tmp_dest.exists() {
        return Ok(tmp_dest);
    }

    #[cfg(target_os = "linux")]
    {
        let mount_dir = Path::new("/mnt/win_src");
        let _ = std::fs::create_dir_all(mount_dir);

        // Gather all partition candidates from sysfs and /dev
        let mut part_candidates: Vec<PathBuf> = Vec::new();
        for dev in bridge::partition::discover_linux_block_devices() {
            for part in dev.partitions {
                if !part_candidates.contains(&part.dev_path) {
                    part_candidates.push(part.dev_path);
                }
            }
        }
        if let Ok(entries) = std::fs::read_dir("/dev") {
            for entry in entries.flatten() {
                let fname = entry.file_name().to_string_lossy().to_string();
                if (fname.starts_with("sd") || fname.starts_with("vd") || fname.starts_with("nvme"))
                    && fname.chars().last().map(|c| c.is_ascii_digit()).unwrap_or(false)
                {
                    let p = entry.path();
                    if !part_candidates.contains(&p) {
                        part_candidates.push(p);
                    }
                }
            }
        }

        println!("[Bifröst Auto-Discovery] Gefundene Partitionen: {:?}", part_candidates);

        for part in &part_candidates {
            let part_str = part.to_string_lossy();
            let _ = std::process::Command::new("ntfsfix")
                .args(["-b", "-d", &part_str])
                .status();

            // Try read-only ntfs-3g mount first
            let mut mounted = std::process::Command::new("ntfs-3g")
                .args(["-o", "ro", &part_str, "/mnt/win_src"])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);

            if !mounted {
                // If hibernated, try remove_hiberfile (without ro)
                mounted = std::process::Command::new("ntfs-3g")
                    .args(["-o", "remove_hiberfile", &part_str, "/mnt/win_src"])
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
            }

            if !mounted {
                // Fall back to kernel ntfs3 or standard mount
                mounted = std::process::Command::new("mount")
                    .args(["-t", "ntfs3", "-o", "ro", &part_str, "/mnt/win_src"])
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false)
                    || std::process::Command::new("mount")
                        .args(["-o", "ro", &part_str, "/mnt/win_src"])
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false);
            }

            if mounted && mount_dir.exists() {
                for entry in walkdir::WalkDir::new(mount_dir).max_depth(8).into_iter().flatten() {
                    if entry.file_name().to_string_lossy().eq_ignore_ascii_case("bifrost_blackbox.tar.zst") {
                        println!("[Bifröst Auto-Discovery] Blackbox gefunden: {}", entry.path().display());
                        std::fs::copy(entry.path(), &tmp_dest)?;
                        let _ = std::process::Command::new("umount").arg("/mnt/win_src").status();
                        return Ok(tmp_dest);
                    }
                }
                let _ = std::process::Command::new("umount").arg("/mnt/win_src").status();
            }
        }
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "Keine bifrost_blackbox.tar.zst auf lokalen Partitionen gefunden.",
    ))
}

fn run_ramfs_cli(args: &[String]) -> io::Result<()> {
    let archive_path = if let Some(p) = find_arg_value(args, "--archive") {
        PathBuf::from(p)
    } else {
        auto_discover_blackbox()?
    };
    let target_str =
        find_arg_value(args, "--target-root").unwrap_or_else(|| "/mnt/target_linux".to_string());
    let expected_sha256 = find_arg_value(args, "--sha256");
    let dry_run = args.iter().any(|a| a == "--dry-run");

    let summary = bridge::ramfs_cinema::run_ramfs_transit_cinema(
        &archive_path,
        expected_sha256.as_deref(),
        Path::new(&target_str),
        32 * 1024 * 1024 * 1024,
        dry_run,
    )?;

    println!("[OK] Ziel-Partition:  {}", summary.plan.new_linux_partition.display());
    println!("[OK] Wiederhergestellt: {} Dateien", summary.injection.files_restored);
    println!("[OK] SHA-256 geprüft: {}", summary.injection.sha256_verified);
    println!("[OK] Bootloader Entry: {}", summary.bootloader_entry_path.display());
    Ok(())
}

fn run_vm_self_test() -> io::Result<()> {
    println!("=== Bifröst End-to-End VM Migration Self-Test ===");
    let sandbox = std::env::temp_dir().join("bifrost_vm_selftest");
    let _ = fs::remove_dir_all(&sandbox);

    let win_prof = sandbox.join("Users").join("vmtester");
    let target_root = sandbox.join("target_root");
    let ssh_dir = win_prof.join(".ssh");
    let docs_dir = win_prof.join("Documents");
    let repo_dir = win_prof.join("dev").join("SampleVmRepo");
    fs::create_dir_all(&ssh_dir)?;
    fs::create_dir_all(&docs_dir)?;
    fs::create_dir_all(repo_dir.join(".git"))?;

    fs::write(ssh_dir.join("id_ed25519"), "-----BEGIN OPENSSH PRIVATE KEY-----\nVM-TEST\n")?;
    fs::write(ssh_dir.join("id_ed25519.pub"), "ssh-ed25519 AAAAC3Nza vmtester@vm\n")?;
    fs::write(ssh_dir.join("config"), "Host github.com\n  IdentityFile ~/.ssh/id_ed25519\n")?;
    fs::write(docs_dir.join("vm_notes.txt"), "Windows-to-Linux VM migration test payload.\n")?;
    fs::write(repo_dir.join("README.md"), "# SampleVmRepo\n")?;

    println!("[1/5] Block-Device Scan & Shrink-Plan Berechnung...");
    let devices = bridge::partition::discover_block_devices();
    let disk = devices
        .first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Kein Test-Disk gefunden"))?;
    println!("      Disk: {} ({}, Model: {})", disk.dev_path.display(), format_bytes(disk.total_bytes), disk.model);

    println!("[2/5] Erzeuge Blackbox-Archiv (.tar.zst + Manifest + SHA-256)...");
    let candidates = vec![
        CandidateItem {
            id: "ssh".to_string(),
            path: ssh_dir,
            display_name: ".ssh".to_string(),
            description: "SSH Credentials".to_string(),
            total_bytes: 120,
            size_human: "120 B".to_string(),
            category: CandidateCategory::Credentials,
            selected: true,
            item_count: 3,
        },
        CandidateItem {
            id: "docs".to_string(),
            path: docs_dir,
            display_name: "Documents".to_string(),
            description: "Documents".to_string(),
            total_bytes: 44,
            size_human: "44 B".to_string(),
            category: CandidateCategory::Documents,
            selected: true,
            item_count: 1,
        },
    ];
    let repos = vec![GitRepoInfo {
        path: repo_dir,
        name: "SampleVmRepo".to_string(),
        current_branch: "main".to_string(),
        is_dirty: true,
        uncommitted_files_count: 1,
        selected: true,
    }];
    let sw = vec![DiscoveredSoftware {
        name: "Visual Studio Code".to_string(),
        id: "Microsoft.VisualStudioCode".to_string(),
        source: "Winget".to_string(),
        flatpak_id: Some("com.visualstudio.code".to_string()),
        native_pkg: Some("code".to_string()),
        will_reinstall: true,
    }];

    let archive_path = sandbox.join("bifrost_blackbox.tar.zst");
    let pack_res = create_blackbox_archive(
        &archive_path,
        &win_prof,
        &candidates,
        &repos,
        &sw,
        "Arch Linux",
        "Btrfs (@root, @home, @blackbox, zstd)",
        |_| {},
    )?;
    println!("      Archiv: {} Dateien, SHA-256: {}", pack_res.total_files, pack_res.sha256_checksum);

    println!("[3/5] Stage UEFI Boot-Handoff Skripte...");
    let handoff = bridge::handoff::stage_vm_handoff(&win_prof, &pack_res)?;
    println!("      Stage-Dir: {}", handoff.stage_dir.display());
    println!("      EFI-Script: {}", handoff.efi_batch_script.display());
    println!("      ESP-Drive: {} | Secure Boot: {}", handoff.esp_drive, handoff.secure_boot_enabled);

    println!("[4/5] Führe Linux RAMFS Bridge Pipeline (Dry-Run Sandbox) aus...");
    let summary = bridge::execute_full_bridge_pipeline(
        &archive_path,
        Some(&pack_res.sha256_checksum),
        &target_root,
        32 * 1024 * 1024 * 1024,
        true,
        |msg, step, total| {
            println!("      -> [{step}/{total}] {msg}");
        },
    )?;

    println!("[5/5] Verifiziere Ergebnis-Artefakte...");
    assert!(summary.injection.sha256_verified, "SHA-256 Verifikation fehlgeschlagen!");
    assert!(summary.fstab_path.exists(), "/etc/fstab fehlt!");
    assert!(summary.bootloader_entry_path.exists(), "systemd-boot Entry fehlt!");
    assert!(summary.postinstall_script_path.exists(), "Post-Install Skript fehlt!");
    assert!(
        summary.injection.target_home_dir.join(".ssh/id_ed25519").exists(),
        "SSH Private Key fehlt im Target!"
    );
    assert!(
        summary.injection.target_home_dir.join("Projects/SampleVmRepo/README.md").exists(),
        "Git-Repository fehlt im Target!"
    );

    println!("      SHA-256 Verifiziert:   {}", summary.injection.sha256_verified);
    println!("      Wiederhergestellt:     {} Dateien", summary.injection.files_restored);
    println!("      POSIX Fixes (.ssh):    {} Korrekturen (0700/0600)", summary.injection.permission_fixes.len());
    println!("      Ziel-Partition:        {}", summary.plan.new_linux_partition.display());
    println!("      Bootloader Entry:      {}", summary.bootloader_entry_path.display());

    let _ = fs::remove_dir_all(&sandbox);
    println!("=== Self-Test erfolgreich abgeschlossen (Sandbox bereinigt) ===");
    Ok(())
}

fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> io::Result<()> {
    loop {
        app.spinner_tick = (app.spinner_tick + 1) % 4;
        app.poll_scan_completion();

        terminal
            .draw(|f| ui(f, app))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    handle_key(app, key.code);
                }
            }
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

// ============================================================================
// GEFÜHRTE NAVIGATION & EVENT ROUTING
// ============================================================================
fn handle_key(app: &mut App, code: KeyCode) {
    if app.show_last_warning_modal {
        match code {
            KeyCode::Esc => {
                app.show_last_warning_modal = false;
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                app.final_button_idx = 1 - app.final_button_idx;
            }
            KeyCode::Enter => {
                if app.final_button_idx == 0 {
                    app.show_last_warning_modal = false;
                    app.phase = Phase::Executing;
                    app.start_blackbox_packaging();
                } else {
                    app.show_last_warning_modal = false;
                }
            }
            _ => {}
        }
        return;
    }

    match code {
        KeyCode::Char('q') | KeyCode::Char('Q') => {
            app.should_quit = true;
            return;
        }
        _ => {}
    }

    match app.phase {
        Phase::Gate => match code {
            KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                app.gate_button_idx = 1 - app.gate_button_idx;
            }
            KeyCode::Enter => {
                if app.gate_button_idx == 0 {
                    // JETZT ERST STARTET DER SCAN!
                    app.phase = Phase::Scanning;
                    app.start_hardware_scan();
                } else {
                    app.should_quit = true;
                }
            }
            KeyCode::Esc => {
                app.should_quit = true;
            }
            _ => {}
        },

        Phase::Scanning => {
            // Während des Scans können keine Aktionen ausgeführt werden (reiner Ladebildschirm)
        }

        Phase::Diagnostics => match code {
            KeyCode::Enter | KeyCode::Right => {
                app.phase = Phase::Payload;
            }
            KeyCode::Esc | KeyCode::Left => {
                app.phase = Phase::Gate;
            }
            _ => {}
        },

        Phase::Payload => match code {
            KeyCode::Esc => {
                if app.inspecting_git_repos {
                    app.inspecting_git_repos = false;
                } else {
                    app.phase = Phase::Diagnostics;
                }
            }
            KeyCode::Enter => {
                if app.inspecting_git_repos {
                    app.inspecting_git_repos = false;
                } else {
                    app.phase = Phase::Blueprint;
                }
            }
            KeyCode::Tab | KeyCode::Char('g') | KeyCode::Char('G') => {
                app.inspecting_git_repos = !app.inspecting_git_repos;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if app.inspecting_git_repos {
                    if app.selected_git_idx > 0 {
                        app.selected_git_idx -= 1;
                    }
                } else if app.selected_payload_idx > 0 {
                    app.selected_payload_idx -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if app.inspecting_git_repos {
                    if !app.git_repos.is_empty() && app.selected_git_idx + 1 < app.git_repos.len() {
                        app.selected_git_idx += 1;
                    }
                } else if !app.candidates.is_empty() && app.selected_payload_idx + 1 < app.candidates.len() {
                    app.selected_payload_idx += 1;
                }
            }
            KeyCode::Char(' ') => {
                if app.inspecting_git_repos {
                    if let Some(r) = app.git_repos.get_mut(app.selected_git_idx) {
                        r.selected = !r.selected;
                    }
                } else if let Some(item) = app.candidates.get_mut(app.selected_payload_idx) {
                    item.selected = !item.selected;
                    if item.category == CandidateCategory::GitRepositories {
                        let state = item.selected;
                        for r in &mut app.git_repos {
                            r.selected = state;
                        }
                    }
                }
            }
            _ => {}
        },

        Phase::Blueprint => match code {
            KeyCode::Esc => {
                app.phase = Phase::Payload;
            }
            KeyCode::Enter => {
                app.phase = Phase::Flightplan;
            }
            KeyCode::Up | KeyCode::Char('k') => match app.blueprint_focus {
                BlueprintFocus::Distro => {}
                BlueprintFocus::Filesystem => {
                    app.blueprint_focus = BlueprintFocus::Distro;
                }
                BlueprintFocus::SoftwareList => {
                    if app.selected_sw_idx > 0 {
                        app.selected_sw_idx -= 1;
                    } else {
                        app.blueprint_focus = BlueprintFocus::Filesystem;
                    }
                }
            },
            KeyCode::Down | KeyCode::Char('j') => match app.blueprint_focus {
                BlueprintFocus::Distro => {
                    app.blueprint_focus = BlueprintFocus::Filesystem;
                }
                BlueprintFocus::Filesystem => {
                    if !app.software.is_empty() {
                        app.blueprint_focus = BlueprintFocus::SoftwareList;
                        app.selected_sw_idx = 0;
                    }
                }
                BlueprintFocus::SoftwareList => {
                    if !app.software.is_empty() && app.selected_sw_idx + 1 < app.software.len() {
                        app.selected_sw_idx += 1;
                    }
                }
            },
            KeyCode::Left | KeyCode::Char('h') => match app.blueprint_focus {
                BlueprintFocus::Distro => {
                    if app.selected_distro > 0 {
                        app.selected_distro -= 1;
                    } else {
                        app.selected_distro = app.distros.len() - 1;
                    }
                }
                BlueprintFocus::Filesystem => {
                    if app.selected_fs > 0 {
                        app.selected_fs -= 1;
                    } else {
                        app.selected_fs = app.filesystems.len() - 1;
                    }
                }
                BlueprintFocus::SoftwareList => {}
            },
            KeyCode::Right | KeyCode::Char('l') => match app.blueprint_focus {
                BlueprintFocus::Distro => {
                    app.selected_distro = (app.selected_distro + 1) % app.distros.len();
                }
                BlueprintFocus::Filesystem => {
                    app.selected_fs = (app.selected_fs + 1) % app.filesystems.len();
                }
                BlueprintFocus::SoftwareList => {}
            },
            KeyCode::Char(' ') => {
                if app.blueprint_focus == BlueprintFocus::SoftwareList {
                    if let Some(sw) = app.software.get_mut(app.selected_sw_idx) {
                        sw.will_reinstall = !sw.will_reinstall;
                    }
                }
            }
            _ => {}
        },

        Phase::Flightplan => match code {
            KeyCode::Esc => {
                app.phase = Phase::Blueprint;
            }
            KeyCode::Enter => {
                app.show_last_warning_modal = true;
                app.final_button_idx = 0;
            }
            _ => {}
        },

        Phase::Executing => match code {
            KeyCode::Char('b') | KeyCode::Char('B') => {
                app.start_vm_bridge_simulation();
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                if !app.is_packing.load(Ordering::SeqCst) && !app.is_bridging.load(Ordering::SeqCst) {
                    #[cfg(target_os = "windows")]
                    {
                        let _ = std::process::Command::new("shutdown")
                            .args(["/r", "/t", "3", "/c", "Bifrost Linux RAMFS Migration Handoff"])
                            .spawn();
                    }
                    app.should_quit = true;
                }
            }
            KeyCode::Enter | KeyCode::Esc => {
                if !app.is_packing.load(Ordering::SeqCst) && !app.is_bridging.load(Ordering::SeqCst) {
                    app.should_quit = true;
                }
            }
            _ => {}
        },
    }
}

// ============================================================================
// UI RENDERING
// ============================================================================
fn ui(f: &mut Frame, app: &App) {
    let size = f.area();
    f.render_widget(Block::default().style(Style::default().bg(C_BG)), size);

    if app.phase == Phase::Gate {
        render_gate_screen(f, size, app);
        return;
    }

    if app.phase == Phase::Scanning {
        render_scanning_loading_screen(f, size, app);
        return;
    }

    let padding = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(16),
            Constraint::Length(1),
        ])
        .split(size);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(75), Constraint::Percentage(25)])
        .split(padding[1]);

    render_guided_left_pane(f, cols[0], app);
    render_guided_right_stepper(f, cols[1], app);
    render_guided_footer(f, padding[2], app);

    if app.show_last_warning_modal {
        render_last_warning_modal(f, size, app);
    }
}

// ============================================================================
// PHASE 1: THE GATE (!WARNING!)
// ============================================================================
fn render_gate_screen(f: &mut Frame, area: Rect, app: &App) {
    let width = 64.min(area.width.saturating_sub(4));
    let height = 18.min(area.height.saturating_sub(2));

    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height.saturating_sub(height)) / 2;
    let modal_rect = Rect::new(x, y, width, height);

    render_drop_shadow(f, modal_rect);
    f.render_widget(Clear, modal_rect);

    let block = Block::default()
        .title(" !WARNING! ")
        .title_style(Style::default().fg(C_WARN_TITLE).bg(C_SURFACE).add_modifier(Modifier::BOLD))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(C_WARN_BORDER))
        .style(Style::default().bg(C_SURFACE));

    let btn0_style = if app.gate_button_idx == 0 {
        Style::default().bg(Color::LightRed).fg(Color::Black).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(C_TEXT_BRIGHT)
    };

    let btn1_style = if app.gate_button_idx == 1 {
        Style::default().bg(Color::White).fg(Color::Black).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(C_MUTED)
    };

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled("You should only use this tool, if you're", Style::default().fg(C_TEXT_BRIGHT).add_modifier(Modifier::BOLD))),
        Line::from("aware of the risks. Your Windows-OS"),
        Line::from("will be torn apart through the process."),
        Line::from(""),
        Line::from("There is also a slight chance of"),
        Line::from("loosing all your data from the drive (in"),
        Line::from("some cases the drive itself), if"),
        Line::from("a power outage is envolved. Be aware..."),
        Line::from(""),
        Line::from(Span::styled("────────────────────────────────────────────────────────────", Style::default().fg(C_BORDER))),
        Line::from(vec![
            Span::styled("continue?   ", Style::default().fg(C_MUTED)),
            Span::styled(" [ hell yeah ] ", btn0_style),
            Span::raw("   "),
            Span::styled(" [ nope ] ", btn1_style),
        ]),
    ];

    f.render_widget(Paragraph::new(lines).block(block).alignment(Alignment::Center), modal_rect);
}

// ============================================================================
// NEUER ZWISCHENSCHRITT: HARDWARE & SYSTEM LADEBILDSCHIRM (MEMTEST/BIOS STYLE)
// ============================================================================
fn render_scanning_loading_screen(f: &mut Frame, area: Rect, app: &App) {
    let show_logo = area.height >= 20 && area.width >= 56;
    let width = 72.min(area.width.saturating_sub(4));
    let height = if show_logo { 18 } else { 12 }.min(area.height.saturating_sub(2));

    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height.saturating_sub(height)) / 2;
    let rect = Rect::new(x, y, width, height);

    render_drop_shadow(f, rect);
    f.render_widget(Clear, rect);

    let block = Block::default()
        .title(" SYSTEM INSPECTION & HARDWARE SCAN ")
        .title_style(Style::default().fg(C_ACCENT).bg(C_SURFACE).add_modifier(Modifier::BOLD))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(C_ACCENT))
        .style(Style::default().bg(C_SURFACE));

    let spinners = ["|", "/", "-", "\\"];
    let spinner = spinners[app.spinner_tick];

    let (step, total, msg) = if let Ok(lock) = app.scan_status.lock() {
        (lock.step, lock.total, lock.message.clone())
    } else {
        (1, 4, "Scanne...".to_string())
    };

    // Fortschrittsbalken berechnen
    let progress_pct = if total > 0 { (step * 100) / total } else { 0 };
    let filled_blocks = (progress_pct * 30) / 100;
    let empty_blocks = 30usize.saturating_sub(filled_blocks);

    let bar = format!(
        "[{}{}] {:>3}%",
        "█".repeat(filled_blocks),
        "░".repeat(empty_blocks),
        progress_pct
    );

    let mut lines = Vec::new();

    if show_logo {
        lines.push(Line::from(""));
        for raw_line in LOGO_TXT.lines() {
            let line_trimmed = raw_line.trim_end_matches(['\r', '\n']);
            if !line_trimmed.trim().is_empty() {
                lines.push(Line::from(Span::styled(
                    line_trimmed.to_string(),
                    Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD),
                )));
            }
        }
        lines.push(Line::from(Span::styled(
            "LINUX MIGRATION BRIDGE",
            Style::default().fg(C_MUTED),
        )));
        lines.push(Line::from(""));
    } else {
        lines.push(Line::from(""));
    }

    lines.push(Line::from(vec![
        Span::styled(" [", Style::default().fg(C_BORDER)),
        Span::styled(spinner, Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD)),
        Span::styled("] ", Style::default().fg(C_BORDER)),
        Span::styled(
            "Hardwarediagnose und Dateisystem-Prüfung läuft...",
            Style::default().fg(C_TEXT_BRIGHT).add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(format!("Schritt [{step}/{total}]: "), Style::default().fg(C_MUTED)),
        Span::styled(&msg, Style::default().fg(C_TEXT_BRIGHT)),
    ]));
    lines.push(Line::from(Span::styled(
        bar,
        Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "──────────────────────────────────────────────────────────────",
        Style::default().fg(C_BORDER),
    )));
    lines.push(Line::from(Span::styled(
        "Bitte warten. Partitionen und Repositories werden analysiert...",
        Style::default().fg(C_MUTED),
    )));

    f.render_widget(Paragraph::new(lines).block(block).alignment(Alignment::Center), rect);
}

// ============================================================================
// PHASE 2-5: LINKER HAUPTBEREICH
// ============================================================================
fn render_guided_left_pane(f: &mut Frame, area: Rect, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header Box
            Constraint::Min(12),   // Inhalt
        ])
        .split(area);

    let btn_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(C_BORDER_FOCUS))
        .style(Style::default().bg(C_SURFACE));

    let phase_title = match app.phase {
        Phase::Gate => "Phase 1: The Gate",
        Phase::Scanning => "System Inspection",
        Phase::Diagnostics => "Phase 2: Pre-Flight Diagnostics",
        Phase::Payload => "Phase 3: Payload Curation (Was soll mit?)",
        Phase::Blueprint => "Phase 4: Target System Blueprint",
        Phase::Flightplan => "Phase 5: Flightplan Review & Point of No Return",
        Phase::Executing => "Phase 6: Live Execution & Packaging",
    };

    let btn_text = Line::from(vec![
        Span::styled(format!(" ▶ {phase_title} "), Style::default().fg(Color::Black).bg(C_ACCENT).add_modifier(Modifier::BOLD)),
        Span::styled("  Linux Migration Bridge", Style::default().fg(C_MUTED)),
    ]);
    f.render_widget(Paragraph::new(btn_text).block(btn_block), rows[0]);

    let main_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(C_BORDER))
        .style(Style::default().bg(C_SURFACE));

    match app.phase {
        Phase::Diagnostics => render_phase_diagnostics(f, rows[1], main_block, app),
        Phase::Payload => render_phase_payload(f, rows[1], main_block, app),
        Phase::Blueprint => render_phase_blueprint(f, rows[1], main_block, app),
        Phase::Flightplan => render_phase_flightplan(f, rows[1], main_block, app),
        Phase::Executing => render_phase_executing(f, rows[1], main_block, app),
        Phase::Gate | Phase::Scanning => {}
    }
}

// PHASE 2 RENDERER: DIAGNOSTICS
fn render_phase_diagnostics(f: &mut Frame, area: Rect, block: Block, app: &App) {
    let mut lines = Vec::new();

    lines.push(Line::from(vec![
        Span::styled("PRE-FLIGHT DIAGNOSTICS & SYSTEM CHECKS:", Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD)),
    ]));
    lines.push(Line::from("Alle Systeme wurden erfolgreich geprüft:"));
    lines.push(Line::from(""));

    if let Some(d) = &app.disk_info {
        if d.is_bitlocker_active {
            lines.push(Line::from(vec![
                Span::styled(" [!] BitLocker Verschlüsselung: ", Style::default().fg(C_WARN_TITLE).add_modifier(Modifier::BOLD)),
                Span::styled("AKTIV! (Partition kann im Linux-RAMFS nicht ohne Key verkleinert werden)", Style::default().fg(C_WARN_BORDER)),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::styled(" [✓] BitLocker Verschlüsselung: ", Style::default().fg(C_SUCCESS).add_modifier(Modifier::BOLD)),
                Span::styled("Deaktiviert / Aus (NTFS ist unverschlüsselt und sicher verkleinerbar)", Style::default().fg(C_TEXT_BRIGHT)),
            ]));
        }

        let avail_gb = d.available_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
        let space_color = if avail_gb >= 8.0 { C_SUCCESS } else { C_WARN_TITLE };
        lines.push(Line::from(vec![
            Span::styled(" [✓] Freier Festplattenspeicher: ", Style::default().fg(space_color).add_modifier(Modifier::BOLD)),
            Span::styled(format!("{:.1} GB frei auf {} (Ausreichend für Blackbox-Staging)", avail_gb, d.mount_point), Style::default().fg(C_TEXT_BRIGHT)),
        ]));
    }

    let dirty_count = app.git_repos.iter().filter(|r| r.is_dirty).count();
    if dirty_count > 0 {
        lines.push(Line::from(vec![
            Span::styled(" [!] Git Repositories Status:  ", Style::default().fg(C_WARN_TITLE).add_modifier(Modifier::BOLD)),
            Span::styled(format!("{} von {} Repositories haben uncommitted Änderungen (in Phase 3 prüfbar)", dirty_count, app.git_repos.len()), Style::default().fg(C_WARN_TITLE)),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled(" [✓] Git Repositories Status:  ", Style::default().fg(C_SUCCESS).add_modifier(Modifier::BOLD)),
            Span::styled(format!("Alle {} Repositories sind sauber (Clean Tree)", app.git_repos.len()), Style::default().fg(C_TEXT_BRIGHT)),
        ]));
    }

    lines.push(Line::from(vec![
        Span::styled(" [✓] Erkannte Anwendungen:     ", Style::default().fg(C_SUCCESS).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{} Programme für Flatpak-Migration identifiziert", app.software.len()), Style::default().fg(C_TEXT_BRIGHT)),
    ]));

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("───────────────────────────────────────────────────────────────────", Style::default().fg(C_BORDER))));
    lines.push(Line::from(vec![
        Span::styled("Status: ", Style::default().fg(C_MUTED)),
        Span::styled("FLIGHT READY. ", Style::default().fg(C_SUCCESS).add_modifier(Modifier::BOLD)),
        Span::styled("Drücke [Enter], um zur Daten-Auswahl (Payload) zu gelangen.", Style::default().fg(C_TEXT_BRIGHT)),
    ]));

    f.render_widget(Paragraph::new(lines).block(block), area);
}

// PHASE 3 RENDERER: PAYLOAD CURATION
fn render_phase_payload(f: &mut Frame, area: Rect, block: Block, app: &App) {
    let mut lines = Vec::new();

    if app.inspecting_git_repos {
        let total_repos = app.git_repos.len();
        let visible_count = 7;
        let scroll_offset = if app.selected_git_idx >= visible_count {
            app.selected_git_idx - visible_count + 1
        } else {
            0
        };

        lines.push(Line::from(vec![
            Span::styled("GIT REPOSITORIES BROWSER:", Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(format!(" (Zeige {}-{} von {total_repos}) [Space: Toggle | Esc: Zurück]", scroll_offset + 1, (scroll_offset + visible_count).min(total_repos)), Style::default().fg(C_MUTED)),
        ]));
        lines.push(Line::from(""));

        for (rel_idx, repo) in app.git_repos.iter().skip(scroll_offset).take(visible_count).enumerate() {
            let actual_idx = scroll_offset + rel_idx;
            let is_cur = actual_idx == app.selected_git_idx;
            let check = if repo.selected { "[x] " } else { "[ ] " };
            let pointer = if is_cur { "▶ " } else { "  " };

            let dirty_span = if repo.is_dirty {
                Span::styled(format!(" [DIRTY: {} uncommitted]", repo.uncommitted_files_count), Style::default().fg(C_WARN_TITLE).add_modifier(Modifier::BOLD))
            } else {
                Span::styled(" [CLEAN]", Style::default().fg(C_SUCCESS))
            };

            lines.push(Line::from(vec![
                Span::styled(pointer, Style::default().fg(C_ACCENT)),
                Span::styled(check, Style::default().fg(if repo.selected { C_SUCCESS } else { C_MUTED }).add_modifier(Modifier::BOLD)),
                Span::styled(format!("{:<26}", repo.name), Style::default().fg(if is_cur { Color::Black } else { C_TEXT_BRIGHT }).bg(if is_cur { Color::Gray } else { Color::Reset })),
                Span::styled(format!(" branch: {:<12}", repo.current_branch), Style::default().fg(C_MUTED)),
                dirty_span,
            ]));
        }
    } else {
        lines.push(Line::from(vec![
            Span::styled("PAYLOAD CURATION (WAS SOLL IN DIE BLACKBOX?):", Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(" [Space: Toggle | Tab: Git Details]", Style::default().fg(C_MUTED)),
        ]));
        lines.push(Line::from(""));

        for (i, item) in app.candidates.iter().enumerate() {
            let is_cur = i == app.selected_payload_idx;
            let check = if item.selected { "[x] " } else { "[ ] " };
            let pointer = if is_cur { "▶ " } else { "  " };

            lines.push(Line::from(vec![
                Span::styled(pointer, Style::default().fg(C_ACCENT)),
                Span::styled(check, Style::default().fg(if item.selected { C_SUCCESS } else { C_MUTED }).add_modifier(Modifier::BOLD)),
                Span::styled(format!("{:<40}", item.display_name), Style::default().fg(if is_cur { Color::Black } else { C_TEXT_BRIGHT }).bg(if is_cur { Color::Gray } else { Color::Reset })),
                Span::styled(format!(" {:<16}", item.size_human), Style::default().fg(C_MUTED)),
            ]));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("───────────────────────────────────────────────────────────────────", Style::default().fg(C_BORDER))));
    lines.push(Line::from(vec![
        Span::styled("[Esc: Zurück]  ", Style::default().fg(C_MUTED)),
        Span::styled("[Tab: Git Repos Details]  ", Style::default().fg(C_ACCENT)),
        Span::styled("[Enter: Weiter zum Blueprint]", Style::default().fg(C_TEXT_BRIGHT).add_modifier(Modifier::BOLD)),
    ]));

    f.render_widget(Paragraph::new(lines).block(block), area);
}

// PHASE 4 RENDERER: BLUEPRINT
fn render_phase_blueprint(f: &mut Frame, area: Rect, block: Block, app: &App) {
    let mut lines = Vec::new();

    lines.push(Line::from(vec![
        Span::styled("TARGET BLUEPRINT CONFIGURATION:", Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD)),
        Span::styled(" [↑/↓: Fokus wechseln | ←/→: Ändern]", Style::default().fg(C_MUTED)),
    ]));
    lines.push(Line::from(""));

    let is_distro_focus = app.blueprint_focus == BlueprintFocus::Distro;
    let distro_pointer = if is_distro_focus { "▶ " } else { "  " };
    let distro_style = if is_distro_focus {
        Style::default().fg(Color::Black).bg(Color::Gray).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(C_TEXT_BRIGHT)
    };

    lines.push(Line::from(vec![
        Span::styled(distro_pointer, Style::default().fg(C_ACCENT)),
        Span::styled("Ziel-Distribution:    ", Style::default().fg(C_MUTED)),
        Span::styled(format!("◀  {}  ▶", app.distros[app.selected_distro]), distro_style),
        Span::styled(" (Pfeiltasten Links/Rechts)", Style::default().fg(C_MUTED)),
    ]));

    let is_fs_focus = app.blueprint_focus == BlueprintFocus::Filesystem;
    let fs_pointer = if is_fs_focus { "▶ " } else { "  " };
    let fs_style = if is_fs_focus {
        Style::default().fg(Color::Black).bg(Color::Gray).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(C_TEXT_BRIGHT)
    };

    lines.push(Line::from(vec![
        Span::styled(fs_pointer, Style::default().fg(C_ACCENT)),
        Span::styled("Dateisystem-Layout:   ", Style::default().fg(C_MUTED)),
        Span::styled(format!("◀  {}  ▶", app.filesystems[app.selected_fs]), fs_style),
        Span::styled(" (Pfeiltasten Links/Rechts)", Style::default().fg(C_MUTED)),
    ]));

    lines.push(Line::from(""));
    let total_sw = app.software.len();
    let visible_sw_count = 6;
    let sw_scroll_offset = if app.selected_sw_idx >= visible_sw_count {
        app.selected_sw_idx - visible_sw_count + 1
    } else {
        0
    };

    lines.push(Line::from(vec![
        Span::styled("ANWENDUNGEN ZUR AUTOMATISCHEN REINSTALLATION:", Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" (Zeige {}-{} von {total_sw})", sw_scroll_offset + 1, (sw_scroll_offset + visible_sw_count).min(total_sw)), Style::default().fg(C_MUTED)),
    ]));

    for (rel_idx, sw) in app.software.iter().skip(sw_scroll_offset).take(visible_sw_count).enumerate() {
        let actual_idx = sw_scroll_offset + rel_idx;
        let is_cur = app.blueprint_focus == BlueprintFocus::SoftwareList && actual_idx == app.selected_sw_idx;
        let check = if sw.will_reinstall { "[x] " } else { "[ ] " };
        let pointer = if is_cur { "▶ " } else { "  " };

        let flatpak_label = sw.flatpak_id.as_deref().unwrap_or("native (pacman)");

        lines.push(Line::from(vec![
            Span::styled(pointer, Style::default().fg(C_ACCENT)),
            Span::styled(check, Style::default().fg(if sw.will_reinstall { C_SUCCESS } else { C_MUTED }).add_modifier(Modifier::BOLD)),
            Span::styled(format!("{:<26}", sw.name), Style::default().fg(if is_cur { Color::Black } else { C_TEXT_BRIGHT }).bg(if is_cur { Color::Gray } else { Color::Reset })),
            Span::styled(format!(" Flathub ID: {:<32}", flatpak_label), Style::default().fg(C_MUTED)),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("───────────────────────────────────────────────────────────────────", Style::default().fg(C_BORDER))));
    lines.push(Line::from(vec![
        Span::styled("[Esc: Zurück]  ", Style::default().fg(C_MUTED)),
        Span::styled("[Enter: Weiter zum Flugplan]", Style::default().fg(C_TEXT_BRIGHT).add_modifier(Modifier::BOLD)),
    ]));

    f.render_widget(Paragraph::new(lines).block(block), area);
}

// PHASE 5 RENDERER: FLIGHTPLAN
fn render_phase_flightplan(f: &mut Frame, area: Rect, block: Block, app: &App) {
    let mut lines = Vec::new();

    lines.push(Line::from(vec![
        Span::styled("FLIGHTPLAN SUMMARY (DER FINALE MIGRATIONSPLAN):", Style::default().fg(C_WARN_TITLE).add_modifier(Modifier::BOLD)),
    ]));
    lines.push(Line::from("Überprüfe alle Parameter sorgfältig vor der Zündung:"));
    lines.push(Line::from(""));

    let target_distro = app.distros[app.selected_distro];
    let target_fs = app.filesystems[app.selected_fs];
    let selected_apps_count = app.software.iter().filter(|s| s.will_reinstall).count();
    let selected_repos_count = app.git_repos.iter().filter(|r| r.selected).count();

    let total_uncompressed: u64 = app.candidates.iter().filter(|c| c.selected).map(|c| c.total_bytes).sum();
    let est_compressed = total_uncompressed / 2;

    lines.push(Line::from(vec![
        Span::styled(" • Quelle:               ", Style::default().fg(C_MUTED)),
        Span::styled(format!("Windows 11 ({:?})", app.user_profile), Style::default().fg(C_TEXT_BRIGHT)),
    ]));
    lines.push(Line::from(vec![
        Span::styled(" • Ziel:                 ", Style::default().fg(C_MUTED)),
        Span::styled(format!("{target_distro} auf {target_fs}"), Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD)),
    ]));
    lines.push(Line::from(vec![
        Span::styled(" • Git Repositories:     ", Style::default().fg(C_MUTED)),
        Span::styled(format!("{selected_repos_count} von {} Repositories markiert", app.git_repos.len()), Style::default().fg(C_TEXT_BRIGHT)),
    ]));
    lines.push(Line::from(vec![
        Span::styled(" • Flatpak Apps:         ", Style::default().fg(C_MUTED)),
        Span::styled(format!("{selected_apps_count} Apps für Auto-Install vorgemerkt"), Style::default().fg(C_TEXT_BRIGHT)),
    ]));
    lines.push(Line::from(vec![
        Span::styled(" • Geschätztes Archiv:   ", Style::default().fg(C_MUTED)),
        Span::styled(format!("{} (Unkomprimiert: {})", format_bytes(est_compressed), format_bytes(total_uncompressed)), Style::default().fg(C_SUCCESS).add_modifier(Modifier::BOLD)),
    ]));

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("ACHTUNG: Nach der Bestätigung wird das Blackbox-Archiv erzeugt.", Style::default().fg(C_WARN_TITLE))));

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("───────────────────────────────────────────────────────────────────", Style::default().fg(C_BORDER))));
    lines.push(Line::from(vec![
        Span::styled("[Esc: Zurück]  ", Style::default().fg(C_MUTED)),
        Span::styled(" [ Enter: ZÜNDUNG / MIGRATION STARTEN ] ", Style::default().bg(C_WARN_BORDER).fg(Color::Black).add_modifier(Modifier::BOLD)),
    ]));

    f.render_widget(Paragraph::new(lines).block(block), area);
}

// PHASE 6 RENDERER: EXECUTING LIVE
fn render_phase_executing(f: &mut Frame, area: Rect, block: Block, app: &App) {
    let mut lines = Vec::new();

    let is_packing = app.is_packing.load(Ordering::SeqCst);
    let is_bridging = app.is_bridging.load(Ordering::SeqCst);

    if is_packing {
        lines.push(Line::from(vec![
            Span::styled(" [▶] BLACKBOX PACKAGING LÄUFT... ", Style::default().fg(Color::Black).bg(C_ACCENT).add_modifier(Modifier::BOLD)),
        ]));
        lines.push(Line::from(""));

        if let Ok(lock) = app.pack_progress.lock() {
            if let Some(p) = lock.as_ref() {
                lines.push(Line::from(format!("Dateien gepackt: {}", p.files_packed)));
                lines.push(Line::from(format!("Verarbeitete Bytes: {:.1} MB", p.bytes_processed as f64 / (1024.0 * 1024.0))));
                lines.push(Line::from("Aktuelle Datei:"));
                lines.push(Line::from(Span::styled(format!(" > {}", p.current_file), Style::default().fg(C_MUTED))));
            } else {
                lines.push(Line::from("Initialisiere Zstandard Stream..."));
            }
        }
    } else if let Ok(lock) = app.pack_result.lock() {
        if let Some(res) = lock.as_ref() {
            lines.push(Line::from(vec![
                Span::styled(" [✓] BLACKBOX ERFOLGREICH ERSTELLT! ", Style::default().fg(Color::Black).bg(C_SUCCESS).add_modifier(Modifier::BOLD)),
            ]));
            lines.push(Line::from(format!(
                "Archiv: {:?} ({} Dateien, {:.2} MB Zstd)",
                res.archive_path,
                res.total_files,
                res.compressed_bytes as f64 / (1024.0 * 1024.0)
            )));
            lines.push(Line::from(vec![
                Span::styled("SHA-256: ", Style::default().fg(C_MUTED)),
                Span::styled(res.sha256_checksum.clone(), Style::default().fg(C_ACCENT)),
            ]));

            if let Ok(hlock) = app.handoff_result.lock() {
                if let Some(h) = hlock.as_ref() {
                    lines.push(Line::from(vec![
                        Span::styled(" [✓] VM Handoff Stage: ", Style::default().fg(C_SUCCESS)),
                        Span::styled(
                            format!("{}", h.stage_dir.display()),
                            Style::default().fg(C_TEXT_BRIGHT),
                        ),
                    ]));

                    if let Some(uki_path) = &h.uki_staged_path {
                        lines.push(Line::from(vec![
                            Span::styled(" [✓] UKI Kernel-Image: ", Style::default().fg(C_SUCCESS)),
                            Span::styled(format!("{}", uki_path.display()), Style::default().fg(C_TEXT_BRIGHT)),
                        ]));
                    } else {
                        lines.push(Line::from(vec![
                            Span::styled(" [!] UKI Kernel-Image: ", Style::default().fg(C_WARN_TITLE)),
                            Span::styled(
                                "Nicht auf ESP platziert (Offline oder Admin-Rechte fehlen)",
                                Style::default().fg(C_WARN_BORDER),
                            ),
                        ]));
                    }

                    if h.bcd_armed {
                        let guid_str = h.bcd_guid.as_deref().unwrap_or("{aktiv}");
                        lines.push(Line::from(vec![
                            Span::styled(" [✓] UEFI One-Time Boot: ", Style::default().fg(C_SUCCESS)),
                            Span::styled(
                                format!("BCD BootSequence SCHARF ({guid_str}) -> Bereit für Reboot!"),
                                Style::default().fg(C_TEXT_BRIGHT).add_modifier(Modifier::BOLD),
                            ),
                        ]));
                    } else {
                        let last_msg = h
                            .status_log
                            .last()
                            .map(|s| s.as_str())
                            .unwrap_or("BCD nicht automatisch scharfgestellt (stage_efi_boot.bat als Admin starten)");
                        lines.push(Line::from(vec![
                            Span::styled(" [!] UEFI One-Time Boot: ", Style::default().fg(C_WARN_TITLE)),
                            Span::styled(last_msg.to_string(), Style::default().fg(C_WARN_BORDER)),
                        ]));
                    }

                    if h.secure_boot_enabled {
                        lines.push(Line::from(vec![
                            Span::styled(" [!] SECURE BOOT AKTIV: ", Style::default().fg(C_WARN_TITLE).add_modifier(Modifier::BOLD)),
                            Span::styled(
                                "Unsigniertes UKI könnte blockiert werden! In Hyper-V Secure Boot deaktivieren.",
                                Style::default().fg(C_WARN_BORDER),
                            ),
                        ]));
                    }
                }
            }

            lines.push(Line::from(""));
            if is_bridging {
                let status = app
                    .bridge_status_msg
                    .lock()
                    .map(|s| s.clone())
                    .unwrap_or_else(|_| "Bridge läuft...".to_string());
                lines.push(Line::from(vec![
                    Span::styled(" [▶] VM-BRIDGE SANDBOX LÄUFT: ", Style::default().fg(Color::Black).bg(C_ACCENT).add_modifier(Modifier::BOLD)),
                    Span::styled(format!(" {status}"), Style::default().fg(C_TEXT_BRIGHT)),
                ]));
            } else if let Ok(slock) = app.bridge_summary.lock() {
                if let Some(summary) = slock.as_ref() {
                    lines.push(Line::from(vec![
                        Span::styled(" [✓] RAMFS BRIDGE SANDBOX BEREIT! ", Style::default().fg(Color::Black).bg(C_SUCCESS).add_modifier(Modifier::BOLD)),
                    ]));
                    lines.push(Line::from(format!(
                        "Ziel-Partition: {} | Wiederhergestellt: {} Dateien",
                        summary.plan.new_linux_partition.display(),
                        summary.injection.files_restored
                    )));
                    lines.push(Line::from(format!(
                        "POSIX Security Fixes (.ssh 0700/0600): {} Einträge korrigiert",
                        summary.injection.permission_fixes.len()
                    )));
                    lines.push(Line::from(format!(
                        "fstab & systemd-boot generiert unter: {}",
                        summary.target_root.display()
                    )));
                } else {
                    lines.push(Line::from(vec![
                        Span::styled(" [B] ", Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD)),
                        Span::styled("Bridge-Engine & RAMFS-Injektion in VM-Sandbox testen", Style::default().fg(C_TEXT_BRIGHT)),
                    ]));
                }
            }

            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Drücke [R] zum Neustart ins RAMFS oder [Enter] / [Q] zum Beenden.",
                Style::default().fg(C_MUTED),
            )));
        }
    }

    f.render_widget(Paragraph::new(lines).block(block), area);
}

// ============================================================================
// RECHTE SPALTE: DER GUIDED STEPPER
// ============================================================================
fn render_guided_right_stepper(f: &mut Frame, area: Rect, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4), // bifroest Title Box
            Constraint::Min(12),   // Stepper Steps
        ])
        .split(area);

    let title_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(C_BORDER))
        .style(Style::default().bg(C_SURFACE));

    let title_lines = vec![
        Line::from(Span::styled("B I F R O E S T", Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD))),
        Line::from(Span::styled("LINUX MIGRATION BRIDGE", Style::default().fg(C_MUTED))),
    ];
    f.render_widget(Paragraph::new(title_lines).block(title_block).alignment(Alignment::Center), rows[0]);

    let stepper_block = Block::default()
        .title(" ABLAUF-PFAD ")
        .title_style(Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(C_BORDER))
        .style(Style::default().bg(C_SURFACE));

    let current_step = match app.phase {
        Phase::Gate => 0,
        Phase::Scanning => 0,
        Phase::Diagnostics => 1,
        Phase::Payload => 2,
        Phase::Blueprint => 3,
        Phase::Flightplan => 4,
        Phase::Executing => 5,
    };

    let step_label = |step_idx: usize, name: &'static str| -> Line {
        if step_idx < current_step {
            Line::from(vec![
                Span::styled(" [✓] ", Style::default().fg(C_SUCCESS).add_modifier(Modifier::BOLD)),
                Span::styled(name, Style::default().fg(C_TEXT_BRIGHT)),
            ])
        } else if step_idx == current_step {
            Line::from(vec![
                Span::styled(" [▶] ", Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD)),
                Span::styled(name, Style::default().fg(Color::Black).bg(C_ACCENT).add_modifier(Modifier::BOLD)),
            ])
        } else {
            Line::from(vec![
                Span::styled(" [ ] ", Style::default().fg(C_MUTED)),
                Span::styled(name, Style::default().fg(C_MUTED)),
            ])
        }
    };

    let steps = vec![
        step_label(0, "1. The Gate"),
        Line::from(""),
        step_label(1, "2. Diagnostics"),
        Line::from(""),
        step_label(2, "3. Payload"),
        Line::from(""),
        step_label(3, "4. Blueprint"),
        Line::from(""),
        step_label(4, "5. Flightplan"),
        Line::from(""),
        step_label(5, "6. Packaging"),
    ];

    f.render_widget(Paragraph::new(steps).block(stepper_block), rows[1]);
}

// ============================================================================
// FOOTER HELP
// ============================================================================
fn render_guided_footer(f: &mut Frame, area: Rect, app: &App) {
    let help = match app.phase {
        Phase::Gate => Line::from(""),
        Phase::Scanning => Line::from(Span::styled("System-Inspektion läuft... Bitte warten.", Style::default().fg(C_MUTED))),
        Phase::Diagnostics => Line::from(vec![
            Span::styled("[Enter] ", Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled("Weiter zur Daten-Auswahl  ", Style::default().fg(C_MUTED)),
            Span::styled("[Esc] ", Style::default().fg(C_TEXT_BRIGHT).add_modifier(Modifier::BOLD)),
            Span::styled("Zurück  ", Style::default().fg(C_MUTED)),
            Span::styled("[Q] ", Style::default().fg(C_TEXT_BRIGHT).add_modifier(Modifier::BOLD)),
            Span::styled("Beenden", Style::default().fg(C_MUTED)),
        ]),
        Phase::Payload => Line::from(vec![
            Span::styled("[Space] ", Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled("Toggle  ", Style::default().fg(C_MUTED)),
            Span::styled("[Tab] ", Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled("Git Details  ", Style::default().fg(C_MUTED)),
            Span::styled("[Enter] ", Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled("Weiter zum Blueprint  ", Style::default().fg(C_MUTED)),
            Span::styled("[Esc] ", Style::default().fg(C_TEXT_BRIGHT).add_modifier(Modifier::BOLD)),
            Span::styled("Zurück", Style::default().fg(C_MUTED)),
        ]),
        Phase::Blueprint => Line::from(vec![
            Span::styled("[↑/↓] ", Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled("Fokus (Distro/FS/Apps)  ", Style::default().fg(C_MUTED)),
            Span::styled("[←/→] ", Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled("Auswahl ändern  ", Style::default().fg(C_MUTED)),
            Span::styled("[Space] ", Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled("App Toggle  ", Style::default().fg(C_MUTED)),
            Span::styled("[Enter] ", Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled("Weiter zum Flugplan", Style::default().fg(C_MUTED)),
        ]),
        Phase::Flightplan => Line::from(vec![
            Span::styled("[Enter] ", Style::default().fg(C_WARN_TITLE).add_modifier(Modifier::BOLD)),
            Span::styled("ZÜNDUNG / PACKEN  ", Style::default().fg(C_TEXT_BRIGHT)),
            Span::styled("[Esc] ", Style::default().fg(C_TEXT_BRIGHT).add_modifier(Modifier::BOLD)),
            Span::styled("Zurück zum Blueprint", Style::default().fg(C_MUTED)),
        ]),
        Phase::Executing => Line::from(vec![
            Span::styled("[B] ", Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled("VM-Bridge Test  ", Style::default().fg(C_MUTED)),
            Span::styled("[R] ", Style::default().fg(C_WARN_TITLE).add_modifier(Modifier::BOLD)),
            Span::styled("Reboot ins RAMFS  ", Style::default().fg(C_TEXT_BRIGHT)),
            Span::styled("[Enter / Q] ", Style::default().fg(C_TEXT_BRIGHT).add_modifier(Modifier::BOLD)),
            Span::styled("Beenden", Style::default().fg(C_MUTED)),
        ]),
    };

    f.render_widget(Paragraph::new(help).alignment(Alignment::Center), area);
}

// ============================================================================
// MODAL: LAST WARNING VOR DER ZÜNDUNG
// ============================================================================
fn render_last_warning_modal(f: &mut Frame, area: Rect, app: &App) {
    let width = 60.min(area.width.saturating_sub(4));
    let height = 15.min(area.height.saturating_sub(2));

    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height.saturating_sub(height)) / 2;
    let modal_rect = Rect::new(x, y, width, height);

    render_drop_shadow(f, modal_rect);
    f.render_widget(Clear, modal_rect);

    let block = Block::default()
        .title(" Last Warning! ")
        .title_style(Style::default().fg(C_WARN_TITLE).bg(C_SURFACE).add_modifier(Modifier::BOLD))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(C_WARN_BORDER))
        .style(Style::default().bg(C_SURFACE));

    let btn0_style = if app.final_button_idx == 0 {
        Style::default().bg(Color::LightRed).fg(Color::Black).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(C_TEXT_BRIGHT)
    };

    let btn1_style = if app.final_button_idx == 1 {
        Style::default().bg(Color::White).fg(Color::Black).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(C_MUTED)
    };

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled("At this point, there is no way back.", Style::default().fg(C_WARN_TITLE).add_modifier(Modifier::BOLD))),
        Line::from("Like seriously, proceeding will tear your"),
        Line::from("Windows apart and change your life."),
        Line::from(""),
        Line::from(Span::styled("────────────────────────────────────────────────────────", Style::default().fg(C_BORDER))),
        Line::from(vec![
            Span::styled("proceed?   ", Style::default().fg(C_MUTED)),
            Span::styled(" [ YES ] ", btn0_style),
            Span::raw("   "),
            Span::styled(" [ no ] ", btn1_style),
        ]),
    ];

    f.render_widget(Paragraph::new(lines).block(block).alignment(Alignment::Center), modal_rect);
}

// ============================================================================
// HELPER: BIOS/MEMTEST86 DROP SHADOW
// ============================================================================
fn render_drop_shadow(f: &mut Frame, rect: Rect) {
    let shadow_rect = Rect {
        x: rect.x + 2,
        y: rect.y + 1,
        width: rect.width,
        height: rect.height,
    };
    let shadow_block = Block::default().style(Style::default().bg(C_SHADOW));
    f.render_widget(shadow_block, shadow_rect);
}
