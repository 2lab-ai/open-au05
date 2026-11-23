//! Interactive terminal UI (`open-au05 tui`) for live monitoring and control
//! of the AU05 device, battery, LEDs, bindings, dictation, and AI agent hooks.

use crate::control;
use crate::daemon::DaemonStatus;
use crate::system;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, List, ListItem, Paragraph, Wrap};
use ratatui::Terminal;
use serde_json::json;
use std::io::stdout;
use std::time::{Duration, Instant};

pub fn run_tui() -> Result<(), String> {
    enable_raw_mode().map_err(|e| format!("failed to enable raw mode: {e}"))?;
    stdout()
        .execute(EnterAlternateScreen)
        .map_err(|e| format!("failed to enter alternate screen: {e}"))?;

    let backend = ratatui::backend::CrosstermBackend::new(stdout());
    let mut terminal =
        Terminal::new(backend).map_err(|e| format!("failed to create terminal: {e}"))?;

    let res = tui_loop(&mut terminal);

    let _ = disable_raw_mode();
    let _ = stdout().execute(LeaveAlternateScreen);
    res
}

fn fetch_status() -> Result<DaemonStatus, String> {
    let val = control::send_command(&json!({"cmd": "status"}))?;
    serde_json::from_value(val).map_err(|e| format!("invalid status payload: {e}"))
}

