use std::path::Path;

fn main() {
    // Tauri 默认只给正式程序嵌入 Windows 清单（声明需要新版通用控件库）。
    // 测试程序没有清单，在 Windows 上会因「找不到入口点」（0xC0000139）无法启动。
    // 所以关掉默认清单，改由链接器给所有产物（正式程序和测试）都嵌入同一份。
    embed_manifest_for_all_targets();
    let attributes = tauri_build::Attributes::new()
        .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
    if let Err(error) = tauri_build::try_build(attributes) {
        panic!("tauri 构建脚本失败：{error:#}");
    }
}

fn embed_manifest_for_all_targets() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os != "windows" || target_env != "msvc" {
        return;
    }
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("windows-app-manifest.xml");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
}
