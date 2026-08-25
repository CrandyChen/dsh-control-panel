# DSH Control Panel

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Build & Test](https://github.com/CrandyChen/dsh-control-panel/actions/workflows/build.yml/badge.svg)](https://github.com/CrandyChen/dsh-control-panel/actions/workflows/build.yml)

**DSH Control Panel** is a Windows desktop GUI for [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) (DSH):
install, start, stop, update, repair, uninstall, and run DeepSeek Harness, and manage its plugins — without touching the command line.
It can also be used as DSH's desktop client.

[简体中文](README.md)

## What is this?

DSH Control Panel wraps DeepSeek Harness's common terminal operations — **install**, **update**, **uninstall**, **start**, **stop**, **repair**, and **plugin management** — into a graphical interface for easy use. It also has built-in web access, so it can be used as DSH's desktop client.

- **Self-contained & portable**: Developed with Tauri 2, a portable build that runs right after extraction; Node.js and pnpm are bundled, so users don't need to install a runtime separately.
- **DSH dual install modes**: Downloads the **prebuilt kernel** by default; you can also install from the official source (which then requires Git on your machine).
- **Stay up-to-date**: Automatically checks the latest version of DSH; you can choose to upgrade to the latest.
- **Non-intrusive to DSH**: Only wraps DSH-related CLI operations without modifying its source code.

## System Requirements

| Item | Notes |
| --- | --- |
| Windows | 10 / 11 (64-bit) |
| WebView2 Runtime | Included in Win11; install from Microsoft for Win10 if missing |
| Git | **Only needed for the source install mode**; not needed for the prebuilt-kernel mode |

Node.js and pnpm are bundled with the control panel; no separate install is needed.

## DSH download sources

| Install mode | Source |
| --- | --- |
| Prebuilt kernel | <https://github.com/dsh-tauri-desk/deepseek-harness-pkg/releases/latest> (`deepseek-harness-pkg-windows.zip`) |
| Official source | <https://github.com/deepseek-ai/deepseek-harness.git> |

## Installing DeepSeek Harness (two modes)

Click "Install" and choose an install mode:

- **Prebuilt kernel (default)**: Downloads the latest `deepseek-harness-pkg-windows.zip` from GitHub ([deepseek-harness-pkg](https://github.com/dsh-tauri-desk/deepseek-harness-pkg)) and extracts it into the `dsh` subdirectory under the program directory.
- **From official source**: Pick a parent directory (defaults to the program directory); the control panel creates a `deepseek-harness` subdirectory and runs `git clone` → `pnpm install` → `pnpm run build`. This mode requires Git on your machine.

## Feature Overview

### 1. DSH Start / Stop
- Starts the DeepSeek Harness web service; once ready, it opens automatically according to the "Default UI open mode" setting (in-app tab by default, or the system browser; see "Settings"). "Stop" terminates the entire process tree.
- Runs in the background: DSH keeps running as an independent process; closing the panel does not affect it.

### 2. DSH Update
- Checks for updates according to your install mode: the prebuilt kernel compares against the latest GitHub release; the source mode compares against the official repository.
- A scheduled auto-check shows a red **NEW** badge on the Update button when a new version is found; the web service is stopped automatically before updating.

### 3. DSH Repair Installation
- Repairs installations that fail to start. Cleans abnormal state and rebuilds: the source mode runs a git reset + rebuild; the prebuilt mode re-downloads the kernel.

### 4. DSH Plugin Management
- Graphically install / update / uninstall DSH profile plugins (`dsh plugin`); plugins are isolated per profile.
- Supports npm package names, `github:owner/repo[#version]`, GitHub repo URLs, GitHub tarball URLs, or even a full `dsh plugin` command.
- Common issues are handled automatically: when pnpm blocks a plugin's build scripts, it is added to the profile's allowBuilds allowlist and the install retried; plugin operations while the web service runs **stop the service automatically** and **restart it after the operation completes** — no manual intervention needed.

### 5. DSH Uninstall
- Completely removes DeepSeek Harness. The uninstall scope has exactly two items: the **installation directory** and the **DSH user data directory** (~/.dsh — plugins, configuration, credentials, session history, agent presets, etc.).

### 6. Other Tools
- **Open Terminal**: Opens PowerShell in the installation directory.
- **Open UI**: Opens the DSH Web UI in the system browser or a new in-app tab (configurable).
- **Logs**: Daily-rotated log files next to the exe (keeps 5 copies), viewable in real-time within the panel.
- **Settings**: Scheduled new-version check interval, theme, language, etc.

## Screenshots

Main UI
 ![Main UI](assets/main-en.png)

Plugin Management
 ![Plugin Management](assets/plugin-en.png)

## Download & Install

- Download the portable zip from [Releases](https://github.com/CrandyChen/dsh-control-panel/releases):
  Extract and double-click `DSH-Control-Panel.exe` — no installation required; the runtime is bundled.
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
pnpm portable           # Build and package a portable zip with the bundled runtime → dist-portable/
```

## Directory Structure

```
src/                     React frontend (antd 5, light/dark themes, bilingual)
  i18n.ts                UI text dictionary (Simplified Chinese + English)
  usePanel.ts            Core state hook (config/detect/stage/log/tab/action)
  components/            Dialogs and panels (install/update/repair/plugin/uninstall, etc.)
src-tauri/src/           Rust backend (config/logging/process/detect/tools/net/version/
                         install/update/repair/uninstall/web/plugin/prebuilt/terminal/i18n)
scripts/                 Portable packaging (pnpm portable, bundles the Node.js/pnpm runtime)
assets/                  Screenshots referenced by README
.github/workflows/       CI: build.yml (check) + release.yml (auto-release zip on tag)
```

## License

[MIT](LICENSE) © 2026 DSH Control Panel Contributors
