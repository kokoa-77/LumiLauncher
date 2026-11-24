//! 下载引擎：并发下载一批文件，每个文件都校验大小和 SHA-1。
//!
//! - 先写 `<文件名>.part`，校验通过才改名成正式文件——半截文件永远不会被当成好文件
//! - 断点续传：`.part` 还在就带 Range 头接着下；服务器不支持续传就从头下
//! - 失败重试：同一个地址最多试 2 次（间隔递增），还不行换下一个候选地址
//! - 已存在且校验通过的文件直接跳过
//! - 随时可以取消；取消后 `.part` 保留，下次接着下
//! - 需要凭据的下载源（CurseForge 的 CDN 要 API Key）用 [`HostHeader`]：
//!   **只对列出的域名**带上请求头，别的域名一律不带

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures_util::{StreamExt, stream};
use reqwest::{Client, StatusCode, header};
use sha1::{Digest, Sha1};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use super::{to_hex, url_host};
use crate::redact::Secret;

const ATTEMPTS_PER_URL: u32 = 2;
/// 带了凭据还被拒绝（401 / 403）时失败原因的开头，上层据此提示「API Key 无效或被撤销」
pub const DENIED_PREFIX: &str = "下载源拒绝了启动器的凭据";
const SIZE_MISMATCH: &str = "大小不对：";
const SHA1_MISMATCH: &str = "SHA-1 校验失败：";
const RETRY_BASE_DELAY: Duration = Duration::from_millis(500);

/// 一个要下载的文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadItem {
    /// 候选地址，按尝试顺序排列
    pub urls: Vec<String>,
    /// 保存位置（调用方负责保证它在允许的目录里）
    pub path: PathBuf,
    /// 期望的 SHA-1（十六进制）。没有时只校验大小
    pub sha1: Option<String>,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DownloadProgress {
    pub done_files: u64,
    pub total_files: u64,
    pub done_bytes: u64,
    /// 已知大小的文件总字节数（有些文件事先不知道大小）
    pub total_bytes: u64,
    /// 这次真从网络收到的字节，实时累加（算速度用）：已经有、直接跳过的文件不算，
    /// 失败重试前收到的也算（网络确实传了）
    pub received_bytes: u64,
    /// 第一个地址下不下来、换了地址才下好的文件数（下载历史里的「换源」）
    pub fallbacks: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DownloadReport {
    /// 实际下载的文件数
    pub downloaded: u64,
    /// 已存在且校验通过、直接跳过的文件数
    pub skipped: u64,
    /// 本次实际从网络收到的字节数（不含续传前已有的部分）
    pub bytes_received: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFailure {
    pub path: PathBuf,
    /// 最后一次失败的原因
    pub reason: String,
}

impl FileFailure {
    pub fn is_integrity_mismatch(&self) -> bool {
        self.reason.starts_with(SIZE_MISMATCH) || self.reason.starts_with(SHA1_MISMATCH)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("下载已取消")]
    Cancelled,
    #[error("有 {} 个文件下载失败", .0.len())]
    Failed(Vec<FileFailure>),
}

/// 单个文件的最终结果
enum FileResult {
    Downloaded,
    Skipped,
    Failed(FileFailure),
    Cancelled,
}

/// 一次尝试失败的类型
enum AttemptError {
    /// 这个地址还值得再试一次（网络中断、超时、5xx、校验失败）
    Retry(String),
    /// 这个地址没希望了，直接换下一个（404、403 等）
    NextUrl(String),
    Cancelled,
}

struct Counters {
    total_files: u64,
    total_bytes: u64,
    done_files: AtomicU64,
    done_bytes: AtomicU64,
    received_bytes: AtomicU64,
    /// 实时收到的字节（每块都加，见 [`DownloadProgress::received_bytes`]）
    live_received: AtomicU64,
    fallbacks: AtomicU64,
}

impl Counters {
    fn snapshot(&self) -> DownloadProgress {
        let done_bytes = self.done_bytes.load(Ordering::Relaxed);
        DownloadProgress {
            done_files: self.done_files.load(Ordering::Relaxed),
            total_files: self.total_files,
            done_bytes,
            total_bytes: self.total_bytes.max(done_bytes),
            received_bytes: self.live_received.load(Ordering::Relaxed),
            fallbacks: self.fallbacks.load(Ordering::Relaxed),
        }
    }
}

/// 某次尝试里已经计入进度的字节数；尝试失败时要从进度里减回去，避免重试时重复计算。
struct Credit<'a> {
    counters: &'a Counters,
    bytes: u64,
}

impl Credit<'_> {
    fn add(&mut self, bytes: u64) {
        self.counters.done_bytes.fetch_add(bytes, Ordering::Relaxed);
        self.bytes += bytes;
    }

    fn rollback(&mut self) {
        self.counters
            .done_bytes
            .fetch_sub(self.bytes, Ordering::Relaxed);
        self.bytes = 0;
    }
}

