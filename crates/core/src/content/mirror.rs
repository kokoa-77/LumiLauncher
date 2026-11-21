//! Modrinth、CurseForge 接口的镜像：官方失败或太慢时改用 MCIM（www.mcimirror.top）。
//!
//! 按设置「内容与下载」里 Modrinth、CurseForge 两行下载源（[`set_policies`]）：
//! - 自动（默认）：先问官方，5 秒没回或者失败就改问 MCIM，接下来 10 分钟都先问 MCIM（不让每次都白等官方）
//! - 优先官方源：先问官方（等正常的超时），失败才问 MCIM
//! - 优先 MCIM：先问 MCIM，失败才问官方
//! - 仅官方源：不碰 MCIM
//!
//! MCIM 的接口返回的是缓存：条目里带 `sync_at`（上次从官方拉取）和 `checked_at`（上次和官方核对）。
//! 超过 7 天没核对过的算太旧（[`is_stale`]）：后面还能问官方就先问官方，官方也不行才用旧的。
//! MCIM 回 404 是还没缓存（它会排队去抓）：后面还能问官方就问官方。
//! 官方回 400 / 404 / 410 这类「没有这个东西」不换镜像（镜像也不会有）；401 / 403（Key 被拒）、429、5xx、超时、连不上才换。
//! **CurseForge 的 Key 只发给官方**，请求镜像不带 Key（MCIM 不需要）。

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use reqwest::{Client, Method, StatusCode};

use crate::config::DownloadSource;

/// MCIM 的接口地址（官方地址的前缀换成它，路径和参数不变）
const MCIM_MODRINTH: &str = "https://mod.mcimirror.top/modrinth";
const MCIM_CURSEFORGE: &str = "https://mod.mcimirror.top/curseforge";
/// 「自动」时官方最多等多久，过了就改问 MCIM
const AUTO_PATIENCE: Duration = Duration::from_secs(5);
/// 「自动」时官方不行之后，多久之内先问 MCIM
const PREFER_MIRROR_FOR: Duration = Duration::from_secs(600);
/// MCIM 的缓存多久没和官方核对过算太旧
const STALE_AFTER: Duration = Duration::from_secs(7 * 24 * 3600);

static MODRINTH: LazyLock<Route> =
    LazyLock::new(|| Route::new("Modrinth", "https://api.modrinth.com", MCIM_MODRINTH));
static CURSEFORGE: LazyLock<Route> =
    LazyLock::new(|| Route::new("CurseForge", "https://api.curseforge.com", MCIM_CURSEFORGE));

/// Modrinth 的接口
pub fn modrinth() -> &'static Route {
    &MODRINTH
}

/// CurseForge 的接口
pub fn curseforge() -> &'static Route {
    &CURSEFORGE
}

/// 设置改了：两行下载源立刻生效（启动时、保存设置时调）
pub fn set_policies(modrinth: DownloadSource, curseforge: DownloadSource) {
    MODRINTH.set_policy(modrinth);
    CURSEFORGE.set_policy(curseforge);
}

/// 一个平台的接口：官方地址、MCIM 地址、现在的下载源设置
pub struct Route {
    label: &'static str,
    official: String,
    mirror: String,
    policy: AtomicU8,
    patience: Duration,
    /// 「自动」时官方不行了：到这个时候之前先问 MCIM
    prefer_mirror_until: Mutex<Option<Instant>>,
}

/// 要问的一个地方
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// 官方；`patient` 为假时最多等 [`Route::patience`]
    Official {
        patient: bool,
    },
    Mirror,
}

/// 问到的内容
#[derive(Debug)]
pub struct Fetched {
    pub bytes: Vec<u8>,
    /// 是不是从 MCIM 来的
    pub from_mirror: bool,
}

