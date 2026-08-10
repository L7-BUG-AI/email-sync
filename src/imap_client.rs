//! IMAP 连接与文件夹枚举。
//!
//! 连接真实服务器，此模块以编译 + 错误路径测试为主；
//! 纯逻辑函数（解析 LIST 响应）有单元测试。

use anyhow::{Context, Result};
use imap::types::{Name, NameAttribute};
use imap::Session;
use native_tls::TlsConnector;

use crate::config::Config;

/// 已登录的 IMAP 会话类型（TLS 连接）
pub type ImapSession = Session<native_tls::TlsStream<std::net::TcpStream>>;

/// 连接并登录 IMAP 服务器
pub fn connect(cfg: &Config) -> Result<ImapSession> {
    let tls = TlsConnector::builder()
        .build()
        .context("构建 TLS 连接器失败")?;
    let client = imap::connect((cfg.server.as_str(), cfg.port), &cfg.server, &tls)
        .with_context(|| format!("连接 IMAP 服务器 {}:{} 失败", cfg.server, cfg.port))?;
    let session = client
        .login(&cfg.email, &cfg.auth_code)
        .map_err(|(e, _client)| {
            anyhow::anyhow!("登录失败（检查邮箱与授权码）: {}: {e}", cfg.email)
        })?;
    Ok(session)
}

/// 列出所有可选文件夹（跳过 \Noselect 虚拟节点）
pub fn list_folders(session: &mut ImapSession) -> Result<Vec<String>> {
    let names = session.list(Some(""), Some("*")).context("LIST 请求失败")?;
    Ok(names
        .into_iter()
        .filter(|n| !is_noselect(n))
        .map(|n| n.name().to_string())
        .collect())
}

/// 判断 Name 是否带 \Noselect 属性
/// 注意：有的服务器发 `\NoSelect`（大写 S），会被解析成 Custom 变体，需兼容
fn is_noselect(name: &Name) -> bool {
    name.attributes().iter().any(|a| match a {
        NameAttribute::NoSelect => true,
        NameAttribute::Custom(s) => {
            s.eq_ignore_ascii_case("\\NoSelect") || s.eq_ignore_ascii_case("\\Noselect")
        }
        _ => false,
    })
}

/// 从 LIST 原始响应行解析文件夹名（纯逻辑，测试用）
/// 输入示例：`* LIST (\HasNoChildren) "/" "INBOX"`
#[allow(dead_code)] // 测试辅助，保留
pub fn parse_list_line(line: &str) -> Option<String> {
    // 跳过 * 和 LIST
    let rest = line
        .trim_start_matches('*')
        .trim()
        .strip_prefix("LIST")?
        .trim();
    // 属性部分 (…) 可能不存在
    let rest = rest.strip_prefix('(').map(|r| {
        // 跳过属性括号
        let end = r.find(')')?;
        Some(&r[end + 1..])
    })??;
    let rest = rest.trim();
    // 分隔符（引号或 NIL）后是文件夹名（可能带引号）
    let name_part = rest.split_once(char::is_whitespace)?.1.trim();
    Some(name_part.trim_matches('"').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_list_line_basic() {
        let line = "* LIST (\\HasNoChildren) \"/\" \"INBOX\"";
        assert_eq!(parse_list_line(line).unwrap(), "INBOX");
    }

    #[test]
    fn parse_list_line_with_flags_and_quotes() {
        let line = "* LIST (\\Noselect \\HasChildren) \"/\" \"[Gmail]\"";
        assert_eq!(parse_list_line(line).unwrap(), "[Gmail]");
    }

    #[test]
    fn parse_list_line_unquoted() {
        let line = "* LIST () \"/\" Sent";
        assert_eq!(parse_list_line(line).unwrap(), "Sent");
    }

    #[test]
    fn parse_list_line_invalid_returns_none() {
        assert!(parse_list_line("garbage").is_none());
        assert!(parse_list_line("* NO").is_none());
    }
}
