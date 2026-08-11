//! SQLite 存储：folders（同步进度）+ messages（邮件内容 + 附件 zip BLOB）。
//! 用户用数据库工具直接查询本库。

use anyhow::Result;
use rusqlite::Connection;
use serde::Serialize;

/// 文件夹同步进度
#[allow(dead_code)] // id/uidvalidity 供后续功能（状态展示/重置检测）使用
#[derive(Debug, Clone)]
pub struct FolderState {
    pub id: i64,
    pub uidvalidity: u32,
    pub last_uid: u32,
}

/// 邮件记录（入库用）
#[allow(dead_code)] // 部分字段/方法供后续功能使用
pub struct MailRecord<'a> {
    pub folder_id: i64,
    pub uid: u32,
    pub message_id: Option<&'a str>,
    pub subject: Option<&'a str>,
    pub from_addr: Option<&'a str>,
    pub to_addr: Option<&'a str>,
    pub date: Option<&'a str>,
    /// 正文（已 zstd 压缩字节）
    pub body_text: Option<&'a [u8]>,
    pub body_html: Option<&'a [u8]>,
    pub att_name: Option<&'a str>,
    pub att_data: Option<&'a [u8]>,
    /// false = 仅元数据（ENVELOPE），true = 含正文/附件全文
    pub full_body: bool,
}

pub struct Db {
    conn: Connection,
}

impl Db {
    /// 打开（或创建）数据库并初始化表结构（幂等）
    pub fn open(path: &str) -> Result<Db> {
        let conn = Connection::open(path)?;
        // 并发写（Web 补拉 vs 后台同步）时等待锁而不是立即报错
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        let db = Db { conn };
        db.init()?;
        Ok(db)
    }

    fn init(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS folders (
                id          INTEGER PRIMARY KEY,
                name        TEXT UNIQUE NOT NULL,
                uidvalidity INTEGER NOT NULL,
                last_uid    INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS messages (
                id          INTEGER PRIMARY KEY,
                folder_id   INTEGER NOT NULL REFERENCES folders(id),
                uid         INTEGER NOT NULL,
                message_id  TEXT,
                subject     TEXT,
                from_addr   TEXT,
                to_addr     TEXT,
                date        TEXT,
                received_at TEXT,
                body_text   BLOB,
                body_html   BLOB,
                att_name    TEXT,
                att_data    BLOB,
                full_body   INTEGER NOT NULL DEFAULT 0,
                UNIQUE(folder_id, uid)
            );
            CREATE INDEX IF NOT EXISTS idx_messages_folder_uid ON messages(folder_id, uid);
            CREATE INDEX IF NOT EXISTS idx_messages_subject ON messages(subject);",
        )?;
        // 旧库迁移（幂等）：
        // 1. full_body 列（旧库无此列）
        let mut has_col = self
            .conn
            .prepare("SELECT COUNT(*) FROM pragma_table_info('messages') WHERE name='full_body'")?;
        let n: i64 = has_col.query_row([], |r| r.get(0))?;
        if n == 0 {
            self.conn.execute_batch(
                "ALTER TABLE messages ADD COLUMN full_body INTEGER NOT NULL DEFAULT 0;
                 UPDATE messages SET full_body = 1;",
            )?;
        }
        // 2. zip_data → att_data、zip_name → att_name（zip 格式 → tar.zst 格式）
        for (old, new) in [("zip_data", "att_data"), ("zip_name", "att_name")] {
            let mut has = self
                .conn
                .prepare(&format!("SELECT COUNT(*) FROM pragma_table_info('messages') WHERE name='{old}'"))?;
            let cnt: i64 = has.query_row([], |r| r.get(0))?;
            if cnt > 0 {
                self.conn
                    .execute_batch(&format!("ALTER TABLE messages RENAME COLUMN {old} TO {new};"))?;
            }
        }
        Ok(())
    }

