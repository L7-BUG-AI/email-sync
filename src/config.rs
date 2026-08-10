//! 配置解析：IMAP 服务器、邮箱、授权码。
//!
//! 读取 `~/.config/email-sync/config.txt`（或 `--config` 指定）。
//! 首次运行自动生成带注释的模板。纯 std，不引 serde。

use std::fs;
use std::path::Path;

/// 邮件同步配置
#[derive(Debug, Clone)]
pub struct Config {
    pub server: String,
    pub port: u16,
    pub email: String,
    pub auth_code: String,
}

/// 默认配置文件路径：Linux `~/.config/email-sync/config.txt`
pub fn default_config_path() -> std::path::PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        Path::new(&home)
            .join(".config")
            .join("email-sync")
            .join("config.txt")
    } else {
        Path::new("config.txt").to_path_buf()
    }
}

impl Config {
    /// 从文件加载；文件不存在时自动生成模板并返回带指引的错误。
    pub fn load(path: &Path) -> anyhow::Result<Config> {
        if !path.exists() {
            write_template(path)?;
            anyhow::bail!(
                "配置文件不存在，已生成模板：{}\n请填入 IMAP 授权码后重新运行。",
                path.display()
            );
        }
        let text = fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("读取配置失败 {}: {e}", path.display()))?;
        Self::parse(&text)
    }

    /// 解析配置文本（每行 key=value，# 注释，空行忽略）
    pub fn parse(text: &str) -> anyhow::Result<Config> {
        let mut server = None;
        let mut port = 993u16;
        let mut email = None;
        let mut auth_code = None;

        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let k = k.trim().to_ascii_lowercase();
            let v = v.trim();
            match k.as_str() {
                "server" => server = Some(v.to_string()),
                "port" => {
                    port = v
                        .parse()
                        .map_err(|_| anyhow::anyhow!("port 不是有效数字: {v}"))?
                }
                "email" => email = Some(v.to_string()),
                "auth_code" => auth_code = Some(v.to_string()),
                _ => {}
            }
        }

        let server = server.ok_or_else(|| anyhow::anyhow!("配置缺少 server 字段"))?;
        let email = email.ok_or_else(|| anyhow::anyhow!("配置缺少 email 字段"))?;
        let auth_code = auth_code.ok_or_else(|| anyhow::anyhow!("配置缺少 auth_code 字段"))?;

        Ok(Config {
            server,
            port,
            email,
            auth_code,
        })
    }
}

/// 生成配置模板（带注释说明）
pub fn write_template(path: &Path) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let template = "\
# email-sync 配置模板
# 每行 key=value，'#' 开头为注释。填写后保存，重新运行即可。

# IMAP 服务器地址（企业微信邮箱: imap.exmail.qq.com / QQ: imap.qq.com / 163: imap.163.com）
server=imap.exmail.qq.com

# IMAP 端口（SSL 默认 993）
port=993

# 邮箱地址
email=

# IMAP 授权码（不是邮箱密码！网页设置里开启 IMAP 服务后生成）
auth_code=
";
    fs::write(path, template)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_normal_config() {
        let cfg = Config::parse(
            "# 注释\nserver=imap.qq.com\nport=993\nemail=a@qq.com\nauth_code=abcdefgh12345678\n",
        )
        .unwrap();
        assert_eq!(cfg.server, "imap.qq.com");
        assert_eq!(cfg.port, 993);
        assert_eq!(cfg.email, "a@qq.com");
        assert_eq!(cfg.auth_code, "abcdefgh12345678");
    }

    #[test]
    fn parse_missing_field_errors() {
        assert!(Config::parse("server=imap.qq.com\nemail=a@qq.com\n").is_err());
        assert!(Config::parse("").is_err());
    }

    #[test]
    fn parse_case_insensitive_keys() {
        let cfg = Config::parse("SERVER=imap.qq.com\nEMAIL=a@qq.com\nAUTH_CODE=xxx\n").unwrap();
        assert_eq!(cfg.server, "imap.qq.com");
        assert_eq!(cfg.auth_code, "xxx");
    }

    #[test]
    fn parse_invalid_port_errors() {
        assert!(Config::parse("server=x\nemail=a@b\nport=notnum\nauth_code=xxx\n").is_err());
    }

    #[test]
    fn template_roundtrip() {
        let path = std::env::temp_dir().join(format!("es-config-{}", std::process::id()));
        write_template(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("server="));
        assert!(text.contains("auth_code="));
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }
}
