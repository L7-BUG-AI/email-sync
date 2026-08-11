//! Web UI：axum HTTP 服务，浏览器访问查看邮件。
//!
//! 路由：
//!   GET  /                        → 页面（HTML）
//!   GET  /api/folders             → 文件夹列表 + 邮件数
//!   GET  /api/messages            → 邮件列表（?folder=&search=&page=）
//!   GET  /api/messages/:id        → 邮件详情
//!   GET  /api/messages/:id/attachments → 附件 zip 下载
//!   GET  /api/status              → 数据库统计
//!   POST /api/sync                → 触发同步（返回新同步数量）

use std::sync::{Arc, Mutex};

use anyhow::Result;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::db::{Db, FolderCount, MessageRow};

/// 共享状态
pub struct AppState {
    pub db: Db,
    pub cfg: Config,
    pub db_path: String,
}

type SharedState = Arc<Mutex<AppState>>;

/// 启动 Web 服务
pub async fn serve(db: Db, cfg: Config, db_path: String, port: u16) -> Result<()> {
    let state: SharedState = Arc::new(Mutex::new(AppState { db, cfg, db_path }));
    let app = Router::new()
        .route("/", get(index))
        .route("/api/folders", get(api_folders))
        .route("/api/messages", get(api_messages))
        .route("/api/messages/:id", get(api_message))
        .route("/api/messages/:id/attachments", get(api_attachments))
        .route("/api/status", get(api_status))
        .route("/api/sync", post(api_sync))
        .with_state(state);

    let addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    println!("🌐 email-sync Web UI: http://localhost:{port}");
    axum::serve(listener, app).await?;
    Ok(())
}

// ---------- 页面 ----------

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

// ---------- API ----------

async fn api_folders(State(state): State<SharedState>) -> Result<Json<Vec<FolderCount>>, ApiError> {
    let st = state.lock().unwrap();
    let folders = st.db.list_folder_counts().map_err(ApiError::from)?;
    Ok(Json(folders))
}

#[derive(Deserialize)]
struct ListQuery {
    folder: Option<String>,
    search: Option<String>,
    page: Option<i64>,
}

#[derive(Serialize)]
struct MessageList {
    total: i64,
    page: i64,
    pages: i64,
    messages: Vec<MessageRow>,
}

async fn api_messages(
    State(state): State<SharedState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<MessageList>, ApiError> {
    const PAGE_SIZE: i64 = 30;
    let st = state.lock().unwrap();
    let page = q.page.unwrap_or(1).max(1);
    let total = st
        .db
        .count_messages(q.folder.as_deref(), q.search.as_deref())
        .map_err(ApiError::from)?;
    let messages = st
        .db
        .query_messages(
            q.folder.as_deref(),
            q.search.as_deref(),
            PAGE_SIZE,
            (page - 1) * PAGE_SIZE,
        )
        .map_err(ApiError::from)?;
    let pages = if total == 0 { 1 } else { (total + PAGE_SIZE - 1) / PAGE_SIZE };
    Ok(Json(MessageList {
        total,
        page,
        pages,
        messages,
    }))
}

async fn api_message(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<MessageRow>, ApiError> {
    let st = state.lock().unwrap();
    let mut msg = st
        .db
        .get_message(id)
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::msg("邮件不存在"))?;
    // 仅元数据时按需补拉全文
    if !msg.full_body {
        let folder = msg.folder.clone();
        let uid = msg.uid;
        let cfg = st.cfg.clone();
        match crate::imap_client::connect(&cfg)
            .and_then(|mut s| crate::sync::fetch_full_message(&st.db, &mut s, &folder, uid))
        {
            Ok(true) => {
                msg = st
                    .db
                    .get_message(id)
                    .map_err(ApiError::from)?
                    .ok_or_else(|| ApiError::msg("邮件不存在"))?;
            }
            Ok(false) => {}
            Err(e) => {
                eprintln!("[warn] 补拉全文失败 id={id}: {e:#}");
            }
        }
    }
    Ok(Json(msg))
}

async fn api_attachments(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Response, ApiError> {
    let st = state.lock().unwrap();
    // 仅元数据时先补拉全文（附件在全文里）
    let msg = st
        .db
        .get_message(id)
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::msg("邮件不存在"))?;
    if !msg.full_body {
        let folder = msg.folder.clone();
        let uid = msg.uid;
        let cfg = st.cfg.clone();
        let _ = crate::imap_client::connect(&cfg)
            .and_then(|mut s| crate::sync::fetch_full_message(&st.db, &mut s, &folder, uid));
    }
    let (name, data) = st
        .db
        .get_attachment(id)
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::msg("该邮件没有附件"))?;
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/zip")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", name),
        )
        .body(Body::from(data))
        .unwrap())
}

