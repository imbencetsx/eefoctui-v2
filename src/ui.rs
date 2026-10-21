use crate::{
    app::{App, Control, Popup, View, percent},
    model::*,
};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Cell, Clear, Paragraph, Row, Sparkline, Table, Wrap},
};
use std::{collections::VecDeque, time::Duration};

// Glacier and the two burgundy tones are sampled from the supplied swatch.
pub const BG: Color = Color::Rgb(0, 0, 0);
pub const PANEL: Color = BG;
const RAISED: Color = Color::Rgb(18, 18, 18);
const EDGE: Color = Color::Rgb(58, 60, 65);
const TEXT: Color = Color::Rgb(216, 222, 225);
const MUTED: Color = Color::Rgb(147, 151, 159);
const GLACIER: Color = Color::Rgb(174, 223, 252);
const BURGUNDY: Color = Color::Rgb(84, 24, 32);
const WINE_EDGE: Color = Color::Rgb(112, 47, 62);
const ICE_EDGE: Color = Color::Rgb(59, 93, 111);
const GREEN: Color = Color::Rgb(117, 186, 143);
const YELLOW: Color = Color::Rgb(213, 197, 123);
const ROSE: Color = Color::Rgb(211, 131, 152);
const RED: Color = ROSE;
const SELECT: Color = Color::Rgb(29, 19, 20);

fn panel(title: impl Into<Line<'static>>) -> Block<'static> {
    Block::default()
        .title(title.into().style(Style::default().fg(TEXT).bold()))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(WINE_EDGE))
        .style(Style::default().bg(PANEL).fg(TEXT))
}
fn columns(area: Rect, constraints: impl IntoIterator<Item = Constraint>) -> std::rc::Rc<[Rect]> {
    Layout::default()
        .direction(Direction::Horizontal)
        .spacing(1)
        .constraints(constraints)
        .split(area)
}
fn rows(area: Rect, constraints: impl IntoIterator<Item = Constraint>) -> std::rc::Rc<[Rect]> {
    stacked(area, constraints, 0)
}
fn stacked(
    area: Rect,
    constraints: impl IntoIterator<Item = Constraint>,
    gap: u16,
) -> std::rc::Rc<[Rect]> {
    Layout::default()
        .direction(Direction::Vertical)
        .spacing(gap)
        .constraints(constraints)
        .split(area)
}

fn text(frame: &mut Frame, area: Rect, content: impl Into<ratatui::text::Text<'static>>) {
    frame.render_widget(
        Paragraph::new(content).style(Style::default().fg(TEXT)),
        area,
    );
}
fn label(key: impl Into<String>, value: impl Into<String>) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!(" {:<13}", key.into()), Style::default().fg(MUTED)),
        Span::raw(value.into()),
    ])
}
fn nonempty(s: &str) -> String {
    if s.is_empty() { "—".into() } else { s.into() }
}
fn number(n: Option<f64>) -> String {
    n.map_or_else(|| "—".into(), |n| format!("{n:.1}%"))
}

enum ButtonState {
    Disabled,
    Enabled,
    Selected,
}
impl ButtonState {
    fn new(selected: bool, enabled: bool) -> Self {
        if !enabled {
            Self::Disabled
        } else if selected {
            Self::Selected
        } else {
            Self::Enabled
        }
    }
}

fn button(
    frame: &mut Frame,
    app: &mut App,
    rect: Rect,
    caption: &str,
    control: Control,
    state: ButtonState,
    tint: Color,
) {
    let active = matches!(state, ButtonState::Selected);
    let enabled = !matches!(state, ButtonState::Disabled);
    if rect.width < 4 || rect.height < 3 {
        return;
    }
    let pressed = enabled
        && app.pressed.as_ref().is_some_and(|(name, at)| {
            name == &format!("{control:?}") && at.elapsed() < Duration::from_millis(180)
        });
    let fg = if !enabled {
        MUTED
    } else if active {
        tint
    } else {
        TEXT
    };
    let bg = if pressed || !enabled {
        Color::Rgb(8, 8, 8)
    } else if active {
        BURGUNDY
    } else {
        RAISED
    };
    let (top, bottom) = if !enabled {
        (Color::Rgb(34, 35, 38), BG)
    } else if pressed {
        // Reverse the bevel to make a triggered keycap feel pressed into the face.
        (BG, ICE_EDGE)
    } else {
        (if active { tint } else { WINE_EDGE }, BG)
    };
    // Three solid rows, open sides and two columns of breathing room. The upper
    // and lower eighth-block strokes match the reference keycap silhouette.
    let face = Rect::new(rect.x, rect.y, rect.width.saturating_sub(2), 3);
    let style = Style::default().fg(fg).bg(bg).add_modifier(if enabled {
        Modifier::BOLD
    } else {
        Modifier::empty()
    });
    frame.render_widget(Block::default().style(style), face);
    frame.render_widget(
        Paragraph::new("▔".repeat(face.width as usize)).style(Style::default().fg(top).bg(bg)),
        Rect::new(face.x, face.y, face.width, 1),
    );
    let content = if let Some((key, label)) = caption.split_once("  ") {
        Line::from(vec![
            Span::styled(
                key.to_owned(),
                Style::default().fg(if enabled { tint } else { MUTED }),
            ),
            Span::raw(format!(" {label}")),
        ])
    } else {
        Line::from(caption.to_owned())
    };
    frame.render_widget(
        Paragraph::new(content)
            .alignment(Alignment::Center)
            .style(style),
        Rect::new(face.x, face.y + face.height / 2, face.width, 1),
    );
    frame.render_widget(
        Paragraph::new("▁".repeat(face.width as usize)).style(Style::default().fg(bottom).bg(bg)),
        Rect::new(face.x, face.bottom() - 1, face.width, 1),
    );
    if enabled {
        app.hits.push((face, control));
    }
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    app.hits.clear();
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(BG).fg(TEXT)),
        area,
    );
    if area.width < 58 || area.height < 18 {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from("efc / eefoctuiv2").fg(GLACIER).bold(),
                Line::from(""),
                Line::from("A little more room, please."),
                Line::from("Resize to at least 58 × 18."),
                Line::from(""),
                Line::from("Ctrl+Q exits · Ctrl+C shows exit help"),
            ])
            .block(panel(" Terminal size "))
            .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    let inner = area.inner(Margin::new(1, 0));
    let roomy = area.height >= 26;
    let metrics_height = if area.height >= 32 { 5 } else { 0 };
    let parts = rows(
        inner,
        [
            Constraint::Length(3), // heading
            Constraint::Length(4), // keycaps + a blank row underneath
            Constraint::Length(metrics_height),
            Constraint::Length(u16::from(metrics_height > 0)),
            Constraint::Min(4),                   // active view
            Constraint::Length(u16::from(roomy)), // separate the action bar
            Constraint::Length(4),                // action keycaps + a blank row underneath
            Constraint::Length(1),                // status
            Constraint::Length(1),                // keyboard hints
        ],
    );
    header(frame, app, parts[0]);
    navigation(frame, app, parts[1]);
    if metrics_height > 0 {
        metrics(frame, app, parts[2]);
    }
    match app.view {
        View::Containers => containers(frame, app, parts[4]),
        View::Compose => compose(frame, app, parts[4]),
        View::Ports => ports(frame, app, parts[4]),
        View::System => system(frame, app, parts[4]),
    }
    actions(frame, app, parts[6]);
    status(frame, app, parts[7]);
    text(
        frame,
        parts[8],
        Line::from(vec![
            Span::styled(" ↑↓ / jk ", Style::default().fg(GLACIER)),
            Span::styled("select   ", Style::default().fg(MUTED)),
            Span::styled("/ ", Style::default().fg(GLACIER)),
            Span::styled("filter   ", Style::default().fg(MUTED)),
            Span::styled("? ", Style::default().fg(GLACIER)),
            Span::styled("help   ", Style::default().fg(MUTED)),
            Span::styled("Ctrl+Q ", Style::default().fg(GLACIER)),
            Span::styled("quit", Style::default().fg(MUTED)),
        ]),
    );
    if app.searching {
        search(frame, app, parts[7]);
    }
    if app.popup.is_some() {
        popup(frame, app, area);
    }
}

