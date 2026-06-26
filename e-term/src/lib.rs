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

/// A character grid with a cursor.
pub struct Screen {
    pub rows: usize,
    pub cols: usize,
    grid: Vec<Vec<char>>,
    cx: usize,
    cy: usize,
}

impl Screen {
    fn new(rows: usize, cols: usize) -> Self {
        Self {
            rows,
            cols,
            grid: vec![vec![' '; cols]; rows],
            cx: 0,
            cy: 0,
        }
    }

    pub fn resize(&mut self, rows: usize, cols: usize) {
        self.grid.resize(rows, vec![' '; cols]);
        for row in &mut self.grid {
            row.resize(cols, ' ');
        }
        self.rows = rows;
        self.cols = cols;
        self.cy = self.cy.min(rows.saturating_sub(1));
        self.cx = self.cx.min(cols.saturating_sub(1));
    }

    /// Snapshot the visible grid as trimmed text lines.
    pub fn lines(&self) -> Vec<String> {
        self.grid
            .iter()
            .map(|row| {
                let s: String = row.iter().collect();
                s.trim_end().to_string()
            })
            .collect()
    }

    fn newline(&mut self) {
        if self.cy + 1 >= self.rows {
            self.grid.remove(0);
            self.grid.push(vec![' '; self.cols]);
        } else {
            self.cy += 1;
        }
    }

    fn put(&mut self, c: char) {
        if self.cx >= self.cols {
            self.cx = 0;
            self.newline();
        }
        if let Some(row) = self.grid.get_mut(self.cy) {
            if let Some(cell) = row.get_mut(self.cx) {
                *cell = c;
            }
        }
        self.cx += 1;
    }

    fn erase_in_display(&mut self, mode: u16) {
        match mode {
            0 => {
                // Cursor to end of screen.
                if let Some(row) = self.grid.get_mut(self.cy) {
                    for c in row.iter_mut().skip(self.cx) {
                        *c = ' ';
                    }
                }
                for row in self.grid.iter_mut().skip(self.cy + 1) {
                    row.iter_mut().for_each(|c| *c = ' ');
                }
            }
            1 => {
                for row in self.grid.iter_mut().take(self.cy) {
                    row.iter_mut().for_each(|c| *c = ' ');
                }
                if let Some(row) = self.grid.get_mut(self.cy) {
                    for c in row.iter_mut().take(self.cx + 1) {
                        *c = ' ';
                    }
                }
            }
            _ => {
                for row in &mut self.grid {
                    row.iter_mut().for_each(|c| *c = ' ');
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
            0 => row.iter_mut().skip(self.cx).for_each(|c| *c = ' '),
            1 => row.iter_mut().take(self.cx + 1).for_each(|c| *c = ' '),
            _ => row.iter_mut().for_each(|c| *c = ' '),
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

    /// Current screen as text lines.
    pub fn snapshot(&self) -> Vec<String> {
        self.screen
            .lock()
            .map(|s| s.lines())
            .unwrap_or_default()
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
