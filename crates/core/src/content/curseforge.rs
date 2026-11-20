//! CurseForge API（v1）：批量查文件、批量查项目、按指纹识别、搜索。
//!
//! 文档：<https://docs.curseforge.com/rest-api/>。每个请求都要带启动器自己的 API Key（请求头 `x-api-key`）；
//! 7 月 16 日起 CDN 下载也要带。**Key 只发给 CurseForge 自己的域名**（[`KEY_HOSTS`]）。
//! 官方不行或没有 Key 时按「CurseForge」那行下载源设置改问 MCIM 镜像（D18b，见 [`super::mirror`]），镜像不带 Key。
//!
//! 作者禁止第三方下载的文件，接口返回的 `downloadUrl` 是空的：**不自己拼 CDN 地址**，
//! 交给补件流程（[`super::claim`]）让用户在浏览器里下载。

use std::collections::{HashMap, HashSet, VecDeque};

use reqwest::Client;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use super::detail::{
    BodyFormat, ContentVersion, GalleryImage, LinkKind, ProjectDetail, ProjectMember, ReleaseType,
    https_only, links_of, release_versions,
};
use super::modrinth::{self, PlanItem, PlanReason, ProjectHit, SearchPage};
use super::{ContentError, ContentKind, ContentSource, Delivery, SearchSort};
use crate::net::{HostHeader, HttpError, url_encode, url_host};
use crate::redact::Secret;

const API: &str = "https://api.curseforge.com/v1";
/// Minecraft 在 CurseForge 上的游戏编号
pub const GAME_ID: u32 = 432;
/// 运行时读 Key 的环境变量（开发时用；正式版在打包时编进程序）
pub const KEY_ENV: &str = "MCL_CURSEFORGE_KEY";
pub const KEY_HEADER: &str = "x-api-key";
/// 会带上 Key 的域名：接口和下载 CDN
pub const KEY_HOSTS: &[&str] = &[
    "api.curseforge.com",
    "edge.forgecdn.net",
    "mediafilez.forgecdn.net",
];
/// 文件下载地址只接受这些域名（都是 https）
pub const DOWNLOAD_HOSTS: &[&str] = &["edge.forgecdn.net", "mediafilez.forgecdn.net"];
/// 精确下载页只接受官网
const WEBSITE: &str = "https://www.curseforge.com/";
/// 批量接口一次最多送多少个编号（官方没写上限，整合包 500 个实测可以）
const BATCH: usize = 500;

/// 项目类型（`classId`）
pub mod class {
    pub const MOD: u32 = 6;
    pub const RESOURCE_PACK: u32 = 12;
    pub const WORLD: u32 = 17;
    pub const MODPACK: u32 = 4471;
    pub const SHADER: u32 = 6552;
    pub const DATA_PACK: u32 = 6945;
}

/// 加载器（`modLoaderType`）
pub const LOADER_NEOFORGE: u32 = 6;
/// CurseForge 的加载器编号：Forge 1、Fabric 4、Quilt 5（挂上的别家版本按它筛文件，D19）
pub const LOADER_FORGE: u32 = 1;
pub const LOADER_FABRIC: u32 = 4;
pub const LOADER_QUILT: u32 = 5;

/// 通用加载器短名（`neoforge` / `fabric` / `forge` / `quilt`）→ CurseForge 加载器编号。
pub fn loader_id(slug: &str) -> Option<u32> {
    match slug.to_ascii_lowercase().as_str() {
        "forge" => Some(LOADER_FORGE),
        "fabric" => Some(LOADER_FABRIC),
        "quilt" => Some(LOADER_QUILT),
        "neoforge" => Some(LOADER_NEOFORGE),
        _ => None,
    }
}

/// 依赖关系（`relationType`）：3 = 必需
const RELATION_REQUIRED: u32 = 3;
/// Iris 在 CurseForge 上的项目编号：装光影时自动补上
pub const IRIS_PROJECT: u32 = 455_508;
/// 前置最多解析几层，防止循环依赖或异常数据拖垮
const MAX_DEPENDENCY_DEPTH: usize = 6;

/// 各类内容在 CurseForge 上的项目类型
pub fn class_for(kind: ContentKind) -> u32 {
    match kind {
        ContentKind::Mod => class::MOD,
        ContentKind::ResourcePack => class::RESOURCE_PACK,
        ContentKind::Shader => class::SHADER,
    }
}

/// 项目类型 → 内容种类（整合包、存档等不是实例内容，返回 `None`）
pub fn kind_of_class(class_id: Option<u32>) -> Option<ContentKind> {
    match class_id? {
        class::MOD => Some(ContentKind::Mod),
        class::RESOURCE_PACK => Some(ContentKind::ResourcePack),
        class::SHADER => Some(ContentKind::Shader),
        _ => None,
    }
}

/// 启动器的 CurseForge Key：先看环境变量（开发用），再看打包时编进来的。
pub fn api_key() -> Option<Secret<String>> {
    std::env::var(KEY_ENV)
        .ok()
        .or_else(|| option_env!("MCL_CURSEFORGE_KEY").map(str::to_owned))
        .map(|key| key.trim().to_owned())
        .filter(|key| !key.is_empty())
        .map(Secret::new)
}

/// CurseForge 客户端（克隆是廉价的）。
#[derive(Clone)]
pub struct CurseForge {
    client: std::sync::Arc<std::sync::RwLock<Client>>,
    key: Option<Secret<String>>,
}

#[derive(Deserialize)]
struct Data<T> {
    data: T,
}

impl CurseForge {
    pub fn new(client: Client, key: Option<Secret<String>>) -> Self {
        Self {
            client: std::sync::Arc::new(std::sync::RwLock::new(client)),
            key,
        }
    }

    /// 用 [`api_key`] 找到的 Key
    pub fn from_env(client: Client) -> Self {
        Self::new(client, api_key())
    }

    pub fn has_key(&self) -> bool {
        self.key.is_some()
    }

    /// 能不能问 CurseForge：有 Key，或者没 Key 但允许用 MCIM 镜像（D18b）
    pub fn usable(&self) -> bool {
        self.has_key()
            || super::mirror::curseforge().policy() != crate::config::DownloadSource::OfficialOnly
    }