fn header(frame: &mut Frame, app: &App, area: Rect) {
    let split = columns(
        area,
        [
            Constraint::Min(28),
            Constraint::Length(if area.width > 95 { 50 } else { 25 }),
        ],
    );
    text(
        frame,
        split[0],
        vec![
            Line::from(""),
            Line::from(vec![
                Span::styled(" ▰ efc ", Style::default().fg(GLACIER).bold()),
                Span::styled("/ ", Style::default().fg(EDGE)),
                Span::styled("container control room", Style::default().fg(TEXT)),
                Span::styled(
                    if area.width > 115 {
                        "   eefoctuiv2"
                    } else {
                        ""
                    },
                    Style::default().fg(MUTED),
                ),
            ]),
        ],
    );
    let state = if app.demo {
        "DEMO / simulated data".to_owned()
    } else if app.loading {
        "○ connecting to Docker".into()
    } else if app.docker.error.is_some() {
        "○ Docker offline".into()
    } else if app.docker.data_error.is_some() {
        format!("● {} · container data error", nonempty(&app.docker.context))
    } else {
        format!("● {}", nonempty(&app.docker.context))
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(""),
            Line::from(state).fg(if app.demo {
                GLACIER
            } else if app.docker.error.is_some() || app.docker.data_error.is_some() {
                RED
            } else {
                GREEN
            }),
        ])
        .alignment(Alignment::Right),
        split[1],
    );
}

fn navigation(frame: &mut Frame, app: &mut App, area: Rect) {
    let widths = if area.width < 76 {
        [15, 14, 12, 12]
    } else {
        [21, 18, 15, 15]
    };
    let mut x = area.x;
    for (i, view) in View::ALL.into_iter().enumerate() {
        let width = widths[i].min(area.right().saturating_sub(x));
        button(
            frame,
            app,
            Rect::new(x, area.y, width, 3),
            &format!("{}  {}", i + 1, view.name()),
            Control::View(view),
            ButtonState::new(app.view == view, true),
            GLACIER,
        );
        x += width;
    }
    if area.right().saturating_sub(x) >= 25 {
        frame.render_widget(
            Paragraph::new(format!(
                "{}  ·  {}",
                if app.host.name.is_empty() {
                    "localhost"
                } else {
                    &app.host.name
                },
                app.view.name().to_lowercase()
            ))
            .style(Style::default().fg(MUTED))
            .alignment(Alignment::Right),
            Rect::new(x, area.y + 1, area.right() - x, 1),
        );
    }
}

#[derive(Clone, Copy)]
enum Meter {
    Cpu,
    Memory,
}

fn gradient(value: f64, width: usize, meter: Meter) -> Line<'static> {
    let filled = ((value.clamp(0.0, 100.0) / 100.0) * width as f64).round() as usize;
    let (start, end) = match meter {
        Meter::Cpu => ([103.0, 181.0, 138.0], [218.0, 201.0, 126.0]),
        Meter::Memory => ([135.0, 51.0, 71.0], [220.0, 139.0, 160.0]),
    };
    Line::from(
        (0..width)
            .map(|i| {
                let t = i as f64 / width.saturating_sub(1).max(1) as f64;
                let color = if i >= filled {
                    Color::Rgb(35, 36, 39)
                } else {
                    Color::Rgb(
                        (start[0] + (end[0] - start[0]) * t) as u8,
                        (start[1] + (end[1] - start[1]) * t) as u8,
                        (start[2] + (end[2] - start[2]) * t) as u8,
                    )
                };
                Span::styled("▌", Style::default().fg(color))
            })
            .collect::<Vec<_>>(),
    )
}

fn metric(
    frame: &mut Frame,
    rect: Rect,
    title: (&str, Color),
    value: String,
    description: String,
    bar: Option<(f64, Meter)>,
) {
    let (title, color) = title;
    let block = panel(Line::from(format!(" {title} ")).fg(color)).border_style(
        Style::default().fg(match color {
            GREEN => Color::Rgb(49, 82, 58),
            ROSE => WINE_EDGE,
            _ => ICE_EDGE,
        }),
    );
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let mut content = vec![
        Line::from(vec![
            Span::raw(" "),
            Span::styled(value, Style::default().fg(color).bold()),
        ]),
        Line::from(format!(" {description}")).fg(MUTED),
    ];
    if let Some((v, meter)) = bar {
        let mut line = gradient(v, inner.width.saturating_sub(2) as usize, meter);
        line.spans.insert(0, Span::raw(" "));
        content.push(line);
    }
    frame.render_widget(Paragraph::new(content), inner);
}

