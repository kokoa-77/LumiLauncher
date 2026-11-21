//! 用户对 Java 的选择：手动添加的 Java、每个大版本默认用哪个。
//!
//! 存在应用数据目录的 `java.json`（里面是路径，所以不放进前端会整份回传的设置里）。
//! 界面只认 Java 的 ID（[`super::java_id`]），路径只在后端。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

use super::{JavaInstall, JavaOrigin, home_key, select};
use crate::fs::{AtomicWriteError, read_json, write_json_atomic};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaPrefs {
    /// 手动添加的 Java（主目录）
    #[serde(default)]
    pub manual: Vec<PathBuf>,
    /// 大版本 → 默认用的 Java（主目录）；没写的大版本自动挑
    #[serde(default)]
    pub defaults: BTreeMap<u32, PathBuf>,
}

impl JavaPrefs {
    /// 加一个手动添加的 Java。已经有了返回 `false`。
    pub fn add_manual(&mut self, home: PathBuf) -> bool {
        let key = home_key(&home);
        if self.manual.iter().any(|h| home_key(h) == key) {
            return false;
        }
        self.manual.push(home);
        true
    }

    /// 不再记着这个 Java：从手动添加的列表里去掉，它是哪个大版本的默认也一起去掉（变回自动）
    pub fn forget(&mut self, home: &Path) {
        let key = home_key(home);
        self.manual.retain(|h| home_key(h) != key);
        self.defaults.retain(|_, h| home_key(h) != key);
    }

    /// 给一个大版本选默认的 Java；`None` 变回自动
    pub fn set_default(&mut self, major: u32, home: Option<PathBuf>) {
        match home {
            Some(home) => {
                self.defaults.insert(major, home);
            }
            None => {
                self.defaults.remove(&major);
            }
        }
    }
}

/// `java.json` 的读写入口，内存里缓存一份。
pub struct JavaPrefsStore {
    path: PathBuf,
    cached: Mutex<JavaPrefs>,
}

impl JavaPrefsStore {
    /// 打开（或初始化）。文件坏了就当没选过，记一行日志（这里只有几个路径，丢了重新选一下就行）。
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let prefs = match read_json::<JavaPrefs>(&path) {
            Ok(prefs) => prefs.unwrap_or_default(),
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "读取 Java 设置失败，按没选过处理");
                JavaPrefs::default()
            }
        };
        Self {
            path,
            cached: Mutex::new(prefs),
        }
    }

    fn lock(&self) -> MutexGuard<'_, JavaPrefs> {
        self.cached.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn get(&self) -> JavaPrefs {
        self.lock().clone()
    }

    /// 改一下并保存。保存失败时内存里的值不变。
    pub fn update<R>(
        &self,
        change: impl FnOnce(&mut JavaPrefs) -> R,
    ) -> Result<R, AtomicWriteError> {
        let mut cached = self.lock();
        let mut next = cached.clone();
        let result = change(&mut next);
        if next != *cached {
            write_json_atomic(&self.path, &next)?;
            *cached = next;
        }
        Ok(result)
    }
}

/// 设置里 Java 列表的一项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct JavaEntry {
    pub id: String,
    pub major: u32,
    /// 完整版本号，例如 `21.0.7`
    pub version: String,
    /// 谁做的，例如 Microsoft
    pub vendor: Option<String>,
    pub is_64bit: bool,
    pub origin: JavaOrigin,
    /// 在哪（只用来显示，界面不拿它做任何事）
    pub location: String,
    /// 用户给这个大版本选的就是它
    pub chosen: bool,
    /// 这个大版本没选时，自动会用它
    pub auto: bool,
}

/// 设置里的 Java 列表。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct JavaOverview {
    /// 按大版本从新到旧，同一个大版本里新的在前
    pub entries: Vec<JavaEntry>,
    /// 开发开关 `MCL_IGNORE_SYSTEM_JAVA` 开着：只认启动器下载的
    pub system_ignored: bool,
}

