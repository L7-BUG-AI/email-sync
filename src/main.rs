mod config;
mod db;
mod imap_client;
mod attach;
mod parse;
mod sync;

fn main() {
    let cfg = config::Config::load(&config::default_config_path()).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    println!("配置加载成功: {} @ {}:{}", cfg.email, cfg.server, cfg.port);
}
