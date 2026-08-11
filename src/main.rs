//! email-sync —— 把 IMAP 邮箱所有文件夹的邮件增量同步到本地 SQLite。
//!
//! 用法：
//!   email-sync sync                # 同步所有文件夹（默认命令）
//!   email-sync sync --folder INBOX # 只同步指定文件夹
//!   email-sync status              # 显示数据库统计
//!   email-sync --config <path>     # 指定配置文件（默认 ~/.config/email-sync/config.txt）

mod attach;
mod config;
mod db;
mod rfc2047;
mod imap_client;
mod parse;
mod sync;
mod web;

use anyhow::Result;

/// 数据库默认路径：~/.local/share/email-sync/email.db
fn default_db_path() -> std::path::PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        std::path::Path::new(&home)
            .join(".local")
            .join("share")
            .join("email-sync")
            .join("email.db")
    } else {
        std::path::PathBuf::from("email.db")
    }
}

fn open_db() -> Result<(db::Db, String)> {
    let db_path = default_db_path();
    if let Some(dir) = db_path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    let db = db::Db::open(db_path.to_str().unwrap())?;
    Ok((db, db_path.display().to_string()))
}

fn main() -> Result<()> {
    // 简单参数解析（无 clap，保持体积）：--config 最先处理
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut cfg_path = config::default_config_path();
    let mut command = String::from("sync");
    let mut folder_filter: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--config" => {
                i += 1;
                if i < args.len() {
                    cfg_path = std::path::PathBuf::from(&args[i]);
                }
            }
            "--folder" => {
                i += 1;
                if i < args.len() {
                    folder_filter = Some(args[i].clone());
                }
            }
            "sync" | "status" | "serve" => command = args[i].clone(),
            other => {
                eprintln!("未知参数: {other}");
                print_usage();
                std::process::exit(2);
            }
        }
        i += 1;
    }

    // 加载配置（不存在时自动生成模板并提示）
    let cfg = config::Config::load(&cfg_path)?;

    let db_path = default_db_path();
    if let Some(dir) = db_path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    let db = db::Db::open(db_path.to_str().unwrap())?;

    match command.as_str() {
        "sync" => {
            println!("连接 {}（{}）...", cfg.server, cfg.email);
            let mut session = imap_client::connect(&cfg)?;
            if let Some(folder) = &folder_filter {
                let n = sync::sync_folder(&db, &mut session, folder)?;
                println!("{folder}: 新增 {n} 封");
            } else {
                let (folders, total) = sync::sync_all(&db, &mut session)?;
                println!("完成：{folders} 个文件夹，新增 {total} 封邮件");
            }
            session.logout().ok();
        }
        "status" => {
            let (db, db_path) = open_db()?;
            let (folders, messages) = db.stats()?;
            println!("数据库: {db_path}");
            println!("文件夹数: {folders}");
            println!("邮件总数: {messages}");
        }
        "serve" => {
            let (db, db_path) = open_db()?;
            let port = std::env::var("EMAIL_SYNC_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(8080);
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(web::serve(db, cfg, db_path, port))?;
        }
        _ => unreachable!(),
    }
    Ok(())
}

fn print_usage() {
    println!(
        "用法:\n  email-sync sync                # 同步所有文件夹（默认）\n  \
         email-sync sync --folder INBOX # 只同步指定文件夹\n  \
         email-sync status              # 显示数据库统计\n  \
         email-sync --config <path>     # 指定配置文件"
    );
}
