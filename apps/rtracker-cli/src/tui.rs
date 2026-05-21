//! Tracker-style TUI: pattern grid with cursor + cell editing, waveform pane
//! with playhead, hot-reload from disk. Edits apply to the audio buffer
//! immediately; `s` persists to disk.

use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Line as CanvasLine};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Terminal;
use rtracker_core::{Cell, Note, Pattern};

use crate::engine::AudioEngine;
use crate::playback::{file_mtime, read_pattern, render_pattern_at};

const TRACK_COLORS: &[Color] = &[
    Color::Cyan,
    Color::Magenta,
    Color::Yellow,
    Color::Green,
    Color::Blue,
    Color::LightRed,
    Color::LightCyan,
    Color::LightMagenta,
];

const CELL_WIDTH: usize = 9;

struct App {
    path: PathBuf,
    pattern: Pattern,
    last_mtime: Option<SystemTime>,
    // Cursor:
    cursor_row: u32,
    cursor_track: usize,
    octave: i32,
    // Edit state:
    dirty: bool,
    /// Set when we save: suppress the immediate self-triggered reload.
    skip_next_reload: bool,
    // Status line:
    status: Option<(String, Instant, StatusKind)>,
    error: Option<String>,
    scroll: u32,
}

#[derive(Clone, Copy)]
enum StatusKind {
    Info,
    Ok,
    Warn,
}

impl App {
    fn new(path: PathBuf) -> Result<Self> {
        let pattern = read_pattern(&path).context("initial pattern read")?;
        let last_mtime = file_mtime(&path);
        Ok(Self {
            last_mtime,
            path,
            pattern,
            cursor_row: 0,
            cursor_track: 0,
            octave: 3,
            dirty: false,
            skip_next_reload: false,
            status: None,
            error: None,
            scroll: 0,
        })
    }

    fn set_status(&mut self, msg: impl Into<String>, kind: StatusKind) {
        self.status = Some((msg.into(), Instant::now(), kind));
    }
}

pub fn run(input: PathBuf) -> Result<()> {
    let mut app = App::new(input)?;
    let initial = render_pattern_at(&app.path, 48000)?.0;
    let engine = AudioEngine::start(initial)?;
    re_render(&app, &engine, true)?;

    let mut terminal = setup_terminal()?;
    let res = event_loop(&mut terminal, &mut app, &engine);
    restore_terminal(&mut terminal)?;
    res
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(out);
    Ok(Terminal::new(backend)?)
}

fn restore_terminal(t: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(t.backend_mut(), LeaveAlternateScreen)?;
    t.show_cursor()?;
    Ok(())
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    engine: &AudioEngine,
) -> Result<()> {
    let tick = Duration::from_millis(33);
    let mut last_tick = Instant::now();

    loop {
        // File watch.
        if let Some(now) = file_mtime(&app.path) {
            if Some(now) != app.last_mtime {
                app.last_mtime = Some(now);
                if app.skip_next_reload {
                    app.skip_next_reload = false;
                } else {
                    match read_pattern(&app.path).and_then(|p| {
                        let (buf, _) = render_pattern_at(&app.path, engine.device_sr)?;
                        Ok((p, buf))
                    }) {
                        Ok((p, buf)) => {
                            app.pattern = p;
                            engine.swap_buffer(buf);
                            app.error = None;
                            // Clamp cursor in case rows shrank.
                            app.cursor_row = app.cursor_row.min(app.pattern.rows.saturating_sub(1));
                            app.cursor_track = app.cursor_track.min(app.pattern.tracks.len().saturating_sub(1));
                            app.set_status("reloaded from disk", StatusKind::Ok);
                        }
                        Err(e) => app.error = Some(format!("reload: {e}")),
                    }
                }
            }
        }
        if let Some((_, t, _)) = app.status {
            if t.elapsed() > Duration::from_millis(1500) {
                app.status = None;
            }
        }

        terminal.draw(|f| draw(f, app, engine))?;

        let timeout = tick.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            if let Event::Key(k) = event::read()? {
                if k.kind != KeyEventKind::Press {
                    continue;
                }
                if handle_key(k.code, k.modifiers, app, engine)? {
                    return Ok(());
                }
            }
        }
        if last_tick.elapsed() >= tick {
            last_tick = Instant::now();
        }
    }
}