/// 把扫描结果和用户的选择合成设置里的列表。
pub fn overview(installs: &[JavaInstall], prefs: &JavaPrefs, system_ignored: bool) -> JavaOverview {
    let entries = installs
        .iter()
        .map(|install| {
            let key = home_key(&install.home);
            let chosen = prefs
                .defaults
                .get(&install.major)
                .is_some_and(|home| home_key(home) == key);
            let auto =
                select(installs, install.major).is_some_and(|best| home_key(&best.home) == key);
            JavaEntry {
                id: install.id(),
                major: install.major,
                version: install.version.clone(),
                vendor: install.vendor.clone(),
                is_64bit: install.is_64bit,
                origin: install.origin,
                location: install.home.to_string_lossy().into_owned(),
                chosen,
                auto,
            }
        })
        .collect();
    JavaOverview {
        entries,
        system_ignored,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install(home: &str, version: &str, origin: JavaOrigin) -> JavaInstall {
        JavaInstall {
            home: PathBuf::from(home),
            executable: PathBuf::from(home).join("bin").join("java.exe"),
            version: version.to_owned(),
            major: version.split('.').next().unwrap().parse().unwrap(),
            is_64bit: true,
            origin,
            vendor: Some("Microsoft".to_owned()),
        }
    }

    #[test]
    fn manual_entries_are_not_duplicated_and_forgetting_resets_defaults() {
        let mut prefs = JavaPrefs::default();
        assert!(prefs.add_manual(PathBuf::from(r"C:\Java\jdk-21")));
        assert!(
            !prefs.add_manual(PathBuf::from("c:/java/JDK-21/")),
            "同一个目录"
        );
        prefs.set_default(21, Some(PathBuf::from(r"C:\Java\jdk-21")));
        prefs.set_default(17, Some(PathBuf::from(r"C:\Java\jdk-17")));

        prefs.forget(Path::new(r"C:\Java\jdk-21"));
        assert!(prefs.manual.is_empty());
        assert!(!prefs.defaults.contains_key(&21), "删掉的那个不再是默认");
        assert!(prefs.defaults.contains_key(&17));

        prefs.set_default(17, None);
        assert!(prefs.defaults.is_empty());
    }

    #[test]
    fn store_saves_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("java.json");
        let store = JavaPrefsStore::open(&path);
        assert_eq!(store.get(), JavaPrefs::default());
        store
            .update(|p| {
                p.add_manual(PathBuf::from(r"C:\Java\jdk-21"));
                p.set_default(21, Some(PathBuf::from(r"C:\Java\jdk-21")));
            })
            .unwrap();
        let reopened = JavaPrefsStore::open(&path).get();
        assert_eq!(reopened.manual, [PathBuf::from(r"C:\Java\jdk-21")]);
        assert_eq!(reopened.defaults[&21], PathBuf::from(r"C:\Java\jdk-21"));
    }

    #[test]
    fn broken_file_counts_as_nothing_chosen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("java.json");
        std::fs::write(&path, "{ broken").unwrap();
        assert_eq!(JavaPrefsStore::open(&path).get(), JavaPrefs::default());
    }

    #[test]
    fn overview_marks_chosen_and_automatic_entries() {
        let installs = [
            install(r"C:\Java\jdk-21.0.7", "21.0.7", JavaOrigin::System),
            install(
                r"D:\mc\.mcl-java\java-runtime-delta",
                "21.0.3",
                JavaOrigin::Downloaded,
            ),
            install(r"C:\Java\jdk-17", "17.0.12", JavaOrigin::Manual),
        ];
        let mut prefs = JavaPrefs::default();
        prefs.set_default(
            21,
            Some(PathBuf::from(r"D:\mc\.mcl-java\java-runtime-delta")),
        );

        let list = overview(&installs, &prefs, false);
        let flags: Vec<_> = list
            .entries
            .iter()
            .map(|e| (e.version.as_str(), e.chosen, e.auto))
            .collect();
        assert_eq!(
            flags,
            [
                ("21.0.7", false, true),
                ("21.0.3", true, false),
                ("17.0.12", false, true),
            ]
        );
        assert_eq!(list.entries[0].id.len(), 12);
        assert_eq!(
            list.entries[1].location,
            r"D:\mc\.mcl-java\java-runtime-delta"
        );
    }
}