    /// 按名取文件夹（不存在返回 None）
    pub fn get_folder(&self, name: &str) -> Result<Option<FolderState>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, uidvalidity, last_uid FROM folders WHERE name = ?1")?;
        let mut rows = stmt.query_map([name], |r| {
            Ok(FolderState {
                id: r.get(0)?,
                uidvalidity: r.get::<_, i64>(1)? as u32,
                last_uid: r.get::<_, i64>(2)? as u32,
            })
        })?;
        match rows.next() {
            Some(Ok(f)) => Ok(Some(f)),
            Some(Err(e)) => Err(e.into()),
            None => Ok(None),
        }
    }

    /// 插入或更新文件夹（UIDVALIDITY 变化时重置 last_uid），返回文件夹 id
    pub fn upsert_folder(&self, name: &str, uidvalidity: u32) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO folders (name, uidvalidity, last_uid) VALUES (?1, ?2, 0)
             ON CONFLICT(name) DO UPDATE SET
               uidvalidity = CASE
                 WHEN folders.uidvalidity = ?2 THEN folders.uidvalidity
                 ELSE ?2  -- UIDVALIDITY 变化：下面统一把 last_uid 归零
               END,
               last_uid = CASE
                 WHEN folders.uidvalidity = ?2 THEN folders.last_uid
                 ELSE 0
               END",
            rusqlite::params![name, uidvalidity],
        )?;
        let id = self.conn.query_row(
            "SELECT id FROM folders WHERE name = ?1",
            [name],
            |r| r.get::<_, i64>(0),
        )?;
        Ok(id)
    }

    /// 更新文件夹已同步的最大 UID
    pub fn update_last_uid(&self, folder_id: i64, last_uid: u32) -> Result<()> {
        self.conn.execute(
            "UPDATE folders SET last_uid = ?1 WHERE id = ?2",
            rusqlite::params![last_uid, folder_id],
        )?;
        Ok(())
    }

    /// 插入一封邮件（UNIQUE(folder_id, uid) 幂等，重复插入忽略）
    #[allow(dead_code)]
    pub fn insert_message(&self, m: &MailRecord) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO messages
                (folder_id, uid, message_id, subject, from_addr, to_addr, date,
                 received_at, body_text, body_html, att_name, att_data, full_body)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'), ?8, ?9, ?10, ?11, ?12)",
            rusqlite::params![
                m.folder_id,
                m.uid,
                m.message_id,
                m.subject,
                m.from_addr,
                m.to_addr,
                m.date,
                m.body_text,
                m.body_html,
                m.att_name,
                m.att_data,
                m.full_body,
            ],
        )?;
        Ok(())
    }

    /// 开始事务（批量插入提速）
    pub fn begin(&self) -> Result<()> {
        self.conn.execute_batch("BEGIN")?;
        Ok(())
    }

    /// 提交事务
    pub fn commit(&self) -> Result<()> {
        self.conn.execute_batch("COMMIT")?;
        Ok(())
    }

    /// 元数据已存在则跳过；不存在则插入（同步用，幂等）
    pub fn insert_meta_if_absent(&self, m: &MailRecord) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO messages
                (folder_id, uid, message_id, subject, from_addr, to_addr, date,
                 received_at, full_body)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'), 0)",
            rusqlite::params![
                m.folder_id,
                m.uid,
                m.message_id,
                m.subject,
                m.from_addr,
                m.to_addr,
                m.date,
            ],
        )?;
        Ok(())
    }

    /// 补拉全文后更新：正文/附件/full_body=1（按 folder_id+uid）
    pub fn update_full_body(
        &self,
        folder_id: i64,
        uid: u32,
        body_text: Option<&[u8]>,
        body_html: Option<&[u8]>,
        att_name: Option<&str>,
        att_data: Option<&[u8]>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET body_text=?1, body_html=?2, att_name=?3, att_data=?4, full_body=1
             WHERE folder_id=?5 AND uid=?6",
            rusqlite::params![body_text, body_html, att_name, att_data, folder_id, uid],
        )?;
        Ok(())
    }

    /// 按 folder+uid 找本地消息 id（补拉用）
    pub fn find_message(&self, folder_id: i64, uid: u32) -> Result<Option<i64>> {
        let mut stmt =
            self.conn
                .prepare("SELECT id FROM messages WHERE folder_id=?1 AND uid=?2")?;
        let mut rows = stmt.query_map([folder_id, uid as i64], |r| r.get::<_, i64>(0))?;
        match rows.next() {
            Some(Ok(id)) => Ok(Some(id)),
            Some(Err(e)) => Err(e.into()),
            None => Ok(None),
        }
    }

    /// 统计：文件夹数与邮件总数
    pub fn stats(&self) -> Result<(i64, i64)> {
        let folders = self
            .conn
            .query_row("SELECT COUNT(*) FROM folders", [], |r| r.get(0))?;
        let messages = self
            .conn
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))?;
        Ok((folders, messages))
    }

    /// 每个文件夹的邮件数（Web UI 侧栏）
    pub fn list_folder_counts(&self) -> Result<Vec<FolderCount>> {
        let mut stmt = self.conn.prepare(
            "SELECT f.name, COUNT(m.id) AS n
             FROM folders f LEFT JOIN messages m ON m.folder_id = f.id
             GROUP BY f.id ORDER BY n DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(FolderCount {
                name: r.get(0)?,
                count: r.get(1)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// 分页查询邮件（可选文件夹过滤 + 主题/发件人搜索）
    pub fn query_messages(
        &self,
        folder: Option<&str>,
        search: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<MessageRow>> {
        let (sql, params) = build_query(folder, search, true);
        // ?N 是 SQLite 编号参数，LIMIT/OFFSET 必须用不同编号
        let n = params.len() + 1;
        let sql = format!("{sql} LIMIT ?{n} OFFSET ?{n2}", n = n, n2 = n + 1);
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query(rusqlite::params_from_iter(
            params
                .into_iter()
                .chain([rusqlite::types::Value::from(limit), rusqlite::types::Value::from(offset)]),
        ))?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(row_from(r)?);
        }
        Ok(out)
    }

    /// 符合过滤条件的邮件总数（分页用）
    pub fn count_messages(&self, folder: Option<&str>, search: Option<&str>) -> Result<i64> {
        let (sql, params) = build_query(folder, search, false);
        let mut stmt = self.conn.prepare(&sql)?;
        let n = stmt.query_row(rusqlite::params_from_iter(params), |r| r.get(0))?;
        Ok(n)
    }

    /// 单封邮件详情
    pub fn get_message(&self, id: i64) -> Result<Option<MessageRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT m.id, f.name, m.uid, m.message_id, m.subject, m.from_addr, m.to_addr,
                    m.date, m.received_at, m.body_text, m.body_html, m.att_name,
                    (m.att_data IS NOT NULL) AS has_att, m.full_body
             FROM messages m JOIN folders f ON m.folder_id = f.id
             WHERE m.id = ?1",
        )?;
        let mut rows = stmt.query_map([id], row_from)?;
        match rows.next() {
            Some(Ok(r)) => Ok(Some(r)),
            Some(Err(e)) => Err(e.into()),
            None => Ok(None),
        }
    }

    /// 附件（att_name + att_data tar.zst），无附件返回 None
    pub fn get_attachment(&self, id: i64) -> Result<Option<(String, Vec<u8>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT att_name, att_data FROM messages WHERE id = ?1 AND att_data IS NOT NULL",
        )?;
        let mut rows = stmt.query_map([id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
        })?;
        match rows.next() {
            Some(Ok(v)) => Ok(Some(v)),
            Some(Err(e)) => Err(e.into()),
            None => Ok(None),
        }
    }

    /// 测试用：内存数据库
    #[cfg(test)]
    fn in_memory() -> Result<Db> {
        let conn = Connection::open_in_memory()?;
        let db = Db { conn };
        db.init()?;
        Ok(db)
    }
}

