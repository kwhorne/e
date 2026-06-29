//! A small PTY-backed terminal with a minimal VT100 screen model.
//!
//! Not a full terminal emulator — it handles printing, the common control
//! characters, cursor movement and erase sequences, which covers shells and
//! ordinary command output. Complex full-screen TUIs are out of scope.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{Context, Result};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use vte::{Params, Parser, Perform};

/// One screen cell: a character with optional foreground / background colours.
#[derive(Clone, Copy, PartialEq)]
pub struct Cell {
    pub ch: char,
    pub fg: Option<(u8, u8, u8)>,
    pub bg: Option<(u8, u8, u8)>,
}

impl Cell {
    const BLANK: Cell = Cell {
        ch: ' ',
        fg: None,
        bg: None,
    };
}

/// A coloured run of text within a line: `(text, fg, bg)`.
pub type Run = (String, Option<(u8, u8, u8)>, Option<(u8, u8, u8)>);

/// Maximum number of scrolled-off lines kept for scrollback.
const SCROLLBACK_MAX: usize = 5000;

/// Group a row of cells into coloured runs by `(fg, bg)`. Trailing blank cells
/// are dropped unless they carry a background colour (so highlighted regions,
/// e.g. selected/diff lines, still render).
fn line_runs(row: &[Cell]) -> Vec<Run> {
    let last = row
        .iter()
        .rposition(|c| c.ch != ' ' || c.bg.is_some())
        .map(|i| i + 1)
        .unwrap_or(0);
    let mut runs: Vec<Run> = Vec::new();
    let mut cur = String::new();
    let mut cur_fg = None;
    let mut cur_bg = None;
    for (i, cell) in row[..last].iter().enumerate() {
        if i == 0 {
            cur_fg = cell.fg;
            cur_bg = cell.bg;
        } else if cell.fg != cur_fg || cell.bg != cur_bg {
            runs.push((std::mem::take(&mut cur), cur_fg, cur_bg));
            cur_fg = cell.fg;
            cur_bg = cell.bg;
        }
        cur.push(cell.ch);
    }
    if !cur.is_empty() {
        runs.push((cur, cur_fg, cur_bg));
    }
    runs
}

/// A character grid with a cursor.
pub struct Screen {
    pub rows: usize,
    pub cols: usize,
    grid: Vec<Vec<Cell>>,
    /// Lines that scrolled off the top, oldest first (capped).
    scrollback: std::collections::VecDeque<Vec<Cell>>,
    /// How many lines the view is scrolled up from the live bottom (0 = bottom).
    scroll: usize,
    cx: usize,
    cy: usize,
    fg: Option<(u8, u8, u8)>,
    bg: Option<(u8, u8, u8)>,
}

impl Screen {
    fn new(rows: usize, cols: usize) -> Self {
        Self {
            rows,
            cols,
            grid: vec![vec![Cell::BLANK; cols]; rows],
            scrollback: std::collections::VecDeque::new(),
            scroll: 0,
            cx: 0,
            cy: 0,
            fg: None,
            bg: None,
        }
    }

    /// Scroll the view up (into history) by `n` lines.
    pub fn scroll_up(&mut self, n: usize) {
        self.scroll = (self.scroll + n).min(self.scrollback.len());
    }

    /// Scroll the view down (towards the live bottom) by `n` lines.
    pub fn scroll_down(&mut self, n: usize) {
        self.scroll = self.scroll.saturating_sub(n);
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll = 0;
    }

    pub fn resize(&mut self, rows: usize, cols: usize) {
        self.grid.resize(rows, vec![Cell::BLANK; cols]);
        for row in &mut self.grid {
            row.resize(cols, Cell::BLANK);
        }
        self.rows = rows;
        self.cols = cols;
        self.cy = self.cy.min(rows.saturating_sub(1));
        self.cx = self.cx.min(cols.saturating_sub(1));
        self.scroll = self.scroll.min(self.scrollback.len());
    }

    /// The `rows` visible lines (respecting the scroll position), each as a
    /// list of coloured runs. Trailing blank cells are dropped unless they
    /// carry a background colour.
    pub fn runs(&self) -> Vec<Vec<Run>> {
        let s = self.scrollback.len();
        // Window of `rows` lines ending `scroll` lines above the live bottom.
        let start = (s + self.rows).saturating_sub(self.rows + self.scroll);
        (0..self.rows)
            .map(|vr| {
                let idx = start + vr;
                let row = if idx < s {
                    &self.scrollback[idx]
                } else {
                    &self.grid[idx - s]
                };
                line_runs(row)
            })
            .collect()
    }

