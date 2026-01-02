use std::io::{self, Stdout};
use std::path::Path;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap};
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::status::MutagenSession;

pub struct App {
    pub current_stage: i32,
    pub total_stages: i32,
    pub stage_sessions: Vec<SessionState>,
    pub should_quit: bool,
    pub is_final_stage: bool,
    pub message: Option<String>,
}

#[derive(Clone)]
pub struct SessionState {
    pub name: String,
    pub alpha: String,
    pub beta: String,
    pub status: Option<MutagenSession>,
    pub created: bool,
}

impl App {
    pub fn new(total_stages: i32) -> Self {
        Self {
            current_stage: 0,
            total_stages,
            stage_sessions: Vec::new(),
            should_quit: false,
            is_final_stage: false,
            message: None,
        }
    }

    pub fn set_stage(&mut self, stage: i32, sessions: Vec<SessionState>, is_final: bool) {
        self.current_stage = stage;
        self.stage_sessions = sessions;
        self.is_final_stage = is_final;
        self.message = None;
    }

    pub fn update_session_status(&mut self, sessions: &[MutagenSession]) {
        for session in sessions {
            if let Some(state) = self.stage_sessions.iter_mut().find(|s| s.name == session.name) {
                state.status = Some(session.clone());
            }
        }
    }

    pub fn mark_session_created(&mut self, name: &str) {
        if let Some(state) = self.stage_sessions.iter_mut().find(|s| s.name == name) {
            state.created = true;
        }
    }

    pub fn all_sessions_complete(&self) -> bool {
        self.stage_sessions.iter().all(|s| {
            s.status.as_ref().map(|st| st.is_complete()).unwrap_or(false)
        })
    }

    pub fn set_message(&mut self, msg: String) {
        self.message = Some(msg);
    }
}

pub type Tui = Terminal<CrosstermBackend<Stdout>>;

pub fn init() -> io::Result<Tui> {
    io::stdout().execute(EnterAlternateScreen)?;
    enable_raw_mode()?;
    Terminal::new(CrosstermBackend::new(io::stdout()))
}

pub fn restore() -> io::Result<()> {
    io::stdout().execute(LeaveAlternateScreen)?;
    disable_raw_mode()?;
    Ok(())
}

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Header
            Constraint::Min(5),     // Table
            Constraint::Length(3),  // Footer
        ])
        .split(area);

    // Header
    let stage_info = if app.is_final_stage {
        format!("Stage {}/{} [FINAL - WATCH MODE]", app.current_stage + 1, app.total_stages)
    } else {
        format!("Stage {}/{}", app.current_stage + 1, app.total_stages)
    };

    let header = Paragraph::new(stage_info)
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .block(Block::default().borders(Borders::ALL).title(" Mutagen Sync "));
    frame.render_widget(header, chunks[0]);

    // Table
    let header_cells = ["", "Session", "Status", "Target"]
        .iter()
        .map(|h| Cell::from(*h).style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)));
    let header_row = Row::new(header_cells).height(1);

    let rows = app.stage_sessions.iter().map(|session| {
        let (icon, status_text, style) = if let Some(st) = &session.status {
            let icon = if st.is_complete() {
                "●"
            } else if st.is_syncing() {
                "◐"
            } else if st.is_connected() {
                "○"
            } else {
                "◌"
            };

            let style = if st.is_complete() {
                Style::default().fg(Color::Green)
            } else if st.is_syncing() {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::White)
            };

            (icon, st.status_display().to_string(), style)
        } else if session.created {
            ("○", "Created".to_string(), Style::default().fg(Color::Gray))
        } else {
            ("◌", "Pending".to_string(), Style::default().fg(Color::DarkGray))
        };

        Row::new(vec![
            Cell::from(icon).style(style),
            Cell::from(session.name.clone()).style(Style::default().fg(Color::White)),
            Cell::from(status_text).style(style),
            Cell::from(truncate_str(&session.beta, 50)).style(Style::default().fg(Color::Gray)),
        ])
    });

    let widths = [
        Constraint::Length(2),
        Constraint::Length(20),
        Constraint::Length(20),
        Constraint::Min(30),
    ];

    let table = Table::new(rows, widths)
        .header(header_row)
        .block(Block::default().borders(Borders::ALL).title(" Sessions "));
    frame.render_widget(table, chunks[1]);

    // Footer
    let footer_text = if let Some(msg) = &app.message {
        msg.clone()
    } else if app.is_final_stage {
        "Press 'q' to quit".to_string()
    } else {
        "Syncing... Press 'q' to abort".to_string()
    };

    let footer = Paragraph::new(footer_text)
        .style(Style::default().fg(Color::Gray))
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(footer, chunks[2]);
}

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("...{}", &s[s.len() - max_len + 3..])
    }
}

pub async fn poll_status(mutagen_bin: &Path) -> Vec<MutagenSession> {
    let output = Command::new(mutagen_bin)
        .args(["sync", "list", "--template", "{{json .}}"])
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            serde_json::from_str(&stdout).unwrap_or_default()
        }
        _ => Vec::new(),
    }
}

pub fn handle_events() -> io::Result<bool> {
    if event::poll(Duration::from_millis(100))? {
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press && key.code == KeyCode::Char('q') {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub enum TuiMessage {
    UpdateStatus(Vec<MutagenSession>),
    SessionCreated(String),
    StageComplete,
    SetMessage(String),
    Quit,
}

pub fn spawn_status_poller(
    mutagen_bin: std::path::PathBuf,
    tx: mpsc::Sender<TuiMessage>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let sessions = poll_status(&mutagen_bin).await;
            if tx.send(TuiMessage::UpdateStatus(sessions)).await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    })
}
