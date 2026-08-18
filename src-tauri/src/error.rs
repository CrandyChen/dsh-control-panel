//! 错误类型与友好提示映射（按当前界面语言输出中文 / 英文）。

use crate::i18n;

#[derive(Debug)]
pub enum AppError {
    ProgramNotFound(String),
    StepFailed { step: String, exit_code: i32 },
    Network(String),
    /// 仓库主机（默认 github.com）不可达：clone / pull 前预检失败。
    NetworkUnreachable(String),
    NotInstalled,
    AlreadyInstalled(String),
    PortInUse(u16),
    InvalidPath(String),
    NotValidInstall(String),
    NotInPreview(String),
    Io(String),
}

impl AppError {
    /// 面向用户的友好提示（按当前界面语言）。
    pub fn friendly(&self) -> String {
        match i18n::get_lang() {
            i18n::Lang::Zh => self.friendly_zh(),
            i18n::Lang::En => self.friendly_en(),
        }
    }

    fn friendly_zh(&self) -> String {
        match self {
            AppError::ProgramNotFound(p) => {
                format!("未找到程序「{p}」。请确认已安装 Git / pnpm 且已加入 PATH，然后重试。")
            }
            AppError::StepFailed { step, exit_code } => {
                format!("步骤「{step}」执行失败（退出码 {exit_code}）。请查看下方日志了解详细原因。")
            }
            AppError::Network(msg) => format!("网络错误：{msg}。请检查网络连接后重试。"),
            AppError::NetworkUnreachable(host) => format!(
                "网络不可达：无法连接 {host}:443。请检查网络连接、代理或 VPN 设置，确保能访问 GitHub 后重试。"
            ),
            AppError::NotInstalled => {
                "尚未安装 DeepSeek Harness，请先点击「安装」完成安装。".to_string()
            }
            AppError::AlreadyInstalled(dir) => {
                format!("{dir} 已是有效的 DeepSeek Harness 安装目录，请使用「更新」功能。")
            }
            AppError::PortInUse(port) => {
                format!("端口 {port} 已被其他程序占用。请先关闭占用该端口的程序（可能是已运行的 DeepSeek Harness）。")
            }
            AppError::InvalidPath(p) => format!("路径不合法，已拒绝操作：{p}"),
            AppError::NotValidInstall(p) => {
                format!("{p} 不是有效的 DeepSeek Harness 安装（需包含 .git 与 @deepseek-ai/dsh-root 标识）。GitHub zip 解压的目录无法更新，不在支持范围。")
            }
            AppError::NotInPreview(p) => format!("路径不在卸载清单内，已拒绝删除：{p}"),
            AppError::Io(msg) => format!("文件操作失败：{msg}"),
        }
    }

    fn friendly_en(&self) -> String {
        match self {
            AppError::ProgramNotFound(p) => format!(
                "Program not found: {p}. Make sure Git / pnpm are installed and on PATH, then retry."
            ),
            AppError::StepFailed { step, exit_code } => format!(
                "Step \"{step}\" failed (exit code {exit_code}). See the log below for details."
            ),
            AppError::Network(msg) => format!("Network error: {msg}. Check your network and retry."),
            AppError::NetworkUnreachable(host) => format!(
                "Network unreachable: cannot connect to {host}:443. Check your network, proxy or VPN settings so GitHub is accessible, then retry."
            ),
            AppError::NotInstalled => {
                "DeepSeek Harness is not installed yet. Click \"Install\" to install it first."
                    .to_string()
            }
            AppError::AlreadyInstalled(dir) => format!(
                "{dir} is already a valid DeepSeek Harness installation. Use \"Update\" instead."
            ),
            AppError::PortInUse(port) => format!(
                "Port {port} is already in use by another program (possibly a running DeepSeek Harness). Close the program using it first."
            ),
            AppError::InvalidPath(p) => format!("Invalid path, operation rejected: {p}"),
            AppError::NotValidInstall(p) => format!(
                "{p} is not a valid DeepSeek Harness installation (requires .git and the @deepseek-ai/dsh-root marker). Directories extracted from GitHub zip cannot be updated and are not supported."
            ),
            AppError::NotInPreview(p) => format!("Path is not in the uninstall list, deletion rejected: {p}"),
            AppError::Io(msg) => format!("File operation failed: {msg}"),
        }
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.friendly())
    }
}

impl std::error::Error for AppError {}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::Io(e.to_string())
    }
}