/// Count tiles rather than percentage fill: every square is one real container.
/// Empty squares are inactive containers; an overflow label keeps large counts honest.
fn container_tiles(containers: &[Container], width: u16) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }
    if containers.is_empty() {
        return Line::from("—").fg(MUTED);
    }
    let total = containers.len();
    let mut visible = total.min((width as usize).div_ceil(2));
    while visible > 0 {
        let suffix = if visible < total {
            format!(" +{}", total - visible)
        } else {
            String::new()
        };
        if visible * 2 - 1 + suffix.len() <= width as usize {
            break;
        }
        visible -= 1;
    }
    if visible == 0 {
        return Line::from("…").fg(MUTED);
    }
    let mut spans = Vec::new();
    for (i, container) in containers.iter().take(visible).enumerate() {
        if i > 0 {
            spans.push(Span::raw(" "));
        }
        let t = if total == 1 {
            1.0
        } else {
            i as f64 / (total - 1) as f64
        };
        let color = if container.running() {
            Color::Rgb(
                (81.0 + 93.0 * t) as u8,
                (143.0 + 80.0 * t) as u8,
                (173.0 + 79.0 * t) as u8,
            )
        } else {
            ROSE
        };
        spans.push(Span::styled(
            if container.running() { "■" } else { "□" },
            Style::default().fg(color),
        ));
    }
    if visible < total {
        spans.push(Span::styled(
            format!(" +{}", total - visible),
            Style::default().fg(MUTED),
        ));
    }
    Line::from(spans)
}

fn container_metric(frame: &mut Frame, app: &App, rect: Rect) {
    let available = !app.loading && app.docker.error.is_none() && app.docker.data_error.is_none();
    let total = app.docker.containers.len();
    let running = app.docker.containers.iter().filter(|c| c.running()).count();
    let mut description = if app.loading {
        "Waiting for the engine".into()
    } else if app.docker.data_error.is_some() {
        "Engine replied · retrying".into()
    } else if !available {
        "Engine is offline".into()
    } else {
        format!("{running} up · {} down", total - running)
    };
    let project_count = projects(&app.docker.containers, app.docker.configured.as_ref()).len();
    let with_projects = format!("{description} · {project_count} projects");
    if available && with_projects.chars().count() <= rect.width.saturating_sub(4) as usize {
        description = with_projects;
    }
    metric(
        frame,
        rect,
        ("Containers", GLACIER),
        if app.loading {
            "Connecting…".into()
        } else if app.docker.data_error.is_some() {
            "Data error".into()
        } else if available {
            format!("{total} containers")
        } else {
            "Unavailable".into()
        },
        description,
        None,
    );
    let inner = panel("").inner(rect);
    if available && inner.height >= 3 {
        frame.render_widget(
            Paragraph::new(container_tiles(
                &app.docker.containers,
                inner.width.saturating_sub(2),
            )),
            Rect::new(inner.x + 1, inner.y + 2, inner.width.saturating_sub(2), 1),
        );
    }
}

fn metrics(frame: &mut Frame, app: &App, area: Rect) {
    let cells = columns(area, [Constraint::Percentage(25); 4]);
    let ready = app.last_host.is_some();
    metric(
        frame,
        cells[0],
        ("Host CPU", GREEN),
        if ready {
            format!("{:.1}%", app.host.cpu)
        } else {
            "Sampling…".into()
        },
        format!("{} logical cores", app.host.cores.len()),
        ready.then_some((app.host.cpu as f64, Meter::Cpu)),
    );
    metric(
        frame,
        cells[1],
        ("Host memory", ROSE),
        if ready {
            format!(
                "{:.1}%",
                percent(app.host.memory_used, app.host.memory_total)
            )
        } else {
            "Sampling…".into()
        },
        format!(
            "{} / {}",
            bytes(app.host.memory_used),
            bytes(app.host.memory_total)
        ),
        ready.then_some((
            percent(app.host.memory_used, app.host.memory_total),
            Meter::Memory,
        )),
    );
    container_metric(frame, app, cells[2]);
    metric(
        frame,
        cells[3],
        ("Docker engine", GLACIER),
        if app.docker.engine.is_empty() {
            "—".into()
        } else {
            format!("v{}", app.docker.engine)
        },
        if app.demo {
            "simulated environment".into()
        } else {
            nonempty(&app.docker.context)
        },
        None,
    );
}

fn section_header(title: &str, count: usize, query: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!(" {title} "), Style::default().fg(TEXT).bold()),
        Span::styled(format!(" {count} "), Style::default().fg(GLACIER)),
        Span::styled(
            if query.is_empty() {
                "".into()
            } else {
                format!(" / {query} ")
            },
            Style::default().fg(GLACIER),
        ),
    ])
}

fn empty(frame: &mut Frame, rect: Rect, title: &str, message: String) {
    frame.render_widget(
        Paragraph::new(message)
            .block(panel(format!(" {title} ")))
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(MUTED)),
        rect,
    );
}

fn row_hits(app: &mut App, area: Rect, count: usize) {
    // One border row + one header row + one blank row before table data.
    let start = area.y.saturating_add(3);
    for y in start..area.bottom().saturating_sub(1) {
        let index = app.table.offset() + (y - start) as usize;
        if index < count {
            app.hits.push((
                Rect::new(area.x + 1, y, area.width.saturating_sub(2), 1),
                Control::Row(index),
            ));
        }
    }
}

fn docker_connection_message(error: &str) -> String {
    let guidance = if error.contains("Cannot run docker") {
        "Docker's CLI is unavailable. Install Docker or add docker to PATH."
    } else if error.contains(".docker/run/docker.sock")
        && error.contains("no such file or directory")
    {
        "Docker Desktop's engine socket is missing. Open Docker Desktop
  and wait for the engine to finish starting."
    } else {
        "Start your Docker engine, then check the selected Docker context
  and any DOCKER_HOST / DOCKER_CONTEXT overrides."
    };
    format!(
        "\n  efc cannot reach the Docker engine.\n\n  {guidance}\n\n  Reconnects automatically; Shift+R retries now.\n  Host resources and local ports remain available.\n\n  Connection details\n  {error}\n\n  Use efc --demo to explore without an engine."
    )
}

