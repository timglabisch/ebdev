use crate::command::{MutagenSessionProgress, MutagenSyncPhase};
use std::collections::HashMap;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

const BAR_WIDTH: usize = 10;
const DIR_BAR_WIDTH: usize = 16;
const MAX_DIRS: usize = 5;

/// Visual properties derived from a sync phase.
struct PhaseStyle {
    icon: &'static str,
    icon_style: Style,
    name_style: Style,
    bar_style: Style,
}

impl PhaseStyle {
    fn from_phase(phase: &MutagenSyncPhase) -> Self {
        match phase {
            MutagenSyncPhase::Ready => Self {
                icon: "✓",
                icon_style: Style::default().fg(Color::Green),
                name_style: Style::default(),
                bar_style: Style::default().fg(Color::Green),
            },
            MutagenSyncPhase::Active => Self {
                icon: "●",
                icon_style: Style::default().fg(Color::Yellow),
                name_style: Style::default(),
                bar_style: Style::default().fg(Color::Yellow),
            },
            MutagenSyncPhase::Pending => Self {
                icon: "◌",
                icon_style: Style::default().fg(Color::DarkGray),
                name_style: Style::default().fg(Color::DarkGray),
                bar_style: Style::default().fg(Color::DarkGray),
            },
            MutagenSyncPhase::Halted(_) => Self {
                icon: "✗",
                icon_style: Style::default().fg(Color::Red),
                name_style: Style::default().fg(Color::Red),
                bar_style: Style::default().fg(Color::Red),
            },
        }
    }
}

fn progress_bar(percent: u8, width: usize, style: Style) -> Vec<Span<'static>> {
    let filled = (percent as usize * width / 100).min(width);
    let empty = width - filled;
    vec![
        Span::styled("█".repeat(filled), style),
        Span::styled("░".repeat(empty), Style::default().fg(Color::DarkGray)),
    ]
}

/// Truncate a path for display. If too long, show `…/last/segments`.
fn truncate_path(path: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if path.len() <= max_width {
        return path.to_string();
    }
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return path[..max_width].to_string();
    }
    let mut result = String::new();
    for part in parts.iter().rev() {
        let candidate = if result.is_empty() {
            part.to_string()
        } else {
            format!("{}/{}", part, result)
        };
        if candidate.len() + 2 > max_width {
            break;
        }
        result = candidate;
    }
    if result.len() < path.len() {
        let truncated = format!("…/{}", result);
        if truncated.len() <= max_width {
            truncated
        } else {
            format!("…{}", &path[path.len().saturating_sub(max_width.saturating_sub(1))..])
        }
    } else {
        result
    }
}

/// Format a file count compactly (e.g., 8200 → "8.2k")
fn format_count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        format!("{}", n)
    }
}

/// Extract the first path segment (top-level directory) from a file path.
fn first_dir_segment(path: &str) -> String {
    let trimmed = path.trim_start_matches('/');
    let parts: Vec<&str> = trimmed.split('/').collect();
    if parts.len() > 1 {
        format!("{}/", parts[0])
    } else {
        ".".to_string()
    }
}

struct DirectoryTracker {
    /// directory → attributed file count
    dirs: HashMap<String, u64>,
    /// last known received_files for delta calculation
    last_received_files: u64,
    /// current file path (for display)
    current_path: Option<String>,
}

impl DirectoryTracker {
    fn new() -> Self {
        Self {
            dirs: HashMap::new(),
            last_received_files: 0,
            current_path: None,
        }
    }

    fn update(&mut self, session: &MutagenSessionProgress) {
        if let Some(ref file) = session.current_file {
            self.current_path = Some(file.clone());
            let delta = session.files_done.saturating_sub(self.last_received_files);
            if delta > 0 {
                let dir = first_dir_segment(file);
                *self.dirs.entry(dir).or_insert(0) += delta;
                self.last_received_files = session.files_done;
            }
        } else if session.files_done == 0 {
            self.current_path = None;
        }
    }

    /// Get top directories sorted by count descending
    fn top_dirs(&self) -> Vec<(&str, u64)> {
        let mut entries: Vec<(&str, u64)> = self.dirs.iter().map(|(k, &v)| (k.as_str(), v)).collect();
        entries.sort_by(|a, b| b.1.cmp(&a.1));
        entries.truncate(MAX_DIRS);
        entries
    }

    fn total_attributed(&self) -> u64 {
        self.dirs.values().sum()
    }

    fn clear(&mut self) {
        self.dirs.clear();
        self.last_received_files = 0;
        self.current_path = None;
    }
}

struct SessionTracker {
    latest: MutagenSessionProgress,
    expanded: bool,
    dir_tracker: DirectoryTracker,
}

impl SessionTracker {
    fn new(session: MutagenSessionProgress) -> Self {
        let mut tracker = Self {
            latest: session,
            expanded: false,
            dir_tracker: DirectoryTracker::new(),
        };
        tracker.dir_tracker.update(&tracker.latest);
        tracker
    }

    fn update(&mut self, session: MutagenSessionProgress) {
        self.dir_tracker.update(&session);

        // Auto-expand when staging with file info
        if session.phase == MutagenSyncPhase::Active && session.current_file.is_some() {
            self.expanded = true;
        }

        // Auto-collapse when ready
        if session.phase == MutagenSyncPhase::Ready {
            self.expanded = false;
            self.dir_tracker.clear();
        }

        self.latest = session;
    }

