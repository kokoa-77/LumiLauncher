//! 纯本地网络环境指纹记忆（`network-profiles.json`）：
//! - LRU 上限 24 条，单条 TTL 7 天，绝不参加便携导出；
//! - 指纹由本地代理分类 + 计费状态 + 内核选路前缀 SHA-256 截断 16 位 hex 构成（零 SSID / MAC / 公网 IP / PII）。

use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::fs::{read_json, write_json_atomic};
use crate::now_millis;

use super::proxy_watch::ProxyKindTag;
use super::to_hex;

const PROFILE_FORMAT: u32 = 1;
const MAX_PROFILES: usize = 24;
const PROFILE_TTL_MS: u64 = 7 * 24 * 3600 * 1000;
const MAX_HOSTS_PER_PROFILE: usize = 32;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostPerfProfile {
    pub k_opt: u8,
    pub peak_bps: f64,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProfileEntry {
    last_used_ms: u64,
    hosts: HashMap<String, HostPerfProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProfilesFile {
    format: u32,
    profiles: HashMap<String, ProfileEntry>,
}

impl Default for ProfilesFile {
    fn default() -> Self {
        Self {
            format: PROFILE_FORMAT,
            profiles: HashMap::new(),
        }
    }
}

/// 生成当前网络环境的 16 位 SHA-256 脱敏指纹（仅用本地路由前缀、代理粗分类与计费态，零 PII）。
pub fn compute_network_signature(proxy_tag: ProxyKindTag, metered: bool) -> String {
    let v4_prefix = local_ipv4_prefix().unwrap_or_else(|| "none".to_owned());
    let has_v6 = has_ipv6_route();
    let raw = format!("{proxy_tag:?}|metered={metered}|v4={v4_prefix}|v6={has_v6}");
    let digest = Sha256::digest(raw.as_bytes());
    let hex = to_hex(&digest);
    hex[..16].to_owned()
}

fn local_ipv4_prefix() -> Option<String> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    // UDP connect 仅触发内核选路表查表，不发送任何数据包
    sock.connect("1.1.1.1:53").ok()?;
    match sock.local_addr().ok()? {
        SocketAddr::V4(v4) => {
            let [a, b, c, _] = v4.ip().octets();
            Some(format!("{a}.{b}.{c}.0/24"))
        }
        SocketAddr::V6(_) => None,
    }
}

fn has_ipv6_route() -> bool {
    let Ok(sock) = UdpSocket::bind("[::]:0") else {
        return false;
    };
    sock.connect("[2606:4700:4700::1111]:53").is_ok()
}

/// 本地网络环境指纹存储（线程安全、超限 LRU 淘汰与 TTL 清理）。
pub struct NetworkProfileStore {
    path: Option<PathBuf>,
    state: Mutex<ProfilesFile>,
}

impl NetworkProfileStore {
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let mut data = match read_json::<ProfilesFile>(&path) {
            Ok(Some(file)) if file.format <= PROFILE_FORMAT => file,
            _ => ProfilesFile::default(),
        };
        prune_expired_and_cap(&mut data.profiles, now_millis());
        Self {
            path: Some(path),
            state: Mutex::new(data),
        }
    }

    pub fn in_memory() -> Self {
        Self {
            path: None,
            state: Mutex::new(ProfilesFile::default()),
        }
    }

    pub fn get_host(&self, signature: &str, host: &str) -> Option<HostPerfProfile> {
        let now = now_millis();
        let host_key = host.to_ascii_lowercase();
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        prune_expired_and_cap(&mut guard.profiles, now);
        let entry = guard.profiles.get_mut(signature)?;
        let perf = entry.hosts.get(&host_key)?.clone();
        if now.saturating_sub(perf.updated_at_ms) > PROFILE_TTL_MS {
            entry.hosts.remove(&host_key);
            return None;
        }
        entry.last_used_ms = now;
        Some(perf)
    }

    pub fn record_host(&self, signature: &str, host: &str, k_opt: u8, peak_bps: f64) {
        let now = now_millis();
        let host_key = host.to_ascii_lowercase();
        let clamped_k = k_opt.clamp(1, 32);
        let safe_bps = if peak_bps.is_finite() {
            peak_bps.max(0.0)
        } else {
            0.0
        };
        let snapshot = {
            let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let entry = guard.profiles.entry(signature.to_owned()).or_default();
            entry.last_used_ms = now;
            let blended_bps = match entry.hosts.get(&host_key) {
                Some(prev) if prev.peak_bps > 0.0 => prev.peak_bps * 0.4 + safe_bps * 0.6,
                _ => safe_bps,
            };
            entry.hosts.insert(
                host_key,
                HostPerfProfile {
                    k_opt: clamped_k,
                    peak_bps: blended_bps,
                    updated_at_ms: now,
                },
            );
            if entry.hosts.len() > MAX_HOSTS_PER_PROFILE
                && let Some(oldest_host) = entry
                    .hosts
                    .iter()
                    .min_by_key(|(_, v)| v.updated_at_ms)
                    .map(|(k, _)| k.clone())
            {
                entry.hosts.remove(&oldest_host);
            }
            prune_expired_and_cap(&mut guard.profiles, now);
            guard.clone()
        };
        if let Some(path) = &self.path {
            save_async_or_sync(path, &snapshot);
        }
    }

    pub fn clear_all(&self) {
        let snapshot = {
            let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
            guard.profiles.clear();
            guard.clone()
        };
        if let Some(path) = &self.path {
            save_async_or_sync(path, &snapshot);
        }
    }

    pub fn save_now(&self) {
        let Some(path) = &self.path else {
            return;
        };
        let snapshot = self.state.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Err(err) = write_json_atomic(path, &snapshot) {
            tracing::warn!(%err, "保存网络环境指纹失败");
        }
    }
}