fn docker_data_message(error: &str) -> String {
    format!(
        "\n  Docker replied, but efc could not decode the container list.\n\n  The engine is reachable. This is a data-format error,\n  not evidence that Docker is stopped.\n\n  Shift+R retries; update efc if the problem persists.\n\n  Details\n  {error}"
    )
}

fn containers(frame: &mut Frame, app: &mut App, area: Rect) {
    let split = columns(
        area,
        if area.width >= 108 {
            vec![Constraint::Percentage(66), Constraint::Percentage(34)]
        } else {
            vec![Constraint::Percentage(100), Constraint::Length(0)]
        },
    );
    let activity_height = if area.height >= 23 && split[1].width > 0 {
        8
    } else {
        0
    };
    let left = stacked(
        split[0],
        [Constraint::Min(5), Constraint::Length(activity_height)],
        u16::from(activity_height > 0),
    );
    let table_area = left[0];
    if app.loading {
        empty(frame, table_area, "Containers", "\n  Connecting to Docker…\n\n  Host metrics and ports work independently.\n  Press 4 to explore the system, or Ctrl+Q to quit.".into());
    } else if let Some(error) = &app.docker.error {
        empty(
            frame,
            table_area,
            "Docker is offline",
            docker_connection_message(error),
        );
    } else if let Some(error) = &app.docker.data_error {
        empty(
            frame,
            table_area,
            "Container data error",
            docker_data_message(error),
        );
    } else {
        let compact = table_area.width < 76;
        let containers = app.containers();
        let count = containers.len();
        if count == 0 {
            empty(
                frame,
                table_area,
                "Containers",
                if app.query.is_empty() {
                    "\n  No containers yet.\n\n  Start a workload with Docker, or load a Compose file:\n  efc --compose ./compose.yaml\n\n  Press 2 to manage Compose projects.".into()
                } else {
                    format!(
                        "\n  No matches for \"{}\".\n\n  Press Esc to clear the filter.",
                        app.query
                    )
                },
            );
        } else {
            let data: Vec<Row> = containers
                .into_iter()
                .map(|c| {
                    let status = if c.running() {
                        "● up"
                    } else if c.state == "paused" {
                        "◐ paused"
                    } else {
                        "○ down"
                    };
                    let state = Cell::from(status).style(Style::default().fg(if c.running() {
                        GREEN
                    } else if c.state == "paused" {
                        GLACIER
                    } else {
                        MUTED
                    }));
                    let mem = if c.running() {
                        c.memory.split('/').next().unwrap_or("—").trim().to_owned()
                    } else {
                        "—".into()
                    };
                    let mut cells = vec![
                        Cell::from(c.name.clone()).style(Style::default().bold()),
                        state,
                        Cell::from(number(c.cpu)).style(Style::default().fg(GREEN)),
                        Cell::from(mem).style(Style::default().fg(ROSE)),
                    ];
                    if !compact {
                        cells.push(
                            Cell::from(nonempty(&c.project)).style(Style::default().fg(GLACIER)),
                        );
                    }
                    cells.push(
                        Cell::from(if compact {
                            c.image.clone()
                        } else {
                            nonempty(&c.ports)
                        })
                        .style(Style::default().fg(MUTED)),
                    );
                    Row::new(cells)
                })
                .collect();
            let constraints = if compact {
                vec![
                    Constraint::Percentage(25),
                    Constraint::Length(9),
                    Constraint::Length(8),
                    Constraint::Length(12),
                    Constraint::Min(8),
                ]
            } else {
                vec![
                    Constraint::Percentage(19),
                    Constraint::Length(8),
                    Constraint::Length(7),
                    Constraint::Length(11),
                    Constraint::Percentage(17),
                    Constraint::Min(10),
                ]
            };
            let headings = if compact {
                vec!["NAME", "STATE", "CPU", "MEMORY", "IMAGE"]
            } else {
                vec!["NAME", "STATE", "CPU", "MEMORY", "COMPOSE", "PORTS"]
            };
            let table = Table::new(data, constraints)
                .header(
                    Row::new(headings)
                        .style(Style::default().fg(MUTED))
                        .bottom_margin(1),
                )
                .block(panel(section_header("Containers", count, &app.query)))
                .column_spacing(1)
                .row_highlight_style(Style::default().bg(SELECT).fg(GLACIER))
                .highlight_symbol("▎");
            frame.render_stateful_widget(table, table_area, &mut app.table);
            row_hits(app, table_area, count);
        }
    }
    if split[1].width > 0 {
        inspector(frame, app, split[1]);
    }
    if left[1].height > 0 {
        let lines = app
            .activity
            .iter()
            .rev()
            .take(left[1].height.saturating_sub(2) as usize)
            .map(|event| {
                Line::from(vec![
                    Span::styled(
                        format!(" {:>3}s  ", event.at.elapsed().as_secs()),
                        Style::default().fg(MUTED),
                    ),
                    Span::styled(
                        event.text.clone(),
                        Style::default().fg(if event.error { RED } else { TEXT }),
                    ),
                ])
            })
            .collect::<Vec<_>>();
        frame.render_widget(
            Paragraph::new(lines).block(panel(" Session activity ")),
            left[1],
        );
    }
}

fn history(
    frame: &mut Frame,
    rect: Rect,
    title: String,
    history: &VecDeque<u64>,
    color: Color,
    max: Option<u64>,
) {
    let data: Vec<u64> = history.iter().copied().collect();
    let spark = Sparkline::default()
        .block(panel(title))
        .data(&data)
        .style(Style::default().fg(color))
        .max(max.unwrap_or_else(|| data.iter().max().copied().unwrap_or(1).max(1)));
    frame.render_widget(spark, rect);
}

