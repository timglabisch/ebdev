use super::tab_bar::ActiveTab;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;
use std::cell::Cell;
use std::rc::Rc;

const SPARKLE_FRAMES: &[&str] = &["✻", "✼", "✽", "✾", "✿", "✾", "✽", "✼"];

const BRAND_TEXTS: &[&str] = &[
    "lulzen",
    "timmen",
    "daddeln",
    "fooen..",
    "ultrajannen",
    "migränieren",
    "wuseln",
    "bugseinabauen",
    "spionieren",
    "rumwirren",
    "grübeln",
    "dockern",
    "yannicken",
    "brunieren",
    "frickeln",
    "committen",
    "holzen",
    "easybillen",
    "tüfteln",
    "npmen",
    "phpen",
    "php2php",
    "rubyen",
    "klumpen",
    "normieren",
    "basteln",
    "fischern",
    "sinnieren",
    "debuggen",
    "willen",
    "knobeln",
    "refactorn",
    "christen",
    "compilen",
    "brocken",
    "werkeln",
    "transpilen",
    "easybill",
    "koschern",
    "builden",
    "zaubern",
    "optimieren",
    "rebasen",
    "lucasen",
    "brüten",
    "fetchen",
    "bocklern",
    "stashen",
    "hirnen",
    "felixen",
    "branchen",
    "schrauben",
    "eintimen",
    "linten",
    "nilsen",
    "pondern",
    "easybill",
    "bertolieren",
    "queryen",
    "dengeln",
    "cachen",
    "fritschen",
    "hashen",
    "wurschteln",
    "skalieren",
    "borisen",
    "migrieren",
    "drechseln",
    "bundlen",
    "martineln",
    "rollbacken",
    "glabisieren",
    "brauen",
    "vincenten",
    "jonglieren",
    "landeln",
    "meditieren",
    "phpen",
    "dennisen",
    "crunchen",
    "philippen",
    "halluzinieren",
    "pipelinen",
    "paulen",
    "sedlern",
    "philosophieren",
    "mutieren",
    "joschen",
    "orchestrieren",
    "andreasen",
    "improvisieren",
    "pascalen",
    "mysqlen",
    "verklumpen",
    "rubinieren",
    "schechtmannen",
    "tidben",
    "kleemannen",
    "vendten",
    "hanschen",
    "kalkulieren",
    "schaflitzln",
];

const DOT_FRAMES: &[&str] = &["", ".", "..", "..."];

/// Sparkle icon changes every N ticks (~200ms at 50ms tick rate)
const SPARKLE_INTERVAL: usize = 4;
/// Brand text changes every N ticks (~15s at 50ms tick rate)
const TEXT_INTERVAL: usize = 300;
/// Dots cycle every N ticks (~500ms at 50ms tick rate)
const DOTS_INTERVAL: usize = 10;

/// Smooth HSL hue rotation → RGB color for brand animation
fn hue_to_rgb(tick: usize) -> Color {
    // Full cycle every ~12s (240 ticks at 50ms)
    let hue = (tick % 240) as f64 / 240.0 * 360.0;
    let s = 0.7_f64;
    let l = 0.65_f64;

    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((hue / 60.0) % 2.0 - 1.0_f64).abs());
    let m = l - c / 2.0;

    let (r, g, b) = if hue < 60.0 {
        (c, x, 0.0)
    } else if hue < 120.0 {
        (x, c, 0.0)
    } else if hue < 180.0 {
        (0.0, c, x)
    } else if hue < 240.0 {
        (0.0, x, c)
    } else if hue < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };

    Color::Rgb(
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    )
}

pub fn draw_help(frame: &mut Frame, area: Rect, has_registered_tasks: bool, auto_quit: bool, compact_mode: bool, active_tab: ActiveTab, compact_area: &Rc<Cell<Rect>>, tick: usize) {
    // Brand animation (left side, fixed reservation)
    let sparkle = SPARKLE_FRAMES[(tick / SPARKLE_INTERVAL) % SPARKLE_FRAMES.len()];
    let slot = tick / TEXT_INTERVAL;
    let brand_text = BRAND_TEXTS[slot.wrapping_mul(2654435761) % BRAND_TEXTS.len()];
    let dots = DOT_FRAMES[(tick / DOTS_INTERVAL) % DOT_FRAMES.len()];
    let brand_color = hue_to_rgb(tick);
    let brand_style = Style::default().fg(brand_color).add_modifier(Modifier::BOLD);

    let mut spans = vec![
        Span::styled(format!("{} ", sparkle), brand_style),
        Span::styled("ebdev", brand_style),
        Span::styled(" - ", Style::default().fg(Color::DarkGray)),
        Span::styled(format!("{}{}", brand_text, dots), brand_style),
    ];

    // Pad brand to fixed width so help text doesn't jump
    let brand_width: u16 = spans.iter().map(|s| s.width() as u16).sum();
    const BRAND_RESERVED: u16 = 30;
    let brand_pad = BRAND_RESERVED.saturating_sub(brand_width) as usize;
    spans.push(Span::raw(" ".repeat(brand_pad)));

    // Auto-exit indicator (red background when active)
    if auto_quit {
        spans.push(Span::styled(
            " AUTO-EXIT ",
            Style::default().fg(Color::White).bg(Color::Red),
        ));
        spans.push(Span::raw(" "));
    }

    let dim = Style::default().fg(Color::DarkGray);

    match active_tab {
        ActiveTab::Output => {
            let help_text = if has_registered_tasks {
                "j/k: navigate | Enter: expand/pin | ↑↓: scroll | /: run task | "
            } else {
                "j/k: navigate | Enter: expand/pin | ↑↓: scroll | "
            };
            spans.push(Span::styled(help_text, dim));

            let x_before_compact: u16 = spans.iter().map(|s| s.width() as u16).sum::<u16>() + area.x;

            let compact_label = if compact_mode { "c: sidebar" } else { "c: compact" };
            spans.push(Span::styled(compact_label, dim));
            spans.push(Span::styled(" | x: kill", dim));

            compact_area.set(Rect::new(x_before_compact, area.y, compact_label.len() as u16, 1));

            spans.push(Span::styled(" | q: quit", dim));
        }
        ActiveTab::Tasks => {
            compact_area.set(Rect::default());
            spans.push(Span::styled("j/k: navigate | Enter: run task | 1: output | q: back", dim));
        }
        ActiveTab::Flags => {
            compact_area.set(Rect::default());
            spans.push(Span::styled("j/k: navigate | Space: toggle | 1: output | q: back", dim));
        }
    }

    let help = Paragraph::new(Line::from(spans));
    frame.render_widget(help, area);
}
