//! Embedded container terminal and complete, disk-backed Docker log history.
use crate::{backend, logs::History, model::AttachTarget};
use anyhow::{Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use ratatui::layout::Rect;
use std::{
    io::{self, Read, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};

struct Client(Box<dyn Child + Send + Sync>);
impl Drop for Client {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            // Only the local CLI is terminated. Attach disables signal proxying.
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Logs,
    Terminal,
}

#[derive(Default)]
pub struct Replies(Vec<u8>);
impl vt100::Callbacks for Replies {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        _: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        if self.0.len() > 4096 {
            return;
        }
        let first = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        let response = match (i1, c, first) {
            (None, 'n', 5) => "\x1b[0n".into(),
            (None, 'n', 6) => {
                let (r, c) = screen.cursor_position();
                format!("\x1b[{};{}R", r + 1, c + 1)
            }
            (None, 'c', 0) => "\x1b[?1;2c".into(),
            (Some(b'>'), 'c', 0) => "\x1b[>0;0;0c".into(),
            _ => String::new(),
        };
        self.0.extend_from_slice(response.as_bytes());
    }
}

struct Attached {
    client: Client,
    master: Box<dyn MasterPty + Send>,
    input: SyncSender<Vec<u8>>,
    input_error: Receiver<String>,
    output: Receiver<Vec<u8>>,
    output_only: bool,
    eof: bool,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Config {
    tty: bool,
    stdin_once: bool,
}

pub struct Session {
    pub target: AttachTarget,
    pub history: History,
    pub parser: vt100::Parser<Replies>,
    pub focus: Focus,
    pub show_terminal: bool,
    pub can_attach: bool,
    pub following: bool,
    pub top: usize,
    pub horizontal: u16,
    pub log_height: usize,
    pub log_rect: Rect,
    pub terminal_rect: Rect,
    pub terminal_status: String,
    pub log_status: String,
    pub search_edit: bool,
    pub search_query: String,
    pub search_status: String,
    pub error: Option<String>,
    pub demo: bool,
    logs: Option<Client>,
    log_result: Option<Receiver<Result<(), String>>>,
    pending: Option<Receiver<Result<String>>>,
    attached: Option<Attached>,
    search_result: Option<Receiver<Result<Option<usize>, String>>>,
    search_cancel: Arc<AtomicBool>,
    size: (u16, u16),
    pending_ctrl_p: bool,
    demo_tick: Instant,
    demo_input: Vec<u8>,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.search_cancel.store(true, Ordering::Relaxed);
    }
}

impl Session {
    pub fn open(target: AttachTarget, attach: bool, can_attach: bool, demo: bool) -> Result<Self> {
        let mut session = Self {
            target,
            history: History::new()?,
            parser: vt100::Parser::new_with_callbacks(8, 80, 10_000, Replies::default()),
            focus: if attach { Focus::Terminal } else { Focus::Logs },
            show_terminal: attach,
            can_attach,
            following: true,
            top: 0,
            horizontal: 0,
            log_height: 10,
            log_rect: Rect::default(),
            terminal_rect: Rect::default(),
            terminal_status: "Not attached".into(),
            log_status: "Loading all retained logs · following live output".into(),
            search_edit: false,
            search_query: String::new(),
            search_status: String::new(),
            error: None,
            demo,
            logs: None,
            log_result: None,
            pending: None,
            attached: None,
            search_result: None,
            search_cancel: Arc::new(AtomicBool::new(false)),
            size: (80, 8),
            pending_ctrl_p: false,
            demo_tick: Instant::now(),
            demo_input: Vec::new(),
        };
        if demo {
            for i in 0..850 {
                session.history.append(
                    format!(
                        "2026-10-21T12:{:02}:{:02}Z  {:04}  INFO  {}  {}\n",
                        (i / 60) % 60,
                        i % 60,
                        i + 1,
                        session.target.name,
                        if i == 0 {
                            "simulated logs · service starting"
                        } else if i % 13 == 0 {
                            "health check passed"
                        } else {
                            "request completed  status=200  duration=12ms"
                        }
                    )
                    .as_bytes(),
                )?;
            }
            session.log_status = "DEMO · simulated logs".into();
        } else {
            session.start_logs()?;
        }
        if attach {
            session.attach();
        }
        Ok(session)
    }

    fn start_logs(&mut self) -> Result<()> {
        // A shared PTY combines stdout and stderr in arrival order without a
        // line-count/output cap. Its reader writes directly to the history file.
        let pair = native_pty_system().openpty(pty_size(80, 24))?;
        let mut command = CommandBuilder::new(backend::docker_executable());
        command.args([
            "logs",
            "--follow",
            "--timestamps",
            "--tail",
            "all",
            "--",
            &self.target.id,
        ]);
        let client = Client(
            pair.slave
                .spawn_command(command)
                .context("Could not start Docker log stream")?,
        );
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader()?;
        let history = self.history.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let result = (|| -> Result<()> {
                let _master = pair.master;
                let mut bytes = [0; 16 * 1024];
                loop {
                    match reader.read(&mut bytes) {
                        Ok(0) => break,
                        Ok(n) => history.append(&bytes[..n])?,
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                        Err(e) if e.raw_os_error() == Some(libc::EIO) => break,
                        Err(e) => return Err(e.into()),
                    }
                }
                Ok(())
            })()
            .map_err(|e| format!("{e:#}"));
            let _ = tx.send(result);
        });
        self.logs = Some(client);
        self.log_result = Some(rx);
        Ok(())
    }

    pub fn attach(&mut self) {
        if !self.can_attach || self.pending.is_some() || self.attached.is_some() {
            return;
        }
        self.show_terminal = true;
        self.focus = Focus::Terminal;
        self.error = None;
        self.parser =
            vt100::Parser::new_with_callbacks(self.size.1, self.size.0, 10_000, Replies::default());
        if self.demo {
            self.terminal_status = "DEMO · simulated terminal".into();
            self.parser.process(b"\x1b[38;2;174;223;252mDemo container console\x1b[0m\r\nType a command to try the embedded terminal.\r\n$ ");
            return;
        }
        self.terminal_status = "Connecting to main process…".into();
        let id = self.target.id.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let result = backend::command(
                "docker",
                &[
                    "inspect".into(),
                    "--format".into(),
                    "{{json .Config}}".into(),
                    "--type".into(),
                    "container".into(),
                    "--".into(),
                    id,
                ],
                Duration::from_secs(12),
            );
            let _ = tx.send(result);
        });
        self.pending = Some(rx);
    }

    fn connect(&mut self, config: &str) -> Result<()> {
        let config: Config = serde_json::from_str(config.trim())
            .context("Unexpected container terminal configuration")?;
        let output_only = !config.tty && config.stdin_once;
        let pair = native_pty_system().openpty(pty_size(self.size.0, self.size.1))?;
        let mut command = CommandBuilder::new(backend::docker_executable());
        command.args(["attach", "--sig-proxy=false", "--detach-keys=ctrl-p,ctrl-q"]);
        if output_only {
            command.arg("--no-stdin");
        }
        command.args(["--", &self.target.id]);
        let client = Client(
            pair.slave
                .spawn_command(command)
                .context("Could not start docker attach")?,
        );
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader()?;
        let mut writer = pair.master.take_writer()?;
        let (input_tx, input_rx) = mpsc::sync_channel::<Vec<u8>>(32);
        let (error_tx, error_rx) = mpsc::sync_channel(1);
        thread::spawn(move || {
            for bytes in input_rx {
                if let Err(e) = writer.write_all(&bytes).and_then(|_| writer.flush()) {
                    let _ = error_tx.send(e.to_string());
                    break;
                }
            }
        });
        let (tx, rx) = mpsc::sync_channel(32);
        thread::spawn(move || {
            let mut buffer = [0; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.send(buffer[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break, // PTYs return EIO on slave close.
                }
            }
        });
        self.terminal_status = if output_only {
            "Output-only session · preserving container stdin"
        } else {
            "Attached · main process"
        }
        .into();
        self.attached = Some(Attached {
            client,
            master: pair.master,
            input: input_tx,
            input_error: error_rx,
            output: rx,
            output_only,
            eof: false,
        });
        Ok(())
    }

    pub fn tick(&mut self) {
        if let Some(rx) = &self.pending {
            let ready = match rx.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(anyhow::anyhow!(
                    "Container terminal check did not complete"
                ))),
                Err(mpsc::TryRecvError::Empty) => None,
            };
            if let Some(result) = ready {
                self.pending = None;
                if let Err(e) = result.and_then(|config| self.connect(&config)) {
                    self.terminal_status = "Attach failed".into();
                    self.error = Some(format!("{e:#}"));
                }
            }
        }
        let mut ended = None;
        if let Some(attach) = &mut self.attached {
            for _ in 0..32 {
                match attach.output.try_recv() {
                    Ok(bytes) => self.parser.process(&bytes),
                    Err(mpsc::TryRecvError::Disconnected) => {
                        attach.eof = true;
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                }
            }
            let replies = std::mem::take(&mut self.parser.callbacks_mut().0);
            if !replies.is_empty()
                && !attach.output_only
                && let Err(e) = attach.input.try_send(replies)
            {
                self.error = Some(format!("Container input is unavailable · {e}"));
            }
            if let Ok(e) = attach.input_error.try_recv() {
                self.error = Some(format!("Container input failed · {e}"));
            }
            match attach.client.0.try_wait() {
                Ok(Some(status)) if attach.eof => ended = Some(status.exit_code()),
                Err(e) => self.error = Some(e.to_string()),
                _ => {}
            }
        }
        if let Some(code) = ended {
            self.attached = None;
            self.terminal_status = if code == 0 {
                "Attach session ended".into()
            } else {
                format!("Attach exited with status {code}")
            };
            // Keep the final screen and logs available for inspection.
        }
        if let Some(rx) = &self.log_result {
            match rx.try_recv() {
                Ok(Err(e)) => {
                    self.error = Some(e);
                    self.log_status = "Log capture failed".into();
                    self.logs = None;
                    self.log_result = None;
                }
                Ok(Ok(())) => {
                    if !self.log_status.starts_with("Docker logs exited") {
                        self.log_status = "Log stream ended · history retained".into();
                    }
                    self.log_result = None;
                }
                _ => {}
            }
        }
        if let Some(client) = &mut self.logs
            && let Ok(Some(status)) = client.0.try_wait()
        {
            self.log_status = if status.success() {
                "Log stream ended · history retained".into()
            } else {
                format!(
                    "Docker logs exited with status {} · see output above",
                    status.exit_code()
                )
            };
            self.logs = None;
        }
        if let Some(rx) = &self.search_result
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok(Some(line)) => {
                    self.following = false;
                    self.top = line;
                    self.search_status = format!("Match on line {}", line + 1);
                }
                Ok(None) => self.search_status = "No match in captured history".into(),
                Err(e) => self.error = Some(e),
            }
            self.search_result = None;
        }
        if self.demo && self.demo_tick.elapsed() >= Duration::from_secs(1) {
            if let Err(e) = self.history.append(
                format!(
                    "2026-10-21T12:15:00Z  INFO  {}  live request completed  status=200\n",
                    self.target.name
                )
                .as_bytes(),
            ) {
                self.error = Some(e.to_string());
            }
            self.demo_tick = Instant::now();
        }
        if self.logs.is_some()
            && self.history.stats().bytes > 0
            && self.log_status.starts_with("Loading")
        {
            self.log_status = "Retained history + live output".into();
        }
        self.clamp_logs();
    }

    pub fn is_connected(&self) -> bool {
        self.pending.is_some() || self.attached.is_some() || (self.demo && self.show_terminal)
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        let size = (cols.max(1), rows.max(1));
        if size == self.size {
            return;
        }
        self.size = size;
        self.parser.screen_mut().set_size(size.1, size.0);
        if let Some(attach) = &self.attached
            && let Err(e) = attach.master.resize(pty_size(size.0, size.1))
        {
            self.error = Some(e.to_string());
        }
    }

    pub fn clamp_logs(&mut self) {
        let last = self.history.stats().lines.saturating_sub(self.log_height);
        self.top = if self.following {
            last
        } else {
            self.top.min(last)
        };
    }
    pub fn scroll_logs(&mut self, delta: isize) {
        self.following = false;
        self.top = self.top.saturating_add_signed(delta);
        self.clamp_logs();
    }
    pub fn follow(&mut self) {
        self.search_status.clear();
        self.following = true;
        self.horizontal = 0;
        self.clamp_logs();
    }
    pub fn toggle_focus(&mut self) {
        self.focus = if self.focus == Focus::Logs && self.show_terminal {
            Focus::Terminal
        } else {
            Focus::Logs
        };
    }
    pub fn search(&mut self, next: bool) {
        if self.search_query.is_empty() {
            return;
        }
        self.search_cancel.store(true, Ordering::Relaxed);
        self.search_cancel = Arc::new(AtomicBool::new(false));
        let cancelled = self.search_cancel.clone();
        let history = self.history.clone();
        let query = self.search_query.clone();
        let from = self.top.saturating_add(usize::from(next));
        let (tx, rx) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let _ = tx.send(
                history
                    .find(&query, from, &cancelled)
                    .map_err(|e| format!("{e:#}")),
            );
        });
        self.search_result = Some(rx);
        self.search_status = "Searching captured history…".into();
    }
    pub fn paste(&mut self, text: &str) {
        if self.search_edit {
            self.search_query
                .extend(text.chars().filter(|c| !c.is_control()));
            return;
        }
        if self.focus != Focus::Terminal {
            return;
        }
        // Pasted detach/control bytes are never sent to the main process.
        let text: String = text
            .chars()
            .filter(|c| !matches!(c, '\x03' | '\x10' | '\x11'))
            .collect();
        let bytes = if self.parser.screen().bracketed_paste() {
            format!("\x1b[200~{text}\x1b[201~").into_bytes()
        } else {
            text.into_bytes()
        };
        self.write_input(&bytes);
    }
    fn write_input(&mut self, bytes: &[u8]) {
        self.parser.screen_mut().set_scrollback(0);
        if self.demo {
            for &byte in bytes {
                if byte == b'\r' || byte == b'\n' {
                    let reply = format!(
                        "\r\n[DEMO] {}\r\n$ ",
                        String::from_utf8_lossy(&self.demo_input)
                    );
                    self.parser.process(reply.as_bytes());
                    self.demo_input.clear();
                } else if byte == 0x7f {
                    while self.demo_input.last().is_some_and(|b| b & 0xc0 == 0x80) {
                        self.demo_input.pop();
                    }
                    self.demo_input.pop();
                    self.parser.process(b"\x08 \x08");
                } else {
                    self.demo_input.push(byte);
                    self.parser.process(&[byte]);
                }
            }
        } else if let Some(attach) = &mut self.attached
            && !attach.output_only
            && !bytes.is_empty()
            && let Err(e) = attach.input.try_send(bytes.to_vec())
        {
            self.error = Some(format!(
                "Container input is busy or closed; this input was not sent · {e}"
            ));
        }
    }

    /// Returns true when the entire workspace should close. These UI keys never
    /// leak through to the process, while ordinary input stays terminal input.
    pub fn key(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && matches!(key.code, KeyCode::Char('q' | 'c')) {
            return true;
        }
        if self.search_edit {
            match key.code {
                KeyCode::Esc => self.search_edit = false,
                KeyCode::Enter => {
                    self.search_edit = false;
                    self.search(false);
                }
                KeyCode::Backspace => {
                    self.search_query.pop();
                }
                KeyCode::Char('u') if ctrl => self.search_query.clear(),
                KeyCode::Char(c) if !ctrl => self.search_query.push(c),
                _ => {}
            }
            return false;
        }
        if (ctrl && key.code == KeyCode::Char('w')) || key.code == KeyCode::BackTab {
            self.toggle_focus();
            return false;
        }
        match key.code {
            KeyCode::F(2) => {
                self.follow();
                return false;
            }
            KeyCode::PageUp => {
                self.focus = Focus::Logs;
                self.scroll_logs(-(self.log_height as isize).max(1));
                return false;
            }
            KeyCode::PageDown => {
                self.focus = Focus::Logs;
                self.scroll_logs(self.log_height as isize);
                return false;
            }
            _ => {}
        }
        if self.focus == Focus::Logs {
            match key.code {
                KeyCode::Esc => return true,
                KeyCode::Up | KeyCode::Char('k') => self.scroll_logs(-1),
                KeyCode::Down | KeyCode::Char('j') => self.scroll_logs(1),
                KeyCode::Home | KeyCode::Char('g') => {
                    self.following = false;
                    self.top = 0;
                }
                KeyCode::End | KeyCode::Char('G' | 'f') => self.follow(),
                KeyCode::Left | KeyCode::Char('h') => {
                    self.horizontal = self.horizontal.saturating_sub(8)
                }
                KeyCode::Right | KeyCode::Char('l') => {
                    self.horizontal = self.horizontal.saturating_add(8)
                }
                KeyCode::Char('/') => {
                    self.search_edit = true;
                    self.search_status.clear();
                }
                KeyCode::Char('n') | KeyCode::F(3) => self.search(true),
                KeyCode::Char('a') => self.attach(),
                _ => {}
            }
        } else {
            if ctrl && key.code == KeyCode::Char('p') {
                self.pending_ctrl_p = true;
                return false;
            }
            if self.pending_ctrl_p {
                self.write_input(&[0x10]);
                self.pending_ctrl_p = false;
            }
            let bytes = encode_key(key, self.parser.screen().application_cursor());
            self.write_input(&bytes);
        }
        false
    }
}