/// 只发给指定域名的请求头（例如 CurseForge 的 `x-api-key`）。
#[derive(Clone)]
pub struct HostHeader {
    /// 只有这些域名（完全相同，不含子域名）的请求才带这个头
    pub hosts: &'static [&'static str],
    pub name: &'static str,
    pub value: Secret<String>,
}

impl HostHeader {
    fn applies_to(&self, url: &str) -> bool {
        url_host(url).is_some_and(|host| self.hosts.contains(&host.as_str()))
    }
}

pub struct Downloader {
    client: Client,
    concurrency: usize,
    quick_verify: bool,
    host_header: Option<HostHeader>,
}

impl Downloader {
    pub fn new(client: Client, concurrency: usize) -> Self {
        Self {
            client,
            concurrency: concurrency.max(1),
            quick_verify: false,
            host_header: None,
        }
    }

    /// 给指定域名的下载带上请求头（凭据）。
    pub fn with_host_header(mut self, header: Option<HostHeader>) -> Self {
        self.host_header = header;
        self
    }

    /// 快速校验：已存在的文件只要大小对就算通过，不算哈希。
    /// 用于启动前的检查（几千个资源文件逐个算哈希太慢）；安装和「修复游戏文件」用完整校验。
    /// 新下载的文件不受影响，始终完整校验。
    pub fn quick_verify(mut self, enabled: bool) -> Self {
        self.quick_verify = enabled;
        self
    }

    /// 下载一批文件。部分失败时会把其他文件下完，最后统一返回失败清单。
    /// `on_progress` 调用很频繁，需要节流时由调用方处理（任务系统已自带节流）。
    pub async fn download_all<F>(
        &self,
        items: &[DownloadItem],
        cancel: &CancellationToken,
        on_progress: F,
    ) -> Result<DownloadReport, DownloadError>
    where
        F: Fn(DownloadProgress) + Send + Sync,
    {
        let counters = Counters {
            total_files: items.len() as u64,
            total_bytes: items.iter().filter_map(|i| i.size).sum(),
            done_files: AtomicU64::new(0),
            done_bytes: AtomicU64::new(0),
            received_bytes: AtomicU64::new(0),
            live_received: AtomicU64::new(0),
            fallbacks: AtomicU64::new(0),
        };

        // 先把每个文件的任务建好再并发执行：如果在流上挂 `.map(闭包)`，闭包会跨 await 存活，
        // 编译器的已知问题（rust-lang/rust#102211）会让整个下载任务判定为「不能跨线程」，
        // 界面的后台任务（要求 Send）就用不了它
        let tasks: Vec<_> = items
            .iter()
            .map(|item| self.download_one(item, cancel, &counters, &on_progress))
            .collect();
        let results: Vec<FileResult> = stream::iter(tasks)
            .buffer_unordered(self.concurrency)
            .collect()
            .await;

        if cancel.is_cancelled() {
            return Err(DownloadError::Cancelled);
        }

        let mut report = DownloadReport {
            bytes_received: counters.received_bytes.load(Ordering::Relaxed),
            ..DownloadReport::default()
        };
        let mut failures = Vec::new();
        for result in results {
            match result {
                FileResult::Downloaded => report.downloaded += 1,
                FileResult::Skipped => report.skipped += 1,
                FileResult::Failed(failure) => failures.push(failure),
                FileResult::Cancelled => return Err(DownloadError::Cancelled),
            }
        }
        if failures.is_empty() {
            Ok(report)
        } else {
            Err(DownloadError::Failed(failures))
        }
    }

