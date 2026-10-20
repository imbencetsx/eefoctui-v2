use crate::model::*;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::{
    collections::HashMap,
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver, SyncSender},
    thread,
    time::{Duration, Instant},
};
use sysinfo::{Disks, Networks, System};

const OUTPUT_LIMIT: usize = 2 * 1024 * 1024;

fn drain(mut stream: impl Read) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = [0; 8192];
    while let Ok(n) = stream.read(&mut buf) {
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n.min(OUTPUT_LIMIT.saturating_sub(out.len()))]);
    }
    out
}

/// No shell interpolation. Drain both pipes concurrently and bound every process.
pub fn command(program: &str, args: &[String], timeout: Duration) -> Result<String> {
    run_command(program, args, timeout).map(|(stdout, _)| stdout)
}

fn run_command(program: &str, args: &[String], timeout: Duration) -> Result<(String, String)> {
    let executable = if program == "docker" {
        docker_executable()
    } else {
        PathBuf::from(program)
    };
    let mut cmd = Command::new(executable);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd
        .spawn()
        .with_context(|| format!("Cannot run {program}. Is it installed and on PATH?"))?;
    let stdout = child.stdout.take().context("Missing stdout")?;
    let stderr = child.stderr.take().context("Missing stderr")?;
    let out = thread::spawn(move || drain(stdout));
    let err = thread::spawn(move || drain(stderr));
    let start = Instant::now();
    let mut expired = false;
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        if start.elapsed() > timeout {
            expired = true;
            #[cfg(unix)]
            {
                // The process group includes Docker CLI plugins, so a hung plugin
                // cannot keep the output pipes (or worker) alive after timeout.
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
            }
            let _ = child.kill();
            break child.wait()?;
        }
        thread::sleep(Duration::from_millis(30));
    };
    let out = String::from_utf8_lossy(&out.join().unwrap_or_default()).into_owned();
    let err = String::from_utf8_lossy(&err.join().unwrap_or_default())
        .trim()
        .to_owned();
    if expired {
        bail!(
            "{program} timed out after {}s; refresh to check the final state",
            timeout.as_secs()
        );
    }
    if !status.success() {
        bail!(
            "{}",
            if err.is_empty() {
                format!("{program} exited with {status}")
            } else {
                err
            }
        );
    }
    Ok((out, err))
}

fn docker_executable() -> PathBuf {
    // An explicitly installed CLI on PATH always wins (including remote-engine
    // wrappers). Desktop's bundled CLI also works before PATH setup is finished.
    if cfg!(target_os = "macos") {
        let on_path = std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|dir| dir.join("docker").is_file())
        });
        if !on_path {
            let mut candidates = vec![PathBuf::from(
                "/Applications/Docker.app/Contents/Resources/bin/docker",
            )];
            if let Some(home) = std::env::var_os("HOME") {
                candidates.push(
                    PathBuf::from(home)
                        .join("Applications/Docker.app/Contents/Resources/bin/docker"),
                );
            }
            if let Some(path) = candidates.into_iter().find(|p| p.is_file()) {
                return path;
            }
        }
    }
    PathBuf::from("docker")
}

