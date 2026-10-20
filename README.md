# eefoctuiv2

**`efc` — a fullscreen Docker & Compose control room, built in Rust.**

Pure black surfaces with glacier blue and burgundy accents sampled from the
reference palette. Segmented green/yellow CPU and burgundy/rose memory meters
bring a restrained btop feel. Solid keycaps keep their beveled press feedback,
with two columns between buttons and a full blank row beneath each button bar.
Keyboard first, with clickable tabs, rows, buttons, and wheel scrolling.
The Containers summary shows one gradient-shaded square per container: filled
for running, outlined for inactive, with an exact +N overflow count when needed.
No special icon font required.

## Install & run

Requires Rust **1.95+**. Supported host monitoring platforms: **macOS and Linux**.
For container controls, install the Docker CLI, run your engine, and install the
**Docker Compose v2 plugin** (`docker compose`). Use a terminal with truecolor and
Unicode support for the best appearance.

```sh
cargo install --path . --locked
efc
```

Cargo installs `efc` in `~/.cargo/bin`; put that directory on your `PATH` if needed.
From the project, you can also use `cargo run --release --`.

```sh
efc --demo                          # Simulated environment, no Docker needed
efc --compose ./compose.yaml         # Load a stack, including an uncreated one
efc --view ports                     # Open directly to port inspection
efc --view system --no-mouse         # Host monitor; allow native text selection
```

The app fills the terminal's alternate screen and restores it on exit. Its minimum
size is 58 columns × 18 rows; 120 × 36 or larger shows the selected-container
inspector, and taller layouts include session activity. `Ctrl+C` opens a reminder
to use `Ctrl+Q`; exiting efc leaves containers running.

## What you can do

- **Containers:** start, gracefully stop, and restart; inspect state, image, Compose
  service, ports, CPU, memory, network I/O, block I/O, process count, and CPU history.
  Open the last 250 log lines (both stdout and stderr) or the complete inspect JSON.
- **Compose:** automatically discover projects through labels on all containers,
  including stopped ones. Start, stop, or restart a whole project; bring up missing
  services using `up --detach --no-recreate`. Discovered multi-file configurations
  and project working directories are preserved as separate command arguments.
- **Ports:** inspect local TCP listeners and unconnected UDP bindings, including
  address, process and PID. Docker-published port mappings appear with their own
  source label. Filter by port, protocol, address, process, PID or container.
- **System:** host CPU and per-core usage, RAM, swap, mounted filesystem usage,
  network bytes per sample, OS, kernel, architecture, CPU model, uptime and load.
- **Session activity:** connection changes, detected state changes, dispatched
  actions and results. This is an in-memory session log.

Stops, restarts, and Compose up require confirmation; **Cancel is selected by
default**. Commands run in a background worker with timeouts, bounded output, and
no shell interpolation. Host sampling runs independently from Docker commands.

## Keys

| Key | Action |
| --- | --- |
| `1` / `2` / `3` / `4` | Containers / Compose / Ports / System |
| `Tab` / `Shift+Tab` | Next / previous view |
| `↑` / `↓`, `k` / `j` | Select a row or scroll |
| `PgUp` / `PgDn` | Jump 10 rows; scroll popup by 12 lines |
| `Home` / `End`, `g` / `G` | First / last row |
| `/` | Filter; `Enter` applies, `Esc` leaves input |
| `Esc` | Close popup, or clear the current filter |
| `Ctrl+U` | Clear text while filtering |
| `s` / `x` / `r` | Start / stop / restart the selection |
| `u` | Bring up the selected Compose project |
| `l` | Container logs |
| `i` / `Enter` | Container inspect JSON |
| `←` / `→`, `h` / `l` in logs/inspect | Pan horizontally |
| `Shift+R` / `Ctrl+L` | Refresh Docker |
| `?` / `F1` | Help |
| `Ctrl+C` / `q` | Show exit reminder |
| `Ctrl+Q` | Quit from anywhere |

In confirmation dialogs, use `Tab` or arrows to choose, `Enter` to select,
`y` to confirm, or `n` / `Esc` to cancel. The mouse can also click either button.

## Data & platform details

`efc` uses the Docker CLI, respecting the current context, `DOCKER_HOST`, and your
existing Docker authentication. On macOS, if `docker` is absent from `PATH`,
efc also checks Docker Desktop’s bundled CLI in `/Applications` and `~/Applications`.
This does not start the Docker engine; Docker Desktop must be running. **Docker resources belong to the active engine;
system resources belong to the local host.** On Docker Desktop, container memory
limits refer to the Linux VM. With a remote Docker context, published bindings
refer to the remote engine, not local listeners. Exposed container-only ports in
the Containers view do not imply a host listener.

Docker refreshes roughly every 3 seconds **after** collection completes; stats
collection can add time. CPU history holds the last 60 received samples. Docker
CPU can exceed 100% on multicore workloads. Missing stats show `—`. System metrics
sample approximately every second; sockets and disks refresh every 5 host samples.
The status line displays the age of the last snapshots. Network totals exclude
loopback and report bytes **per sample**, not an assumed fixed-rate measurement.

Linux uses `ss` when available, falling back to `lsof`; macOS uses `lsof`. Process
visibility follows the current user's permissions. UDP entries are bindings, not
proof a service will answer. The app never requests elevated privileges. Local
host monitoring stays available when Docker is missing or disconnected. If the
Desktop socket is missing, open Docker Desktop and wait for its engine to start.
efc reconnects automatically; `Shift+R` retries immediately.

To load an uncreated Compose project, pass `--compose`. Auto-discovery finds stacks
that already have labeled containers. Compose operations require the referenced
files and their environment/secrets to remain available locally. Start only starts
existing containers; **Bring up** can pull images and create missing services.
efc does not remove containers, images, networks, or volumes.

Logs and inspect are on-demand snapshots. Reopen them to refresh. Both log streams
are retained, with stderr under a separate heading; cross-stream order is not
reconstructed. Output is capped at 2 MiB per stream. Collection commands time out
after 12 seconds, host socket commands after 3, and actions after 120. A timed-out
action may already have changed engine state; refresh before retrying. Quitting
does not cancel an in-flight Docker operation. Demo mode is explicitly labeled and
never sends Docker commands.

## Development & checks

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build
uv run --with pyte==0.8.2 python tests/terminal_smoke.py
```

The PTY smoke test runs the actual binary against a temporary Docker CLI fixture,
verifies container and multi-file Compose commands, navigates the screens, checks
both log streams, confirms a real host socket is visible, and verifies terminal
restoration. It never manages real containers. Unit tests cover parsing, timeouts,
confirmation defaults, keyboard behavior, selection stability, and rendering at
six terminal sizes. The fixture does not replace a real-workload start/stop
check. You can verify a live connection and the empty state against `docker ps`.

Capture the **actual Ratatui buffer** as a self-contained HTML preview or text:

```sh
efc --demo --snapshot docs/preview-containers.html
efc --demo --view system --width 120 --height 36 --snapshot docs/preview-system.html
efc --demo --snapshot /tmp/efc.txt
```

## Stack

Rust · Ratatui · Crossterm · sysinfo · Docker CLI · Compose v2

MIT license.
