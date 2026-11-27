//! 原子写：先写同目录的临时文件并刷盘，再改名覆盖目标。
//! 中途断电或崩溃时，目标文件要么是旧内容，要么是完整的新内容，不会半截。

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio_util::sync::CancellationToken;

/// Windows Defender（MsMpEng）等实时扫描短期锁文件时的阶梯退避（总计 ~2.24 秒）。
const WIN_LOCK_BACKOFF_MS: &[u64] = &[30, 80, 180, 350, 600, 1000];

/// 判断 I/O 错误是否属于可短时重试的文件占用/锁错误（`PermissionDenied` 或 Windows OS 5/32/33）。
pub fn is_win_lock_error(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::PermissionDenied || matches!(err.raw_os_error(), Some(5 | 32 | 33))
}

fn retry_sync_with_backoff<F>(mut op: F) -> io::Result<()>
where
    F: FnMut() -> io::Result<()>,
{
    let mut attempt = 0usize;
    loop {
        match op() {
            Ok(()) => return Ok(()),
            Err(err) if is_win_lock_error(&err) && attempt < WIN_LOCK_BACKOFF_MS.len() => {
                std::thread::sleep(Duration::from_millis(WIN_LOCK_BACKOFF_MS[attempt]));
                attempt += 1;
            }
            Err(err) => return Err(err),
        }
    }
}

async fn retry_async_with_backoff<F, Fut>(cancel: &CancellationToken, mut op: F) -> io::Result<()>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = io::Result<()>>,
{
    let mut attempt = 0usize;
    loop {
        if cancel.is_cancelled() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        match op().await {
            Ok(()) => return Ok(()),
            Err(err) if is_win_lock_error(&err) && attempt < WIN_LOCK_BACKOFF_MS.len() => {
                let delay = Duration::from_millis(WIN_LOCK_BACKOFF_MS[attempt]);
                attempt += 1;
                tokio::select! {
                    () = tokio::time::sleep(delay) => {}
                    () = cancel.cancelled() => {
                        return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
                    }
                }
            }
            Err(err) => return Err(err),
        }
    }
}

/// 带 Windows 锁文件阶梯退避的同步改名。
pub fn rename_with_win_retry_sync(from: &Path, to: &Path) -> io::Result<()> {
    retry_sync_with_backoff(|| fs::rename(from, to))
}

/// 带 Windows 锁文件阶梯退避的同步删文件（不存在视作成功）。
pub fn remove_with_win_retry_sync(path: &Path) -> io::Result<()> {
    retry_sync_with_backoff(|| match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    })
}

/// 带 Windows 锁文件阶梯退避与取消检查的异步改名。
pub async fn rename_with_win_retry(
    from: &Path,
    to: &Path,
    cancel: &CancellationToken,
) -> io::Result<()> {
    retry_async_with_backoff(cancel, || tokio::fs::rename(from, to)).await
}

/// 带 Windows 锁文件阶梯退避与取消检查的异步删文件（不存在视作成功）。
pub async fn remove_with_win_retry(path: &Path, cancel: &CancellationToken) -> io::Result<()> {
    retry_async_with_backoff(cancel, || async {
        match tokio::fs::remove_file(path).await {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err),
        }
    })
    .await
}

#[derive(Debug, thiserror::Error)]
pub enum AtomicWriteError {
    #[error("目标路径不合法：{0}")]
    InvalidTarget(PathBuf),
    #[error("写入文件失败：{path}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("序列化 JSON 失败")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum ReadJsonError {
    #[error("读取文件失败：{path}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("文件内容不是合法的 JSON：{path}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

/// 原子地把 `bytes` 写到 `path`，父目录不存在时自动创建。
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), AtomicWriteError> {
    write_atomic_from(path, &mut &bytes[..]).map(|_| ())
}

