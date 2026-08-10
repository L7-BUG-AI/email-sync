//! 附件打包：把邮件附件压缩成 zip 二进制（BLOB）+ 生成压缩包名。
//!
//! zip_name 规则：`attachments-<序号>.zip`（无附件返回 None）。

use std::io::Write;

/// 一个附件（文件名 + 内容）
pub struct Attachment {
    pub name: Option<String>,
    pub data: Vec<u8>,
}

/// 打包结果：压缩包名 + zip 二进制
pub struct ZipPack {
    pub name: String,
    pub data: Vec<u8>,
}

/// 把附件列表打包成 zip；无附件返回 None
pub fn pack_attachments(attachments: &[Attachment]) -> Option<ZipPack> {
    if attachments.is_empty() {
        return None;
    }
    let mut buf = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (i, att) in attachments.iter().enumerate() {
            let name = att
                .name
                .clone()
                .unwrap_or_else(|| format!("attachment-{}.bin", i + 1));
            // 防 zip slip：文件名清理掉路径分隔符，只保留末尾文件名
            let safe_name = name
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(&name)
                .to_string();
            // 重名时加序号避免覆盖
            let final_name = if i == 0 {
                safe_name
            } else {
                let dup = attachments[..i]
                    .iter()
                    .any(|a| a.name.as_deref() == Some(safe_name.as_str()));
                if dup {
                    let (stem, ext) = match safe_name.rfind('.') {
                        Some(p) => (&safe_name[..p], &safe_name[p..]),
                        None => (safe_name.as_str(), ""),
                    };
                    format!("{stem}-{}{ext}", i + 1)
                } else {
                    safe_name
                }
            };
            writer
                .start_file(final_name, opts)
                .expect("start zip entry");
            writer.write_all(&att.data).expect("write zip data");
        }
        writer.finish().expect("finish zip");
    }
    Some(ZipPack {
        name: "attachments.zip".to_string(),
        data: buf,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_attachments() -> Vec<Attachment> {
        vec![
            Attachment {
                name: Some("报告.pdf".to_string()),
                data: b"%PDF-1.4 fake pdf content".to_vec(),
            },
            Attachment {
                name: Some("photo.jpg".to_string()),
                data: b"\xff\xd8\xff\xe0 fake jpeg".to_vec(),
            },
        ]
    }

    #[test]
    fn no_attachments_returns_none() {
        assert!(pack_attachments(&[]).is_none());
    }

    #[test]
    fn packs_all_attachments() {
        let pack = pack_attachments(&sample_attachments()).unwrap();
        assert_eq!(pack.name, "attachments.zip");
        assert!(!pack.data.is_empty());
        // zip 以 PK 开头
        assert_eq!(&pack.data[..2], b"PK");
    }

    #[test]
    fn zip_is_valid_and_contains_entries() {
        let pack = pack_attachments(&sample_attachments()).unwrap();
        let reader = std::io::Cursor::new(&pack.data);
        let mut zip = zip::ZipArchive::new(reader).unwrap();
        assert_eq!(zip.len(), 2);
        let mut names: Vec<String> = (0..zip.len())
            .map(|i| zip.by_index(i).unwrap().name().to_string())
            .collect();
        names.sort();
        assert_eq!(names, vec!["photo.jpg".to_string(), "报告.pdf".to_string()]);
    }

    #[test]
    fn zip_slip_paths_sanitized() {
        let atts = vec![Attachment {
            name: Some("../../etc/passwd".to_string()),
            data: b"x".to_vec(),
        }];
        let pack = pack_attachments(&atts).unwrap();
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&pack.data)).unwrap();
        assert_eq!(zip.by_index(0).unwrap().name(), "passwd");
    }

    #[test]
    fn duplicate_names_get_suffix() {
        let atts = vec![
            Attachment {
                name: Some("a.txt".to_string()),
                data: b"1".to_vec(),
            },
            Attachment {
                name: Some("a.txt".to_string()),
                data: b"2".to_vec(),
            },
        ];
        let pack = pack_attachments(&atts).unwrap();
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&pack.data)).unwrap();
        let mut names: Vec<String> = (0..zip.len())
            .map(|i| zip.by_index(i).unwrap().name().to_string())
            .collect();
        names.sort();
        assert_eq!(names, vec!["a-2.txt".to_string(), "a.txt".to_string()]);
    }
}