/// 文件夹邮件数（Web UI 侧栏）
#[derive(Debug, Serialize)]
pub struct FolderCount {
    pub name: String,
    pub count: i64,
}

/// 邮件行（Web UI 列表/详情）
#[derive(Debug, Serialize)]
pub struct MessageRow {
    pub id: i64,
    pub folder: String,
    pub uid: u32,
    pub message_id: Option<String>,
    pub subject: Option<String>,
    pub from_addr: Option<String>,
    pub to_addr: Option<String>,
    pub date: Option<String>,
    pub received_at: Option<String>,
    pub body_text: Option<String>,
    pub body_html: Option<String>,
    pub att_name: Option<String>,
    pub has_attachment: bool,
    /// 是否已含正文/附件（false = 仅元数据，点开时按需补拉）
    pub full_body: bool,
}

/// 构造查询 SQL + 参数（is_list: true 查行，false 查 COUNT）
fn build_query(folder: Option<&str>, search: Option<&str>, is_list: bool) -> (String, Vec<rusqlite::types::Value>) {
    let mut sql = if is_list {
        "SELECT m.id, f.name, m.uid, m.message_id, m.subject, m.from_addr, m.to_addr,
                m.date, m.received_at, m.body_text, m.body_html, m.att_name,
                (m.att_data IS NOT NULL) AS has_att, m.full_body
         FROM messages m JOIN folders f ON m.folder_id = f.id".to_string()
    } else {
        "SELECT COUNT(*) FROM messages m JOIN folders f ON m.folder_id = f.id".to_string()
    };
    let mut params: Vec<rusqlite::types::Value> = Vec::new();
    let mut wheres: Vec<String> = Vec::new();
    if let Some(f) = folder {
        if f != "全部" {
            wheres.push("f.name = ?".to_string());
            params.push(f.to_string().into());
        }
    }
    if let Some(s) = search {
        let s = s.trim();
        if !s.is_empty() {
            wheres.push("(m.subject LIKE ? OR m.from_addr LIKE ? OR m.to_addr LIKE ?)".to_string());
            let like = format!("%{s}%");
            params.push(like.clone().into());
            params.push(like.clone().into());
            params.push(like.into());
        }
    }
    if !wheres.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&wheres.join(" AND "));
    }
    if is_list {
        sql.push_str(" ORDER BY COALESCE(m.date, m.received_at) DESC");
    }
    (sql, params)
}