    /// 底下用的 HTTP 客户端（同一个连接池，问 Modrinth 时也用它）
    pub fn http(&self) -> Client {
        self.client
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// 切换代理后热替换 HTTP 客户端（D26a）。
    pub fn set_client(&self, client: Client) {
        *self.client.write().unwrap_or_else(|e| e.into_inner()) = client;
    }

    /// 给下载器用：只对 CurseForge 的域名带 Key
    pub fn download_header(&self) -> Option<HostHeader> {
        self.key.as_ref().map(|key| HostHeader {
            hosts: KEY_HOSTS,
            name: KEY_HEADER,
            value: key.clone(),
        })
    }

    /// 问一次接口：有 Key 先按设置问官方（带 Key），官方不行或没 Key 时问 MCIM（不带 Key）
    async fn send<T: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        url: &str,
        body: Option<&[u8]>,
        what: &str,
    ) -> Result<T, ContentError> {
        if !self.usable() {
            return Err(ContentError::NoCurseForgeKey);
        }
        let key = self
            .key
            .as_ref()
            .map(|key| (KEY_HEADER, key.expose().as_str()));
        let client = self.http();
        let fetched = super::mirror::curseforge()
            .fetch(&client, method, url, body, key, self.has_key())
            .await
            .map_err(|error| match error {
                super::mirror::FetchError::Denied { status, .. } => {
                    tracing::warn!(what, status, "CurseForge 拒绝了请求");
                    ContentError::CurseForgeDenied {
                        what: what.to_owned(),
                    }
                }
                super::mirror::FetchError::Unavailable { reason, .. } => {
                    ContentError::Http(HttpError::AllSourcesFailed {
                        last: format!("{url}：{reason}"),
                    })
                }
                rejected @ super::mirror::FetchError::Rejected { .. } => {
                    ContentError::CurseForge(format!("{what}失败：{rejected}"))
                }
            })?;
        serde_json::from_slice(&fetched.bytes)
            .map_err(|e| ContentError::CurseForge(format!("{what}的结果格式不对：{e}")))
    }

    async fn get<T: DeserializeOwned>(&self, path: &str, what: &str) -> Result<T, ContentError> {
        let url = format!("{API}{path}");
        self.send(reqwest::Method::GET, &url, None, what).await
    }

    async fn post<T: DeserializeOwned>(
        &self,
        path: &str,
        body: &serde_json::Value,
        what: &str,
    ) -> Result<T, ContentError> {
        let url = format!("{API}{path}");
        let body = serde_json::to_vec(body).map_err(|e| ContentError::CurseForge(e.to_string()))?;
        self.send(reqwest::Method::POST, &url, Some(&body), what)
            .await
    }

    /// 按文件编号批量查（顺序不保证；查不到的文件不在结果里）。
    pub async fn files(&self, ids: &[u32]) -> Result<Vec<File>, ContentError> {
        let mut out = Vec::with_capacity(ids.len());
        for chunk in ids.chunks(BATCH) {
            let page: Data<Vec<File>> = self
                .post(
                    "/mods/files",
                    &serde_json::json!({ "fileIds": chunk }),
                    "查询文件信息",
                )
                .await?;
            out.extend(page.data);
        }
        Ok(out)
    }

    /// 按项目编号批量查。
    pub async fn mods(&self, ids: &[u32]) -> Result<HashMap<u32, Mod>, ContentError> {
        let mut out = HashMap::with_capacity(ids.len());
        for chunk in ids.chunks(BATCH) {
            let page: Data<Vec<Mod>> = self
                .post(
                    "/mods",
                    &serde_json::json!({ "modIds": chunk }),
                    "查询项目信息",
                )
                .await?;
            out.extend(page.data.into_iter().map(|m| (m.id, m)));
        }
        Ok(out)
    }

    /// 按指纹识别文件：返回 指纹 → 文件（只算完全匹配的）。
    pub async fn identify(&self, fingerprints: &[u32]) -> Result<HashMap<u32, File>, ContentError> {
        let mut out = HashMap::new();
        for chunk in fingerprints.chunks(BATCH) {
            let page: Data<FingerprintMatches> = self
                .post(
                    &format!("/fingerprints/{GAME_ID}"),
                    &serde_json::json!({ "fingerprints": chunk }),
                    "按指纹识别文件",
                )
                .await?;
            for found in page.data.exact_matches {
                out.insert(found.file.file_fingerprint, found.file);
            }
        }
        Ok(out)
    }

    /// 搜索项目。排序换成 CurseForge 的 `sortField`（见 [`SearchSort::curseforge_field`]）。
    pub async fn search(&self, query: &SearchQuery<'_>) -> Result<SearchPage, ContentError> {
        let (mods, total, offset) = self.search_mods(query).await?;
        Ok(SearchPage {
            total,
            offset,
            hits: mods.into_iter().map(Mod::into_hit).collect(),
        })
    }

    /// 搜索项目，返回平台的原样数据（带每个加载器最新的文件）、总数、从第几条开始。
    pub async fn search_mods(
        &self,
        query: &SearchQuery<'_>,
    ) -> Result<(Vec<Mod>, u32, u32), ContentError> {
        let limit = query.limit.clamp(1, 50);
        // 接口规定 index + pageSize 不能超过 10000
        let offset = query.offset.min(10_000 - limit);
        let text = query.text.trim();
        let mut url = format!(
            "/mods/search?gameId={GAME_ID}&classId={}&index={offset}&pageSize={limit}&sortOrder=desc&sortField={}",
            query.class_id,
            query.sort.curseforge_field(text)
        );
        if !text.is_empty() {
            url.push_str(&format!("&searchFilter={}", url_encode(text)));
        }
        // 只给一个分类：`categoryIds` 给好几个时是「同时属于」，2026-11-12 实测 [412,413] 只剩 380 个
        if let Some(category) = query.category {
            url.push_str(&format!("&categoryId={category}"));
        }
        if let Some(version) = query.game_version {
            url.push_str(&format!("&gameVersion={}", url_encode(version)));
        }
        if let Some(loader) = query.loader {
            url.push_str(&format!("&modLoaderType={loader}"));
        }
        let page: SearchResponse = self.get(&url, "搜索 CurseForge").await?;
        Ok((
            page.data,
            page.pagination.total_count.min(10_000),
            page.pagination.index,
        ))
    }