/// Returns Ok(true) to quit.
fn handle_key(
    code: KeyCode,
    mods: KeyModifiers,
    app: &mut App,
    engine: &AudioEngine,
) -> Result<bool> {
    match (code, mods) {
        (KeyCode::Char('q'), _) | (KeyCode::Esc, _) => return Ok(true),
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => return Ok(true),

        // Transport
        (KeyCode::Char(' '), _) => engine.toggle_paused(),
        (KeyCode::Char('R'), _) | (KeyCode::Char('r'), KeyModifiers::CONTROL) => engine.rewind(),

        // Cursor movement
        (KeyCode::Up, _) => move_cursor_row(app, -1),
        (KeyCode::Down, _) => move_cursor_row(app, 1),
        (KeyCode::Left, _) => move_cursor_track(app, -1),
        (KeyCode::Right, _) | (KeyCode::Tab, _) => move_cursor_track(app, 1),
        (KeyCode::BackTab, _) => move_cursor_track(app, -1),
        (KeyCode::PageUp, _) => move_cursor_row(app, -(app.pattern.lines_per_beat as i32)),
        (KeyCode::PageDown, _) => move_cursor_row(app, app.pattern.lines_per_beat as i32),
        (KeyCode::Home, _) => app.cursor_row = 0,
        (KeyCode::End, _) => app.cursor_row = app.pattern.rows.saturating_sub(1),

        // Octave
        (KeyCode::Char(d @ '1'..='9'), _) => {
            app.octave = (d as i32) - ('0' as i32);
            app.set_status(format!("octave {}", app.octave), StatusKind::Info);
        }

        // Note entry: lowercase a-g = natural, uppercase A-G = sharp.
        (KeyCode::Char(ch), _) if is_note_letter(ch) => {
            let (letter, sharp) = if ch.is_ascii_uppercase() {
                (ch.to_ascii_uppercase(), true)
            } else {
                (ch.to_ascii_uppercase(), false)
            };
            let name = format_note(letter, sharp, app.octave);
            set_cell(app, Note::Name(name.clone()));
            apply_edit(app, engine)?;
            move_cursor_row(app, 1);                              // step-advance
            app.set_status(format!("set {}", name), StatusKind::Info);
        }

        // Clear
        (KeyCode::Delete, _) | (KeyCode::Backspace, _) => {
            if clear_cell(app) {
                apply_edit(app, engine)?;
                app.set_status("cleared", StatusKind::Info);
            }
        }

        // Save
        (KeyCode::Char('s'), _) => match save_pattern(app) {
            Ok(()) => app.set_status("saved", StatusKind::Ok),
            Err(e) => app.set_status(format!("save failed: {e}"), StatusKind::Warn),
        },

        // Scroll independent of cursor (just nudges visible window if cursor offscreen)
        _ => {}
    }
    Ok(false)
}

fn is_note_letter(ch: char) -> bool {
    matches!(ch.to_ascii_lowercase(), 'a' | 'b' | 'c' | 'd' | 'e' | 'f' | 'g')
}

fn format_note(letter: char, sharp: bool, octave: i32) -> String {
    if sharp {
        format!("{}#{}", letter, octave)
    } else {
        format!("{}-{}", letter, octave)
    }
}

fn move_cursor_row(app: &mut App, delta: i32) {
    let n = app.pattern.rows as i32;
    if n == 0 {
        return;
    }
    let mut r = app.cursor_row as i32 + delta;
    r = r.rem_euclid(n);
    app.cursor_row = r as u32;
    // Keep cursor visible by adjusting scroll.
    let view = 16u32;
    if app.cursor_row < app.scroll {
        app.scroll = app.cursor_row;
    } else if app.cursor_row >= app.scroll + view {
        app.scroll = app.cursor_row.saturating_sub(view - 1);
    }
}

fn move_cursor_track(app: &mut App, delta: i32) {
    let n = app.pattern.tracks.len() as i32;
    if n == 0 {
        return;
    }
    let mut t = app.cursor_track as i32 + delta;
    t = t.rem_euclid(n);
    app.cursor_track = t as usize;
}

fn set_cell(app: &mut App, note: Note) {
    let row = app.cursor_row;
    let track = match app.pattern.tracks.get_mut(app.cursor_track) {
        Some(t) => t,
        None => return,
    };
    if let Some(c) = track.cells.iter_mut().find(|c| c.row == row) {
        c.note = note;
    } else {
        track.cells.push(Cell {
            row,
            note,
            velocity: None,
            duration_rows: None,
            fx: vec![],
            pitch_env: None,
        });
        track.cells.sort_by_key(|c| c.row);
    }
}

fn clear_cell(app: &mut App) -> bool {
    let row = app.cursor_row;
    let Some(track) = app.pattern.tracks.get_mut(app.cursor_track) else {
        return false;
    };
    let before = track.cells.len();
    track.cells.retain(|c| c.row != row);
    track.cells.len() != before
}

fn apply_edit(app: &mut App, engine: &AudioEngine) -> Result<()> {
    re_render(app, engine, true)?;
    app.dirty = true;
    Ok(())
}