/// 和 [`write_atomic`] 一样，但边读边写（解压整合包里的大文件用，不用先整个读进内存）。
/// 返回写了多少字节。读到一半出错时目标文件不动，临时文件删掉。
pub fn write_atomic_from(path: &Path, reader: &mut impl Read) -> Result<u64, AtomicWriteError> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| AtomicWriteError::InvalidTarget(path.to_path_buf()))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| AtomicWriteError::InvalidTarget(path.to_path_buf()))?;

    fs::create_dir_all(parent).map_err(|source| AtomicWriteError::Io {
        path: parent.to_path_buf(),
        source,
    })?;

    let tmp = parent.join(format!(
        ".{}.{}.tmp",
        file_name.to_string_lossy(),
        uuid::Uuid::new_v4().simple()
    ));

    let result = copy_and_sync(&tmp, reader).and_then(|written| {
        rename_with_win_retry_sync(&tmp, path)?;
        Ok(written)
    });
    if let Err(source) = &result {
        // 尽力清理临时文件；清理失败不掩盖真正的错误，只记日志。
        if let Err(cleanup) = remove_with_win_retry_sync(&tmp) {
            tracing::warn!(tmp = %tmp.display(), error = %cleanup, "清理临时文件失败");
        }
        tracing::debug!(path = %path.display(), error = %source, "原子写失败");
    }
    result.map_err(|source| AtomicWriteError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn copy_and_sync(tmp: &Path, reader: &mut impl Read) -> io::Result<u64> {
    let file = OpenOptions::new().write(true).create_new(true).open(tmp)?;
    let mut writer = io::BufWriter::with_capacity(256 * 1024, file);
    let written = io::copy(reader, &mut writer)?;
    writer.flush()?;
    writer.get_ref().sync_all()?;
    Ok(written)
}

/// 把值格式化成带缩进的 JSON 后原子写入。
pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), AtomicWriteError> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    write_atomic(path, &bytes)
}

/// 读取 JSON 文件。文件不存在时返回 `Ok(None)`。
pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, ReadJsonError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(ReadJsonError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|source| ReadJsonError::Parse {
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Sample {
        name: String,
        count: u32,
    }

    fn leftover_tmp_files(dir: &Path) -> usize {
        fs::read_dir(dir)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp")
            })
            .count()
    }

    #[test]
    fn writes_and_reads_back_creating_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/deeper/sample.json");
        let value = Sample {
            name: "方块".into(),
            count: 3,
        };

        write_json_atomic(&path, &value).unwrap();

        assert_eq!(read_json::<Sample>(&path).unwrap(), Some(value));
        assert_eq!(leftover_tmp_files(path.parent().unwrap()), 0);
    }

    #[test]
    fn overwrites_existing_file_completely() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.json");
        write_atomic(&path, b"a much longer original content").unwrap();

        write_atomic(&path, b"short").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"short");
        assert_eq!(leftover_tmp_files(dir.path()), 0);
    }

    /// 读到一半出错（压缩包损坏）：原来的文件不动，临时文件不留
    #[test]
    fn a_failed_stream_keeps_the_old_file() {
        struct Broken(usize);
        impl Read for Broken {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                if self.0 == 0 {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "坏了"));
                }
                self.0 -= 1;
                buf[0] = b'x';
                Ok(1)
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("world.dat");
        write_atomic(&path, b"old").unwrap();

        assert!(write_atomic_from(&path, &mut Broken(1000)).is_err());

        assert_eq!(fs::read(&path).unwrap(), b"old");
        assert_eq!(leftover_tmp_files(dir.path()), 0);
        assert_eq!(
            write_atomic_from(&path, &mut &b"streamed"[..]).unwrap(),
            8,
            "能读完就写进去，返回写了多少"
        );
        assert_eq!(fs::read(&path).unwrap(), b"streamed");
    }

    #[test]
    fn missing_file_reads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let result = read_json::<Sample>(&dir.path().join("absent.json")).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn corrupt_json_is_a_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        fs::write(&path, b"{ not json").unwrap();

        let err = read_json::<Sample>(&path).unwrap_err();
        assert!(matches!(err, ReadJsonError::Parse { .. }));
    }

    #[test]
    fn recognizes_windows_lock_errors() {
        assert!(is_win_lock_error(&io::Error::new(
            io::ErrorKind::PermissionDenied,
            "locked"
        )));
        for code in [5, 32, 33] {
            assert!(is_win_lock_error(&io::Error::from_raw_os_error(code)));
        }
        assert!(!is_win_lock_error(&io::Error::new(
            io::ErrorKind::NotFound,
            "missing"
        )));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn win_lock_retry_recovers_and_honors_cancel() {
        let mut attempts = 0u32;
        let cancel = CancellationToken::new();
        retry_async_with_backoff(&cancel, || {
            attempts += 1;
            let n = attempts;
            async move {
                if n < 3 {
                    Err(io::Error::from_raw_os_error(32))
                } else {
                    Ok(())
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(attempts, 3);

        let cancelled = CancellationToken::new();
        cancelled.cancel();
        let err = retry_async_with_backoff(&cancelled, || async {
            Err(io::Error::from_raw_os_error(32))
        })
        .await
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Interrupted);
    }
}
