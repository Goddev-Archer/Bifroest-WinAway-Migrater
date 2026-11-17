use std::{
    io,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Gauge, Paragraph},
    Frame, Terminal,
};

use super::{execute_full_bridge_pipeline_with_mode, BridgeExecutionSummary};

// Bifröst 16-Color ANSI-Safe Cyberpunk Palette (avoiding DarkGray due to Linux vt.c intensity-bit bug)
const C_TITLE: Color = Color::Cyan;
const C_RUNES: Color = Color::Magenta;
const C_MIDGARD: Color = Color::LightRed;
const C_ASGARD: Color = Color::LightGreen;
const C_GOLD: Color = Color::Yellow;
const C_TEXT: Color = Color::White;
const C_MUTED: Color = Color::Gray;

const RAINBOW_COLORS: [Color; 6] = [
    Color::LightRed,
    Color::Yellow,
    Color::LightGreen,
    Color::Cyan,
    Color::LightBlue,
    Color::Magenta,
];

#[derive(Debug, Clone)]
pub struct TransitStatus {
    pub step: usize,
    pub total_steps: usize,
    pub percent: u16,
    pub status_text: String,
    pub finished: bool,
    pub error: Option<String>,
    pub summary: Option<BridgeExecutionSummary>,
    pub clean_install: bool,
    pub awaiting_strategy_confirm: bool,
    pub auto_confirm_secs: u64,
}

impl Default for TransitStatus {
    fn default() -> Self {
        Self {
            step: 1,
            total_steps: 5,
            percent: 0,
            status_text: "Initializing Bifröst Transit Engine...".to_string(),
            finished: false,
            error: None,
            summary: None,
            clean_install: true,
            awaiting_strategy_confirm: false,
            auto_confirm_secs: 10,
        }
    }
}

/// Strips raw carriage returns and control characters so external command output cannot corrupt the TUI cursor.
fn sanitize_terminal_text(raw: &str) -> String {
    raw.lines()
        .map(|line| {
            let last_seg = line
                .split('\r')
                .filter_map(|seg| {
                    let trimmed = seg.trim();
                    if !trimmed.is_empty() {
                        Some(trimmed)
                    } else {
                        None
                    }
                })
                .last()
                .unwrap_or("");
            last_seg
                .chars()
                .filter(|c| !c.is_control())
                .collect::<String>()
        })
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" | ")
}

/// Prompts the user inside the Bifröst Cinema TUI to choose Clean Install vs Dual-Boot with a 10s auto-confirm countdown.
fn select_migration_strategy(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    initial_clean_install: bool,
) -> io::Result<Option<bool>> {
    let mut selected_clean = initial_clean_install;
    let mut tick: usize = 0;
    let prompt_start = Instant::now();
    let timeout = Duration::from_secs(10);

    loop {
        tick = (tick + 1) % 10000;
        let elapsed = prompt_start.elapsed();
        let remaining = timeout.saturating_sub(elapsed).as_secs();

        let prompt_status = TransitStatus {
            step: 1,
            total_steps: 5,
            percent: 0,
            status_text: "Select migration strategy to begin Bifröst transit...".to_string(),
            finished: false,
            error: None,
            summary: None,
            clean_install: selected_clean,
            awaiting_strategy_confirm: true,
            auto_confirm_secs: remaining,
        };

        terminal.draw(|f| {
            render_cinema_frame(f, &prompt_status, tick, elapsed);
        })?;

        if elapsed >= timeout {
            return Ok(Some(selected_clean));
        }

        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(k) = event::read()? {
                match k.code {
                    KeyCode::Enter => return Ok(Some(selected_clean)),
                    KeyCode::Char('1') => selected_clean = true,
                    KeyCode::Char('2') => selected_clean = false,
                    KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') => {
                        selected_clean = !selected_clean;
                    }
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(None),
                    _ => {}
                }
            }
        }
    }
}