/// Compile + render the in-memory pattern, swap into engine.
/// `from_memory = true` uses `app.pattern`; otherwise re-reads disk.
fn re_render(app: &App, engine: &AudioEngine, from_memory: bool) -> Result<()> {
    let buf = if from_memory {
        let mut p = app.pattern.clone();
        p.sample_rate = engine.device_sr;
        let piece = p.compile().context("compile pattern")?;
        let base = app.path.parent().unwrap_or_else(|| Path::new("."));
        rtracker_render::render_with_dir(&piece, base).context("render piece")?
    } else {
        render_pattern_at(&app.path, engine.device_sr)?.0
    };
    engine.swap_buffer(buf);
    Ok(())
}

fn save_pattern(app: &mut App) -> Result<()> {
    let text = serde_json::to_string_pretty(&app.pattern)?;
    std::fs::write(&app.path, text)?;
    app.dirty = false;
    // Suppress our own mtime-change from triggering an external-reload.
    app.skip_next_reload = true;
    app.last_mtime = file_mtime(&app.path);
    Ok(())
}

fn draw(f: &mut ratatui::Frame, app: &App, engine: &AudioEngine) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(10),
            Constraint::Length(1),
        ])
        .split(area);
    draw_header(f, chunks[0], app, engine);
    draw_grid(f, chunks[1], app, engine);
    draw_waveform(f, chunks[2], engine);
    draw_footer(f, chunks[3], app);
}