    /// Cursor position in *visible* coordinates, or `None` when scrolled away.
    pub fn visible_cursor(&self) -> Option<(usize, usize)> {
        let row = self.cy + self.scroll;
        if row < self.rows {
            Some((row, self.cx))
        } else {
            None
        }
    }

    fn newline(&mut self) {
        if self.cy + 1 >= self.rows {
            // The top line scrolls off into the scrollback buffer.
            let line = self.grid.remove(0);
            self.scrollback.push_back(line);
            while self.scrollback.len() > SCROLLBACK_MAX {
                self.scrollback.pop_front();
            }
            self.grid.push(vec![Cell::BLANK; self.cols]);
            // If the user is scrolled up, keep the view anchored on the same
            // lines as new output arrives below.
            if self.scroll > 0 {
                self.scroll = (self.scroll + 1).min(self.scrollback.len());
            }
        } else {
            self.cy += 1;
        }
    }

    fn put(&mut self, c: char) {
        if self.cx >= self.cols {
            self.cx = 0;
            self.newline();
        }
        let fg = self.fg;
        let bg = self.bg;
        if let Some(row) = self.grid.get_mut(self.cy) {
            if let Some(cell) = row.get_mut(self.cx) {
                *cell = Cell { ch: c, fg, bg };
            }
        }
        self.cx += 1;
    }

    fn erase_in_display(&mut self, mode: u16) {
        match mode {
            0 => {
                if let Some(row) = self.grid.get_mut(self.cy) {
                    for c in row.iter_mut().skip(self.cx) {
                        *c = Cell::BLANK;
                    }
                }
                for row in self.grid.iter_mut().skip(self.cy + 1) {
                    row.iter_mut().for_each(|c| *c = Cell::BLANK);
                }
            }
            1 => {
                for row in self.grid.iter_mut().take(self.cy) {
                    row.iter_mut().for_each(|c| *c = Cell::BLANK);
                }
                if let Some(row) = self.grid.get_mut(self.cy) {
                    for c in row.iter_mut().take(self.cx + 1) {
                        *c = Cell::BLANK;
                    }
                }
            }
            _ => {
                for row in &mut self.grid {
                    row.iter_mut().for_each(|c| *c = Cell::BLANK);
                }
                self.cx = 0;
                self.cy = 0;
            }
        }
    }

    fn erase_in_line(&mut self, mode: u16) {
        let Some(row) = self.grid.get_mut(self.cy) else {
            return;
        };
        match mode {
            0 => row.iter_mut().skip(self.cx).for_each(|c| *c = Cell::BLANK),
            1 => row
                .iter_mut()
                .take(self.cx + 1)
                .for_each(|c| *c = Cell::BLANK),
            _ => row.iter_mut().for_each(|c| *c = Cell::BLANK),
        }
    }