fn inspector(frame: &mut Frame, app: &App, area: Rect) {
    let Some(c) = app.selected_container() else {
        empty(
            frame,
            area,
            "Inspector",
            "\n  Select a container to see its image,\n  resource use, ports and Compose service."
                .into(),
        );
        return;
    };
    let block = panel(" Selected container ").border_style(Style::default().fg(ICE_EDGE));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut content = vec![
        Line::from(format!(" {}", c.name)).fg(GLACIER).bold(),
        Line::from(format!(" {}", c.status)).fg(if c.running() { GREEN } else { MUTED }),
        Line::from(""),
        label("Image", &c.image),
        label("ID", c.id.chars().take(12).collect::<String>()),
        label("Project", nonempty(&c.project)),
        label("Service", nonempty(&c.service)),
        Line::from(""),
        label("CPU", number(c.cpu)),
        label("Memory", &c.memory),
        label("Net I/O", nonempty(&c.net)),
        label("Block I/O", nonempty(&c.io)),
        label("Processes", nonempty(&c.pids)),
        Line::from(""),
        Line::from(" Ports / bindings").fg(MUTED),
    ];
    for binding in c.ports.split(", ") {
        content.push(Line::from(format!(" {}", nonempty(binding))));
    }
    if let Some(e) = &app.docker.stats_error {
        content.push(Line::from(""));
        content.push(Line::from(format!(" Metrics unavailable: {e}")).fg(RED));
    }
    let history_height = if inner.height >= 22 { 5 } else { 0 };
    let parts = stacked(
        inner,
        [Constraint::Min(1), Constraint::Length(history_height)],
        u16::from(history_height > 0),
    );
    frame.render_widget(Paragraph::new(content).wrap(Wrap { trim: false }), parts[0]);
    if history_height > 0 {
        let empty_history = VecDeque::new();
        history(
            frame,
            parts[1],
            " CPU · last 60 samples ".into(),
            app.container_history.get(&c.id).unwrap_or(&empty_history),
            GLACIER,
            None,
        );
    }
}

fn compose(frame: &mut Frame, app: &mut App, area: Rect) {
    let parts = stacked(
        area,
        [
            Constraint::Min(4),
            Constraint::Length(if area.height >= 14 { 6 } else { 3 }),
        ],
        1,
    );
    let projects = app.projects();
    let count = projects.len();
    if let Some(error) = &app.docker.error {
        empty(
            frame,
            parts[0],
            "Compose",
            format!("\n  Docker is unavailable: {error}"),
        );
    } else if let Some(error) = &app.docker.data_error {
        empty(
            frame,
            parts[0],
            "Container data error",
            docker_data_message(error),
        );
    } else if count == 0 {
        empty(frame, parts[0], "Compose", "\n  No Compose projects found.\n\n  Projects are discovered from container labels, including\n  stopped containers. To create a new stack, launch:\n\n  efc --compose /path/to/compose.yaml\n\n  Then select the project and press U to bring it up.".into());
    } else {
        let data = projects
            .iter()
            .map(|p| {
                Row::new(vec![
                    Cell::from(p.name.clone()).style(Style::default().fg(TEXT).bold()),
                    Cell::from(format!("{} / {}", p.running, p.services))
                        .style(Style::default().fg(if p.running > 0 { GREEN } else { MUTED })),
                    Cell::from(format!("{:.1}%", p.cpu)),
                    Cell::from(nonempty(&p.working_dir)).style(Style::default().fg(MUTED)),
                ])
            })
            .collect::<Vec<_>>();
        let table = Table::new(
            data,
            [
                Constraint::Percentage(27),
                Constraint::Length(14),
                Constraint::Length(10),
                Constraint::Min(12),
            ],
        )
        .header(
            Row::new(["PROJECT", "UP / CONTAINERS", "CPU TOTAL", "DIRECTORY"])
                .style(Style::default().fg(MUTED))
                .bottom_margin(1),
        )
        .block(panel(section_header("Compose projects", count, &app.query)))
        .column_spacing(2)
        .highlight_symbol("▎")
        .row_highlight_style(Style::default().bg(SELECT).fg(GLACIER));
        frame.render_stateful_widget(table, parts[0], &mut app.table);
        row_hits(app, parts[0], count);
    }
    let mut content = Vec::new();
    if let Some(error) = &app.docker.compose_error {
        content.push(Line::from(format!(" Compose config error: {error}")).fg(RED));
    } else if let Some(p) = projects.get(app.selected) {
        content.push(label("Compose files", p.config_files.join(", ")));
    }
    content.extend([
        Line::from(" S start existing containers   X stop project   R restart project").fg(MUTED),
        Line::from(" U bring up · creates missing services, preserves existing containers")
            .fg(GLACIER),
    ]);
    frame.render_widget(
        Paragraph::new(content)
            .block(panel(" Project controls "))
            .wrap(Wrap { trim: false }),
        parts[1],
    );
}

fn ports(frame: &mut Frame, app: &mut App, area: Rect) {
    let parts = stacked(
        area,
        [
            Constraint::Min(4),
            Constraint::Length(if area.height >= 14 { 5 } else { 3 }),
        ],
        1,
    );
    let ports = app.ports();
    let count = ports.len();
    if count == 0 {
        empty(
            frame,
            parts[0],
            "Listening ports & bindings",
            format!(
                "\n  {}\n\n  Clear the filter with Esc or wait for the next host scan.",
                if app.query.is_empty() {
                    "No visible listeners found."
                } else {
                    "No matching ports."
                }
            ),
        );
    } else {
        let data = ports
            .iter()
            .map(|p| {
                Row::new(vec![
                    Cell::from(p.port.clone()).style(Style::default().fg(GLACIER).bold()),
                    Cell::from(p.protocol.clone()),
                    Cell::from(p.address.clone()),
                    Cell::from(p.process.clone()),
                    Cell::from(p.pid.clone()),
                    Cell::from(p.source.clone()).style(
                        Style::default().fg(if p.source == "docker" { GLACIER } else { GREEN }),
                    ),
                ])
            })
            .collect::<Vec<_>>();
        let table = Table::new(
            data,
            [
                Constraint::Length(7),
                Constraint::Length(6),
                Constraint::Percentage(23),
                Constraint::Min(12),
                Constraint::Length(8),
                Constraint::Length(8),
            ],
        )
        .header(
            Row::new([
                "PORT",
                "PROTO",
                "BIND ADDRESS",
                "PROCESS / CONTAINER",
                "PID",
                "SOURCE",
            ])
            .style(Style::default().fg(MUTED))
            .bottom_margin(1),
        )
        .block(panel(section_header(
            "Listening ports & bindings",
            count,
            &app.query,
        )))
        .column_spacing(1)
        .highlight_symbol("▎")
        .row_highlight_style(Style::default().bg(SELECT).fg(GLACIER));
        frame.render_stateful_widget(table, parts[0], &mut app.table);
        row_hits(app, parts[0], count);
    }
    let mut lines = vec![
        Line::from(" Host: local TCP listeners + unconnected UDP bindings · scan every 5s")
            .fg(MUTED),
        Line::from(" Docker: published bindings from the active context (may be remote).")
            .fg(MUTED),
        Line::from(" Process visibility follows OS permissions; no privilege escalation.")
            .fg(MUTED),
    ];
    if let Some(e) = &app.host.ports_error {
        lines.insert(
            0,
            Line::from(format!(" Host scan unavailable: {e}")).fg(RED),
        );
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel(" Socket notes "))
            .wrap(Wrap { trim: false }),
        parts[1],
    );
}

