use std::collections::BTreeMap;

#[derive(Clone, Debug, Default)]
pub struct Container {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub status: String,
    pub ports: String,
    pub project: String,
    pub service: String,
    pub config_files: String,
    pub working_dir: String,
    pub cpu: Option<f64>,
    pub memory: String,
    pub memory_percent: Option<f64>,
    pub net: String,
    pub io: String,
    pub pids: String,
}

impl Container {
    pub fn running(&self) -> bool {
        self.state == "running"
    }
}

#[derive(Clone, Debug, Default)]
pub struct Project {
    pub name: String,
    pub config_files: Vec<String>,
    pub working_dir: String,
    pub services: usize,
    pub running: usize,
    pub cpu: f64,
    pub memory_percent: f64,
}

pub fn projects(containers: &[Container], configured: Option<&Project>) -> Vec<Project> {
    let mut groups = BTreeMap::<String, Project>::new();
    for c in containers.iter().filter(|c| !c.project.is_empty()) {
        let p = groups.entry(c.project.clone()).or_insert_with(|| Project {
            name: c.project.clone(),
            config_files: c
                .config_files
                .split(',')
                .filter(|f| !f.is_empty())
                .map(String::from)
                .collect(),
            working_dir: c.working_dir.clone(),
            ..Project::default()
        });
        p.services += 1;
        p.running += usize::from(c.running());
        p.cpu += c.cpu.unwrap_or_default();
        p.memory_percent += c.memory_percent.unwrap_or_default();
    }
    if let Some(config) = configured {
        let p = groups
            .entry(config.name.clone())
            .or_insert_with(|| config.clone());
        p.config_files = config.config_files.clone();
        p.working_dir = config.working_dir.clone();
        p.services = p.services.max(config.services);
    }
    groups.into_values().collect()
}

#[derive(Clone, Debug, Default)]
pub struct Port {
    pub protocol: String,
    pub address: String,
    pub port: String,
    pub process: String,
    pub pid: String,
    pub source: String,
}