fn pty_size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn encode_key(key: KeyEvent, application: bool) -> Vec<u8> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let modifier = 1 + u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(ctrl);
    let arrow = |c| {
        if modifier > 1 {
            format!("\x1b[1;{modifier}{c}")
        } else {
            format!("\x1b{}{c}", if application { 'O' } else { '[' })
        }
    };
    let bytes = match key.code {
        KeyCode::Char(c) if ctrl => match c {
            ' ' | '@' => vec![0],
            '?' => vec![127],
            c if c.is_ascii() && ('@'..='_').contains(&c.to_ascii_uppercase()) => {
                vec![(c.to_ascii_uppercase() as u8) & 0x1f]
            }
            _ => Vec::new(),
        },
        KeyCode::Char(c) => c.to_string().into_bytes(),
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::Backspace => vec![127],
        KeyCode::Esc => vec![27],
        KeyCode::Up => return arrow('A').into_bytes(),
        KeyCode::Down => return arrow('B').into_bytes(),
        KeyCode::Right => return arrow('C').into_bytes(),
        KeyCode::Left => return arrow('D').into_bytes(),
        KeyCode::Home => return arrow('H').into_bytes(),
        KeyCode::End => return arrow('F').into_bytes(),
        KeyCode::Insert => b"\x1b[2~".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::F(n @ 1..=4) => vec![27, b'O', b'P' + n - 1],
        KeyCode::F(n @ 5..=12) => format!(
            "\x1b[{}~",
            [15, 17, 18, 19, 20, 21, 23, 24][usize::from(n - 5)]
        )
        .into_bytes(),
        _ => Vec::new(),
    };
    if alt {
        [vec![27], bytes].concat()
    } else {
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn embedded_keys_keep_unicode_tab_arrows_and_terminal_modes() {
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        assert_eq!(encode_key(key(KeyCode::Char('é')), false), "é".as_bytes());
        assert_eq!(encode_key(key(KeyCode::Tab), false), b"\t");
        assert_eq!(encode_key(key(KeyCode::Up), false), b"\x1b[A");
        assert_eq!(encode_key(key(KeyCode::Up), true), b"\x1bOA");
        assert_eq!(
            encode_key(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL), false),
            b"\x1b[1;5D"
        );
    }
    #[test]
    fn emulator_contains_ansi_cursor_queries_and_external_terminal_effects() {
        let mut parser = vt100::Parser::new_with_callbacks(5, 30, 100, Replies::default());
        parser.process(b"\x1b[31mred\x1b[0m\x1b[2;4Hok\x1b[6n\x1b]52;c;AAAA\x07");
        assert_eq!(
            parser.screen().cell(0, 0).unwrap().fgcolor(),
            vt100::Color::Idx(1)
        );
        assert_eq!(parser.screen().cell(1, 3).unwrap().contents(), "o");
        assert_eq!(parser.callbacks().0, b"\x1b[2;6R");
    }
}
