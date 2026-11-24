//! 用 DoH（DNS over HTTPS）解析下载域名（设置里「用 DoH 解析下载域名」，默认开）。
//!
//! 有的网络会把 Mojang、Modrinth 的域名解析到错的地址（DNS 污染、运营商劫持），下载就一直失败。
//! 开着时先问 DoH，问不到再用系统 DNS，**不会因为 DoH 坏了就上不了网**：
//! - 海外官方源（Mojang、Modrinth、CurseForge、NeoForge、Fabric……）先问 Cloudflare，其余域名先问阿里，
//!   一边问不到再问另一边。实测：阿里会把 `maven.neoforged.net` 答成污染过的地址（连不上），
//!   Cloudflare 的对。地址直接写 IP，问 DoH 本身不用解析域名
//! - 系统 DNS 给的是 198.18.x.x（代理软件「假地址」模式，代理在接管域名和分流）：DoH 让开，用系统 DNS，
//!   不然拿真实地址直连会绕过代理（实测）
//! - 两个都问不到：这次用系统 DNS，接下来两分钟都直接用系统 DNS（公司、学校网络常常拦 DoH，不让每个域名都等超时）
//! - 问到的地址按 TTL 记一会儿（30 秒到 10 分钟），同一个域名不反复问
//! - `localhost`、没有点的内网名字直接用系统 DNS
//!
//! 设置里关掉后立刻生效（[`Doh::set_enabled`]），不用重启。

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use reqwest::Client;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use serde::Deserialize;

use super::http::{HttpError, USER_AGENT, client_builder};

/// 国内的在前、海外的在后（海外官方源的域名倒过来问，见 [`overseas`]）。
/// 都是 JSON 接口（`?name=域名&type=1`），Cloudflare 要带 `accept: application/dns-json`
const SERVERS: &[&str] = &["https://223.5.5.5/resolve", "https://1.1.1.1/dns-query"];
/// 海外官方源的域名（含子域名）：国内的 DNS 常把它们污染掉，先问海外的 DoH
const OVERSEAS: &[&str] = &[
    "mojang.com",
    "minecraft.net",
    "minecraftservices.com",
    "modrinth.com",
    "curseforge.com",
    "forgecdn.net",
    "neoforged.net",
    "minecraftforge.net",
    "fabricmc.net",
    "quiltmc.org",
    "github.com",
    "githubusercontent.com",
    "adoptium.net",
];
/// 先看一眼系统 DNS 最多等多久（看它是不是代理软件的假地址）
const SYSTEM_PEEK: Duration = Duration::from_millis(1500);
/// 发现代理在接管域名之后，多久之内都直接用系统 DNS
const PROXIED_FOR: Duration = Duration::from_secs(600);
/// 每个 DoH 服务最多等多久
const QUERY_TIMEOUT: Duration = Duration::from_millis(2500);
/// DoH 都问不到之后，多久之内直接用系统 DNS
const SKIP_AFTER_FAILURE: Duration = Duration::from_secs(120);
const MIN_TTL: u64 = 30;
const MAX_TTL: u64 = 600;
const MAX_CACHED: usize = 512;

const TYPE_A: u16 = 1;
const TYPE_AAAA: u16 = 28;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// DoH 解析器。`Arc<Doh>` 交给 [`build_client_with_doh`]，同一份留着在设置变了时开关
pub struct Doh {
    enabled: AtomicBool,
    client: Client,
    servers: Vec<String>,
    cache: Mutex<HashMap<String, Cached>>,
    skip_until: Mutex<Option<Instant>>,
    /// 系统 DNS 是代理软件的假地址：到这个时候之前都用系统 DNS
    proxied_until: Mutex<Option<Instant>>,
}

struct Cached {
    ips: Vec<IpAddr>,
    expires: Instant,
}

#[derive(Deserialize)]
struct Answer {
    #[serde(rename = "Status")]
    status: u32,
    #[serde(rename = "Answer", default)]
    answer: Vec<Record>,
}

#[derive(Deserialize)]
struct Record {
    #[serde(rename = "type")]
    kind: u16,
    #[serde(rename = "TTL", default)]
    ttl: u64,
    data: String,
}