fn save_async_or_sync(path: &Path, file: &ProfilesFile) {
    if let Err(err) = write_json_atomic(path, file) {
        tracing::debug!(%err, "写入 network-profiles.json 失败");
    }
}

fn prune_expired_and_cap(profiles: &mut HashMap<String, ProfileEntry>, now: u64) {
    profiles.retain(|_, entry| {
        entry
            .hosts
            .retain(|_, h| now.saturating_sub(h.updated_at_ms) <= PROFILE_TTL_MS);
        !entry.hosts.is_empty() && now.saturating_sub(entry.last_used_ms) <= PROFILE_TTL_MS
    });
    while profiles.len() > MAX_PROFILES {
        let Some(oldest_key) = profiles
            .iter()
            .min_by_key(|(_, entry)| entry.last_used_ms)
            .map(|(k, _)| k.clone())
        else {
            break;
        };
        profiles.remove(&oldest_key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_is_fixed_16_hex_chars() {
        let sig = compute_network_signature(ProxyKindTag::Direct, false);
        assert_eq!(sig.len(), 16);
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(
            compute_network_signature(ProxyKindTag::Direct, false),
            compute_network_signature(ProxyKindTag::SystemProxy, true)
        );
    }

    #[test]
    fn enforces_lru_24_cap_and_persists_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("network-profiles.json");
        let store = NetworkProfileStore::open(&path);

        for i in 0..30 {
            let sig = format!("sig-{i:02}");
            store.record_host(&sig, "cdn.modrinth.com", 8, 20_000_000.0);
        }
        {
            let guard = store.state.lock().unwrap();
            assert_eq!(guard.profiles.len(), MAX_PROFILES);
        }
        let reloaded = NetworkProfileStore::open(&path);
        let perf = reloaded.get_host("sig-29", "cdn.modrinth.com").unwrap();
        assert_eq!(perf.k_opt, 8);
        assert!(perf.peak_bps > 1_000_000.0);
    }
}