fn system(frame: &mut Frame, app: &App, area: Rect) {
    if app.last_host.is_none() {
        empty(frame, area, "System", "\n  Sampling host resources…".into());
        return;
    }
    let split = columns(
        area,
        [Constraint::Percentage(44), Constraint::Percentage(56)],
    );
    let h = &app.host;
    let history_height = if area.height >= 19 { 6 } else { 0 };
    let parts = stacked(
        split[0],
        [Constraint::Min(7), Constraint::Length(history_height)],
        u16::from(history_height > 0),
    );
    let info = vec![
        Line::from(format!(" {}", h.name)).fg(GLACIER).bold(),
        Line::from(""),
        label("OS", &h.os),
        label("Kernel", &h.kernel),
        label("Architecture", &h.arch),
        label("CPU", &h.cpu_name),
        label("Logical cores", h.cores.len().to_string()),
        label("Uptime", uptime(h.uptime)),
        label(
            "Load avg",
            format!("{:.2}  {:.2}  {:.2}", h.load[0], h.load[1], h.load[2]),
        ),
        Line::from(""),
        label(
            "Memory",
            format!("{} / {}", bytes(h.memory_used), bytes(h.memory_total)),
        ),
        label(
            "Swap",
            format!("{} / {}", bytes(h.swap_used), bytes(h.swap_total)),
        ),
        Line::from(""),
        label(
            "Net received",
            if h.network_sampled {
                format!("{} / sample", bytes(h.rx))
            } else {
                "Sampling…".into()
            },
        ),
        label(
            "Net sent",
            if h.network_sampled {
                format!("{} / sample", bytes(h.tx))
            } else {
                "Sampling…".into()
            },
        ),
        Line::from(""),
        Line::from(" Local host · loopback excluded from network totals").fg(MUTED),
    ];
    frame.render_widget(
        Paragraph::new(info)
            .block(panel(" System information "))
            .wrap(Wrap { trim: false })
            .scroll((app.system_scroll.min(u16::MAX as usize) as u16, 0)),
        parts[0],
    );
    if parts[1].height > 0 {
        history(
            frame,
            parts[1],
            " Host CPU · 60 samples ".into(),
            &app.cpu_history,
            GLACIER,
            Some(100),
        );
    }
    let mut lines = vec![
        Line::from(" CPU cores").fg(GLACIER).bold(),
        Line::from(" Local host utilization · 1s sampling").fg(MUTED),
        Line::from(""),
    ];
    let bar_width = split[1].width.saturating_sub(22).min(45) as usize;
    for (i, cpu) in h.cores.iter().enumerate() {
        let mut core = Line::from(vec![Span::styled(
            format!(" {i:>3}  "),
            Style::default().fg(MUTED),
        )]);
        core.spans
            .extend(gradient(*cpu as f64, bar_width, Meter::Cpu).spans);
        core.spans.push(Span::styled(
            format!("  {cpu:5.1}%"),
            Style::default().fg(GREEN),
        ));
        lines.push(core);
    }
    lines.extend([
        Line::from(""),
        Line::from(" Filesystems").fg(YELLOW).bold(),
        Line::from(""),
    ]);
    for disk in &h.disks {
        lines.push(Line::from(format!(" {}", disk.mount)).fg(MUTED));
        lines.push(Line::from(format!(
            "   {} / {}    {:.0}%",
            bytes(disk.used),
            bytes(disk.total),
            percent(disk.used, disk.total)
        )));
    }
    lines.extend([
        Line::from(""),
        Line::from(" ↑↓ / PgUp PgDn to scroll").fg(MUTED),
    ]);
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel(" Resources ").border_style(Style::default().fg(ICE_EDGE)))
            .scroll((app.system_scroll.min(u16::MAX as usize) as u16, 0)),
        split[1],
    );
}

fn actions(frame: &mut Frame, app: &mut App, area: Rect) {
    let available = !app.busy
        && !app.loading
        && app.docker.error.is_none()
        && app.docker.data_error.is_none()
        && app.target().is_some();
    let mut controls = match app.view {
        View::Containers => vec![
            (
                "s  Start",
                Control::Start,
                GREEN,
                available && app.selected_container().is_some_and(|c| !c.running()),
            ),
            (
                "x  Stop",
                Control::Stop,
                RED,
                available && app.selected_container().is_some_and(Container::running),
            ),
            ("r  Restart", Control::Restart, YELLOW, available),
            (
                "a  Attach",
                Control::Attach,
                GLACIER,
                available && app.selected_container().is_some_and(Container::running),
            ),
            ("l  Logs", Control::Logs, TEXT, available),
            ("i  Inspect", Control::Inspect, TEXT, available),
        ],
        View::Compose => vec![
            ("s  Start", Control::Start, GREEN, available),
            ("x  Stop", Control::Stop, RED, available),
            ("r  Restart", Control::Restart, YELLOW, available),
            ("u  Bring up", Control::Up, GLACIER, available),
        ],
        _ => vec![],
    };
    controls.push(("R  Refresh", Control::Refresh, TEXT, !app.busy));
    controls.push(("?  Help", Control::Help, TEXT, true));
    let preferred_width = if area.width >= controls.len() as u16 * 15 {
        15
    } else {
        12
    };
    let mut x = area.x;
    for (caption, control, tint, enabled) in controls {
        let width = preferred_width.max(caption.chars().count() as u16 + 3);
        if area.right().saturating_sub(x) < width {
            break;
        }
        button(
            frame,
            app,
            Rect::new(x, area.y, width, 3),
            caption,
            control,
            ButtonState::new(false, enabled),
            tint,
        );
        x += width;
    }
}