fn row_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<MessageRow> {
    Ok(MessageRow {
        id: r.get(0)?,
        folder: r.get(1)?,
        uid: r.get::<_, i64>(2)? as u32,
        message_id: r.get(3)?,
        subject: r.get(4)?,
        from_addr: r.get(5)?,
        to_addr: r.get(6)?,
        date: r.get(7)?,
        received_at: r.get(8)?,
        body_text: r.get::<_, Option<Vec<u8>>>(9)?.and_then(|b| decompress_text(&b).ok()),
        body_html: r.get::<_, Option<Vec<u8>>>(10)?.and_then(|b| decompress_text(&b).ok()),
        att_name: r.get(11)?,
        has_attachment: r.get::<_, i64>(12)? != 0,
        full_body: r.get::<_, i64>(13)? != 0,
    })
}

/// zstd 压缩正文（level 1，速度优先）
pub fn compress_text(text: &str) -> Vec<u8> {
    zstd::stream::encode_all(text.as_bytes(), crate::attach::ZSTD_LEVEL)
        .expect("zstd compress body")
}

/// zstd 解压正文
pub fn decompress_text(data: &[u8]) -> Result<String> {
    let mut out = Vec::new();
    zstd::stream::copy_decode(data, &mut out)?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_creates_tables() {
        let db = Db::in_memory().unwrap();
        let n: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('folders','messages')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn init_twice_is_idempotent() {
        let db = Db::in_memory().unwrap();
        db.init().unwrap(); // 第二次不报错
    }

    #[test]
    fn upsert_folder_roundtrip() {
        let db = Db::in_memory().unwrap();
        let id = db.upsert_folder("INBOX", 100).unwrap();
        let f = db.get_folder("INBOX").unwrap().unwrap();
        assert_eq!(f.id, id);
        assert_eq!(f.uidvalidity, 100);
        assert_eq!(f.last_uid, 0);
    }

    #[test]
    fn uidvalidity_change_resets_last_uid() {
        let db = Db::in_memory().unwrap();
        db.upsert_folder("INBOX", 100).unwrap();
        db.update_last_uid(1, 500).unwrap();
        // UIDVALIDITY 不变：last_uid 保留
        db.upsert_folder("INBOX", 100).unwrap();
        assert_eq!(db.get_folder("INBOX").unwrap().unwrap().last_uid, 500);
        // UIDVALIDITY 变化：last_uid 归零
        db.upsert_folder("INBOX", 999).unwrap();
        let f = db.get_folder("INBOX").unwrap().unwrap();
        assert_eq!(f.uidvalidity, 999);
        assert_eq!(f.last_uid, 0);
    }

    #[test]
    fn insert_message_idempotent() {
        let db = Db::in_memory().unwrap();
        let fid = db.upsert_folder("INBOX", 1).unwrap();
        let rec = MailRecord {
            folder_id: fid,
            uid: 42,
            message_id: Some("m1"),
            subject: Some("hello"),
            from_addr: Some("a@b.com"),
            to_addr: None,
            date: None,
            body_text: Some(&compress_text("body")),
            body_html: None,
            att_name: Some("att.tar.zst"),
            att_data: Some(&[1, 2, 3]),
            full_body: true,
        };
        db.insert_message(&rec).unwrap();
        db.insert_message(&rec).unwrap(); // 重复插入被忽略
        let n: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
        // zip 字段确认
        let (name, data): (String, Vec<u8>) = db
            .conn
            .query_row(
                "SELECT att_name, att_data FROM messages WHERE uid=42",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(name, "att.tar.zst");
        assert_eq!(data, vec![1, 2, 3]);
    }

    #[test]
    fn stats_counts() {
        let db = Db::in_memory().unwrap();
        db.upsert_folder("INBOX", 1).unwrap();
        db.upsert_folder("Sent", 1).unwrap();
        assert_eq!(db.stats().unwrap(), (2, 0));
    }
}

#[cfg(test)]
mod query_tests {
    use super::*;

    fn seed(db: &Db) -> (i64, i64) {
        let inbox = db.upsert_folder("INBOX", 1).unwrap();
        let sent = db.upsert_folder("Sent", 1).unwrap();
        let body = compress_text("body");
        db.insert_message(&rec(inbox, 1, "周报 8月", "a@x.com", Some(&body)))
            .unwrap();
        db.insert_message(&rec(inbox, 2, "会议纪要", "b@x.com", None))
            .unwrap();
        db.insert_message(&rec(sent, 1, "回复：周报", "me@x.com", None))
            .unwrap();
        (inbox, sent)
    }

    fn rec<'a>(
        folder_id: i64,
        uid: u32,
        subject: &'a str,
        from: &'a str,
        body: Option<&'a [u8]>,
    ) -> MailRecord<'a> {
        MailRecord {
            folder_id,
            uid,
            message_id: Some("m"),
            subject: Some(subject),
            from_addr: Some(from),
            to_addr: None,
            date: Some("2026-08-10"),
            body_text: body,
            body_html: None,
            att_name: None,
            att_data: None,
            full_body: true,
        }
    }

    #[test]
    fn folder_counts() {
        let db = Db::in_memory().unwrap();
        seed(&db);
        let counts = db.list_folder_counts().unwrap();
        let map: std::collections::HashMap<_, _> =
            counts.into_iter().map(|c| (c.name, c.count)).collect();
        assert_eq!(map.get("INBOX"), Some(&2));
        assert_eq!(map.get("Sent"), Some(&1));
    }

    #[test]
    fn query_all_paginated() {
        let db = Db::in_memory().unwrap();
        seed(&db);
        let all = db.query_messages(None, None, 10, 0).unwrap();
        assert_eq!(all.len(), 3);
        // date 相同，排序不稳定，只验证集合
        let subs: Vec<&str> = all.iter().map(|m| m.subject.as_deref().unwrap()).collect();
        assert!(subs.contains(&"周报 8月"));
        assert!(subs.contains(&"会议纪要"));
        let page = db.query_messages(None, None, 2, 0).unwrap();
        assert_eq!(page.len(), 2);
        let page2 = db.query_messages(None, None, 2, 2).unwrap();
        assert_eq!(page2.len(), 1);
        assert_eq!(db.count_messages(None, None).unwrap(), 3);
    }

    #[test]
    fn query_by_folder() {
        let db = Db::in_memory().unwrap();
        seed(&db);
        let inbox = db.query_messages(Some("INBOX"), None, 10, 0).unwrap();
        assert_eq!(inbox.len(), 2);
        assert!(inbox.iter().all(|m| m.folder == "INBOX"));
        let sent = db.query_messages(Some("Sent"), None, 10, 0).unwrap();
        assert_eq!(sent.len(), 1);
        // "全部"不过滤
        assert_eq!(db.query_messages(Some("全部"), None, 10, 0).unwrap().len(), 3);
    }

    #[test]
    fn query_by_search() {
        let db = Db::in_memory().unwrap();
        seed(&db);
        let r = db.query_messages(None, Some("周报"), 10, 0).unwrap();
        assert_eq!(r.len(), 2);
        let r2 = db.query_messages(None, Some("x.com"), 10, 0).unwrap();
        assert_eq!(r2.len(), 3); // a@x.com, b@x.com, me@x.com
        assert_eq!(db.count_messages(None, Some("周报")).unwrap(), 2);
    }

    #[test]
    fn get_message_detail() {
        let db = Db::in_memory().unwrap();
        seed(&db);
        let m = db.get_message(1).unwrap().unwrap();
        assert_eq!(m.folder, "INBOX");
        assert!(!m.has_attachment);
        assert_eq!(m.body_text.as_deref(), Some("body"));
        assert!(db.get_message(999).unwrap().is_none());
    }

    #[test]
    fn get_attachment_only_when_present() {
        let db = Db::in_memory().unwrap();
        let fid = db.upsert_folder("INBOX", 1).unwrap();
        db.insert_message(&MailRecord {
            folder_id: fid,
            uid: 1,
            message_id: None,
            subject: Some("带附件"),
            from_addr: None,
            to_addr: None,
            date: None,
            body_text: None,
            body_html: None,
            att_name: Some("a.tar.zst"),
            att_data: Some(&[1, 2, 3]),
            full_body: true,
        })
        .unwrap();
        let att = db.get_attachment(1).unwrap().unwrap();
        assert_eq!(att.0, "a.tar.zst");
        assert_eq!(att.1, vec![1, 2, 3]);
        // 无附件邮件
        db.insert_message(&MailRecord {
            folder_id: fid,
            uid: 2,
            message_id: None,
            subject: Some("无附件"),
            from_addr: None,
            to_addr: None,
            date: None,
            body_text: None,
            body_html: None,
            att_name: None,
            att_data: None,
            full_body: true,
        })
        .unwrap();
        assert!(db.get_attachment(2).unwrap().is_none());
    }
}
