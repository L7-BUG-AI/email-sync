# email-sync

把 IMAP 邮箱**所有文件夹**的邮件**增量同步**到本地 SQLite，附件自动打包成 zip 存二进制字段。

用数据库连接工具（DBeaver / Navicat / sqlite3 CLI）直接查库即可，无 GUI。

## 用法

```bash
email-sync sync                 # 同步所有文件夹（默认命令）
email-sync sync --folder INBOX  # 只同步指定文件夹
email-sync status               # 查看数据库统计
email-sync --config <path>      # 指定配置文件
```

## 配置文件

首次运行自动生成模板，路径：

```
~/.config/email-sync/config.txt
```

```ini
# email-sync 配置
server=imap.exmail.qq.com   # IMAP 服务器（企业微信 imap.exmail.qq.com / QQ imap.qq.com / 163 imap.163.com）
port=993                    # IMAP SSL 端口
email=you@example.com       # 邮箱地址
auth_code=xxxxxxxxxxxx      # IMAP 授权码（不是邮箱密码！网页设置开启 IMAP 后生成）
```

## 数据库

默认路径：`~/.local/share/email-sync/email.db`

### 表结构

**folders**（同步进度，每文件夹一行）

| 字段 | 说明 |
|---|---|
| id | 主键 |
| name | IMAP 文件夹名 |
| uidvalidity | UID 有效性标记（服务器重置 UID 时自动全量重同步）|
| last_uid | 已同步的最大 UID（增量断点）|

**messages**（邮件，一封一行）

| 字段 | 说明 |
|---|---|
| id | 主键 |
| folder_id | 所属文件夹 |
| uid | IMAP UID |
| message_id | Message-ID 头 |
| subject / from_addr / to_addr / date | 邮件头 |
| received_at | 本地入库时间 |
| body_text | 纯文本正文 |
| body_html | HTML 正文 |
| **zip_name** | 附件压缩包名（无附件为 NULL）|
| **zip_data** | 附件 zip 压缩后的二进制（无附件为 NULL）|

### 常用查询

```sql
-- 全部邮件（按日期倒序）
SELECT date, from_addr, subject FROM messages ORDER BY date DESC;

-- 只看有附件的邮件
SELECT date, from_addr, subject, zip_name FROM messages WHERE zip_data IS NOT NULL;

-- 按发件人过滤
SELECT subject, date FROM messages WHERE from_addr LIKE '%example.com%';

-- 全文搜索主题
SELECT subject, date FROM messages WHERE subject LIKE '%周报%';
```

### 导出附件

```bash
# 把某封邮件的 zip 附件导出为文件
sqlite3 ~/.local/share/email-sync/email.db \
  "SELECT writefile('/tmp/attachments.zip', zip_data) FROM messages WHERE id = 1;"
```

## 技术栈

- `imap` (2.x) + `native-tls` — IMAP 拉取（UID 增量跟踪）
- `mailparse` — RFC822 解析（主题/正文/附件）
- `rusqlite` (bundled) — SQLite 存储
- `zip` — 附件打包（deflate）

## 构建

```bash
cargo build --release   # 产物 target/release/email-sync（约 1.9MB）
cargo test              # 26 个单元测试
```

## 说明

- 同步是**只增**的：重复运行不会重复入库（`UNIQUE(folder_id, uid)` 幂等），也不删除本地已存的邮件
- 首次全量同步较慢（邮件多时），之后增量同步只拉新邮件
- 附件打包为 zip 后存 BLOB；无附件的邮件 zip 字段为 NULL
