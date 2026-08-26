//! 运行环境工具检测。
//!
//! 源码安装（source）的 git 操作由内置 git2 (libgit2) 完成，不再依赖外部 Git；
//! 运行时 node/pnpm 在安装/启动时按需下载。因此本模块不返回任何必装项，
//! 安装不会被「缺 Git」拦截。保留该结构以便将来新增可检测的运行时工具。

use serde::Serialize;

/// 单个工具的检测结果（序列化后字段为 camelCase）。
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolStatus {
    /// 工具标识。
    pub id: String,
    /// 展示名称。
    pub name: String,
    /// 是否已安装。
    pub installed: bool,
    /// 检测到的版本。
    pub version: Option<String>,
    /// 版本是否满足最低要求。
    pub ok: bool,
    /// 是否必装。
    pub required: bool,
    /// 不满足最低要求时的人类可读说明。
    pub detail: Option<String>,
}

/// 检测运行环境工具。现无任何外部必装项（git 由内置 libgit2 完成，node/pnpm 按需下载），
/// 返回空数组，安装流程不会被拦截。
pub fn detect_tools(_mode: &str) -> Vec<ToolStatus> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_tools_require_nothing_external() {
        // 无论哪种模式，都不再要求外部 git / node / pnpm。
        assert!(detect_tools("source").is_empty());
        assert!(detect_tools("prebuilt").is_empty());
    }
}
