use crate::{backend, model::*};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{layout::Rect, widgets::TableState};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::mpsc::SyncSender,
    time::Instant,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum View {
    Containers,
    Compose,
    Ports,
    System,
}
impl View {
    pub const ALL: [Self; 4] = [Self::Containers, Self::Compose, Self::Ports, Self::System];
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|v| *v == self).unwrap_or(0)
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Containers => "Containers",
            Self::Compose => "Compose",
            Self::Ports => "Ports",
            Self::System => "System",
        }
    }
}

pub enum Popup {
    Interrupt,
    Help,
    Confirm {
        action: Action,
        accept: bool,
    },
    Text {
        title: String,
        content: String,
        scroll: u16,
        horizontal: u16,
    },
    Loading {
        title: String,
    },
}

#[derive(Clone, Debug)]
pub enum Control {
    View(View),
    Row(usize),
    Start,
    Stop,
    Restart,
    Attach,
    Up,
    Logs,
    Inspect,
    Refresh,
    Help,
    Cancel,
    Confirm,
    Quit,
    Filter,
    ClearFilter,
}

pub struct Notice {
    pub text: String,
    pub error: bool,
    pub at: Instant,
}

pub struct App {
    pub docker: DockerSnapshot,
    pub host: Host,
    pub view: View,
    pub selected: usize,
    pub table: TableState,
    pub query: String,
    pub searching: bool,
    pub popup: Option<Popup>,
    pub notice: Option<Notice>,
    pub activity: VecDeque<Notice>,
    pub busy: bool,
    pub loading: bool,
    pub demo: bool,
    pub quit: bool,
    pub hits: Vec<(Rect, Control)>,
    pub cpu_history: VecDeque<u64>,
    pub mem_history: VecDeque<u64>,
    pub container_history: BTreeMap<String, VecDeque<u64>>,
    pub last_docker: Option<Instant>,
    pub last_host: Option<Instant>,
    pub system_scroll: usize,
    pub pressed: Option<(String, Instant)>,
    pub pending_attach: Option<AttachTarget>,
    sender: Option<SyncSender<Request>>,
}

impl App {
    pub fn new(demo_mode: bool, sender: Option<SyncSender<Request>>) -> Self {
        let (docker, host) = if demo_mode {
            demo()
        } else {
            (DockerSnapshot::default(), Host::default())
        };
        let mut app = Self {
            docker,
            host,
            view: View::Containers,
            selected: 0,
            table: TableState::default().with_selected(0),
            query: String::new(),
            searching: false,
            popup: None,
            notice: None,
            activity: VecDeque::new(),
            busy: false,
            loading: !demo_mode,
            demo: demo_mode,
            quit: false,
            hits: Vec::new(),
            cpu_history: VecDeque::new(),
            mem_history: VecDeque::new(),
            container_history: BTreeMap::new(),
            last_docker: demo_mode.then(Instant::now),
            last_host: demo_mode.then(Instant::now),
            system_scroll: 0,
            pressed: None,
            pending_attach: None,
            sender,
        };
        if demo_mode {
            for i in 0..60 {
                push(
                    &mut app.cpu_history,
                    (19.0 + (i as f64 * 0.32).sin() * 8.0 + (i as f64 * 0.9).cos() * 4.0) as u64,
                );
                push(&mut app.mem_history, 36 + i / 30);
                for c in &app.docker.containers {
                    let value = (c.cpu.unwrap_or_default()
                        * (0.7 + (i as f64 * 0.35).sin().abs() * 0.6)
                        * 10.0) as u64;
                    push(
                        app.container_history.entry(c.id.clone()).or_default(),
                        value,
                    );
                }
            }
        }
        app.record(
            if demo_mode {
                "Demo session opened · actions are simulated"
            } else {
                "Session opened · connecting to Docker"
            },
            false,
        );
        app
    }

