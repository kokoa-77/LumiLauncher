//! 原子写：先写同目录的临时文件并刷盘，再改名覆盖目标。
//! 中途断电或崩溃时，目标文件要么是旧内容，要么是完整的新内容，不会半截。

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

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

/// 和 [`write_atomic`] 一样，但边读边写（解压整合包里的大文件用，不用先整个读进内存，D17l）。
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
        fs::rename(&tmp, path)?;
        Ok(written)
    });
    if let Err(source) = &result {
        // 尽力清理临时文件；清理失败不掩盖真正的错误，只记日志。
        if let Err(cleanup) = fs::remove_file(&tmp)
            && cleanup.kind() != io::ErrorKind::NotFound
        {
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
}