    async fn download_one<F>(
        &self,
        item: &DownloadItem,
        cancel: &CancellationToken,
        counters: &Counters,
        on_progress: &F,
    ) -> FileResult
    where
        F: Fn(DownloadProgress),
    {
        if cancel.is_cancelled() {
            return FileResult::Cancelled;
        }
        if is_valid_existing(item, self.quick_verify).await {
            counters.done_files.fetch_add(1, Ordering::Relaxed);
            if let Some(size) = item.size {
                counters.done_bytes.fetch_add(size, Ordering::Relaxed);
            }
            on_progress(counters.snapshot());
            return FileResult::Skipped;
        }

        let mut last_reason = String::from("没有可用的下载地址");
        for (index, url) in item.urls.iter().enumerate() {
            for attempt in 0..ATTEMPTS_PER_URL {
                let mut credit = Credit { counters, bytes: 0 };
                match self
                    .attempt(url, item, cancel, &mut credit, on_progress)
                    .await
                {
                    Ok(received) => {
                        counters.done_files.fetch_add(1, Ordering::Relaxed);
                        if index > 0 {
                            counters.fallbacks.fetch_add(1, Ordering::Relaxed);
                        }
                        counters
                            .received_bytes
                            .fetch_add(received, Ordering::Relaxed);
                        on_progress(counters.snapshot());
                        tracing::debug!(path = %item.path.display(), url, "下载完成");
                        return FileResult::Downloaded;
                    }
                    Err(AttemptError::Cancelled) => {
                        credit.rollback();
                        return FileResult::Cancelled;
                    }
                    Err(AttemptError::NextUrl(reason)) => {
                        credit.rollback();
                        tracing::debug!(url, reason, "换下一个下载地址");
                        last_reason = reason;
                        break;
                    }
                    Err(AttemptError::Retry(reason)) => {
                        credit.rollback();
                        tracing::debug!(url, attempt, reason, "下载失败，准备重试");
                        last_reason = reason;
                        if attempt + 1 < ATTEMPTS_PER_URL {
                            let delay = RETRY_BASE_DELAY * 2u32.pow(attempt);
                            tokio::select! {
                                () = tokio::time::sleep(delay) => {}
                                () = cancel.cancelled() => return FileResult::Cancelled,
                            }
                        }
                    }
                }
            }
        }

        tracing::warn!(path = %item.path.display(), reason = %last_reason, "文件下载失败");
        FileResult::Failed(FileFailure {
            path: item.path.clone(),
            reason: last_reason,
        })
    }

