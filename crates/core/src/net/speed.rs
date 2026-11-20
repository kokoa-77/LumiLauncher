//! 测速选源（D18）：下载源设成「自动」时，先测一下官方和 BMCLAPI 谁快，先用快的，另一个照旧当后备。
//!
//! - 测的是同一个文件（Minecraft 版本清单，约 280 KB）从两边完整下载要多久，同时发，先下完的赢
//! - 结果记 30 分钟；两边都失败时按官方优先，2 分钟后再测
//! - 同一时间只测一次：好几个任务一起开始时，后来的等前面那次的结果
//! - 启动游戏前的检查不等测速（[`SourcePicker::cached`]）：测过用测的，没测过先官方，进游戏不多等
//! - 设置里的「测速」按钮（[`SourcePicker::measure`]）：两边都等下完，显示各自用了多久
//!
//! 模组没有镜像可测：MCIM 的文件地址实测（2026-11-15）只是 302 跳回 Modrinth 官方 CDN。

use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

use reqwest::Client;
use serde::Serialize;

use super::to_mirror;
use crate::config::DownloadSource;
use crate::now_millis;

const PROBE_URL: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
/// 一边最多等多久
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);
const KEEP: Duration = Duration::from_secs(30 * 60);
const KEEP_AFTER_FAILURE: Duration = Duration::from_secs(120);

/// 一次测速的结果
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct SpeedTest {
    /// 官方下完用了多少毫秒；失败、超时或者没等它（另一边先下完了）时为空
    pub official_ms: Option<f64>,
    /// BMCLAPI 用了多少毫秒
    pub mirror_ms: Option<f64>,
    /// 自动模式接下来先用哪个（`official` 或 `mirror`）
    pub pick: DownloadSource,
    /// 什么时候测的（毫秒级 Unix 时间戳）
    pub at: f64,
}

impl SpeedTest {
    fn from_times(official_ms: Option<f64>, mirror_ms: Option<f64>) -> Self {
        let pick = match (official_ms, mirror_ms) {
            (Some(official), Some(mirror)) if mirror < official => DownloadSource::Mirror,
            (None, Some(_)) => DownloadSource::Mirror,
            _ => DownloadSource::Official,
        };
        Self {
            official_ms,
            mirror_ms,
            pick,
            at: now_millis() as f64,
        }
    }

    fn failed(&self) -> bool {
        self.official_ms.is_none() && self.mirror_ms.is_none()
    }
}

pub struct SourcePicker {
    client: RwLock<Client>,
    official: String,
    mirror: String,
    last: Mutex<Option<(Instant, SpeedTest)>>,
    probing: tokio::sync::Mutex<()>,
}

impl SourcePicker {
    pub fn new(client: Client) -> Self {
        let mirror = to_mirror(PROBE_URL).unwrap_or_else(|| PROBE_URL.to_owned());
        Self::with_urls(client, PROBE_URL.to_owned(), mirror)
    }

    fn with_urls(client: Client, official: String, mirror: String) -> Self {
        Self {
            client: RwLock::new(client),
            official,
            mirror,
            last: Mutex::new(None),
            probing: tokio::sync::Mutex::new(()),
        }
    }