    /// 一个项目的详情：项目信息和介绍（HTML）同时请求。
    pub async fn project_detail(&self, mod_id: u32) -> Result<ProjectDetail, ContentError> {
        let project_path = format!("/mods/{mod_id}");
        let body_path = format!("/mods/{mod_id}/description");
        let (project, body) = tokio::join!(
            self.get::<Data<Mod>>(&project_path, "查询项目详情"),
            self.get::<Data<String>>(&body_path, "读取项目介绍"),
        );
        let project = project?.data;
        let body = match body {
            Ok(body) => body.data,
            Err(error) => {
                tracing::warn!(%error, mod_id, "读取 CurseForge 项目介绍失败，详情页只显示简介");
                String::new()
            }
        };
        Ok(project.into_detail(body))
    }

    /// 一个项目的文件（可按游戏版本、加载器筛），新的在前。
    pub async fn mod_files(
        &self,
        mod_id: u32,
        game_version: Option<&str>,
        loader: Option<u32>,
    ) -> Result<Vec<File>, ContentError> {
        let mut url = format!("/mods/{mod_id}/files?pageSize=50");
        if let Some(version) = game_version {
            url.push_str(&format!("&gameVersion={}", url_encode(version)));
        }
        if let Some(loader) = loader {
            url.push_str(&format!("&modLoaderType={loader}"));
        }
        let page: Data<Vec<File>> = self.get(&url, "查询项目的文件").await?;
        let mut files = page.data;
        // 文件编号越大越新；不依赖接口的排序
        files.sort_by_key(|f| std::cmp::Reverse(f.id));
        Ok(files)
    }

    /// 列出项目的版本列表（项目「版本」页签与实例「切换版本」共用，D25e）。
    pub async fn project_versions(
        &self,
        mod_id: u32,
        game_version: Option<&str>,
        loader: Option<u32>,
    ) -> Result<Vec<ContentVersion>, ContentError> {
        let files = self
            .mod_files(
                mod_id,
                game_version.filter(|s| !s.trim().is_empty()),
                loader,
            )
            .await?;
        let has_manual = files
            .iter()
            .any(|f| !f.is_server_pack && f.usable_download_url().is_none());
        let website = if has_manual {
            self.mods(&[mod_id])
                .await
                .ok()
                .and_then(|mods| mods.get(&mod_id).and_then(|m| m.links.website_url.clone()))
        } else {
            None
        };
        Ok(files
            .into_iter()
            .filter(|f| !f.is_server_pack)
            .map(|f| {
                let delivery = if f.usable_download_url().is_some() {
                    Delivery::Direct
                } else {
                    Delivery::Manual
                };
                let page_url = (delivery == Delivery::Manual)
                    .then(|| download_page(website.as_deref(), f.id))
                    .flatten();
                let mut loaders = Vec::new();
                let mut game_versions = Vec::new();
                for tag in &f.game_versions {
                    match tag.to_ascii_lowercase().as_str() {
                        "neoforge" | "forge" | "fabric" | "quilt" => {
                            let lower = tag.to_ascii_lowercase();
                            if !loaders.contains(&lower) {
                                loaders.push(lower);
                            }
                        }
                        _ if super::detail::version_key(tag).is_some()
                            && !game_versions.contains(tag) =>
                        {
                            game_versions.push(tag.clone());
                        }
                        _ => {}
                    }
                }
                let release_type = match f.release_type {
                    2 => ReleaseType::Beta,
                    3 => ReleaseType::Alpha,
                    _ => ReleaseType::Release,
                };
                let name = if f.display_name.trim().is_empty() {
                    f.file_name.clone()
                } else {
                    f.display_name.clone()
                };
                ContentVersion {
                    id: f.id.to_string(),
                    project_id: f.mod_id.to_string(),
                    source: ContentSource::CurseForge,
                    version_number: name.clone(),
                    name,
                    release_type,
                    game_versions,
                    loaders,
                    published: f.file_date,
                    downloads: f.download_count,
                    changelog: None,
                    changelog_format: BodyFormat::Html,
                    file_name: f.file_name,
                    file_size: f.file_length as f64,
                    delivery,
                    page_url,
                }
            })
            .collect())
    }

    /// 懒加载 CurseForge 单个文件的更新日志（HTML 原文，前端清洗后展示，D25e）。
    pub async fn file_changelog(&self, mod_id: u32, file_id: u32) -> Result<String, ContentError> {
        let page: Data<String> = self
            .get(
                &format!("/mods/{mod_id}/files/{file_id}/changelog"),
                "读取版本更新日志",
            )
            .await?;
        Ok(page.data)
    }

    /// 批量查一组 CurseForge 项目在指定游戏版本与加载器下的最新兼容文件（D25e）。
    /// 先用单次 `POST /mods` 读 `latestFilesIndexes` 并批量 `POST /mods/files` 拉全量文件信息；
    /// 索引里没命中（如部分光影/材质包未按加载器建索引）再按需回退单项目查询。
    /// 返回 `mod_id -> (最新 File, 项目官网地址)`。
    pub async fn latest_compatible_files(
        &self,
        mod_ids: &[u32],
        game_version: &str,
        loader: Option<u32>,
        kind: ContentKind,
    ) -> Result<HashMap<u32, (File, Option<String>)>, ContentError> {
        if mod_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let mods = self.mods(mod_ids).await?;
        let wanted_loader = if kind == ContentKind::Mod {
            loader
        } else {
            None
        };
        let mut index_file_ids: Vec<u32> = Vec::new();
        let mut fallback_mods: Vec<u32> = Vec::new();
        for &mod_id in mod_ids {
            let Some(m) = mods.get(&mod_id) else {
                continue;
            };
            if let Some(file_id) =
                pick_latest_index(&m.latest_files_indexes, game_version, wanted_loader)
            {
                index_file_ids.push(file_id);
            } else {
                fallback_mods.push(mod_id);
            }
        }
        let mut out: HashMap<u32, (File, Option<String>)> = HashMap::new();
        if !index_file_ids.is_empty() {
            for file in self.files(&index_file_ids).await? {
                if file.is_server_pack {
                    continue;
                }
                let website = mods
                    .get(&file.mod_id)
                    .and_then(|m| m.links.website_url.clone());
                out.insert(file.mod_id, (file, website));
            }
        }
        // 索引未命中（常见于无加载器标签的资源包/光影，或旧缓存索引）：按需兜底查一次文件列表
        for mod_id in fallback_mods {
            let mut files = match self
                .mod_files(mod_id, Some(game_version), wanted_loader)
                .await
            {
                Ok(files) => files,
                Err(error) => {
                    tracing::warn!(%error, mod_id, "回退查询 CurseForge 文件列表失败");
                    continue;
                }
            };
            if files.is_empty() && kind != ContentKind::Mod {
                files = self.mod_files(mod_id, None, None).await.unwrap_or_default();
            }
            if let Some(file) = best_file(files) {
                let website = mods.get(&mod_id).and_then(|m| m.links.website_url.clone());
                out.insert(mod_id, (file, website));
            }
        }
        Ok(out)
    }

