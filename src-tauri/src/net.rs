//! 网络可达性检查：git clone / pull 前的仓库主机连通性探测。
//!
//! 通过 TCP 连接仓库主机 443 端口判断网络是否可达（默认 github.com；
//! 配置了 DSH_CONTROL_PANEL_REPO_URL 镜像源时针对镜像主机探测，避免误报）。

use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::error::AppError;

/// 主机解析 + TCP 连通探测：任一解析地址在超时内连接成功即视为可达。
pub fn tcp_reachable(host: &str, port: u16) -> bool {
    let addrs: Vec<_> = match (host, port).to_socket_addrs() {
        Ok(a) => a.collect(),
        Err(_) => return false,
    };
    for addr in addrs {
        if TcpStream::connect_timeout(&addr, Duration::from_secs(3)).is_ok() {
            return true;
        }
    }
    false
}

/// 从仓库 URL 提取主机名（去掉协议、路径与端口）。
pub fn host_of_url(url: &str) -> String {
    let rest = url
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    authority
        .split(':')
        .next()
        .filter(|h| !h.is_empty())
        .unwrap_or("github.com")
        .to_string()
}

/// 仓库主机可达性检查（git clone / git pull 执行前调用）。
pub fn ensure_repo_reachable() -> Result<(), AppError> {
    let url = crate::config::repo_url();
    let host = host_of_url(&url);
    if !tcp_reachable(&host, 443) {
        return Err(AppError::NetworkUnreachable(host));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn host_extracted_from_https_url() {
        assert_eq!(
            host_of_url("https://github.com/deepseek-ai/deepseek-harness.git"),
            "github.com"
        );
        assert_eq!(host_of_url("https://github.com/"), "github.com");
        assert_eq!(host_of_url("http://127.0.0.1:4873/x.git"), "127.0.0.1");
        assert_eq!(host_of_url("  https://mirror.example.com/a/b.git "), "mirror.example.com");
    }

    #[test]
    fn host_falls_back_to_github() {
        assert_eq!(host_of_url(""), "github.com");
        assert_eq!(host_of_url("not-a-url"), "not-a-url");
    }

    #[test]
    fn tcp_reachable_detects_open_and_closed_ports() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(tcp_reachable("127.0.0.1", port));
        // 端口 1 为保留端口，用户程序无法监听，连接会被拒绝。
        assert!(!tcp_reachable("127.0.0.1", 1));
    }

    #[test]
    fn tcp_reachable_false_for_unreachable_host() {
        // 注意：不使用 no-such-host.invalid 这类主机名——运营商 DNS 劫持
        // 可能将其解析到可达 IP 导致断言失败；127.0.0.1:1 不依赖 DNS，
        // 且端口 1 为保留端口（用户程序无法监听），连接必然被拒绝。
        assert!(!tcp_reachable("127.0.0.1", 1));
    }
}
