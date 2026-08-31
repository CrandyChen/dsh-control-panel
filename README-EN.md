# DSH Control Panel

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Build & Test](https://github.com/CrandyChen/dsh-control-panel/actions/workflows/build.yml/badge.svg)](https://github.com/CrandyChen/dsh-control-panel/actions/workflows/build.yml)

**DSH Control Panel** is a Windows desktop GUI for [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) (DSH): install, start, stop, update, repair, uninstall, and run DeepSeek Harness, and manage its plugins. It can also be used as DSH's desktop client.

[简体中文](README.md)

## What is this?

DSH Control Panel wraps DeepSeek Harness's common terminal operations — **install**, **update**, **uninstall**, **start**, **stop**, **repair**, and **plugin management** — into a graphical interface for easy use. It also has built-in web access, so it can be used as DSH's desktop client.

- **Self-contained & portable**: Built with Tauri 2, a portable build that runs right after extraction; Node.js and other dependencies are downloaded automatically on first install.
- **Multi-version kernel management**: Install and manage multiple DSH kernel versions at once (prebuilt kernels are stored per version in independent directories, plus a single source install); all versions are independent and share the same data directory (~/.dsh — API credentials, session history, plugins, etc.).
- **DSH dual install modes**: Downloads the **prebuilt kernel** by default — you can pick any published version from GitHub in the install dialog; you can also install from the official source (latest version).
- **Flexible install/repair**: Even after DSH is installed, the "Install" button stays available, so you can install a different mode or a different kernel version. An already-installed same (mode, version) is marked **Installed** and offers **Repair install**.
- **Stay up-to-date**: Automatically checks the latest version of DSH; you can update to the latest. A newly updated version is installed as an independent kernel and becomes the current one; the old version is kept and can be switched to or uninstalled.
- **Non-intrusive to DSH**: Only wraps DSH-related CLI operations without modifying its source code.
- **New-kernel compatible**: Adapts to the new DSH kernels' browser-session auth (token-based URLs); readiness detection works with both old and new kernels, avoiding false "start failed" reports.

## System Requirements

| Item | Notes |
| --- | --- |
| Windows | 10 / 11 (64-bit) |
| WebView2 Runtime | Included in Win11; install from Microsoft for Win10 if missing |

The program has built-in Git client support; Node.js and pnpm are downloaded automatically on first install — none requires a separate install.

## Installing DeepSeek Harness (two modes, multiple versions)

The install location is fixed to the control panel's own directory. Prebuilt kernels are extracted into a `dsh-<version>` subdirectory per version (independent, can coexist); a source install is cloned into the `deepseek-harness` subdirectory. On startup the program only checks for a DSH installation in this directory and automatically adopts one if found.

Click "Install" and choose an install mode:

- **Prebuilt kernel (default)**: The dialog lists the installable versions published on GitHub ([deepseek-harness-pkg](https://github.com/dsh-tauri-desk/deepseek-harness-pkg)), so you can pick any version, or install the latest directly. An already-installed version is marked **Installed** and offers **Repair install**.
- **From official source**: Installs the latest version (`git clone` of the official source → `pnpm install` → `pnpm run build`). When already installed, this mode offers **Repair install**.

Even after DSH is installed, you can still install other versions; distinct kernel versions are independent.

## Feature Overview

### 1. DSH Start / Stop
- Clicking "Start" opens a picker when multiple kernel versions are installed, letting you choose which kernel to start (each option notes the install mode and version, e.g. "Prebuilt kernel install - 0.1.2-alpha.2"); the last successfully started version is pre-selected, falling back to the latest prebuilt kernel on first run.
- Starts the DeepSeek Harness web service; once ready, it opens automatically according to the "Default UI open mode" setting (in-app tab by default, or the system browser; see "Settings"). "Stop" terminates the entire process tree.
- Runs in the background: DSH keeps running as an independent process; closing the panel does not affect it.

### 2. DSH Update
- Checks for updates according to your install mode: the prebuilt kernel compares against the latest GitHub release; the source mode compares against the official repository.
- A scheduled auto-check shows a red **NEW** badge on the Update button when a new version is found; the web service is stopped automatically before updating.

### 3. DSH Repair Installation
- Repairs a specific kernel version that fails to start. Cleans abnormal state and rebuilds: the source mode runs a git reset + rebuild; the prebuilt mode re-downloads and re-extracts that version's kernel.
- The entry point is in the install dialog: click **Repair install** on an already-installed version.

### 4. DSH Plugin Management
- Graphically install / update / uninstall DSH profile plugins (`dsh plugin`); plugins are isolated per profile.
- Supports npm package names, `github:owner/repo[#version]`, GitHub repo URLs, GitHub tarball URLs, or even a full `dsh plugin` command.
- Select multiple plugins for a one-click "Update selected" batch update.
- Common issues are handled automatically: when pnpm blocks a plugin's build scripts, it is added to the profile's allowBuilds allowlist and the install retried; plugin operations while the web service runs stop the service automatically and restart it after the operation completes.

### 5. DSH Uninstall
- The uninstall dialog lists every installed kernel version (each noting the install mode and version) with multi-select; it also includes the optional **DSH user data directory** (~/.dsh — plugins, configuration, credentials, session history, agent presets, etc.).
- Deleting one kernel version leaves the others unaffected.

## Screenshots

Main UI
 ![Main UI](assets/main-en.png)

Plugin Management
 ![Plugin Management](assets/plugin-en.png)

## Download & Install

- Download the portable zip from [Releases](https://github.com/CrandyChen/dsh-control-panel/releases):
  Extract and double-click `DSH-Control-Panel.exe` — no installation required (Node.js/pnpm are downloaded automatically on first install/start; Git is built-in).
- Or build from source (see below).

## Development Environment

Building the control panel itself requires:

- Node.js ≥ 22.19 or ≥ 24, pnpm ≥ 11.7
- Rust (rustup stable) + VS Build Tools (check "Desktop development with C++")
- WebView2 Runtime (included in Win10/11)

```bash
pnpm install
pnpm tauri dev          # Dev mode (hot reload)
pnpm tauri build        # Build exe
pnpm portable           # Build and package a portable zip → dist-portable/ (bundles the runtime by default)
pnpm portable --no-runtime   # Package a lightweight zip without the bundled runtime (auto-downloads on first install)
```

## License

[MIT](LICENSE) © 2026 DSH Control Panel Contributors