    /// 从用户点的项目出发，解析出要装的全部文件（含必需前置；装光影时补上 Iris）。
    /// 作者禁止第三方下载的文件，查 Modrinth 上有没有一模一样的：有就标成从 Modrinth 下，没有就标成要用户下载。
    pub async fn resolve(
        &self,
        client: &Client,
        target: &Target<'_>,
        mod_id: u32,
        kind: ContentKind,
    ) -> Result<Vec<PlanItem>, ContentError> {
        self.resolve_with_file(client, target, mod_id, None, kind)
            .await
    }

    /// 解析安装 CurseForge 指定文件版本（含必需前置，D25e）。
    pub async fn resolve_version(
        &self,
        client: &Client,
        target: &Target<'_>,
        mod_id: u32,
        file_id: u32,
        kind: ContentKind,
    ) -> Result<Vec<PlanItem>, ContentError> {
        self.resolve_with_file(client, target, mod_id, Some(file_id), kind)
            .await
    }

    async fn resolve_with_file(
        &self,
        client: &Client,
        target: &Target<'_>,
        mod_id: u32,
        requested_file_id: Option<u32>,
        kind: ContentKind,
    ) -> Result<Vec<PlanItem>, ContentError> {
        if kind == ContentKind::Mod && target.loader.is_none() {
            return Err(ContentError::NeedsLoader);
        }
        let mut queue: VecDeque<(u32, Option<u32>, PlanReason, usize)> =
            VecDeque::from([(mod_id, requested_file_id, PlanReason::Requested, 0)]);
        if kind == ContentKind::Shader && !target.installed.contains(&IRIS_PROJECT) {
            if target.loader.is_none() {
                return Err(ContentError::NeedsLoader);
            }
            queue.push_back((IRIS_PROJECT, None, PlanReason::ShaderLoader, 0));
        }
        let mut chosen: Vec<(File, PlanReason, ContentKind)> = Vec::new();
        let mut seen = HashSet::new();
        while let Some((project, file_id, reason, depth)) = queue.pop_front() {
            if depth > MAX_DEPENDENCY_DEPTH || !seen.insert(project) {
                continue;
            }
            if reason != PlanReason::Requested && target.installed.contains(&project) {
                continue;
            }
            // 前置和 Iris 都是模组；只有用户点的那个按它自己的类型选版本
            let item_kind = if reason == PlanReason::Requested {
                kind
            } else {
                ContentKind::Mod
            };
            let file = match file_id {
                Some(fid) => self
                    .files(&[fid])
                    .await?
                    .into_iter()
                    .find(|f| f.id == fid && f.mod_id == project)
                    .ok_or_else(|| {
                        ContentError::CurseForge(format!("找不到项目 {project} 的文件 {fid}"))
                    })?,
                None => self.pick_file(project, target, item_kind).await?,
            };
            for dependency in file.required_dependencies() {
                queue.push_back((dependency, None, PlanReason::Dependency, depth + 1));
            }
            chosen.push((file, reason, item_kind));
        }

        let ids: Vec<u32> = chosen.iter().map(|(f, ..)| f.mod_id).collect();
        let mods = self.mods(&ids).await?;
        let blocked: Vec<String> = chosen
            .iter()
            .filter(|(f, ..)| f.usable_download_url().is_none())
            .filter_map(|(f, ..)| f.sha1())
            .collect();
        let on_modrinth = if blocked.is_empty() {
            HashMap::new()
        } else {
            modrinth::files_by_sha1(client, &blocked)
                .await
                .unwrap_or_else(|error| {
                    tracing::warn!(%error, "问 Modrinth 有没有同样的文件失败");
                    HashMap::new()
                })
        };
        Ok(chosen
            .into_iter()
            .map(|(file, reason, item_kind)| {
                let project = mods.get(&file.mod_id);
                let delivery = if file.usable_download_url().is_some() {
                    Delivery::Direct
                } else if file.sha1().is_some_and(|h| on_modrinth.contains_key(&h)) {
                    Delivery::Modrinth
                } else {
                    Delivery::Manual
                };
                PlanItem {
                    project_id: file.mod_id.to_string(),
                    version_id: file.id.to_string(),
                    title: project.map_or_else(|| file.display_name.clone(), |m| m.name.clone()),
                    version_number: file.display_name.clone(),
                    file_name: file.file_name.clone(),
                    size: file.file_length as f64,
                    kind: project
                        .and_then(|m| kind_of_class(m.class_id))
                        .unwrap_or(item_kind),
                    icon_url: project.and_then(Mod::icon_url),
                    reason,
                    source: ContentSource::CurseForge,
                    delivery,
                    page_url: (delivery == Delivery::Manual)
                        .then(|| {
                            download_page(
                                project.and_then(|m| m.links.website_url.as_deref()),
                                file.id,
                            )
                        })
                        .flatten(),
                }
            })
            .collect())
    }

