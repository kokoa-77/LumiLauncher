//! 内容仓库：模组、光影、材质包按 SHA-1 存一份。
//!
//! 规则：**实例里的文件被替换或删除之前，先进仓库**。所以更新、删除都能撤销，
//! 以后的「改动前快照」也只需要记清单（哪个文件名对应哪个哈希），几乎不占空间。
//!
//! 仓库放在游戏目录里（`<游戏目录>/.mcl-store/objects/<前两位>/<哈希>`），和实例同一个盘，
//! 文件进仓库只是改名，不用复制。

use std::io;
use std::path::{Path, PathBuf};

use crate::net::sha1_file;

pub const STORE_DIR: &str = ".mcl-store";

#[derive(Debug, Clone)]
pub struct ContentStore {
    root: PathBuf,
}

impl ContentStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn object(&self, sha1: &str) -> io::Result<PathBuf> {
        if sha1.len() != 40 || !sha1.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("不是有效的 SHA-1：{sha1}"),
            ));
        }
        let sha1 = sha1.to_ascii_lowercase();
        Ok(self.root.join("objects").join(&sha1[..2]).join(sha1))
    }

    pub fn contains(&self, sha1: &str) -> bool {
        self.object(sha1).is_ok_and(|p| p.is_file())
    }

    /// 把实例里的文件**移进**仓库（文件要被删除或替换时用），返回它的 SHA-1。
    /// 仓库里已经有同样的内容时，直接删掉原文件。
    pub async fn take(&self, file: &Path) -> io::Result<String> {
        let sha1 = sha1_file(file).await?;
        self.move_in(file, &sha1).await?;
        Ok(sha1)
    }

    /// 和 [`Self::take`] 一样，但先核对内容是不是 `sha1`（改动日志里记的就是它）：
    /// 对不上就不动，返回错误
    pub async fn take_expect(&self, file: &Path, sha1: &str) -> io::Result<()> {
        let actual = sha1_file(file).await?;
        if !actual.eq_ignore_ascii_case(sha1) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "文件在这期间被改过，请再试一次",
            ));
        }
        self.move_in(file, &actual).await
    }

    async fn move_in(&self, file: &Path, sha1: &str) -> io::Result<()> {
        let object = self.object(sha1)?;
        if object.is_file() {
            tokio::fs::remove_file(file).await?;
        } else {
            if let Some(parent) = object.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            // 同一个盘：改名即可；万一跨盘（游戏目录被软链接到别处）就复制再删
            if tokio::fs::rename(file, &object).await.is_err() {
                copy_atomic(file, &object).await?;
                tokio::fs::remove_file(file).await?;
            }
        }
        Ok(())
    }

    /// 把一个文件**复制**进仓库（原文件不动，补件时用户下载文件夹里的文件就是这样进来的）。
    /// 复制完再核对一次哈希，防止复制途中文件被改；仓库里已经有了就什么都不做。
    pub async fn add_copy(&self, file: &Path, sha1: &str) -> io::Result<()> {
        let object = self.object(sha1)?;
        if object.is_file() {
            return Ok(());
        }
        if let Some(parent) = object.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let tmp = object.with_extension(format!("part-{}", uuid::Uuid::new_v4().simple()));
        tokio::fs::copy(file, &tmp).await?;
        let result = match sha1_file(&tmp).await {
            Ok(actual) if actual.eq_ignore_ascii_case(sha1) => {
                tokio::fs::rename(&tmp, &object).await
            }
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "复制途中文件内容变了，请重新下载",
            )),
            Err(error) => Err(error),
        };
        if result.is_err()
            && let Err(cleanup) = tokio::fs::remove_file(&tmp).await
        {
            tracing::warn!(%cleanup, "清理临时文件失败");
        }
        result
    }

    /// 从仓库**复制**一份放回实例（仓库里保留，方便以后再撤销）。放回前核对哈希。
    pub async fn restore(&self, sha1: &str, dest: &Path) -> io::Result<()> {
        let object = self.object(sha1)?;
        let actual = sha1_file(&object).await?;
        if !actual.eq_ignore_ascii_case(sha1) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("仓库里的文件已损坏（{sha1}）"),
            ));
        }
        copy_atomic(&object, dest).await
    }
}

/// 先复制成临时文件再改名，中途失败不会留下半个文件
pub(super) async fn copy_atomic(from: &Path, to: &Path) -> io::Result<()> {
    if let Some(parent) = to.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = to.with_extension(format!("part-{}", uuid::Uuid::new_v4().simple()));
    tokio::fs::copy(from, &tmp).await?;
    if let Err(error) = tokio::fs::rename(&tmp, to).await {
        if let Err(cleanup) = tokio::fs::remove_file(&tmp).await {
            tracing::warn!(%cleanup, "清理临时文件失败");
        }
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn take_and_restore_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContentStore::new(dir.path().join(STORE_DIR));
        let file = dir.path().join("mods/a.jar");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"mod bytes").unwrap();

        let sha1 = store.take(&file).await.unwrap();
        assert!(!file.exists(), "移进仓库后原文件不在了");
        assert!(store.contains(&sha1));

        store.restore(&sha1, &file).await.unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"mod bytes");
        assert!(store.contains(&sha1), "放回后仓库里还留着");

        // 再移一次同样内容：仓库已有，只删原文件
        assert_eq!(store.take(&file).await.unwrap(), sha1);
        assert!(!file.exists());
    }

    #[tokio::test]
    async fn add_copy_keeps_the_original_and_checks_the_hash() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContentStore::new(dir.path().join(STORE_DIR));
        let file = dir.path().join("Downloads/a.jar");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"mod bytes").unwrap();
        let sha1 = sha1_file(&file).await.unwrap();

        store.add_copy(&file, &sha1).await.unwrap();
        assert!(file.exists(), "原文件还在");
        assert!(store.contains(&sha1));
        store.add_copy(&file, &sha1).await.unwrap();

        let wrong = "0".repeat(40);
        assert!(store.add_copy(&file, &wrong).await.is_err());
        assert!(!store.contains(&wrong), "哈希不对不收");
    }

    #[tokio::test]
    async fn refuses_corrupt_objects_and_bad_hashes() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContentStore::new(dir.path());
        let file = dir.path().join("a.jar");
        std::fs::write(&file, b"x").unwrap();
        let sha1 = store.take(&file).await.unwrap();
        std::fs::write(store.object(&sha1).unwrap(), b"tampered").unwrap();
        assert!(store.restore(&sha1, &file).await.is_err());
        assert!(store.restore("../../evil", &file).await.is_err());
        assert!(!store.contains("zz"));
    }
}