/// 都没问到
#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    /// 官方明确说没有（400 / 404 / 410），换镜像也没用
    #[error("{label} 返回 HTTP {status}")]
    Rejected { label: &'static str, status: u16 },
    /// 官方拒绝了 Key（401 / 403），镜像也没问到
    #[error("{label} 拒绝了请求（HTTP {status}）")]
    Denied { label: &'static str, status: u16 },
    /// 官方和镜像都没问到（最后一个原因）
    #[error("{label} 和 MCIM 镜像都没问到：{reason}")]
    Unavailable { label: &'static str, reason: String },
}

/// 一次尝试的结果
enum Attempt {
    Ok(Vec<u8>),
    /// 换下一个地方还有希望
    Next(String),
    /// 官方明确说没有：不用再试
    Final(FetchError),
}

fn encode_policy(policy: DownloadSource) -> u8 {
    match policy {
        DownloadSource::Auto => 0,
        DownloadSource::Official => 1,
        DownloadSource::Mirror => 2,
        DownloadSource::OfficialOnly => 3,
    }
}

fn decode_policy(value: u8) -> DownloadSource {
    match value {
        1 => DownloadSource::Official,
        2 => DownloadSource::Mirror,
        3 => DownloadSource::OfficialOnly,
        _ => DownloadSource::Auto,
    }
}

impl Route {
    pub fn new(label: &'static str, official: &str, mirror: &str) -> Self {
        Self {
            label,
            official: official.trim_end_matches('/').to_owned(),
            mirror: mirror.trim_end_matches('/').to_owned(),
            policy: AtomicU8::new(encode_policy(DownloadSource::Auto)),
            patience: AUTO_PATIENCE,
            prefer_mirror_until: Mutex::new(None),
        }
    }

    pub fn set_policy(&self, policy: DownloadSource) {
        let old = self.policy.swap(encode_policy(policy), Ordering::Relaxed);
        if old != encode_policy(policy) {
            tracing::info!(platform = self.label, ?policy, "接口下载源改了");
            *self.preferred() = None;
        }
    }

    pub fn policy(&self) -> DownloadSource {
        decode_policy(self.policy.load(Ordering::Relaxed))
    }

    fn preferred(&self) -> std::sync::MutexGuard<'_, Option<Instant>> {
        self.prefer_mirror_until
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// 官方地址换成 MCIM 地址；不是这个平台官方接口的地址返回 `None`
    fn mirror_url(&self, official_url: &str) -> Option<String> {
        let rest = official_url.strip_prefix(&self.official)?;
        (rest.is_empty() || rest.starts_with('/')).then(|| format!("{}{rest}", self.mirror))
    }

    /// 按设置排出这次要问的顺序。`official_usable` 为假（CurseForge 没有 Key）时不问官方
    fn plan(&self, official_usable: bool, has_mirror: bool) -> Vec<Target> {
        let policy = self.policy();
        let mirror_first = match policy {
            DownloadSource::Mirror => true,
            DownloadSource::Auto => self.preferred().is_some_and(|until| Instant::now() < until),
            DownloadSource::Official | DownloadSource::OfficialOnly => false,
        };
        let official = Target::Official {
            patient: !(policy == DownloadSource::Auto && !mirror_first),
        };
        let mut order = if mirror_first {
            vec![Target::Mirror, official]
        } else {
            vec![official, Target::Mirror]
        };
        order.retain(|target| match target {
            Target::Official { .. } => official_usable,
            Target::Mirror => has_mirror && policy != DownloadSource::OfficialOnly,
        });
        order
    }

    /// 问一次（GET 或 POST JSON）。`key` 是只发给官方的请求头（CurseForge 的 `x-api-key`）。
    pub async fn fetch(
        &self,
        client: &Client,
        method: Method,
        official_url: &str,
        body: Option<&[u8]>,
        key: Option<(&str, &str)>,
        official_usable: bool,
    ) -> Result<Fetched, FetchError> {
        let mirror_url = self.mirror_url(official_url);
        let plan = self.plan(official_usable, mirror_url.is_some());
        let mut last = String::from("没有可以问的地方");
        let mut denied: Option<u16> = None;
        let mut stale: Option<Vec<u8>> = None;
        for (index, target) in plan.iter().enumerate() {
            let official_later = plan[index + 1..]
                .iter()
                .any(|t| matches!(t, Target::Official { .. }));
            match *target {
                Target::Official { patient } => {
                    let attempt = self
                        .attempt(client, &method, official_url, body, key, !patient)
                        .await;
                    match attempt {
                        Attempt::Ok(bytes) => {
                            if self.policy() == DownloadSource::Auto {
                                *self.preferred() = None;
                            }
                            return Ok(Fetched {
                                bytes,
                                from_mirror: false,
                            });
                        }
                        Attempt::Final(error) => return Err(error),
                        Attempt::Next(reason) => {
                            if let Some(status) = denied_status(&reason) {
                                denied = Some(status);
                            }
                            if self.policy() == DownloadSource::Auto {
                                *self.preferred() = Some(Instant::now() + PREFER_MIRROR_FOR);
                            }
                            tracing::warn!(platform = self.label, url = official_url, %reason, "官方接口没问到，换 MCIM");
                            last = reason;
                        }
                    }
                }
                Target::Mirror => {
                    let Some(url) = mirror_url.as_deref() else {
                        continue;
                    };
                    match self.attempt(client, &method, url, body, None, false).await {
                        Attempt::Ok(bytes) => {
                            if official_later && is_stale(&bytes, now_secs()) {
                                tracing::info!(
                                    platform = self.label,
                                    url,
                                    "MCIM 的缓存太旧，先问官方"
                                );
                                stale = Some(bytes);
                                continue;
                            }
                            return Ok(Fetched {
                                bytes,
                                from_mirror: true,
                            });
                        }
                        Attempt::Next(reason) => {
                            tracing::warn!(platform = self.label, url, %reason, "MCIM 没问到");
                            last = format!("MCIM：{reason}");
                        }
                        Attempt::Final(other) => last = format!("MCIM：{other}"),
                    }
                }
            }
        }
        if let Some(bytes) = stale {
            tracing::warn!(
                platform = self.label,
                url = official_url,
                "官方也没问到，先用 MCIM 的旧缓存"
            );
            return Ok(Fetched {
                bytes,
                from_mirror: true,
            });
        }
        if let Some(status) = denied {
            return Err(FetchError::Denied {
                label: self.label,
                status,
            });
        }
        Err(FetchError::Unavailable {
            label: self.label,
            reason: last,
        })
    }

    async fn attempt(
        &self,
        client: &Client,
        method: &Method,
        url: &str,
        body: Option<&[u8]>,
        key: Option<(&str, &str)>,
        impatient: bool,
    ) -> Attempt {
        let mut request = client
            .request(method.clone(), url)
            .header(reqwest::header::ACCEPT, "application/json");
        if let Some((name, value)) = key {
            request = request.header(name, value);
        }
        if let Some(body) = body {
            request = request
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.to_vec());
        }
        let is_official = url.starts_with(&self.official);
        let fetch = async {
            let response = request.send().await?;
            let status = response.status();
            let bytes = response.bytes().await?;
            Ok::<_, reqwest::Error>((status, bytes))
        };
        let result = if impatient {
            match tokio::time::timeout(self.patience, fetch).await {
                Ok(result) => result,
                Err(_) => {
                    return Attempt::Next(format!("{} 秒没回", self.patience.as_secs()));
                }
            }
        } else {
            fetch.await
        };
        let (status, bytes) = match result {
            Ok(found) => found,
            Err(error) => return Attempt::Next(format!("连不上：{error}")),
        };
        if status.is_success() {
            return Attempt::Ok(bytes.to_vec());
        }
        if is_official
            && matches!(
                status,
                StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND | StatusCode::GONE
            )
        {
            return Attempt::Final(FetchError::Rejected {
                label: self.label,
                status: status.as_u16(),
            });
        }
        if !is_official && status == StatusCode::NOT_FOUND {
            return Attempt::Next("还没缓存（MCIM 会去抓，稍后再试）".to_owned());
        }
        Attempt::Next(format!("HTTP {}", status.as_u16()))
    }
}

/// 失败原因里带的 401 / 403（Key 被拒）
fn denied_status(reason: &str) -> Option<u16> {
    ["HTTP 401", "HTTP 403"]
        .iter()
        .find(|code| reason == **code)
        .and_then(|code| code.trim_start_matches("HTTP ").parse().ok())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// MCIM 返回的内容太旧了吗：找所有条目里最早的 `checked_at`（没有就用 `sync_at`），
/// 超过 [`STALE_AFTER`] 就算旧。没有这两个字段（搜索结果）不算旧。
pub fn is_stale(bytes: &[u8], now: u64) -> bool {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return false;
    };
    let mut oldest: Option<u64> = None;
    collect_checked(&value, &mut oldest);
    oldest.is_some_and(|at| now.saturating_sub(at) > STALE_AFTER.as_secs())
}

fn collect_checked(value: &serde_json::Value, oldest: &mut Option<u64>) {
    match value {
        serde_json::Value::Object(map) => {
            let stamp = map
                .get("checked_at")
                .or_else(|| map.get("sync_at"))
                .and_then(serde_json::Value::as_str)
                .and_then(parse_utc);
            if let Some(at) = stamp {
                *oldest = Some(oldest.map_or(at, |old| old.min(at)));
            }
            for child in map.values() {
                collect_checked(child, oldest);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_checked(item, oldest);
            }
        }
        _ => {}
    }
}

/// `2026-11-15T11:10:38.443Z` → Unix 秒数（只认 UTC 的 `Z` 结尾和 `+00:00`）
fn parse_utc(text: &str) -> Option<u64> {
    let text = text.trim();
    let body = text
        .strip_suffix('Z')
        .or_else(|| text.strip_suffix("+00:00"))?;
    let (date, time) = body.split_once('T')?;
    let mut date_parts = date.split('-').map(str::parse::<i64>);
    let (year, month, day) = (
        date_parts.next()?.ok()?,
        date_parts.next()?.ok()?,
        date_parts.next()?.ok()?,
    );
    let time = time.split('.').next()?;
    let mut time_parts = time.split(':').map(str::parse::<i64>);
    let (hour, minute, second) = (
        time_parts.next()?.ok()?,
        time_parts.next()?.ok()?,
        time_parts.next()?.ok()?,
    );
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // 公历日期 → 从 1970-01-01 起的天数（Howard Hinnant 的 days_from_civil）
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + hour * 3600 + minute * 60 + second).ok()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    /// 真的问一次 MCIM（要联网，平时不跑）：`cargo test -p launcher-core real_mcim -- --ignored`
    #[tokio::test]
    #[ignore = "要联网"]
    async fn real_mcim_answers_modrinth_and_curseforge() {
        let client = crate::net::build_client().unwrap();
        let modrinth = Route::new("Modrinth", "https://api.modrinth.com", MCIM_MODRINTH);
        modrinth.set_policy(DownloadSource::Mirror);
        let got = modrinth
            .fetch(
                &client,
                Method::GET,
                "https://api.modrinth.com/v2/project/AANobbMI",
                None,
                None,
                true,
            )
            .await
            .unwrap();
        assert!(got.from_mirror);
        // CurseForge 没 Key 时只问镜像
        let curseforge = Route::new("CurseForge", "https://api.curseforge.com", MCIM_CURSEFORGE);
        let got = curseforge
            .fetch(
                &client,
                Method::GET,
                "https://api.curseforge.com/v1/mods/394468",
                None,
                None,
                false,
            )
            .await
            .unwrap();
        assert!(got.from_mirror);
        assert!(String::from_utf8_lossy(&got.bytes).contains("Sodium"));
    }

    #[test]
    fn parses_mcim_timestamps() {
        assert_eq!(parse_utc("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_utc("2026-11-15T11:10:38.443Z"), Some(1_794_741_038));
        assert_eq!(parse_utc("2024-02-29T00:00:00+00:00"), Some(1_709_164_800));
        assert_eq!(parse_utc("2026-11-15 11:10:38"), None);
        assert_eq!(parse_utc("2026-13-01T00:00:00Z"), None);
    }

    #[test]
    fn stale_means_not_checked_for_a_week() {
        let now = parse_utc("2026-11-15T00:00:00Z").unwrap();
        let fresh =
            br#"{"id":"a","sync_at":"2026-11-01T00:00:00Z","checked_at":"2026-11-14T00:00:00Z"}"#;
        assert!(!is_stale(fresh, now), "核对过就不算旧，哪怕很久没变");
        let old =
            br#"[{"checked_at":"2026-11-14T00:00:00Z"},{"checked_at":"2026-11-01T00:00:00Z"}]"#;
        assert!(is_stale(old, now), "有一条太旧就算旧");
        let only_sync = br#"{"data":{"sync_at":"2026-10-01T00:00:00Z"}}"#;
        assert!(is_stale(only_sync, now));
        assert!(
            !is_stale(br#"{"hits":[]}"#, now),
            "搜索结果没有日期，不算旧"
        );
        assert!(!is_stale(b"not json", now));
    }

    #[test]
    fn plan_follows_the_policy() {
        let route = Route::new("Modrinth", "https://api.modrinth.com", MCIM_MODRINTH);
        let official = Target::Official { patient: true };
        let impatient = Target::Official { patient: false };
        assert_eq!(
            route.plan(true, true),
            [impatient, Target::Mirror],
            "自动：官方只等一会儿"
        );
        route.set_policy(DownloadSource::Official);
        assert_eq!(route.plan(true, true), [official, Target::Mirror]);
        route.set_policy(DownloadSource::Mirror);
        assert_eq!(route.plan(true, true), [Target::Mirror, official]);
        route.set_policy(DownloadSource::OfficialOnly);
        assert_eq!(route.plan(true, true), [official]);
        route.set_policy(DownloadSource::Auto);
        assert_eq!(route.plan(false, true), [Target::Mirror], "没 Key 不问官方");
        assert_eq!(
            route
                .mirror_url("https://api.modrinth.com/v2/project/a?x=1")
                .as_deref(),
            Some("https://mod.mcimirror.top/modrinth/v2/project/a?x=1")
        );
        assert_eq!(route.mirror_url("https://api.modrinth.com.evil/v2"), None);
    }

    /// 本机小服务器：路径 → （等多少毫秒，状态码，内容）；记下每个请求的路径和有没有带 Key
    #[derive(Clone)]
    struct Reply {
        delay_ms: u64,
        status: u16,
        body: String,
    }

    type Log = Arc<Mutex<Vec<(String, bool)>>>;

    async fn server(routes: HashMap<&'static str, Reply>) -> (String, Log) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let log: Log = Arc::new(Mutex::new(Vec::new()));
        let seen = log.clone();
        let routes = Arc::new(routes);
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let routes = routes.clone();
                let seen = seen.clone();
                tokio::spawn(async move {
                    let mut buffer = vec![0u8; 8192];
                    let n = socket.read(&mut buffer).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buffer[..n]).into_owned();
                    let path = request.split_whitespace().nth(1).unwrap_or("").to_owned();
                    let keyed = request.to_ascii_lowercase().contains("x-api-key:");
                    seen.lock().unwrap().push((path.clone(), keyed));
                    let reply = routes.get(path.as_str()).cloned().unwrap_or(Reply {
                        delay_ms: 0,
                        status: 404,
                        body: "{}".to_owned(),
                    });
                    tokio::time::sleep(Duration::from_millis(reply.delay_ms)).await;
                    let text = format!(
                        "HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        reply.status,
                        reply.body.len(),
                        reply.body
                    );
                    let _ = socket.write_all(text.as_bytes()).await;
                });
            }
        });
        (base, log)
    }

    fn reply(status: u16, body: &str) -> Reply {
        Reply {
            delay_ms: 0,
            status,
            body: body.to_owned(),
        }
    }

    /// 官方和镜像各一个本机服务器
    async fn setup(
        official: HashMap<&'static str, Reply>,
        mirror: HashMap<&'static str, Reply>,
    ) -> (Route, String, Log, Log) {
        let (official_base, official_log) = server(official).await;
        let (mirror_base, mirror_log) = server(mirror).await;
        let mut route = Route::new("Modrinth", &official_base, &format!("{mirror_base}/mirror"));
        route.patience = Duration::from_millis(300);
        (route, official_base, official_log, mirror_log)
    }

    fn client() -> Client {
        Client::builder().no_proxy().build().unwrap()
    }

    #[tokio::test]
    async fn auto_uses_the_mirror_when_official_fails_and_remembers_it() {
        let (route, base, official_log, mirror_log) = setup(
            HashMap::from([("/v2/a", reply(503, "{}"))]),
            HashMap::from([("/mirror/v2/a", reply(200, r#"{"from":"mcim"}"#))]),
        )
        .await;
        let url = format!("{base}/v2/a");
        let got = route
            .fetch(&client(), Method::GET, &url, None, None, true)
            .await
            .unwrap();
        assert!(got.from_mirror);
        assert_eq!(got.bytes, br#"{"from":"mcim"}"#);
        // 接下来先问 MCIM，不再等官方
        route
            .fetch(&client(), Method::GET, &url, None, None, true)
            .await
            .unwrap();
        assert_eq!(official_log.lock().unwrap().len(), 1, "第二次没去问官方");
        assert_eq!(mirror_log.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn auto_gives_up_on_a_slow_official_api() {
        let (route, base, _, _) = setup(
            HashMap::from([(
                "/v2/slow",
                Reply {
                    delay_ms: 1500,
                    status: 200,
                    body: r#"{"from":"official"}"#.to_owned(),
                },
            )]),
            HashMap::from([("/mirror/v2/slow", reply(200, r#"{"from":"mcim"}"#))]),
        )
        .await;
        let got = route
            .fetch(
                &client(),
                Method::GET,
                &format!("{base}/v2/slow"),
                None,
                None,
                true,
            )
            .await
            .unwrap();
        assert!(got.from_mirror, "官方太慢就改问 MCIM");
    }

    #[tokio::test]
    async fn official_not_found_is_final() {
        let (route, base, _, mirror_log) = setup(
            HashMap::from([("/v2/missing", reply(404, "{}"))]),
            HashMap::new(),
        )
        .await;
        let error = route
            .fetch(
                &client(),
                Method::GET,
                &format!("{base}/v2/missing"),
                None,
                None,
                true,
            )
            .await
            .unwrap_err();
        assert!(matches!(error, FetchError::Rejected { status: 404, .. }));
        assert!(
            mirror_log.lock().unwrap().is_empty(),
            "官方说没有，不去问镜像"
        );
    }

    #[tokio::test]
    async fn stale_mirror_data_checks_official_first_and_is_the_last_resort() {
        let stale = r#"{"id":"a","checked_at":"2020-01-01T00:00:00Z"}"#;
        let (route, base, official_log, _) = setup(
            HashMap::from([("/v2/a", reply(503, "{}"))]),
            HashMap::from([("/mirror/v2/a", reply(200, stale))]),
        )
        .await;
        route.set_policy(DownloadSource::Mirror);
        let got = route
            .fetch(
                &client(),
                Method::GET,
                &format!("{base}/v2/a"),
                None,
                None,
                true,
            )
            .await
            .unwrap();
        assert_eq!(official_log.lock().unwrap().len(), 1, "太旧就先问官方");
        assert!(got.from_mirror, "官方也不行，只好用旧的");
        assert_eq!(got.bytes, stale.as_bytes());
    }

    #[tokio::test]
    async fn mirror_miss_falls_back_to_official() {
        let (route, base, _, _) = setup(
            HashMap::from([("/v2/new", reply(200, r#"{"from":"official"}"#))]),
            HashMap::new(),
        )
        .await;
        route.set_policy(DownloadSource::Mirror);
        let got = route
            .fetch(
                &client(),
                Method::GET,
                &format!("{base}/v2/new"),
                None,
                None,
                true,
            )
            .await
            .unwrap();
        assert!(!got.from_mirror, "MCIM 还没缓存就问官方");
    }

    #[tokio::test]
    async fn official_only_never_touches_the_mirror() {
        let (route, base, _, mirror_log) = setup(
            HashMap::from([("/v2/a", reply(503, "{}"))]),
            HashMap::from([("/mirror/v2/a", reply(200, "{}"))]),
        )
        .await;
        route.set_policy(DownloadSource::OfficialOnly);
        let error = route
            .fetch(
                &client(),
                Method::GET,
                &format!("{base}/v2/a"),
                None,
                None,
                true,
            )
            .await
            .unwrap_err();
        assert!(matches!(error, FetchError::Unavailable { .. }));
        assert!(mirror_log.lock().unwrap().is_empty());
    }

    /// CurseForge 的 Key 只发给官方；Key 被拒时镜像兜底，镜像也不行就报「拒绝」
    #[tokio::test]
    async fn the_key_only_goes_to_the_official_api() {
        let (route, base, official_log, mirror_log) = setup(
            HashMap::from([("/v1/mods/1", reply(403, "{}"))]),
            HashMap::from([("/mirror/v1/mods/1", reply(200, r#"{"data":{}}"#))]),
        )
        .await;
        let got = route
            .fetch(
                &client(),
                Method::POST,
                &format!("{base}/v1/mods/1"),
                Some(br#"{"modIds":[1]}"#),
                Some(("x-api-key", "secret")),
                true,
            )
            .await
            .unwrap();
        assert!(got.from_mirror);
        assert!(official_log.lock().unwrap()[0].1, "官方带 Key");
        assert!(!mirror_log.lock().unwrap()[0].1, "镜像不带 Key");

        let (route, base, _, _) =
            setup(HashMap::from([("/v1/x", reply(401, "{}"))]), HashMap::new()).await;
        let error = route
            .fetch(
                &client(),
                Method::GET,
                &format!("{base}/v1/x"),
                None,
                Some(("x-api-key", "k")),
                true,
            )
            .await
            .unwrap_err();
        assert!(matches!(error, FetchError::Denied { status: 401, .. }));
    }
}
