//! 打开 PowerShell 终端（进入安装目录）。

use std::process::Command;

use crate::error::AppError;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// Windows：强制子进程新建独立控制台窗口。
///
/// 若不指定，子进程（powershell）会附着到父进程的控制台：release 构建为
/// GUI 子系统（无控制台）时虽会自动新建，但 dev 构建为 Console 子系统，
/// 会附着到控制面板所在的控制台（如从终端启动 dev 时直接混入该终端），
/// 行为不一致。显式指定后，任何模式下都弹出独立的新窗口。
#[cfg(windows)]
const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

/// 打开一个新的 PowerShell 窗口，工作目录为安装目录。
pub fn open_terminal(dir: &str) -> Result<(), String> {
    let mut cmd = Command::new("powershell.exe");
    cmd.arg("-NoExit");
    cmd.current_dir(dir);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NEW_CONSOLE);
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| AppError::Io(e.to_string()).friendly())
}