fn docker(args: &[&str]) -> Result<String> {
    command(
        "docker",
        &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        Duration::from_secs(12),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DockerRow {
    #[serde(rename = "ID")]
    id: String,
    names: String,
    image: String,
    state: String,
    status: String,
    ports: String,
    #[serde(default)]
    project: String,
    #[serde(default)]
    service: String,
    #[serde(default)]
    config_files: String,
    #[serde(default)]
    working_dir: String,
}

const PS_FORMAT: &str = r#"{"ID":{{json .ID}},"Names":{{json .Names}},"Image":{{json .Image}},"State":{{json .State}},"Status":{{json .Status}},"Ports":{{json .Ports}},"Project":{{json (.Label "com.docker.compose.project")}},"Service":{{json (.Label "com.docker.compose.service")}},"ConfigFiles":{{json (.Label "com.docker.compose.project.config_files")}},"WorkingDir":{{json (.Label "com.docker.compose.project.working_dir")}}"#;

pub fn parse_containers(output: &str) -> Result<Vec<Container>> {
    let mut containers = Vec::new();
    for line in output.lines().filter(|s| !s.trim().is_empty()) {
        let row: DockerRow = serde_json::from_str(line).context("Unexpected docker ps output")?;
        containers.push(Container {
            id: row.id,
            name: row.names,
            image: row.image,
            state: row.state,
            status: row.status,
            ports: row.ports,
            project: row.project,
            service: row.service,
            config_files: row.config_files,
            working_dir: row.working_dir,
            memory: "—".into(),
            ..Container::default()
        });
    }
    containers.sort_by(|a, b| b.running().cmp(&a.running()).then(a.name.cmp(&b.name)));
    Ok(containers)
}

pub fn apply_stats(containers: &mut [Container], output: &str) -> Result<()> {
    let mut rows = HashMap::new();
    for line in output.lines().filter(|s| !s.trim().is_empty()) {
        let v: serde_json::Value =
            serde_json::from_str(line).context("Unexpected docker stats output")?;
        if let Some(name) = v["Name"].as_str() {
            rows.insert(name.to_owned(), v);
        }
    }
    for c in containers {
        if let Some(v) = rows.get(&c.name) {
            let field = |key: &str| v[key].as_str().unwrap_or("—").to_owned();
            c.cpu = field("CPUPerc").trim_end_matches('%').parse().ok();
            c.memory = field("MemUsage");
            c.memory_percent = field("MemPerc").trim_end_matches('%').parse().ok();
            c.net = field("NetIO");
            c.io = field("BlockIO");
            c.pids = field("PIDs");
        }
    }
    Ok(())
}

fn configured_project(path: &std::path::Path) -> Result<Project> {
    let path = path.to_string_lossy().into_owned();
    let output = docker(&["compose", "-f", &path, "config", "--format", "json"])?;
    let v: serde_json::Value = serde_json::from_str(&output)?;
    let name = v["name"]
        .as_str()
        .context("Compose config did not report a project name")?
        .to_owned();
    Ok(Project {
        name,
        config_files: vec![path.clone()],
        working_dir: PathBuf::from(&path)
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_string_lossy()
            .into_owned(),
        services: v["services"].as_object().map_or(0, |s| s.len()),
        ..Project::default()
    })
}

fn snapshot(
    compose: Option<&std::path::Path>,
    engine: &mut String,
    context: &mut String,
) -> DockerSnapshot {
    let mut snap = DockerSnapshot::default();
    match docker(&["ps", "--all", "--no-trunc", "--format", PS_FORMAT])
        .and_then(|s| parse_containers(&s))
    {
        Ok(containers) => snap.containers = containers,
        Err(e) => {
            snap.error = Some(format!("{e:#}"));
            return snap;
        }
    }
    if engine.is_empty() {
        *engine = docker(&["version", "--format", "{{.Server.Version}} "])
            .unwrap_or_default()
            .trim()
            .to_owned();
    }
    if context.is_empty() {
        *context = docker(&["context", "show"])
            .unwrap_or_default()
            .trim()
            .to_owned();
    }
    snap.engine = engine.clone();
    snap.context = context.clone();
    if snap.containers.iter().any(Container::running)
        && let Err(e) = docker(&["stats", "--no-stream", "--format", "{{json .}}"])
            .and_then(|output| apply_stats(&mut snap.containers, &output))
    {
        snap.stats_error = Some(format!("{e:#}"));
    }
    if let Some(path) = compose {
        match configured_project(path) {
            Ok(p) => snap.configured = Some(p),
            Err(e) => snap.compose_error = Some(format!("{e:#}")),
        }
    }
    snap
}

pub fn action_args(action: &Action) -> Result<Vec<String>> {
    let mut args = Vec::<String>::new();
    match &action.target {
        Target::Container { id, .. } => {
            if action.verb == Verb::Up {
                bail!("Bring up is a Compose-only action");
            }
            args.extend([action.verb.command().into(), "--".into(), id.clone()]);
        }
        Target::Compose(project) => {
            if project.config_files.is_empty() {
                bail!(
                    "No Compose files were recorded for this project. Launch efc --compose /path/to/compose.yaml"
                );
            }
            args.extend([
                "compose".into(),
                "--project-name".into(),
                project.name.clone(),
            ]);
            if !project.working_dir.is_empty() {
                args.extend(["--project-directory".into(), project.working_dir.clone()]);
            }
            for file in &project.config_files {
                args.extend(["-f".into(), file.clone()]);
            }
            args.push(action.verb.command().into());
            if action.verb == Verb::Up {
                args.extend(["--detach".into(), "--no-recreate".into()]);
            }
        }
    }
    Ok(args)
}

pub fn spawn(compose: Option<PathBuf>) -> (SyncSender<Request>, Receiver<Update>) {
    let (tx, rx) = mpsc::channel();
    let (request_tx, request_rx) = mpsc::sync_channel(8);
    let docker_tx = tx.clone();
    thread::spawn(move || {
        let mut engine = String::new();
        let mut context = String::new();
        loop {
            if docker_tx
                .send(Update::Docker(snapshot(
                    compose.as_deref(),
                    &mut engine,
                    &mut context,
                )))
                .is_err()
            {
                break;
            }
            let until = Instant::now() + Duration::from_secs(3);
            loop {
                match request_rx.recv_timeout(until.saturating_duration_since(Instant::now())) {
                    Ok(Request::Refresh) => break,
                    Ok(Request::Action(action)) => {
                        let result = action_args(&action)
                            .and_then(|args| command("docker", &args, Duration::from_secs(120)))
                            .map(|_| {
                                format!(
                                    "{} completed · {}",
                                    action.verb.label(),
                                    action.target.name()
                                )
                            })
                            .map_err(|e| format!("{e:#}"));
                        let _ = docker_tx.send(Update::ActionDone(result));
                        break;
                    }
                    Ok(Request::Logs { id, name }) => {
                        let args = ["logs", "--tail", "250", "--timestamps", &id]
                            .into_iter()
                            .map(String::from)
                            .collect::<Vec<_>>();
                        let content = run_command("docker", &args, Duration::from_secs(12))
                            .map(|(stdout, stderr)| {
                                if stderr.is_empty() {
                                    stdout
                                } else {
                                    format!("{stdout}\n── stderr ──\n{stderr}")
                                }
                            })
                            .map_err(|e| format!("{e:#}"));
                        let _ = docker_tx.send(Update::Text {
                            title: format!("Logs · {name}"),
                            content,
                        });
                    }
                    Ok(Request::Inspect { id, name }) => {
                        let content = docker(&["inspect", &id])
                            .and_then(|s| {
                                Ok(serde_json::to_string_pretty(&serde_json::from_str::<
                                    serde_json::Value,
                                >(
                                    &s
                                )?)?)
                            })
                            .map_err(|e| format!("{e:#}"));
                        let _ = docker_tx.send(Update::Text {
                            title: format!("Inspect · {name}"),
                            content,
                        });
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
        }
    });
    thread::spawn(move || {
        let mut system = System::new();
        let mut disks = Disks::new_with_refreshed_list();
        let mut networks = Networks::new_with_refreshed_list();
        let mut ports = Vec::new();
        let mut ports_error = None;
        let mut tick = 0u64;
        loop {
            let start = Instant::now();
            system.refresh_cpu_usage();
            system.refresh_memory();
            if tick.is_multiple_of(5) {
                disks.refresh(true);
                match host_ports() {
                    Ok(p) => {
                        ports = p;
                        ports_error = None;
                    }
                    Err(e) => {
                        ports_error = Some(format!("{e:#}"));
                        ports.clear();
                    }
                }
            }
            networks.refresh(true);
            let load = System::load_average();
            let host = Host {
                name: System::host_name().unwrap_or_else(|| "localhost".into()),
                os: System::long_os_version().unwrap_or_else(|| std::env::consts::OS.into()),
                kernel: System::kernel_version().unwrap_or_default(),
                arch: System::cpu_arch(),
                cpu_name: system
                    .cpus()
                    .first()
                    .map(|c| c.brand().to_owned())
                    .unwrap_or_default(),
                cores: system.cpus().iter().map(|c| c.cpu_usage()).collect(),
                cpu: system.global_cpu_usage(),
                memory_used: system.used_memory(),
                memory_total: system.total_memory(),
                swap_used: system.used_swap(),
                swap_total: system.total_swap(),
                uptime: System::uptime(),
                load: [load.one, load.five, load.fifteen],
                disks: disks
                    .iter()
                    .map(|d| Disk {
                        mount: d.mount_point().display().to_string(),
                        used: d.total_space().saturating_sub(d.available_space()),
                        total: d.total_space(),
                    })
                    .collect(),
                rx: networks
                    .iter()
                    .filter(|(name, _)| !name.starts_with("lo"))
                    .map(|(_, n)| n.received())
                    .sum(),
                tx: networks
                    .iter()
                    .filter(|(name, _)| !name.starts_with("lo"))
                    .map(|(_, n)| n.transmitted())
                    .sum(),
                network_sampled: tick > 0,
                ports: ports.clone(),
                ports_error: ports_error.clone(),
            };
            if tx.send(Update::Host(host)).is_err() {
                break;
            }
            tick += 1;
            thread::sleep(Duration::from_secs(1).saturating_sub(start.elapsed()));
        }
    });
    (request_tx, rx)
}

pub fn split_endpoint(endpoint: &str) -> Option<(String, String)> {
    let (address, port) = endpoint.rsplit_once(':')?;
    if port.parse::<u16>().is_err() {
        return None;
    }
    Some((address.trim_matches(['[', ']']).to_owned(), port.to_owned()))
}

pub fn parse_lsof(output: &str, protocol: &str) -> Vec<Port> {
    let mut ports = Vec::new();
    let mut pid = String::new();
    let mut process = String::new();
    for line in output.lines() {
        if let Some(v) = line.strip_prefix('p') {
            pid = v.into();
        } else if let Some(v) = line.strip_prefix('c') {
            process = v.into();
        } else if let Some(v) = line.strip_prefix('n') {
            if v.contains("->") {
                continue;
            }
            if let Some((address, port)) = split_endpoint(v) {
                ports.push(Port {
                    protocol: protocol.into(),
                    address,
                    port,
                    process: process.clone(),
                    pid: pid.clone(),
                    source: "host".into(),
                });
            }
        }
    }
    ports
}

fn parse_ss(output: &str) -> Vec<Port> {
    let mut ports = Vec::new();
    for line in output.lines() {
        let cols: Vec<_> = line.split_whitespace().collect();
        if cols.len() < 6 {
            continue;
        }
        if let Some((address, port)) = split_endpoint(cols[4]) {
            let owner = cols.get(6..).unwrap_or_default().join(" ");
            let process = owner
                .split('"')
                .nth(1)
                .unwrap_or("permission restricted")
                .to_owned();
            let pid = owner
                .split("pid=")
                .nth(1)
                .and_then(|s| s.split(',').next())
                .unwrap_or("—")
                .to_owned();
            ports.push(Port {
                protocol: cols[0].to_uppercase(),
                address,
                port,
                process,
                pid,
                source: "host".into(),
            });
        }
    }
    ports
}

fn host_ports() -> Result<Vec<Port>> {
    if cfg!(target_os = "linux")
        && let Ok(output) = command(
            "ss",
            &["-H".into(), "-lntup".into()],
            Duration::from_secs(3),
        )
    {
        return Ok(parse_ss(&output));
    }
    let mut ports = Vec::new();
    for (protocol, flags) in [
        ("TCP", vec!["-nP", "-iTCP", "-sTCP:LISTEN", "-Fpcn"]),
        ("UDP", vec!["-nP", "-iUDP", "-Fpcn"]),
    ] {
        // lsof exits 1 (and prints nothing) when there are no matching sockets.
        match command(
            "lsof",
            &flags.into_iter().map(String::from).collect::<Vec<_>>(),
            Duration::from_secs(3),
        ) {
            Ok(out) => ports.extend(parse_lsof(&out, protocol)),
            Err(e) if e.to_string().contains("exit status: 1") => {}
            Err(e) => return Err(e),
        }
    }
    ports.sort_by_key(|p| {
        (
            p.port.parse::<u16>().unwrap_or(0),
            p.protocol.clone(),
            p.pid.clone(),
        )
    });
    ports.dedup_by(|a, b| {
        a.protocol == b.protocol && a.address == b.address && a.port == b.port && a.pid == b.pid
    });
    Ok(ports)
}

pub fn published_ports(containers: &[Container]) -> Vec<Port> {
    let mut ports = Vec::new();
    for c in containers.iter().filter(|c| c.running()) {
        for binding in c.ports.split(", ") {
            let Some((host, target)) = binding.split_once("->") else {
                continue;
            };
            if let Some((address, port)) = split_endpoint(host) {
                let protocol = target
                    .rsplit_once('/')
                    .map_or("TCP", |(_, p)| p)
                    .to_uppercase();
                ports.push(Port {
                    protocol,
                    address,
                    port,
                    process: format!("{} → {target}", c.name),
                    pid: "—".into(),
                    source: "docker".into(),
                });
            }
        }
    }
    ports
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn docker_json_and_stats_handle_missing_samples_and_multicore_cpu() {
        let mut c = parse_containers(r#"{"ID":"abc","Names":"api","Image":"api:v1","State":"running","Status":"Up","Ports":"127.0.0.1:3000->80/tcp","Project":"dev","Service":"api","ConfigFiles":"/tmp/a.yaml,/tmp/b.yaml","WorkingDir":"/tmp"}"#).unwrap();
        assert!(c[0].cpu.is_none());
        apply_stats(&mut c, r#"{"Name":"api","CPUPerc":"201.25%","MemUsage":"10MiB / 1GiB","MemPerc":"0.98%","NetIO":"1kB / 2kB","BlockIO":"0B / 0B","PIDs":"3"}"#).unwrap();
        assert_eq!(c[0].cpu, Some(201.25));
        assert_eq!(projects(&c, None)[0].config_files.len(), 2);
        assert!(parse_containers("not json").is_err());
    }
    #[test]
    fn compose_preserves_multiple_files_and_paths_as_arguments() {
        let action = Action {
            verb: Verb::Up,
            target: Target::Compose(Project {
                name: "my-project".into(),
                config_files: vec![
                    "/tmp/my app/compose.yaml".into(),
                    "/tmp/override.yaml".into(),
                ],
                working_dir: "/tmp/my app".into(),
                ..Project::default()
            }),
        };
        assert_eq!(
            action_args(&action).unwrap(),
            [
                "compose",
                "--project-name",
                "my-project",
                "--project-directory",
                "/tmp/my app",
                "-f",
                "/tmp/my app/compose.yaml",
                "-f",
                "/tmp/override.yaml",
                "up",
                "--detach",
                "--no-recreate"
            ]
        );
    }
    #[test]
    fn ports_handle_ipv6_and_ignore_outbound_udp() {
        let ports = parse_lsof(
            "p42\ncnode\nn[::1]:5173\nn127.0.0.1:5353->8.8.8.8:53\n",
            "UDP",
        );
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].port, "5173");
        assert_eq!(ports[0].address, "::1");
        let ports = parse_ss("tcp LISTEN 0 128 [::]:8080 [::]:* users:((\"node\",pid=42,fd=9))");
        assert_eq!(ports[0].pid, "42");
        assert_eq!(ports[0].process, "node");
        assert_eq!(published_ports(&demo().0.containers).len(), 4);
    }
    #[test]
    fn subprocess_failures_and_timeouts_are_reported() {
        assert!(command("efc-no-such-command", &[], Duration::from_secs(1)).is_err());
        #[cfg(unix)]
        {
            let start = Instant::now();
            let err = command(
                "sh",
                &["-c".into(), "sleep 5".into()],
                Duration::from_millis(80),
            )
            .unwrap_err();
            assert!(err.to_string().contains("timed out"));
            assert!(start.elapsed() < Duration::from_secs(2));
        }
    }
}
