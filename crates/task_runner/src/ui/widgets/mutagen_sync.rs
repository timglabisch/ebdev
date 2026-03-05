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
        let expanded = session.phase == MutagenSyncPhase::Active && session.current_file.is_some();
        let mut tracker = Self {
            latest: session,
            expanded,
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

        if let Some(ref mode) = s.sync_mode {
            spans.push(Span::styled(
                format!("  {}", mode),
                Style::default().fg(Color::DarkGray),
            ));
        }

        if let Some(interval) = s.polling_interval {
            spans.push(Span::styled(
                format!("  poll:{}s", interval),
                Style::default().fg(Color::DarkGray),
            ));
        }

        if s.files_total > 0 {
            spans.push(Span::styled(
                format!("  {:>3}%  {}/{}",
                    s.percent,
                    format_count(s.files_done),
                    format_count(s.files_total),
                ),
                Style::default().fg(Color::DarkGray),
            ));
        } else if s.endpoint_files > 0 || s.endpoint_dirs > 0 {
            spans.push(Span::styled(
                format!("  {} files, {} dirs",
                    format_count(s.endpoint_files),
                    format_count(s.endpoint_dirs),
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

/// Helper to create a MutagenSessionProgress for tests.
#[cfg(test)]
fn test_session(name: &str, phase: MutagenSyncPhase, status: &str, percent: u8) -> MutagenSessionProgress {
    MutagenSessionProgress {
        name: name.to_string(),
        phase,
        status_label: status.to_string(),
        percent,
        current_file: None,
        files_done: 0,
        files_total: 0,
        total_received_bytes: 0,
        endpoint_files: 0,
        endpoint_dirs: 0,
        polling_interval: None,
        sync_mode: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // Helper function tests
    // ========================================================================

    #[test]
    fn test_format_count_plain() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(1), "1");
        assert_eq!(format_count(999), "999");
    }

    #[test]
    fn test_format_count_thousands() {
        assert_eq!(format_count(1_000), "1.0k");
        assert_eq!(format_count(1_500), "1.5k");
        assert_eq!(format_count(8_200), "8.2k");
        assert_eq!(format_count(999_999), "1000.0k");
    }

    #[test]
    fn test_format_count_millions() {
        assert_eq!(format_count(1_000_000), "1.0M");
        assert_eq!(format_count(2_500_000), "2.5M");
    }

    #[test]
    fn test_first_dir_segment_nested() {
        assert_eq!(first_dir_segment("vendor/autoload.php"), "vendor/");
        assert_eq!(first_dir_segment(".phpstan/cache/file.php"), ".phpstan/");
        assert_eq!(first_dir_segment("/src/main.rs"), "src/");
    }

    #[test]
    fn test_first_dir_segment_root_file() {
        assert_eq!(first_dir_segment("file.txt"), ".");
        assert_eq!(first_dir_segment("Makefile"), ".");
    }

    #[test]
    fn test_truncate_path_short() {
        assert_eq!(truncate_path("src/main.rs", 30), "src/main.rs");
    }

    #[test]
    fn test_truncate_path_long() {
        let long = "vendor/composer/autoload_classmap.php";
        let result = truncate_path(long, 20);
        assert!(result.len() <= 20);
        assert!(result.starts_with('…'));
    }

    #[test]
    fn test_truncate_path_zero_width() {
        assert_eq!(truncate_path("anything", 0), "");
    }

    // ========================================================================
    // DirectoryTracker tests
    // ========================================================================

    #[test]
    fn test_directory_tracker_basic() {
        let mut tracker = DirectoryTracker::new();
        let mut session = test_session("app", MutagenSyncPhase::Active, "staging", 50);
        session.current_file = Some("vendor/autoload.php".to_string());
        session.files_done = 100;
        session.files_total = 1000;

        tracker.update(&session);

        assert_eq!(tracker.total_attributed(), 100);
        let dirs = tracker.top_dirs();
        assert_eq!(dirs.len(), 1);
        assert_eq!(dirs[0].0, "vendor/");
        assert_eq!(dirs[0].1, 100);
    }

    #[test]
    fn test_directory_tracker_delta_accumulation() {
        let mut tracker = DirectoryTracker::new();
        let mut session = test_session("app", MutagenSyncPhase::Active, "staging", 50);

        // First update: 100 files in vendor/
        session.current_file = Some("vendor/file1.php".to_string());
        session.files_done = 100;
        tracker.update(&session);

        // Second update: 50 more files now in src/
        session.current_file = Some("src/main.rs".to_string());
        session.files_done = 150;
        tracker.update(&session);

        let dirs = tracker.top_dirs();
        assert_eq!(dirs.len(), 2);
        // vendor/ has 100, src/ has 50 → sorted by count desc
        assert_eq!(dirs[0].0, "vendor/");
        assert_eq!(dirs[0].1, 100);
        assert_eq!(dirs[1].0, "src/");
        assert_eq!(dirs[1].1, 50);
    }

    #[test]
    fn test_directory_tracker_no_delta() {
        let mut tracker = DirectoryTracker::new();
        let mut session = test_session("app", MutagenSyncPhase::Active, "staging", 50);
        session.current_file = Some("vendor/file.php".to_string());
        session.files_done = 100;
        tracker.update(&session);

        // Same files_done → no delta
        session.current_file = Some("src/main.rs".to_string());
        tracker.update(&session);

        assert_eq!(tracker.top_dirs().len(), 1); // only vendor/
    }

    #[test]
    fn test_directory_tracker_clear() {
        let mut tracker = DirectoryTracker::new();
        let mut session = test_session("app", MutagenSyncPhase::Active, "staging", 50);
        session.current_file = Some("vendor/file.php".to_string());
        session.files_done = 100;
        tracker.update(&session);

        tracker.clear();
        assert_eq!(tracker.total_attributed(), 0);
        assert!(tracker.current_path.is_none());
        assert!(tracker.top_dirs().is_empty());
    }

    // ========================================================================
    // MutagenSyncWidget tests
    // ========================================================================

    #[test]
    fn test_widget_empty() {
        let widget = MutagenSyncWidget::new();
        assert!(widget.is_empty());
        assert_eq!(widget.height(), 0);
    }

    #[test]
    fn test_widget_update_and_height() {
        let mut widget = MutagenSyncWidget::new();
        widget.update(&[
            test_session("app", MutagenSyncPhase::Ready, "watching", 100),
            test_session("worker", MutagenSyncPhase::Ready, "watching", 100),
        ]);
        assert!(!widget.is_empty());
        // 2 sessions (collapsed, 1 line each) + 2 border = 4
        assert_eq!(widget.height(), 4);
    }

    #[test]
    fn test_widget_removes_stale_sessions() {
        let mut widget = MutagenSyncWidget::new();
        widget.update(&[
            test_session("app", MutagenSyncPhase::Ready, "watching", 100),
            test_session("worker", MutagenSyncPhase::Ready, "watching", 100),
        ]);
        assert_eq!(widget.height(), 4);

        // Update with only one session → other is removed
        widget.update(&[
            test_session("app", MutagenSyncPhase::Ready, "watching", 100),
        ]);
        assert_eq!(widget.height(), 3); // 1 session + 2 border
    }

    #[test]
    fn test_widget_auto_expand_on_staging() {
        let mut widget = MutagenSyncWidget::new();
        let mut staging = test_session("app", MutagenSyncPhase::Active, "staging-beta", 50);
        staging.current_file = Some("vendor/file.php".to_string());
        staging.files_done = 100;
        staging.files_total = 1000;

        widget.update(&[staging]);
        // Expanded: 1 header + 1 dir + 1 current file + 2 border = 5
        assert!(widget.height() > 3);
    }

    #[test]
    fn test_widget_auto_collapse_on_ready() {
        let mut widget = MutagenSyncWidget::new();

        // First: staging with file info → auto-expand
        let mut staging = test_session("app", MutagenSyncPhase::Active, "staging-beta", 50);
        staging.current_file = Some("vendor/file.php".to_string());
        staging.files_done = 100;
        staging.files_total = 1000;
        widget.update(&[staging]);
        let expanded_height = widget.height();

        // Then: ready → auto-collapse
        widget.update(&[test_session("app", MutagenSyncPhase::Ready, "watching", 100)]);
        assert!(widget.height() < expanded_height);
        assert_eq!(widget.height(), 3); // 1 session + 2 border
    }

    #[test]
    fn test_widget_clear() {
        let mut widget = MutagenSyncWidget::new();
        widget.update(&[test_session("app", MutagenSyncPhase::Ready, "watching", 100)]);
        assert!(!widget.is_empty());

        widget.clear();
        assert!(widget.is_empty());
        assert_eq!(widget.height(), 0);
    }

    #[test]
    fn test_widget_shows_sync_mode_in_header() {
        let mut session = test_session("app", MutagenSyncPhase::Ready, "watching", 100);
        session.sync_mode = Some("1w-create".to_string());

        let mut widget = MutagenSyncWidget::new();
        widget.update(&[session]);

        let tracker = widget.trackers.get("app").unwrap();
        let line = tracker.render_header_line();

        let has_mode = line.spans.iter().any(|span| span.content.contains("1w-create"));
        assert!(has_mode, "Expected '1w-create' in header line spans: {:?}",
            line.spans.iter().map(|s| s.content.as_ref()).collect::<Vec<_>>());
    }

    #[test]
    fn test_widget_shows_polling_in_header() {
        let mut session = test_session("app", MutagenSyncPhase::Ready, "watching", 100);
        session.polling_interval = Some(10);

        let mut widget = MutagenSyncWidget::new();
        widget.update(&[session]);

        let tracker = widget.trackers.get("app").unwrap();
        let line = tracker.render_header_line();

        // Check that one of the spans contains "poll:10s"
        let has_poll = line.spans.iter().any(|span| {
            span.content.contains("poll:10s")
        });
        assert!(has_poll, "Expected 'poll:10s' in header line spans: {:?}",
            line.spans.iter().map(|s| s.content.as_ref()).collect::<Vec<_>>());
    }

    #[test]
    fn test_widget_no_polling_in_header() {
        let session = test_session("app", MutagenSyncPhase::Ready, "watching", 100);

        let mut widget = MutagenSyncWidget::new();
        widget.update(&[session]);

        let tracker = widget.trackers.get("app").unwrap();
        let line = tracker.render_header_line();

        let has_poll = line.spans.iter().any(|span| {
            span.content.contains("poll:")
        });
        assert!(!has_poll, "Should not contain 'poll:' in header line when polling is disabled");
    }
}
