//! 下载引擎的集成测试：用本机的极简 HTTP 服务器模拟各种网络情况。

// 测试代码允许直接 unwrap（clippy 不把测试文件里的辅助函数算作测试）
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};

use launcher_core::net::{DownloadError, DownloadItem, DownloadProgress, Downloader, sha1_file};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

/// 服务器收到的每个请求：(路径, Range 头)
type RequestLog = Mutex<Vec<(String, Option<String>)>>;

#[derive(Clone)]
enum Behavior {
    /// 正常返回，支持 Range 续传
    Serve(Vec<u8>),
    /// 第一次请求只发一半就断开，之后正常（支持 Range）
    DropHalfwayOnce(Vec<u8>),
    NotFound,
}

struct Server {
    addr: SocketAddr,
    /// 每次请求的 (路径, Range 头)
    requests: Arc<RequestLog>,
}

impl Server {
    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    fn requests_for(&self, path: &str) -> Vec<Option<String>> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| p == path)
            .map(|(_, range)| range.clone())
            .collect()
    }
}

async fn start_server(routes: Vec<(&'static str, Behavior)>) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let routes: Arc<HashMap<String, Behavior>> =
        Arc::new(routes.into_iter().map(|(p, b)| (p.to_owned(), b)).collect());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let dropped = Arc::new(Mutex::new(HashSet::new()));

    let (routes_c, requests_c) = (routes.clone(), requests.clone());
    tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                break;
            };
            let (routes, requests, dropped) =
                (routes_c.clone(), requests_c.clone(), dropped.clone());
            tokio::spawn(async move {
                handle(socket, &routes, &requests, &dropped).await;
            });
        }
    });
    Server { addr, requests }
}

async fn handle(
    mut socket: TcpStream,
    routes: &HashMap<String, Behavior>,
    requests: &RequestLog,
    dropped: &Mutex<HashSet<String>>,
) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        match socket.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let head = String::from_utf8_lossy(&buf).to_string();
    let path = head.split_whitespace().nth(1).unwrap_or("/").to_owned();
    let range = head
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("range: bytes=")
                .map(str::to_owned)
        })
        .map(|r| r.trim_end_matches('-').to_owned());
    requests
        .lock()
        .unwrap()
        .push((path.clone(), range.clone().map(|r| format!("bytes={r}-"))));

    let (body, drop_now) = match routes.get(&path) {
        Some(Behavior::Serve(body)) => (body.clone(), false),
        Some(Behavior::DropHalfwayOnce(body)) => {
            let first = dropped.lock().unwrap().insert(path.clone());
            (body.clone(), first)
        }
        Some(Behavior::NotFound) | None => {
            let _ = socket
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await;
            return;
        }
    };

    let start: usize = range.and_then(|r| r.parse().ok()).unwrap_or(0);
    let slice = &body[start.min(body.len())..];
    let header = if start > 0 {
        format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
            slice.len(),
            start,
            body.len() - 1,
            body.len()
        )
    } else {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            slice.len()
        )
    };
    let _ = socket.write_all(header.as_bytes()).await;
    let sent = if drop_now {
        &slice[..slice.len() / 2]
    } else {
        slice
    };
    let _ = socket.write_all(sent).await;
    let _ = socket.flush().await;
    // 函数结束时连接被关闭；drop_now 的情况下客户端会收到不完整的内容
}

fn body(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u64 * 31 + u64::from(seed)) as u8)
        .collect()
}

async fn sha1_of(bytes: &[u8]) -> String {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tmp");
    std::fs::write(&path, bytes).unwrap();
    sha1_file(&path).await.unwrap()
}

fn downloader() -> Downloader {
    // 测试连的是本机，绕开系统代理
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    Downloader::new(client, 4)
}

fn no_part_files(dir: &Path) -> bool {
    !std::fs::read_dir(dir)
        .unwrap()
        .any(|e| e.unwrap().file_name().to_string_lossy().ends_with(".part"))
}