    /// 从一个地址尝试下载一次。成功时返回本次从网络收到的字节数。
    async fn attempt<F>(
        &self,
        url: &str,
        item: &DownloadItem,
        cancel: &CancellationToken,
        credit: &mut Credit<'_>,
        on_progress: &F,
    ) -> Result<u64, AttemptError>
    where
        F: Fn(DownloadProgress),
    {
        let part = part_path(&item.path);
        if let Some(parent) = item.path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| AttemptError::Retry(format!("无法创建目录：{e}")))?;
        }

        let partial_size = fs::metadata(&part).await.ok().map(|m| m.len());
        let mut resume_from = partial_size.unwrap_or(0);
        if let Some(size) = item.size {
            if partial_size == Some(size) {
                // 上次已经下完但还没来得及改名：直接校验
                credit.add(resume_from);
                finish(&part, item).await?;
                return Ok(0);
            }
            if resume_from > size {
                remove_quietly(&part).await;
                resume_from = 0;
            }
        }

        let mut request = self.client.get(url);
        if resume_from > 0 {
            request = request.header(header::RANGE, format!("bytes={resume_from}-"));
        }
        let authorized = self.host_header.as_ref().filter(|h| h.applies_to(url));
        if let Some(auth) = authorized {
            request = request.header(auth.name, auth.value.expose().as_str());
        }
        let response = tokio::select! {
            response = request.send() => response
                .map_err(|e| AttemptError::Retry(format!("连接失败：{e}")))?,
            () = cancel.cancelled() => return Err(AttemptError::Cancelled),
        };

        let status = response.status();
        let mut file = if status == StatusCode::PARTIAL_CONTENT && resume_from > 0 {
            credit.add(resume_from);
            fs::OpenOptions::new().append(true).open(&part).await
        } else if status.is_success() {
            // 服务器不支持续传（或本来就是从头下）：清空重写
            resume_from = 0;
            fs::File::create(&part).await
        } else if status == StatusCode::RANGE_NOT_SATISFIABLE {
            remove_quietly(&part).await;
            return Err(AttemptError::Retry("续传位置无效，改为从头下载".to_owned()));
        } else if is_retryable(status) {
            return Err(AttemptError::Retry(format!("服务器返回 {status}")));
        } else if authorized.is_some()
            && matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
        {
            return Err(AttemptError::NextUrl(format!(
                "{DENIED_PREFIX}（服务器返回 {status}）"
            )));
        } else {
            return Err(AttemptError::NextUrl(format!("服务器返回 {status}")));
        }
        .map_err(|e| AttemptError::Retry(format!("无法写入临时文件：{e}")))?;

        let mut written = resume_from;
        let mut body = response.bytes_stream();
        loop {
            let next = tokio::select! {
                next = body.next() => next,
                () = cancel.cancelled() => {
                    // 尽量把已收到的部分落盘，下次续传；失败也不影响取消
                    if let Err(error) = file.flush().await {
                        tracing::debug!(%error, "取消时写入临时文件失败");
                    }
                    return Err(AttemptError::Cancelled);
                }
            };
            match next {
                None => break,
                Some(Err(error)) => {
                    if let Err(flush_error) = file.flush().await {
                        tracing::debug!(%flush_error, "中断时写入临时文件失败");
                    }
                    return Err(AttemptError::Retry(format!("下载中断：{error}")));
                }
                Some(Ok(chunk)) => {
                    file.write_all(&chunk)
                        .await
                        .map_err(|e| AttemptError::Retry(format!("写入失败：{e}")))?;
                    let len = chunk.len() as u64;
                    written += len;
                    credit.add(len);
                    credit
                        .counters
                        .live_received
                        .fetch_add(len, Ordering::Relaxed);
                    if item.size.is_some_and(|size| written > size) {
                        drop(file);
                        remove_quietly(&part).await;
                        return Err(AttemptError::Retry(
                            "收到的内容比预期大，可能不是正确的文件".to_owned(),
                        ));
                    }
                    on_progress(credit.counters.snapshot());
                }
            }
        }
        file.flush()
            .await
            .map_err(|e| AttemptError::Retry(format!("写入失败：{e}")))?;
        drop(file);

        finish(&part, item).await?;
        Ok(written - resume_from)
    }
}

/// 校验 `.part`，通过后改名成正式文件；校验失败就删掉 `.part`。
async fn finish(part: &Path, item: &DownloadItem) -> Result<(), AttemptError> {
    match verify(part, item).await {
        Ok(Ok(())) => {}
        Ok(Err(mismatch)) => {
            remove_quietly(part).await;
            return Err(AttemptError::Retry(mismatch));
        }
        Err(error) => return Err(AttemptError::Retry(format!("无法读取下载的文件：{error}"))),
    }
    fs::rename(part, &item.path)
        .await
        .map_err(|e| AttemptError::Retry(format!("无法保存文件（可能被占用）：{e}")))
}

