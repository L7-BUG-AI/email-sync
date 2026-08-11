//! 附件打包：多附件 tar 打包 + zstd level 1 压缩（tar.zst）。
//!
//! 产出 `attachments-<序号>.tar.zst`（无附件返回 None）。
//! 压缩等级 1 = 速度优先（邮件附件中小文件场景）。

use std::io::Cursor;
use std::sync::atomic::{AtomicU32, Ordering};

/// zstd 压缩等级（1 = 最快，速度优先）
pub const ZSTD_LEVEL: i32 = 1;

/// 一个附件（文件名 + 内容）
pub struct Attachment {
    pub name: Option<String>,
    pub data: Vec<u8>,
}

/// 打包结果
pub struct PackedAtt {
    /// 压缩包名（如 attachments-000123.tar.zst）
    pub name: String,
    /// tar.zst 压缩二进制
    pub data: Vec<u8>,
}

/// 全局序号（文件名去重用）
static SEQ: AtomicU32 = AtomicU32::new(0);

/// 把附件列表打包成 tar.zst；无附件返回 None
pub fn pack_attachments(atts: &[Attachment]) -> Option<PackedAtt> {
    if atts.is_empty() {
        return None;
    }
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let name = format!("attachments-{n:06}.tar.zst");

    // 1) tar 打包（文件名清洗防路径穿越，zip slip 思路沿用）
    let mut tar_buf = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut tar_buf);
        for att in atts {
            let file_name = clean_name(&att.name);
            let mut header = tar::Header::new_gnu();
            header.set_size(att.data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, file_name, Cursor::new(&att.data))
                .expect("tar append");
        }
        builder.finish().expect("tar finish");
    }

    // 2) zstd level 1 压缩
    let compressed = zstd::stream::encode_all(Cursor::new(&tar_buf), ZSTD_LEVEL)
        .expect("zstd encode");

    Some(PackedAtt { name, data: compressed })
}

/// 清洗附件名：去掉路径分隔符和非法字符（防 tar 路径穿越）
fn clean_name(name: &Option<String>) -> String {
    let raw = name.as_deref().unwrap_or("attachment");
    // 去路径：只保留最后一段（反斜杠/正斜杠都算分隔）
    let base = raw
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(raw)
        .trim();
    if base.is_empty() {
        "attachment".to_string()
    } else {
        base.to_string()
    }
}

/// 解压并列出 tar.zst 内容（Web UI 展示/下载用，测试用）
pub fn list_archive(data: &[u8]) -> Vec<String> {
    let mut decoder = zstd::stream::read::Decoder::new(Cursor::new(data))
        .expect("zstd decode");
    let mut archive = tar::Archive::new(&mut decoder);
    archive
        .entries()
        .expect("tar entries")
        .filter_map(|e| e.ok())
        .map(|e| e.path().map(|p| p.display().to_string()).unwrap_or_default())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_attachments() -> Vec<Attachment> {
        vec![
            Attachment {
                name: Some("report.pdf".to_string()),
                data: b"%PDF-1.4 fake pdf content".to_vec(),
            },
            Attachment {
                name: Some("photo.jpg".to_string()),
                data: vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10],
            },
        ]
    }

    #[test]
    fn packs_and_lists_tar_zst() {
        let packed = pack_attachments(&sample_attachments()).expect("pack");
        assert!(packed.name.ends_with(".tar.zst"));
        // zstd magic: 28 B5 2F FD
        assert_eq!(&packed.data[..4], &[0x28, 0xb5, 0x2f, 0xfd], "zstd magic");
        let names = list_archive(&packed.data);
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"report.pdf".to_string()));
        assert!(names.contains(&"photo.jpg".to_string()));
    }

    #[test]
    fn returns_none_for_empty() {
        assert!(pack_attachments(&[]).is_none());
    }

    #[test]
    fn strips_path_traversal() {
        // zip slip 防护：../ 和绝对路径都要被清洗
        let atts = vec![Attachment {
            name: Some("../../etc/passwd".to_string()),
            data: b"evil".to_vec(),
        }];
        let packed = pack_attachments(&atts).expect("pack");
        let names = list_archive(&packed.data);
        assert_eq!(names.len(), 1);
        assert_eq!(names[0], "passwd", "路径穿越被清洗");
    }

    #[test]
    fn handles_duplicate_names() {
        // 同名附件：tar 允许重复条目，不 panic
        let atts = vec![
            Attachment {
                name: Some("a.txt".to_string()),
                data: b"one".to_vec(),
            },
            Attachment {
                name: Some("a.txt".to_string()),
                data: b"two".to_vec(),
            },
        ];
        let packed = pack_attachments(&atts).expect("pack");
        assert_eq!(list_archive(&packed.data).len(), 2);
    }

    #[test]
    fn handles_chinese_names() {
        let atts = vec![Attachment {
            name: Some("发票.pdf".to_string()),
            data: b"invoice".to_vec(),
        }];
        let packed = pack_attachments(&atts).expect("pack");
        let names = list_archive(&packed.data);
        assert_eq!(names[0], "发票.pdf");
    }

    #[test]
    fn zstd_level_is_one() {
        assert_eq!(ZSTD_LEVEL, 1);
    }
}