    /// 切换代理后热替换 HTTP 客户端，并清空过期测速缓存（D26a）。
    pub fn set_client(&self, client: Client) {
        *self.client.write().unwrap_or_else(|e| e.into_inner()) = client;
        *self.lock() = None;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<(Instant, SpeedTest)>> {
        self.last.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 还没过期的测速结果
    fn fresh(&self) -> Option<SpeedTest> {
        let last = self.lock();
        let (at, test) = (*last)?;
        let keep = if test.failed() {
            KEEP_AFTER_FAILURE
        } else {
            KEEP
        };
        (at.elapsed() < keep).then_some(test)
    }

    fn remember(&self, test: SpeedTest) {
        tracing::info!(
            official_ms = ?test.official_ms,
            mirror_ms = ?test.mirror_ms,
            pick = ?test.pick,
            "测速选源"
        );
        *self.lock() = Some((Instant::now(), test));
    }

    /// 最近一次测速（给设置页显示；过期的也给）
    pub fn last(&self) -> Option<SpeedTest> {
        self.lock().map(|(_, test)| test)
    }

    /// 不等测速：「自动」测过用测的结果，没测过先官方
    pub fn cached(&self, setting: DownloadSource) -> DownloadSource {
        match setting {
            DownloadSource::Auto => self
                .fresh()
                .map_or(DownloadSource::Official, |test| test.pick),
            other => other,
        }
    }

    /// 「自动」换成官方优先或镜像优先（要测时先测，两边同时发，先下完的赢）；别的设置原样返回
    pub async fn resolve(&self, setting: DownloadSource) -> DownloadSource {
        if setting != DownloadSource::Auto {
            return setting;
        }
        if let Some(test) = self.fresh() {
            return test.pick;
        }
        let _probing = self.probing.lock().await;
        // 等锁的时候前面那次可能已经测完了
        if let Some(test) = self.fresh() {
            return test.pick;
        }
        let test = self.race().await;
        self.remember(test);
        test.pick
    }

    /// 两边都测完（设置里的「测速」按钮）
    pub async fn measure(&self) -> SpeedTest {
        let _probing = self.probing.lock().await;
        let (official, mirror) = tokio::join!(self.probe(&self.official), self.probe(&self.mirror));
        let test = SpeedTest::from_times(official, mirror);
        self.remember(test);
        test
    }

    /// 同时发，先下完的赢；先回来的失败了就等另一边
    async fn race(&self) -> SpeedTest {
        let official = self.probe(&self.official);
        let mirror = self.probe(&self.mirror);
        tokio::pin!(official, mirror);
        tokio::select! {
            ms = &mut official => match ms {
                Some(_) => SpeedTest::from_times(ms, None),
                None => SpeedTest::from_times(None, mirror.await),
            },
            ms = &mut mirror => match ms {
                Some(_) => SpeedTest::from_times(None, ms),
                None => SpeedTest::from_times(official.await, None),
            },
        }
    }

    /// 完整下载一次，返回用了多少毫秒；失败或超时为空
    async fn probe(&self, url: &str) -> Option<f64> {
        let start = Instant::now();
        let client = self
            .client
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let fetch = async {
            client
                .get(url)
                .send()
                .await?
                .error_for_status()?
                .bytes()
                .await
        };
        match tokio::time::timeout(PROBE_TIMEOUT, fetch).await {
            Ok(Ok(_)) => Some(start.elapsed().as_secs_f64() * 1000.0),
            Ok(Err(error)) => {
                tracing::debug!(url, %error, "测速失败");
                None
            }
            Err(_) => {
                tracing::debug!(url, "测速超时");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// 本机的小服务器：每个请求等 `delay` 再回一个小文件，返回它的地址
    async fn slow_server(delay: Duration) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let mut buffer = [0u8; 1024];
                    let _ = socket.read(&mut buffer).await;
                    tokio::time::sleep(delay).await;
                    let _ = socket
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                        )
                        .await;
                });
            }
        });
        format!("http://{address}/manifest.json")
    }

    fn picker(official: String, mirror: String) -> SourcePicker {
        SourcePicker::with_urls(Client::new(), official, mirror)
    }

    /// 没人听的端口：马上连不上
    const DEAD: &str = "http://127.0.0.1:9/manifest.json";

    #[test]
    fn pick_follows_the_times() {
        assert_eq!(
            SpeedTest::from_times(Some(300.0), Some(100.0)).pick,
            DownloadSource::Mirror
        );
        assert_eq!(
            SpeedTest::from_times(Some(100.0), Some(300.0)).pick,
            DownloadSource::Official
        );
        assert_eq!(
            SpeedTest::from_times(None, Some(300.0)).pick,
            DownloadSource::Mirror
        );
        assert_eq!(
            SpeedTest::from_times(None, None).pick,
            DownloadSource::Official,
            "都不通时按官方优先（失败还会换镜像）"
        );
    }

    #[tokio::test]
    async fn auto_picks_whoever_finishes_first_and_remembers_it() {
        let slow = slow_server(Duration::from_millis(600)).await;
        let fast = slow_server(Duration::ZERO).await;
        let picker = picker(slow, fast);
        assert_eq!(
            picker.cached(DownloadSource::Auto),
            DownloadSource::Official
        );
        assert_eq!(
            picker.resolve(DownloadSource::Auto).await,
            DownloadSource::Mirror
        );
        assert_eq!(
            picker.cached(DownloadSource::Auto),
            DownloadSource::Mirror,
            "测过以后不等也知道"
        );
        assert_eq!(
            picker.resolve(DownloadSource::OfficialOnly).await,
            DownloadSource::OfficialOnly,
            "不是自动就不测"
        );
    }

    #[tokio::test]
    async fn a_failing_side_does_not_win() {
        let ok = slow_server(Duration::from_millis(100)).await;
        let picker = picker(DEAD.to_owned(), ok);
        assert_eq!(
            picker.resolve(DownloadSource::Auto).await,
            DownloadSource::Mirror,
            "官方先回来但失败了，等镜像"
        );
    }

    #[tokio::test]
    async fn measuring_waits_for_both() {
        let official = slow_server(Duration::ZERO).await;
        let mirror = slow_server(Duration::from_millis(200)).await;
        let picker = picker(official, mirror);
        let test = picker.measure().await;
        assert!(test.official_ms.is_some() && test.mirror_ms.is_some());
        assert_eq!(test.pick, DownloadSource::Official);
        assert_eq!(picker.last(), Some(test));
    }

    #[tokio::test]
    async fn both_down_falls_back_to_official_for_a_short_while() {
        let picker = picker(DEAD.to_owned(), DEAD.to_owned());
        assert_eq!(
            picker.resolve(DownloadSource::Auto).await,
            DownloadSource::Official
        );
        assert!(picker.last().is_some_and(|t| t.failed()));
        assert!(picker.fresh().is_some(), "两分钟内不再测");
    }
}
