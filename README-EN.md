# DSH Control Panel

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Build & Test](https://github.com/CrandyChen/dsh-control-panel/actions/workflows/build.yml/badge.svg)](https://github.com/CrandyChen/dsh-control-panel/actions/workflows/build.yml)

**DSH Control Panel** is a Windows desktop GUI for [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) (DSH):
install, start, stop, update, repair, uninstall DeepSeek Harness, and manage its plugins — all without touching the command line.

[简体中文](README.md)

## What is this?

DSH Control Panel wraps DeepSeek Harness's common terminal operations — **install**, **update**, **uninstall**, **start**, **stop**, **repair**, and **plugin management** — into a graphical interface, making it accessible for beginners.

- **Beginner-friendly**: Fully graphical UI with friendly guidance and prompts.
- **Lightweight**: Built on Tauri 2, following DeepSeek Harness's official "**Run from source**" approach without bundling dependencies. Single-file, compact, and portable.
- **Stay up-to-date**: Automatically fetches the latest DeepSeek Harness source code and builds the newest version.
- **Non-intrusive to DSH**: Only wraps DSH-related CLI operations without modifying its source code.

## Prerequisites for Installing DSH

Since DSH runs via the "Run from source" method, the following dependencies are required:

| Tool | Requirement | Notes |
| --- | --- | --- |
| Windows | 10 / 11 (64-bit) | WebView2 Runtime (included in Win11; install from Microsoft for Win10 if missing) |
| Git | Any recent version | For cloning / updating DeepSeek Harness |
| Node.js | ≥ 22.19 or ≥ 24 | Required by the DeepSeek Harness engine (LTS recommended) |
| pnpm | ≥ 11.7 | Install after Node.js |
| Python | Recommended (optional) | Used by some extended features; does not affect install / start / update |

The control panel automatically detects the above environment on launch. If anything is missing or outdated, in-app installation guidance will be provided.

## Feature Overview

### 1. DSH Installation
- **Description**: Installs DeepSeek Harness using the official "**Run from source**" method.
- **Usage**: Click "Install" and select any parent directory — the control panel will automatically create a `deepseek-harness` subdirectory.
- **Wrapped commands**: `git clone https://github.com/deepseek-ai/deepseek-harness.git` → `pnpm install` → `pnpm run build`.

### 2. DSH Start / Stop
- **Description**: Starts or stops the DeepSeek Harness web service.
- **Usage**: Click "Start" — once the service is ready, `http://127.0.0.1:3080` automatically opens in an in-app tab; "Stop" terminates the entire process tree.
- **Wrapped commands**: `pnpm dsh web`.

### 3. DSH Update
- **Description**: Checks whether the official DSH source has updates. When a new version is available, a dialog shows the details for you to choose "Update Now" or "Ignore". Background scheduled auto-check displays a red **NEW** badge on the Update button when updates are found.
- **Usage**: Click "Update" → review the details dialog → confirm execution; the web service is automatically stopped before updating.
- **Wrapped commands**: `git fetch` / `git rev-parse` / `git rev-list --count` / `git log` (check) → `git pull --ff-only` → `pnpm install` → `pnpm run build` (apply).

### 4. DSH Repair Installation
- **Description**: Repairs installations that fail to start (commonly caused by interrupted plugin installs/updates leaving residual broken states). Automatically cleans lock files and leftover processes, resets the install directory to official code, and rebuilds. In the worst case, deletes and re-clones the install directory.
- **Usage**: Click "Repair Installation" (also automatically suggested via a friendly dialog when DSH fails to start).
- **Wrapped commands**: `git fetch` → `git reset --hard origin/<branch>` → `git clean -fdx` → `pnpm install` → `pnpm run build` (deep repair: delete `node_modules`; fallback: full re-clone).

### 5. DSH Plugin Management
- **Description**: Graphically install / update / uninstall DSH profile plugins (`dsh plugin`): smart input (npm package name, `github:owner/repo[#ref]`, GitHub URL, or full command), multi-select / one-click uninstall, isolated per-profile; automatically handles common pnpm issues (build scripts blocked — including the `prepare` script of git-hosted plugins, `ERR_PNPM_GIT_DEP_PREPARE_NOT_ALLOWED` → auto-parses and writes allowBuilds, then retries; global `dsh` unavailable → auto-falls back to `pnpm dsh`).
- **Usage**: Open "Plugin Manager"; while the web service is running, **only new plugins can be installed** (update / uninstall are disabled — stop the service first). After installation, a prompt to restart DSH to activate plugins is shown.
- **Wrapped commands**: `dsh plugin --profile <name> add|update|remove <identifier...>`.

### 6. DSH Uninstall
- **Description**: Completely removes DeepSeek Harness.
- **Usage**: Click "Uninstall" and confirm the items to remove — **installation directory** and **DSH user data directory**. Live progress is shown while sizes / file counts are being calculated (cancellable), so the UI never appears frozen.
- **Wrapped commands**: None (direct directory deletion with safety guards).

### 7. Other Tools
- **Open Terminal**: Opens PowerShell in the installation directory.
- **Open UI**: Opens the DSH Web UI in the system browser or a new in-app tab (configurable).
- **Environment Check**: Detects Git / Node.js / pnpm / Python and can open installation guides.
- **Logs**: Daily-rotated log files next to the exe (keeps 5 copies), viewable in real-time within the panel.
- **Settings**: Scheduled new-version check interval, theme, language settings, etc.

## Screenshots

Main UI 
 ![Main UI ](assets/main-en.png) 
 
Plugin Management 
 ![Plugin Management](assets/plugin-en.png) 

## Download & Install

- Download the portable zip from [Releases](https://github.com/CrandyChen/dsh-control-panel/releases):
  Extract and double-click `DSH-Control-Panel.exe` — no installation required.
- Or build from source (see below).

## Development Environment

- Node.js ≥ 22.19 or ≥ 24, pnpm ≥ 11.7
- Rust (rustup stable) + VS Build Tools (check "Desktop development with C++")
- WebView2 Runtime (included in Win10/11)

```bash
pnpm install
pnpm tauri dev          # Dev mode (hot reload)
pnpm tauri build        # Build exe
pnpm portable           # Build and package portable zip → dist-portable/
```

## Directory Structure

```
src/                     React frontend (antd 5, light/dark themes, bilingual)
  i18n.ts                UI text dictionary (Simplified Chinese + English)
  usePanel.ts            Core state hook (config/detect/stage/log/tab/action)
  components/            Dialogs and panels (install/update/repair/plugin/uninstall, etc.)
src-tauri/src/           Rust backend (config/logging/process/detect/tools/net/version/
                         install/update/repair/uninstall/web/plugin/terminal/i18n)
scripts/                 Portable packaging (pnpm portable)
assets/                  Screenshots referenced by README
.github/workflows/       CI: build.yml (check) + release.yml (auto-release zip on tag)
```

## License

[MIT](LICENSE) © 2026 DSH Control Panel Contributors