    pub fn containers(&self) -> Vec<&Container> {
        let q = self.query.to_lowercase();
        self.docker
            .containers
            .iter()
            .filter(|c| {
                format!(
                    "{} {} {} {} {}",
                    c.name, c.image, c.project, c.state, c.ports
                )
                .to_lowercase()
                .contains(&q)
            })
            .collect()
    }
    pub fn projects(&self) -> Vec<Project> {
        let q = self.query.to_lowercase();
        projects(&self.docker.containers, self.docker.configured.as_ref())
            .into_iter()
            .filter(|p| p.name.to_lowercase().contains(&q))
            .collect()
    }
    pub fn ports(&self) -> Vec<Port> {
        let q = self.query.to_lowercase();
        let mut ports = self.host.ports.clone();
        ports.extend(backend::published_ports(&self.docker.containers));
        ports.retain(|p| {
            format!(
                "{} {} {} {} {} {}",
                p.protocol, p.address, p.port, p.process, p.pid, p.source
            )
            .to_lowercase()
            .contains(&q)
        });
        ports.sort_by_key(|p| {
            (
                p.port.parse::<u16>().unwrap_or(0),
                p.source.clone(),
                p.address.clone(),
            )
        });
        ports
    }
    pub fn count(&self) -> usize {
        match self.view {
            View::Containers => self.containers().len(),
            View::Compose => self.projects().len(),
            View::Ports => self.ports().len(),
            View::System => 0,
        }
    }
    pub fn selected_container(&self) -> Option<&Container> {
        self.containers().get(self.selected).copied()
    }
    pub fn target(&self) -> Option<Target> {
        match self.view {
            View::Containers => self.selected_container().map(|c| Target::Container {
                id: c.id.clone(),
                name: c.name.clone(),
            }),
            View::Compose => self
                .projects()
                .get(self.selected)
                .cloned()
                .map(Target::Compose),
            _ => None,
        }
    }
    fn clamp(&mut self) {
        self.selected = self.selected.min(self.count().saturating_sub(1));
        self.table
            .select((self.count() > 0).then_some(self.selected));
    }
    pub fn tell(&mut self, text: impl Into<String>, error: bool) {
        let text = text.into();
        self.record(text.clone(), error);
        self.notice = Some(Notice {
            text,
            error,
            at: Instant::now(),
        });
    }
    fn record(&mut self, text: impl Into<String>, error: bool) {
        if self.activity.len() >= 100 {
            self.activity.pop_front();
        }
        self.activity.push_back(Notice {
            text: text.into(),
            error,
            at: Instant::now(),
        });
    }
    pub fn update(&mut self, update: Update) {
        match update {
            Update::Docker(docker) => {
                if docker.error != self.docker.error
                    || docker.data_error != self.docker.data_error
                    || self.loading
                {
                    if let Some(error) = &docker.error {
                        self.record(format!("Docker offline · {error}"), true);
                    } else if let Some(error) = &docker.data_error {
                        self.record(format!("Docker data error · {error}"), true);
                    } else {
                        self.record(
                            format!(
                                "Connected · {} containers · {}",
                                docker.containers.len(),
                                docker.context
                            ),
                            false,
                        );
                    }
                }
                let changes: Vec<_> = docker
                    .containers
                    .iter()
                    .filter_map(|c| {
                        self.docker
                            .containers
                            .iter()
                            .find(|old| old.id == c.id && old.state != c.state)
                            .map(|old| format!("{} · {} → {}", c.name, old.state, c.state))
                    })
                    .collect();
                for change in changes {
                    self.record(change, false);
                }
                let selected_id = self.selected_container().map(|c| c.id.clone());
                let selected_project = self.projects().get(self.selected).map(|p| p.name.clone());
                for c in &docker.containers {
                    if let Some(cpu) = c.cpu {
                        push(
                            self.container_history.entry(c.id.clone()).or_default(),
                            (cpu * 10.0) as u64,
                        );
                    }
                }
                self.container_history
                    .retain(|id, _| docker.containers.iter().any(|c| &c.id == id));
                self.docker = docker;
                self.loading = false;
                self.last_docker = Some(Instant::now());
                if self.view == View::Containers {
                    if let Some(index) = self
                        .containers()
                        .iter()
                        .position(|c| Some(&c.id) == selected_id.as_ref())
                    {
                        self.selected = index;
                    }
                } else if self.view == View::Compose
                    && let Some(index) = self
                        .projects()
                        .iter()
                        .position(|p| Some(&p.name) == selected_project.as_ref())
                {
                    self.selected = index;
                }
                self.clamp();
            }
            Update::Host(host) => {
                if self.last_host.is_some() {
                    push(&mut self.cpu_history, host.cpu as u64);
                }
                push(
                    &mut self.mem_history,
                    percent(host.memory_used, host.memory_total) as u64,
                );
                self.host = host;
                self.last_host = Some(Instant::now());
                self.clamp();
            }
            Update::ActionDone(result) => {
                self.busy = false;
                match result {
                    Ok(s) => self.tell(s, false),
                    Err(e) => {
                        self.tell("Action failed · press Esc to close details", true);
                        self.popup = Some(Popup::Text {
                            title: "Docker action failed".into(),
                            content: e,
                            scroll: 0,
                            horizontal: 0,
                        });
                    }
                }
            }
            Update::Text { title, content } => {
                // A dismissed/changed popup must never reopen when a slow request finishes.
                if matches!(&self.popup, Some(Popup::Loading { title: pending }) if pending == &title)
                {
                    self.popup = Some(Popup::Text {
                        title,
                        content: content.unwrap_or_else(|e| e),
                        scroll: 0,
                        horizontal: 0,
                    });
                }
            }
        }
    }
    fn send(&mut self, request: Request) -> bool {
        if let Some(sender) = &self.sender {
            if sender.try_send(request).is_ok() {
                return true;
            }
            self.tell("Worker queue is busy · try again shortly", true);
        }
        false
    }
    fn request_action(&mut self, verb: Verb) {
        if self.busy {
            self.tell("An action is already in progress", true);
            return;
        }
        if self.loading || self.docker.error.is_some() {
            self.tell("Docker is unavailable · refresh the connection first", true);
            return;
        }
        if self.docker.data_error.is_some() {
            self.tell(
                "Container data is unavailable · refresh before managing containers",
                true,
            );
            return;
        }
        let Some(target) = self.target() else {
            self.tell("Select a container or Compose project first", true);
            return;
        };
        if verb == Verb::Up && !matches!(target, Target::Compose(_)) {
            return;
        }
        if let Target::Container { .. } = target
            && let Some(c) = self.selected_container()
        {
            if verb == Verb::Start && c.running() {
                self.tell("This container is already running", false);
                return;
            }
            if verb == Verb::Stop && !c.running() {
                self.tell("This container is not running", false);
                return;
            }
        }
        let action = Action { verb, target };
        if matches!(verb, Verb::Stop | Verb::Restart | Verb::Up) {
            self.popup = Some(Popup::Confirm {
                action,
                accept: false,
            });
        } else {
            self.execute(action);
        }
    }
    fn execute(&mut self, action: Action) {
        if self.demo {
            for c in &mut self.docker.containers {
                let matches = match &action.target {
                    Target::Container { id, .. } => &c.id == id,
                    Target::Compose(p) => c.project == p.name,
                };
                if matches {
                    c.state = if action.verb == Verb::Stop {
                        "exited"
                    } else {
                        "running"
                    }
                    .into();
                    c.status = if c.running() {
                        "Up a few seconds (demo)"
                    } else {
                        "Exited (0) just now (demo)"
                    }
                    .into();
                    c.cpu = c.running().then_some(0.4);
                    c.memory_percent = c.running().then_some(0.2);
                    c.memory = if c.running() {
                        "18.4 MiB / 8 GiB"
                    } else {
                        "—"
                    }
                    .into();
                }
            }
            self.tell(
                format!(
                    "DEMO · {} simulated for {}",
                    action.verb.label(),
                    action.target.name()
                ),
                false,
            );
        } else if self.send(Request::Action(action.clone())) {
            self.busy = true;
            self.tell(
                format!("{} · {}…", action.verb.label(), action.target.name()),
                false,
            );
        }
    }
    pub fn control(&mut self, control: Control) {
        self.pressed = Some((format!("{control:?}"), Instant::now()));
        match control {
            Control::View(view) => {
                self.query.clear();
                self.searching = false;
                self.view = view;
                self.selected = 0;
                self.table = TableState::default();
                self.clamp();
                self.system_scroll = 0;
            }
            Control::Row(index) => {
                self.selected = index;
                self.clamp();
            }
            Control::Start => self.request_action(Verb::Start),
            Control::Stop => self.request_action(Verb::Stop),
            Control::Restart => self.request_action(Verb::Restart),
            Control::Attach => {
                if self.view != View::Containers {
                    return;
                }
                if self.busy
                    || self.loading
                    || self.docker.error.is_some()
                    || self.docker.data_error.is_some()
                {
                    self.tell(
                        "Attach is unavailable while Docker is busy or disconnected",
                        true,
                    );
                    return;
                }
                let Some(c) = self.selected_container() else {
                    self.tell("Select a running container to attach", false);
                    return;
                };
                if !c.running() {
                    self.tell("Start this container before attaching", false);
                    return;
                }
                let target = AttachTarget {
                    id: c.id.clone(),
                    name: c.name.clone(),
                };
                if self.demo {
                    self.popup = Some(Popup::Text {
                        title: format!("Attach · {}", target.name),
                        content: "DEMO · no real container is attached.\n\nIn live mode, Attach connects to the running container's main\nprocess with an interactive terminal.\n\nCtrl+Q or Ctrl+C detaches and returns to efc.\nCtrl+P, Ctrl+Q also detaches.\n\nAttach does not launch a shell or restart the container.".into(),
                        scroll: 0, horizontal: 0,
                    });
                } else {
                    self.pending_attach = Some(target);
                }
            }
            Control::Up => self.request_action(Verb::Up),
            Control::Logs | Control::Inspect => {
                if self.view != View::Containers {
                    return;
                }
                let Some(c) = self.selected_container().cloned() else {
                    return;
                };
                let logs = matches!(control, Control::Logs);
                let title = format!("{} · {}", if logs { "Logs" } else { "Inspect" }, c.name);
                if self.demo {
                    let content = if logs {
                        format!(
                            "[DEMO · simulated logs]\n\n2026-10-20T16:47:20Z  INFO  {} starting\n2026-10-20T16:47:21Z  INFO  configuration loaded\n2026-10-20T16:47:21Z  INFO  health check passed\n2026-10-20T16:47:22Z  INFO  ready to accept connections\n\nLive mode shows the last 250 lines from docker logs.",
                            c.name
                        )
                    } else {
                        format!(
                            "[DEMO · sample details]\n\nName: {}\nImage: {}\nID: {}\nProject: {}\nService: {}\nPorts: {}",
                            c.name, c.image, c.id, c.project, c.service, c.ports
                        )
                    };
                    self.popup = Some(Popup::Text {
                        title,
                        content,
                        scroll: 0,
                        horizontal: 0,
                    });
                } else {
                    let req = if logs {
                        Request::Logs {
                            id: c.id,
                            name: c.name,
                        }
                    } else {
                        Request::Inspect {
                            id: c.id,
                            name: c.name,
                        }
                    };
                    if self.send(req) {
                        self.popup = Some(Popup::Loading { title });
                    }
                }
            }
            Control::Refresh => {
                if self.demo {
                    self.tell("DEMO · sample data refreshed", false);
                } else if self.send(Request::Refresh) {
                    self.tell("Refreshing Docker connection…", false);
                }
            }
            Control::Help => self.popup = Some(Popup::Help),
            Control::Cancel => self.popup = None,
            Control::Confirm => {
                if let Some(Popup::Confirm { action, .. }) = self.popup.take() {
                    self.execute(action);
                }
            }
            Control::Quit => self.quit = true,
            Control::Filter => {
                if self.view != View::System {
                    self.searching = true;
                }
            }
            Control::ClearFilter => {
                self.query.clear();
                self.selected = 0;
                self.clamp();
            }
        }
    }
    pub fn navigate(&mut self, delta: isize) {
        if self.view == View::System {
            self.system_scroll = self.system_scroll.saturating_add_signed(delta).min(
                self.host
                    .cores
                    .len()
                    .saturating_add(self.host.disks.len())
                    .saturating_add(12),
            );
            return;
        }
        let count = self.count();
        if count > 0 {
            self.selected = (self.selected as isize + delta).clamp(0, count as isize - 1) as usize;
        }
        self.clamp();
    }
    pub fn key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('q') => {
                    self.quit = true;
                    return;
                }
                KeyCode::Char('c') => {
                    self.searching = false;
                    self.popup = Some(Popup::Interrupt);
                    return;
                }
                KeyCode::Char('l') => {
                    self.control(Control::Refresh);
                    return;
                }
                KeyCode::Char('u') if self.searching => {
                    self.query.clear();
                    self.selected = 0;
                    self.clamp();
                    return;
                }
                _ => {}
            }
        }
        if let Some(popup) = &mut self.popup {
            match (popup, key.code) {
                (_, KeyCode::Esc) => self.popup = None,
                (Popup::Confirm { .. }, KeyCode::Char('y')) => self.control(Control::Confirm),
                (Popup::Confirm { .. }, KeyCode::Char('n')) => self.popup = None,
                (Popup::Confirm { accept, .. }, KeyCode::Tab | KeyCode::Left | KeyCode::Right) => {
                    *accept = !*accept
                }
                (Popup::Confirm { accept, .. }, KeyCode::Enter) => {
                    if *accept {
                        self.control(Control::Confirm);
                    } else {
                        self.popup = None;
                    }
                }
                (
                    Popup::Text {
                        scroll, content, ..
                    },
                    KeyCode::Down | KeyCode::Char('j'),
                ) => {
                    *scroll = scroll.saturating_add(1).min(
                        content
                            .lines()
                            .count()
                            .saturating_sub(1)
                            .min(u16::MAX as usize) as u16,
                    )
                }
                (Popup::Text { scroll, .. }, KeyCode::Up | KeyCode::Char('k')) => {
                    *scroll = scroll.saturating_sub(1)
                }
                (
                    Popup::Text {
                        scroll, content, ..
                    },
                    KeyCode::PageDown,
                ) => {
                    *scroll = scroll.saturating_add(12).min(
                        content
                            .lines()
                            .count()
                            .saturating_sub(1)
                            .min(u16::MAX as usize) as u16,
                    )
                }
                (Popup::Text { scroll, .. }, KeyCode::PageUp) => {
                    *scroll = scroll.saturating_sub(12)
                }
                (Popup::Text { horizontal, .. }, KeyCode::Left | KeyCode::Char('h')) => {
                    *horizontal = horizontal.saturating_sub(4)
                }
                (
                    Popup::Text {
                        horizontal,
                        content,
                        ..
                    },
                    KeyCode::Right | KeyCode::Char('l'),
                ) => {
                    *horizontal = horizontal.saturating_add(4).min(
                        content
                            .lines()
                            .map(|s| s.chars().count())
                            .max()
                            .unwrap_or(0)
                            .min(u16::MAX as usize) as u16,
                    )
                }
                (Popup::Text { scroll, .. }, KeyCode::Home) => *scroll = 0,
                (
                    Popup::Text {
                        scroll, content, ..
                    },
                    KeyCode::End,
                ) => {
                    *scroll = content
                        .lines()
                        .count()
                        .saturating_sub(12)
                        .min(u16::MAX as usize) as u16
                }
                (Popup::Help | Popup::Interrupt, KeyCode::Enter | KeyCode::Char('?')) => {
                    self.popup = None
                }
                _ => {}
            }
            return;
        }
        if self.searching {
            match key.code {
                KeyCode::Esc | KeyCode::Enter => self.searching = false,
                KeyCode::Backspace => {
                    self.query.pop();
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.query.push(c)
                }
                _ => {}
            }
            self.selected = 0;
            self.clamp();
            return;
        }
        match key.code {
            KeyCode::Char('1') => self.control(Control::View(View::Containers)),
            KeyCode::Char('2') => self.control(Control::View(View::Compose)),
            KeyCode::Char('3') => self.control(Control::View(View::Ports)),
            KeyCode::Char('4') => self.control(Control::View(View::System)),
            KeyCode::Tab => self.control(Control::View(View::ALL[(self.view.index() + 1) % 4])),
            KeyCode::BackTab => self.control(Control::View(View::ALL[(self.view.index() + 3) % 4])),
            KeyCode::Down | KeyCode::Char('j') => self.navigate(1),
            KeyCode::Up | KeyCode::Char('k') => self.navigate(-1),
            KeyCode::PageDown => self.navigate(10),
            KeyCode::PageUp => self.navigate(-10),
            KeyCode::Home | KeyCode::Char('g') => {
                self.selected = 0;
                self.system_scroll = 0;
                self.clamp();
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.selected = self.count().saturating_sub(1);
                self.system_scroll = self.host.cores.len();
                self.clamp();
            }
            KeyCode::Char('s') => self.control(Control::Start),
            KeyCode::Char('x') => self.control(Control::Stop),
            KeyCode::Char('r') => self.control(Control::Restart),
            KeyCode::Char('a') => self.control(Control::Attach),
            KeyCode::Char('u') => self.control(Control::Up),
            KeyCode::Char('l') => self.control(Control::Logs),
            KeyCode::Char('i') | KeyCode::Enter => self.control(Control::Inspect),
            KeyCode::Char('R') => self.control(Control::Refresh),
            KeyCode::Char('/') => self.control(Control::Filter),
            KeyCode::Esc => self.control(Control::ClearFilter),
            KeyCode::F(1) | KeyCode::Char('?') => self.control(Control::Help),
            KeyCode::Char('q') => self.popup = Some(Popup::Interrupt),
            _ => {}
        }
    }
    pub fn click(&mut self, x: u16, y: u16) {
        let hit = self
            .hits
            .iter()
            .rev()
            .find(|(rect, _)| rect.contains((x, y).into()))
            .map(|(_, control)| control.clone());
        if let Some(control) = hit {
            self.control(control);
        }
    }
}