#[tokio::test]
async fn downloads_and_verifies_a_file() {
    let content = body(200_000, 1);
    let server = start_server(vec![("/a.jar", Behavior::Serve(content.clone()))]).await;
    let dir = tempfile::tempdir().unwrap();
    let item = DownloadItem {
        urls: vec![server.url("/a.jar")],
        path: dir.path().join("sub/a.jar"),
        sha1: Some(sha1_of(&content).await),
        size: Some(content.len() as u64),
    };

    let report = downloader()
        .download_all(
            std::slice::from_ref(&item),
            &CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap();

    assert_eq!(report.downloaded, 1);
    assert_eq!(std::fs::read(&item.path).unwrap(), content);
    assert!(no_part_files(&dir.path().join("sub")));
}

#[tokio::test]
async fn resumes_after_the_connection_drops() {
    let content = body(512 * 1024, 2);
    let server = start_server(vec![(
        "/big.jar",
        Behavior::DropHalfwayOnce(content.clone()),
    )])
    .await;
    let dir = tempfile::tempdir().unwrap();
    let item = DownloadItem {
        urls: vec![server.url("/big.jar")],
        path: dir.path().join("big.jar"),
        sha1: Some(sha1_of(&content).await),
        size: Some(content.len() as u64),
    };

    downloader()
        .download_all(
            std::slice::from_ref(&item),
            &CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap();

    assert_eq!(std::fs::read(&item.path).unwrap(), content);
    let requests = server.requests_for("/big.jar");
    assert_eq!(requests.len(), 2, "断开后应该再请求一次");
    assert_eq!(requests[0], None);
    let resumed_from: u64 = requests[1]
        .as_deref()
        .and_then(|r| r.strip_prefix("bytes=")?.strip_suffix('-')?.parse().ok())
        .expect("第二次请求应该带 Range 头续传");
    assert!(resumed_from > 0, "应该从断开的位置接着下");
}

#[tokio::test]
async fn falls_back_to_the_next_url_on_404_and_bad_content() {
    let content = body(50_000, 3);
    let server = start_server(vec![
        ("/missing", Behavior::NotFound),
        ("/wrong", Behavior::Serve(body(50_000, 9))),
        ("/good", Behavior::Serve(content.clone())),
    ])
    .await;
    let dir = tempfile::tempdir().unwrap();
    let item = DownloadItem {
        urls: vec![
            server.url("/missing"),
            server.url("/wrong"),
            server.url("/good"),
        ],
        path: dir.path().join("c.jar"),
        sha1: Some(sha1_of(&content).await),
        size: Some(content.len() as u64),
    };

    let last = Mutex::new(DownloadProgress::default());
    downloader()
        .download_all(
            std::slice::from_ref(&item),
            &CancellationToken::new(),
            |p| *last.lock().unwrap() = p,
        )
        .await
        .unwrap();

    assert_eq!(std::fs::read(&item.path).unwrap(), content);
    let last = *last.lock().unwrap();
    assert_eq!(last.fallbacks, 1, "换了地址才下好的记一次换源");
    assert!(
        last.received_bytes >= content.len() as u64,
        "网络收到的字节含内容不对那次"
    );
    assert_eq!(
        server.requests_for("/missing").len(),
        1,
        "404 不重试，直接换地址"
    );
    assert_eq!(
        server.requests_for("/wrong").len(),
        2,
        "内容不对重试一次再换地址"
    );
    assert!(no_part_files(dir.path()));
}

#[tokio::test]
async fn skips_files_that_are_already_valid() {
    let content = body(10_000, 4);
    let server = start_server(vec![("/d.jar", Behavior::Serve(content.clone()))]).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("d.jar");
    std::fs::write(&path, &content).unwrap();
    let item = DownloadItem {
        urls: vec![server.url("/d.jar")],
        path,
        sha1: Some(sha1_of(&content).await),
        size: Some(content.len() as u64),
    };

    let report = downloader()
        .download_all(
            std::slice::from_ref(&item),
            &CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap();

    assert_eq!((report.downloaded, report.skipped), (0, 1));
    assert!(
        server.requests_for("/d.jar").is_empty(),
        "已经是好文件就不该发请求"
    );
}

#[tokio::test]
async fn one_failure_does_not_stop_the_others() {
    let good = body(20_000, 5);
    let server = start_server(vec![("/good", Behavior::Serve(good.clone()))]).await;
    let dir = tempfile::tempdir().unwrap();
    let items = vec![
        DownloadItem {
            urls: vec![server.url("/good")],
            path: dir.path().join("good.jar"),
            sha1: Some(sha1_of(&good).await),
            size: Some(good.len() as u64),
        },
        DownloadItem {
            urls: vec![server.url("/nope")],
            path: dir.path().join("bad.jar"),
            sha1: None,
            size: None,
        },
    ];

    let error = downloader()
        .download_all(&items, &CancellationToken::new(), |_| {})
        .await
        .unwrap_err();

    match error {
        DownloadError::Failed(failures) => {
            assert_eq!(failures.len(), 1);
            assert!(failures[0].path.ends_with("bad.jar"));
            assert!(
                failures[0].reason.contains("404"),
                "失败原因要写清楚：{}",
                failures[0].reason
            );
        }
        other => panic!("应该是部分失败，实际是 {other:?}"),
    }
    assert_eq!(std::fs::read(dir.path().join("good.jar")).unwrap(), good);
}

#[tokio::test]
async fn cancelled_downloads_report_cancelled() {
    let server = start_server(vec![("/e", Behavior::Serve(body(1000, 6)))]).await;
    let dir = tempfile::tempdir().unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let item = DownloadItem {
        urls: vec![server.url("/e")],
        path: dir.path().join("e"),
        sha1: None,
        size: None,
    };

    let result = downloader().download_all(&[item], &cancel, |_| {}).await;
    assert!(matches!(result, Err(DownloadError::Cancelled)));
}

#[tokio::test]
async fn progress_reaches_the_total() {
    let a = body(30_000, 7);
    let b = body(70_000, 8);
    let server = start_server(vec![
        ("/a", Behavior::Serve(a.clone())),
        ("/b", Behavior::DropHalfwayOnce(b.clone())),
    ])
    .await;
    let dir = tempfile::tempdir().unwrap();
    let items = vec![
        DownloadItem {
            urls: vec![server.url("/a")],
            path: dir.path().join("a"),
            sha1: Some(sha1_of(&a).await),
            size: Some(a.len() as u64),
        },
        DownloadItem {
            urls: vec![server.url("/b")],
            path: dir.path().join("b"),
            sha1: Some(sha1_of(&b).await),
            size: Some(b.len() as u64),
        },
    ];
    let last = Arc::new(Mutex::new(DownloadProgress::default()));
    let last_c = last.clone();

    downloader()
        .download_all(&items, &CancellationToken::new(), move |p| {
            *last_c.lock().unwrap() = p
        })
        .await
        .unwrap();

    let p = *last.lock().unwrap();
    assert_eq!(p.done_files, 2);
    assert_eq!(p.total_files, 2);
    assert_eq!(p.done_bytes, 100_000, "续传不能把字节重复计算");
    assert_eq!(p.total_bytes, 100_000);
}

#[tokio::test]
async fn rejects_html_hijack_and_unhashed_truncated_jar() {
    let html_server = start_server(vec![(
        "/hijack.jar",
        Behavior::Serve(b"<!DOCTYPE html><html><body>blocked</body></html>".to_vec()),
    )])
    .await;
    let dir = tempfile::tempdir().unwrap();
    let item = DownloadItem {
        urls: vec![html_server.url("/hijack.jar")],
        path: dir.path().join("mod.jar"),
        sha1: None,
        size: None,
    };
    let err = downloader()
        .download_all(&[item], &CancellationToken::new(), |_| {})
        .await
        .unwrap_err();
    match err {
        DownloadError::Failed(failures) => {
            assert!(failures[0].is_integrity_mismatch());
        }
        other => panic!("unexpected {other:?}"),
    }

    // 截断的无哈希 JAR 也必须在结构验尸中被拦截
    let partial_zip = b"PK\x03\x04\x14\x00\x00\x00\x08\x00truncated_jar_no_eocd".to_vec();
    let bad_jar_server = start_server(vec![("/cut.jar", Behavior::Serve(partial_zip))]).await;
    let err2 = downloader()
        .download_all(
            &[DownloadItem {
                urls: vec![bad_jar_server.url("/cut.jar")],
                path: dir.path().join("cut.jar"),
                sha1: None,
                size: None,
            }],
            &CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap_err();
    assert!(matches!(err2, DownloadError::Failed(_)));
}

#[tokio::test]
async fn ram_fast_path_spills_over_1_mib_to_disk_cleanly() {
    let payload = body(1_350_000, 11);
    let expected_sha1 = sha1_of(&payload).await;
    let server = start_server(vec![(
        "/large_unknown.jar",
        Behavior::Serve(payload.clone()),
    )])
    .await;
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("large_unknown.jar");

    downloader()
        .download_all(
            &[DownloadItem {
                urls: vec![server.url("/large_unknown.jar")],
                path: target.clone(),
                sha1: Some(expected_sha1),
                size: None, // 未知大小触发 RAM 快车道 -> >1 MiB 无缝溢出到磁盘
            }],
            &CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap();

    assert_eq!(std::fs::read(&target).unwrap(), payload);
    assert!(no_part_files(dir.path()));
}
