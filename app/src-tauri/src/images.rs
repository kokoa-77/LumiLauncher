//! 启动器自己的图片协议 `mcl-image`：把本地图片（存档截图、世界缩略图）交给界面显示。
//!
//! 界面拿到的是 `http://mcl-image.localhost/<编号>` 这种地址，**编号是一次性生成的，不是路径**；
//! 只有后端登记过的图片能被读到（开发合同第 3 节：界面不碰路径）。只认 png / jpg。
//!
//! 地址后面可以带 `?w=<宽度>`：大图按这个宽度缩小（缓存在内容仓库的 `thumbs/` 里），
//! 首页铺满整窗时只要窗口那么大，不用把 4K 截图整张交给网页内核。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use launcher_core::content::store::STORE_DIR;
use launcher_core::saves::thumb;
use tauri::http::header::{ACCESS_CONTROL_ALLOW_ORIGIN, CACHE_CONTROL, CONTENT_TYPE, HeaderValue};
use tauri::http::{Request, Response, StatusCode};
use tauri::{Manager, Runtime, UriSchemeContext, UriSchemeResponder};

use crate::state::AppState;

pub const SCHEME: &str = "mcl-image";
/// Windows 上自定义协议走 `http://<协议>.localhost`
#[cfg(windows)]
const ORIGIN: &str = "http://mcl-image.localhost";
#[cfg(not(windows))]
const ORIGIN: &str = "mcl-image://localhost";
/// 单张图片最大多少（4K 截图也就十来 MB）
const MAX_BYTES: u64 = 32 * 1024 * 1024;
/// 界面能要的宽度范围；按 64 像素一档取整，免得每个窗口宽度都生成一张缓存
const MIN_WIDTH: u32 = 64;
const MAX_WIDTH: u32 = 3840;
const WIDTH_STEP: u32 = 64;

/// 从 `w=1280` 这样的查询串里取宽度（取整到 64 的倍数，限制在合理范围）
fn requested_width(query: Option<&str>) -> Option<u32> {
    let value = query?
        .split('&')
        .find_map(|pair| pair.strip_prefix("w="))?
        .parse::<u32>()
        .ok()?;
    let rounded = value.clamp(MIN_WIDTH, MAX_WIDTH).div_ceil(WIDTH_STEP) * WIDTH_STEP;
    Some(rounded.min(MAX_WIDTH))
}

/// 登记过的图片：编号 ↔ 路径
#[derive(Default)]
pub struct Images {
    inner: Mutex<Registry>,
}

#[derive(Default)]
struct Registry {
    by_token: HashMap<String, PathBuf>,
    by_path: HashMap<PathBuf, String>,
}

impl Images {
    /// 登记一张图片，返回给界面用的地址（同一张图多次登记得到同一个地址）
    pub fn url_for(&self, path: &Path) -> String {
        let mut registry = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let token = match registry.by_path.get(path) {
            Some(token) => token.clone(),
            None => {
                let token = uuid::Uuid::new_v4().simple().to_string();
                registry.by_path.insert(path.to_path_buf(), token.clone());
                registry.by_token.insert(token.clone(), path.to_path_buf());
                token
            }
        };
        format!("{ORIGIN}/{token}")
    }

    fn lookup(&self, token: &str) -> Option<PathBuf> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .by_token
            .get(token)
            .cloned()
    }
}

fn content_type(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        _ => None,
    }
}

fn response(status: StatusCode, mime: Option<&'static str>, body: Vec<u8>) -> Response<Vec<u8>> {
    let mut response = Response::new(body);
    *response.status_mut() = status;
    if let Some(mime) = mime {
        response
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static(mime));
    }
    // 截图会被新截图替换成别的编号，同一个编号的内容不变，可以放心缓存
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("max-age=3600"));
    // 界面要在画布上读图片的颜色（随实例变色），得允许跨源读取；能读到的只有登记过的图片
    response
        .headers_mut()
        .insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    response
}

/// 处理 `mcl-image` 请求（在后台线程读文件，不卡界面）。
pub fn handle<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let app = ctx.app_handle().clone();
    let token = request.uri().path().trim_start_matches('/').to_owned();
    let width = requested_width(request.uri().query());
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let Some(path) = state.images.lookup(&token) else {
            responder.respond(response(StatusCode::NOT_FOUND, None, Vec::new()));
            return;
        };
        let Some(mut mime) = content_type(&path) else {
            responder.respond(response(StatusCode::FORBIDDEN, None, Vec::new()));
            return;
        };
        let mut path = path;
        if let Some(width) = width {
            let cache = state.game().root().join(STORE_DIR).join("thumbs");
            match thumb::thumbnail(&path, width, &cache) {
                Ok(small) => {
                    // 缩小版是 JPEG，透明底的图是 PNG
                    if small != path {
                        mime = content_type(&small).unwrap_or("image/jpeg");
                    }
                    path = small;
                }
                // 缩不了就给原图，不影响显示
                Err(error) => tracing::warn!(%error, file = %path.display(), "生成缩略图失败"),
            }
        }
        let read = std::fs::metadata(&path).and_then(|meta| {
            if meta.len() > MAX_BYTES {
                Err(std::io::Error::other("图片太大"))
            } else {
                std::fs::read(&path)
            }
        });
        match read {
            Ok(bytes) => responder.respond(response(StatusCode::OK, Some(mime), bytes)),
            Err(error) => {
                tracing::warn!(%error, file = %path.display(), "读图片失败");
                responder.respond(response(StatusCode::NOT_FOUND, None, Vec::new()));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_image_gets_the_same_opaque_url() {
        let images = Images::default();
        let a = images.url_for(Path::new("D:/game/saves/w/icon.png"));
        let b = images.url_for(Path::new("D:/game/saves/w/icon.png"));
        let c = images.url_for(Path::new("D:/game/screenshots/1.png"));
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(!a.contains("game"), "地址里不带路径：{a}");
        let token = a.rsplit('/').next().unwrap();
        assert_eq!(
            images.lookup(token).as_deref(),
            Some(Path::new("D:/game/saves/w/icon.png"))
        );
        assert_eq!(images.lookup("../../etc/passwd"), None);
    }

    #[test]
    fn widths_are_rounded_and_bounded() {
        assert_eq!(requested_width(Some("w=1180")), Some(1216));
        assert_eq!(requested_width(Some("w=1280")), Some(1280));
        assert_eq!(requested_width(Some("x=1&w=10")), Some(64));
        assert_eq!(requested_width(Some("w=99999")), Some(3840));
        assert_eq!(requested_width(Some("w=abc")), None);
        assert_eq!(requested_width(None), None);
    }

    #[test]
    fn only_serves_pictures() {
        assert_eq!(content_type(Path::new("a.PNG")), Some("image/png"));
        assert_eq!(content_type(Path::new("a.jpg")), Some("image/jpeg"));
        assert_eq!(content_type(Path::new("level.dat")), None);
        assert_eq!(content_type(Path::new("noext")), None);
    }
}
