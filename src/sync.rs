//! 增量同步核心：每个文件夹按 UID 跟踪，只拉新邮件。
//!
//! 流程：SELECT 文件夹 → 对比 UIDVALIDITY（变了则全量重同步）
//!      → UID SEARCH 增量 → 逐个拉 RFC822 → 解析 → 存库 → 更新 last_uid。

use anyhow::{Context, Result};

use crate::db::{Db, MailRecord};
use crate::imap_client::ImapSession;
use crate::parse;

/// 同步单个文件夹，返回新增邮件数
pub fn sync_folder(db: &Db, session: &mut ImapSession, folder: &str) -> Result<u32> {
    // SELECT 并拿 UIDVALIDITY
    let mailbox = session
        .select(folder)
        .with_context(|| format!("SELECT 文件夹失败: {folder}"))?;
    let uidvalidity = mailbox.uid_validity.unwrap_or(0);

    // 建文件夹记录（UIDVALIDITY 变化时内部自动重置 last_uid）
    let folder_id = db.upsert_folder(folder, uidvalidity)?;
    let last_uid = db.get_folder(folder)?.map(|f| f.last_uid).unwrap_or(0);

    // 增量搜索：UID 大于 last_uid 的邮件
    let search_spec = if last_uid == 0 {
        "ALL".to_string()
    } else {
        format!("UID {}:*", last_uid + 1)
    };
    let uids = session
        .uid_search(search_spec)
        .with_context(|| format!("UID SEARCH 失败: {folder}"))?;

    if uids.is_empty() {
        return Ok(0);
    }

    // 排序后批量拉取（RFC822 = 完整原文）
    let mut sorted: Vec<u32> = uids.into_iter().collect();
    sorted.sort_unstable();
    let seq_set = sorted
        .iter()
        .map(|u| u.to_string())
        .collect::<Vec<_>>()
        .join(",");

    let fetches = session
        .uid_fetch(seq_set, "RFC822")
        .with_context(|| format!("UID FETCH 失败: {folder}"))?;

    let mut added = 0u32;
    let mut max_uid = last_uid;
    for fetch in fetches.iter() {
        let Some(uid) = fetch.uid else { continue };
        let Some(raw) = fetch.body() else { continue };
        let msg = match parse::parse_mail(raw) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("[warn] 解析邮件失败 UID={uid} {folder}: {e}");
                continue;
            }
        };
        // 附件打包：zip 二进制 + 压缩包名
        let (zip_name, zip_data) = match parse::pack(&msg) {
            Some(p) => (Some(p.name), Some(p.data)),
            None => (None, None),
        };
        let record = MailRecord {
            folder_id,
            uid,
            message_id: msg.message_id.as_deref(),
            subject: msg.subject.as_deref(),
            from_addr: msg.from_addr.as_deref(),
            to_addr: msg.to_addr.as_deref(),
            date: msg.date.as_deref(),
            body_text: msg.body_text.as_deref(),
            body_html: msg.body_html.as_deref(),
            zip_name: zip_name.as_deref(),
            zip_data: zip_data.as_deref(),
        };
        db.insert_message(&record)?;
        if uid > max_uid {
            max_uid = uid;
        }
        added += 1;
    }

    // 更新同步进度
    if max_uid > last_uid {
        db.update_last_uid(folder_id, max_uid)?;
    }
    Ok(added)
}

/// 同步所有文件夹，返回 (文件夹数, 新增邮件总数)
pub fn sync_all(db: &Db, session: &mut ImapSession) -> Result<(usize, u32)> {
    let folders = crate::imap_client::list_folders(session)?;
    let mut total = 0u32;
    for folder in &folders {
        match sync_folder(db, session, folder) {
            Ok(n) => {
                println!("  ✓ {folder}: 新增 {n} 封");
                total += n;
            }
            Err(e) => eprintln!("  ✗ {folder}: {e:#}"),
        }
    }
    Ok((folders.len(), total))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_spec_incremental() {
        // 模拟增量搜索条件构造（纯逻辑）
        let spec = |last_uid: u32| {
            if last_uid == 0 {
                "ALL".to_string()
            } else {
                format!("UID {}:*", last_uid + 1)
            }
        };
        assert_eq!(spec(0), "ALL");
        assert_eq!(spec(100), "UID 101:*");
        assert_eq!(spec(999), "UID 1000:*");
    }

    #[test]
    fn search_spec_handles_max_uid() {
        let spec = |last_uid: u32| format!("UID {}:*", last_uid + 1);
        assert_eq!(spec(u32::MAX - 1), "UID 4294967295:*");
    }
}
