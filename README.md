# email-sync

把 IMAP 邮箱**所有文件夹**的邮件**增量同步**到本地 SQLite，附件自动打包成 zip 存二进制字段。

提供两种使用方式：
- **Web UI**：浏览器查看邮件、搜索、下载附件、一键同步（推荐）
- **DB 直查**：用数据库连接工具（DBeaver / Navicat / sqlite3 CLI）直接查库

## 用法

```bash
email-sync serve                 # 启动 Web UI（默认 http://localhost:8080）
EMAIL_SYNC_PORT=9000 email-sync serve   # 自定义端口

email-sync sync                 # 同步所有文件夹（默认命令）
email-sync sync --folder INBOX  # 只同步指定文件夹
email-sync status               # 查看数据库统计
email-sync --config <path>      # 指定配置文件
```

## Web UI

启动 `email-sync serve` 后，浏览器打开 `http://localhost:8080`：

- 📁 左侧文件夹列表（全部/Junk/INBOX/Sent/Drafts，带邮件数）
- 📋 中间邮件列表（分页 30 封/页，主题/发件人/日期，📎 附件标记）
- 🔍 顶部搜索（按主题/发件人/收件人，实时过滤）
- 📄 右侧详情面板（正文 + 附件下载链接）
- 🔄 "立即同步"按钮（页面上直接触发增量同步）

架构：Rust `axum` 内置 HTTP 服务 + 单 HTML 原生 JS（参考 motrix-next 的"Rust 后端 + Web 前端"思路，砍掉桌面打包层）。API：

| 路由 | 说明 |
|---|---|
| `GET /` | 页面 |
| `GET /api/folders` | 文件夹 + 邮件数 |
| `GET /api/messages?folder=&search=&page=` | 邮件列表（分页）|
| `GET /api/messages/:id` | 邮件详情 |
| `GET /api/messages/:id/attachments` | 附件 tar.zst 下载 |
| `GET /api/status` | 数据库统计 |
| `POST /api/sync` | 触发增量同步 |

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
- `zstd` (level 1) + `tar` — 附件打包 tar.zst、正文压缩
- `axum` + `tokio` — Web UI 服务

## 构建

```bash
cargo build --release   # 产物 target/release/email-sync（约 2.3MB）
cargo test              # 32 个单元测试
```

## 存储格式（v0.2）

- **附件**：多附件 tar 打包 + zstd level 1 压缩，存 `att_data` BLOB（文件名 `att_name`，如 `attachments-000000.tar.zst`）
- **正文**：body_text/body_html 用 zstd level 1 压缩后存 BLOB，Web UI 读取时自动解压
- **DB 工具直查**：正文/附件看到的是压缩二进制（乱码）——看正文用 Web UI，附件下载后 `tar -xf xxx.tar.zst` 解压
- 压缩等级常量：`src/attach.rs` 的 `ZSTD_LEVEL = 1`（速度优先）

## 说明

- 同步是**只增**的：重复运行不会重复入库（`UNIQUE(folder_id, uid)` 幂等），也不删除本地已存的邮件
- 首次全量同步较慢（邮件多时），之后增量同步只拉新邮件
- 附件打包为 zip 后存 BLOB；无附件的邮件 zip 字段为 NULL