fn status(frame: &mut Frame, app: &App, area: Rect) {
    let (message, color) = if let Some(n) = app
        .notice
        .as_ref()
        .filter(|n| n.at.elapsed() < Duration::from_secs(9) || app.busy)
    {
        (format!(" {}", n.text), if n.error { RED } else { GLACIER })
    } else if app.busy {
        (
            " Docker action in progress… · host monitoring remains active".into(),
            GLACIER,
        )
    } else if app.demo {
        (
            " DEMO · simulated data and actions · launch efc for your real machine".into(),
            GLACIER,
        )
    } else if app.docker.data_error.is_some() {
        (
            " Docker connected · container data could not be decoded · R to retry".into(),
            RED,
        )
    } else if app.docker.stats_error.is_some() {
        (
            " Container metrics unavailable · Docker stats failed · R to retry".into(),
            RED,
        )
    } else if app.docker.compose_error.is_some() {
        (
            " Compose file could not be loaded · see the Compose view".into(),
            RED,
        )
    } else {
        (
            format!(
                " {} / {} · Docker {} · host {}",
                if app.count() == 0 {
                    0
                } else {
                    app.selected + 1
                },
                app.count(),
                app.last_docker.map_or_else(
                    || "connecting".into(),
                    |t| format!("{}s ago", t.elapsed().as_secs())
                ),
                app.last_host.map_or_else(
                    || "sampling".into(),
                    |t| format!("{}s ago", t.elapsed().as_secs())
                )
            ),
            MUTED,
        )
    };
    frame.render_widget(
        Paragraph::new(message).style(Style::default().fg(color).bg(PANEL)),
        area,
    );
}

fn search(frame: &mut Frame, app: &App, area: Rect) {
    frame.render_widget(
        Paragraph::new(format!(
            " / {}▏  Enter to apply · Esc to leave · Ctrl+U clear",
            app.query
        ))
        .style(Style::default().fg(TEXT).bg(SELECT)),
        area,
    );
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(4));
    let height = height.min(area.height.saturating_sub(2));
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

fn popup(frame: &mut Frame, app: &mut App, area: Rect) {
    // Cover the screen with a calm scrim, preserving context while isolating the dialog.
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            if let Some(cell) = frame.buffer_mut().cell_mut((x, y)) {
                cell.set_fg(EDGE);
            }
        }
    }
    app.hits.clear();
    let (width, height) = match app.popup {
        Some(Popup::Text { .. }) => (
            area.width.saturating_sub(8).min(130),
            area.height.saturating_sub(6),
        ),
        Some(Popup::Help) => (78, 30),
        Some(Popup::Confirm { .. }) => (68, 15),
        _ => (62, 13),
    };
    let rect = centered(area, width, height);
    frame.render_widget(Clear, rect);
    let (title, lines) = match app.popup.as_ref().unwrap() {
        Popup::Interrupt => (
            " Take a breath ".to_owned(),
            vec![
                Line::from(""),
                Line::from(" Ctrl+C doesn't close this control room.")
                    .fg(TEXT)
                    .bold(),
                Line::from(""),
                Line::from(" Use Ctrl+Q to quit efc.").fg(GLACIER),
                Line::from(" Your containers keep running when you leave.").fg(MUTED),
            ],
        ),
        Popup::Confirm { action, .. } => {
            let scope = match &action.target {
                Target::Compose(_) => "every service in this Compose project",
                _ => "the selected container",
            };
            (
                format!(" {} {}? ", action.verb.label(), action.target.name()),
                vec![
                    Line::from(""),
                    Line::from(format!(" {} will affect {scope}.", action.verb.label())).fg(TEXT),
                    Line::from(""),
                    Line::from(match action.verb {
                        Verb::Stop => " Docker sends a graceful stop signal. Volumes are kept.",
                        Verb::Restart => " Running services will briefly be unavailable.",
                        Verb::Up => " Compose may create containers and pull missing images.",
                        _ => "",
                    })
                    .fg(MUTED),
                    Line::from(" Existing containers are preserved; nothing is removed.").fg(MUTED),
                    Line::from(""),
                    Line::from(" Tab / ← → choose · Enter select · Y confirm · Esc cancel")
                        .fg(GLACIER),
                ],
            )
        }
        Popup::Help => (
            " The controls ".into(),
            vec![
                Line::from(""),
                Line::from(" NAVIGATE").fg(GLACIER).bold(),
                label("1 / 2 / 3 / 4", "Containers / Compose / Ports / System"),
                label("Tab / S-Tab", "Next / previous view"),
                label("↑↓ or j / k", "Select row / scroll system"),
                label("PgUp / PgDn", "Jump 10 rows"),
                label("Home / End", "First / last row"),
                label("/", "Filter the active list · Esc clears"),
                Line::from(""),
                Line::from(" CONTROL").fg(GLACIER).bold(),
                label("s / x / r", "Start / stop / restart selection"),
                label("u", "Compose up -d --no-recreate"),
                label("l / i", "Container logs / inspect JSON"),
                label("a", "Attach · Ctrl+Q / Ctrl+C return to efc"),
                label("Shift+R", "Refresh Docker connection"),
                label("Ctrl+L", "Refresh Docker connection"),
                Line::from(""),
                Line::from(" FEEL AT HOME").fg(GLACIER).bold(),
                label("Mouse", "Click tabs, rows and buttons; wheel scrolls"),
                label("Ctrl+C", "Exit reminder · your session stays open"),
                label("Ctrl+Q", "Quit from anywhere"),
                label("Esc", "Close popup / clear filter"),
            ],
        ),
        Popup::Text { title, .. } => (format!(" {title} "), vec![]),
        Popup::Loading { title } => (
            format!(" {title} "),
            vec![
                Line::from(""),
                Line::from(" Waiting for Docker…").fg(GLACIER),
                Line::from(""),
                Line::from(" Esc dismisses this window.").fg(MUTED),
            ],
        ),
    };
    let block = panel(title).border_style(Style::default().fg(GLACIER));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let content_height = inner.height.saturating_sub(5);
    let body = Rect::new(
        inner.x + 1,
        inner.y,
        inner.width.saturating_sub(2),
        content_height,
    );
    if let Some(Popup::Text {
        content,
        scroll,
        horizontal,
        ..
    }) = &app.popup
    {
        frame.render_widget(
            Paragraph::new(clean_text(content))
                .style(Style::default().fg(TEXT))
                .scroll((*scroll, *horizontal)),
            body,
        );
    } else {
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
    }
    let bottom = Rect::new(
        inner.x + 1,
        inner.bottom().saturating_sub(4),
        inner.width.saturating_sub(2),
        3,
    );
    match app.popup.as_ref() {
        Some(Popup::Confirm { accept, .. }) => {
            let accept = *accept;
            button(
                frame,
                app,
                Rect::new(bottom.x, bottom.y, 15, 3),
                "Cancel",
                Control::Cancel,
                ButtonState::new(!accept, true),
                TEXT,
            );
            button(
                frame,
                app,
                Rect::new(bottom.x + 17, bottom.y, 17, 3),
                "Confirm",
                Control::Confirm,
                ButtonState::new(accept, true),
                GLACIER,
            );
        }
        Some(Popup::Interrupt) => {
            button(
                frame,
                app,
                Rect::new(bottom.x, bottom.y, 21, 3),
                "Enter  Keep working",
                Control::Cancel,
                ButtonState::Selected,
                TEXT,
            );
            button(
                frame,
                app,
                Rect::new(bottom.x + 23, bottom.y, 20, 3),
                "Ctrl+Q  Quit",
                Control::Quit,
                ButtonState::Enabled,
                GLACIER,
            );
        }
        _ => {
            button(
                frame,
                app,
                Rect::new(bottom.x, bottom.y, 18, 3),
                "Esc  Close",
                Control::Cancel,
                ButtonState::Enabled,
                TEXT,
            );
            if bottom.width > 50 {
                text(
                    frame,
                    Rect::new(bottom.x + 20, bottom.y + 1, bottom.width - 20, 1),
                    Line::from("↑↓ scroll · ←→ pan").fg(MUTED),
                );
            }
        }
    }
}

