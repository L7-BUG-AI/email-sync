//! 邮件解析：RFC822 原文 → messages 表字段（mailparse）。
//!
//! 提取：message_id / subject / from / to / date / body_text / body_html / 附件。

use anyhow::Result;
use mailparse::DispositionType;

use crate::attach::{pack_attachments, Attachment, ZipPack};

/// 解析结果（对应 messages 表字段 + 附件）
#[allow(dead_code)]
pub struct ParsedMessage {
    pub message_id: Option<String>,
    pub subject: Option<String>,
    pub from_addr: Option<String>,
    pub to_addr: Option<String>,
    pub date: Option<String>,
    pub body_text: Option<String>,
    pub body_html: Option<String>,
    pub attachments: Vec<Attachment>,
}

/// 解析邮件原文
pub fn parse_mail(raw: &[u8]) -> Result<ParsedMessage> {
    let mail = mailparse::parse_mail(raw)?;
    let mut out = ParsedMessage {
        message_id: header(&mail, "Message-ID"),
        subject: header(&mail, "Subject"),
        from_addr: header(&mail, "From"),
        to_addr: header(&mail, "To"),
        date: header(&mail, "Date"),
        body_text: None,
        body_html: None,
        attachments: Vec::new(),
    };

    // 正文：递归找 text/plain 和 text/html 子部分
    // 注意：不能依赖 mail.get_body()——对 multipart/mixed（带附件）会返回空
    if let Some(text) = find_text_body(&mail) {
        out.body_text = Some(text);
    }
    if let Some(html) = find_html_body(&mail) {
        out.body_html = Some(html);
    }

    // 附件：递归收集
    collect_attachments(&mail, &mut out.attachments);
    Ok(out)
}

/// 把解析结果打包成附件 zip（无附件返回 None）
pub fn pack(mail: &ParsedMessage) -> Option<ZipPack> {
    pack_attachments(&mail.attachments)
}

/// 取指定头字段（去重取第一个非空）
fn header(mail: &mailparse::ParsedMail<'_>, key: &str) -> Option<String> {
    mail.headers
        .iter()
        .find(|h| h.get_key().eq_ignore_ascii_case(key))
        .map(|h| h.get_value())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// 递归查找纯文本正文（text/plain 子部分）
fn find_text_body(mail: &mailparse::ParsedMail<'_>) -> Option<String> {
    if mail.ctype.mimetype == "text/plain" {
        let body = mail.get_body().ok()?;
        if !body.trim().is_empty() {
            return Some(body);
        }
    }
    for sub in &mail.subparts {
        if let Some(t) = find_text_body(sub) {
            return Some(t);
        }
    }
    None
}

/// 递归查找 HTML 正文（text/html 子部分）
fn find_html_body(mail: &mailparse::ParsedMail<'_>) -> Option<String> {
    if mail.ctype.mimetype == "text/html" {
        return mail.get_body().ok();
    }
    for sub in &mail.subparts {
        if let Some(h) = find_html_body(sub) {
            return Some(h);
        }
    }
    None
}

/// 递归收集附件（Content-Disposition: attachment）
fn collect_attachments(mail: &mailparse::ParsedMail<'_>, out: &mut Vec<Attachment>) {
    for sub in &mail.subparts {
        let is_attachment = matches!(
            sub.get_content_disposition().disposition,
            DispositionType::Attachment
        );
        if is_attachment {
            if let Ok(data) = sub.get_body_raw() {
                // 附件名：Content-Disposition filename 优先，其次 Content-Type name
                let name = sub
                    .get_content_disposition()
                    .params
                    .get("filename")
                    .or_else(|| sub.ctype.params.get("name"))
                    .cloned();
                out.push(Attachment {
                    name,
                    data: data.into_boxed_slice().into_vec(),
                });
            }
        } else if !sub.subparts.is_empty() {
            // 嵌套 multipart：递归
            collect_attachments(sub, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一封含纯文本 + HTML + 两个附件的样例邮件
    fn sample_mail() -> Vec<u8> {
        let boundary = "----=_Part_0_12345.67890";
        let inner = "----=_Part_1_99999.11111";
        format!(
            "From: Alice <alice@example.com>\r\n\
             To: Bob <bob@example.com>\r\n\
             Subject: =?UTF-8?B?5rWL6K+V5Lq65Lq6?=\r\n\
             Date: Tue, 10 Aug 2026 10:00:00 +0800\r\n\
             Message-ID: <test-123@example.com>\r\n\
             MIME-Version: 1.0\r\n\
             Content-Type: multipart/mixed; boundary=\"{boundary}\"\r\n\
             \r\n\
             --{boundary}\r\n\
             Content-Type: multipart/alternative; boundary=\"{inner}\"\r\n\
             \r\n\
             --{inner}\r\n\
             Content-Type: text/plain; charset=utf-8\r\n\
             \r\n\
             你好，这是纯文本正文。\r\n\
             --{inner}\r\n\
             Content-Type: text/html; charset=utf-8\r\n\
             \r\n\
             <html><body><b>你好</b>，这是 HTML 正文。</body></html>\r\n\
             --{inner}--\r\n\
             --{boundary}\r\n\
             Content-Type: application/pdf; name=\"report.pdf\"\r\n\
             Content-Disposition: attachment; filename=\"report.pdf\"\r\n\
             Content-Transfer-Encoding: base64\r\n\
             \r\n\
             JVBERi0xLjQgc2FtcGxl\r\n\
             --{boundary}\r\n\
             Content-Type: image/jpeg; name=\"photo.jpg\"\r\n\
             Content-Disposition: attachment; filename=\"photo.jpg\"\r\n\
             Content-Transfer-Encoding: base64\r\n\
             \r\n\
             /9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAA==\r\n\
             --{boundary}--\r\n"
        )
        .into_bytes()
    }

    #[test]
    fn parse_headers() {
        let m = parse_mail(&sample_mail()).unwrap();
        assert_eq!(m.message_id.unwrap(), "<test-123@example.com>");
        assert!(m.subject.unwrap().contains("测试"));
        assert!(m.from_addr.unwrap().contains("alice@example.com"));
        assert!(m.to_addr.unwrap().contains("bob@example.com"));
        assert!(m.date.unwrap().contains("2026"));
    }

    #[test]
    fn parse_bodies() {
        let m = parse_mail(&sample_mail()).unwrap();
        let text = m.body_text.unwrap();
        assert!(text.contains("纯文本正文"));
        let html = m.body_html.unwrap();
        assert!(html.contains("<html>"));
    }

    #[test]
    fn parse_attachments() {
        let m = parse_mail(&sample_mail()).unwrap();
        assert_eq!(m.attachments.len(), 2);
        let names: Vec<Option<&str>> = m.attachments.iter().map(|a| a.name.as_deref()).collect();
        assert!(names.contains(&Some("report.pdf")));
        assert!(names.contains(&Some("photo.jpg")));
    }

    #[test]
    fn pack_attachments_to_zip() {
        let m = parse_mail(&sample_mail()).unwrap();
        let pack = pack(&m).unwrap();
        assert_eq!(pack.name, "attachments.zip");
        assert_eq!(&pack.data[..2], b"PK");
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&pack.data)).unwrap();
        assert_eq!(zip.len(), 2);
    }
}