    fn sgr(&mut self, params: &Params) {
        let codes: Vec<u16> = params
            .iter()
            .map(|p| p.first().copied().unwrap_or(0))
            .collect();
        if codes.is_empty() {
            self.fg = None;
            self.bg = None;
            return;
        }
        let mut i = 0;
        while i < codes.len() {
            match codes[i] {
                0 => {
                    self.fg = None;
                    self.bg = None;
                }
                39 => self.fg = None,
                49 => self.bg = None,
                30..=37 => self.fg = Some(ansi16(codes[i] - 30)),
                90..=97 => self.fg = Some(ansi16_bright(codes[i] - 90)),
                40..=47 => self.bg = Some(ansi16(codes[i] - 40)),
                100..=107 => self.bg = Some(ansi16_bright(codes[i] - 100)),
                38 | 48 => {
                    let is_fg = codes[i] == 38;
                    match codes.get(i + 1) {
                        Some(5) => {
                            if let Some(&n) = codes.get(i + 2) {
                                let c = xterm256(n as u8);
                                if is_fg {
                                    self.fg = Some(c);
                                } else {
                                    self.bg = Some(c);
                                }
                            }
                            i += 2;
                        }
                        Some(2) => {
                            if let (Some(&r), Some(&g), Some(&b)) =
                                (codes.get(i + 2), codes.get(i + 3), codes.get(i + 4))
                            {
                                let c = (r as u8, g as u8, b as u8);
                                if is_fg {
                                    self.fg = Some(c);
                                } else {
                                    self.bg = Some(c);
                                }
                            }
                            i += 4;
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }
}

impl Perform for Screen {
    fn print(&mut self, c: char) {
        self.put(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => self.newline(),
            b'\r' => self.cx = 0,
            0x08 => self.cx = self.cx.saturating_sub(1),
            b'\t' => self.cx = ((self.cx / 8) + 1) * 8,
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, _inter: &[u8], _ignore: bool, action: char) {
        let first = params.iter().next().and_then(|p| p.first().copied());
        let n = first.unwrap_or(0).max(1) as usize;
        match action {
            'm' => self.sgr(params),
            'A' => self.cy = self.cy.saturating_sub(n),
            'B' => self.cy = (self.cy + n).min(self.rows.saturating_sub(1)),
            'C' => self.cx = (self.cx + n).min(self.cols.saturating_sub(1)),
            'D' => self.cx = self.cx.saturating_sub(n),
            'G' => self.cx = n.saturating_sub(1).min(self.cols.saturating_sub(1)),
            'd' => self.cy = n.saturating_sub(1).min(self.rows.saturating_sub(1)),
            'H' | 'f' => {
                let mut it = params.iter();
                let row = it
                    .next()
                    .and_then(|p| p.first().copied())
                    .unwrap_or(1)
                    .max(1) as usize;
                let col = it
                    .next()
                    .and_then(|p| p.first().copied())
                    .unwrap_or(1)
                    .max(1) as usize;
                self.cy = (row - 1).min(self.rows.saturating_sub(1));
                self.cx = (col - 1).min(self.cols.saturating_sub(1));
            }
            'J' => self.erase_in_display(first.unwrap_or(0)),
            'K' => self.erase_in_line(first.unwrap_or(0)),
            _ => {}
        }
    }
}

/// A live terminal session.
pub struct Terminal {
    _master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    _child: Box<dyn Child + Send + Sync>,
    screen: Arc<Mutex<Screen>>,
}

impl Terminal {
    /// Spawn `shell` in `cwd`. `on_update` is called (on the reader thread)
    /// whenever output is received, so the UI can repaint.
    pub fn spawn(
        shell: &str,
        cwd: &Path,
        rows: usize,
        cols: usize,
        on_update: Box<dyn Fn() + Send>,
    ) -> Result<Self> {
        let mut cmd = CommandBuilder::new(shell);
        cmd.cwd(cwd);
        Self::spawn_builder(cmd, rows, cols, on_update)
    }

    /// Run a command line through the user's **interactive login** shell
    /// (`$SHELL -ilc "..."`), so PATH and the usual environment are available.
    /// The interactive flag is important: tools installed via nvm/rbenv/etc. are
    /// commonly added to PATH in `.zshrc`/`.bashrc`, which non-interactive
    /// shells skip — that's why a GUI-launched app sees "command not found".
    /// Used to launch CLI agents (Elyra, Claude Code, Codex …).
    pub fn spawn_command(
        cmdline: &str,
        cwd: &Path,
        rows: usize,
        cols: usize,
        on_update: Box<dyn Fn() + Send>,
    ) -> Result<Self> {
        let mut cmd = CommandBuilder::new(default_shell());
        cmd.arg("-ilc");
        cmd.arg(cmdline);
        cmd.cwd(cwd);
        Self::spawn_builder(cmd, rows, cols, on_update)
    }

    fn spawn_builder(
        mut cmd: CommandBuilder,
        rows: usize,
        cols: usize,
        on_update: Box<dyn Fn() + Send>,
    ) -> Result<Self> {
        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize {
                rows: rows as u16,
                cols: cols as u16,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("openpty")?;

        cmd.env("TERM", "xterm-256color");
        let child = pair.slave.spawn_command(cmd).context("spawn command")?;

        let mut reader = pair.master.try_clone_reader().context("clone reader")?;
        let writer = pair.master.take_writer().context("take writer")?;
        let screen = Arc::new(Mutex::new(Screen::new(rows, cols)));

        {
            let screen = screen.clone();
            thread::spawn(move || {
                let mut parser = Parser::new();
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if let Ok(mut s) = screen.lock() {
                                parser.advance(&mut *s, &buf[..n]);
                            }
                            on_update();
                        }
                    }
                }
            });
        }

        Ok(Self {
            _master: pair.master,
            writer,
            _child: child,
            screen,
        })
    }

    /// Send bytes to the shell (keyboard input).
    pub fn write(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    /// Current screen as per-line coloured runs.
    pub fn snapshot_runs(&self) -> Vec<Vec<Run>> {
        self.screen.lock().map(|s| s.runs()).unwrap_or_default()
    }

    /// Current cursor position `(row, col)` in visible coordinates, or `None`
    /// when the view is scrolled away from the cursor.
    pub fn cursor(&self) -> Option<(usize, usize)> {
        self.screen.lock().ok().and_then(|s| s.visible_cursor())
    }

    /// Scroll the view into / out of the scrollback history.
    pub fn scroll_up(&self, n: usize) {
        if let Ok(mut s) = self.screen.lock() {
            s.scroll_up(n);
        }
    }
    pub fn scroll_down(&self, n: usize) {
        if let Ok(mut s) = self.screen.lock() {
            s.scroll_down(n);
        }
    }
    pub fn scroll_to_bottom(&self) {
        if let Ok(mut s) = self.screen.lock() {
            s.scroll_to_bottom();
        }
    }

    pub fn resize(&self, rows: usize, cols: usize) {
        let _ = self._master.resize(PtySize {
            rows: rows as u16,
            cols: cols as u16,
            pixel_width: 0,
            pixel_height: 0,
        });
        if let Ok(mut s) = self.screen.lock() {
            s.resize(rows, cols);
        }
    }
}

/// The user's preferred shell.
pub fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string())
}

/// The 8 standard ANSI colours (index 0..=7).
fn ansi16(i: u16) -> (u8, u8, u8) {
    match i {
        0 => (0x2b, 0x30, 0x39), // black (slightly lifted for visibility)
        1 => (0xe0, 0x6c, 0x75), // red
        2 => (0x98, 0xc3, 0x79), // green
        3 => (0xe5, 0xc0, 0x7b), // yellow
        4 => (0x61, 0xaf, 0xef), // blue
        5 => (0xc6, 0x78, 0xdd), // magenta
        6 => (0x56, 0xb6, 0xc2), // cyan
        _ => (0xab, 0xb2, 0xbf), // white
    }
}

/// The 8 bright ANSI colours (index 0..=7).
fn ansi16_bright(i: u16) -> (u8, u8, u8) {
    match i {
        0 => (0x5c, 0x63, 0x70),
        1 => (0xff, 0x8b, 0x94),
        2 => (0xb5, 0xe8, 0x90),
        3 => (0xff, 0xe2, 0x9a),
        4 => (0x8a, 0xc6, 0xff),
        5 => (0xe0, 0x9a, 0xf5),
        6 => (0x7e, 0xd6, 0xe0),
        _ => (0xff, 0xff, 0xff),
    }
}

/// xterm 256-colour palette.
fn xterm256(n: u8) -> (u8, u8, u8) {
    match n {
        0..=7 => ansi16(n as u16),
        8..=15 => ansi16_bright((n - 8) as u16),
        16..=231 => {
            let n = n - 16;
            let r = n / 36;
            let g = (n % 36) / 6;
            let b = n % 6;
            let conv = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            (conv(r), conv(g), conv(b))
        }
        _ => {
            let v = 8 + (n - 232) * 10;
            (v, v, v)
        }
    }
}

#[cfg(test)]
mod scroll_tests {
    use super::*;
    use vte::Parser;

    fn feed(s: &mut Screen, p: &mut Parser, bytes: &[u8]) {
        p.advance(s, bytes);
    }

    #[test]
    fn scrollback_and_bg() {
        let mut s = Screen::new(3, 10);
        let mut p = Parser::new();
        // 6 lines -> 3 scroll off into scrollback.
        for i in 0..6 {
            feed(&mut s, &mut p, format!("line{i}\r\n").as_bytes());
        }
        assert!(s.scrollback.len() >= 3, "scrollback={}", s.scrollback.len());
        // Scroll up reveals older lines.
        s.scroll_up(2);
        let runs = s.runs();
        assert_eq!(runs.len(), 3);
        // Background colour parsing (red bg = SGR 41).
        let mut s2 = Screen::new(2, 10);
        let mut p2 = Parser::new();
        feed(&mut s2, &mut p2, b"\x1b[41mX\x1b[0m");
        let r = s2.runs();
        assert!(
            r[0].iter().any(|(_, _, bg)| bg.is_some()),
            "no bg run: {:?}",
            r[0]
        );
    }
}