pub fn percent(used: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        used as f64 / total as f64 * 100.0
    }
}
pub fn push(history: &mut VecDeque<u64>, value: u64) {
    if history.len() >= 60 {
        history.pop_front();
    }
    history.push_back(value);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    #[test]
    fn stop_requires_confirmation_and_defaults_to_cancel() {
        let mut app = App::new(true, None);
        app.key(key(KeyCode::Char('x')));
        assert!(matches!(
            app.popup,
            Some(Popup::Confirm { accept: false, .. })
        ));
        app.key(key(KeyCode::Enter));
        assert!(app.docker.containers[0].running());
        app.key(key(KeyCode::Char('x')));
        app.key(key(KeyCode::Char('y')));
        assert!(!app.docker.containers[0].running());
    }
    #[test]
    fn control_c_warns_and_control_q_quits_even_inside_search_or_popup() {
        let mut app = App::new(true, None);
        app.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(!app.quit);
        assert!(matches!(app.popup, Some(Popup::Interrupt)));
        app.key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL));
        assert!(app.quit);
    }
    #[test]
    fn filter_is_keyboard_local_and_selection_survives_refresh() {
        let mut app = App::new(true, None);
        app.key(key(KeyCode::Char('/')));
        for c in "redis".chars() {
            app.key(key(KeyCode::Char(c)));
        }
        assert_eq!(app.count(), 1);
        assert_eq!(app.selected_container().unwrap().name, "redis");
        app.key(key(KeyCode::Enter));
        let mut snap = app.docker.clone();
        snap.containers.reverse();
        app.update(Update::Docker(snap));
        assert_eq!(app.selected_container().unwrap().name, "redis");
        assert!(!app.busy);
    }
    #[test]
    fn attach_requires_a_running_container_and_respects_input_focus() {
        let mut app = App::new(true, None);
        app.key(key(KeyCode::Char('a')));
        assert!(matches!(app.popup, Some(Popup::Text { .. })));
        assert!(app.pending_attach.is_none());
        app.popup = None;
        app.demo = false;
        app.control(Control::Attach);
        assert_eq!(
            app.pending_attach.take().unwrap().id,
            app.selected_container().unwrap().id
        );
        app.busy = true;
        app.control(Control::Attach);
        assert!(app.pending_attach.is_none());
        app.busy = false;
        app.docker.containers[0].state = "exited".into();
        app.control(Control::Attach);
        assert!(app.pending_attach.is_none());
        app.key(key(KeyCode::Char('/')));
        app.key(key(KeyCode::Char('a')));
        assert_eq!(app.query, "a");
        assert!(app.pending_attach.is_none());
    }
}
