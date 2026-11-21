//! 多人游戏的服务器列表：实例目录下的 `servers.dat`（不压缩的 NBT）。
//!
//! 资源中心「加入服务器」时往里加一条，游戏里的「多人游戏」就能看到。
//! 读不懂的文件（坏了、里面有我们没保留内容的数组）一律报错、不覆盖，免得把玩家自己加的服务器弄丢。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::nbt::{self, NbtError, Value};
use crate::fs::{AtomicWriteError, write_atomic};

const FILE: &str = "servers.dat";
/// 服务器名最长多少个字（游戏里显示不下太长的）
const MAX_NAME: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerEntry {
    pub name: String,
    pub address: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ServersError {
    #[error("服务器地址不对：{0}")]
    BadAddress(String),
    #[error("读服务器列表失败")]
    Read(#[source] std::io::Error),
    #[error("服务器列表文件读不懂，没有改动它")]
    Corrupt(#[source] NbtError),
    #[error("保存服务器列表失败")]
    Write(#[from] AtomicWriteError),
}

fn path_of(instance_dir: &Path) -> PathBuf {
    instance_dir.join(FILE)
}

/// 地址只认主机名 / IP 加可选端口：字母数字和 `.-_:[]`，不超过 255 个字符
fn valid_address(address: &str) -> bool {
    !address.is_empty()
        && address.len() <= 255
        && address
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-_:[]".contains(&b))
}

fn same_address(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// 读出整个文件的根标签；没有文件时是一个空的根
fn read_root(path: &Path) -> Result<BTreeMap<String, Value>, ServersError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(ServersError::Read(error)),
    };
    match nbt::parse(&bytes).map_err(ServersError::Corrupt)? {
        Value::Compound(map) => Ok(map),
        _ => Err(ServersError::Corrupt(NbtError::NotCompound)),
    }
}

/// 列表里的服务器（读不懂时返回空，只用来显示）
pub fn list(instance_dir: &Path) -> Vec<ServerEntry> {
    let Ok(root) = read_root(&path_of(instance_dir)) else {
        return Vec::new();
    };
    let Some(Value::List(items)) = root.get("servers") else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let address = item.get("ip")?.as_str()?.to_owned();
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(&address)
                .to_owned();
            Some(ServerEntry { name, address })
        })
        .collect()
}

/// 加一个服务器到列表最前面；同一个地址已经在里面就不重复加，返回 `false`。
pub fn add(instance_dir: &Path, name: &str, address: &str) -> Result<bool, ServersError> {
    let address = address.trim();
    if !valid_address(address) {
        return Err(ServersError::BadAddress(address.to_owned()));
    }
    let name: String = name.trim().chars().take(MAX_NAME).collect();
    let path = path_of(instance_dir);
    let mut root = read_root(&path)?;
    let mut servers = match root.remove("servers") {
        Some(Value::List(items)) => items,
        None => Vec::new(),
        Some(_) => return Err(ServersError::Corrupt(NbtError::NotCompound)),
    };
    let exists = servers.iter().any(|item| {
        item.get("ip")
            .and_then(Value::as_str)
            .is_some_and(|ip| same_address(ip, address))
    });
    if exists {
        return Ok(false);
    }
    let mut entry = BTreeMap::new();
    entry.insert(
        "name".to_owned(),
        Value::String(if name.is_empty() {
            address.to_owned()
        } else {
            name
        }),
    );
    entry.insert("ip".to_owned(), Value::String(address.to_owned()));
    servers.insert(0, Value::Compound(entry));
    root.insert("servers".to_owned(), Value::List(servers));
    let bytes = nbt::write(&Value::Compound(root)).map_err(ServersError::Corrupt)?;
    write_atomic(&path, &bytes)?;
    Ok(true)
}

/// 读取一个 `servers.dat` 文件里的完整 NBT 条目列表；文件不存在返回空列表。
fn read_raw_entries(path: &Path) -> Result<(BTreeMap<String, Value>, Vec<Value>), ServersError> {
    let mut root = read_root(path)?;
    let servers = match root.remove("servers") {
        Some(Value::List(items)) => items,
        None => Vec::new(),
        Some(_) => return Err(ServersError::Corrupt(NbtError::NotCompound)),
    };
    Ok((root, servers))
}

fn entry_ip(value: &Value) -> Option<&str> {
    value.get("ip").and_then(Value::as_str).map(str::trim)
}

/// 把 `from` 中的服务器条目按地址无损去重合并进 `to`（完整保留图标与原生 NBT 字段）。
/// 返回新增写入到 `to` 的条目数；`from` 不存在时返回 0。
pub fn merge_files(from: &Path, to: &Path) -> Result<usize, ServersError> {
    if !from.is_file() {
        return Ok(0);
    }
    let (_, from_entries) = read_raw_entries(from)?;
    if from_entries.is_empty() {
        return Ok(0);
    }
    let (mut to_root, mut to_entries) = read_raw_entries(to)?;
    let mut added = 0usize;
    for entry in from_entries {
        let Some(ip) = entry_ip(&entry) else {
            continue;
        };
        if ip.is_empty() {
            continue;
        }
        let exists = to_entries
            .iter()
            .filter_map(entry_ip)
            .any(|existing| same_address(existing, ip));
        if !exists {
            to_entries.push(entry);
            added += 1;
        }
    }
    if added == 0 {
        return Ok(0);
    }
    to_root.insert("servers".to_owned(), Value::List(to_entries));
    let bytes = nbt::write(&Value::Compound(to_root)).map_err(ServersError::Corrupt)?;
    write_atomic(to, &bytes)?;
    Ok(added)
}

/// 校验 `from` 为合法 NBT 后完整覆盖写入 `to`，返回条目数。
pub fn overwrite_file(from: &Path, to: &Path) -> Result<usize, ServersError> {
    if !from.is_file() {
        return Ok(0);
    }
    let (mut root, entries) = read_raw_entries(from)?;
    let count = entries.len();
    root.insert("servers".to_owned(), Value::List(entries));
    let bytes = nbt::write(&Value::Compound(root)).map_err(ServersError::Corrupt)?;
    write_atomic(to, &bytes)?;
    Ok(count)
}

/// 统计 `from` 中有多少个服务器地址在 `to` 中缺失（只读，供同步差异预览）。
pub fn diff_missing_count(from: &Path, to: &Path) -> usize {
    let Ok((_, from_entries)) = read_raw_entries(from) else {
        return 0;
    };
    let Ok((_, to_entries)) = read_raw_entries(to) else {
        return 0;
    };
    from_entries
        .iter()
        .filter_map(entry_ip)
        .filter(|ip| !ip.is_empty())
        .filter(|ip| {
            !to_entries
                .iter()
                .filter_map(entry_ip)
                .any(|existing| same_address(existing, ip))
        })
        .count()
}

/// 统计一个 `servers.dat` 文件里的服务器数量（读不懂或不存在时返回 0）。
pub fn count_file(path: &Path) -> usize {
    read_raw_entries(path)
        .map(|(_, entries)| entries.len())
        .unwrap_or(0)
}

/// 读取指定 `servers.dat` 文件里的服务器条目（供共享同步库可视化管理使用）。
pub fn list_file(path: &Path) -> Vec<ServerEntry> {
    let Ok((_, items)) = read_raw_entries(path) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let address = item.get("ip")?.as_str()?.to_owned();
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(&address)
                .to_owned();
            Some(ServerEntry { name, address })
        })
        .collect()
}

