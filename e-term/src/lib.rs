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

/// One screen cell: a character and an optional foreground colour.
#[derive(Clone, Copy)]
pub struct Cell {
    pub ch: char,
    pub fg: Option<(u8, u8, u8)>,
}

impl Cell {
    const BLANK: Cell = Cell { ch: ' ', fg: None };
}

/// A foreground-coloured run of text within a line.
pub type Run = (String, Option<(u8, u8, u8)>);

/// A character grid with a cursor.
pub struct Screen {
    pub rows: usize,
    pub cols: usize,
    grid: Vec<Vec<Cell>>,
    cx: usize,
    cy: usize,
    fg: Option<(u8, u8, u8)>,
}

impl Screen {
    fn new(rows: usize, cols: usize) -> Self {
        Self {
            rows,
            cols,
            grid: vec![vec![Cell::BLANK; cols]; rows],
            cx: 0,
            cy: 0,
            fg: None,
        }
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
    }

    /// Snapshot each line as foreground-coloured runs (trailing blanks dropped).
    pub fn runs(&self) -> Vec<Vec<Run>> {
        self.grid
            .iter()
            .map(|row| {
                let last = row
                    .iter()
                    .rposition(|c| c.ch != ' ')
                    .map(|i| i + 1)
                    .unwrap_or(0);
                let mut runs: Vec<Run> = Vec::new();
                let mut cur = String::new();
                let mut cur_fg = None;
                for (i, cell) in row[..last].iter().enumerate() {
                    if i == 0 {
                        cur_fg = cell.fg;
                    } else if cell.fg != cur_fg {
                        runs.push((std::mem::take(&mut cur), cur_fg));
                        cur_fg = cell.fg;
                    }
                    cur.push(cell.ch);
                }
                if !cur.is_empty() {
                    runs.push((cur, cur_fg));
                }
                runs
            })
            .collect()
    }

    fn newline(&mut self) {
        if self.cy + 1 >= self.rows {
            self.grid.remove(0);
            self.grid.push(vec![Cell::BLANK; self.cols]);
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
        if let Some(row) = self.grid.get_mut(self.cy) {
            if let Some(cell) = row.get_mut(self.cx) {
                *cell = Cell { ch: c, fg };
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
            1 => row.iter_mut().take(self.cx + 1).for_each(|c| *c = Cell::BLANK),
            _ => row.iter_mut().for_each(|c| *c = Cell::BLANK),
        }
    }

    fn sgr(&mut self, params: &Params) {
        let codes: Vec<u16> = params.iter().map(|p| p.first().copied().unwrap_or(0)).collect();
        if codes.is_empty() {
            self.fg = None;
            return;
        }
        let mut i = 0;
        while i < codes.len() {
            match codes[i] {
                0 => self.fg = None,
                39 => self.fg = None,
                30..=37 => self.fg = Some(ansi16(codes[i] - 30)),
                90..=97 => self.fg = Some(ansi16_bright(codes[i] - 90)),
                38 => match codes.get(i + 1) {
                    Some(5) => {
                        if let Some(&n) = codes.get(i + 2) {
                            self.fg = Some(xterm256(n as u8));
                        }
                        i += 2;
                    }
                    Some(2) => {
                        if let (Some(&r), Some(&g), Some(&b)) =
                            (codes.get(i + 2), codes.get(i + 3), codes.get(i + 4))
                        {
                            self.fg = Some((r as u8, g as u8, b as u8));
                        }
                        i += 4;
                    }
                    _ => {}
                },
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
                let row = it.next().and_then(|p| p.first().copied()).unwrap_or(1).max(1) as usize;
                let col = it.next().and_then(|p| p.first().copied()).unwrap_or(1).max(1) as usize;
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
        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize {
                rows: rows as u16,
                cols: cols as u16,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("openpty")?;

        let mut cmd = CommandBuilder::new(shell);
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        let child = pair.slave.spawn_command(cmd).context("spawn shell")?;

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
