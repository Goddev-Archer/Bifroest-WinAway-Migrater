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

use super::{execute_full_bridge_pipeline, BridgeExecutionSummary};

// Bifröst 16-Color ANSI-Safe Cyberpunk Palette
const C_TITLE: Color = Color::Cyan;
const C_RUNES: Color = Color::Magenta;
const C_MIDGARD: Color = Color::LightRed;
const C_ASGARD: Color = Color::LightGreen;
const C_GOLD: Color = Color::Yellow;
const C_TEXT: Color = Color::White;
const C_MUTED: Color = Color::DarkGray;

#[derive(Debug, Clone)]
pub struct TransitStatus {
    pub step: usize,
    pub total_steps: usize,
    pub percent: u16,
    pub status_text: String,
    pub finished: bool,
    pub error: Option<String>,
    pub summary: Option<BridgeExecutionSummary>,
}

impl Default for TransitStatus {
    fn default() -> Self {
        Self {
            step: 1,
            total_steps: 5,
            percent: 0,
            status_text: "Initialisiere Bifröst Transit Engine...".to_string(),
            finished: false,
            error: None,
            summary: None,
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
    // If not a TTY (piped/testing), fall back to CLI logging
    if !std::io::IsTerminal::is_terminal(&io::stdout()) {
        println!("[Bifröst RAMFS] Non-interactive TTY - starte direkten CLI-Modus...");
        return execute_full_bridge_pipeline(
            archive_path,
            expected_sha256,
            target_root,
            requested_linux_bytes,
            dry_run,
            |msg, step, total| {
                println!("[{step}/{total}] {msg}");
            },
        );
    }

    let status = Arc::new(Mutex::new(TransitStatus::default()));
    let cancel = Arc::new(AtomicBool::new(false));

    // Spawn pipeline in background thread
    let status_bg = Arc::clone(&status);
    let archive_owned = archive_path.to_path_buf();
    let target_owned = target_root.to_path_buf();
    let sha256_owned = expected_sha256.map(|s| s.to_string());

    let worker_handle = thread::spawn(move || {
        let res = execute_full_bridge_pipeline(
            &archive_owned,
            sha256_owned.as_deref(),
            &target_owned,
            requested_linux_bytes,
            dry_run,
            |msg, step, total| {
                if let Ok(mut lock) = status_bg.lock() {
                    lock.step = step;
                    lock.total_steps = total;
                    lock.percent = ((step as f32 / total as f32) * 90.0) as u16;
                    lock.status_text = msg.to_string();
                }
            },
        );

        if let Ok(mut lock) = status_bg.lock() {
            match res {
                Ok(summary) => {
                    lock.percent = 100;
                    lock.status_text = "Transit erfolgreich! Asgard-Subvolumes bereit.".to_string();
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

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

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

        // Check if finished and start countdown
        if cur_status.finished {
            if cur_status.error.is_some() {
                // Wait for user exit on error
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

        // Allow graceful quit / skip if requested
        if event::poll(Duration::from_millis(40))? {
            if let Event::Key(k) = event::read()? {
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

    Err(io::Error::new(io::ErrorKind::Interrupted, "Migration abgebrochen"))
}

fn render_cinema_frame(f: &mut Frame, st: &TransitStatus, tick: usize, elapsed: Duration) {
    let size = f.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5), // Header
            Constraint::Length(9), // Bifröst Rainbow Bridge
            Constraint::Length(8), // Step Matrix & Telemetry
            Constraint::Min(3),    // Status bar / Countdown
        ])
        .split(size);

    // 1. Header Block
    let header_text = vec![
        Line::from(vec![
            Span::styled("  ██████╗ ██╗███████╗██████╗  ██████╗ ███████╗████████╗ ", Style::default().fg(C_TITLE).add_modifier(Modifier::BOLD)),
            Span::styled(" [ RAMFS TRANSIT ENGINE ]", Style::default().fg(C_GOLD).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::styled("  ██╔══██╗██║██╔════╝██╔══██╗██╔═══██╗██╔════╝╚══██╔══╝ ", Style::default().fg(C_TITLE)),
            Span::styled(" DIE BRÜCKE NACH ASGARD (LINUX)", Style::default().fg(C_TEXT)),
        ]),
        Line::from(vec![
            Span::styled("  ██████╔╝██║█████╗  ██████╔╝██║   ██║███████╗   ██║    ", Style::default().fg(C_TITLE)),
            Span::styled(format!(" Laufzeit: {:02}:{:02}", elapsed.as_secs() / 60, elapsed.as_secs() % 60), Style::default().fg(C_MUTED)),
        ]),
    ];
    let header_p = Paragraph::new(header_text).block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(C_MUTED))
            .border_type(BorderType::Rounded),
    );
    f.render_widget(header_p, chunks[0]);

    // 2. Center: The Animated Bifröst Bridge (Cinema Scene)
    render_bifrost_bridge(f, chunks[1], st, tick);

    // 3. Lower: Step Checklist & Telemetry
    render_step_matrix(f, chunks[2], st);

    // 4. Bottom: Dual-line Progress & Completion Countdown
    render_bottom_status(f, chunks[3], st);
}

fn render_bifrost_bridge(f: &mut Frame, area: Rect, st: &TransitStatus, tick: usize) {
    let bridge_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(C_RUNES))
        .border_type(BorderType::Double)
        .title(Span::styled(" ⚡ BIFRÖST KONTINUUM ⚡ ", Style::default().fg(C_GOLD).add_modifier(Modifier::BOLD)));

    let inner = bridge_block.inner(area);
    f.render_widget(bridge_block, area);

    if inner.width < 50 || inner.height < 5 {
        return;
    }

    // Rune particle stream symbols
    let runes = ["ᚠ", "ᚢ", "ᚦ", "ᚨ", "ᚱ", "ᚲ", "ᚷ", "ᚹ", "ᚺ", "ᚾ", "ᛁ", "ᛃ", "ᛇ", "ᛈ", "ᛉ", "ᛋ", "ᛏ", "ᛒ", "ᛖ", "ᛗ", "ᛚ", "ᛜ", "ᛟ", "ᛞ", "≈", "≋", "∿", "✦", "⚡"];

    // Portals
    let midgard_portal = "[ MIDGARD / WIN11 ]";
    let asgard_portal = "[ ASGARD / LINUX ]";

    let portal_width = 20;
    let bridge_width = inner.width.saturating_sub(portal_width * 2 + 2) as usize;

    // Calculate Blackbox slider position (0 to bridge_width - 1)
    let box_slider = if bridge_width > 0 {
        ((st.percent as f32 / 100.0) * (bridge_width as f32)) as usize
    } else {
        0
    };

    let mut lines = Vec::new();

    // Top beam of the bridge
    let mut top_beam = String::from("╒");
    for i in 0..bridge_width {
        let sym = if (i + tick) % 3 == 0 { "═" } else { "─" };
        top_beam.push_str(sym);
    }
    top_beam.push('╕');

    lines.push(Line::from(vec![
        Span::styled(format!("{:^width$}", "┌──────────────────┐", width = portal_width as usize), Style::default().fg(C_MIDGARD)),
        Span::styled(top_beam, Style::default().fg(C_TITLE)),
        Span::styled(format!("{:^width$}", "┌──────────────────┐", width = portal_width as usize), Style::default().fg(C_ASGARD)),
    ]));

    // Middle 1: Portal Names + Upper Particle Stream
    let mut stream_upper = String::new();
    for i in 0..bridge_width {
        if i == box_slider {
            stream_upper.push_str("▲▲");
        } else {
            let r_idx = (i + tick * 2) % runes.len();
            stream_upper.push_str(runes[r_idx]);
        }
        if stream_upper.chars().count() >= bridge_width {
            break;
        }
    }

    lines.push(Line::from(vec![
        Span::styled(format!("{:^width$}", midgard_portal, width = portal_width as usize), Style::default().fg(C_MIDGARD).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{:<width$}", stream_upper, width = bridge_width), Style::default().fg(C_RUNES)),
        Span::styled(format!("{:^width$}", asgard_portal, width = portal_width as usize), Style::default().fg(C_ASGARD).add_modifier(Modifier::BOLD)),
    ]));

    // Middle 2: The Core Blackbox Transit
    let mut transit_line = String::new();
    let box_str = "[📦 BLACKBOX]";
    let box_len = 13;

    let safe_slider = box_slider.min(bridge_width.saturating_sub(box_len));
    for i in 0..bridge_width {
        if i == safe_slider {
            transit_line.push_str(box_str);
        } else if i > safe_slider && i < safe_slider + box_len {
            continue;
        } else {
            let ch = if (i + tick) % 2 == 0 { "»" } else { " " };
            transit_line.push_str(ch);
        }
        if transit_line.chars().count() >= bridge_width {
            break;
        }
    }

    let box_color = if st.finished { C_ASGARD } else { C_GOLD };

    lines.push(Line::from(vec![
        Span::styled(format!("{:^width$}", "│   NTFS Volume    │", width = portal_width as usize), Style::default().fg(C_MIDGARD)),
        Span::styled(format!("{:<width$}", transit_line, width = bridge_width), Style::default().fg(box_color).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{:^width$}", "│  Btrfs Subvols   │", width = portal_width as usize), Style::default().fg(C_ASGARD)),
    ]));

    // Middle 3: Lower Particle Stream
    let mut stream_lower = String::new();
    for i in 0..bridge_width {
        if i == box_slider {
            stream_lower.push_str("▼▼");
        } else {
            let r_idx = (i + tick * 3 + 7) % runes.len();
            stream_lower.push_str(runes[r_idx]);
        }
        if stream_lower.chars().count() >= bridge_width {
            break;
        }
    }

    lines.push(Line::from(vec![
        Span::styled(format!("{:^width$}", "└──────────────────┘", width = portal_width as usize), Style::default().fg(C_MIDGARD)),
        Span::styled(format!("{:<width$}", stream_lower, width = bridge_width), Style::default().fg(C_RUNES)),
        Span::styled(format!("{:^width$}", "└──────────────────┘", width = portal_width as usize), Style::default().fg(C_ASGARD)),
    ]));

    // Bottom beam of the bridge
    let mut bot_beam = String::from("╘");
    for i in 0..bridge_width {
        let sym = if (i + tick) % 3 == 0 { "═" } else { "─" };
        bot_beam.push_str(sym);
    }
    bot_beam.push('╛');

    lines.push(Line::from(vec![
        Span::styled(format!("{:^width$}", "      (Start)       ", width = portal_width as usize), Style::default().fg(C_MUTED)),
        Span::styled(bot_beam, Style::default().fg(C_TITLE)),
        Span::styled(format!("{:^width$}", "      (Ziel)        ", width = portal_width as usize), Style::default().fg(C_MUTED)),
    ]));

    let p = Paragraph::new(lines).alignment(Alignment::Left);
    f.render_widget(p, inner);
}

fn render_step_matrix(f: &mut Frame, area: Rect, st: &TransitStatus) {
    let steps = [
        (1, "Scan & Shrink-Planung", "Block-Devices ermitteln & NTFS-Grenze berechnen"),
        (2, "NTFS-Verkleinerung", "Zerstörungsfreier ntfsresize & GPT-Partitionierung"),
        (3, "Btrfs-Schmiede", "Subvolumes (@root, @home, @blackbox) & /etc/fstab"),
        (4, "Seelentransit (Injektion)", "Blackbox entpacken & POSIX-Rechte (0700/0600)"),
        (5, "Systemd-Signalfeuer", "systemd-boot auf ESP & Flatpak First-Boot Dienst"),
    ];

    let mut lines = Vec::new();

    for (num, title, desc) in steps {
        let (icon, color) = if st.step > num || st.finished {
            ("[✓]", C_ASGARD)
        } else if st.step == num {
            ("[▶]", C_GOLD)
        } else {
            ("[ ]", C_MUTED)
        };

        lines.push(Line::from(vec![
            Span::styled(format!("  {icon} Phase {num}: "), Style::default().fg(color).add_modifier(Modifier::BOLD)),
            Span::styled(format!("{:<28}", title), Style::default().fg(if st.step == num { C_TEXT } else { C_MUTED })),
            Span::styled(format!(" │ {desc}"), Style::default().fg(C_MUTED)),
        ]));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(C_MUTED))
        .border_type(BorderType::Rounded)
        .title(Span::styled(" PHASEN-TELEMETRIE ", Style::default().fg(C_TITLE).add_modifier(Modifier::BOLD)));

    let p = Paragraph::new(lines).block(block);
    f.render_widget(p, area);
}

fn render_bottom_status(f: &mut Frame, area: Rect, st: &TransitStatus) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(2)])
        .split(area);

    // Progress Gauge
    let gauge_title = format!(" FORTSCHRITT: {}% │ {} ", st.percent, st.status_text);
    let gauge = Gauge::default()
        .block(Block::default())
        .gauge_style(
            Style::default()
                .fg(if st.finished { C_ASGARD } else { C_TITLE })
                .bg(C_MUTED)
                .add_modifier(Modifier::BOLD),
        )
        .percent(st.percent)
        .label(Span::styled(gauge_title, Style::default().fg(C_TEXT).add_modifier(Modifier::BOLD)));
    f.render_widget(gauge, chunks[0]);

    // Bottom Message / Completion Alert
    let bottom_text = if let Some(err) = &st.error {
        Line::from(vec![
            Span::styled("  [FEHLER] ", Style::default().fg(C_MIDGARD).add_modifier(Modifier::BOLD)),
            Span::styled(err, Style::default().fg(C_TEXT)),
            Span::styled(" (Drücke Enter zum Beenden)", Style::default().fg(C_MUTED)),
        ])
    } else if st.finished {
        Line::from(vec![
            Span::styled("  ✦ ✦ ✦ TRANSIT VOLLENDET! ", Style::default().fg(C_ASGARD).add_modifier(Modifier::BOLD)),
            Span::styled("Willkommen in Asgard! Automatischer Neustart in 5 Sekunden... ", Style::default().fg(C_GOLD).add_modifier(Modifier::BOLD)),
            Span::styled("(Tippe 'reboot -f' zum sofortigen Sprung)", Style::default().fg(C_MUTED)),
        ])
    } else {
        Line::from(vec![
            Span::styled("  » Status: ", Style::default().fg(C_GOLD)),
            Span::styled(&st.status_text, Style::default().fg(C_TEXT)),
        ])
    };

    let p = Paragraph::new(vec![bottom_text]);
    f.render_widget(p, chunks[1]);
}
