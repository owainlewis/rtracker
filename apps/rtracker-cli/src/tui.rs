//! Tracker-style TUI: an arrangement of patterns (a song) on top of a pattern
//! grid editor. The grid edits one pattern at a time; an arrangement strip
//! shows the pattern order and lets you add / clone / delete / reorder. Playback
//! has two scopes — loop the current pattern, or play the whole song through —
//! toggled with `m`. Edits apply to the audio buffer immediately; `s` persists
//! to disk (as a song when there's more than one pattern, else a bare pattern).

use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
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
use rtracker_core::{Cell, Note, Pattern, Song};

use crate::engine::AudioEngine;
use crate::playback::{file_mtime, read_song};

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

#[derive(Clone, Copy, PartialEq)]
enum PlayScope {
    /// Loop the pattern currently being edited.
    Pattern,
    /// Play every pattern in arrangement order, then loop.
    Song,
}

struct App {
    path: PathBuf,
    song: Song,
    /// Index of the pattern being viewed / edited.
    current: usize,
    /// True when the file was loaded in song format (or has grown past one
    /// pattern) — controls whether `save` writes a song or a bare pattern.
    loaded_as_song: bool,
    last_mtime: Option<SystemTime>,
    // Cursor:
    cursor_row: u32,
    cursor_track: usize,
    octave: i32,
    // Edit state:
    dirty: bool,
    skip_next_reload: bool,
    follow_playhead: bool,
    play_scope: PlayScope,
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

/// Per-pattern timeline placement when the whole song is rendered at `sr`.
struct Placement {
    start_frame: u64,
    len_frames: u64,
    spr: u64,
    rows: u32,
}

fn spr_at(p: &Pattern, sr: u32) -> u64 {
    ((sr as f64 * 60.0) / (p.tempo_bpm as f64 * p.lines_per_beat as f64))
        .round()
        .max(1.0) as u64
}

impl App {
    fn new(path: PathBuf) -> Result<Self> {
        let (song, loaded_as_song) = read_song(&path).context("initial song read")?;
        let last_mtime = file_mtime(&path);
        Ok(Self {
            last_mtime,
            path,
            // Default to song scope when there's an actual arrangement to play.
            play_scope: if song.patterns.len() > 1 { PlayScope::Song } else { PlayScope::Pattern },
            song,
            current: 0,
            loaded_as_song,
            cursor_row: 0,
            cursor_track: 0,
            octave: 3,
            dirty: false,
            skip_next_reload: false,
            follow_playhead: true,
            status: None,
            error: None,
            scroll: 0,
        })
    }

    fn cur(&self) -> &Pattern {
        &self.song.patterns[self.current]
    }

    fn cur_mut(&mut self) -> &mut Pattern {
        &mut self.song.patterns[self.current]
    }

    /// Cumulative timeline placement of each pattern at sample rate `sr`.
    fn layout(&self, sr: u32) -> Vec<Placement> {
        let mut out = Vec::with_capacity(self.song.patterns.len());
        let mut off = 0u64;
        for p in &self.song.patterns {
            let spr = spr_at(p, sr);
            let len = spr.saturating_mul(p.rows as u64);
            out.push(Placement { start_frame: off, len_frames: len, spr, rows: p.rows });
            off = off.saturating_add(len);
        }
        out
    }

    /// (pattern index, row within it) currently under the playhead.
    fn playing(&self, engine: &AudioEngine) -> (usize, u32) {
        let frame = engine.playhead_frame();
        match self.play_scope {
            PlayScope::Pattern => {
                let p = self.cur();
                let spr = spr_at(p, engine.device_sr).max(1);
                let rows = p.rows.max(1) as u64;
                (self.current, ((frame / spr) % rows) as u32)
            }
            PlayScope::Song => {
                let layout = self.layout(engine.device_sr);
                for (i, pl) in layout.iter().enumerate() {
                    if pl.len_frames > 0 && frame < pl.start_frame + pl.len_frames {
                        let row = ((frame - pl.start_frame) / pl.spr).min(pl.rows.saturating_sub(1) as u64);
                        return (i, row as u32);
                    }
                }
                (self.song.patterns.len().saturating_sub(1), 0)
            }
        }
    }

