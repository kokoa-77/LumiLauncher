// 发布版不弹控制台窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // 以管理员身份被再开一次、只做一件事（整理内存）时，做完就退出，不开窗口
    if let Some(code) = launcher_app_lib::run_helper() {
        std::process::exit(code);
    }
    launcher_app_lib::run();
}
