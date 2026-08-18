// 打包 portable zip：pnpm tauri build → 组装目录 → 压缩。
// 用法：pnpm portable（或 node scripts/package-portable.mjs）

import { execSync, spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
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

console.log(`[1/3] 构建 Tauri 应用（版本 ${version}）...`);
execSync("pnpm tauri build", { stdio: "inherit", cwd: root });

const exePath = exeCandidates.find((p) => existsSync(p));
if (!exePath) {
  throw new Error(`未找到构建产物：${exeCandidates.join(" / ")}`);
}
console.log(`   使用产物：${exePath}`);

console.log("[2/3] 组装 portable 目录 ...");
rmSync(distDir, { recursive: true, force: true });
mkdirSync(distDir, { recursive: true });
copyFileSync(exePath, join(distDir, exeName));
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
    "  · Git / Node.js / pnpm（需加入 PATH）。控制面板启动时会自动检测，",
    "    必装项缺失或版本过低时，点击「安装」会在新标签页打开安装指引",
    "    （官网下载链接 + winget / npm 命令）；Python 为推荐项，缺失不影响安装。",
    "",
    "使用方法：",
    "  1. 双击 DSH-Control-Panel.exe 启动（无需安装，解压即用）",
    "  2. 点击「安装」选择父目录，控制面板会自动创建 deepseek-harness 子目录",
    "     并依次执行：",
    "     git clone https://github.com/deepseek-ai/deepseek-harness.git",
    "     pnpm install",
    "     pnpm run build",
    "     （点击「开始安装」后对话框立即关闭，进度与报错实时显示在日志面板）",
    "  3. 安装完成后点击「启动」打开 DeepSeek Harness Web 界面",
    "     （http://127.0.0.1:3080）",
    "",
    "功能：安装 / 启动 / 停止 / 更新 / 新版本检测（定时+手动）/",
    "      运行环境检测与安装指引 / 插件管理（dsh plugin 安装/更新/卸载）/",
    "      网络预检 / 打开终端 / 完全卸载 / 配置持久化 / 完整日志",
    "",
    "说明：git clone / git pull 前会先检查网络可达性（默认 github.com），",
    "      不可达时弹出友好提示；控制面板只执行标准 git / pnpm 命令，不修改",
    "      DeepSeek Harness 自身文件；控制面板配置与日志保存在 exe 同目录。",
    "",
  ].join("\r\n"),
);

console.log("[3/3] 压缩 zip ...");
rmSync(zipPath, { force: true });
const r = spawnSync("tar", ["-a", "-c", "-f", zipPath, "-C", dirname(distDir), basename(distDir)], {
  stdio: "inherit",
});
if (r.status !== 0) {
  throw new Error("压缩 zip 失败");
}
console.log(`✅ 完成：${zipPath}`);