/// Runs the complete in-place migration wrapped in an animated ASCII Bifröst Cinema.
pub fn run_ramfs_transit_cinema(
    archive_path: &Path,
    expected_sha256: Option<&str>,
    target_root: &Path,
    requested_linux_bytes: u64,
    dry_run: bool,
) -> io::Result<BridgeExecutionSummary> {
    let initial_clean_install = super::inject::read_manifest_from_archive(archive_path)
        .map(|m| m.clean_install)
        .unwrap_or(true);

    // If not a TTY (piped/testing), fall back to CLI logging
    if !std::io::IsTerminal::is_terminal(&io::stdout()) {
        println!("[Bifröst RAMFS] Non-interactive TTY - launching direct CLI mode...");
        return execute_full_bridge_pipeline_with_mode(
            archive_path,
            expected_sha256,
            target_root,
            requested_linux_bytes,
            Some(initial_clean_install),
            dry_run,
            |msg, step, total| {
                println!("[{step}/{total}] {msg}");
            },
        );
    }

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Interactive pre-flight migration strategy prompt (10s auto-confirm)
    let chosen_clean = match select_migration_strategy(&mut terminal, initial_clean_install) {
        Ok(Some(mode)) => mode,
        Ok(None) => {
            let _ = disable_raw_mode();
            let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
            let _ = terminal.show_cursor();
            return Err(io::Error::new(io::ErrorKind::Interrupted, "Migration aborted by user"));
        }
        Err(e) => {
            let _ = disable_raw_mode();
            let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
            let _ = terminal.show_cursor();
            return Err(e);
        }
    };

    let status = Arc::new(Mutex::new(TransitStatus {
        clean_install: chosen_clean,
        ..TransitStatus::default()
    }));
    let cancel = Arc::new(AtomicBool::new(false));

    // Spawn pipeline in background thread after strategy selection
    let status_bg = Arc::clone(&status);
    let archive_owned = archive_path.to_path_buf();
    let target_owned = target_root.to_path_buf();
    let sha256_owned = expected_sha256.map(|s| s.to_string());

    let worker_handle = thread::spawn(move || {
        let res = execute_full_bridge_pipeline_with_mode(
            &archive_owned,
            sha256_owned.as_deref(),
            &target_owned,
            requested_linux_bytes,
            Some(chosen_clean),
            dry_run,
            |msg, step, total| {
                if let Ok(mut lock) = status_bg.lock() {
                    lock.step = step;
                    lock.total_steps = total;
                    lock.percent = ((step as f32 / total as f32) * 90.0) as u16;
                    lock.status_text = sanitize_terminal_text(msg);
                }
            },
        );

        if let Ok(mut lock) = status_bg.lock() {
            match res {
                Ok(summary) => {
                    lock.percent = 100;
                    lock.status_text = "Transit complete! Asgard subvolumes are armed and ready.".to_string();
                    lock.summary = Some(summary);
                    lock.finished = true;
                }
                Err(e) => {
                    lock.error = Some(e.to_string());
                    lock.finished = true;
                }
            }
        }
    });

    let mut tick: usize = 0;
    let mut countdown_start: Option<Instant> = None;
    let start_time = Instant::now();

    loop {
        tick = (tick + 1) % 10000;

        let cur_status = match status.lock() {
            Ok(lock) => lock.clone(),
            Err(_) => break,
        };

        terminal.draw(|f| {
            render_cinema_frame(f, &cur_status, tick, start_time.elapsed());
        })?;

        if cur_status.finished {
            if cur_status.error.is_some() {
                if event::poll(Duration::from_millis(50))? {
                    if let Event::Key(k) = event::read()? {
                        if k.code == KeyCode::Enter || k.code == KeyCode::Esc || k.code == KeyCode::Char('q') {
                            break;
                        }
                    }
                }
                continue;
            }

            if countdown_start.is_none() {
                countdown_start = Some(Instant::now());
            }

            if let Some(cstart) = countdown_start {
                if cstart.elapsed() >= Duration::from_secs(5) {
                    break;
                }
            }
        }

        if event::poll(Duration::from_millis(40))? {
            if let Event::Key(k) = event::read()? {
                if cur_status.finished && cur_status.error.is_none() && k.code == KeyCode::Enter {
                    break;
                }
                if k.code == KeyCode::Char('q') || k.code == KeyCode::Esc {
                    cancel.store(true, Ordering::SeqCst);
                    break;
                }
            }
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    let _ = worker_handle.join();

    let final_status = status.lock().map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
    if let Some(err) = &final_status.error {
        return Err(io::Error::new(io::ErrorKind::Other, err.clone()));
    }
    if let Some(summary) = &final_status.summary {
        return Ok(summary.clone());
    }

    Err(io::Error::new(io::ErrorKind::Interrupted, "Migration aborted by user"))
}

fn render_cinema_frame(f: &mut Frame, st: &TransitStatus, tick: usize, elapsed: Duration) {
    let size = f.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(7), // Header with Isometric BRIDGE ASCII Art
            Constraint::Length(9), // Bifröst Rainbow Bridge
            Constraint::Length(8), // Strategy Selector or Step Matrix
            Constraint::Min(3),    // Status bar / Countdown / Error details
        ])
        .split(size);

    // 1. Header Block (Isometric BRIDGE font + Minimalist telemetry text)
    let header_text = vec![
        Line::from(vec![
            Span::styled(" ______     ______     __     _____     ______     ______    ", Style::default().fg(C_TITLE).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::styled("/\\  == \\   /\\  == \\   /\\ \\   /\\  __-.  /\\  ___\\   /\\  ___\\   ", Style::default().fg(C_TITLE)),
            Span::styled("  THE BRIDGE TO ASGARD", Style::default().fg(C_TEXT).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::styled("\\ \\  __<   \\ \\  __<   \\ \\ \\  \\ \\ \\/\\ \\ \\ \\ \\__ \\  \\ \\  __\\   ", Style::default().fg(C_TITLE)),
            Span::styled(format!("  Elapsed: {:02}:{:02}", elapsed.as_secs() / 60, elapsed.as_secs() % 60), Style::default().fg(C_GOLD).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::styled(" \\ \\_____\\  \\ \\_\\ \\_\\  \\ \\_\\  \\ \\____-  \\ \\_____\\  \\ \\_____\\ ", Style::default().fg(C_TITLE)),
        ]),
        Line::from(vec![
            Span::styled("  \\/_____/   \\/_/ /_/   \\/_/   \\/____/   \\/_____/   \\/_____/ ", Style::default().fg(C_TITLE)),
        ]),
    ];
    let header_p = Paragraph::new(header_text).block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(C_MUTED))
            .border_type(BorderType::Rounded),
    );
    f.render_widget(header_p, chunks[0]);

    // 2. Center: The Animated Bifröst Rainbow Bridge (Cinema Scene)
    render_bifrost_bridge(f, chunks[1], st, tick);

    // 3. Lower: Strategy Decision Panel (Pre-flight) or Step Matrix & Telemetry
    if st.awaiting_strategy_confirm {
        render_strategy_selector(f, chunks[2], st);
    } else {
        render_step_matrix(f, chunks[2], st);
    }

    // 4. Bottom: Dual-line Progress / Completion / Error
    render_bottom_status(f, chunks[3], st);
}

/// Builds a CP437-safe rainbow-colored rune particle stream exact to `bridge_width` columns with an aligned slider marker.
fn build_rainbow_stream_spans(
    bridge_width: usize,
    box_slider: usize,
    marker: &str,
    runes: &[&str],
    tick: usize,
    offset: usize,
) -> Vec<Span<'static>> {
    let mut cells: Vec<(char, Color)> = Vec::with_capacity(bridge_width);
    let mut seq_idx = 0usize;

    while cells.len() < bridge_width {
        let r_idx = (seq_idx + tick * 2 + offset) % runes.len();
        let col = cells.len();
        let color = RAINBOW_COLORS[((col / 3) + tick + offset) % RAINBOW_COLORS.len()];
        for ch in runes[r_idx].chars() {
            if cells.len() >= bridge_width {
                break;
            }
            cells.push((ch, color));
        }
        seq_idx += 1;
    }

    let marker_chars: Vec<char> = marker.chars().collect();
    let safe_pos = box_slider.min(bridge_width.saturating_sub(marker_chars.len()));
    for (idx, ch) in marker_chars.into_iter().enumerate() {
        if let Some(cell) = cells.get_mut(safe_pos + idx) {
            *cell = (ch, C_GOLD);
        }
    }

    group_colored_cells(cells)
}