    /// Lines needed: 1 (session line) + expanded content
    fn height(&self) -> u16 {
        if !self.expanded {
            return 1;
        }
        let dir_count = self.dir_tracker.top_dirs().len().min(MAX_DIRS) as u16;
        let current_file_line = if self.dir_tracker.current_path.is_some() { 1 } else { 0 };
        1 + dir_count + current_file_line
    }

    /// Render session header line (icon + name + status + bar + file counts)
    fn render_header_line(&self) -> Line<'static> {
        let s = &self.latest;
        let ps = PhaseStyle::from_phase(&s.phase);

        let mut spans = vec![
            Span::raw("  "),
            Span::styled(ps.icon, ps.icon_style),
            Span::raw(" "),
            Span::styled(format!("{:<12}", s.name), ps.name_style),
            Span::styled(format!("{:<14}", s.status_label), Style::default().fg(Color::DarkGray)),
        ];
        spans.extend(progress_bar(s.percent, BAR_WIDTH, ps.bar_style));

        if s.files_total > 0 {
            spans.push(Span::styled(
                format!("  {:>3}%  {}/{}",
                    s.percent,
                    format_count(s.files_done),
                    format_count(s.files_total),
                ),
                Style::default().fg(Color::DarkGray),
            ));
        }

        Line::from(spans)
    }

    /// Render expanded directory breakdown + current file lines
    fn render_expanded_lines(&self, max_lines: usize, inner_width: usize) -> Vec<Line<'static>> {
        if !self.expanded {
            return Vec::new();
        }

        let mut lines = Vec::new();
        let top_dirs = self.dir_tracker.top_dirs();
        let total = self.dir_tracker.total_attributed().max(1);

        for (dir, count) in &top_dirs {
            if lines.len() >= max_lines {
                break;
            }
            let dir_percent = (*count as f64 / total as f64 * 100.0) as u8;
            let mut spans = vec![
                Span::raw("    "),
                Span::styled(format!("{:<22}", dir), Style::default().fg(Color::DarkGray)),
            ];
            spans.extend(progress_bar(dir_percent, DIR_BAR_WIDTH, Style::default().fg(Color::Cyan)));
            spans.push(Span::styled(
                format!("  {:>3}%  ({})", dir_percent, format_count(*count)),
                Style::default().fg(Color::DarkGray),
            ));
            lines.push(Line::from(spans));
        }

        if let Some(ref path) = self.dir_tracker.current_path {
            if lines.len() < max_lines {
                let max_path_width = inner_width.saturating_sub(6); // "    → " prefix
                let display_path = truncate_path(path, max_path_width);
                lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::styled("→ ", Style::default().fg(Color::DarkGray)),
                    Span::styled(display_path, Style::default().fg(Color::DarkGray)),
                ]));
            }
        }

        lines
    }
}

pub struct MutagenSyncWidget {
    /// Session trackers keyed by name
    trackers: HashMap<String, SessionTracker>,
    /// Ordered session names (preserves display order from last update)
    order: Vec<String>,
}

impl MutagenSyncWidget {
    pub fn new() -> Self {
        Self {
            trackers: HashMap::new(),
            order: Vec::new(),
        }
    }

    pub fn update(&mut self, sessions: &[MutagenSessionProgress]) {
        let current_names: Vec<String> = sessions.iter().map(|s| s.name.clone()).collect();

        // Remove sessions no longer in the list
        self.trackers.retain(|name, _| current_names.contains(name));
        self.order.retain(|name| current_names.contains(name));

        // Update or insert sessions
        for session in sessions {
            if let Some(tracker) = self.trackers.get_mut(&session.name) {
                tracker.update(session.clone());
            } else {
                self.trackers.insert(session.name.clone(), SessionTracker::new(session.clone()));
                self.order.push(session.name.clone());
            }
        }
    }

    pub fn clear(&mut self) {
        self.trackers.clear();
        self.order.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.trackers.is_empty()
    }

    /// Total height needed including border
    pub fn height(&self) -> u16 {
        if self.trackers.is_empty() {
            return 0;
        }
        let content: u16 = self.order.iter()
            .filter_map(|name| self.trackers.get(name))
            .map(|t| t.height())
            .sum();
        content + 2 // borders
    }

    pub fn draw(&self, frame: &mut Frame, area: Rect) {
        if self.trackers.is_empty() || area.height < 3 {
            return;
        }

        let total_percent = self.trackers.values()
            .map(|t| t.latest.percent as u32)
            .sum::<u32>() / self.trackers.len() as u32;
        let title = format!(" Mutagen Sync ─── {}% ", total_percent);

        let any_halted = self.trackers.values().any(|t| matches!(t.latest.phase, MutagenSyncPhase::Halted(_)));
        let all_ready = self.trackers.values().all(|t| t.latest.phase == MutagenSyncPhase::Ready);
        let border_style = if any_halted {
            Style::default().fg(Color::Red)
        } else if all_ready {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::Yellow)
        };

        let max_visible = area.height.saturating_sub(2) as usize;
        let inner_width = area.width.saturating_sub(2) as usize;
        let mut lines: Vec<Line> = Vec::new();

        for name in &self.order {
            let Some(tracker) = self.trackers.get(name) else { continue };
            if lines.len() >= max_visible {
                break;
            }

            lines.push(tracker.render_header_line());

            let remaining = max_visible.saturating_sub(lines.len());
            if remaining > 0 {
                lines.extend(tracker.render_expanded_lines(remaining, inner_width));
            }
        }

        let widget = Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(title),
        );
        frame.render_widget(widget, area);
    }
}