    /// 给一个项目挑最合适的文件：游戏版本和加载器都对得上，优先正式版，其次最新。
    /// 材质包、光影很多不标具体游戏版本，按版本找不到时就取最新的。
    async fn pick_file(
        &self,
        mod_id: u32,
        target: &Target<'_>,
        kind: ContentKind,
    ) -> Result<File, ContentError> {
        let loader = if kind == ContentKind::Mod {
            target.loader
        } else {
            None
        };
        let mut files = self
            .mod_files(mod_id, Some(target.game_version), loader)
            .await?;
        if files.is_empty() && kind != ContentKind::Mod {
            files = self.mod_files(mod_id, None, None).await?;
        }
        if let Some(file) = best_file(files) {
            return Ok(file);
        }
        let name = match self.mods(&[mod_id]).await {
            Ok(mods) => mods.get(&mod_id).map(|m| m.name.clone()),
            Err(_) => None,
        };
        Err(ContentError::NoCompatibleVersion {
            project: name.unwrap_or_else(|| format!("CurseForge 项目 {mod_id}")),
            game_version: target.game_version.to_owned(),
        })
    }
}

impl CurseForge {
    /// 给一张地图挑文件：先找标了这个游戏版本的；没有就找标的版本不高于实例版本的里最新的
    /// （老地图在新版本打开会自动升级，反过来打不开）。
    pub async fn pick_world_file(
        &self,
        mod_id: u32,
        game_version: &str,
    ) -> Result<File, ContentError> {
        let exact = self.mod_files(mod_id, Some(game_version), None).await?;
        if let Some(file) = best_file(exact) {
            return Ok(file);
        }
        let target = super::detail::version_key(game_version);
        let older: Vec<File> = self
            .mod_files(mod_id, None, None)
            .await?
            .into_iter()
            .filter(|f| {
                f.game_versions
                    .iter()
                    .filter_map(|v| super::detail::version_key(v))
                    .any(|k| target.as_ref().is_some_and(|t| &k <= t))
            })
            .collect();
        best_file(older).ok_or_else(|| ContentError::NoCompatibleVersion {
            project: format!("CurseForge 地图 {mod_id}"),
            game_version: game_version.to_owned(),
        })
    }
}

/// 解析 CurseForge 安装计划需要知道的实例信息。
pub struct Target<'a> {
    pub game_version: &'a str,
    /// 实例的加载器（CurseForge 的编号，例如 [`LOADER_NEOFORGE`]）；原版实例为 `None`
    pub loader: Option<u32>,
    /// 已经装了的 CurseForge 项目（含按指纹认出来的），不会重复安装
    pub installed: &'a HashSet<u32>,
}

/// 文件列表是新的在前：取第一个正式版（服务端包除外），没有正式版就取最新的
fn best_file(files: Vec<File>) -> Option<File> {
    let mut files: Vec<File> = files.into_iter().filter(|f| !f.is_server_pack).collect();
    let index = files.iter().position(File::is_release).unwrap_or(0);
    (index < files.len()).then(|| files.swap_remove(index))
}

/// 搜索条件。
#[derive(Debug, Clone)]
pub struct SearchQuery<'a> {
    pub text: &'a str,
    pub class_id: u32,
    pub game_version: Option<&'a str>,
    pub loader: Option<u32>,
    pub sort: SearchSort,
    /// 只看这一个分类（CurseForge 的分类编号）
    pub category: Option<u32>,
    pub offset: u32,
    pub limit: u32,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    data: Vec<Mod>,
    pagination: Pagination,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pagination {
    index: u32,
    total_count: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FingerprintMatches {
    #[serde(default)]
    exact_matches: Vec<FingerprintMatch>,
}

#[derive(Debug, Deserialize)]
struct FingerprintMatch {
    file: File,
}

/// 一个项目（模组、材质包、整合包……）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mod {
    pub id: u32,
    pub name: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub class_id: Option<u32>,
    #[serde(default)]
    pub links: Links,
    #[serde(default)]
    pub logo: Option<Logo>,
    #[serde(default)]
    pub authors: Vec<Author>,
    #[serde(default)]
    pub download_count: f64,
    /// 作者是否允许第三方启动器下载；`false` 时文件的 `downloadUrl` 为空
    #[serde(default)]
    pub allow_mod_distribution: Option<bool>,
    #[serde(default)]
    pub categories: Vec<Category>,
    #[serde(default)]
    pub screenshots: Vec<Screenshot>,
    #[serde(default)]
    pub date_created: Option<String>,
    #[serde(default)]
    pub date_modified: Option<String>,
    /// 每个游戏版本、加载器最新的那个文件（详情页的「兼容性」从这里拿）
    #[serde(default)]
    pub latest_files_indexes: Vec<FileIndex>,
    /// 每个加载器最新的几个文件（带 SHA-1；生成中文名表时拿它去 Modrinth 对同一个文件）
    #[serde(default)]
    pub latest_files: Vec<File>,
}