#[derive(Serialize)]
struct StatusInfo {
    folders: i64,
    messages: i64,
    db_path: String,
}

async fn api_status(State(state): State<SharedState>) -> Result<Json<StatusInfo>, ApiError> {
    let st = state.lock().unwrap();
    let (folders, messages) = st.db.stats().map_err(ApiError::from)?;
    Ok(Json(StatusInfo {
        folders,
        messages,
        db_path: st.db_path.clone(),
    }))
}

#[derive(Serialize)]
struct SyncResult {
    ok: bool,
    message: String,
}

async fn api_sync(State(state): State<SharedState>) -> Result<Json<SyncResult>, ApiError> {
    let st = state.lock().unwrap();
    let cfg = st.cfg.clone();
    let mut session = crate::imap_client::connect(&cfg).map_err(|e| ApiError::msg(&e.to_string()))?;
    let (folders, total) = crate::sync::sync_all(&st.db, &mut session)
        .map_err(|e| ApiError::msg(&e.to_string()))?;
    session.logout().ok();
    Ok(Json(SyncResult {
        ok: true,
        message: format!("同步完成：{folders} 个文件夹，新增 {total} 封"),
    }))
}

// ---------- 错误 ----------

pub struct ApiError(anyhow::Error);

impl ApiError {
    fn msg(s: &str) -> Self {
        ApiError(anyhow::anyhow!("{s}"))
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        ApiError(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": self.0.to_string() })),
        )
            .into_response()
    }
}

// ---------- 前端页面 ----------

