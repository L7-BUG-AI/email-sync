//! 增量同步核心：每个文件夹按 UID 跟踪，只拉新邮件。
//!
//! 同步只拉**元数据**（ENVELOPE：主题/发件人/日期），正文和附件按需补拉
//! （见 fetch_full_message）——首次全量从"下载全部内容"提速到"只拿头部"。

use anyhow::{Context, Result};
use imap_proto::types::Address;

use crate::db::{Db, MailRecord};
use crate::imap_client::ImapSession;
use crate::parse;
use crate::rfc2047::decode_rfc2047;

/// 同步单个文件夹，返回新增邮件数
pub fn sync_folder(db: &Db, session: &mut ImapSession, folder: &str) -> Result<u32> {
    let mailbox = session
        .select(folder)
        .with_context(|| format!("SELECT 文件夹失败: {folder}"))?;
    let uidvalidity = mailbox.uid_validity.unwrap_or(0);

    let folder_id = db.upsert_folder(folder, uidvalidity)?;
    let last_uid = db.get_folder(folder)?.map(|f| f.last_uid).unwrap_or(0);

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

    // 排序后分批拉取 ENVELOPE（只拿元数据，几百字节/封）
    let mut sorted: Vec<u32> = uids.into_iter().collect();
    sorted.sort_unstable();

    const BATCH_SIZE: usize = 100;
    let mut added = 0u32;
    let mut max_uid = last_uid;

    db.begin()?; // 整个文件夹一个事务，批量插入提速
    for chunk in sorted.chunks(BATCH_SIZE) {
        let seq_set = chunk
            .iter()
            .map(|u| u.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let fetches = session
            .uid_fetch(seq_set, "(UID ENVELOPE)")
            .with_context(|| format!("UID FETCH 失败: {folder}"))?;

        for fetch in fetches.iter() {
            let Some(uid) = fetch.uid else {
                continue;
            };
            let Some(env) = fetch.envelope() else {
                continue;
            };
            // 先转成拥有所有权的 String（避免引用临时值）
            let message_id = env
                .message_id
                .map(|b| decode_rfc2047(&String::from_utf8_lossy(b)));
            let subject = env
                .subject
                .map(|b| decode_rfc2047(&String::from_utf8_lossy(b)));
            let date = env.date.map(|b| String::from_utf8_lossy(b).into_owned());
            let from_addr = first_address(&env.from);
            let to_addr = first_address(&env.to);
            let record = MailRecord {
                folder_id,
                uid,
                message_id: message_id.as_deref(),
                subject: subject.as_deref(),
                from_addr: from_addr.as_deref(),
                to_addr: to_addr.as_deref(),
                date: date.as_deref(),
                body_text: None,
                body_html: None,
                zip_name: None,
                zip_data: None,
                full_body: false,
            };
            // 已存在（含历史完整数据）则跳过，只插入新的
            db.insert_meta_if_absent(&record)?;
            if uid > max_uid {
                max_uid = uid;
            }
            added += 1;
        }
    }
    db.commit()?;

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
                println!("  ✓ {folder}: 新增 {n} 封（元数据）");
                total += n;
            }
            Err(e) => eprintln!("  ✗ {folder}: {e:#}"),
        }
    }
    Ok((folders.len(), total))
}

/// 补拉一封邮件的全文（正文+附件），入库并标记 full_body=1
/// 返回更新后是否成功
pub fn fetch_full_message(
    db: &Db,
    session: &mut ImapSession,
    folder: &str,
    uid: u32,
) -> Result<bool> {
    let folder_id = db.get_folder(folder)?.map(|f| f.id);
    let Some(folder_id) = folder_id else {
        return Ok(false);
    };
    // 已在本地有完整内容则跳过
    let local_id = db.find_message(folder_id, uid)?;
    let Some(_local_id) = local_id else {
        return Ok(false);
    };

    session
        .select(folder)
        .with_context(|| format!("SELECT 文件夹失败: {folder}"))?;
    let fetches = session
        .uid_fetch(uid.to_string(), "RFC822")
        .with_context(|| format!("UID FETCH 失败: {folder}"))?;
    let Some(fetch) = fetches.first() else {
        return Ok(false);
    };
    let Some(raw) = fetch.body() else {
        return Ok(false);
    };

    let msg = parse::parse_mail(raw)?;
    let (zip_name, zip_data) = match parse::pack(&msg) {
        Some(p) => (Some(p.name), Some(p.data)),
        None => (None, None),
    };
    db.update_full_body(
        folder_id,
        uid,
        msg.body_text.as_deref(),
        msg.body_html.as_deref(),
        zip_name.as_deref(),
        zip_data.as_deref(),
    )?;
    Ok(true)
}

/// 从地址列表取第一个 "mailbox@host"（跳过空地址）
fn first_address(addrs: &Option<Vec<Address<'_>>>) -> Option<String> {
    let addrs = addrs.as_ref()?;
    for a in addrs {
        let mailbox = a.mailbox.map(|b| String::from_utf8_lossy(b).into_owned());
        let host = a.host.map(|b| String::from_utf8_lossy(b).into_owned());
        match (mailbox, host) {
            (Some(m), Some(h)) if !m.is_empty() && !h.is_empty() => {
                return Some(format!("{m}@{h}"));
            }
            (Some(m), _) if !m.is_empty() => return Some(m),
            _ => continue,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_spec_incremental() {
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

    #[test]
    fn first_address_formats() {
        // 用 imap-proto 的 Address 构造测试
        
        let empty = None;
        assert_eq!(first_address(&empty), None);

        let addrs = Some(vec![Address {
            name: None,
            adl: None,
            mailbox: Some(b"alice".as_slice()),
            host: Some(b"example.com".as_slice()),
        }]);
        assert_eq!(first_address(&addrs).unwrap(), "alice@example.com");

        let addrs2 = Some(vec![Address {
            name: None,
            adl: None,
            mailbox: Some(b"".as_slice()),
            host: Some(b"x.com".as_slice()),
        }]);
        assert_eq!(first_address(&addrs2), None);
    }
}
