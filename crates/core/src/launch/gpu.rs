//! 高性能显卡：启动前在 Windows「设置 → 系统 → 显示 → 显卡」里给这次用的 java.exe 加一条「高性能」。
//!
//! 写的是当前用户的注册表 `HKCU\Software\Microsoft\DirectX\UserGpuPreferences`（不要管理员权限）：
//! 值的名字是程序的完整路径，内容是 `GpuPreference=2;`（2 = 高性能）。系统设置里的那张列表就是读这里，
//! 所以用户在系统设置里能看到、也能自己改。
//!
//! 我们改过哪些程序记在一个小账本里（应用数据目录的 `gpu-preferences.json`，记下改之前的原值）。
//! 用户关掉这个开关时只撤回账本里的、而且还是我们设的样子的，用户自己原来就设过的不动。

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::fs::{AtomicWriteError, ReadJsonError, read_json, write_json_atomic};

const KEY: &str = r"Software\Microsoft\DirectX\UserGpuPreferences";
const PREFERENCE: &str = "GpuPreference";
const HIGH_PERFORMANCE: &str = "2";

#[derive(Debug, thiserror::Error)]
pub enum GpuError {
    #[error("读写 Windows 显卡设置失败")]
    Registry(#[source] std::io::Error),
    #[error("读取显卡设置记录失败")]
    Read(#[from] ReadJsonError),
    #[error("保存显卡设置记录失败")]
    Write(#[from] AtomicWriteError),
}

/// 我们改过的程序：路径 → 改之前的原值（原来没有这一条为 `None`）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Ledger {
    #[serde(default)]
    changed: BTreeMap<String, Option<String>>,
}

/// `SwapEffectUpgradeEnable=1;GpuPreference=2;` → 按顺序的键值对
fn parse(value: &str) -> Vec<(String, String)> {
    value
        .split(';')
        .filter_map(|pair| {
            let (key, val) = pair.split_once('=')?;
            let key = key.trim();
            (!key.is_empty()).then(|| (key.to_owned(), val.trim().to_owned()))
        })
        .collect()
}

fn format(pairs: &[(String, String)]) -> String {
    pairs.iter().map(|(k, v)| format!("{k}={v};")).collect()
}

/// 已经是高性能了吗
fn is_high_performance(value: &str) -> bool {
    parse(value)
        .iter()
        .any(|(k, v)| k.eq_ignore_ascii_case(PREFERENCE) && v == HIGH_PERFORMANCE)
}

/// 在原值上把 GpuPreference 设成 2，别的键（系统自己加的）原样保留
fn with_high_performance(previous: Option<&str>) -> String {
    let mut pairs = previous.map(parse).unwrap_or_default();
    match pairs
        .iter_mut()
        .find(|(k, _)| k.eq_ignore_ascii_case(PREFERENCE))
    {
        Some((_, v)) => HIGH_PERFORMANCE.clone_into(v),
        None => pairs.push((PREFERENCE.to_owned(), HIGH_PERFORMANCE.to_owned())),
    }
    format(&pairs)
}

/// 注册表里程序路径的写法：反斜杠
fn value_name(exe: &Path) -> String {
    exe.to_string_lossy().replace('/', "\\")
}

/// 撤回时这一条应该变成什么：`None` 表示删掉这一条
fn restored(current: &str, previous: Option<&str>) -> Option<String> {
    if let Some(previous) = previous {
        return Some(previous.to_owned());
    }
    let rest: Vec<_> = parse(current)
        .into_iter()
        .filter(|(k, _)| !k.eq_ignore_ascii_case(PREFERENCE))
        .collect();
    (!rest.is_empty()).then(|| format(&rest))
}

/// 注册表的读写，测试里换成内存里的假注册表
trait Store {
    fn get(&self, name: &str) -> Result<Option<String>, std::io::Error>;
    fn set(&self, name: &str, value: &str) -> Result<(), std::io::Error>;
    fn delete(&self, name: &str) -> Result<(), std::io::Error>;
}

fn prefer_in(store: &dyn Store, exe: &Path, ledger_path: &Path) -> Result<bool, GpuError> {
    let name = value_name(exe);
    let current = store.get(&name).map_err(GpuError::Registry)?;
    if current.as_deref().is_some_and(is_high_performance) {
        return Ok(false);
    }
    let mut ledger: Ledger = read_json(ledger_path)?.unwrap_or_default();
    // 先记账再改：万一改完程序崩了，下次关开关也能撤回
    if !ledger.changed.contains_key(&name) {
        ledger.changed.insert(name.clone(), current.clone());
        write_json_atomic(ledger_path, &ledger)?;
    }
    store
        .set(&name, &with_high_performance(current.as_deref()))
        .map_err(GpuError::Registry)?;
    Ok(true)
}

fn undo_in(store: &dyn Store, ledger_path: &Path) -> Result<usize, GpuError> {
    let ledger: Ledger = read_json(ledger_path)?.unwrap_or_default();
    let mut undone = 0;
    let mut kept = Ledger::default();
    for (name, previous) in &ledger.changed {
        let result = match store.get(name) {
            // 用户后来在系统设置里自己改过：不动
            Ok(Some(current)) if !is_high_performance(&current) => Ok(()),
            Ok(Some(current)) => match restored(&current, previous.as_deref()) {
                Some(value) => store.set(name, &value),
                None => store.delete(name),
            },
            Ok(None) => Ok(()),
            Err(error) => Err(error),
        };
        match result {
            Ok(()) => undone += 1,
            Err(error) => {
                tracing::warn!(%error, program = %name, "撤回高性能显卡设置失败，下次再试");
                kept.changed.insert(name.clone(), previous.clone());
            }
        }
    }
    write_json_atomic(ledger_path, &kept)?;
    Ok(undone)
}

#[cfg(windows)]
struct Registry(winreg::RegKey);

#[cfg(windows)]
impl Store for Registry {
    fn get(&self, name: &str) -> Result<Option<String>, std::io::Error> {
        match self.0.get_value::<String, _>(name) {
            Ok(value) => Ok(Some(value)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
    fn set(&self, name: &str, value: &str) -> Result<(), std::io::Error> {
        self.0.set_value(name, &value)
    }
    fn delete(&self, name: &str) -> Result<(), std::io::Error> {
        match self.0.delete_value(name) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }
}

#[cfg(windows)]
fn registry() -> Result<Registry, GpuError> {
    let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
    let (key, _) = hkcu.create_subkey(KEY).map_err(GpuError::Registry)?;
    Ok(Registry(key))
}

/// 让 `exe` 用高性能显卡。已经是了返回 `false`（没改动）。
#[cfg(windows)]
pub fn prefer_high_performance(exe: &Path, ledger_path: &Path) -> Result<bool, GpuError> {
    let changed = prefer_in(&registry()?, exe, ledger_path)?;
    if changed {
        tracing::info!(program = %exe.display(), "已在 Windows 显卡设置里设为高性能");
    }
    Ok(changed)
}

/// 撤回我们设过的全部「高性能」，返回撤回了几条。
#[cfg(windows)]
pub fn undo_all(ledger_path: &Path) -> Result<usize, GpuError> {
    if !ledger_path.exists() {
        return Ok(0);
    }
    let undone = undo_in(&registry()?, ledger_path)?;
    tracing::info!(undone, "已撤回高性能显卡设置");
    Ok(undone)
}

#[cfg(not(windows))]
pub fn prefer_high_performance(_exe: &Path, _ledger_path: &Path) -> Result<bool, GpuError> {
    Ok(false)
}

#[cfg(not(windows))]
pub fn undo_all(_ledger_path: &Path) -> Result<usize, GpuError> {
    Ok(0)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::*;

    #[derive(Default)]
    struct FakeRegistry(RefCell<HashMap<String, String>>);

    impl Store for FakeRegistry {
        fn get(&self, name: &str) -> Result<Option<String>, std::io::Error> {
            Ok(self.0.borrow().get(name).cloned())
        }
        fn set(&self, name: &str, value: &str) -> Result<(), std::io::Error> {
            self.0
                .borrow_mut()
                .insert(name.to_owned(), value.to_owned());
            Ok(())
        }
        fn delete(&self, name: &str) -> Result<(), std::io::Error> {
            self.0.borrow_mut().remove(name);
            Ok(())
        }
    }

    const JAVA: &str = r"D:\mc\.mcl-java\java-runtime-delta\bin\java.exe";

    #[test]
    fn merges_with_what_windows_already_wrote() {
        assert_eq!(with_high_performance(None), "GpuPreference=2;");
        assert_eq!(
            with_high_performance(Some("SwapEffectUpgradeEnable=1;")),
            "SwapEffectUpgradeEnable=1;GpuPreference=2;"
        );
        assert_eq!(
            with_high_performance(Some("GpuPreference=1;SwapEffectUpgradeEnable=1;")),
            "GpuPreference=2;SwapEffectUpgradeEnable=1;"
        );
        assert!(is_high_performance(
            "SwapEffectUpgradeEnable=0;GpuPreference=2;"
        ));
        assert!(!is_high_performance("GpuPreference=1;"));
    }

    #[test]
    fn sets_and_undoes_a_new_entry() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = dir.path().join("gpu-preferences.json");
        let reg = FakeRegistry::default();

        assert!(prefer_in(&reg, Path::new(JAVA), &ledger).unwrap());
        assert_eq!(reg.get(JAVA).unwrap().as_deref(), Some("GpuPreference=2;"));
        assert!(
            !prefer_in(&reg, Path::new(JAVA), &ledger).unwrap(),
            "第二次不用改"
        );

        assert_eq!(undo_in(&reg, &ledger).unwrap(), 1);
        assert_eq!(reg.get(JAVA).unwrap(), None, "原来没有，撤回后删掉");
        assert_eq!(undo_in(&reg, &ledger).unwrap(), 0, "账本清空了");
    }

    #[test]
    fn undo_restores_the_users_original_choice() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = dir.path().join("gpu-preferences.json");
        let reg = FakeRegistry::default();
        reg.set(JAVA, "GpuPreference=1;SwapEffectUpgradeEnable=1;")
            .unwrap();

        prefer_in(&reg, Path::new(JAVA), &ledger).unwrap();
        undo_in(&reg, &ledger).unwrap();
        assert_eq!(
            reg.get(JAVA).unwrap().as_deref(),
            Some("GpuPreference=1;SwapEffectUpgradeEnable=1;")
        );
    }

    #[test]
    fn leaves_entries_we_did_not_write() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = dir.path().join("gpu-preferences.json");
        let reg = FakeRegistry::default();
        // 用户自己早就设成了高性能：我们不记账，撤回时也不动
        reg.set(JAVA, "GpuPreference=2;").unwrap();
        assert!(!prefer_in(&reg, Path::new(JAVA), &ledger).unwrap());
        assert_eq!(undo_in(&reg, &ledger).unwrap(), 0);
        assert_eq!(reg.get(JAVA).unwrap().as_deref(), Some("GpuPreference=2;"));
    }

    #[test]
    fn keeps_system_keys_and_respects_later_user_changes() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = dir.path().join("gpu-preferences.json");
        let reg = FakeRegistry::default();
        prefer_in(&reg, Path::new(JAVA), &ledger).unwrap();
        // 系统后来自己加了一个键
        reg.set(JAVA, "GpuPreference=2;SwapEffectUpgradeEnable=1;")
            .unwrap();
        undo_in(&reg, &ledger).unwrap();
        assert_eq!(
            reg.get(JAVA).unwrap().as_deref(),
            Some("SwapEffectUpgradeEnable=1;"),
            "只去掉我们加的那一项"
        );

        // 我们设完之后用户在系统设置里改成了「省电」：撤回时不动
        let other = r"C:\Java\bin\java.exe";
        prefer_in(&reg, Path::new(other), &ledger).unwrap();
        reg.set(other, "GpuPreference=1;").unwrap();
        undo_in(&reg, &ledger).unwrap();
        assert_eq!(reg.get(other).unwrap().as_deref(), Some("GpuPreference=1;"));
    }

    #[test]
    fn value_names_use_backslashes() {
        assert_eq!(
            value_name(Path::new("D:/mc/.mcl-java/x/bin/java.exe")),
            r"D:\mc\.mcl-java\x\bin\java.exe"
        );
    }
}
