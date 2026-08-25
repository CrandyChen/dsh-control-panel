// 打包 portable zip：pnpm tauri build → 组装目录（含内置 runtime） → 压缩。
// 用法：pnpm portable（或 node scripts/package-portable.mjs）
//
// 内置运行时（runtime\）用于「更独立」的便携运行：DSH 的源码安装 / 预构建内核与
// 插件管理都依赖 node/pnpm，无需用户全局安装。dev 下若无 runtime 目录则回退系统 PATH。
//
// 运行时来源（优先级）：
//   1) 直接复制本机已全局安装的 Node.js 与 pnpm（最可靠、无需联网）；
//   2) 复制失败时回退到网络下载（NODE_VER / PNPM_VER 钉死版本）。
// 若你只想用现有全局环境、不联网，本脚本默认就会走第 1 条路径。

import { execSync, spawnSync } from "node:child_process";
import {
  copyFileSync,
  cpSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { basename, dirname, join, resolve } from "node:path";

const root = resolve(".");
const version = JSON.parse(readFileSync(join(root, "package.json"), "utf8")).version;
const exeName = "DSH-Control-Panel.exe";
// Tauri 是否将主程序重命名为 productName 取决于构建配置：兼容两种产物名。
const exeCandidates = [
  join(root, "src-tauri", "target", "release", exeName),
  join(root, "src-tauri", "target", "release", "dsh-control-panel.exe"),
];
const folderName = `DSH-Control-Panel-portable-${version}-windows-x64`;
const distDir = join(root, "dist-portable", folderName);
const zipPath = join(root, "dist-portable", `${folderName}.zip`);

// 内置运行时回退下载版本（可在此调整；DSH 要求 Node.js ≥ 22.19 或 ≥ 24，pnpm ≥ 11.7）。
// 注意：仅在「无法从现有环境复制」时才联网下载；正常本机/CI 会直接复制现有环境。
const NODE_VER = "24.19.0";
const PNPM_VER = "11.24.0";

/** 依次尝试多个下载地址，返回首个成功后落盘的本地路径。 */
function downloadFirst(urls, dest) {
  for (const url of urls) {
    const dir = dirname(dest);
    mkdirSync(dir, { recursive: true });
    const res = spawnSync(
      "node",
      [join(root, "scripts", "download.mjs"), url, dest],
      { stdio: "inherit" },
    );
    if (res.status === 0 && existsSync(dest)) return dest;
    // 失败：清理残件，尝试下一个地址。
    try {
      rmSync(dest, { force: true });
    } catch {
      /* ignore */
    }
  }
  throw new Error(`无法下载运行时（尝试了 ${urls.length} 个地址）`);
}

/** 解压 zip 到目标目录（Windows tar 支持 zip）。 */
function extractZip(zip, dir) {
  mkdirSync(dir, { recursive: true });
  const r = spawnSync("tar", ["-xf", zip, "-C", dir], { stdio: "inherit" });
  if (r.status !== 0) throw new Error(`解压 ${zip} 失败`);
}

/** 把 src 目录下所有条目平铺复制到 dst（不保留多余一层包装目录）。 */
function flattenMove(src, dst) {
  if (!existsSync(src)) return;
  for (const name of readdirSync(src)) {
    const from = join(src, name);
    const to = join(dst, name);
    rmSync(to, { recursive: true, force: true });
    cpSync(from, to, { recursive: true });
  }
}

/** 校验运行时完整性：node.exe 与可执行的 pnpm 必须存在，否则打包失败。 */
function verifyRuntime(runtimeDir) {
  const node = join(runtimeDir, "node.exe");
  const pnpm = ["pnpm.exe", "pnpm.cmd", "pnpm.bat"].find((n) =>
    existsSync(join(runtimeDir, n)),
  );
  if (!existsSync(node)) throw new Error("内置运行时缺少 node.exe（Node 解压失败）");
  if (!pnpm) throw new Error("内置运行时缺少 pnpm 可执行文件（pnpm 解压失败）");
  console.log(`   运行时就绪：${node} + ${pnpm}`);
}

/** 从已有环境复制整套 Nodejs（node.exe / npm / npx / corepack / node_modules/npm 等）。
 * 直接使用当前运行脚本的 node（process.execPath），无需依赖 PATH 上的 node。 */
function copyNodeFromEnv(dst) {
  const execPath = process.execPath;
  if (!execPath) return false;
  const home = dirname(execPath);
  if (!existsSync(join(home, "node.exe"))) return false;
  console.log(`   复制 Node 运行时：${home}`);
  flattenMove(home, dst);
  return existsSync(join(dst, "node.exe"));
}

/** 读取 `where <name>` 的输出行（去掉空行）。 */
function whereLines(name) {
  try {
    const r = spawnSync("where", [name], { encoding: "utf8" });
    if (r.status !== 0) return [];
    return (r.stdout || "")
      .split(/\r?\n/)
      .map((s) => s.trim())
      .filter(Boolean);
  } catch {
    return [];
  }
}

/** 定位全局 pnpm 包目录：优先 `npm root -g`，再用 `where pnpm` 反向推断（兼容 node_global 移到其它盘）。 */
function findPnpmPackageDir() {
  const r = spawnSync("npm", ["root", "-g"], { encoding: "utf8" });
  if (r.status === 0 && (r.stdout || "").trim()) {
    const p = join((r.stdout || "").trim(), "pnpm");
    if (existsSync(join(p, "package.json"))) return p;
  }
  for (const line of whereLines("pnpm")) {
    const lower = line.toLowerCase();
    if (!/\.(cmd|bat|ps1)$/.test(lower)) continue;
    const dir = dirname(line);
    for (const cand of [
      join(dir, "node_modules", "pnpm"),
      join(dir, "..", "node_modules", "pnpm"),
    ]) {
      if (existsSync(join(cand, "package.json"))) return cand;
    }
  }
  return "";
}

/** 定位 standalone pnpm.exe（如 @pnpm/exe 或官方 standalone 安装）。 */
function findPnpmExe() {
  for (const line of whereLines("pnpm")) {
    if (line.toLowerCase().endsWith(".exe")) return line;
  }
  return "";
}

/** 把 pnpm 包复制进 runtime，并按包内 bin 字段生成 pnpm.cmd 启动器。 */
function installPnpmPackage(dst, pkgDir) {
  const nm = join(dst, "node_modules");
  mkdirSync(nm, { recursive: true });
  cpSync(pkgDir, join(nm, "pnpm"), { recursive: true });

  // 依据包内 bin 字段确定启动文件（兼容 .cjs/.mjs/.js）。
  let binRel = "bin/pnpm.cjs";
  try {
    const pkg = JSON.parse(readFileSync(join(pkgDir, "package.json"), "utf8"));
    const bin = pkg.bin;
    if (typeof bin === "string") binRel = bin;
    else if (bin && typeof bin === "object") {
      binRel = bin.pnpm || Object.values(bin)[0] || binRel;
    }
  } catch {
    /* 读取失败时用默认 bin/pnpm.cjs */
  }
  if (!existsSync(join(nm, "pnpm", binRel))) {
    const alt = ["bin/pnpm.mjs", "bin/pnpm.cjs", "bin/pnpm.js"].find((f) =>
      existsSync(join(nm, "pnpm", f)),
    );
    if (!alt) return false;
    binRel = alt;
  }

  const cmd = `@echo off\r\n"%~dp0node.exe" "%~dp0node_modules\\pnpm\\${binRel.replace(/\//g, "\\")}" %*\r\n`;
  writeFileSync(join(dst, "pnpm.cmd"), cmd);
  return existsSync(join(dst, "pnpm.cmd"));
}

/** 从已有环境复制全局 pnpm（包形态或 standalone exe），并在 runtime 生成 pnpm.cmd 启动器。 */
function copyPnpmFromEnv(dst) {
  const pkgDir = findPnpmPackageDir();
  if (pkgDir) {
    console.log(`   复制 pnpm 运行时（包形态）：${pkgDir}`);
    return installPnpmPackage(dst, pkgDir);
  }
  const exe = findPnpmExe();
  if (exe) {
    console.log(`   复制 pnpm 运行时（standalone）：${exe}`);
    copyFileSync(exe, join(dst, "pnpm.exe"));
    writeFileSync(join(dst, "pnpm.cmd"), `@echo off\r\n"%~dp0pnpm.exe" %*\r\n`);
    return existsSync(join(dst, "pnpm.cmd"));
  }
  return false;
}

/** 确保 runtime 下存在可执行的 pnpm.cmd：
 *  - 已有则跳过（复制路径已生成）；
 *  - 仅有 pnpm.exe（standalone）则生成转发 .cmd；
 *  - 仅有 node_modules/pnpm/bin/*.cjs（npm 包形态）则生成 node 启动 .cmd。 */
function ensurePnpmCmd(dst) {
  const shim = join(dst, "pnpm.cmd");
  if (existsSync(shim)) return true;
  if (existsSync(join(dst, "pnpm.exe"))) {
    writeFileSync(shim, `@echo off\r\n"%~dp0pnpm.exe" %*\r\n`);
    return true;
  }
  const bin = ["node_modules/pnpm/bin/pnpm.cjs", "node_modules/pnpm/bin/pnpm.mjs"].find((p) =>
    existsSync(join(dst, p)),
  );
  if (bin) {
    writeFileSync(
      shim,
      `@echo off\r\n"%~dp0node.exe" "%~dp0${bin.replace(/\//g, "\\")}" %*\r\n`,
    );
    return true;
  }
  return false;
}

/** 若 src 恰好只有一个子目录且无文件，返回该子目录（处理 zip 顶层包装目录）；否则返回 src。 */
function effectiveExtractRoot(src) {
  if (!existsSync(src)) return src;
  const entries = readdirSync(src);
  const dirs = entries.filter((n) => {
    try {
      return statSync(join(src, n)).isDirectory();
    } catch {
      return false;
    }
  });
  if (dirs.length === 1 && entries.length === 1) return join(src, dirs[0]);
  return src;
}

/** 从网络下载运行时（现有环境不可用时回退）。 */
function downloadRuntime(dst) {
  // Node：node-v24.19.0-win-x64.zip → 解压到 nodeEx → 平铺到 runtime。
  const nodeZip = join(root, "dist-portable", "node-win.zip");
  const nodeEx = join(root, "dist-portable", `node-v${NODE_VER}-win-x64`);
  tempArtifacts.push(nodeZip, nodeEx);
  downloadFirst([`https://nodejs.org/dist/v${NODE_VER}/node-v${NODE_VER}-win-x64.zip`], nodeZip);
  extractZip(nodeZip, nodeEx);
  flattenMove(effectiveExtractRoot(nodeEx), dst);

  // pnpm：GitHub 官方 standalone zip（资产名 pnpm-win32-x64.zip）。
  const pnpmZip = join(root, "dist-portable", "pnpm.zip");
  const pnpmEx = join(root, "dist-portable", "pnpm-ex");
  tempArtifacts.push(pnpmZip, pnpmEx);
  downloadFirst(
    [
      `https://github.com/pnpm/pnpm/releases/download/v${PNPM_VER}/pnpm-win32-x64.zip`,
      `https://github.com/pnpm/pnpm/releases/download/v${PNPM_VER}/pnpm-win-x64.zip`,
    ],
    pnpmZip,
  );
  extractZip(pnpmZip, pnpmEx);
  flattenMove(effectiveExtractRoot(pnpmEx), dst);
}

console.log(`[1/5] 构建 Tauri 应用（版本 ${version}）...`);
execSync("pnpm tauri build", { stdio: "inherit", cwd: root });

const exePath = exeCandidates.find((p) => existsSync(p));
if (!exePath) {
  throw new Error(`未找到构建产物：${exeCandidates.join(" / ")}`);
}
console.log(`   使用产物：${exePath}`);

console.log("[2/5] 组装 portable 目录 ...");
rmSync(distDir, { recursive: true, force: true });
mkdirSync(distDir, { recursive: true });
copyFileSync(exePath, join(distDir, exeName));

console.log("[3/5] 组装内置运行时（Node + pnpm）...");
const runtimeDir = join(distDir, "runtime");
mkdirSync(runtimeDir, { recursive: true });

// 打包临时产物：结束时统一清理，避免残留在 dist-portable。
const tempArtifacts = [];

try {
  // 优先从现有开发环境复制全局 Node / pnpm（更可靠、无需联网）；失败才回退下载。
  console.log("   尝试从现有环境复制运行时...");
  const nodeOk = copyNodeFromEnv(runtimeDir);
  const pnpmOk = copyPnpmFromEnv(runtimeDir);

  if (!nodeOk || !pnpmOk) {
    console.log(
      `   现有环境复制不完整（node=${nodeOk} pnpm=${pnpmOk}），回退到网络下载...`,
    );
    // 网络下载会整包写入 runtime，先清空已复制内容避免混合。
    rmSync(runtimeDir, { recursive: true, force: true });
    mkdirSync(runtimeDir, { recursive: true });
    downloadRuntime(runtimeDir);
  }

  // 源码模式固定用 pnpm.cmd，确保 runtime 中一定存在可执行的 pnpm.cmd。
  if (!ensurePnpmCmd(runtimeDir)) {
    throw new Error("内置运行时缺少可执行的 pnpm.cmd");
  }

  verifyRuntime(runtimeDir);
} catch (e) {
  console.error("[3/5] 内置运行时组装失败：", e.message);
  throw e;
}

console.log("[3/5] 运行时组装完成。");

console.log("[4/5] 生成 README.txt ...");
writeFileSync(
  join(distDir, "README.txt"),
  [
    "DSH Control Panel — DeepSeek Harness 控制面板（便携版）",
    "======================================================",
    `版本：${version}`,
    "",
    "系统要求：",
    "  · Windows 10 / 11（64 位）",
    "  · WebView2 Runtime（Win11 自带，Win10 缺失时请到",
    "    https://developer.microsoft.com/microsoft-edge/webview2/ 安装）",
    "  · 内置 Node.js 与 pnpm（无需单独安装）",
    "  · 源码安装模式需要 Git（预构建内核模式不需要）",
    "",
    "安装 DeepSeek Harness（两种模式任选）：",
    "  1. 预构建内核（默认）：从 GitHub 下载最新 deepseek-harness-pkg-windows.zip，",
    "     解压到程序目录下的 dsh 子目录，无需 Git / Node.js / pnpm。",
    "  2. 源码安装：点击「安装」选择父目录（默认程序运行目录），自动创建",
    "     deepseek-harness 子目录，依次执行：",
    "     git clone https://github.com/deepseek-ai/deepseek-harness.git",
    "     pnpm install",
    "     pnpm run build",
    "     （需本机已安装 Git；Node.js 与 pnpm 已内置）",
    "",
    "使用方法：",
    "  1. 双击 DSH-Control-Panel.exe 启动（无需安装，解压即用）",
    "  2. 点击「安装」按提示选择安装模式；完成后点击「启动」打开",
    "     DeepSeek Harness Web 界面（http://127.0.0.1:3080）",
    "",
    "功能：安装（两种模式）/ 启动 / 停止 / 更新（按模式）/ 新版本检测（定时+手动）/",
    "      插件管理（dsh plugin 安装/更新/卸载，运行中操作自动停/启 web）/",
    "      网络预检 / 打开终端 / 完全卸载 / 配置持久化 / 完整日志",
    "",
    "说明：git clone / git pull 前会先检查网络可达性（默认 github.com），",
    "      不可达时弹出友好提示；控制面板只执行标准 git / pnpm 命令，不修改",
    "      DeepSeek Harness 自身文件；控制面板配置与日志保存在 exe 同目录。",
    "",
  ].join("\r\n"),
);

console.log("[5/5] 压缩 zip ...");
rmSync(zipPath, { force: true });
const r = spawnSync("tar", ["-a", "-c", "-f", zipPath, "-C", dirname(distDir), basename(distDir)], {
  stdio: "inherit",
});
if (r.status !== 0) {
  throw new Error("压缩 zip 失败");
}

// 清理打包临时产物（下载的 zip / 解压目录），避免残留在 dist-portable。
for (const a of tempArtifacts) {
  rmSync(a, { recursive: true, force: true });
}

console.log(`✅ 完成：${zipPath}`);
