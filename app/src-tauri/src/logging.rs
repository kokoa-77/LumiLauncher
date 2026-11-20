//! 日志：按天滚动、保留 7 天；写入前统一经过脱敏（token 不会落进日志文件）。

use std::io::{self, Write};
use std::path::Path;

use launcher_core::redact::redact;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// xcap（找窗口）读不了受保护进程的名字时会自己打一条错误日志，列「谁占着内存」时每次都有好几条，
/// 而失败本身我们已经处理了（返回值里有），所以关掉它自己的日志
const DEFAULT_FILTER: &str = "info,launcher_core=debug,launcher_app_lib=debug,xcap=off";
const KEEP_DAYS: usize = 7;

/// 持有它才能保证退出前日志写完；放进 Tauri 的托管状态里。
pub struct LogGuard(#[allow(dead_code)] WorkerGuard);

/// 每次写入都先脱敏。fmt 层每条日志只调用一次 write，所以能整条处理。
struct RedactingWriter<W>(W);

impl<W: Write> Write for RedactingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let text = String::from_utf8_lossy(buf);
        self.0.write_all(redact(&text).as_bytes())?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

pub fn init(log_dir: &Path) -> Result<LogGuard, String> {
    std::fs::create_dir_all(log_dir).map_err(|e| format!("无法创建日志目录：{e}"))?;
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("launcher")
        .filename_suffix("log")
        .max_log_files(KEEP_DAYS)
        .build(log_dir)
        .map_err(|e| format!("无法创建日志文件：{e}"))?;
    let (file_writer, guard) = tracing_appender::non_blocking(RedactingWriter(appender));

    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    let file_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(file_writer);
    let console_layer = cfg!(debug_assertions)
        .then(|| tracing_subscriber::fmt::layer().with_writer(|| RedactingWriter(io::stderr())));

    tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(console_layer)
        .try_init()
        .map_err(|e| format!("日志系统初始化失败：{e}"))?;

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!(panic = %info, "启动器发生崩溃");
        default_hook(info);
    }));

    Ok(LogGuard(guard))
}