impl Doh {
    pub fn new(enabled: bool) -> Result<Self, HttpError> {
        Self::with_servers(enabled, SERVERS.iter().map(|s| (*s).to_owned()).collect())
    }

    fn with_servers(enabled: bool, servers: Vec<String>) -> Result<Self, HttpError> {
        // 问 DoH 用的客户端用系统默认的解析（地址本身就是 IP，用不上）
        let client = Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_millis(1500))
            .timeout(QUERY_TIMEOUT)
            .build()
            .map_err(HttpError::Build)?;
        Ok(Self {
            enabled: AtomicBool::new(enabled),
            client,
            servers,
            cache: Mutex::new(HashMap::new()),
            skip_until: Mutex::new(None),
            proxied_until: Mutex::new(None),
        })
    }

    pub fn set_enabled(&self, enabled: bool) {
        if self.enabled.swap(enabled, Ordering::Relaxed) != enabled {
            tracing::info!(enabled, "DoH 开关变了");
            self.cache().clear();
            *self.skip() = None;
            *self.proxied() = None;
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, HashMap<String, Cached>> {
        self.cache.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn skip(&self) -> std::sync::MutexGuard<'_, Option<Instant>> {
        self.skip_until.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn proxied(&self) -> std::sync::MutexGuard<'_, Option<Instant>> {
        self.proxied_until.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 最近发现过代理在接管域名
    fn behind_proxy(&self) -> bool {
        self.proxied().is_some_and(|until| Instant::now() < until)
    }

    /// 这个名字要不要问 DoH
    fn wants_doh(&self, host: &str) -> bool {
        if !self.is_enabled() || !needs_public_dns(host) || self.behind_proxy() {
            return false;
        }
        let mut skip = self.skip();
        match *skip {
            Some(until) if Instant::now() < until => false,
            Some(_) => {
                *skip = None;
                true
            }
            None => true,
        }
    }

    fn cached(&self, host: &str) -> Option<Vec<IpAddr>> {
        let mut cache = self.cache();
        match cache.get(host) {
            Some(entry) if Instant::now() < entry.expires => Some(entry.ips.clone()),
            Some(_) => {
                cache.remove(host);
                None
            }
            None => None,
        }
    }

    fn remember(&self, host: &str, ips: &[IpAddr], ttl: u64) {
        let mut cache = self.cache();
        if cache.len() >= MAX_CACHED {
            let now = Instant::now();
            cache.retain(|_, entry| now < entry.expires);
            if cache.len() >= MAX_CACHED {
                cache.clear();
            }
        }
        cache.insert(
            host.to_owned(),
            Cached {
                ips: ips.to_vec(),
                expires: Instant::now() + Duration::from_secs(ttl.clamp(MIN_TTL, MAX_TTL)),
            },
        );
    }

    /// 问 DoH：先问 IPv4，没有再问 IPv6。都问不到返回 `None`。海外官方源先问海外的 DoH
    async fn ask(&self, host: &str) -> Option<(Vec<IpAddr>, u64)> {
        let order: Vec<&String> = if overseas(host) {
            self.servers.iter().rev().collect()
        } else {
            self.servers.iter().collect()
        };
        for server in order {
            match self.query(server, host, TYPE_A).await {
                Ok(Some(found)) => return Some(found),
                Ok(None) => {
                    // 这个服务说没有 IPv4：问一下 IPv6，还没有就是真没有，不用再换服务
                    return self.query(server, host, TYPE_AAAA).await.ok().flatten();
                }
                Err(error) => {
                    tracing::debug!(server = %server, host, error = %error, "DoH 没问到，换下一个");
                }
            }
        }
        None
    }

    /// 问一个 DoH 服务。服务回答了但没有这种地址返回 `Ok(None)`，服务本身出问题返回 `Err`
    async fn query(
        &self,
        server: &str,
        host: &str,
        kind: u16,
    ) -> Result<Option<(Vec<IpAddr>, u64)>, BoxError> {
        let response = self
            .client
            .get(format!(
                "{server}?name={}&type={kind}",
                super::url_encode(host)
            ))
            .header("accept", "application/dns-json")
            .send()
            .await?
            .error_for_status()?;
        let answer: Answer = serde_json::from_slice(&response.bytes().await?)?;
        Ok(parse_answer(&answer, kind))
    }

    async fn lookup(self: Arc<Self>, host: String) -> Result<Vec<SocketAddr>, BoxError> {
        if self.wants_doh(&host) {
            if let Some(ips) = self.cached(&host) {
                return Ok(to_addrs(&ips));
            }
            // 先看一眼系统 DNS：是代理软件的假地址就交给代理，DoH 让开
            let system = tokio::time::timeout(SYSTEM_PEEK, system_lookup(&host)).await;
            if let Ok(Ok(addrs)) = &system
                && !addrs.is_empty()
                && addrs.iter().all(|a| is_fake_ip(a.ip()))
            {
                tracing::info!(
                    host,
                    "系统 DNS 给的是代理软件的假地址（198.18.x.x）：代理在接管域名，DoH 先让开"
                );
                *self.proxied() = Some(Instant::now() + PROXIED_FOR);
                return Ok(addrs.clone());
            }
            if let Some((ips, ttl)) = self.ask(&host).await {
                tracing::debug!(host, count = ips.len(), "DoH 解析");
                self.remember(&host, &ips, ttl);
                return Ok(to_addrs(&ips));
            }
            tracing::warn!(
                host,
                "DoH 问不到，这次用系统 DNS，两分钟内都先用系统 DNS（网络可能拦了 DoH）"
            );
            *self.skip() = Some(Instant::now() + SKIP_AFTER_FAILURE);
            if let Ok(Ok(addrs)) = system {
                return Ok(addrs);
            }
        }
        system_lookup(&host).await
    }
}

/// 「测试 DoH」按钮的一行结果
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct DnsProbe {
    /// 「阿里 DoH」「Cloudflare DoH」「系统 DNS」
    pub name: String,
    /// 问到用了多少毫秒；没问到为空
    pub ms: Option<f64>,
    /// 问到的地址（最多 3 个）
    pub addresses: Vec<String>,
    /// 没问到的原因（中文）
    pub error: Option<String>,
}

/// 测试时问的域名（游戏下载最先用到的那个）
pub const TEST_HOST: &str = "piston-meta.mojang.com";
const SHOWN_ADDRESSES: usize = 3;

impl Doh {
    /// 每个 DoH 服务和系统 DNS 各问一次 [`TEST_HOST`]：不用缓存，开关关着也问（看看开了能不能用）
    pub async fn test(&self) -> Vec<DnsProbe> {
        let doh = self.servers.iter().map(|server| async move {
            let start = Instant::now();
            let result = self.query(server, TEST_HOST, TYPE_A).await;
            let ms = start.elapsed().as_secs_f64() * 1000.0;
            let name = server_label(server);
            match result {
                Ok(Some((ips, _))) => probe_ok(name, ms, ips.iter().map(IpAddr::to_string)),
                Ok(None) => probe_failed(name, "没有这个域名的地址"),
                Err(error) => probe_failed(name, &describe(error.as_ref())),
            }
        });
        let system = async {
            let start = Instant::now();
            let result = system_lookup(TEST_HOST).await;
            let ms = start.elapsed().as_secs_f64() * 1000.0;
            match result {
                Ok(addrs) => probe_ok(
                    if addrs.iter().all(|a| is_fake_ip(a.ip())) {
                        "系统 DNS（代理接管）".to_owned()
                    } else {
                        "系统 DNS".to_owned()
                    },
                    ms,
                    addrs.iter().map(|a| a.ip().to_string()),
                ),
                Err(error) => probe_failed("系统 DNS".to_owned(), &describe(error.as_ref())),
            }
        };
        let (mut probes, system) = tokio::join!(futures_util::future::join_all(doh), system);
        probes.push(system);
        probes
    }
}

fn probe_ok(name: String, ms: f64, addresses: impl Iterator<Item = String>) -> DnsProbe {
    DnsProbe {
        name,
        ms: Some(ms),
        addresses: addresses.take(SHOWN_ADDRESSES).collect(),
        error: None,
    }
}

fn probe_failed(name: String, error: &str) -> DnsProbe {
    DnsProbe {
        name,
        ms: None,
        addresses: Vec::new(),
        error: Some(error.to_owned()),
    }
}

/// 设置页显示的 DoH 服务名
fn server_label(server: &str) -> String {
    let host = super::url_host(server).unwrap_or_else(|| server.to_owned());
    match host.as_str() {
        "223.5.5.5" | "223.6.6.6" => "阿里 DoH".to_owned(),
        "1.1.1.1" | "1.0.0.1" => "Cloudflare DoH".to_owned(),
        other => format!("DoH {other}"),
    }
}

/// 出错的原因说成人话
fn describe(error: &(dyn std::error::Error + Send + Sync + 'static)) -> String {
    match error.downcast_ref::<reqwest::Error>() {
        Some(e) if e.is_timeout() => "超时".to_owned(),
        Some(e) if e.is_connect() => "连不上".to_owned(),
        Some(e) if e.is_status() => format!(
            "服务返回 {}",
            e.status()
                .map_or_else(String::new, |s| s.as_u16().to_string())
        ),
        _ => "出错了".to_owned(),
    }
}

/// 交给 reqwest 的解析器：带着 `Arc<Doh>`，查询时能把它带进异步任务（`Resolve` 只给 `&self`）
#[derive(Clone)]
struct DohResolver(Arc<Doh>);

impl Resolve for DohResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let doh = self.0.clone();
        let host = name.as_str().trim_end_matches('.').to_ascii_lowercase();
        Box::pin(async move {
            doh.lookup(host)
                .await
                .map(|addrs| Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

/// 启动器统一用的 HTTP 客户端，域名先问 DoH（[`Doh`] 关着时和 [`super::build_client`] 一样）
pub fn build_client_with_doh(doh: Arc<Doh>) -> Result<Client, HttpError> {
    build_client_with_proxy(doh, &crate::config::ProxySettings::default())
}

/// 带 DoH 与代理设置的 HTTP 客户端：
/// - `System`：沿用 reqwest 默认跟随 Windows 系统代理；
/// - `Direct`：忽略系统与环境变量代理，直连目标；
/// - `Custom`：使用用户指定的 HTTP 或 SOCKS5 代理。
pub fn build_client_with_proxy(
    doh: Arc<Doh>,
    proxy: &crate::config::ProxySettings,
) -> Result<Client, HttpError> {
    use crate::config::ProxyMode;

    let mut builder = client_builder().dns_resolver(DohResolver(doh));
    match proxy.mode {
        ProxyMode::System => {}
        ProxyMode::Direct => {
            builder = builder.no_proxy();
        }
        ProxyMode::Custom => {
            let Some(url) = proxy.custom_url() else {
                return Err(HttpError::InvalidProxy(format!(
                    "{}:{}",
                    proxy.host, proxy.port
                )));
            };
            let parsed = reqwest::Proxy::all(&url).map_err(HttpError::Build)?;
            builder = builder.proxy(parsed);
        }
    }
    builder.build().map_err(HttpError::Build)
}

/// 代理软件「假地址」（fake-ip）模式用的网段 198.18.0.0/15（Clash、sing-box、v2ray 都默认用它）
fn is_fake_ip(ip: IpAddr) -> bool {
    matches!(ip, IpAddr::V4(v4) if v4.octets()[0] == 198 && v4.octets()[1] & 0xfe == 18)
}

/// 海外官方源的域名（见 [`OVERSEAS`]）
fn overseas(host: &str) -> bool {
    OVERSEAS.iter().any(|suffix| {
        host == *suffix
            || host
                .strip_suffix(suffix)
                .is_some_and(|rest| rest.ends_with('.'))
    })
}

/// 公网域名才问 DoH：`localhost`、没有点的内网名字、`.local` / `.lan` 这类局域网名字用系统 DNS
fn needs_public_dns(host: &str) -> bool {
    if host.parse::<IpAddr>().is_ok() || !host.contains('.') {
        return false;
    }
    let lower = host.to_ascii_lowercase();
    ![".local", ".lan", ".home", ".internal", ".localhost"]
        .iter()
        .any(|suffix| lower.ends_with(suffix))
}

/// 从回答里挑出要的那种地址（跳过 CNAME 这些），TTL 取最小的
fn parse_answer(answer: &Answer, kind: u16) -> Option<(Vec<IpAddr>, u64)> {
    if answer.status != 0 {
        return None;
    }
    let mut ttl = u64::MAX;
    let ips: Vec<IpAddr> = answer
        .answer
        .iter()
        .filter(|r| r.kind == kind)
        .filter_map(|r| {
            let ip = r.data.trim().parse::<IpAddr>().ok()?;
            ttl = ttl.min(r.ttl);
            Some(ip)
        })
        .collect();
    (!ips.is_empty()).then_some((ips, ttl))
}

fn to_addrs(ips: &[IpAddr]) -> Vec<SocketAddr> {
    // 端口填 0：reqwest 会换成网址里的端口（https 默认 443）
    ips.iter().map(|ip| SocketAddr::new(*ip, 0)).collect()
}

async fn system_lookup(host: &str) -> Result<Vec<SocketAddr>, BoxError> {
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, 0)).await?.collect();
    if addrs.is_empty() {
        return Err(format!("{host} 解析不到地址").into());
    }
    Ok(addrs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(json: &str) -> Answer {
        serde_json::from_str(json).unwrap()
    }

    /// 2026-11-14 真实回答（阿里 223.5.5.5）：前面三条是 CNAME，最后才是地址
    #[test]
    fn picks_addresses_and_skips_cnames() {
        let ali = answer(
            r#"{"Status":0,"TC":false,"Question":{"name":"piston-meta.mojang.com.","type":1},"Answer":[
            {"name":"piston-meta.mojang.com.","TTL":266,"type":5,"data":"launcher-meta-cdn.azureedge.net."},
            {"name":"launcher-meta-cdn.azureedge.net.","TTL":1732,"type":5,"data":"launcher-meta-cdn.afd.azureedge.net."},
            {"name":"mr-afd-azuredge.tm-azurefd.net.","TTL":9,"type":1,"data":"150.171.110.100"}]}"#,
        );
        let (ips, ttl) = parse_answer(&ali, TYPE_A).unwrap();
        assert_eq!(ips, ["150.171.110.100".parse::<IpAddr>().unwrap()]);
        assert_eq!(ttl, 9, "TTL 取最小的，记多久再由 MIN_TTL 兜底");
        assert!(parse_answer(&ali, TYPE_AAAA).is_none());
    }

    /// Cloudflare 的「Question」是数组，不影响
    #[test]
    fn reads_cloudflare_answers_too() {
        let cloudflare = answer(
            r#"{"Status":0,"Question":[{"name":"x.com","type":28}],"Answer":[{"name":"x.com","type":28,"TTL":60,"data":"2606:4700::1"}]}"#,
        );
        let (ips, _) = parse_answer(&cloudflare, TYPE_AAAA).unwrap();
        assert!(ips[0].is_ipv6());
    }

    #[test]
    fn no_such_domain_is_no_answer() {
        let nx = answer(r#"{"Status":3,"Question":{"name":"nonexistent.invalid.","type":1}}"#);
        assert!(parse_answer(&nx, TYPE_A).is_none());
    }

    #[test]
    fn local_names_skip_doh() {
        assert!(needs_public_dns("piston-meta.mojang.com"));
        assert!(!needs_public_dns("localhost"));
        assert!(!needs_public_dns("nas"));
        assert!(!needs_public_dns("printer.local"));
        assert!(!needs_public_dns("app.localhost"));
        assert!(!needs_public_dns("127.0.0.1"));
    }

    /// DoH 服务都连不上：用系统 DNS，接下来一段时间不再问 DoH（不让每个域名都等超时）
    #[tokio::test]
    async fn falls_back_to_system_dns_and_stops_asking_for_a_while() {
        // 本机 9 号端口没人听，马上连不上
        let doh = Arc::new(
            Doh::with_servers(true, vec!["https://127.0.0.1:9/resolve".to_owned()]).unwrap(),
        );
        assert!(doh.wants_doh("example.com"));
        // 系统 DNS 这里不一定有网，结果不重要，重要的是问过 DoH 之后进了「先用系统 DNS」
        let _ = doh.clone().lookup("example.com".to_owned()).await;
        assert!(!doh.wants_doh("example.com"), "两分钟内直接用系统 DNS");

        doh.set_enabled(false);
        doh.set_enabled(true);
        assert!(doh.wants_doh("example.com"), "开关一次就重新开始问");
    }

    #[test]
    fn cache_follows_ttl() {
        let doh = Doh::with_servers(true, Vec::new()).unwrap();
        let ip: IpAddr = "1.2.3.4".parse().unwrap();
        doh.remember("a.com", &[ip], 9);
        assert_eq!(doh.cached("a.com"), Some(vec![ip]), "TTL 太短按 30 秒记");
        doh.cache().get_mut("a.com").unwrap().expires = Instant::now();
        assert_eq!(doh.cached("a.com"), None, "过期就不用了");
    }

    #[test]
    fn proxy_fake_ips_are_recognised() {
        for ip in ["198.18.3.1", "198.19.255.1"] {
            assert!(is_fake_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["198.17.0.1", "198.20.0.1", "150.171.110.100"] {
            assert!(!is_fake_ip(ip.parse().unwrap()), "{ip}");
        }
    }

    /// 海外官方源先问 Cloudflare（阿里会污染 maven.neoforged.net），其余域名先问阿里
    #[test]
    fn overseas_official_hosts_ask_cloudflare_first() {
        assert!(overseas("maven.neoforged.net"));
        assert!(overseas("piston-meta.mojang.com"));
        assert!(overseas("cdn.modrinth.com"));
        assert!(overseas("modrinth.com"));
        assert!(!overseas("example.cn"));
        assert!(!overseas("notmodrinth.com"), "不是子域名的不算");
        assert!(!overseas("mirrors.cernet.edu.cn"));
    }

    #[test]
    fn servers_have_readable_names() {
        assert_eq!(server_label("https://223.5.5.5/resolve"), "阿里 DoH");
        assert_eq!(server_label("https://1.1.1.1/dns-query"), "Cloudflare DoH");
        assert_eq!(server_label("https://dns.example/q"), "DoH dns.example");
    }

    /// 测试按钮：DoH 连不上的那行写原因，系统 DNS 那行总在最后
    #[tokio::test]
    async fn test_lists_every_server_and_the_system() {
        let doh = Doh::with_servers(false, vec!["https://127.0.0.1:9/resolve".to_owned()]).unwrap();
        let probes = doh.test().await;
        assert_eq!(probes.len(), 2);
        assert_eq!(probes[0].name, "DoH 127.0.0.1");
        assert!(probes[0].ms.is_none());
        // Windows 上连没人听的端口会重试到超时，别的系统马上被拒
        assert!(
            matches!(probes[0].error.as_deref(), Some("连不上" | "超时")),
            "{:?}",
            probes[0].error
        );
        // 开着代理软件（假地址模式）的电脑上是「系统 DNS（代理接管）」
        assert!(probes[1].name.starts_with("系统 DNS"), "{}", probes[1].name);
    }

    #[test]
    fn turning_off_skips_doh() {
        let doh = Doh::with_servers(true, Vec::new()).unwrap();
        doh.set_enabled(false);
        assert!(!doh.wants_doh("example.com"));
    }

    /// 真的问一次（要联网，平时不跑）：`cargo test -p launcher-core doh -- --ignored`
    #[tokio::test]
    #[ignore = "要联网"]
    async fn resolves_through_real_doh() {
        let doh = Arc::new(Doh::new(true).unwrap());
        let addrs = doh
            .clone()
            .lookup("piston-meta.mojang.com".to_owned())
            .await
            .unwrap();
        assert!(!addrs.is_empty());
        assert!(
            doh.cached("piston-meta.mojang.com").is_some(),
            "问到的是 DoH 的结果"
        );
    }
}
