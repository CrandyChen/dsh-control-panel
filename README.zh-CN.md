# DSH Control Panel（DSH 控制面板）

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Build & Test](https://github.com/CrandyChen/dsh-control-panel/actions/workflows/build.yml/badge.svg)](https://github.com/CrandyChen/dsh-control-panel/actions/workflows/build.yml)

**DSH Control Panel** 是面向 [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness)（DSH）的
Windows 桌面图形化控制面板：安装、启动、停止、更新、修复、卸载 DeepSeek Harness，
并管理它的插件——全程无需接触命令行。

[English](README.md)

## 这是什么？

DSH Control Panel 将 DeepSeek Harness 常用的**安装**、**更新**、**卸载**、**启动**、**停止**、**修复**、**插件管理**等终端命令行操作封装成图形界面，便于新手操作。

- **新手友好**：全图形化界面操作，友好的引导和提示。
- **轻量**：基于Tauri 2，遵循DeepSeek Harness的官方的“**Run from source**”方式，不封装依赖，保持轻量，单文件、小巧的便携。
- **及时更新**：自动拉取DeepSeek Harness最新源码，构建最新版本应用。
- **不侵入 DSH**：只封装DSH相关命令行操作，不修改其源码。

## 安装 DSH 所需的运行环境
- 由于采用 Run from source 方式运行 DeepSeek Harness，因此需要安装相关依赖。

| 工具 | 要求 | 说明 |
| --- | --- | --- |
| Windows | 10 / 11（64 位） | WebView2 Runtime（Win11 自带；Win10 缺失时到微软官网安装） |
| Git | 任意较新版本 | 用于克隆 / 更新 DeepSeek Harness |
| Node.js | ≥ 22.19 或 ≥ 24 | DeepSeek Harness 引擎要求（推荐 LTS） |
| pnpm | ≥ 11.7 | 需先安装 Node.js |
| Python | 推荐（可选） | 部分扩展功能会用到；不影响安装 / 启动 / 更新 |

控制面板启动时会自动检测以上环境，缺失或版本过低时，程序内会有相关安装指引。

## 功能总览

### 1. DSH安装
- **功能说明**：按照官方的“**Run from source**”方式安装DeepSeek Harness。
- **用法**：点击「安装」，任意选择一个父目录——控制面板会自动创建  `deepseek-harness` 子目录。
- **封装的命令**：`git clone https://github.com/deepseek-ai/deepseek-harness.git`  → `pnpm install` → `pnpm run build`。

### 2. DSH 启动 / 停止
- **功能说明**：启动或结束 DeepSeek Harness web 服务。
- **用法**：点击「启动」，服务就绪后自动在程序内标签页打开  `http://127.0.0.1:3080`；「停止」结束整个进程树。
- **封装的命令**：`pnpm dsh web`。

### 3. DSH 更新
- **功能说明**：检测DSH官方源码是否有更新，有新版本时弹出对话框展示详情，由你选择「立即更新」或「忽略」。 后台定时自动检测，有新版本时更新按钮上显示红色 **NEW** 提示。
- **用法**：点击「更新」→ 查看详情对话框 → 确认执行；更新前会自动停止 web 服务。
- **封装的命令**：`git fetch` / `git rev-parse` / `git rev-list --count` /  `git log`（检测）→ `git pull --ff-only` → `pnpm install` → `pnpm run build`（执行）。

### 4. DSH 修复安装
- **功能说明**：修复无法正常启动的安装（常见原因是插件安装 / 更新被中断残留异常状态）。自动清理锁文件与残留进程、将安装目录重置为官方代码并重新构建，最坏情况删除并重新克隆安装目录。
- **用法**：点击「修复安装」（DSH 启动失败时也会以友好对话框自动建议）。
- **封装的命令**：`git fetch` → `git reset --hard origin/<分支>` →  `git clean -fdx` → `pnpm install` → `pnpm run build`（深度修复：删除 `node_modules`；  保底：整体重新克隆）。

### 5. DSH 插件管理
- **功能说明**：图形化安装 / 更新 / 卸载 DSH profile 插件（`dsh plugin`）： 智能输入（npm 包名、`github:owner/repo[#ref]`、GitHub 链接或完整命令）、多选 / 一键卸载、各 profile 互相隔离；自动处理两个常见 pnpm 问题 （构建脚本被拦截 → 自动写入 allowBuilds；全局 `dsh` 不可用 → 自动改用 `pnpm dsh`）。
- **用法**：打开「插件管理」；web 服务运行中**只能安装新插件**（更新 / 卸载被禁用，需先停止服务），安装完成后会提示重启 DSH 使插件生效。
- **封装的命令**：`dsh plugin --profile <名称> add|update|remove <标识...>`。

### 6. DSH 卸载
- **功能说明**：彻底移除 DeepSeek Harness。
- **用法**：点击「卸载」，确认要卸载项—— **安装目录** 与 **DSH 用户数据目录**。
- **封装的命令**：无（直接删除目录，带安全护栏）。

### 7. 其他工具
- **打开终端**：在安装目录打开 PowerShell。
- **打开界面**：在系统浏览器或程序新Tab（可配置）打开 DSH Web 界面。
- **运行环境检测**：检测 Git / Node.js / pnpm / Python，并可打开安装指引。
- **日志**：exe 同目录按天轮转的日志文件（保留 5 份），面板内实时查看。
- **设置**：定时检测新版本及间隔、主题、语言设置等。

## 界面截图

| 主界面（中文） | 主界面（English） |
| --- | --- |
| ![主界面（中文）](assets/main.png) | ![主界面（English）](assets/main-en.png) |

## 下载与安装

- 到 [Releases](https://github.com/CrandyChen/dsh-control-panel/releases) 下载便携版 zip：
  解压后双击 `DSH-Control-Panel.exe` 即可，无需安装。
- 或从源码构建（见下）。

## 开发环境

- Node.js ≥ 22.19 或 ≥ 24、pnpm ≥ 11.7
- Rust（rustup stable）+ VS Build Tools（勾选「使用 C++ 的桌面开发」）
- WebView2 Runtime（Win10/11 自带）

```bash
pnpm install
pnpm tauri dev          # 开发模式（热更新）
pnpm tauri build        # 构建 exe
pnpm portable           # 构建并打包便携 zip → dist-portable/
```


## 目录结构

```
src/                     React 前端（antd 5、深浅主题、中英双语）
  i18n.ts                界面文案字典（简体中文 + English）
  usePanel.ts            核心状态钩子（配置/探测/阶段/日志/标签页/动作）
  components/            对话框与面板（安装/更新/修复/插件/卸载等）
src-tauri/src/           Rust 后端（config/logging/process/detect/tools/net/version/
                         install/update/repair/uninstall/web/plugin/terminal/i18n）
scripts/                 便携打包（pnpm portable）
assets/                  README 引用的界面截图
.github/workflows/       CI：build.yml（检查）+ release.yml（打 tag 自动发布 zip）
```


## 许可

[MIT](LICENSE) © 2026 DSH Control Panel Contributors