    fn set_status(&mut self, msg: impl Into<String>, kind: StatusKind) {
        self.status = Some((msg.into(), Instant::now(), kind));
    }
}

pub fn run(input: PathBuf) -> Result<()> {
    let mut app = App::new(input)?;
    let engine = AudioEngine::start(vec![0.0; 2])?;
    re_render(&app, &engine)?;

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
                    match read_song(&app.path) {
                        Ok((song, was_song)) => {
                            app.song = song;
                            app.loaded_as_song = was_song;
                            app.current = app.current.min(app.song.patterns.len().saturating_sub(1));
                            clamp_cursor(app);
                            app.error = None;
                            re_render(app, engine)?;
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

        // Auto-follow the playhead, Renoise-style. In song scope this also
        // switches the viewed pattern to whichever one is currently playing.
        if app.follow_playhead {
            let (play_pat, play_row) = app.playing(engine);
            if app.play_scope == PlayScope::Song && play_pat != app.current {
                app.current = play_pat;
                clamp_cursor(app);
            }
            let term_size = terminal.size()?;
            // Layout below burns: header 3 + strip 1 + scope 10 + footer 1, and
            // the grid block itself burns 3 (track header, separator, border).
            let visible_rows = term_size
                .height
                .saturating_sub(3 + 1 + 10 + 1 + 3)
                .max(1) as u32;
            let half = visible_rows / 2;
            let max_scroll = app.cur().rows.saturating_sub(visible_rows);
            app.scroll = play_row.saturating_sub(half).min(max_scroll);
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

        // Play scope: loop one pattern vs. play the whole song.
        (KeyCode::Char('m'), _) => {
            app.play_scope = match app.play_scope {
                PlayScope::Pattern => PlayScope::Song,
                PlayScope::Song => PlayScope::Pattern,
            };
            engine.rewind();
            re_render(app, engine)?;
            app.set_status(
                match app.play_scope {
                    PlayScope::Pattern => "scope: pattern loop",
                    PlayScope::Song => "scope: whole song",
                },
                StatusKind::Info,
            );
        }

        // --- Arrangement: pattern selection + management ---------------------
        (KeyCode::Char(']'), _) => select_pattern(app, engine, 1)?,
        (KeyCode::Char('['), _) => select_pattern(app, engine, -1)?,
        (KeyCode::Char('n'), _) => add_pattern(app, engine, false)?, // blank section
        (KeyCode::Char('N'), _) => add_pattern(app, engine, true)?,  // clone of current
        (KeyCode::Char('x'), _) => delete_pattern(app, engine)?,
        (KeyCode::Left, KeyModifiers::CONTROL) => move_pattern(app, engine, -1)?,
        (KeyCode::Right, KeyModifiers::CONTROL) => move_pattern(app, engine, 1)?,

        // Pattern length (in beats) and tempo.
        (KeyCode::Char('='), _) | (KeyCode::Char('+'), _) => change_rows(app, engine, 1)?,
        (KeyCode::Char('-'), _) | (KeyCode::Char('_'), _) => change_rows(app, engine, -1)?,
        (KeyCode::Char('.'), _) => change_tempo(app, engine, 1.0)?,
        (KeyCode::Char(','), _) => change_tempo(app, engine, -1.0)?,
        (KeyCode::Char('>'), _) => change_tempo(app, engine, 10.0)?,
        (KeyCode::Char('<'), _) => change_tempo(app, engine, -10.0)?,

        // Cursor movement — manual navigation disengages playhead-follow.
        (KeyCode::Up, _)        => { app.follow_playhead = false; move_cursor_row(app, -1); }
        (KeyCode::Down, _)      => { app.follow_playhead = false; move_cursor_row(app, 1); }
        (KeyCode::Left, _)      => move_cursor_track(app, -1),
        (KeyCode::Right, _) | (KeyCode::Tab, _) => move_cursor_track(app, 1),
        (KeyCode::BackTab, _)   => move_cursor_track(app, -1),
        (KeyCode::PageUp, _)    => { app.follow_playhead = false; move_cursor_row(app, -(app.cur().lines_per_beat as i32)); }
        (KeyCode::PageDown, _)  => { app.follow_playhead = false; move_cursor_row(app, app.cur().lines_per_beat as i32); }
        (KeyCode::Home, _)      => { app.follow_playhead = false; app.cursor_row = 0; }
        (KeyCode::End, _)       => { app.follow_playhead = false; app.cursor_row = app.cur().rows.saturating_sub(1); }

        // Toggle playhead follow.
        (KeyCode::Char('f'), _) => {
            app.follow_playhead = !app.follow_playhead;
            app.set_status(
                if app.follow_playhead { "follow on" } else { "follow off" },
                StatusKind::Info,
            );
        }

        // Octave
        (KeyCode::Char(d @ '1'..='9'), _) => {
            app.octave = (d as i32) - ('0' as i32);
            app.set_status(format!("octave {}", app.octave), StatusKind::Info);
        }

        // Note entry: lowercase a-g = natural, uppercase A-G = sharp.
        (KeyCode::Char(ch), _) if is_note_letter(ch) => {
            let (letter, sharp) = (ch.to_ascii_uppercase(), ch.is_ascii_uppercase());
            let name = format_note(letter, sharp, app.octave);
            let entry_row = app.cursor_row;
            set_cell(app, Note::Name(name.clone()));
            apply_edit(app, engine)?;
            preview_cursor_cell(app, engine, entry_row);
            move_cursor_row(app, 1);                              // step-advance
            app.set_status(format!("set {}", name), StatusKind::Info);
        }

        // Re-preview the cursor cell.
        (KeyCode::Enter, _) => {
            preview_cursor_cell(app, engine, app.cursor_row);
        }

        // Clear
        (KeyCode::Delete, _) | (KeyCode::Backspace, _) => {
            if clear_cell(app) {
                apply_edit(app, engine)?;
                app.set_status("cleared", StatusKind::Info);
            }
        }

        // Save
        (KeyCode::Char('s'), _) => match save(app) {
            Ok(()) => app.set_status("saved", StatusKind::Ok),
            Err(e) => app.set_status(format!("save failed: {e}"), StatusKind::Warn),
        },

        _ => {}
    }
    Ok(false)
}

fn clamp_cursor(app: &mut App) {
    let max_row = app.cur().rows.saturating_sub(1);
    let max_track = app.cur().tracks.len().saturating_sub(1);
    app.cursor_row = app.cursor_row.min(max_row);
    app.cursor_track = app.cursor_track.min(max_track);
}

/// Move the viewed pattern by `delta`, wrapping. In pattern scope this changes
/// what is playing, so we re-render and rewind.
fn select_pattern(app: &mut App, engine: &AudioEngine, delta: i32) -> Result<()> {
    let n = app.song.patterns.len() as i32;
    if n <= 1 {
        return Ok(());
    }
    // Explicitly choosing a pattern means you want to look at it — stop the
    // view from snapping back to whatever is playing.
    app.follow_playhead = false;
    app.current = (app.current as i32 + delta).rem_euclid(n) as usize;
    clamp_cursor(app);
    if app.play_scope == PlayScope::Pattern {
        engine.rewind();
        re_render(app, engine)?;
    }
    app.set_status(format!("pattern {:02}", app.current), StatusKind::Info);
    Ok(())
}

/// Insert a new pattern after the current one. `clone` keeps the cells;
/// otherwise the new pattern shares the instrument/track skeleton but is empty.
fn add_pattern(app: &mut App, engine: &AudioEngine, clone: bool) -> Result<()> {
    let mut p = app.cur().clone();
    if !clone {
        for t in &mut p.tracks {
            t.cells.clear();
        }
    }
    app.current += 1;
    app.song.patterns.insert(app.current, p);
    clamp_cursor(app);
    app.dirty = true;
    engine.rewind();
    re_render(app, engine)?;
    app.set_status(
        if clone { "cloned pattern" } else { "new pattern" },
        StatusKind::Ok,
    );
    Ok(())
}

fn delete_pattern(app: &mut App, engine: &AudioEngine) -> Result<()> {
    if app.song.patterns.len() <= 1 {
        app.set_status("can't delete the last pattern", StatusKind::Warn);
        return Ok(());
    }
    app.song.patterns.remove(app.current);
    app.current = app.current.min(app.song.patterns.len() - 1);
    clamp_cursor(app);
    app.dirty = true;
    engine.rewind();
    re_render(app, engine)?;
    app.set_status("deleted pattern", StatusKind::Info);
    Ok(())
}

/// Reorder the current pattern within the arrangement (and follow it).
fn move_pattern(app: &mut App, engine: &AudioEngine, delta: i32) -> Result<()> {
    let n = app.song.patterns.len() as i32;
    let to = app.current as i32 + delta;
    if n <= 1 || to < 0 || to >= n {
        return Ok(());
    }
    app.song.patterns.swap(app.current, to as usize);
    app.current = to as usize;
    app.dirty = true;
    engine.rewind();
    re_render(app, engine)?;
    app.set_status(format!("moved → {:02}", app.current), StatusKind::Info);
    Ok(())
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
    let n = app.cur().rows as i32;
    if n == 0 {
        return;
    }
    app.cursor_row = (app.cursor_row as i32 + delta).rem_euclid(n) as u32;
    let view = 16u32;
    if app.cursor_row < app.scroll {
        app.scroll = app.cursor_row;
    } else if app.cursor_row >= app.scroll + view {
        app.scroll = app.cursor_row.saturating_sub(view - 1);
    }
}

fn move_cursor_track(app: &mut App, delta: i32) {
    let n = app.cur().tracks.len() as i32;
    if n == 0 {
        return;
    }
    app.cursor_track = (app.cursor_track as i32 + delta).rem_euclid(n) as usize;
}

fn set_cell(app: &mut App, note: Note) {
    let row = app.cursor_row;
    let track_idx = app.cursor_track;
    let Some(track) = app.cur_mut().tracks.get_mut(track_idx) else {
        return;
    };
    if let Some(c) = track.cells.iter_mut().find(|c| c.row == row) {
        c.note = note;
    } else {
        track.cells.push(Cell {
            row,
            note,
            pitch_ratio: None,
            velocity: None,
            duration_rows: None,
            pan: None,
            fx: vec![],
            pitch_env: None,
        });
        track.cells.sort_by_key(|c| c.row);
    }
}

fn clear_cell(app: &mut App) -> bool {
    let row = app.cursor_row;
    let track_idx = app.cursor_track;
    let Some(track) = app.cur_mut().tracks.get_mut(track_idx) else {
        return false;
    };
    let before = track.cells.len();
    track.cells.retain(|c| c.row != row);
    track.cells.len() != before
}

fn apply_edit(app: &mut App, engine: &AudioEngine) -> Result<()> {
    re_render_edit(app, engine)?;
    app.dirty = true;
    Ok(())
}

/// Re-render after a cell edit (one that doesn't change any pattern's length).
/// In pattern scope this re-renders just the current pattern (already cheap).
/// In song scope it re-renders ONLY the edited pattern and splices it into its
/// region of the song buffer — exact, because each event's audio is confined to
/// its own pattern and the mixer is purely additive (see mixer.rs). Avoids
/// recompiling the whole song on every keystroke. Falls back to a full render
/// if the buffer and layout don't line up (e.g. right after a scope switch).
fn re_render_edit(app: &App, engine: &AudioEngine) -> Result<()> {
    if app.play_scope == PlayScope::Pattern {
        return re_render(app, engine);
    }
    let base = app.path.parent().unwrap_or_else(|| Path::new("."));
    let mut p = app.cur().clone();
    p.sample_rate = engine.device_sr;
    let piece = p.compile().context("compile pattern")?;
    let patch = rtracker_render::render_with_dir(&piece, base).context("render pattern")?;

    let layout = app.layout(engine.device_sr);
    let pl = &layout[app.current];
    let start = (pl.start_frame as usize) * 2;
    let region = (pl.len_frames as usize) * 2;
    let cur = engine.current_buffer();
    if patch.len() == region && start + region <= cur.len() {
        let mut buf = cur.as_ref().clone();
        buf[start..start + region].copy_from_slice(&patch);
        engine.swap_buffer(buf);
        Ok(())
    } else {
        re_render(app, engine)
    }
}

/// Grow or shrink the current pattern by `delta_beats` beats (one beat =
/// `lines_per_beat` rows). Cells that fall outside the new length are dropped.
fn change_rows(app: &mut App, engine: &AudioEngine, delta_beats: i32) -> Result<()> {
    let lpb = app.cur().lines_per_beat.max(1) as i32;
    let new_rows = (app.cur().rows as i32 + lpb * delta_beats).max(lpb) as u32;
    {
        let p = app.cur_mut();
        p.rows = new_rows;
        for t in &mut p.tracks {
            t.cells.retain(|c| c.row < new_rows);
        }
    }
    clamp_cursor(app);
    app.dirty = true;
    engine.rewind();
    re_render(app, engine)?; // length change shifts the song layout → full render
    app.set_status(format!("rows {}", new_rows), StatusKind::Info);
    Ok(())
}

/// Nudge the current pattern's tempo. Per-pattern tempo is allowed; note that
/// changing it desyncs any fixed-length sample slices from the row grid.
fn change_tempo(app: &mut App, engine: &AudioEngine, delta: f32) -> Result<()> {
    let new = (app.cur().tempo_bpm + delta).clamp(20.0, 999.0);
    app.cur_mut().tempo_bpm = new;
    app.dirty = true;
    engine.rewind();
    re_render(app, engine)?;
    app.set_status(format!("{:.0} BPM", new), StatusKind::Info);
    Ok(())
}

/// Compile + render the in-memory song (or just the current pattern, depending
/// on scope) at the engine's device rate, then swap into the audio engine.
fn re_render(app: &App, engine: &AudioEngine) -> Result<()> {
    let base = app.path.parent().unwrap_or_else(|| Path::new("."));
    let piece = match app.play_scope {
        PlayScope::Pattern => {
            let mut p = app.cur().clone();
            p.sample_rate = engine.device_sr;
            p.compile().context("compile pattern")?
        }
        PlayScope::Song => {
            let mut song = app.song.clone();
            for p in &mut song.patterns {
                p.sample_rate = engine.device_sr;
            }
            song.compile().context("compile song")?
        }
    };
    let buf = rtracker_render::render_with_dir(&piece, base).context("render piece")?;
    engine.swap_buffer(buf);
    Ok(())
}

/// Render the cursor cell as a one-shot and send it to the preview bus.
fn preview_cursor_cell(app: &App, engine: &AudioEngine, row: u32) {
    let track_idx = app.cursor_track;
    let Some(track) = app.cur().tracks.get(track_idx) else { return };
    let Some(cell_ref) = track.cells.iter().find(|c| c.row == row) else { return };

    let preview_rows: u32 = 16;
    let mut mini_track = track.clone();
    mini_track.cells = vec![Cell { row: 0, ..cell_ref.clone() }];
    let mini = Pattern {
        sample_rate: engine.device_sr,
        tempo_bpm: app.cur().tempo_bpm,
        lines_per_beat: app.cur().lines_per_beat,
        rows: preview_rows,
        voices: app.cur().voices.clone(),
        samples: app.cur().samples.clone(),
        tracks: vec![mini_track],
        metadata: Default::default(),
    };

    let base = app.path.parent().unwrap_or_else(|| Path::new("."));
    if let Ok(piece) = mini.compile() {
        if let Ok(buf) = rtracker_render::render_with_dir(&piece, base) {
            engine.play_preview(buf);
        }
    }
}

fn save(app: &mut App) -> Result<()> {
    // Write a song once there's a real arrangement, or if we loaded one;
    // otherwise keep the original bare-pattern format for round-tripping.
    let text = if app.loaded_as_song || app.song.patterns.len() > 1 {
        app.loaded_as_song = true;
        serde_json::to_string_pretty(&app.song)?
    } else {
        serde_json::to_string_pretty(&app.song.patterns[0])?
    };
    std::fs::write(&app.path, text)?;
    app.dirty = false;
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
            Constraint::Length(1),
            Constraint::Min(8),
            Constraint::Length(10),
            Constraint::Length(1),
        ])
        .split(area);
    draw_header(f, chunks[0], app, engine);
    draw_song_strip(f, chunks[1], app, engine);
    draw_grid(f, chunks[2], app, engine);
    draw_waveform(f, chunks[3], engine);
    draw_footer(f, chunks[4], app);
}

fn draw_header(f: &mut ratatui::Frame, area: Rect, app: &App, engine: &AudioEngine) {
    let title = app
        .song
        .metadata
        .title
        .clone()
        .or_else(|| app.cur().metadata.title.clone())
        .unwrap_or_else(|| "(untitled)".into());
    let status_lbl = if engine.is_paused() { "⏸ paused" } else { "▶ playing" };
    let status_style = if engine.is_paused() {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
    };
    let scope_lbl = match app.play_scope {
        PlayScope::Pattern => "▭ pattern",
        PlayScope::Song => "≣ song",
    };
    let p = app.cur();
    let info = format!(
        " {} · pat {:02}/{:02} · {} BPM · {} rows · 1/{} · oct {}",
        title, app.current, app.song.patterns.len().saturating_sub(1),
        p.tempo_bpm, p.rows, p.lines_per_beat, app.octave
    );
    let dirty_dot = if app.dirty { "●" } else { "○" };
    let dirty_style = if app.dirty {
        Style::default().fg(Color::LightRed).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let follow_lbl = if app.follow_playhead { "↻ follow" } else { "  free " };
    let follow_style = if app.follow_playhead {
        Style::default().fg(Color::LightCyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let mut spans = vec![
        Span::styled("rtracker", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        Span::raw(info),
        Span::raw("  "),
        Span::styled(status_lbl, status_style),
        Span::raw("  "),
        Span::styled(scope_lbl, Style::default().fg(Color::LightBlue)),
        Span::raw("  "),
        Span::styled(follow_lbl, follow_style),
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
        spans.push(Span::styled(format!("  ✗ {}", e), Style::default().fg(Color::Red)));
    }
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(Color::DarkGray));
    f.render_widget(Paragraph::new(Line::from(spans)).block(block), area);
}

/// The arrangement strip: one chip per pattern in order. The viewed pattern is
/// bracketed/inverse; the playing one is marked with ▶.
fn draw_song_strip(f: &mut ratatui::Frame, area: Rect, app: &App, engine: &AudioEngine) {
    let (play_pat, _) = app.playing(engine);
    let playing_active = !engine.is_paused();
    let mut spans = vec![Span::styled(" song ", Style::default().fg(Color::DarkGray).bold())];
    for i in 0..app.song.patterns.len() {
        let rows = app.song.patterns[i].rows;
        let color = TRACK_COLORS[i % TRACK_COLORS.len()];
        let is_cur = i == app.current;
        let is_play = playing_active && i == play_pat;
        let mark = if is_play { "▶" } else { " " };
        let label = format!("{}{:02}·{}", mark, i, rows);
        let mut style = Style::default().fg(color);
        if is_cur {
            style = Style::default().fg(Color::Black).bg(color).add_modifier(Modifier::BOLD);
        } else if is_play {
            style = style.add_modifier(Modifier::BOLD);
        }
        spans.push(Span::raw(" "));
        spans.push(Span::styled(format!(" {} ", label), style));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_footer(f: &mut ratatui::Frame, area: Rect, _app: &App) {
    let keys = vec![
        ("a-g", "note"),
        ("1-9", "oct"),
        ("Del", "clear"),
        ("[ ]", "pattern"),
        ("n/N", "new/clone"),
        ("x", "del"),
        ("^←→", "reorder"),
        ("-/=", "len"),
        (",/.", "bpm"),
        ("m", "scope"),
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
    let (play_pat, play_row) = app.playing(engine);
    // Only show the playhead arrow when the viewed pattern is the one playing.
    let playhead_row = if play_pat == app.current && !engine.is_paused() {
        Some(play_row)
    } else {
        None
    };

    let p = app.cur();
    let tracks = &p.tracks;
    let row_count = area.height.saturating_sub(3) as u32;
    let visible_from = app.scroll.min(p.rows.saturating_sub(1));
    let visible_to = (visible_from + row_count).min(p.rows);

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
        let is_playhead = playhead_row == Some(r);
        let is_cursor_row = r == app.cursor_row;
        let is_beat = r % p.lines_per_beat == 0;

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
                style = Style::default().fg(Color::Black).bg(color).add_modifier(Modifier::BOLD);
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
    let total_frames = (buf.len() / 2).max(1);
    let head_frame = engine.playhead_frame() as usize % total_frames;

    let window: usize = 4096;
    let half = window / 2;
    let start = (head_frame + total_frames - half) % total_frames;

    let inner_w = area.width.saturating_sub(2) as usize;
    let bins = (inner_w.max(1) * 2).max(64);
    let frames_per_bin = (window / bins).max(1);

    let mut points: Vec<f32> = Vec::with_capacity(bins);
    for b in 0..bins {
        let f_idx = (start + b * frames_per_bin) % total_frames;
        let l = buf[f_idx * 2];
        let r = buf[f_idx * 2 + 1];
        points.push((l + r) * 0.5);
    }

    let canvas = Canvas::default()
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(Span::styled(
                    " scope ",
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                )),
        )
        .marker(Marker::Braille)
        .x_bounds([0.0, 1.0])
        .y_bounds([-1.0, 1.0])
        .paint(move |ctx| {
            ctx.draw(&CanvasLine {
                x1: 0.0, y1: 0.0, x2: 1.0, y2: 0.0,
                color: Color::Rgb(40, 40, 60),
            });
            for i in 0..points.len().saturating_sub(1) {
                let x1 = i as f64 / points.len() as f64;
                let x2 = (i + 1) as f64 / points.len() as f64;
                let y1 = points[i] as f64;
                let y2 = points[i + 1] as f64;
                let amp = y1.abs().max(y2.abs()) as f32;
                ctx.draw(&CanvasLine { x1, y1, x2, y2, color: amp_color(amp) });
            }
            ctx.draw(&CanvasLine {
                x1: 0.5, y1: -1.0, x2: 0.5, y2: 1.0,
                color: Color::Rgb(120, 100, 30),
            });
        });
    f.render_widget(canvas, area);
}

fn amp_color(a: f32) -> Color {
    let a = a.clamp(0.0, 1.0);
    if a < 0.15 {
        Color::Rgb(70, 110, 200)
    } else if a < 0.4 {
        Color::Rgb(120, 180, 240)
    } else if a < 0.7 {
        Color::Rgb(220, 130, 230)
    } else {
        Color::Rgb(255, 100, 160)
    }
}