/// 从项目的 `latest_files_indexes` 里挑出匹配游戏版本与加载器的最新文件编号（优先正式版，其次编号最大）。
pub(crate) fn pick_latest_index(
    indexes: &[FileIndex],
    game_version: &str,
    loader: Option<u32>,
) -> Option<u32> {
    let mut matches: Vec<&FileIndex> = indexes
        .iter()
        .filter(|idx| idx.file_id > 0 && idx.game_version == game_version)
        .filter(|idx| match loader {
            Some(wanted) => idx.mod_loader == Some(wanted),
            None => true,
        })
        .collect();
    matches.sort_by_key(|idx| (idx.release_type == 1, idx.file_id));
    matches.last().map(|idx| idx.file_id)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileIndex {
    #[serde(default)]
    pub game_version: String,
    #[serde(default)]
    pub file_id: u32,
    /// 1 正式版、2 测试版、3 预览版
    #[serde(default)]
    pub release_type: u32,
    /// 加载器编号：1 Forge、4 Fabric、5 Quilt、6 NeoForge
    #[serde(default)]
    pub mod_loader: Option<u32>,
}

/// CurseForge 的加载器编号 → 名字（和 Modrinth 的写法一致）
fn loader_name(id: u32) -> Option<&'static str> {
    match id {
        1 => Some("forge"),
        4 => Some("fabric"),
        5 => Some("quilt"),
        6 => Some("neoforge"),
        _ => None,
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Links {
    #[serde(default)]
    pub website_url: Option<String>,
    #[serde(default)]
    pub wiki_url: Option<String>,
    #[serde(default)]
    pub issues_url: Option<String>,
    #[serde(default)]
    pub source_url: Option<String>,
}

/// 作者上传的截图
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Screenshot {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub thumbnail_url: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Logo {
    #[serde(default)]
    pub thumbnail_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Author {
    pub name: String,
    /// 作者在 CurseForge 上的主页
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Category {
    pub name: String,
}

impl Mod {
    pub fn icon_url(&self) -> Option<String> {
        self.logo
            .as_ref()
            .and_then(|l| l.thumbnail_url.clone())
            .filter(|u| u.starts_with("https://"))
    }

    /// 截图（只认 https）：(大图, 缩略图, 标题)
    fn gallery(&self) -> Vec<GalleryImage> {
        self.screenshots
            .iter()
            .filter_map(|s| {
                let url = s.url.clone().and_then(https_only)?;
                let thumb_url = s
                    .thumbnail_url
                    .clone()
                    .and_then(https_only)
                    .unwrap_or_else(|| url.clone());
                Some(GalleryImage {
                    url,
                    thumb_url,
                    title: s.title.clone().filter(|t| !t.trim().is_empty()),
                })
            })
            .collect()
    }

    fn into_hit(self) -> ProjectHit {
        let manual_only = self.allow_mod_distribution == Some(false);
        let icon_url = self.icon_url();
        let gallery_url = self.gallery().into_iter().next().map(|g| g.thumb_url);
        let author_url = self
            .authors
            .first()
            .and_then(|a| a.url.clone())
            .filter(|url| url.starts_with(WEBSITE));
        // 只认 CurseForge 官网的地址
        let page_url = self
            .links
            .website_url
            .clone()
            .filter(|url| url.starts_with(WEBSITE));
        ProjectHit {
            project_id: self.id.to_string(),
            slug: String::new(),
            title: self.name,
            description: self.summary,
            author: self
                .authors
                .into_iter()
                .next()
                .map(|a| a.name)
                .unwrap_or_default(),
            icon_url,
            downloads: self.download_count,
            display_categories: self.categories.into_iter().map(|c| c.name).collect(),
            source: ContentSource::CurseForge,
            side: None,
            page_url,
            gallery_url,
            author_url,
            updated: self.date_modified,
            published: self.date_created,
            server: None,
            manual_only,
            title_zh: None,
            twin_id: None,
        }
    }

    fn into_detail(self, body: String) -> ProjectDetail {
        let gallery = self.gallery();
        let links = links_of([
            (LinkKind::Source, self.links.source_url.clone()),
            (LinkKind::Issues, self.links.issues_url.clone()),
            (LinkKind::Wiki, self.links.wiki_url.clone()),
        ]);
        let published = self.date_created.clone();
        let game_versions = release_versions(
            self.latest_files_indexes
                .iter()
                .map(|f| f.game_version.clone()),
        );
        let mut loaders: Vec<String> = Vec::new();
        for name in self
            .latest_files_indexes
            .iter()
            .filter_map(|f| f.mod_loader.and_then(loader_name))
        {
            if !loaders.iter().any(|l| l == name) {
                loaders.push(name.to_owned());
            }
        }
        let members = self
            .authors
            .iter()
            .map(|a| ProjectMember {
                name: a.name.clone(),
                role: None,
                avatar_url: None,
                url: a.url.clone().filter(|url| url.starts_with(WEBSITE)),
            })
            .collect();
        let summary = self.summary.clone();
        let hit = self.into_hit();
        // 介绍读不到时用一句话简介顶上
        let body = if body.trim().is_empty() {
            summary
        } else {
            body
        };
        ProjectDetail {
            hit,
            body,
            body_format: BodyFormat::Html,
            gallery,
            links,
            license: None,
            published,
            followers: None,
            game_versions,
            loaders,
            members,
        }
    }
}

/// 一个文件（项目的某个版本）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct File {
    pub id: u32,
    pub mod_id: u32,
    #[serde(default)]
    pub display_name: String,
    pub file_name: String,
    /// 1 正式版、2 测试版、3 预览版
    #[serde(default)]
    pub release_type: u32,
    #[serde(default)]
    pub file_date: Option<String>,
    #[serde(default)]
    pub download_count: Option<f64>,
    #[serde(default)]
    pub hashes: Vec<FileHash>,
    #[serde(default)]
    pub file_length: u64,
    /// 作者禁止第三方下载时为空
    #[serde(default)]
    pub download_url: Option<String>,
    #[serde(default)]
    pub game_versions: Vec<String>,
    #[serde(default)]
    pub dependencies: Vec<FileDependency>,
    #[serde(default)]
    pub file_fingerprint: u32,
    /// 整合包的服务端包（客户端不装）
    #[serde(default)]
    pub is_server_pack: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileHash {
    pub value: String,
    /// 1 = SHA-1，2 = MD5
    pub algo: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDependency {
    pub mod_id: u32,
    pub relation_type: u32,
}

impl File {
    /// 有效的 SHA-1（小写）；没有或格式不对时为 `None`
    pub fn sha1(&self) -> Option<String> {
        self.hashes
            .iter()
            .filter(|h| h.algo == 1)
            .map(|h| h.value.to_ascii_lowercase())
            .find(|h| h.len() == 40 && h.bytes().all(|b| b.is_ascii_hexdigit()))
    }

    /// 能直接下载的地址（必须是 CurseForge 的 CDN）；作者禁止第三方下载时为 `None`
    pub fn usable_download_url(&self) -> Option<&str> {
        self.download_url
            .as_deref()
            .filter(|url| is_curseforge_download(url))
    }

    /// 必需的前置项目
    pub fn required_dependencies(&self) -> impl Iterator<Item = u32> + '_ {
        self.dependencies
            .iter()
            .filter(|d| d.relation_type == RELATION_REQUIRED)
            .map(|d| d.mod_id)
    }

    pub fn is_release(&self) -> bool {
        self.release_type == 1
    }
}

/// 下载地址是不是 CurseForge 自己的 CDN
pub fn is_curseforge_download(url: &str) -> bool {
    url_host(url).is_some_and(|host| DOWNLOAD_HOSTS.contains(&host.as_str()))
}

/// 文件的精确下载页（官网的「下载这个文件」页面，打开后浏览器会开始下载这个版本）。
/// 项目没有官网地址、或地址不是 CurseForge 官网时返回 `None`。
pub fn download_page(website_url: Option<&str>, file_id: u32) -> Option<String> {
    let website = website_url?.trim_end_matches('/');
    website
        .starts_with(WEBSITE)
        .then(|| format!("{website}/download/{file_id}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实接口返回的一个「作者禁止第三方下载」的文件（字段有删减）
    const BLOCKED_FILE: &str = r#"{"id":8240430,"gameId":432,"modId":580036,"isAvailable":true,
        "displayName":"[1.6] Enhanced Boss Bars.zip","fileName":"[1.6] Enhanced Boss Bars.zip","releaseType":1,
        "fileStatus":4,"hashes":[{"value":"A6B76044420DEB0FDA21EE8748D4079DDF7A1F9F","algo":1},
        {"value":"60c460e7ec4b6f11b72642f16519f8fc","algo":2}],"fileLength":123242,"downloadUrl":null,
        "gameVersions":["1.21","1.21.1"],"dependencies":[],"fileFingerprint":3259473498}"#;

    #[test]
    fn parses_a_blocked_file() {
        let file: File = serde_json::from_str(BLOCKED_FILE).unwrap();
        assert_eq!(file.id, 8_240_430);
        assert_eq!(
            file.sha1().as_deref(),
            Some("a6b76044420deb0fda21ee8748d4079ddf7a1f9f"),
            "统一成小写"
        );
        assert_eq!(file.usable_download_url(), None);
        assert_eq!(
            file.file_fingerprint, 3_259_473_498,
            "指纹超过 i32 也能装下"
        );
        assert!(file.is_release());
    }

    #[test]
    fn only_curseforge_cdn_counts_as_a_download_url() {
        let mut file: File = serde_json::from_str(BLOCKED_FILE).unwrap();
        file.download_url = Some("https://edge.forgecdn.net/files/8240/430/a.zip".to_owned());
        assert!(file.usable_download_url().is_some());
        for bad in [
            "http://edge.forgecdn.net/files/a.zip",
            "https://edge.forgecdn.net.evil.com/a.zip",
            "https://evil.com/files/a.zip",
        ] {
            file.download_url = Some(bad.to_owned());
            assert_eq!(file.usable_download_url(), None, "{bad}");
        }
    }

    #[test]
    fn download_page_points_at_the_exact_file() {
        assert_eq!(
            download_page(
                Some("https://www.curseforge.com/minecraft/mc-mods/entityculling/"),
                9_100_286
            )
            .as_deref(),
            Some("https://www.curseforge.com/minecraft/mc-mods/entityculling/download/9100286")
        );
        assert_eq!(download_page(Some("https://evil.com/x"), 1), None);
        assert_eq!(download_page(None, 1), None);
    }

    #[test]
    fn search_hits_reuse_the_common_shape() {
        let json = r#"{"id":238222,"name":"Just Enough Items (JEI)","summary":"View Items",
            "classId":6,"links":{"websiteUrl":"https://www.curseforge.com/minecraft/mc-mods/jei"},
            "logo":{"thumbnailUrl":"https://media.forgecdn.net/avatars/thumbnails/1/2/64/64/x.png"},
            "authors":[{"name":"mezz"}],"downloadCount":1.5e8,"allowModDistribution":true,
            "categories":[{"name":"API and Library"}]}"#;
        let hit = serde_json::from_str::<Mod>(json).unwrap().into_hit();
        assert_eq!(hit.project_id, "238222");
        assert_eq!(hit.author, "mezz");
        assert!(
            hit.icon_url
                .unwrap()
                .starts_with("https://media.forgecdn.net/")
        );
        assert_eq!(hit.display_categories, ["API and Library"]);
        assert_eq!(
            hit.page_url.as_deref(),
            Some("https://www.curseforge.com/minecraft/mc-mods/jei")
        );
        assert!(!hit.manual_only);
    }

    #[test]
    fn hits_whose_author_blocks_launchers_are_manual_only() {
        // 地图常见：作者关了第三方下载，文件没有下载地址，只能去网页上下
        let json = r#"{"id":1059850,"name":"Mini Parkour Paradise","summary":"跑酷",
            "classId":17,"allowModDistribution":false}"#;
        let hit = serde_json::from_str::<Mod>(json).unwrap().into_hit();
        assert!(hit.manual_only);
        // 平台没说的时候不当成禁止
        let unknown = r#"{"id":2,"name":"X","summary":""}"#;
        assert!(
            !serde_json::from_str::<Mod>(unknown)
                .unwrap()
                .into_hit()
                .manual_only
        );
    }

    #[test]
    fn detail_uses_screenshots_author_page_and_falls_back_to_summary() {
        let json = r#"{"id":1,"name":"X","summary":"一句话简介",
            "links":{"websiteUrl":"https://www.curseforge.com/minecraft/mc-mods/x",
                     "sourceUrl":"https://github.com/a/x","issuesUrl":"http://bad.example"},
            "authors":[{"name":"a","url":"https://www.curseforge.com/members/a"}],
            "screenshots":[{"title":"","thumbnailUrl":"https://media.forgecdn.net/t.png","url":"https://media.forgecdn.net/b.png"},
                           {"title":"坏的","url":"ftp://x"}],
            "dateCreated":"2020-01-01T00:00:00Z","dateModified":"2026-11-10T00:00:00Z",
            "latestFilesIndexes":[{"gameVersion":"1.21.1","modLoader":6},{"gameVersion":"1.20.1","modLoader":1},
                                  {"gameVersion":"1.21.1","modLoader":4},{"gameVersion":"1.21.1"}]}"#;
        let detail = serde_json::from_str::<Mod>(json)
            .unwrap()
            .into_detail(String::new());
        assert_eq!(detail.body, "一句话简介");
        assert_eq!(detail.body_format, BodyFormat::Html);
        assert_eq!(detail.gallery.len(), 1);
        assert_eq!(
            detail.gallery[0].thumb_url,
            "https://media.forgecdn.net/t.png"
        );
        assert_eq!(detail.gallery[0].title, None);
        assert_eq!(
            detail.hit.gallery_url.as_deref(),
            Some("https://media.forgecdn.net/t.png")
        );
        assert_eq!(
            detail.hit.author_url.as_deref(),
            Some("https://www.curseforge.com/members/a")
        );
        assert_eq!(detail.hit.updated.as_deref(), Some("2026-11-10T00:00:00Z"));
        assert_eq!(detail.published.as_deref(), Some("2020-01-01T00:00:00Z"));
        assert_eq!(detail.links.len(), 1, "http 的问题反馈地址被去掉了");
        assert_eq!(detail.game_versions, ["1.21.1", "1.20.1"]);
        assert_eq!(detail.loaders, ["neoforge", "forge", "fabric"]);
        assert_eq!(detail.members.len(), 1);
        assert_eq!(
            detail.members[0].url.as_deref(),
            Some("https://www.curseforge.com/members/a")
        );
    }

    #[test]
    fn picks_the_newest_release_and_skips_server_packs() {
        let file = |id: u32, release_type: u32, server: bool| {
            let mut f: File = serde_json::from_str(BLOCKED_FILE).unwrap();
            f.id = id;
            f.release_type = release_type;
            f.is_server_pack = server;
            f
        };
        let picked = best_file(vec![file(5, 2, false), file(4, 1, true), file(3, 1, false)]);
        assert_eq!(picked.unwrap().id, 3, "跳过测试版和服务端包");
        let only_beta = best_file(vec![file(5, 2, false), file(4, 3, false)]);
        assert_eq!(only_beta.unwrap().id, 5, "没有正式版就取最新的");
        assert!(best_file(Vec::new()).is_none());
        assert_eq!(
            kind_of_class(Some(class::SHADER)),
            Some(ContentKind::Shader)
        );
        assert_eq!(kind_of_class(Some(class::MODPACK)), None);
        assert_eq!(class_for(ContentKind::ResourcePack), class::RESOURCE_PACK);
    }

    #[tokio::test]
    async fn refuses_to_call_without_a_key() {
        // D18b 起没 Key 会改问 MCIM；「仅官方源」时才直接拒绝（这样测不用联网）
        crate::content::mirror::curseforge()
            .set_policy(crate::config::DownloadSource::OfficialOnly);
        let cf = CurseForge::new(Client::new(), None);
        assert!(!cf.usable());
        assert!(!cf.has_key());
        assert!(cf.download_header().is_none());
        assert!(matches!(
            cf.files(&[1]).await,
            Err(ContentError::NoCurseForgeKey)
        ));
    }

    /// 真实环境检查（要联网和 `MCL_CURSEFORGE_KEY`，默认不跑）：
    /// `cargo test -p launcher-core real_resolve -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "要联网和 MCL_CURSEFORGE_KEY"]
    async fn real_resolve_marks_how_each_file_is_delivered() {
        let client = crate::net::build_client().unwrap();
        let cf = CurseForge::from_env(client.clone());
        assert!(cf.has_key(), "没有设置 MCL_CURSEFORGE_KEY");
        let installed = HashSet::new();
        let target = Target {
            game_version: "1.21.1",
            loader: Some(LOADER_NEOFORGE),
            installed: &installed,
        };
        let show = |plan: &[PlanItem]| {
            for item in plan {
                println!(
                    "  {:?} {} {} → {:?} {}",
                    item.reason,
                    item.title,
                    item.version_number,
                    item.delivery,
                    item.page_url.as_deref().unwrap_or("")
                );
            }
        };

        // JEI：允许第三方下载
        let jei = cf
            .resolve(&client, &target, 238_222, ContentKind::Mod)
            .await
            .unwrap();
        show(&jei);
        assert_eq!(jei[0].delivery, Delivery::Direct);

        // Better Fog：作者禁止第三方下载（项目编号从整合包里的文件查）
        let better_fog = cf.files(&[8_938_115]).await.unwrap()[0].mod_id;
        let plan = cf
            .resolve(&client, &target, better_fog, ContentKind::Mod)
            .await
            .unwrap();
        show(&plan);
        assert_ne!(plan[0].delivery, Delivery::Direct);
        if plan[0].delivery == Delivery::Manual {
            let page = plan[0].page_url.as_deref().unwrap();
            assert!(page.starts_with("https://www.curseforge.com/") && page.contains("/download/"));
        }

        // 装光影自动补上 Iris 和它的前置 Sodium
        let shader = cf
            .resolve(&client, &target, 627_557, ContentKind::Shader)
            .await
            .unwrap();
        show(&shader);
        assert!(
            shader
                .iter()
                .any(|p| p.project_id == IRIS_PROJECT.to_string())
        );
        assert!(
            shader.iter().any(|p| p.project_id == "394468"),
            "Iris 的前置 Sodium"
        );
    }

    #[test]
    fn picks_latest_release_file_from_indexes() {
        let indexes = vec![
            FileIndex {
                game_version: "1.21.1".into(),
                file_id: 100,
                release_type: 1,
                mod_loader: Some(LOADER_NEOFORGE),
            },
            FileIndex {
                game_version: "1.21.1".into(),
                file_id: 150,
                release_type: 2,
                mod_loader: Some(LOADER_NEOFORGE),
            },
            FileIndex {
                game_version: "1.21.1".into(),
                file_id: 120,
                release_type: 1,
                mod_loader: Some(LOADER_NEOFORGE),
            },
            FileIndex {
                game_version: "1.21.1".into(),
                file_id: 200,
                release_type: 1,
                mod_loader: Some(LOADER_FABRIC),
            },
        ];
        assert_eq!(
            pick_latest_index(&indexes, "1.21.1", Some(LOADER_NEOFORGE)),
            Some(120)
        );
        assert_eq!(
            pick_latest_index(&indexes, "1.21.1", Some(LOADER_FABRIC)),
            Some(200)
        );
        assert_eq!(
            pick_latest_index(&indexes, "1.20.1", Some(LOADER_NEOFORGE)),
            None
        );
    }
}