/// 向指定 `servers.dat` 文件新增一条服务器（同地址不重复添加）。
pub fn add_to_file(path: &Path, name: &str, address: &str) -> Result<bool, ServersError> {
    let address = address.trim();
    if !valid_address(address) {
        return Err(ServersError::BadAddress(address.to_owned()));
    }
    let name: String = name.trim().chars().take(MAX_NAME).collect();
    let (mut root, mut servers) = read_raw_entries(path)?;
    let exists = servers
        .iter()
        .filter_map(entry_ip)
        .any(|ip| same_address(ip, address));
    if exists {
        return Ok(false);
    }
    let mut entry = BTreeMap::new();
    entry.insert(
        "name".to_owned(),
        Value::String(if name.is_empty() {
            address.to_owned()
        } else {
            name
        }),
    );
    entry.insert("ip".to_owned(), Value::String(address.to_owned()));
    servers.push(Value::Compound(entry));
    root.insert("servers".to_owned(), Value::List(servers));
    let bytes = nbt::write(&Value::Compound(root)).map_err(ServersError::Corrupt)?;
    write_atomic(path, &bytes)?;
    Ok(true)
}

/// 从指定 `servers.dat` 文件删除匹配地址的服务器，返回是否删除了条目。
pub fn remove_from_file(path: &Path, address: &str) -> Result<bool, ServersError> {
    if !path.is_file() {
        return Ok(false);
    }
    let (mut root, mut servers) = read_raw_entries(path)?;
    let before = servers.len();
    servers.retain(|item| !entry_ip(item).is_some_and(|ip| same_address(ip, address)));
    if servers.len() == before {
        return Ok(false);
    }
    root.insert("servers".to_owned(), Value::List(servers));
    let bytes = nbt::write(&Value::Compound(root)).map_err(ServersError::Corrupt)?;
    write_atomic(path, &bytes)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_once_and_keeps_existing_servers() {
        let dir = tempfile::tempdir().unwrap();
        assert!(list(dir.path()).is_empty());
        assert!(add(dir.path(), "老服务器", "old.example.com").unwrap());
        assert!(add(dir.path(), "宝可梦服", "play.example.com:25566").unwrap());
        // 同一个地址（大小写、空格不同）不重复加
        assert!(!add(dir.path(), "别名", " PLAY.example.com:25566 ").unwrap());
        let servers = list(dir.path());
        assert_eq!(
            servers,
            vec![
                ServerEntry {
                    name: "宝可梦服".into(),
                    address: "play.example.com:25566".into()
                },
                ServerEntry {
                    name: "老服务器".into(),
                    address: "old.example.com".into()
                },
            ]
        );
    }

    #[test]
    fn rejects_bad_addresses() {
        let dir = tempfile::tempdir().unwrap();
        for bad in ["", "a b.com", "x.com\nrm", "主机.com"] {
            assert!(matches!(
                add(dir.path(), "x", bad),
                Err(ServersError::BadAddress(_))
            ));
        }
        assert!(!dir.path().join(FILE).exists());
    }

    #[test]
    fn never_overwrites_a_file_it_cannot_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        std::fs::write(
            &path,
            b"\x0a\x00\x00\x07\x00\x01x\x00\x00\x00\x02\x01\x02\x00",
        )
        .unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(matches!(
            add(dir.path(), "x", "x.com"),
            Err(ServersError::Corrupt(_))
        ));
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}
