//! SQLite 存储：folders（同步进度）+ messages（邮件内容 + 附件 zip BLOB）。
//! 用户用数据库工具直接查询本库。

use anyhow::Result;
use rusqlite::Connection;

/// 文件夹同步进度
#[derive(Debug, Clone)]
pub struct FolderState {
    pub id: i64,
    pub uidvalidity: u32,
    pub last_uid: u32,
}

/// 邮件记录（入库用）
pub struct MailRecord<'a> {
    pub folder_id: i64,
    pub uid: u32,
    pub message_id: Option<&'a str>,
    pub subject: Option<&'a str>,
    pub from_addr: Option<&'a str>,
    pub to_addr: Option<&'a str>,
    pub date: Option<&'a str>,
    pub body_text: Option<&'a str>,
    pub body_html: Option<&'a str>,
    pub zip_name: Option<&'a str>,
    pub zip_data: Option<&'a [u8]>,
}

pub struct Db {
    conn: Connection,
}

impl Db {
    /// 打开（或创建）数据库并初始化表结构（幂等）
    pub fn open(path: &str) -> Result<Db> {
        let conn = Connection::open(path)?;
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
                body_text   TEXT,
                body_html   TEXT,
                zip_name    TEXT,
                zip_data    BLOB,
                UNIQUE(folder_id, uid)
            );
            CREATE INDEX IF NOT EXISTS idx_messages_folder_uid ON messages(folder_id, uid);
            CREATE INDEX IF NOT EXISTS idx_messages_subject ON messages(subject);",
        )?;
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
    pub fn insert_message(&self, m: &MailRecord) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO messages
                (folder_id, uid, message_id, subject, from_addr, to_addr, date,
                 received_at, body_text, body_html, zip_name, zip_data)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'), ?8, ?9, ?10, ?11)",
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
                m.zip_name,
                m.zip_data,
            ],
        )?;
        Ok(())
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

    /// 测试用：内存数据库
    #[cfg(test)]
    fn in_memory() -> Result<Db> {
        let conn = Connection::open_in_memory()?;
        let db = Db { conn };
        db.init()?;
        Ok(db)
    }
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
            body_text: Some("body"),
            body_html: None,
            zip_name: Some("att.zip"),
            zip_data: Some(&[1, 2, 3]),
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
                "SELECT zip_name, zip_data FROM messages WHERE uid=42",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(name, "att.zip");
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
