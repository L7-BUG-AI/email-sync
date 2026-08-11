//! RFC2047 编码解码：=?charset?B?xxx?= / =?charset?Q?xxx?=
//! IMAP ENVELOPE 返回原始编码，需要解码成 UTF-8（中文邮件常用 GBK）。

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use encoding_rs::Encoding;

/// 解码 RFC2047 编码文本（支持 B/Q 编码 + GBK/UTF-8 等 charset）
pub fn decode_rfc2047(input: &str) -> String {
    if !input.contains("=?") {
        return input.to_string();
    }
    let mut out = String::new();
    let mut rest = input;
    while let Some(start) = rest.find("=?") {
        // 累积普通文本
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        // 格式: =?charset?encoding?data?=
        // 用 splitn 先拆 charset 和 encoding，避免 ?Q? 里的 ?= 被误判
        let after_marker = &rest[2..];
        let mut it = after_marker.splitn(3, '?');
        let (Some(charset), Some(encoding), Some(data_rest)) =
            (it.next(), it.next(), it.next())
        else {
            out.push_str(rest);
            break;
        };
        // data 到下一个 ?= 结束（B 的 padding '=' 和 Q 的 '=XX' 都不含 '?=')
        let Some(end) = data_rest.find("?=") else {
            out.push_str(rest);
            break;
        };
        let data = &data_rest[..end];
        // charset?encoding? 部分的长度（after_marker 减去 data 及其后续）
        let prefix_len = after_marker.len() - data_rest.len();
        let frag_len = 2 + prefix_len + end + 2; // =? + charset?enc? + data + ?=
        let decoded: Option<String> = match encoding.to_ascii_uppercase().as_str() {
            "B" => decode_base64_charset(data, charset),
            "Q" => decode_q_charset(data, charset),
            _ => None,
        };
        match decoded {
            Some(s) => out.push_str(&s),
            None => {
                // 解码失败：保留原文片段
                out.push_str(&rest[..frag_len]);
            }
        }
        // 跳过整个片段 =?charset?encoding?data?=
        rest = &rest[frag_len..];
    }
    out.push_str(rest);
    out
}

/// base64 解码 + charset 转 UTF-8
fn decode_base64_charset(data: &str, charset: &str) -> Option<String> {
    let bytes = STANDARD.decode(data.trim()).ok()?;
    charset_to_utf8(&bytes, charset)
}

/// Quoted-Printable 解码（_ 代表空格）+ charset 转 UTF-8
fn decode_q_charset(data: &str, charset: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(data.len());
    let mut chars = data.chars();
    while let Some(c) = chars.next() {
        if c == '=' {
            let hex: String = chars.by_ref().take(2).collect();
            if hex.len() == 2 {
                if let Ok(b) = u8::from_str_radix(&hex, 16) {
                    bytes.push(b);
                    continue;
                }
            }
            bytes.push(b'=');
        } else if c == '_' {
            bytes.push(b' ');
        } else {
            let mut buf = [0u8; 4];
            bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
    }
    charset_to_utf8(&bytes, charset)
}

/// charset 名 → encoding_rs 编码 → UTF-8 String
fn charset_to_utf8(bytes: &[u8], charset: &str) -> Option<String> {
    let enc = Encoding::for_label(charset.to_ascii_lowercase().as_bytes())?;
    let (cow, _, _) = enc.decode(bytes);
    Some(cow.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_base64_gbk() {
        // "提醒：您有 任务(29)" 的 GBK base64
        let s = "=?GBK?B?zOHQ0aO6xPrT0CDIzs7xKDI5KQ==?=";
        let out = decode_rfc2047(s);
        assert!(out.contains("任务"), "实际: {out}");
    }

    #[test]
    fn decodes_base64_utf8() {
        let s = "=?UTF-8?B?5rWL6K+V6YKu5Lu2?=";
        assert_eq!(decode_rfc2047(s), "测试邮件");
    }

    #[test]
    fn decodes_q_encoding() {
        // "=?UTF-8?Q?=E6=B5=8B=E8=AF=95?=" = "测试"
        let s = "=?UTF-8?Q?=E6=B5=8B=E8=AF=95?=";
        assert_eq!(decode_rfc2047(s), "测试");
    }

    #[test]
    fn passes_through_plain() {
        assert_eq!(decode_rfc2047("Hello World"), "Hello World");
        assert_eq!(decode_rfc2047(""), "");
    }

    #[test]
    fn decodes_mixed_text() {
        // 前缀 + 编码片段
        let s = "Re: =?UTF-8?B?5rWL6K+V6YKu5Lu2?=";
        assert_eq!(decode_rfc2047(s), "Re: 测试邮件");
    }

    #[test]
    fn decodes_multiple_fragments() {
        let s = "=?UTF-8?B?5rWL6K+V?= =?UTF-8?B?6YKu5Lu2?=";
        assert_eq!(decode_rfc2047(s), "测试 邮件");
    }
}