const INDEX_HTML: &str = r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>email-sync 邮件库</title>
<style>
:root { --bg:#0f1115; --panel:#171a21; --border:#262b36; --text:#e6e8ee; --dim:#8b93a5; --accent:#4c8dff; --hover:#1f2430; }
* { margin:0; padding:0; box-sizing:border-box; }
body { background:var(--bg); color:var(--text); font:14px/1.6 -apple-system,"PingFang SC","Microsoft YaHei",sans-serif; }
#app { display:flex; height:100vh; }
.sidebar { width:230px; background:var(--panel); border-right:1px solid var(--border); display:flex; flex-direction:column; }
.brand { padding:16px 18px; font-weight:700; font-size:16px; border-bottom:1px solid var(--border); }
.brand span { color:var(--accent); }
.sidebar nav { flex:1; overflow-y:auto; padding:8px; }
.folder { padding:8px 12px; border-radius:8px; cursor:pointer; display:flex; justify-content:space-between; }
.folder:hover { background:var(--hover); }
.folder.active { background:var(--accent); color:#fff; }
.folder .count { color:var(--dim); font-size:12px; }
.folder.active .count { color:#fff; }
.sync-btn { margin:10px; padding:10px; border:0; border-radius:8px; background:var(--accent); color:#fff; font-size:14px; cursor:pointer; }
.sync-btn:disabled { opacity:.5; cursor:wait; }
.main { flex:1; display:flex; flex-direction:column; min-width:0; }
.toolbar { padding:12px 18px; border-bottom:1px solid var(--border); display:flex; gap:12px; align-items:center; }
.toolbar input { flex:1; padding:8px 12px; border-radius:8px; border:1px solid var(--border); background:var(--panel); color:var(--text); font-size:14px; }
.toolbar .meta { color:var(--dim); font-size:13px; white-space:nowrap; }
.list { flex:1; overflow-y:auto; }
.mail { padding:12px 18px; border-bottom:1px solid var(--border); cursor:pointer; display:flex; gap:12px; align-items:baseline; }
.mail:hover { background:var(--hover); }
.mail.selected { background:var(--hover); box-shadow:inset 3px 0 var(--accent); }
.mail .subject { font-weight:600; flex:1; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
.mail .from { color:var(--dim); width:220px; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; font-size:13px; }
.mail .date { color:var(--dim); font-size:12px; white-space:nowrap; }
.mail .att { color:var(--accent); font-size:12px; }
.pager { padding:10px 18px; border-top:1px solid var(--border); display:flex; justify-content:center; gap:12px; align-items:center; color:var(--dim); }
.pager button { padding:5px 14px; border-radius:6px; border:1px solid var(--border); background:var(--panel); color:var(--text); cursor:pointer; }
.pager button:disabled { opacity:.4; cursor:default; }
.detail { flex:1; display:flex; flex-direction:column; min-width:0; }
.detail-empty { flex:1; display:flex; align-items:center; justify-content:center; color:var(--dim); }
.detail-head { padding:16px 22px; border-bottom:1px solid var(--border); }
.detail-head h2 { font-size:17px; margin-bottom:8px; }
.detail-head .meta { color:var(--dim); font-size:13px; margin-bottom:4px; }
.detail-head .att-link { display:inline-block; margin-top:8px; color:var(--accent); text-decoration:none; background:var(--hover); padding:6px 12px; border-radius:6px; font-size:13px; }
.detail-body { flex:1; overflow-y:auto; padding:20px 22px; }
.detail-body pre { white-space:pre-wrap; word-break:break-word; font:13px/1.7 inherit; }
.detail-body .html { color:var(--dim); font-size:13px; }
.toast { position:fixed; bottom:20px; left:50%; transform:translateX(-50%); background:var(--accent); color:#fff; padding:10px 20px; border-radius:8px; display:none; font-size:14px; z-index:99; }
</style>
</head>
<body>
<div id="app">
  <aside class="sidebar">
    <div class="brand">📧 email-<span>sync</span></div>
    <nav id="folders"></nav>
    <button class="sync-btn" id="syncBtn">🔄 立即同步</button>
  </aside>
  <div class="main">
    <div class="toolbar">
      <input id="search" placeholder="搜索主题 / 发件人 / 收件人..." />
      <span class="meta" id="meta"></span>
    </div>
    <div class="list" id="mailList"></div>
    <div class="pager">
      <button id="prevBtn">‹ 上一页</button>
      <span id="pageInfo"></span>
      <button id="nextBtn">下一页 ›</button>
    </div>
  </div>
  <div class="detail" id="detailPane">
    <div class="detail-empty">← 点击左侧邮件查看详情</div>
  </div>
</div>
<div class="toast" id="toast"></div>
<script>
let curFolder = "全部", curSearch = "", curPage = 1, total = 0, pages = 1, selectedId = null;

async function j(url, opts) {
  const r = await fetch(url, opts);
  if (!r.ok) { const e = await r.json().catch(()=>({})); throw new Error(e.error || r.status); }
  return r.json();
}
function esc(s) { const d = document.createElement("div"); d.textContent = s ?? ""; return d.innerHTML; }
function fmtDate(d) { if (!d) return ""; const m = String(d).match(/(\d{4})[- ](\d{1,2})[- ](\d{1,2})/); return m ? `${m[1]}-${m[2]}-${m[3]}` : String(d).slice(0,16); }
function toast(msg) { const t = document.getElementById("toast"); t.textContent = msg; t.style.display = "block"; setTimeout(()=>t.style.display="none", 3000); }

async function loadFolders() {
  const folders = await j("/api/folders");
  const nav = document.getElementById("folders");
  const items = [{name:"全部",count:folders.reduce((a,f)=>a+f.count,0)}].concat(folders);
  nav.innerHTML = items.map(f => `<div class="folder${f.name===curFolder?" active":""}" data-folder="${esc(f.name)}"><span>${esc(f.name)}</span><span class="count">${f.count}</span></div>`).join("");
  nav.querySelectorAll(".folder").forEach(el => el.onclick = () => { curFolder = el.dataset.folder; curPage = 1; loadFolders(); loadList(); });
}

async function loadList() {
  const q = new URLSearchParams({ page: curPage });
  if (curFolder !== "全部") q.set("folder", curFolder);
  if (curSearch) q.set("search", curSearch);
  const data = await j("/api/messages?" + q);
  total = data.total; pages = data.pages; curPage = data.page;
  document.getElementById("meta").textContent = `共 ${total} 封`;
  document.getElementById("pageInfo").textContent = `${curPage} / ${pages}`;
  document.getElementById("prevBtn").disabled = curPage <= 1;
  document.getElementById("nextBtn").disabled = curPage >= pages;
  const list = document.getElementById("mailList");
  list.innerHTML = data.messages.map(m => `
    <div class="mail${m.id===selectedId?" selected":""}" data-id="${m.id}">
      <span class="subject">${esc(m.subject || "(无主题)")}</span>
      <span class="from">${esc(m.from_addr || "")}</span>
      <span class="date">${fmtDate(m.date)}</span>
      ${m.has_attachment ? '<span class="att">📎</span>' : ""}
    </div>`).join("");
  list.querySelectorAll(".mail").forEach(el => el.onclick = () => { selectedId = Number(el.dataset.id); loadList(); loadDetail(selectedId); });
}

async function loadDetail(id) {
  const m = await j("/api/messages/" + id);
  const pane = document.getElementById("detailPane");
  let body = "";
  if (m.body_text) body = `<pre>${esc(m.body_text)}</pre>`;
  else if (m.body_html) body = `<div class="html">（HTML 邮件，附件可直接下载）</div>`;
  else body = '<div class="html">（无正文）</div>';
  pane.innerHTML = `
    <div class="detail-head">
      <h2>${esc(m.subject || "(无主题)")}</h2>
      <div class="meta">发件人：${esc(m.from_addr || "-")}</div>
      <div class="meta">收件人：${esc(m.to_addr || "-")}</div>
      <div class="meta">日期：${esc(m.date || "")} ｜ 文件夹：${esc(m.folder)}</div>
      ${m.has_attachment ? `<a class="att-link" href="/api/messages/${m.id}/attachments">📎 下载附件 (${esc(m.zip_name || "zip")})</a>` : ""}
    </div>
    <div class="detail-body">${body}</div>`;
}

document.getElementById("search").addEventListener("input", e => {
  clearTimeout(window._st);
  window._st = setTimeout(() => { curSearch = e.target.value; curPage = 1; loadList(); }, 400);
});
document.getElementById("prevBtn").onclick = () => { if (curPage > 1) { curPage--; loadList(); } };
document.getElementById("nextBtn").onclick = () => { if (curPage < pages) { curPage++; loadList(); } };
document.getElementById("syncBtn").onclick = async function() {
  this.disabled = true; this.textContent = "同步中...";
  try {
    const r = await j("/api/sync", { method: "POST" });
    toast(r.message);
    await loadFolders(); await loadList();
  } catch (e) { toast("同步失败：" + e.message); }
  this.disabled = false; this.textContent = "🔄 立即同步";
};

loadFolders().catch(e=>toast("加载失败："+e.message));
loadList().catch(e=>toast("加载失败："+e.message));
</script>
</body>
</html>"#;
