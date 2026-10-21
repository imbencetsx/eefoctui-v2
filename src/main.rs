mod app;
mod attach;
mod backend;
mod logs;
mod model;
mod ui;

use anyhow::{Context, Result, bail};
use app::{App, Control, Popup, View};
use clap::Parser;
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind,
    },
    execute,
};
use ratatui::{
    Terminal,
    backend::TestBackend,
    buffer::CellDiffOption,
    style::{Color, Modifier},
};
use std::{
    fs,
    io::{self, IsTerminal},
    path::PathBuf,
    time::{Duration, Instant},
};

/// eefoctuiv2 — your fullscreen Docker & Compose control room.
#[derive(Parser)]
#[command(
    name = "efc",
    version,
    about,
    after_help = "Inside efc: 1–4 switch views · ? help · Ctrl+Q quit\nDocker CLI and Compose v2 are needed for container management.\nHost metrics work without Docker. Try `efc --demo` to explore."
)]
struct Args {
    /// Open the embedded console in demo mode.
    #[arg(long, requires = "demo")]
    demo_console: bool,
    /// Explore with clearly labeled sample data. Actions are simulated.
    #[arg(long)]
    demo: bool,
    /// Load a Compose file, including a project that has not been created yet.
    #[arg(short = 'f', long, value_name = "FILE", conflicts_with = "demo")]
    compose: Option<PathBuf>,
    /// Leave mouse reporting disabled (useful for terminal text selection).
    #[arg(long)]
    no_mouse: bool,
    /// Open directly in a view.
    #[arg(long, value_enum, default_value = "containers")]
    view: View,
    /// Render a demo frame to .html or .txt without opening the terminal.
    #[arg(long, value_name = "FILE", requires = "demo")]
    snapshot: Option<PathBuf>,
    /// Snapshot terminal width in columns.
    #[arg(long, default_value_t = 156, value_parser = clap::value_parser!(u16).range(20..=400))]
    width: u16,
    /// Snapshot terminal height in rows.
    #[arg(long, default_value_t = 48, value_parser = clap::value_parser!(u16).range(8..=150))]
    height: u16,
}