/// Prevent log escape sequences (including OSC title/clipboard codes) becoming
/// terminal instructions or visible junk in a popup.
pub fn clean_text(input: &str) -> String {
    let mut result = String::new();
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.next() {
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\x07' || (c == '\x1b' && chars.peek() == Some(&'\\')) {
                            if c == '\x1b' {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                _ => {}
            }
        } else if c == '\n' || c == '\t' || !c.is_control() {
            result.push(c);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    #[test]
    fn container_tiles_preserve_counts_states_and_overflow() {
        let running = Container {
            state: "running".into(),
            ..Container::default()
        };
        for count in [1, 6] {
            let line = container_tiles(&vec![running.clone(); count], 32);
            let text = line.to_string();
            assert_eq!(text.chars().filter(|c| *c == '■').count(), count);
            assert!(!text.contains('+'));
        }
        let mixed = container_tiles(
            &[running.clone(), Container::default(), running.clone()],
            32,
        )
        .to_string();
        assert_eq!(mixed.chars().filter(|c| *c == '■').count(), 2);
        assert_eq!(mixed.chars().filter(|c| *c == '□').count(), 1);
        let line = container_tiles(&vec![running; 50], 14);
        assert!(line.width() <= 14);
        let text = line.to_string();
        let shown = text.chars().filter(|c| *c == '■').count();
        let hidden: usize = text.rsplit_once('+').unwrap().1.parse().unwrap();
        assert_eq!(shown + hidden, 50);
        assert_eq!(container_tiles(&[Container::default()], 0).width(), 0);
    }
    #[test]
    fn missing_desktop_socket_explains_how_to_reconnect() {
        let message = docker_connection_message(
            "dial unix /Users/dev/.docker/run/docker.sock: connect: no such file or directory",
        );
        assert!(message.contains("Open Docker Desktop"));
        assert!(message.contains("Reconnects automatically"));
        assert!(!message.contains("Install Docker"));
    }
    #[test]
    fn all_views_and_dialogs_render_at_small_and_large_sizes() {
        for (width, height) in [(20, 8), (58, 18), (80, 24), (120, 36), (156, 48), (200, 60)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let mut app = App::new(true, None);
            for view in View::ALL {
                app.control(Control::View(view));
                terminal.draw(|f| draw(f, &mut app)).unwrap();
                app.control(Control::Help);
                terminal.draw(|f| draw(f, &mut app)).unwrap();
                app.popup = None;
            }
            app.control(Control::View(View::Containers));
            app.control(Control::Stop);
            terminal.draw(|f| draw(f, &mut app)).unwrap();
            app.popup = Some(Popup::Interrupt);
            terminal.draw(|f| draw(f, &mut app)).unwrap();
            app.popup = Some(Popup::Text {
                title: "Logs".into(),
                content: "a long line ".repeat(200),
                scroll: 0,
                horizontal: 0,
            });
            terminal.draw(|f| draw(f, &mut app)).unwrap();
        }
    }
    #[test]
    fn missing_docker_empty_and_filtered_states_render() {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let mut app = App::new(false, None);
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        app.update(Update::Docker(DockerSnapshot {
            error: Some("Docker not found".into()),
            ..DockerSnapshot::default()
        }));
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        app.update(Update::Docker(DockerSnapshot::default()));
        terminal.draw(|f| draw(f, &mut app)).unwrap();
    }
    #[test]
    fn log_escape_sequences_are_removed() {
        assert_eq!(
            clean_text("\x1b[31merror\x1b[0m\n\x1b]52;c;secret\x07done"),
            "error\ndone"
        );
    }
}