/// Groups contiguous same-color cells into minimal Ratatui `Span` slices.
fn group_colored_cells(cells: Vec<(char, Color)>) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut cur_str = String::new();
    let mut cur_color: Option<Color> = None;

    for (ch, color) in cells {
        match cur_color {
            Some(c) if c == color => cur_str.push(ch),
            Some(c) => {
                spans.push(Span::styled(std::mem::take(&mut cur_str), Style::default().fg(c).add_modifier(Modifier::BOLD)));
                cur_str.push(ch);
                cur_color = Some(color);
            }
            None => {
                cur_str.push(ch);
                cur_color = Some(color);
            }
        }
    }
    if let Some(c) = cur_color {
        if !cur_str.is_empty() {
            spans.push(Span::styled(cur_str, Style::default().fg(c).add_modifier(Modifier::BOLD)));
        }
    }
    spans
}

fn render_bifrost_bridge(f: &mut Frame, area: Rect, st: &TransitStatus, tick: usize) {
    let bridge_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(C_RUNES))
        .border_type(BorderType::Double)
        .title(Span::styled(" ♦ BIFRÖST CONTINUUM ♦ ", Style::default().fg(C_GOLD).add_modifier(Modifier::BOLD)));

    let inner = bridge_block.inner(area);
    f.render_widget(bridge_block, area);

    if inner.width < 50 || inner.height < 5 {
        return;
    }

    // 100% CP437 / Linux-framebuffer-safe carved Norse rune ligatures & bridge glyphs
    let runes = [
        "|<", "^|^", "|>", "\\Y/", "|B", "|X|", "//", "<>",
        "|P", "><", "|F", "|*|", "|L", "<*>", "|/", "/\\",
        "|T|", "|M|", "|S|", "♦", "«", "»",
    ];

    let in_ram_transit = st.clean_install && st.step >= 2;
    let midgard_portal = if in_ram_transit { "[ MIDGARD / RAM ]" } else { "[ MIDGARD / WIN11 ]" };
    let midgard_sub = if in_ram_transit { "│ Blackbox Buffered │" } else { "│   NTFS Volume    │" };
    let asgard_portal = "[ ASGARD / LINUX ]";

    let portal_width: usize = 21;
    let bridge_width = (inner.width as usize).saturating_sub(portal_width * 2);
    if bridge_width < 16 {
        return;
    }

    let box_str = "[# BLACKBOX #]";
    let box_chars: Vec<char> = box_str.chars().collect();
    let max_slider = bridge_width.saturating_sub(box_chars.len());
    let box_slider = if st.awaiting_strategy_confirm {
        (tick / 2) % (max_slider.max(1))
    } else {
        (((st.percent as f32 / 100.0) * (max_slider as f32)) as usize).min(max_slider)
    };
    let slider_center = box_slider + (box_chars.len() / 2).saturating_sub(1);

    let mut lines = Vec::new();

    // Top beam of the bridge
    let mut top_beam = String::from("╒");
    for col in 1..bridge_width.saturating_sub(1) {
        top_beam.push(if (col + tick) % 3 == 0 { '═' } else { '─' });
    }
    if bridge_width >= 2 {
        top_beam.push('╕');
    }

    lines.push(Line::from(vec![
        Span::styled(format!("{:^width$}", "┌──────────────────┐", width = portal_width), Style::default().fg(C_MIDGARD)),
        Span::styled(top_beam, Style::default().fg(C_TITLE)),
        Span::styled(format!("{:^width$}", "┌──────────────────┐", width = portal_width), Style::default().fg(C_ASGARD)),
    ]));

    // Middle 1: Portal Names + Upper Rainbow Rune Stream
    let mut upper_spans = Vec::new();
    upper_spans.push(Span::styled(
        format!("{:^width$}", midgard_portal, width = portal_width),
        Style::default().fg(C_MIDGARD).add_modifier(Modifier::BOLD),
    ));
    upper_spans.extend(build_rainbow_stream_spans(bridge_width, slider_center, "▲▲", &runes, tick, 0));
    upper_spans.push(Span::styled(
        format!("{:^width$}", asgard_portal, width = portal_width),
        Style::default().fg(C_ASGARD).add_modifier(Modifier::BOLD),
    ));
    lines.push(Line::from(upper_spans));

    // Middle 2: Exact-column Core Blackbox Transit
    let mut transit_cells: Vec<(char, Color)> = Vec::with_capacity(bridge_width);
    let box_color = if st.finished && st.error.is_none() { C_ASGARD } else { C_GOLD };
    for col in 0..bridge_width {
        if col >= box_slider && col < box_slider + box_chars.len() {
            transit_cells.push((box_chars[col - box_slider], box_color));
        } else if (col + tick) % 2 == 0 {
            let color = RAINBOW_COLORS[((col / 4) + tick) % RAINBOW_COLORS.len()];
            transit_cells.push(('»', color));
        } else {
            transit_cells.push((' ', C_TEXT));
        }
    }
    let mut transit_spans = Vec::new();
    transit_spans.push(Span::styled(
        format!("{:^width$}", midgard_sub, width = portal_width),
        Style::default().fg(C_MIDGARD),
    ));
    transit_spans.extend(group_colored_cells(transit_cells));
    transit_spans.push(Span::styled(
        format!("{:^width$}", "│  Btrfs Subvols   │", width = portal_width),
        Style::default().fg(C_ASGARD),
    ));
    lines.push(Line::from(transit_spans));

    // Middle 3: Lower Rainbow Rune Stream
    let mut lower_spans = Vec::new();
    lower_spans.push(Span::styled(
        format!("{:^width$}", "└──────────────────┘", width = portal_width),
        Style::default().fg(C_MIDGARD),
    ));
    lower_spans.extend(build_rainbow_stream_spans(bridge_width, slider_center, "▼▼", &runes, tick, 7));
    lower_spans.push(Span::styled(
        format!("{:^width$}", "└──────────────────┘", width = portal_width),
        Style::default().fg(C_ASGARD),
    ));
    lines.push(Line::from(lower_spans));

    // Bottom beam of the bridge
    let mut bot_beam = String::from("╘");
    for col in 1..bridge_width.saturating_sub(1) {
        bot_beam.push(if (col + tick) % 3 == 0 { '═' } else { '─' });
    }
    if bridge_width >= 2 {
        bot_beam.push('╛');
    }

    lines.push(Line::from(vec![
        Span::styled(format!("{:^width$}", "(Source)", width = portal_width), Style::default().fg(C_MUTED)),
        Span::styled(bot_beam, Style::default().fg(C_TITLE)),
        Span::styled(format!("{:^width$}", "(Target)", width = portal_width), Style::default().fg(C_MUTED)),
    ]));

    let p = Paragraph::new(lines).alignment(Alignment::Left);
    f.render_widget(p, inner);
}