fn tui_loop(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
) -> Result<(), String> {
    let mut last_poll = Instant::now() - Duration::from_secs(10);
    let mut status: Option<DaemonStatus> = None;
    let mut daemon_err: Option<String> = None;
    let mut notice = String::from("Connected to open-au05 TUI. Press [q] to quit.");
    let doctor_checks = system::run_doctor(None);

    loop {
        if last_poll.elapsed() >= Duration::from_millis(400) {
            last_poll = Instant::now();
            match fetch_status() {
                Ok(s) => {
                    status = Some(s);
                    daemon_err = None;
                }
                Err(e) => {
                    status = None;
                    daemon_err = Some(e);
                }
            }
        }

        terminal
            .draw(|f| {
                let area = f.area();
                let root = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3),
                        Constraint::Length(6),
                        Constraint::Min(10),
                        Constraint::Length(3),
                    ])
                    .split(area);

                // Header
                let header_text = if let Some(ref s) = status {
                    let conn = if s.connected && s.authenticated {
                        Span::styled(
                            "● ONLINE",
                            Style::default()
                                .fg(Color::Green)
                                .add_modifier(Modifier::BOLD),
                        )
                    } else {
                        Span::styled(
                            "○ WAITING FOR AU05",
                            Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::BOLD),
                        )
                    };
                    let mic = if s.mic_linked {
                        Span::styled("  Mic: LINKED", Style::default().fg(Color::Cyan))
                    } else {
                        Span::styled("  Mic: OFFLINE", Style::default().fg(Color::DarkGray))
                    };
                    let mode = if s.paused {
                        Span::styled("  [PAUSED]", Style::default().fg(Color::Red))
                    } else {
                        Span::styled("  [FORWARDING]", Style::default().fg(Color::Green))
                    };
                    Line::from(vec![
                        Span::styled(
                            " open-au05 ",
                            Style::default()
                                .fg(Color::Black)
                                .bg(Color::Cyan)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::raw("  "),
                        conn,
                        mic,
                        mode,
                        Span::raw(format!("  Dictation: {:?}", s.dictation_state)),
                    ])
                } else {
                    Line::from(vec![
                        Span::styled(
                            " open-au05 ",
                            Style::default()
                                .fg(Color::Black)
                                .bg(Color::Yellow)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            "  Daemon offline — start with `open-au05 daemon`",
                            Style::default().fg(Color::Yellow),
                        ),
                    ])
                };
                let header = Paragraph::new(header_text)
                    .block(Block::default().borders(Borders::ALL).title("Ulanzi Vibe Key AU05"));
                f.render_widget(header, root[0]);

                // Top status row: Battery + LEDs + Dictation
                let top = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([
                        Constraint::Percentage(34),
                        Constraint::Percentage(33),
                        Constraint::Percentage(33),
                    ])
                    .split(root[1]);

                let (pct, bat_label) = match status.as_ref().and_then(|s| s.battery.as_ref()) {
                    Some(b) => (
                        b.percent.min(100),
                        format!(
                            "{}% ({:.2}V){}",
                            b.percent,
                            f64::from(b.millivolts) / 1000.0,
                            if b.charging { " ⚡" } else { "" }
                        ),
                    ),
                    None => (0, "n/a".into()),
                };
                let gauge = Gauge::default()
                    .block(Block::default().borders(Borders::ALL).title("Battery"))
                    .gauge_style(Style::default().fg(if pct > 20 {
                        Color::Green
                    } else {
                        Color::Red
                    }))
                    .percent(pct)
                    .label(bat_label);
                f.render_widget(gauge, top[0]);

                let led_lines = if let Some(ref s) = status {
                    vec![
                        Line::from(format!("State: {}", s.active_led_state)),
                        Line::from(format!(
                            "Confirm: {:<8} Cancel: {}",
                            s.led_pattern[0], s.led_pattern[1]
                        )),
                        Line::from(format!("Knob:    {}", s.led_pattern[2])),
                    ]
                } else {
                    vec![Line::from("LED state unavailable")]
                };
                f.render_widget(
                    Paragraph::new(led_lines)
                        .block(Block::default().borders(Borders::ALL).title("Indicator LEDs")),
                    top[1],
                );

                let dict_lines = if let Some(ref s) = status {
                    vec![
                        Line::from(format!("State: {:?}", s.dictation_state)),
                        Line::from(format!(
                            "Last:  {}",
                            s.last_transcript.as_deref().unwrap_or("(none)")
                        )),
                        Line::from(format!(
                            "Error: {}",
                            s.last_error.as_deref().unwrap_or("none")
                        )),
                    ]
                } else {
                    vec![Line::from(
                        daemon_err
                            .as_deref()
                            .unwrap_or("Daemon not running"),
                    )]
                };
                f.render_widget(
                    Paragraph::new(dict_lines)
                        .wrap(Wrap { trim: true })
                        .block(Block::default().borders(Borders::ALL).title("Voice Dictation")),
                    top[2],
                );

                // Main area: Left = Controls + Agent Hooks / Doctor, Right = Live Event Log
                let mid = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(48), Constraint::Percentage(52)])
                    .split(root[2]);

                if let Some(ref s) = status {
                    let left = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
                        .split(mid[0]);

                    let btn_items: Vec<ListItem> = s
                        .buttons
                        .iter()
                        .map(|b| {
                            let val = if b.shortcut_text.is_empty() {
                                format!("fixed:{}", b.fixed_function_name)
                            } else {
                                b.shortcut_text.clone()
                            };
                            ListItem::new(format!("{:<11} -> {val}", b.control.name()))
                        })
                        .collect();
                    f.render_widget(
                        List::new(btn_items).block(
                            Block::default()
                                .borders(Borders::ALL)
                                .title("Onboard Controls"),
                        ),
                        left[0],
                    );

                    let agent_items: Vec<ListItem> = if s.agent_sessions.is_empty() {
                        vec![ListItem::new("(no active coding agent hook sessions)")]
                    } else {
                        s.agent_sessions
                            .iter()
                            .map(|a| {
                                ListItem::new(format!(
                                    "{} [{}] — {} ({}s ago)",
                                    a.agent, a.session_id, a.state, a.age_secs
                                ))
                            })
                            .collect()
                    };
                    f.render_widget(
                        List::new(agent_items).block(
                            Block::default()
                                .borders(Borders::ALL)
                                .title("AI Agent Sessions"),
                        ),
                        left[1],
                    );

                    let event_items: Vec<ListItem> = s
                        .recent_events
                        .iter()
                        .rev()
                        .map(|e| ListItem::new(e.as_str()))
                        .collect();
                    f.render_widget(
                        List::new(event_items)
                            .block(Block::default().borders(Borders::ALL).title("Event Log")),
                        mid[1],
                    );
                } else {
                    let items: Vec<ListItem> = doctor_checks
                        .iter()
                        .map(|c| {
                            let mark = if c.ok { "✓" } else { "✗" };
                            ListItem::new(format!("{mark} {}: {}", c.name, c.detail))
                        })
                        .collect();
                    f.render_widget(
                        List::new(items).block(
                            Block::default()
                                .borders(Borders::ALL)
                                .title("System Doctor (Daemon Offline)"),
                        ),
                        root[2],
                    );
                }

                // Footer controls
                let footer = Paragraph::new(format!(
                    "[p] Pause/Resume  [d] Dictate Toggle  [l] Cycle Light Mode  [t] Test LEDs  [r] Reload  [q] Quit  |  {notice}"
                ))
                .block(Block::default().borders(Borders::ALL));
                f.render_widget(footer, root[3]);
            })
            .map_err(|e| format!("render error: {e}"))?;

        if event::poll(Duration::from_millis(80)).map_err(|e| e.to_string())? {
            if let Event::Key(k) = event::read().map_err(|e| e.to_string())? {
                if k.kind == KeyEventKind::Press {
                    match k.code {
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        KeyCode::Char('p') => {
                            let is_paused = status.as_ref().is_some_and(|s| s.paused);
                            let cmd = if is_paused { "resume" } else { "pause" };
                            notice = match control::send_command(&json!({"cmd": cmd})) {
                                Ok(_) => format!("Sent `{cmd}`"),
                                Err(e) => e,
                            };
                            last_poll = Instant::now() - Duration::from_secs(10);
                        }
                        KeyCode::Char('d') => {
                            notice = match control::send_command(&json!({"cmd": "dictate-toggle"}))
                            {
                                Ok(_) => "Toggled dictation".into(),
                                Err(e) => e,
                            };
                            last_poll = Instant::now() - Duration::from_secs(10);
                        }
                        KeyCode::Char('l') => {
                            notice =
                                match control::send_command(&json!({"cmd": "cycle-light-mode"})) {
                                    Ok(v) => format!(
                                        "Light mode -> {}",
                                        v.get("mode").and_then(|m| m.as_str()).unwrap_or("?")
                                    ),
                                    Err(e) => e,
                                };
                            last_poll = Instant::now() - Duration::from_secs(10);
                        }
                        KeyCode::Char('t') => {
                            notice = match control::send_command(&json!({"cmd": "test-leds"})) {
                                Ok(_) => "LED test complete".into(),
                                Err(e) => e,
                            };
                        }
                        KeyCode::Char('r') => {
                            notice = match control::send_command(&json!({"cmd": "reload"})) {
                                Ok(_) => "Config reloaded".into(),
                                Err(e) => e,
                            };
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    Ok(())
}
