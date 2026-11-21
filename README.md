# PowerWatch

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
sqlite3 ~/.local/share/powerwatch/history.db "SELECT * FROM readings ORDER BY timestamp DESC LIMIT 20;"
```

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
| `--alert-run <command>` | Shell command to run once when the alert fires |

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
| `s` | Toggle suggestions panel |
| `Enter` | Confirm the first actionable suggestion |
| `y` / `Y` | Apply the action being confirmed |
| `n` / `N` | Cancel the confirmation (the suggestion stays in the list) |
| `s` again | Close the panel |

Nothing runs on its own: press `Enter` to open the confirmation, then `y` to apply it. The TUI has two rules, total draw above 75 W or CPU draw above 30 W for 3 seconds, and both propose switching to the power saver profile. The one-time confirmation token belongs to the web dashboard (see below).

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
GET  /api/snapshot           # latest power readings as JSON
GET  /api/history?since=10m  # readings since a given timestamp
GET  /api/suggestions        # active energy-saving suggestions
POST /api/suggestions/apply  # apply an action (requires token)
GET  /api/processes/top      # top 5 CPU-consuming processes
```

This is intended for your own machine only, there is no authentication or HTTPS. It's not recommended to expose it on a network.

## Energy-Saving Suggestions

PowerWatch can detect sustained high power draw and propose actions to reduce it. Every action requires explicit confirmation through a modal dialog with a one-time token; nothing is applied automatically.

### Implemented Suggestions

| Suggestion | Condition (web) | Condition (TUI) | Action | Safety |
|------------|-----------------|-----------------|--------|--------|
| **Switch to power saver** | Total > 15W for 3s, or CPU > 8W for 3s | Total > 75W for 3s, or CPU > 30W for 3s | Change power profile to `power-saver` | Modifies system profile |
| **Show top processes** | CPU > 8W for 10s | Not available | Display top 5 CPU processes in a modal | Read-only, no system changes |

The web dashboard skips the power saver suggestion when the machine is already in that profile. It reads the current profile with `powerprofilesctl` (Linux), `pmset` (macOS) or `powercfg` (Windows, matched by the scheme's GUID, so localized names work). If the profile can't be detected, for instance with a custom Windows plan or a missing command, the suggestion still appears.

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
