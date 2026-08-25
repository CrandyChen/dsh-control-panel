// 安装指引 HTML 生成器（中 / 英双语）。
//
// 生成自包含的 HTML 文档（内联 CSS/JS），以 blob URL 形式在新 tab 打开：
// - 仅用于源码安装模式：必装项 Git 缺失或版本过低 → 醒目红色卡片，给出官网下载链接与命令行安装方式；
// - 预构建内核模式无需任何外部环境，不触发本指引；
// - 外链带 data-external 属性，内联脚本拦截点击并向父窗口 postMessage，
//   由控制面板调用 opener 插件在系统浏览器打开。

import type { Lang } from "./i18n";
import type { ToolStatus } from "./types";

interface ToolMeta {
  name: string;
  site: string;
  siteLabel: string;
  commands: { label: string; cmd: string }[];
  note?: string;
}

interface GuideStrings {
  htmlLang: string;
  title: string;
  sub: string;
  footer: string;
  okBadge: (v: string) => string;
  lowBadge: (v: string) => string;
  missingRequiredBadge: string;
  missingOptionalBadge: string;
  requiredBadge: string;
  optionalBadge: string;
  introRequired: string;
  introOptional: string;
  siteLink: (label: string) => string;
  copy: string;
  copied: string;
  tools: Record<string, ToolMeta>;
}

const zh: GuideStrings = {
  htmlLang: "zh-CN",
  title: "运行环境安装指引（Git）",
  sub: "DSH Control Panel 检测到以下运行环境状态。源码安装 DeepSeek Harness 需要 Git；Node.js 与 pnpm 已内置在控制面板中，无需单独安装。",
  footer:
    "提示：安装完成后，请重新打开控制面板（或点击主界面的「刷新」）重新检测。若仍有问题，可查看「运行日志」了解详细报错。",
  okBadge: (v) => `✓ 已安装（${v}）`,
  lowBadge: (v) => `⚠ 版本过低（${v}）`,
  missingRequiredBadge: "✗ 未安装",
  missingOptionalBadge: "○ 未安装（可选）",
  requiredBadge: "必装",
  optionalBadge: "推荐（可选）",
  introRequired: "完成安装后，重新打开控制面板（或点击「刷新」）即可继续安装。",
  introOptional: "此项为可选项：缺失不阻塞安装，建议按需安装。",
  siteLink: (label) => `前往 ${label} 下载安装包`,
  copy: "复制",
  copied: "已复制",
  tools: {
    git: {
      name: "Git",
      site: "https://git-scm.com/download/win",
      siteLabel: "Git 官网下载",
      commands: [
        { label: "winget 安装（推荐）", cmd: "winget install --id Git.Git -e --source winget" },
        { label: "验证安装", cmd: "git --version" },
      ],
      note: "源码安装与更新 DeepSeek Harness 依赖 Git 拉取代码。",
    },
  },
};

const en: GuideStrings = {
  htmlLang: "en",
  title: "Runtime Setup Guide (Git)",
  sub: "DSH Control Panel detected the following runtime state. Installing DeepSeek Harness from source requires Git; Node.js and pnpm are bundled with the control panel, so no separate install is needed.",
  footer:
    "Tip: after finishing, reopen the control panel (or click \"Refresh\" on the home view) to re-check. If issues persist, see the \"Log\" for details.",
  okBadge: (v) => `✓ Installed (${v})`,
  lowBadge: (v) => `⚠ Version too low (${v})`,
  missingRequiredBadge: "✗ Not installed",
  missingOptionalBadge: "○ Not installed (optional)",
  requiredBadge: "Required",
  optionalBadge: "Recommended (optional)",
  introRequired: "After completing the install, reopen the control panel (or click \"Refresh\") to continue.",
  introOptional: "This item is optional: installing it is not required to proceed.",
  siteLink: (label) => `Visit ${label} to download the installer`,
  copy: "Copy",
  copied: "Copied",
  tools: {
    git: {
      name: "Git",
      site: "https://git-scm.com/download/win",
      siteLabel: "the Git download page",
      commands: [
        { label: "Install via winget (recommended)", cmd: "winget install --id Git.Git -e --source winget" },
        { label: "Verify installation", cmd: "git --version" },
      ],
      note: "Installing and updating DeepSeek Harness from source relies on Git to fetch code.",
    },
  },
};