#[derive(Clone, Debug, Default)]
pub struct Disk {
    pub mount: String,
    pub used: u64,
    pub total: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Host {
    pub name: String,
    pub os: String,
    pub kernel: String,
    pub arch: String,
    pub cpu_name: String,
    pub cores: Vec<f32>,
    pub cpu: f32,
    pub memory_used: u64,
    pub memory_total: u64,
    pub swap_used: u64,
    pub swap_total: u64,
    pub uptime: u64,
    pub load: [f64; 3],
    pub disks: Vec<Disk>,
    pub rx: u64,
    pub tx: u64,
    pub network_sampled: bool,
    pub ports: Vec<Port>,
    pub ports_error: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct DockerSnapshot {
    pub containers: Vec<Container>,
    pub engine: String,
    pub context: String,
    pub error: Option<String>,
    pub stats_error: Option<String>,
    pub configured: Option<Project>,
    pub compose_error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verb {
    Start,
    Stop,
    Restart,
    Up,
}
impl Verb {
    pub fn label(self) -> &'static str {
        match self {
            Self::Start => "Start",
            Self::Stop => "Stop",
            Self::Restart => "Restart",
            Self::Up => "Bring up",
        }
    }
    pub fn command(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
            Self::Up => "up",
        }
    }
}

#[derive(Clone, Debug)]
pub enum Target {
    Container { id: String, name: String },
    Compose(Project),
}
impl Target {
    pub fn name(&self) -> &str {
        match self {
            Self::Container { name, .. } => name,
            Self::Compose(p) => &p.name,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Action {
    pub verb: Verb,
    pub target: Target,
}

#[derive(Clone, Debug)]
pub enum Request {
    Refresh,
    Action(Action),
    Logs { id: String, name: String },
    Inspect { id: String, name: String },
}

pub enum Update {
    Docker(DockerSnapshot),
    Host(Host),
    ActionDone(Result<String, String>),
    Text {
        title: String,
        content: Result<String, String>,
    },
}

pub fn bytes(n: u64) -> String {
    let units = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", units[i])
    }
}

pub fn uptime(s: u64) -> String {
    format!("{}d {:02}h {:02}m", s / 86400, s / 3600 % 24, s / 60 % 60)
}

pub fn demo() -> (DockerSnapshot, Host) {
    let rows = [
        (
            "gateway",
            "nginx:1.27-alpine",
            "running",
            "edge",
            "0.0.0.0:8080->80/tcp",
            0.7,
            "18.4 MiB / 8 GiB",
            0.2,
        ),
        (
            "api",
            "ghcr.io/acme/api:latest",
            "running",
            "workbench",
            "127.0.0.1:3000->3000/tcp",
            12.4,
            "246.8 MiB / 8 GiB",
            3.0,
        ),
        (
            "postgres",
            "postgres:17-alpine",
            "running",
            "workbench",
            "127.0.0.1:5432->5432/tcp",
            2.1,
            "128.2 MiB / 8 GiB",
            1.6,
        ),
        (
            "redis",
            "redis:7-alpine",
            "running",
            "workbench",
            "6379/tcp",
            0.3,
            "12.1 MiB / 8 GiB",
            0.1,
        ),
        (
            "worker",
            "ghcr.io/acme/worker:latest",
            "running",
            "workbench",
            "",
            8.6,
            "186.4 MiB / 8 GiB",
            2.3,
        ),
        (
            "grafana",
            "grafana/grafana:11",
            "exited",
            "observability",
            "",
            0.0,
            "—",
            0.0,
        ),
        (
            "prometheus",
            "prom/prometheus:v3",
            "running",
            "observability",
            "127.0.0.1:9090->9090/tcp",
            1.8,
            "94.2 MiB / 8 GiB",
            1.1,
        ),
        (
            "playground",
            "ubuntu:24.04",
            "exited",
            "",
            "",
            0.0,
            "—",
            0.0,
        ),
    ];
    let containers = rows
        .into_iter()
        .enumerate()
        .map(
            |(i, (name, image, state, project, ports, cpu, memory, mp))| Container {
                id: format!("a9e03b7f{i:04}"),
                name: name.into(),
                image: image.into(),
                state: state.into(),
                status: if state == "running" {
                    "Up 3 hours (healthy)"
                } else {
                    "Exited (0) 2 hours ago"
                }
                .into(),
                ports: ports.into(),
                project: project.into(),
                service: name.into(),
                config_files: format!("/home/dev/{project}/compose.yaml"),
                working_dir: format!("/home/dev/{project}"),
                cpu: (state == "running").then_some(cpu),
                memory: memory.into(),
                memory_percent: (state == "running").then_some(mp),
                net: "2.4 MB / 840 kB".into(),
                io: "0 B / 12.3 MB".into(),
                pids: "12".into(),
            },
        )
        .collect();
    let host = Host {
        name: "studio.local".into(),
        os: "Ubuntu 24.04 LTS".into(),
        kernel: "6.8.0-51-generic".into(),
        arch: "aarch64".into(),
        cpu_name: "Apple M4 · virtualized".into(),
        cores: vec![22., 38., 12., 46., 18., 8., 32., 16.],
        cpu: 24.,
        memory_used: 6_420_000_000,
        memory_total: 17_179_869_184,
        swap_used: 0,
        swap_total: 2_147_483_648,
        uptime: 741_420,
        load: [1.28, 1.46, 1.32],
        disks: vec![Disk {
            mount: "/".into(),
            used: 145_000_000_000,
            total: 500_000_000_000,
        }],
        rx: 247_800,
        tx: 82_400,
        network_sampled: true,
        ports: vec![
            Port {
                protocol: "TCP".into(),
                address: "127.0.0.1".into(),
                port: "5173".into(),
                process: "node".into(),
                pid: "24918".into(),
                source: "host".into(),
            },
            Port {
                protocol: "TCP".into(),
                address: "127.0.0.1".into(),
                port: "3000".into(),
                process: "docker-proxy".into(),
                pid: "1820".into(),
                source: "host".into(),
            },
            Port {
                protocol: "TCP".into(),
                address: "0.0.0.0".into(),
                port: "22".into(),
                process: "sshd".into(),
                pid: "961".into(),
                source: "host".into(),
            },
        ],
        ..Host::default()
    };
    (
        DockerSnapshot {
            containers,
            engine: "27.5.1".into(),
            context: "desktop-linux".into(),
            ..DockerSnapshot::default()
        },
        host,
    )
}
