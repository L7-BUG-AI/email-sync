//! 构建脚本：编译前自动构建前端（npm run build → dist → src/assets/web/）。
//! 依赖：node/npm 需在 PATH（服务器在 ~/.hermes/node/bin）。

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=src/assets/web");
    println!("cargo:rerun-if-changed=web/src");
    println!("cargo:rerun-if-changed=web/package.json");

    // 前端产物已存在则跳过（避免每次 rebuild 都跑 npm）
    if std::path::Path::new("src/assets/web/index.html").exists() {
        println!("cargo:warning=web assets found, skipping npm build");
        return;
    }

    println!("cargo:warning=building web frontend...");
    let status = Command::new("npm")
        .arg("run")
        .arg("build")
        .current_dir("web")
        .status()
        .expect("npm run build 失败（需要 node/npm，见 web/package.json）");
    if !status.success() {
        panic!("前端构建失败");
    }
}