/** HTML 转义，避免工具名/版本中的特殊字符破坏页面结构。 */
function esc(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function statusBadge(t: ToolStatus, g: GuideStrings): string {
  if (t.installed && !t.ok) return g.lowBadge(esc(t.version ?? ""));
  if (t.installed) return g.okBadge(esc(t.version ?? ""));
  return t.required ? g.missingRequiredBadge : g.missingOptionalBadge;
}

function toolCard(t: ToolStatus, meta: ToolMeta, g: GuideStrings): string {
  const missing = !t.installed;
  const low = t.installed && !t.ok;
  // 未安装 / 版本过低才需要完整指引卡片；已满足则展示简洁状态行。
  if (!missing && !low) {
    return `
      <div class="card ok">
        <div class="card-head"><span class="tool-name">${esc(meta.name)}</span><span class="badge ok">${g.okBadge(esc(t.version ?? ""))}</span></div>
      </div>`;
  }
  const kind = missing ? "missing" : "warn";
  const requiredBadge = t.required
    ? `<span class="badge required">${g.requiredBadge}</span>`
    : `<span class="badge optional">${g.optionalBadge}</span>`;
  const status = statusBadge(t, g);
  const commands = meta.commands
    .map(
      (c) => `
        <div class="cmd-row">
          <div class="cmd-label">${esc(c.label)}</div>
          <div class="cmd-box">
            <code>${esc(c.cmd)}</code>
            <button class="copy-btn" onclick="copyCmd(this)">${g.copy}</button>
          </div>
        </div>`,
    )
    .join("");
  const note = meta.note ? `<div class="note">${esc(meta.note)}</div>` : "";
  const detail = t.detail ? `<div class="note warn">${esc(t.detail)}</div>` : "";
  return `
    <div class="card ${kind}">
      <div class="card-head">
        <span class="tool-name">${esc(meta.name)}</span>
        ${requiredBadge}
        <span class="badge ${t.installed ? "warn" : "missing"}">${status}</span>
      </div>
      <p class="intro">
        ${t.required ? g.introRequired : g.introOptional}
      </p>
      <a class="site-link" data-external href="${esc(meta.site)}" rel="noopener">${esc(g.siteLink(meta.siteLabel))}</a>
      ${commands}
      ${note}
      ${detail}
    </div>`;
}

/** 生成完整的安装指引 HTML 文档（以 blob URL 形式在新 tab 打开）。 */
export function buildInstallGuideHtml(tools: ToolStatus[], lang: Lang): string {
  const g = lang === "zh-CN" ? zh : en;
  const order = ["git"];
  const sorted = [...tools].sort((a, b) => {
    const ia = order.indexOf(a.id);
    const ib = order.indexOf(b.id);
    return (ia === -1 ? 99 : ia) - (ib === -1 ? 99 : ib);
  });
  const cards = sorted
    .map((t) =>
      toolCard(
        t,
        g.tools[t.id] ?? {
          name: t.name,
          site: "https://github.com",
          siteLabel: t.name,
          commands: [],
        },
        g,
      ),
    )
    .join("\n");

  return `<!DOCTYPE html>
<html lang="${g.htmlLang}">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${esc(g.title)} · DSH Control Panel</title>
<style>
  :root { color-scheme: dark; }
  * { box-sizing: border-box; }
  body {
    margin: 0;
    background: #141414;
    color: #e6e6e6;
    font-family: "Segoe UI", "Microsoft YaHei", "PingFang SC", sans-serif;
    line-height: 1.7;
  }
  .wrap { max-width: 860px; margin: 0 auto; padding: 28px 20px 48px; }
  h1 { font-size: 22px; margin: 0 0 4px; }
  .sub { color: #a6a6a6; font-size: 13px; margin-bottom: 20px; }
  .card {
    border: 1px solid #303030;
    border-radius: 10px;
    background: #1d1d1d;
    padding: 16px 18px;
    margin-bottom: 14px;
  }
  .card.ok { border-color: #274a2e; }
  .card.warn { border-left: 4px solid #d89614; }
  .card.missing { border-left: 4px solid #d32029; }
  .card-head { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; margin-bottom: 6px; }
  .tool-name { font-size: 16px; font-weight: 600; }
  .badge {
    font-size: 12px;
    padding: 1px 10px;
    border-radius: 999px;
    border: 1px solid #434343;
    color: #c9c9c9;
  }
  .badge.ok { color: #49aa19; border-color: #274a2e; }
  .badge.warn { color: #d89614; border-color: #5a4a1a; }
  .badge.missing { color: #e84749; border-color: #5a2326; }
  .badge.required { color: #e84749; border-color: #5a2326; }
  .badge.optional { color: #1677ff; border-color: #15395b; }
  .intro { margin: 4px 0 10px; font-size: 13px; color: #b8b8b8; }
  .site-link {
    display: inline-block;
    color: #1677ff;
    text-decoration: none;
    font-size: 14px;
    margin-bottom: 10px;
  }
  .site-link:hover { text-decoration: underline; }
  .cmd-row { margin-bottom: 8px; }
  .cmd-label { font-size: 12px; color: #9a9a9a; margin-bottom: 3px; }
  .cmd-box {
    display: flex;
    align-items: center;
    gap: 8px;
    background: #111;
    border: 1px solid #2a2a2a;
    border-radius: 6px;
    padding: 6px 8px;
  }
  .cmd-box code {
    flex: 1;
    font-family: Consolas, "Courier New", monospace;
    font-size: 12.5px;
    color: #d4d4d4;
    word-break: break-all;
    white-space: pre-wrap;
  }
  .copy-btn {
    background: #262626;
    color: #c9c9c9;
    border: 1px solid #434343;
    border-radius: 5px;
    font-size: 12px;
    padding: 2px 10px;
    cursor: pointer;
    flex-shrink: 0;
  }
  .copy-btn:hover { border-color: #1677ff; color: #fff; }
  .note { font-size: 12.5px; color: #9a9a9a; margin-top: 8px; }
  .note.warn { color: #d89614; }
  .footer {
    margin-top: 18px;
    font-size: 12px;
    color: #7a7a7a;
    border-top: 1px solid #2a2a2a;
    padding-top: 12px;
  }
</style>
</head>
<body>
<div class="wrap">
  <h1>${esc(g.title)}</h1>
  <div class="sub">${esc(g.sub)}</div>
  ${cards}
  <div class="footer">
    ${esc(g.footer)}
  </div>
</div>
<script>
  var COPY = ${JSON.stringify(g.copy)};
  var COPIED = ${JSON.stringify(g.copied)};
  // 外部链接：交给控制面板在系统浏览器打开（父窗口监听 dsh-open-external 消息）。
  document.addEventListener("click", function (e) {
    var a = e.target && e.target.closest ? e.target.closest("a[data-external]") : null;
    if (a) {
      e.preventDefault();
      window.parent.postMessage({ type: "dsh-open-external", url: a.href }, "*");
    }
  });
  function copyCmd(btn) {
    var code = btn.parentElement.querySelector("code").textContent;
    var done = function () {
      btn.textContent = COPIED;
      setTimeout(function () { btn.textContent = COPY; }, 1500);
    };
    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(code).then(done, function () { fallbackCopy(code, btn, done); });
    } else {
      fallbackCopy(code, btn, done);
    }
  }
  function fallbackCopy(text, btn, done) {
    var ta = document.createElement("textarea");
    ta.value = text;
    document.body.appendChild(ta);
    ta.select();
    try { document.execCommand("copy"); done(); } catch (err) { /* ignore */ }
    document.body.removeChild(ta);
  }
</script>
</body>
</html>`;
}
