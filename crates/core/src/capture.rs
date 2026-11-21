//! 截游戏窗口的画面（自动封面用）。
//!
//! Windows 上用系统自带的 PrintWindow（`PW_RENDERFULLCONTENT | PW_CLIENTONLY`）：
//! 不用游戏配合、不按 F2、不抢焦点，只截窗口的内容区（不带标题栏和边框）。
//! 只在这个窗口正在最前面、没有最小化时截：游戏失去焦点会自动暂停、弹出菜单，那时截到的是菜单。
//! 别的系统暂时不截（总是返回 `None`）。
//!
//! 底层调用都在两个库里（xcap：找窗口、看在不在最前面；win-screenshot：截内容区），
//! 本项目不写 `unsafe`。

use image::RgbImage;

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("找游戏窗口失败：{0}")]
    Find(String),
    #[error("截游戏画面失败：{0}")]
    Capture(String),
}

/// 截 `pid` 这个进程现在在最前面的窗口。它不在最前面、最小化了、找不到时返回 `Ok(None)`。
/// 会调用系统接口，在后台线程里用。
#[cfg(windows)]
pub fn focused_window(pid: u32) -> Result<Option<RgbImage>, CaptureError> {
    use win_screenshot::capture::{Area, Using, capture_window_ex};

    let windows = match xcap::Window::all() {
        Ok(windows) => windows,
        Err(error) => {
            // 锁屏、UAC 安全桌面或无交互桌面（如 CI）下 EnumWindows 会失败，
            // 此时游戏窗口肯定不在最前面，按「当前没有前台窗口」处理。
            tracing::debug!(%error, "现在读不到系统窗口列表（可能已锁屏或处于非交互桌面）");
            return Ok(None);
        }
    };
    let Some(window) = windows
        .into_iter()
        .find(|w| w.pid().is_ok_and(|p| p == pid) && w.is_focused().unwrap_or(false))
    else {
        return Ok(None);
    };
    if window.is_minimized().unwrap_or(true) {
        return Ok(None);
    }
    let id = window.id().map_err(|e| CaptureError::Find(e.to_string()))?;
    // 64 位 Windows 的窗口句柄也只用低 32 位，系统保证截断后再符号扩展回来是同一个句柄
    let hwnd = id as i32 as isize;
    let shot = capture_window_ex(hwnd, Using::PrintWindow, Area::ClientOnly, None, None)
        .map_err(|e| CaptureError::Capture(e.to_string()))?;
    let rgba = image::RgbaImage::from_raw(shot.width, shot.height, shot.pixels)
        .ok_or_else(|| CaptureError::Capture("画面数据和尺寸对不上".to_owned()))?;
    Ok(Some(image::DynamicImage::ImageRgba8(rgba).into_rgb8()))
}

#[cfg(not(windows))]
pub fn focused_window(_pid: u32) -> Result<Option<RgbImage>, CaptureError> {
    Ok(None)
}

/// 截 `pid` 这个进程的第一个可见窗口，不管它在不在最前面（只给命令行调试工具用，验证截图本身对不对）。
#[cfg(windows)]
pub fn any_window(pid: u32) -> Result<Option<RgbImage>, CaptureError> {
    use win_screenshot::capture::{Area, Using, capture_window_ex};

    let windows = xcap::Window::all().map_err(|e| CaptureError::Find(e.to_string()))?;
    let Some(window) = windows
        .into_iter()
        .find(|w| w.pid().is_ok_and(|p| p == pid) && !w.is_minimized().unwrap_or(true))
    else {
        return Ok(None);
    };
    let id = window.id().map_err(|e| CaptureError::Find(e.to_string()))?;
    let shot = capture_window_ex(
        id as i32 as isize,
        Using::PrintWindow,
        Area::ClientOnly,
        None,
        None,
    )
    .map_err(|e| CaptureError::Capture(e.to_string()))?;
    let rgba = image::RgbaImage::from_raw(shot.width, shot.height, shot.pixels)
        .ok_or_else(|| CaptureError::Capture("画面数据和尺寸对不上".to_owned()))?;
    Ok(Some(image::DynamicImage::ImageRgba8(rgba).into_rgb8()))
}

#[cfg(not(windows))]
pub fn any_window(_pid: u32) -> Result<Option<RgbImage>, CaptureError> {
    Ok(None)
}
