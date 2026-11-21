# PowerWatch

[![CI](https://github.com/lnpotter/PowerWatch/actions/workflows/ci.yml/badge.svg)](https://github.com/lnpotter/PowerWatch/actions/workflows/ci.yml)

A lightweight, cross-platform tool for measuring and estimating power consumption of PC components in real-time. Provides three interfaces: a CLI for scripts and one-off checks, a terminal dashboard for live monitoring, and a browser-based dashboard with charts and history.

## Overview

PowerWatch reports real or estimated power draw (in watts) for each major component: CPU, GPU (NVIDIA/AMD), RAM, and disk. Every reading is tagged as either `(measured)` or `(estimated)`, and the total reflects the least-trustworthy input, so you always know what you can rely on.

Linux is the primary, fully tested platform. Windows and macOS are supported in code but haven't been validated on real hardware yet.

## What It Measures

| Component | Source | Kind |
|-----------|--------|------|
| CPU | RAPL (`/sys/class/powercap`) | Measured |
| GPU (NVIDIA) | NVML | Measured |
| GPU (AMD) | hwmon (`power1_average`) | Measured |
| RAM | Heuristic (GB used × watts/GB) | Estimated |
| Disk | Idle/active watts by type (NVMe uses drive's own power-state table when `nvme-cli` is available) | Estimated |
| Total | Sum of the above | Mixed (tagged) |

Sensors that aren't available on your machine are silently omitted. Intel iGPUs aren't supported yet.

On Linux, PowerWatch reads the first RAPL package domain, the first AMD GPU and the first physical disk it finds. Virtual block devices (loop, ram, zram, device-mapper, md, nbd) and optical drives are skipped, and an entry it can't read is passed over instead of stopping the search.

On NVMe drives, if `nvme-cli` is installed and the drive is readable, PowerWatch uses the manufacturer's declared max power per power state (`nvme id-ctrl`) rather than a generic guess. It falls back to the heuristic automatically when `nvme-cli` isn't available or the drive isn't NVMe.

## Install

You need a recent Rust toolchain (stable, 1.85 or newer). No system libraries are required: SQLite is bundled, and the NVML binding loads dynamically only when the NVIDIA driver is present.

```bash
cargo build --workspace --release
```

Binaries are placed under `target/release/`: `powerwatch` (CLI), `powerwatch-tui` (terminal dashboard), and `powerwatch-web` (browser dashboard). Copy them anywhere on your `PATH` if you like:

```bash
sudo cp target/release/powerwatch target/release/powerwatch-tui target/release/powerwatch-web /usr/local/bin/
```

### CPU readings need a one-time setup

RAPL is root-only by default. Either run the CLI with `sudo`, or install the included udev rule:

```bash
sudo ./scripts/setup-udev.sh
```

See `docs/setup-udev.md` for what the rule does and why the restriction exists.

## Usage

```bash
# One-off snapshot
powerwatch

# JSON output (for scripts or pipes)
powerwatch --json

# Live terminal dashboard, refreshing once a second
powerwatch-tui

# Browser dashboard (local only, no auth)
powerwatch-web               # http://127.0.0.1:3000
powerwatch-web --port 4000   # custom port
powerwatch-web --log         # also write to the history database
```

### History logging

The CLI can record readings to a local SQLite database for later analysis:

```bash
# Log for 60 seconds
powerwatch --log --duration 60s

# Also accepts "5m", "1h", or a bare number of seconds
powerwatch --log --duration 5m
```

History lives at `~/.local/share/powerwatch/history.db`. You can inspect it directly:

```bash
sqlite3 ~/.local/share/powerwatch/history.db "SELECT * FROM readings ORDER BY ts DESC LIMIT 20;"
```

The database runs in WAL mode, so you will also see `history.db-wal` and `history.db-shm` next to it. That is what lets the CLI, the TUI and the web dashboard write to and read from the same file at once.

Readings older than 7 days are deleted when logging starts (and once an hour while the web dashboard keeps logging), so the file stays bounded: with four series sampled once per second it grows by roughly 30 MB per day. Set `POWERWATCH_RETENTION_DAYS` to change the window, for example `POWERWATCH_RETENTION_DAYS=30`, or to `0` to keep everything.

Query averaged readings from a logged session:

```bash
powerwatch history --since 1h
powerwatch history --since 30m
```

### Alerts

Get notified when a component stays above a threshold for a configured duration. Works in both the CLI and TUI:

```bash
# CLI: needs --watch so there's something to sustain
powerwatch --watch --alert-above 50 --alert-for 30s \
  --alert-run "notify-send 'PowerWatch' 'total power is high'"

# TUI: always live, no --watch needed
powerwatch-tui --alert-component cpu --alert-above 20 --alert-for 10s
```

| Flag | Meaning |
|------|---------|
| `--alert-above <watts>` | Enable the alert (required) |
| `--alert-component <name>` | Which sensor to watch (`cpu`, `gpu`, `ram`, `disk`, or `total`) |
| `--alert-for <duration>` | How long the threshold must be sustained (`10s`, `2m`, default: immediate) |
| `--alert-run <command>` | Shell command to run once when the alert fires (`sh -c` on Linux and macOS, `cmd /C` on Windows) |

The alert fires **once per excursion** above the threshold (not once per second) so a hook won't spam you. It can fire again after dropping back below and crossing upward a second time. Without `--alert-run`, the CLI prints `🔔 alert active` each cycle and the TUI shows a red banner.

## Live Dashboard (TUI)

![TUI](docs/images/Image_TUI.png)

```bash
powerwatch-tui
```

Opens a full-screen dashboard updating once per second: the total at top, one block per sensor below (current watts, confidence, a sparkline of the last minute, and min/max), and a footer listing available keys.

| Key | Action |
|-----|--------|
| `q` | Quit |
| `p` | Pause/resume sampling (freezes the last frame, shows `PAUSED`) |
| `l` | Start/stop logging to the history database |
| `s` | Open/close the suggestions panel |

If every sensor fails mid-session, the total line reports the failure directly instead of going quiet.

### Suggestions Panel (TUI)

The TUI can evaluate power-draw patterns and surface energy-saving suggestions. Press `s` to open the panel:

| Key | Action |
|-----|--------|
| `Up` / `Down` (or `k` / `j`) | Move the selection |
| `Enter` | Confirm the selected suggestion's action |
| `y` / `Y` | Apply the action being confirmed |
| `n` / `N` | Cancel the confirmation (the suggestion stays in the list) |
| `d` / `D` | Dismiss the selected suggestion |

Nothing runs on its own: select a suggestion, press `Enter` to open the confirmation (the footer shows the exact command it will run), then `y` to apply it. If the list changes while you are confirming, `y` only applies the suggestion you confirmed, and does nothing if it has already gone away. If the action fails, the reason is shown in the panel footer and the suggestion stays in the list. By default the TUI has two rules, total draw above 15 W or CPU draw above 8 W for 3 seconds (both can be changed, see [Configuration](#configuration)), and both propose switching to the power saver profile. The one-time confirmation token belongs to the web dashboard (see below).

## Web Dashboard

![WEB](docs/images/Image_WEB.png)

```bash
powerwatch-web               # http://127.0.0.1:3000
powerwatch-web --port 4000   # custom port
powerwatch-web --log         # also log continuously to the history database
```

A local HTTP server with a single self-contained HTML page: no separate assets, no CDN, no external JS. Everything is embedded at compile time. The dashboard polls the same live data the CLI and TUI show, plus a chart of the last 10 minutes pulled from the history database:

- **Live table**: current watts and confidence per sensor, refreshed automatically.
- **Chart**: click a legend entry to toggle that series; the y-axis rescales so smaller signals stay visible. Hover over the chart for exact values at a given point.
- **Suggestions**: energy-saving proposals appear in a dedicated section, each with a confirmation modal.
- **Process monitor**: when CPU draw stays high, a "Show top processes" suggestion opens a modal displaying the five most CPU-intensive processes, refreshing every 2 seconds.
- **--log** makes the server write to `~/.local/share/powerwatch/history.db` continuously, the same database the CLI's `--log`/`history` and the TUI's `l` key use. Without `--log`, the chart only shows what something else logged.

API endpoints:

```
GET  /api/snapshot            # latest power readings as JSON
GET  /api/history?since=10m   # readings since a given timestamp
GET  /api/suggestions         # active energy-saving suggestions
POST /api/suggestions/apply   # apply an action (requires token)
GET  /api/processes/top       # top 5 CPU-consuming processes
POST /api/suggestions/dismiss # dismiss a suggestion by id
```

This is intended for your own machine only, there is no authentication or HTTPS. It's not recommended to expose it on a network.

## Energy-Saving Suggestions

PowerWatch can detect sustained high power draw and propose actions to reduce it. Every action requires explicit confirmation through a modal dialog that shows the exact command it will run, with a one-time token; nothing is applied automatically.

A suggestion appears once per excursion above its threshold and goes away on its own once the draw has stayed under it for 5 seconds, so a brief dip around the threshold does not make it flicker. You can also dismiss it (the `✕` button in the web dashboard, `d` in the TUI). A dismissed suggestion stays gone until the draw has stayed under the threshold for 5 seconds and then crosses it again.

### Implemented Suggestions

| Suggestion | Default condition | Where | Action | Safety |
|------------|-------------------|-------|--------|--------|
| **Switch to power saver** | Total > 15W for 3s, or CPU > 8W for 3s | Web and TUI | Change power profile to `power-saver` | Modifies system profile |
| **Show top processes** | CPU > 8W for 10s | Web only | Display top 5 CPU processes in a modal | Read-only, no system changes |

The thresholds and durations above are defaults; see [Configuration](#configuration) to change them.

Both the web dashboard and the TUI skip the power saver suggestion when the machine is already in that profile. It reads the current profile with `powerprofilesctl` (Linux), `pmset` (macOS) or `powercfg` (Windows, matched by the scheme's GUID, so localized names work). If the profile can't be detected, for instance with a custom Windows plan or a missing command, the suggestion still appears.

If an action fails (for example `powerprofilesctl` isn't installed), the dashboard shows the error and keeps the suggestion in the list.

### Power Profile Commands

| Profile | Linux | macOS | Windows |
|---------|-------|-------|---------|
| Power Saver | `powerprofilesctl set power-saver` | `pmset -a lowpowermode 1` | `powercfg /setactive SCHEME_MAX` |
| Balanced | `powerprofilesctl set balanced` | `pmset -a lowpowermode 0` | `powercfg /setactive SCHEME_BALANCED` |
| Performance | `powerprofilesctl set performance` | `pmset -a lowpowermode 0` | `powercfg /setactive SCHEME_MIN` |

The command is chosen for the OS PowerWatch was built for. On macOS, `pmset -a` may require elevated privileges.

### Security Notes

The web dashboard can execute system commands from a browser click. While it binds only to `127.0.0.1` and has no authentication, consider these risks:

- **Confirmation tokens** mitigate CSRF. Each token is single-use and server-generated.
- **Never expose the port on a network**. If someone reaches the dashboard remotely, a crafted page could trigger actions via CSRF.
- **Double confirmation** is shown in the browser (`confirm()` dialog) in addition to the token.

Use the web dashboard only on trusted local machines.

### Platform-Specific Sensor Sources

**Linux**: direct filesystem reads (`/sys/class/powercap` for RAPL, `hwmon` for AMD GPU, NVML for NVIDIA, `iostat`/`nvme-cli` for disk).

**Windows**: LibreHardwareMonitor WMI provider when available (real CPU RAPL/MSR), falling back to `GetSystemTimes` × assumed TDP for CPU; NVML unchanged; RAM uses `GlobalMemoryStatusEx`; disk uses the PDH performance counter API.

**macOS**: `powermetrics` for CPU/GPU (requires `sudo`), `vm_stat` + `sysctl hw.memsize` for RAM, `iostat` for disk. CPU and GPU are measured; disk and RAM are estimated.

## Configuration

The suggestion thresholds can be changed with a TOML file. The first time `powerwatch-tui` or `powerwatch-web` runs, it creates `config.toml` in the project folder (the one that contains `Cargo.toml`), filled with the defaults from [`config.example.toml`](config.example.toml). Edit it and restart. Every key is optional, so you only need to keep the ones you want to change:

```toml
[suggestions.power_saver]
cpu_watts = 25.0
sustained_seconds = 5

[suggestions.top_processes]
enabled = false
```

`powerwatch-tui` and `powerwatch-web` look for the file in this order:

1. `--config <path>`
2. the `POWERWATCH_CONFIG` environment variable
3. `config.toml` in the project folder. PowerWatch finds the folder from the location of the executable, so it works from any directory and under `sudo`, and it creates the file there if it does not exist.
4. `$XDG_CONFIG_HOME/powerwatch/config.toml`, or `~/.config/powerwatch/config.toml` (`%APPDATA%\powerwatch\config.toml` on Windows). This is only used when the executable is not inside the project folder, for example after copying it to `/usr/local/bin`, and PowerWatch does not create it.

A file named with the flag or the variable must exist. Unknown keys, values of the wrong type and thresholds that are not positive numbers stop the program with a message that names the problem, instead of silently falling back to the defaults. `config.toml` holds your own settings, so add `/config.toml` to `.gitignore` to keep it out of your commits.

| Key | Default | Meaning |
|-----|---------|---------|
| `suggestions.clear_after_seconds` | `5` | How long a suggestion stays after the draw falls back under its threshold |
| `suggestions.power_saver.enabled` | `true` | Turns the "Switch to power saver" suggestions on or off |
| `suggestions.power_saver.total_watts` | `15.0` | Total draw above which it is suggested |
| `suggestions.power_saver.cpu_watts` | `8.0` | CPU draw above which it is suggested |
| `suggestions.power_saver.sustained_seconds` | `3` | How long the draw must stay above the threshold |
| `suggestions.top_processes.enabled` | `true` | Turns the "Show top processes" suggestion on or off (web only) |
| `suggestions.top_processes.cpu_watts` | `8.0` | CPU draw above which it is suggested |
| `suggestions.top_processes.sustained_seconds` | `10` | How long the draw must stay above the threshold |

Running with `sudo` is fine: the project folder comes from the executable, not from `~`, and a file created under `sudo` is handed back to your user. If you copy the binaries elsewhere, `~` can point to root's home instead of yours; pass the file explicitly, and your shell expands `~` before `sudo` runs:

```bash
sudo powerwatch-tui --config ~/.config/powerwatch/config.toml
```

## Development

```bash
cargo test --workspace   # unit tests - no real hardware required
cargo clippy --workspace # lints
cargo fmt                # formatting
cargo build --release    # optimized binaries
```

Hardware-facing sensors (RAPL, NVML, hwmon) take a path or handle as a constructor argument, so tests exercise parsing and calculation logic against fixture data rather than live hardware. Manual validation on real hardware is still recommended before trusting numbers on an unfamiliar machine.

## Windows Support (Experimental)

Building on Windows:

```powershell
cargo build --release
.\target\release\powerwatch.exe
.\target\release\powerwatch-tui.exe
```

Most likely failure points, in rough order of confidence:
1. LibreHardwareMonitor WMI query (`powerwatch-core/src/sensors/windows/lhm.rs`) — COM/WMI via the `wmi` crate is the least verifiable piece blind.
2. Disk sensor (`powerwatch-core/src/sensors/windows/disk.rs`) — PDH API behavior can vary.
3. CPU fallback path — `GetSystemTimes` semantics (kernel time includes idle) are easy to get subtly wrong.
4. RAM sensor — simplest of the four, most likely to just work.

## macOS Support (Experimental)

CPU and GPU are **measured** via `powermetrics` (Apple reports real wattage). The trade-off: `powermetrics` always requires root, so the whole process must run under `sudo`:

```bash
sudo ./target/release/powerwatch
sudo ./target/release/powerwatch-tui
```

Most likely failure points:
1. Disk sensor (`powerwatch-core/src/sensors/macos/disk.rs`) — `iostat` column layout depends on disk count.
2. CPU/GPU sensor — the plist parsing is simple and fixture-tested, but the exact `powermetrics` invocation is unverified.
3. RAM sensor — `vm_stat` "available" memory is an approximation; numbers should be in the right ballpark but may not match Activity Monitor exactly.

## License

[MIT](https://github.com/lnpotter/PowerWatch/blob/main/LICENSE)