fn render_strategy_selector(f: &mut Frame, area: Rect, st: &TransitStatus) {
    let opt1_badge = if st.clean_install { "[>>]" } else { "[  ]" };
    let opt2_badge = if !st.clean_install { "[>>]" } else { "[  ]" };
    let opt1_style = if st.clean_install {
        Style::default().fg(C_ASGARD).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(C_MUTED)
    };
    let opt2_style = if !st.clean_install {
        Style::default().fg(C_GOLD).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(C_MUTED)
    };

    let lines = vec![
        Line::from(vec![
            Span::styled("  Choose how Windows is handled AFTER your Blackbox is safely loaded into RAM:", Style::default().fg(C_TEXT)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(format!("  {opt1_badge} [1] CLEAN INSTALL (Recommended): "), opt1_style),
            Span::styled("Wipe Windows & dedicate 100% disk to Linux Btrfs", Style::default().fg(if st.clean_install { C_TEXT } else { C_MUTED })),
        ]),
        Line::from(vec![
            Span::styled(format!("  {opt2_badge} [2] DUAL-BOOT: "), opt2_style),
            Span::styled("Shrink Windows NTFS partition & install Linux side-by-side", Style::default().fg(if !st.clean_install { C_TEXT } else { C_MUTED })),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                format!("  [1/2 or Arrows] Switch   [Enter] Start Now   (Auto-starting in {}s)", st.auto_confirm_secs),
                Style::default().fg(C_TITLE).add_modifier(Modifier::BOLD),
            ),
        ]),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(C_GOLD))
        .border_type(BorderType::Double)
        .title(Span::styled(" MIGRATION STRATEGY DECISION ", Style::default().fg(C_GOLD).add_modifier(Modifier::BOLD)));

    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_step_matrix(f: &mut Frame, area: Rect, st: &TransitStatus) {
    let phase2_desc = if st.clean_install {
        "Clean Install: Full Disk Btrfs"
    } else {
        "Dual-Boot: NTFS Shrink & GPT"
    };

    let steps = [
        (1, "Scan & Strategy Plan", "Discover block devices & secure Blackbox in RAM"),
        (2, "Partition Layout Engine", phase2_desc),
        (3, "Btrfs Subvolume Forge", "Create (@root, @home, @blackbox) & /etc/fstab"),
        (4, "Soul Transit (Injection)", "Extract Blackbox from RAM & set POSIX (0700/0600)"),
        (5, "Extract Linux Rootfs & Boot", "Extract Linux Rootfs to @root & arm systemd-boot"),
    ];

    let mut lines = Vec::new();
    let all_ok = st.finished && st.error.is_none();

    for (num, title, desc) in steps {
        let is_done = st.step > num || all_ok;
        let is_err = st.error.is_some() && st.step == num;
        let is_active = !is_done && !is_err && st.step == num;

        let (icon, color) = if is_done {
            ("[OK]", C_ASGARD)
        } else if is_err {
            ("[!!]", C_MIDGARD)
        } else if is_active {
            ("[>>]", C_GOLD)
        } else {
            ("[..]", C_MUTED)
        };

        lines.push(Line::from(vec![
            Span::styled(format!("  {icon} Phase {num}: "), Style::default().fg(color).add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("{:<27}", title),
                Style::default().fg(if is_active || is_done { C_TEXT } else { C_MUTED }),
            ),
            Span::styled(format!(" | {desc}"), Style::default().fg(C_MUTED)),
        ]));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(C_MUTED))
        .border_type(BorderType::Rounded)
        .title(Span::styled(" PHASE TELEMETRY ", Style::default().fg(C_TITLE).add_modifier(Modifier::BOLD)));

    let p = Paragraph::new(lines).block(block);
    f.render_widget(p, area);
}

fn render_bottom_status(f: &mut Frame, area: Rect, st: &TransitStatus) {
    if let Some(err) = &st.error {
        let err_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(C_MIDGARD))
            .border_type(BorderType::Double)
            .title(Span::styled(" [!] PIPELINE ERROR - DETAILS [!] ", Style::default().fg(C_MIDGARD).add_modifier(Modifier::BOLD)));

        let clean_err = sanitize_terminal_text(err);

        let lines = vec![
            Line::from(vec![
                Span::styled(" Action: ", Style::default().fg(C_GOLD).add_modifier(Modifier::BOLD)),
                Span::styled("Press [Enter] or [Q] to reboot cleanly.", Style::default().fg(C_TEXT)),
            ]),
            Line::from(vec![
                Span::styled(" Details: ", Style::default().fg(C_TITLE).add_modifier(Modifier::BOLD)),
                Span::styled(clean_err, Style::default().fg(C_TEXT)),
            ]),
        ];
        let p = Paragraph::new(lines).block(err_block).wrap(ratatui::widgets::Wrap { trim: true });
        f.render_widget(p, area);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(2)])
        .split(area);

    let clean_status = sanitize_terminal_text(&st.status_text);
    let gauge_title = format!(" PROGRESS: {}% | {} ", st.percent, clean_status);
    let gauge = Gauge::default()
        .block(Block::default())
        .gauge_style(
            Style::default()
                .fg(if st.finished { C_ASGARD } else { C_TITLE })
                .bg(Color::Blue)
                .add_modifier(Modifier::BOLD),
        )
        .percent(st.percent)
        .label(Span::styled(gauge_title, Style::default().fg(C_TEXT).add_modifier(Modifier::BOLD)));
    f.render_widget(gauge, chunks[0]);

    let bottom_text = if st.awaiting_strategy_confirm {
        Line::from(vec![
            Span::styled("  >> Strategy Pending: ", Style::default().fg(C_GOLD).add_modifier(Modifier::BOLD)),
            Span::styled("Select [1] Clean Install or [2] Dual-Boot above.", Style::default().fg(C_TEXT)),
        ])
    } else if st.finished {
        Line::from(vec![
            Span::styled("  *** TRANSIT COMPLETE! ", Style::default().fg(C_ASGARD).add_modifier(Modifier::BOLD)),
            Span::styled("Welcome to Asgard! Automated reboot in 5 seconds... ", Style::default().fg(C_GOLD).add_modifier(Modifier::BOLD)),
            Span::styled("(Press Enter to reboot immediately)", Style::default().fg(C_MUTED)),
        ])
    } else {
        Line::from(vec![
            Span::styled("  >> Status: ", Style::default().fg(C_GOLD)),
            Span::styled(clean_status, Style::default().fg(C_TEXT)),
        ])
    };

    let p = Paragraph::new(vec![bottom_text]);
    f.render_widget(p, chunks[1]);
}
