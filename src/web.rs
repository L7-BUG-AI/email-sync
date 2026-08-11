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
use include_dir::{include_dir, Dir};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::db::{Db, FolderCount, MessageRow};

/// 嵌入的前端构建产物（web/dist → src/assets/web，build.rs 负责构建）
static WEB_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/src/assets/web");

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
        .route("/assets/:path", get(static_asset))
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

// ---------- 页面与静态资源 ----------

async fn index() -> Result<Html<&'static str>, ApiError> {
    let file = WEB_DIR
        .get_file("index.html")
        .ok_or_else(|| ApiError::msg("前端未构建（web/ 下 npm run build）"))?;
    let content = file
        .contents_utf8()
        .ok_or_else(|| ApiError::msg("index.html 非 UTF-8"))?;
    Ok(Html(content))
}

/// 静态资源：/assets/xxx.js|css|png...
async fn static_asset(Path(path): Path<String>) -> Result<Response, ApiError> {
    let rel = format!("assets/{path}");
    let file = WEB_DIR
        .get_file(&rel)
        .ok_or_else(|| ApiError::msg("资源不存在"))?;
    let data = file.contents();
    let mime = if rel.ends_with(".js") {
        "application/javascript"
    } else if rel.ends_with(".css") {
        "text/css"
    } else if rel.ends_with(".png") {
        "image/png"
    } else if rel.ends_with(".svg") {
        "image/svg+xml"
    } else if rel.ends_with(".woff2") {
        "font/woff2"
    } else if rel.ends_with(".json") {
        "application/json"
    } else {
        "application/octet-stream"
    };
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .body(Body::from(data.to_vec()))
        .unwrap())
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
        .header(header::CONTENT_TYPE, "application/zstd")
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