/// 外层错误是读文件失败，内层错误是内容不对（中文说明）。
async fn verify(path: &Path, item: &DownloadItem) -> io::Result<Result<(), String>> {
    let len = fs::metadata(path).await?.len();
    if let Some(size) = item.size
        && len != size
    {
        return Ok(Err(format!(
            "{SIZE_MISMATCH}应为 {size} 字节，实际 {len} 字节"
        )));
    }
    if let Some(expected) = &item.sha1 {
        let actual = sha1_file(path).await?;
        if !actual.eq_ignore_ascii_case(expected) {
            return Ok(Err(format!(
                "{SHA1_MISMATCH}应为 {expected}，实际 {actual}"
            )));
        }
    }
    Ok(Ok(()))
}

async fn is_valid_existing(item: &DownloadItem, quick: bool) -> bool {
    let Ok(metadata) = fs::metadata(&item.path).await else {
        return false;
    };
    if quick && let Some(size) = item.size {
        return metadata.len() == size;
    }
    matches!(verify(&item.path, item).await, Ok(Ok(())))
}

/// 计算文件的 SHA-1（小写十六进制），在后台线程里读，不阻塞异步任务。
pub async fn sha1_file(path: &Path) -> io::Result<String> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut file = std::fs::File::open(&path)?;
        let mut hasher = Sha1::new();
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let n = file.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
        }
        Ok(to_hex(&hasher.finalize()))
    })
    .await
    .map_err(io::Error::other)?
}

fn part_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".part");
    path.with_file_name(name)
}

fn is_retryable(status: StatusCode) -> bool {
    status.is_server_error()
        || matches!(
            status,
            StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_EARLY | StatusCode::TOO_MANY_REQUESTS
        )
}

async fn remove_quietly(path: &Path) {
    if let Err(error) = fs::remove_file(path).await
        && error.kind() != io::ErrorKind::NotFound
    {
        tracing::debug!(path = %path.display(), %error, "删除临时文件失败");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_header_only_goes_to_listed_hosts() {
        let header = HostHeader {
            hosts: &["edge.forgecdn.net"],
            name: "x-api-key",
            value: Secret::new("k".to_owned()),
        };
        assert!(header.applies_to("https://edge.forgecdn.net/files/1/2/a.jar"));
        assert!(header.applies_to("https://EDGE.forgecdn.net:443/files/a.jar"));
        for other in [
            "http://edge.forgecdn.net/files/a.jar",
            "https://edge.forgecdn.net.evil.com/a.jar",
            "https://evil.com/edge.forgecdn.net/a.jar",
            "https://user@evil.com/a.jar",
            "https://cdn.modrinth.com/a.jar",
        ] {
            assert!(!header.applies_to(other), "{other}");
        }
    }

    #[test]
    fn part_file_sits_next_to_the_target() {
        assert_eq!(
            part_path(Path::new("a/b/client.jar")),
            PathBuf::from("a/b/client.jar.part")
        );
    }

    #[test]
    fn retry_only_transient_statuses() {
        assert!(is_retryable(StatusCode::SERVICE_UNAVAILABLE));
        assert!(is_retryable(StatusCode::TOO_MANY_REQUESTS));
        assert!(!is_retryable(StatusCode::NOT_FOUND));
        assert!(!is_retryable(StatusCode::FORBIDDEN));
    }

    #[tokio::test]
    async fn downloads_an_empty_file_without_a_partial_file() {
        use tokio::io::AsyncReadExt;

        let server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let serve = tokio::spawn(async move {
            let (mut socket, _) = server.accept().await.unwrap();
            let mut request = [0; 4096];
            assert!(socket.read(&mut request).await.unwrap() > 0);
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
        });

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty");
        let item = DownloadItem {
            urls: vec![format!("http://{address}/empty")],
            path: path.clone(),
            sha1: Some("da39a3ee5e6b4b0d3255bfef95601890afd80709".to_owned()),
            size: Some(0),
        };
        let downloader = Downloader::new(Client::builder().no_proxy().build().unwrap(), 1);
        downloader
            .download_all(&[item], &CancellationToken::new(), |_| {})
            .await
            .unwrap();
        serve.await.unwrap();
        assert!(fs::read(path).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn sha1_of_known_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hello.txt");
        std::fs::write(&path, b"hello").unwrap();
        assert_eq!(
            sha1_file(&path).await.unwrap(),
            "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d"
        );
    }
}
