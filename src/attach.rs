use crate::{backend, model::AttachTarget};
use anyhow::{Context, Result};
use portable_pty::{Child, CommandBuilder, PtySize, native_pty_system};
use std::{
    io::{self, Read, Write},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

pub enum Outcome {
    Detached,
    Exited { code: u32, output: String },
}

struct Client(Box<dyn Child + Send + Sync>);
impl Drop for Client {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            // Terminate only the local attach client. Signal proxying is disabled,
            // and detach input is never sent into the container's terminal.
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

struct RawInput;
impl Drop for RawInput {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

fn forward_output(bytes: &[u8], stdout: &mut impl Write, tail: &mut Vec<u8>) -> Result<()> {
    stdout.write_all(bytes)?;
    tail.extend_from_slice(bytes);
    const LIMIT: usize = 16 * 1024;
    if tail.len() > LIMIT {
        tail.drain(..tail.len() - LIMIT);
    }
    Ok(())
}

fn drain_output(rx: &Receiver<Vec<u8>>, stdout: &mut impl Write, tail: &mut Vec<u8>) -> Result<()> {
    // Bound each batch so a very noisy container cannot starve detach input.
    for bytes in rx.try_iter().take(32) {
        forward_output(&bytes, stdout, tail)?;
    }
    stdout.flush()?;
    Ok(())
}

#[cfg(unix)]
fn read_input(input: &mut [u8]) -> Result<Option<usize>> {
    let mut fd = libc::pollfd {
        fd: libc::STDIN_FILENO,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: fd points to one initialized pollfd for this process's stdin.
    let ready = unsafe { libc::poll(&mut fd, 1, 40) };
    if ready < 0 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            return Ok(Some(0));
        }
        return Err(error.into());
    }
    if fd.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
        return Ok(None);
    }
    if fd.revents & libc::POLLIN == 0 {
        return Ok(Some(0));
    }
    match io::stdin().read(input) {
        Ok(0) => Ok(None),
        Ok(n) => Ok(Some(n)),
        Err(e) if e.kind() == io::ErrorKind::Interrupted => Ok(Some(0)),
        Err(e) => Err(e.into()),
    }
}

/// Own the input escape keys rather than relying on Docker's TTY-dependent
/// detach handling. Raw bytes, terminal output and resize events pass through;
/// Ctrl+C/Ctrl+Q always return to efc, including for non-interactive containers.
#[cfg(unix)]
pub fn run(target: &AttachTarget, mut tick: impl FnMut()) -> Result<Outcome> {
    crossterm::terminal::enable_raw_mode()?;
    let _raw = RawInput;
    let mut stdout = io::stdout().lock();
    write!(
        stdout,
        "\r\nConnecting to {} · Ctrl+Q / Ctrl+C return to efc\r\n\r\n",
        target.name
    )?;
    stdout.flush()?;
    // Docker closes stdin on disconnect when StdinOnce is set without a TTY.
    // In that specific case use output-only attach to preserve the process.
    let id = target.id.clone();
    let (config_tx, config_rx) = mpsc::sync_channel(1);
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
        let _ = config_tx.send(result);
    });
    let mut input = [0; 4096];
    let config = loop {
        tick();
        match config_rx.try_recv() {
            Ok(result) => {
                break result.context("Could not check the container's terminal configuration")?;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                anyhow::bail!("Container terminal check did not complete")
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        let Some(n) = read_input(&mut input)? else {
            return Ok(Outcome::Detached);
        };
        if input[..n].iter().any(|b| matches!(b, 0x03 | 0x11)) {
            return Ok(Outcome::Detached);
        }
    };
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Config {
        tty: bool,
        stdin_once: bool,
    }
    let config: Config = serde_json::from_str(config.trim())
        .context("Unexpected container terminal configuration")?;
    let output_only = !config.tty && config.stdin_once;
    let (mut cols, mut rows) = crossterm::terminal::size()?;
    let pair = native_pty_system()
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("Could not create the attach terminal")?;
    let mut command = CommandBuilder::new(backend::docker_executable());
    command.args(["attach", "--sig-proxy=false", "--detach-keys=ctrl-p,ctrl-q"]);
    if output_only {
        command.arg("--no-stdin");
        write!(
            stdout,
            "Output-only session · preserving this container's stdin\r\n\r\n"
        )?;
        stdout.flush()?;
    }
    command.args(["--", &target.id]);
    let mut client = Client(
        pair.slave
            .spawn_command(command)
            .context("Could not start docker attach")?,
    );
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader()?;
    let mut writer = pair.master.take_writer()?;
    let (output_tx, output_rx) = mpsc::sync_channel(32);
    thread::spawn(move || {
        let mut buffer = [0; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    if output_tx.send(buffer[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                // Unix PTYs may return EIO when their slave closes.
                Err(_) => break,
            }
        }
    });
    write!(
        stdout,
        "\r\nAttached to {} · Ctrl+Q / Ctrl+C return to efc\r\n\r\n",
        target.name
    )?;
    stdout.flush()?;
    let mut tail = Vec::new();
    let mut pending_ctrl_p = false;
    loop {
        tick();
        drain_output(&output_rx, &mut stdout, &mut tail)?;
        if let Some(status) = client.0.try_wait()? {
            // Preserve final stderr/error lines written immediately before exit.
            let until = Instant::now() + Duration::from_millis(250);
            while let Ok(bytes) =
                output_rx.recv_timeout(until.saturating_duration_since(Instant::now()))
            {
                forward_output(&bytes, &mut stdout, &mut tail)?;
                if Instant::now() >= until {
                    break;
                }
            }
            stdout.flush()?;
            return Ok(Outcome::Exited {
                code: status.exit_code(),
                output: String::from_utf8_lossy(&tail).into_owned(),
            });
        }
        let size = crossterm::terminal::size()?;
        if size != (cols, rows) {
            (cols, rows) = size;
            pair.master.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })?;
        }
        let Some(n) = read_input(&mut input)? else {
            return Ok(Outcome::Detached);
        };
        if n == 0 {
            continue;
        }
        let mut outgoing = Vec::with_capacity(n + 1);
        for &byte in &input[..n] {
            if byte == 0x03 || byte == 0x11 {
                if !output_only {
                    writer.write_all(&outgoing)?;
                    writer.flush()?;
                }
                return Ok(Outcome::Detached);
            }
            if pending_ctrl_p {
                outgoing.push(0x10);
                pending_ctrl_p = false;
            }
            if byte == 0x10 {
                pending_ctrl_p = true;
            } else {
                outgoing.push(byte);
            }
        }
        if !output_only {
            writer.write_all(&outgoing)?;
            writer.flush()?;
        }
    }
}

#[cfg(not(unix))]
pub fn run(_target: &AttachTarget, _tick: impl FnMut()) -> Result<Outcome> {
    anyhow::bail!("Interactive attach is currently supported on Linux and macOS")
}