struct Restore;
impl Drop for Restore {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste);
        ratatui::restore();
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    if let Some(path) = &args.snapshot {
        return snapshot(&args, path);
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!(
            "efc needs an interactive terminal. Run efc in your terminal, or use efc --demo --snapshot preview.html"
        );
    }
    let compose = args
        .compose
        .as_ref()
        .map(|p| {
            p.canonicalize()
                .with_context(|| format!("Cannot open Compose file {}", p.display()))
        })
        .transpose()?;
    let (sender, receiver) = if args.demo {
        (None, None)
    } else {
        let (tx, rx) = backend::spawn(compose);
        (Some(tx), Some(rx))
    };
    let mut terminal = ratatui::try_init()?;
    let _restore = Restore;
    execute!(io::stdout(), EnableBracketedPaste)?;
    if !args.no_mouse {
        execute!(io::stdout(), EnableMouseCapture)?;
    }
    let mut app = App::new(args.demo, sender);
    app.control(Control::View(args.view));
    app.pressed = None;
    if args.demo_console {
        app.control(Control::Attach);
    }
    let mut last_demo = Instant::now();
    let mut tick = 0u64;
    while !app.quit {
        if let Some(rx) = &receiver {
            while let Ok(update) = rx.try_recv() {
                app.update(update);
            }
        }
        if app.demo && last_demo.elapsed() >= Duration::from_secs(1) {
            tick += 1;
            app.host.cpu = (24.0 + (tick as f32 * 0.4).sin() * 7.0).max(0.0);
            app.host.uptime += 1;
            app::push(&mut app.cpu_history, app.host.cpu as u64);
            app.last_host = Some(Instant::now());
            app.last_docker = Some(Instant::now());
            last_demo = Instant::now();
        }
        if let Some(session) = &mut app.console {
            session.tick();
        }
        terminal.draw(|frame| ui::draw(frame, &mut app))?;
        if event::poll(Duration::from_millis(80))? {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => app.key(key),
                Event::Paste(text) => {
                    if let Some(session) = &mut app.console {
                        session.paste(&text);
                    }
                }
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::Down(event::MouseButton::Left) => {
                        app.click(mouse.column, mouse.row)
                    }
                    MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                        let down = mouse.kind == MouseEventKind::ScrollDown;
                        if let Some(session) = &mut app.console {
                            if session
                                .terminal_rect
                                .contains((mouse.column, mouse.row).into())
                            {
                                let current = session.parser.screen().scrollback();
                                session.parser.screen_mut().set_scrollback(if down {
                                    current.saturating_sub(3)
                                } else {
                                    current.saturating_add(3)
                                });
                            } else {
                                session.scroll_logs(if down { 3 } else { -3 });
                            }
                        } else if matches!(app.popup, Some(Popup::Text { .. })) {
                            for _ in 0..3 {
                                app.key(KeyEvent::new(
                                    if down { KeyCode::Down } else { KeyCode::Up },
                                    KeyModifiers::NONE,
                                ));
                            }
                        } else if app.popup.is_none() {
                            app.navigate(if down { 3 } else { -3 });
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }
    Ok(())
}

fn snapshot(args: &Args, path: &std::path::Path) -> Result<()> {
    let mut terminal = Terminal::new(TestBackend::new(args.width, args.height))?;
    let mut app = App::new(true, None);
    app.control(Control::View(args.view));
    app.pressed = None;
    if args.demo_console {
        app.control(Control::Attach);
    }
    terminal.draw(|frame| ui::draw(frame, &mut app))?;
    let buffer = terminal.backend().buffer();
    let html = path.extension().is_some_and(|s| s == "html");
    let mut output = String::new();
    if html {
        output.push_str(&format!(r#"<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>efc · {} · demo</title><style>*{{box-sizing:border-box}}body{{margin:0;color:#d8dee1;font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace}}.terminal{{background:#000000;padding:16px;border-radius:12px;overflow:hidden}}pre{{margin:0;font-family:inherit;font-size:11px;line-height:1.45;font-variant-ligatures:none;white-space:pre}}.row{{display:flex;height:1.45em}}.row span{{display:block;min-width:0;flex:0 0 calc(1em * .61);line-height:1.45}}.note{{font-size:12px;color:var(--muted-foreground,#979ba9);padding:10px 0;line-height:1.6}}</style><div class="terminal"><pre role="img" aria-label="Actual efc terminal rendering, {} view, simulated data">"#, args.view.name(), args.view.name()));
    }
    for y in 0..args.height {
        if html {
            output.push_str("<span class=\"row\">");
        }
        for x in 0..args.width {
            let cell = &buffer[(x, y)];
            if cell.diff_option == CellDiffOption::Skip {
                continue;
            }
            if html {
                let weight = if cell.modifier.contains(Modifier::BOLD) {
                    "font-weight:700;"
                } else {
                    ""
                };
                output.push_str(&format!(
                    "<span style=\"color:{};background:{};{}\">{}</span>",
                    css_color(cell.fg),
                    css_color(cell.bg),
                    weight,
                    escape(cell.symbol())
                ));
            } else {
                output.push_str(cell.symbol());
            }
        }
        if html {
            output.push_str("</span>");
        } else {
            output.push('\n');
        }
    }
    if html {
        output.push_str(&format!(r#"</pre></div><div class="note">Actual Rust / Ratatui output · {} × {} terminal cells · DEMO: simulated data</div><script>const fit=()=>{{const p=document.querySelector('pre');const w=p.parentElement.clientWidth-32;p.style.fontSize=Math.min(12,w/({}*.61))+'px'}};new ResizeObserver(fit).observe(document.querySelector('.terminal'));fit();</script></html>"#, args.width, args.height, args.width));
    }
    fs::write(path, output).with_context(|| format!("Cannot write {}", path.display()))?;
    println!(
        "Saved {} demo frame to {}",
        args.view.name(),
        path.display()
    );
    Ok(())
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
fn css_color(color: Color) -> String {
    match color {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Reset => "inherit".into(),
        Color::Black => "#000000".into(),
        Color::White => "#ffffff".into(),
        _ => "#979ba9".into(),
    }
}