fn draw_header(f: &mut ratatui::Frame, area: Rect, app: &App, engine: &AudioEngine) {
    let title = app
        .pattern
        .metadata
        .title
        .clone()
        .unwrap_or_else(|| "(untitled)".into());
    let status_lbl = if engine.is_paused() { "⏸ paused" } else { "▶ playing" };
    let status_style = if engine.is_paused() {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
    };
    let info = format!(
        " {} · {} BPM · {} rows · 1/{} · oct {}",
        title, app.pattern.tempo_bpm, app.pattern.rows, app.pattern.lines_per_beat, app.octave
    );
    let dirty_dot = if app.dirty { "●" } else { "○" };
    let dirty_style = if app.dirty {
        Style::default().fg(Color::LightRed).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let mut spans = vec![
        Span::styled("rtracker", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        Span::raw(info),
        Span::raw("  "),
        Span::styled(status_lbl, status_style),
        Span::raw("  "),
        Span::styled(dirty_dot, dirty_style),
    ];
    if let Some((msg, _, kind)) = &app.status {
        let color = match kind {
            StatusKind::Ok => Color::LightGreen,
            StatusKind::Warn => Color::LightRed,
            StatusKind::Info => Color::LightYellow,
        };
        spans.push(Span::raw("  "));
        spans.push(Span::styled(msg.clone(), Style::default().fg(color)));
    }
    if let Some(e) = &app.error {
        spans.push(Span::styled(
            format!("  ✗ {}", e),
            Style::default().fg(Color::Red),
        ));
    }
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(Color::DarkGray));
    f.render_widget(Paragraph::new(Line::from(spans)).block(block), area);
}

fn draw_footer(f: &mut ratatui::Frame, area: Rect, _app: &App) {
    let keys = vec![
        ("a-g", "natural"),
        ("A-G", "sharp"),
        ("1-9", "octave"),
        ("Del", "clear"),
        ("←→↑↓", "cursor"),
        ("PgUp/Dn", "beat"),
        ("Space", "play"),
        ("s", "save"),
        ("q", "quit"),
    ];
    let mut spans = vec![Span::raw(" ")];
    for (i, (k, label)) in keys.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ", Style::default().fg(Color::DarkGray)));
        }
        spans.push(Span::styled(*k, Style::default().fg(Color::Yellow).bold()));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(*label, Style::default().fg(Color::Gray)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_grid(f: &mut ratatui::Frame, area: Rect, app: &App, engine: &AudioEngine) {
    let spr = app.pattern.samples_per_row().max(1);
    let total_rows = app.pattern.rows.max(1) as u64;
    let head_frame = engine.playhead_frame();
    let playhead_row = ((head_frame / spr) % total_rows) as u32;

    let tracks = &app.pattern.tracks;
    let row_count = area.height.saturating_sub(3) as u32;
    let visible_from = app.scroll.min(app.pattern.rows.saturating_sub(1));
    let visible_to = (visible_from + row_count).min(app.pattern.rows);

    let mut lines: Vec<Line> = Vec::with_capacity(area.height as usize);

    // Track header.
    let mut header_spans = vec![Span::styled(
        "  #  ",
        Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
    )];
    for (i, t) in tracks.iter().enumerate() {
        let mut style = Style::default()
            .fg(TRACK_COLORS[i % TRACK_COLORS.len()])
            .add_modifier(Modifier::BOLD);
        if i == app.cursor_track {
            style = style.add_modifier(Modifier::UNDERLINED);
        }
        header_spans.push(Span::styled(format!("{:^w$}", short(&t.name), w = CELL_WIDTH), style));
    }
    lines.push(Line::from(header_spans));
    lines.push(Line::from(Span::styled(
        "─".repeat(area.width as usize),
        Style::default().fg(Color::DarkGray),
    )));

    for r in visible_from..visible_to {
        let is_playhead = r == playhead_row;
        let is_cursor_row = r == app.cursor_row;
        let is_beat = r % app.pattern.lines_per_beat == 0;

        let arrow = if is_playhead { "▶" } else { " " };
        let row_num_style = if is_playhead {
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
        } else if is_beat {
            Style::default().fg(Color::Gray).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let mut spans = vec![
            Span::styled(arrow, Style::default().fg(Color::Yellow).bold()),
            Span::styled(format!(" {:02} ", r), row_num_style),
        ];

        for (i, t) in tracks.iter().enumerate() {
            let color = TRACK_COLORS[i % TRACK_COLORS.len()];
            let cell = t.cells.iter().find(|c| c.row == r);
            let text = match cell {
                None => "    ·    ".to_string(),
                Some(c) => format!("{:^w$}", format_cell(&c.note), w = CELL_WIDTH),
            };
            let mut style = if cell.is_some() {
                Style::default().fg(color).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let is_cursor_cell = is_cursor_row && i == app.cursor_track;
            if is_cursor_cell {
                style = Style::default()
                    .fg(Color::Black)
                    .bg(color)
                    .add_modifier(Modifier::BOLD);
            } else if is_playhead {
                style = style.bg(Color::Rgb(20, 20, 40));
            } else if is_cursor_row {
                style = style.bg(Color::Rgb(12, 18, 30));
            }
            spans.push(Span::styled(text, style));
        }

        let line = Line::from(spans);
        let line = if is_playhead {
            line.style(Style::default().bg(Color::Rgb(20, 20, 40)))
        } else if is_cursor_row {
            line.style(Style::default().bg(Color::Rgb(12, 18, 30)))
        } else {
            line
        };
        lines.push(line);
    }

    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(Color::DarkGray));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn short(s: &str) -> String {
    if s.chars().count() <= CELL_WIDTH - 2 {
        s.to_string()
    } else {
        s.chars().take(CELL_WIDTH - 2).collect()
    }
}

fn format_cell(note: &Note) -> String {
    match note {
        Note::Name(n) => n.clone(),
        Note::Hz(h) => {
            if *h < 100.0 {
                format!("{:.1}", h)
            } else {
                format!("{}", *h as u32)
            }
        }
    }
}

fn draw_waveform(f: &mut ratatui::Frame, area: Rect, engine: &AudioEngine) {
    let buf = engine.current_buffer();
    let len_frames = buf.len() / 2;
    let head_frame = engine.playhead_frame() as f64;
    let total_frames = len_frames.max(1) as f64;
    let playhead_x = head_frame / total_frames;

    let inner_w = area.width.saturating_sub(2) as usize;
    let bins = inner_w.max(1) * 2;
    let frames_per_bin = (len_frames / bins).max(1);

    let mut peaks: Vec<f32> = Vec::with_capacity(bins);
    for b in 0..bins {
        let start = b * frames_per_bin;
        let end = (start + frames_per_bin).min(len_frames);
        let mut max = 0.0f32;
        for i in start..end {
            let l = buf[i * 2].abs();
            let r = buf[i * 2 + 1].abs();
            let s = l.max(r);
            if s > max {
                max = s;
            }
        }
        peaks.push(max);
    }

    let buf_for_canvas: Arc<Vec<f32>> = buf.clone();
    let canvas = Canvas::default()
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(Span::styled(
                    " loop waveform ",
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                )),
        )
        .marker(Marker::Braille)
        .x_bounds([0.0, 1.0])
        .y_bounds([-1.0, 1.0])
        .paint(move |ctx| {
            let _ = &buf_for_canvas;
            for (i, &p) in peaks.iter().enumerate() {
                let x = i as f64 / peaks.len() as f64;
                ctx.draw(&CanvasLine {
                    x1: x,
                    y1: -(p as f64),
                    x2: x,
                    y2: p as f64,
                    color: amp_color(p),
                });
            }
            ctx.draw(&CanvasLine {
                x1: playhead_x,
                y1: -1.0,
                x2: playhead_x,
                y2: 1.0,
                color: Color::Yellow,
            });
        });
    f.render_widget(canvas, area);
}

fn amp_color(a: f32) -> Color {
    let a = a.clamp(0.0, 1.0);
    if a < 0.25 {
        Color::Rgb(30, 60, 140)
    } else if a < 0.5 {
        Color::Rgb(40, 130, 200)
    } else if a < 0.75 {
        Color::Rgb(180, 90, 200)
    } else {
        Color::Rgb(240, 80, 160)
    }
}
