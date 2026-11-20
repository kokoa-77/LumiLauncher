//! Modrinth API（v2）：搜索、选版本、解析前置、按哈希识别已装的文件。
//!
//! 文档：<https://docs.modrinth.com/api/>。只读接口不需要登录；请求头带启动器自己的 User-Agent。

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use reqwest::Client;
use serde::{Deserialize, Serialize};

use super::detail::{
    BodyFormat, ContentVersion, GalleryImage, LinkKind, ProjectDetail, ProjectMember, ReleaseType,
    https_only, release_versions,
};

use super::{ContentError, ContentKind, ContentSource, Delivery, SearchSort, modrinth_category};
use crate::net::{HttpError, url_encode as encode};

const API: &str = "https://api.modrinth.com/v2";
/// Modrinth 的文件 CDN
const CDN_HOST: &str = "cdn.modrinth.com";
/// Iris 的项目 ID：装光影时自动补上（NeoForge / Fabric 上运行光影需要它）
pub const IRIS_PROJECT: &str = "YL57xq9U";
/// 前置最多解析几层，防止循环依赖或异常数据拖垮
const MAX_DEPENDENCY_DEPTH: usize = 6;

/// 搜索结果里的一个项目。
/// 给界面用的搜索结果（字段齐全，不含可选的怪形状）。收藏夹也原样存这一份（所以能读回来）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct ProjectHit {
    pub project_id: String,
    pub slug: String,
    pub title: String,
    pub description: String,
    pub author: String,
    pub icon_url: Option<String>,
    pub downloads: f64,
    pub display_categories: Vec<String>,
    /// 哪个平台的项目（`project_id` 是这个平台的编号）
    pub source: ContentSource,
    /// 要装在哪一端（Modrinth 的标注；CurseForge 没有这项）
    pub side: Option<Side>,
    /// 项目在平台官网上的页面（「在浏览器中打开」用）
    pub page_url: Option<String>,
    /// 展示图（大图卡片用）：Modrinth 的精选图，CurseForge 的第一张截图
    #[serde(default)]
    pub gallery_url: Option<String>,
    /// 作者主页（点作者名用浏览器打开）
    #[serde(default)]
    pub author_url: Option<String>,
    /// 最后更新时间（ISO 8601，界面换算成「4 小时前」）
    #[serde(default)]
    pub updated: Option<String>,
    /// 项目创建时间（ISO 8601；两个平台合在一起按「最新发布」排时用）
    #[serde(default)]
    pub published: Option<String>,
    /// 多人服务器才有：地址、在线人数、要不要先装它的整合包
    #[serde(default)]
    pub server: Option<ServerInfo>,
    /// 作者不许第三方启动器下载（CurseForge 的设置）：只能去平台页面手动下载
    #[serde(default)]
    pub manual_only: bool,
    /// 中文名（中文名表或装过的文件自带的；D17e）。搜索命令返回前补上
    #[serde(default)]
    pub title_zh: Option<String>,
    /// 同一个项目在另一个平台上的编号（中文名表或同一个文件确认过的；D17e）。搜索命令返回前补上
    #[serde(default)]
    pub twin_id: Option<String>,
}

/// 一个多人服务器（Modrinth 的服务器列表）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    /// 服务器地址（可以带端口）
    pub address: String,
    /// 平台最近一次测到的在线人数、最多人数
    pub players_online: Option<f64>,
    pub players_max: Option<f64>,
    /// 支持的游戏版本（新的在前）
    #[serde(default)]
    pub game_versions: Vec<String>,
    /// 要先装服务器自己的整合包才能进：整合包在 Modrinth 上的版本编号
    pub modpack_version: Option<String>,
    /// 那个整合包的项目编号：大多就是服务器自己，也有服务器用别人发的整合包
    #[serde(default)]
    pub modpack_project: Option<String>,
}

/// 短名、用户名这类放进网址的值：只认字母数字和 `-_.`
fn url_safe(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

/// Modrinth 用户主页
pub(crate) fn modrinth_user(name: &str) -> Option<String> {
    url_safe(name).then(|| format!("https://modrinth.com/user/{name}"))
}

/// Modrinth 项目页：`https://modrinth.com/<类型>/<短名>`。类型或短名不像正常值时不给地址。
fn modrinth_page(project_type: &str, slug: &str) -> Option<String> {
    let kind = match project_type {
        "mod" | "modpack" | "resourcepack" | "shader" | "datapack" | "plugin" => project_type,
        _ => return None,
    };
    url_safe(slug).then(|| format!("https://modrinth.com/{kind}/{slug}"))
}

/// 一个项目要装在哪一端。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum Side {
    /// 只装在客户端（服务端用不了）
    Client,
    /// 只装在服务端
    Server,
    /// 两端都必须装
    Both,
    /// 装在哪一端都行
    Either,
}

/// Modrinth 的 `client_side` / `server_side`（required、optional、unsupported、unknown）→ 装在哪一端
fn side_of(client: &str, server: &str) -> Option<Side> {
    let ok = |s: &str| matches!(s, "required" | "optional");
    match (client, server) {
        (c, "unsupported") if ok(c) => Some(Side::Client),
        ("unsupported", s) if ok(s) => Some(Side::Server),
        ("required", "required") => Some(Side::Both),
        (c, s) if ok(c) && ok(s) => Some(Side::Either),
        _ => None,
    }
}

/// Modrinth 返回的原始格式（下划线命名，很多字段可能缺）
#[derive(Debug, Deserialize)]
struct RawHit {
    project_id: String,
    slug: String,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    icon_url: Option<String>,
    #[serde(default)]
    downloads: f64,
    #[serde(default)]
    display_categories: Vec<String>,
    #[serde(default)]
    client_side: String,
    #[serde(default)]
    server_side: String,
    #[serde(default)]
    project_type: String,
    /// 作者选的精选图
    #[serde(default)]
    featured_gallery: Option<String>,
    #[serde(default)]
    gallery: Vec<String>,
    #[serde(default)]
    date_modified: Option<String>,
    #[serde(default)]
    date_created: Option<String>,
}

impl From<RawHit> for ProjectHit {
    fn from(raw: RawHit) -> Self {
        let page_url = modrinth_page(&raw.project_type, &raw.slug);
        let gallery_url = raw
            .featured_gallery
            .filter(|u| !u.is_empty())
            .or_else(|| raw.gallery.into_iter().next())
            .and_then(https_only);
        let author_url = modrinth_user(&raw.author);
        Self {
            project_id: raw.project_id,
            slug: raw.slug,
            title: raw.title,
            description: raw.description,
            author: raw.author,
            icon_url: raw.icon_url.filter(|u| !u.is_empty()),
            downloads: raw.downloads,
            side: side_of(&raw.client_side, &raw.server_side),
            page_url,
            display_categories: raw.display_categories,
            source: ContentSource::Modrinth,
            gallery_url,
            author_url,
            updated: raw.date_modified,
            published: raw.date_created,
            server: None,
            manual_only: false,
            title_zh: None,
            twin_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct SearchPage {
    pub hits: Vec<ProjectHit>,
    pub total: u32,
    pub offset: u32,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    hits: Vec<RawHit>,
    total_hits: u32,
    offset: u32,
}

/// 搜索条件。
#[derive(Debug, Clone)]
pub struct SearchQuery<'a> {
    pub text: &'a str,
    pub kind: ContentKind,
    pub game_version: Option<&'a str>,
    /// 模组加载器（例如 `neoforge`），只对模组生效
    pub loader: Option<&'a str>,
    pub sort: SearchSort,
    /// 只看这一个分类（Modrinth 的分类名，例如 `technology`）
    pub category: Option<&'a str>,
    pub offset: u32,
    pub limit: u32,
}

pub async fn search(client: &Client, query: &SearchQuery<'_>) -> Result<SearchPage, ContentError> {
    let mut facets: Vec<Vec<String>> =
        vec![vec![format!("project_type:{}", query.kind.project_type())]];
    if let Some(version) = query.game_version {
        facets.push(vec![format!("versions:{version}")]);
    }
    if query.kind == ContentKind::Mod
        && let Some(loader) = query.loader
    {
        facets.push(vec![format!("categories:{loader}")]);
    }
    push_category(&mut facets, query.category);
    let url = search_url(
        &format!("{API}/search"),
        &facets,
        query.text,
        query.sort,
        query.offset,
        query.limit,
    )?;
    fetch_page(client, url).await
}

/// 分类筛选：和其他条件是「并且」的关系（facets 外层是 AND）
fn push_category(facets: &mut Vec<Vec<String>>, category: Option<&str>) {
    if let Some(category) = category.and_then(modrinth_category) {
        facets.push(vec![format!("categories:{category}")]);
    }
}

/// 目前能导入的整合包加载器。
pub const PACK_LOADERS: &[&str] = &["neoforge", "fabric", "forge", "quilt"];

/// 搜索整合包：只列能装的加载器。
///
/// 注意：搜索的筛选只保证「某个版本用了这个加载器」，不保证最新版也是，
/// 所以真正装的版本还要用 [`pack_versions`] 再按加载器挑一次。
pub async fn search_modpacks(
    client: &Client,
    text: &str,
    sort: SearchSort,
    category: Option<&str>,
    offset: u32,
    limit: u32,
) -> Result<SearchPage, ContentError> {
    let mut facets = vec![
        vec!["project_type:modpack".to_owned()],
        PACK_LOADERS
            .iter()
            .map(|l| format!("categories:{l}"))
            .collect(),
    ];
    push_category(&mut facets, category);
    let url = search_url(&format!("{API}/search"), &facets, text, sort, offset, limit)?;
    fetch_page(client, url).await
}

/// 拼搜索地址（v2、v3 共用）
fn search_url(
    endpoint: &str,
    facets: &[Vec<String>],
    text: &str,
    sort: SearchSort,
    offset: u32,
    limit: u32,
) -> Result<String, ContentError> {
    let facets = serde_json::to_string(facets).map_err(|e| ContentError::Api(e.to_string()))?;
    Ok(format!(
        "{endpoint}?query={}&facets={}&index={}&offset={offset}&limit={}",
        encode(text.trim()),
        encode(&facets),
        sort.modrinth_index(text),
        limit.clamp(1, 50)
    ))
}

async fn fetch_page(client: &Client, url: String) -> Result<SearchPage, ContentError> {
    let response: SearchResponse = get_json(client, &[url]).await?;
    Ok(SearchPage {
        hits: response.hits.into_iter().map(Into::into).collect(),
        total: response.total_hits,
        offset: response.offset,
    })
}

/// Modrinth 项目接口的原始格式（只取详情页要用的）
#[derive(Debug, Deserialize)]
struct RawProject {
    id: String,
    slug: String,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    icon_url: Option<String>,
    #[serde(default)]
    downloads: f64,
    #[serde(default)]
    followers: Option<f64>,
    #[serde(default)]
    categories: Vec<String>,
    #[serde(default)]
    client_side: String,
    #[serde(default)]
    server_side: String,
    #[serde(default)]
    project_type: String,
    #[serde(default)]
    published: Option<String>,
    #[serde(default)]
    updated: Option<String>,
    #[serde(default)]
    license: Option<RawLicense>,
    #[serde(default)]
    issues_url: Option<String>,
    #[serde(default)]
    source_url: Option<String>,
    #[serde(default)]
    wiki_url: Option<String>,
    #[serde(default)]
    discord_url: Option<String>,
    #[serde(default)]
    donation_urls: Vec<RawDonation>,
    #[serde(default)]
    gallery: Vec<RawGallery>,
    #[serde(default)]
    game_versions: Vec<String>,
    #[serde(default)]
    loaders: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RawLicense {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
}

#[derive(Debug, Deserialize)]
struct RawDonation {
    #[serde(default)]
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawGallery {
    url: String,
    #[serde(default)]
    featured: bool,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    ordering: i64,
}

#[derive(Debug, Deserialize)]
struct RawMember {
    #[serde(default)]
    team_id: String,
    user: RawUser,
    #[serde(default)]
    role: String,
    #[serde(default)]
    ordering: i64,
}

#[derive(Debug, Deserialize)]
struct RawUser {
    username: String,
    #[serde(default)]
    avatar_url: Option<String>,
}

/// 角色排第几：所有者 → 项目负责人 / 主要开发者 → 其他人
fn role_rank(role: &str) -> u8 {
    match role.trim().to_ascii_lowercase().as_str() {
        "owner" => 0,
        "project lead" | "lead developer" | "lead" | "creator" | "author" => 1,
        _ => 2,
    }
}

/// 作者团队：所有者在前，其次项目负责人，其余按作者排的顺序
fn members_of(mut members: Vec<RawMember>) -> Vec<ProjectMember> {
    members.sort_by_key(|m| (role_rank(&m.role), m.ordering));
    members
        .into_iter()
        .map(|m| ProjectMember {
            url: modrinth_user(&m.user.username),
            avatar_url: m.user.avatar_url.and_then(https_only),
            role: Some(m.role).filter(|r| !r.trim().is_empty()),
            name: m.user.username,
        })
        .collect()
}

/// 许可证名：自定义许可证 Modrinth 只给编号（`LicenseRef-Polyform-Shield-1.0.0`），去掉前缀、把横线换成空格
fn license_name(license: Option<RawLicense>) -> Option<String> {
    let license = license?;
    let raw = if license.name.trim().is_empty() {
        license.id
    } else {
        license.name
    };
    let name = match raw.strip_prefix("LicenseRef-") {
        Some(rest) => rest.replace('-', " "),
        None => raw,
    };
    (!name.trim().is_empty()).then_some(name)
}

impl RawProject {
    fn into_detail(self, members: Vec<ProjectMember>) -> ProjectDetail {
        let author = members.first().map(|m| m.name.clone()).unwrap_or_default();
        let mut gallery = self.gallery;
        // 精选图在前，其余按作者排的顺序
        gallery.sort_by_key(|g| (!g.featured, g.ordering));
        let gallery: Vec<GalleryImage> = gallery
            .into_iter()
            .filter_map(|g| {
                let url = https_only(g.url)?;
                Some(GalleryImage {
                    thumb_url: url.clone(),
                    url,
                    title: g.title.filter(|t| !t.trim().is_empty()),
                })
            })
            .collect();
        let links = super::detail::links_of(
            [
                (LinkKind::Source, self.source_url),
                (LinkKind::Issues, self.issues_url),
                (LinkKind::Wiki, self.wiki_url),
                (LinkKind::Discord, self.discord_url),
            ]
            .into_iter()
            .chain(
                self.donation_urls
                    .into_iter()
                    .map(|d| (LinkKind::Donate, d.url)),
            ),
        );
        let hit = ProjectHit {
            page_url: modrinth_page(&self.project_type, &self.slug),
            author_url: modrinth_user(&author),
            gallery_url: gallery.first().map(|g| g.url.clone()),
            project_id: self.id,
            slug: self.slug,
            title: self.title,
            description: self.description,
            author,
            icon_url: self.icon_url.filter(|u| !u.is_empty()),
            downloads: self.downloads,
            display_categories: self.categories,
            source: ContentSource::Modrinth,
            side: side_of(&self.client_side, &self.server_side),
            updated: self.updated,
            published: self.published.clone(),
            server: None,
            manual_only: false,
            title_zh: None,
            twin_id: None,
        };
        ProjectDetail {
            hit,
            body: self.body,
            body_format: BodyFormat::Markdown,
            gallery,
            links,
            license: license_name(self.license),
            published: self.published,
            followers: self.followers,
            game_versions: release_versions(self.game_versions),
            loaders: self.loaders,
            members,
        }
    }
}

/// 一个项目的详情（介绍、图片、链接）。项目信息和团队成员同时请求；作者拿不到不影响看介绍。
pub async fn project_detail(
    client: &Client,
    project_id: &str,
) -> Result<ProjectDetail, ContentError> {
    let project_url = format!("{API}/project/{}", encode(project_id));
    let members_url = format!("{API}/project/{}/members", encode(project_id));
    let project_urls = [project_url];
    let members_urls = [members_url];
    let (project, members) = tokio::join!(
        get_json::<RawProject>(client, &project_urls),
        get_json::<Vec<RawMember>>(client, &members_urls),
    );
    let project = project?;
    let members = match members {
        Ok(members) => members_of(members),
        Err(error) => {
            tracing::warn!(%error, project = project_id, "读取 Modrinth 项目成员失败，详情页不显示作者");
            Vec::new()
        }
    };
    Ok(project.into_detail(members))
}

/// Modrinth 第 3 版搜索接口里的一个服务器（只取要用的）
#[derive(Debug, Deserialize)]
struct RawServerHit {
    project_id: String,
    #[serde(default)]
    slug: String,
    name: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    icon_url: Option<String>,
    #[serde(default)]
    downloads: f64,
    #[serde(default)]
    display_categories: Vec<String>,
    #[serde(default)]
    featured_gallery: Option<String>,
    #[serde(default)]
    gallery: Vec<String>,
    #[serde(default)]
    date_modified: Option<String>,
    #[serde(default)]
    date_created: Option<String>,
    #[serde(default)]
    game_versions: Vec<String>,
    #[serde(default)]
    minecraft_java_server: Option<RawJavaServer>,
}

#[derive(Debug, Deserialize)]
struct RawJavaServer {
    #[serde(default)]
    address: String,
    #[serde(default)]
    content: Option<RawServerContent>,
    #[serde(default)]
    ping: Option<RawPing>,
}

#[derive(Debug, Deserialize)]
struct RawServerContent {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    version_id: Option<String>,
    #[serde(default)]
    project_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawPing {
    #[serde(default)]
    data: Option<RawPingData>,
}

#[derive(Debug, Deserialize)]
struct RawPingData {
    #[serde(default)]
    players_online: Option<f64>,
    #[serde(default)]
    players_max: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct ServerSearchResponse {
    hits: Vec<RawServerHit>,
    #[serde(default)]
    total_hits: u32,
    #[serde(default)]
    offset: u32,
}

impl RawServerHit {
    /// 没有地址的服务器进不去，不要
    fn into_hit(self) -> Option<ProjectHit> {
        let java = self.minecraft_java_server?;
        let address = java.address.trim().to_owned();
        if address.is_empty() {
            return None;
        }
        let ping = java.ping.and_then(|p| p.data);
        let modpack = java.content.filter(|c| c.kind == "modpack");
        let modpack_project = modpack
            .as_ref()
            .and_then(|c| c.project_id.clone())
            .filter(|v| url_safe(v));
        let modpack_version = modpack.and_then(|c| c.version_id).filter(|v| url_safe(v));
        let gallery_url = self
            .featured_gallery
            .filter(|u| !u.is_empty())
            .or_else(|| self.gallery.into_iter().next())
            .and_then(https_only);
        Some(ProjectHit {
            // 服务器页的路径以后可能变，用通用的项目地址，Modrinth 会自己跳过去
            page_url: url_safe(&self.project_id)
                .then(|| format!("https://modrinth.com/project/{}", self.project_id)),
            author_url: modrinth_user(&self.author),
            project_id: self.project_id,
            slug: self.slug,
            title: self.name,
            description: self.summary,
            author: self.author,
            icon_url: self.icon_url.filter(|u| !u.is_empty()),
            downloads: self.downloads,
            display_categories: self.display_categories,
            source: ContentSource::Modrinth,
            side: None,
            gallery_url,
            updated: self.date_modified,
            published: self.date_created,
            server: Some(ServerInfo {
                address,
                players_online: ping.as_ref().and_then(|p| p.players_online),
                players_max: ping.as_ref().and_then(|p| p.players_max),
                game_versions: release_versions(self.game_versions),
                modpack_version,
                modpack_project,
            }),
            manual_only: false,
            title_zh: None,
            twin_id: None,
        })
    }
}

/// 搜多人服务器（Modrinth 的服务器列表，第 3 版接口）。没有关键词时按热度排。
pub async fn search_servers(
    client: &Client,
    text: &str,
    sort: SearchSort,
    category: Option<&str>,
    offset: u32,
    limit: u32,
) -> Result<SearchPage, ContentError> {
    let mut facets = vec![vec!["project_types:minecraft_java_server".to_owned()]];
    push_category(&mut facets, category);
    let url = search_url(
        "https://api.modrinth.com/v3/search",
        &facets,
        text,
        sort,
        offset,
        limit,
    )?;
    let response: ServerSearchResponse = get_json(client, &[url]).await?;
    Ok(SearchPage {
        hits: response
            .hits
            .into_iter()
            .filter_map(RawServerHit::into_hit)
            .collect(),
        total: response.total_hits,
        offset: response.offset,
    })
}

/// 整合包里能装的版本（加载器在 [`PACK_LOADERS`] 里），新的在前。
pub async fn pack_versions(
    client: &Client,
    project_id: &str,
) -> Result<Vec<Version>, ContentError> {
    let loaders =
        serde_json::to_string(PACK_LOADERS).map_err(|e| ContentError::Api(e.to_string()))?;
    let url = format!(
        "{API}/project/{}/version?loaders={}",
        encode(project_id),
        encode(&loaders)
    );
    get_json(client, &[url]).await
}

#[derive(Debug, Clone, Deserialize)]
pub struct Version {
    pub id: String,
    pub project_id: String,
    #[serde(default)]
    pub name: String,
    pub version_number: String,
    #[serde(default)]
    pub version_type: String,
    #[serde(default)]
    pub date_published: Option<String>,
    #[serde(default)]
    pub downloads: Option<f64>,
    #[serde(default)]
    pub changelog: Option<String>,
    #[serde(default)]
    pub loaders: Vec<String>,
    #[serde(default)]
    pub game_versions: Vec<String>,
    #[serde(default)]
    pub files: Vec<VersionFile>,
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
}

/// 列出项目的版本（可按游戏版本、加载器过滤），新的在前（D25e）。
pub async fn project_versions(
    client: &Client,
    project_id: &str,
    game_version: Option<&str>,
    loader: Option<&str>,
) -> Result<Vec<ContentVersion>, ContentError> {
    let mut url = format!("{API}/project/{}/version", encode(project_id));
    let mut params = Vec::new();
    if let Some(gv) = game_version.filter(|s| !s.trim().is_empty()) {
        let json = serde_json::to_string(&[gv]).map_err(|e| ContentError::Api(e.to_string()))?;
        params.push(format!("game_versions={}", encode(&json)));
    }
    if let Some(l) = loader.filter(|s| !s.trim().is_empty()) {
        let json = serde_json::to_string(&[l]).map_err(|e| ContentError::Api(e.to_string()))?;
        params.push(format!("loaders={}", encode(&json)));
    }
    if !params.is_empty() {
        url.push('?');
        url.push_str(&params.join("&"));
    }
    let versions: Vec<Version> = get_json(client, &[url]).await?;
    Ok(versions
        .into_iter()
        .filter_map(|v| {
            let file = v.primary_file()?.clone();
            let release_type = match v.version_type.as_str() {
                "beta" => ReleaseType::Beta,
                "alpha" => ReleaseType::Alpha,
                _ => ReleaseType::Release,
            };
            let display_name = if v.name.trim().is_empty() {
                v.version_number.clone()
            } else {
                v.name
            };
            Some(ContentVersion {
                id: v.id,
                project_id: v.project_id,
                source: ContentSource::Modrinth,
                name: display_name,
                version_number: v.version_number,
                release_type,
                game_versions: v.game_versions,
                loaders: v.loaders,
                published: v.date_published,
                downloads: v.downloads,
                changelog: v.changelog.filter(|c| !c.trim().is_empty()),
                changelog_format: BodyFormat::Markdown,
                file_name: file.filename,
                file_size: file.size as f64,
                delivery: Delivery::Direct,
                page_url: None,
            })
        })
        .collect())
}

impl Version {
    /// 主文件（没标主文件就取第一个）
    pub fn primary_file(&self) -> Option<&VersionFile> {
        self.files
            .iter()
            .find(|f| f.primary)
            .or_else(|| self.files.first())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct VersionFile {
    pub url: String,
    pub filename: String,
    #[serde(default)]
    pub primary: bool,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub hashes: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Dependency {
    #[serde(default)]
    pub version_id: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
    pub dependency_type: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Project {
    id: String,
    title: String,
    project_type: String,
    #[serde(default)]
    team: String,
    #[serde(default)]
    icon_url: Option<String>,
}

/// 安装计划里的一项。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct PlanItem {
    pub project_id: String,
    pub version_id: String,
    pub title: String,
    pub version_number: String,
    pub file_name: String,
    pub size: f64,
    pub kind: ContentKind,
    pub icon_url: Option<String>,
    pub reason: PlanReason,
    /// 哪个平台的项目（`project_id` / `version_id` 是这个平台的编号）
    #[serde(default)]
    pub source: ContentSource,
    /// 怎么拿到文件
    #[serde(default)]
    pub delivery: Delivery,
    /// 要用户在浏览器里下载时的精确下载页
    #[serde(default)]
    pub page_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum PlanReason {
    /// 用户点的
    Requested,
    /// 前置
    Dependency,
    /// 装光影需要的光影加载器（Iris）
    ShaderLoader,
}

/// 解析安装计划需要知道的实例信息。
pub struct Target<'a> {
    pub game_version: &'a str,
    /// 实例的模组加载器（`neoforge`）；原版实例为 `None`
    pub loader: Option<&'a str>,
    /// 已经装了的项目（按文件哈希识别出来的），不会重复安装
    pub installed: &'a HashSet<String>,
}

/// 从用户点的项目出发，解析出要装的全部版本（含必需前置；装光影时补上 Iris）。
pub async fn resolve(
    client: &Client,
    target: &Target<'_>,
    project_id: &str,
    kind: ContentKind,
) -> Result<Vec<PlanItem>, ContentError> {
    resolve_with_version(client, target, project_id, None, kind).await
}

/// 解析安装指定版本（含必需前置；装光影时补上 Iris，D25e）。
pub async fn resolve_version(
    client: &Client,
    target: &Target<'_>,
    project_id: &str,
    version_id: &str,
    kind: ContentKind,
) -> Result<Vec<PlanItem>, ContentError> {
    resolve_with_version(client, target, project_id, Some(version_id), kind).await
}

async fn resolve_with_version(
    client: &Client,
    target: &Target<'_>,
    project_id: &str,
    requested_version: Option<&str>,
    kind: ContentKind,
) -> Result<Vec<PlanItem>, ContentError> {
    if kind == ContentKind::Mod && target.loader.is_none() {
        return Err(ContentError::NeedsLoader);
    }
    let mut queue: VecDeque<(String, Option<String>, PlanReason, usize)> = VecDeque::from([(
        project_id.to_owned(),
        requested_version.map(str::to_owned),
        PlanReason::Requested,
        0,
    )]);
    if kind == ContentKind::Shader && !target.installed.contains(IRIS_PROJECT) {
        if target.loader.is_none() {
            return Err(ContentError::NeedsLoader);
        }
        queue.push_back((IRIS_PROJECT.to_owned(), None, PlanReason::ShaderLoader, 0));
    }

    let mut chosen: Vec<(Version, PlanReason)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    while let Some((project, version_id, reason, depth)) = queue.pop_front() {
        if depth > MAX_DEPENDENCY_DEPTH || !seen.insert(project.clone()) {
            continue;
        }
        if reason != PlanReason::Requested && target.installed.contains(&project) {
            continue;
        }
        let version = match version_id {
            Some(id) => fetch_version(client, &id).await?,
            None => pick_version(client, &project, target, kind_for_project(reason, kind)).await?,
        };
        for dependency in version
            .dependencies
            .iter()
            .filter(|d| d.dependency_type == "required")
        {
            let dep_project = match (&dependency.project_id, &dependency.version_id) {
                (Some(project), _) => project.clone(),
                (None, Some(version)) => fetch_version(client, version).await?.project_id,
                (None, None) => continue,
            };
            queue.push_back((
                dep_project,
                dependency.version_id.clone(),
                PlanReason::Dependency,
                depth + 1,
            ));
        }
        chosen.push((version, reason));
    }

    let ids: Vec<String> = chosen.iter().map(|(v, _)| v.project_id.clone()).collect();
    let projects = fetch_projects(client, &ids).await?;
    chosen
        .into_iter()
        .map(|(version, reason)| {
            let project = projects.get(&version.project_id);
            let file = version.primary_file().ok_or_else(|| {
                ContentError::Api(format!("版本 {} 没有文件", version.version_number))
            })?;
            Ok(PlanItem {
                project_id: version.project_id.clone(),
                version_id: version.id.clone(),
                title: project.map_or_else(|| version.project_id.clone(), |p| p.title.clone()),
                version_number: version.version_number.clone(),
                file_name: file.filename.clone(),
                size: file.size as f64,
                kind: project
                    .and_then(|p| ContentKind::from_project_type(&p.project_type))
                    .unwrap_or(ContentKind::Mod),
                icon_url: project.and_then(|p| p.icon_url.clone()),
                reason,
                source: ContentSource::Modrinth,
                delivery: Delivery::Direct,
                page_url: None,
            })
        })
        .collect()
}

/// 前置和 Iris 都是模组；只有用户点的那个按它自己的类型选版本
fn kind_for_project(reason: PlanReason, requested: ContentKind) -> ContentKind {
    match reason {
        PlanReason::Requested => requested,
        PlanReason::Dependency | PlanReason::ShaderLoader => ContentKind::Mod,
    }
}

/// 给一个项目挑最合适的版本：游戏版本和加载器都对得上，优先正式版，其次最新。
async fn pick_version(
    client: &Client,
    project: &str,
    target: &Target<'_>,
    kind: ContentKind,
) -> Result<Version, ContentError> {
    let loaders = loaders_for(kind, target.loader);
    let url = format!(
        "{API}/project/{}/version?loaders={}&game_versions={}",
        encode(project),
        encode(&serde_json::to_string(&loaders).unwrap_or_default()),
        encode(&serde_json::to_string(&[target.game_version]).unwrap_or_default()),
    );
    let versions: Vec<Version> = get_json(client, &[url]).await?;
    best_version(versions).ok_or_else(|| ContentError::NoCompatibleVersion {
        project: project.to_owned(),
        game_version: target.game_version.to_owned(),
    })
}

/// 版本列表是新的在前：取第一个正式版，没有正式版就取最新的
fn best_version(versions: Vec<Version>) -> Option<Version> {
    let release = versions.iter().position(|v| v.version_type == "release");
    let index = release.unwrap_or(0);
    versions.into_iter().nth(index)
}

pub async fn fetch_version(client: &Client, id: &str) -> Result<Version, ContentError> {
    get_json(client, &[format!("{API}/version/{}", encode(id))]).await
}

/// 按 ID 批量取版本（安装时用：装的就是用户确认过的那些版本）
pub async fn fetch_versions(client: &Client, ids: &[String]) -> Result<Vec<Version>, ContentError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let ids = serde_json::to_string(ids).map_err(|e| ContentError::Api(e.to_string()))?;
    get_json(client, &[format!("{API}/versions?ids={}", encode(&ids))]).await
}

async fn fetch_projects(
    client: &Client,
    ids: &[String],
) -> Result<HashMap<String, Project>, ContentError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let ids = serde_json::to_string(ids).map_err(|e| ContentError::Api(e.to_string()))?;
    let projects: Vec<Project> =
        get_json(client, &[format!("{API}/projects?ids={}", encode(&ids))]).await?;
    Ok(projects.into_iter().map(|p| (p.id.clone(), p)).collect())
}

/// 一批项目的作者（项目编号 → 作者名）：一次查项目、一次批量查团队，和项目数量无关。
pub async fn project_owners(
    client: &Client,
    ids: &[String],
) -> Result<HashMap<String, String>, ContentError> {
    let projects = fetch_projects(client, ids).await?;
    let teams: Vec<String> = projects
        .values()
        .map(|p| p.team.clone())
        .filter(|t| url_safe(t))
        .collect();
    if teams.is_empty() {
        return Ok(HashMap::new());
    }
    let teams_json = serde_json::to_string(&teams).map_err(|e| ContentError::Api(e.to_string()))?;
    let groups: Vec<Vec<RawMember>> = get_json(
        client,
        &[format!("{API}/teams?ids={}", encode(&teams_json))],
    )
    .await?;
    let owner_of_team: HashMap<String, String> = groups
        .into_iter()
        .filter_map(|members| {
            let team = members.first()?.team_id.clone();
            let owner = members_of(members).into_iter().next()?.name;
            Some((team, owner))
        })
        .collect();
    Ok(projects
        .into_values()
        .filter_map(|p| owner_of_team.get(&p.team).map(|o| (p.id, o.clone())))
        .collect())
}

/// 按 SHA-1 识别文件属于哪个项目（整合包装的、手动放进去的模组也能认出来）。
/// 返回 哈希 → 项目 ID；Modrinth 上没有的文件不在结果里。
pub async fn identify(
    client: &Client,
    sha1_hashes: &[String],
) -> Result<HashMap<String, String>, ContentError> {
    Ok(versions_for_hashes(client, sha1_hashes)
        .await?
        .into_iter()
        .map(|(hash, v)| (hash, v.project_id))
        .collect())
}

/// 按 SHA-1 批量查文件对应的版本（一次请求）。
async fn versions_for_hashes(
    client: &Client,
    sha1_hashes: &[String],
) -> Result<BTreeMap<String, Version>, ContentError> {
    if sha1_hashes.is_empty() {
        return Ok(BTreeMap::new());
    }
    post_json(
        client,
        "version_files",
        &serde_json::json!({ "hashes": sha1_hashes, "algorithm": "sha1" }),
        "识别已装文件",
    )
    .await
}

/// 文件在 Modrinth 上支持哪些加载器和游戏版本（拖进来的模组看能不能装，D17g）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileCompat {
    pub project_id: String,
    pub version_number: String,
    pub loaders: Vec<String>,
    pub game_versions: Vec<String>,
}

/// 按 SHA-1 批量查（一次请求），返回 小写哈希 → 支持情况；Modrinth 上没有的不在里面。
pub async fn compat_by_sha1(
    client: &Client,
    sha1_hashes: &[String],
) -> Result<HashMap<String, FileCompat>, ContentError> {
    Ok(versions_for_hashes(client, sha1_hashes)
        .await?
        .into_iter()
        .map(|(hash, v)| {
            (
                hash.to_ascii_lowercase(),
                FileCompat {
                    project_id: v.project_id,
                    version_number: v.version_number,
                    loaders: v.loaders,
                    game_versions: v.game_versions,
                },
            )
        })
        .collect())
}

/// 按 SHA-1 找 Modrinth 上**一模一样的文件**（一次请求），返回 哈希 → 文件。
/// 只收 Modrinth 自己 CDN 上的文件。用来替补 CurseForge 上作者禁止第三方下载的文件：
/// 同一个哈希就是同一个文件，而且是作者自己发在 Modrinth 上的。
pub async fn files_by_sha1(
    client: &Client,
    sha1_hashes: &[String],
) -> Result<HashMap<String, VersionFile>, ContentError> {
    let versions = versions_for_hashes(client, sha1_hashes).await?;
    Ok(versions
        .into_iter()
        .filter_map(|(hash, version)| {
            let file = version.files.into_iter().find(|f| {
                f.hashes
                    .get("sha1")
                    .is_some_and(|h| h.eq_ignore_ascii_case(&hash))
            })?;
            let on_cdn = crate::net::url_host(&file.url).as_deref() == Some(CDN_HOST);
            on_cdn.then_some((hash.to_ascii_lowercase(), file))
        })
        .collect())
}

/// 远程身份：文件在 Modrinth 上是哪个项目的哪个版本
#[derive(Debug, Clone)]
pub struct RemoteIdentity {
    pub project_id: String,
    pub version_id: String,
    pub version_number: String,
    pub title: String,
    pub icon_url: Option<String>,
}

/// 按 SHA-1 批量识别，带上项目名字和图标（两次请求，和文件数量无关）。
pub async fn identify_detailed(
    client: &Client,
    sha1_hashes: &[String],
) -> Result<HashMap<String, RemoteIdentity>, ContentError> {
    let versions = versions_for_hashes(client, sha1_hashes).await?;
    let ids: Vec<String> = versions.values().map(|v| v.project_id.clone()).collect();
    let projects = fetch_projects(client, &ids).await?;
    Ok(versions
        .into_iter()
        .map(|(hash, v)| {
            let project = projects.get(&v.project_id);
            let identity = RemoteIdentity {
                title: project.map_or_else(|| v.project_id.clone(), |p| p.title.clone()),
                icon_url: project.and_then(|p| p.icon_url.clone()),
                project_id: v.project_id,
                version_id: v.id,
                version_number: v.version_number,
            };
            (hash, identity)
        })
        .collect())
}

/// 按 SHA-1 批量查「同一个项目、适用于这个游戏版本和加载器的最新版本」（一次请求）。
/// 返回 哈希 → 最新版本；Modrinth 上没有的文件不在结果里。
pub async fn latest_versions(
    client: &Client,
    sha1_hashes: &[String],
    loaders: &[&str],
    game_version: &str,
) -> Result<BTreeMap<String, Version>, ContentError> {
    if sha1_hashes.is_empty() {
        return Ok(BTreeMap::new());
    }
    post_json(
        client,
        "version_files/update",
        &serde_json::json!({
            "hashes": sha1_hashes,
            "algorithm": "sha1",
            "loaders": loaders,
            "game_versions": [game_version],
        }),
        "检查更新",
    )
    .await
}

/// 各类内容在 Modrinth 上用的「加载器」筛选值
pub fn loaders_for(kind: ContentKind, mod_loader: Option<&str>) -> Vec<&str> {
    match kind {
        ContentKind::Mod => mod_loader.into_iter().collect(),
        ContentKind::Shader => vec!["iris", "optifine"],
        ContentKind::ResourcePack => vec!["minecraft"],
    }
}

/// 问 Modrinth 的接口：按「Modrinth」那行下载源设置，官方不行时改问 MCIM 镜像（D18b，见 [`super::mirror`]）。
/// `urls` 只用第一个（官方地址）；镜像地址由它换出来
async fn get_json<T: serde::de::DeserializeOwned>(
    client: &Client,
    urls: &[String],
) -> Result<T, ContentError> {
    let url = urls
        .first()
        .ok_or_else(|| ContentError::Api("没有要问的地址".to_owned()))?;
    let fetched = super::mirror::modrinth()
        .fetch(client, reqwest::Method::GET, url, None, None, true)
        .await
        .map_err(|error| unreachable_api(url, &error))?;
    serde_json::from_slice(&fetched.bytes)
        .map_err(|e| ContentError::Api(format!("Modrinth 返回的数据格式不对：{e}")))
}

async fn post_json<T: serde::de::DeserializeOwned>(
    client: &Client,
    path: &str,
    body: &serde_json::Value,
    what: &str,
) -> Result<T, ContentError> {
    let body = serde_json::to_vec(body).map_err(|e| ContentError::Api(e.to_string()))?;
    let url = format!("{API}/{path}");
    let fetched = super::mirror::modrinth()
        .fetch(client, reqwest::Method::POST, &url, Some(&body), None, true)
        .await
        .map_err(|e| ContentError::Api(format!("{what}失败：{e}")))?;
    serde_json::from_slice(&fetched.bytes)
        .map_err(|e| ContentError::Api(format!("{what}结果格式不对：{e}")))
}

/// 官方和镜像都没问到：说成「网络请求失败」（界面按网络问题提示）；官方明确拒绝的照实说
fn unreachable_api(url: &str, error: &super::mirror::FetchError) -> ContentError {
    match error {
        super::mirror::FetchError::Unavailable { reason, .. } => {
            ContentError::Http(HttpError::AllSourcesFailed {
                last: format!("{url}：{reason}"),
            })
        }
        other => ContentError::Api(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(id: &str, kind: &str) -> Version {
        Version {
            id: id.to_owned(),
            project_id: "p".to_owned(),
            name: String::new(),
            version_number: id.to_owned(),
            version_type: kind.to_owned(),
            date_published: None,
            downloads: None,
            changelog: None,
            loaders: Vec::new(),
            game_versions: Vec::new(),
            files: Vec::new(),
            dependencies: Vec::new(),
        }
    }

    #[test]
    fn project_detail_keeps_featured_image_first_and_drops_unsafe_links() {
        let json = r#"{"id":"AANobbMI","slug":"sodium","title":"Sodium","description":"快",
            "body":"Sodium 很快","icon_url":"https://cdn.modrinth.com/i.png","downloads":10,
            "followers":5,"categories":["optimization"],"client_side":"required","server_side":"unsupported",
            "project_type":"mod","published":"2020-01-01T00:00:00Z","updated":"2026-11-01T00:00:00Z",
            "license":{"id":"LicenseRef-Polyform-Shield-1.0.0","name":""},
            "source_url":"https://github.com/CaffeineMC/sodium","issues_url":"http://bad.example",
            "discord_url":"","donation_urls":[{"url":"https://ko-fi.com/x"}],
            "gallery":[{"url":"https://cdn.modrinth.com/b.webp","featured":false,"ordering":0},
                       {"url":"https://cdn.modrinth.com/a.webp","featured":true,"title":"主图","ordering":5},
                       {"url":"javascript:1","featured":false,"ordering":1}]}"#;
        let raw: RawProject = serde_json::from_str(json).unwrap();
        let detail = raw.into_detail(vec![ProjectMember {
            name: "jellysquid3".into(),
            role: Some("Owner".into()),
            avatar_url: None,
            url: Some("https://modrinth.com/user/jellysquid3".into()),
        }]);
        assert_eq!(detail.gallery.len(), 2);
        assert_eq!(detail.gallery[0].url, "https://cdn.modrinth.com/a.webp");
        assert_eq!(detail.gallery[0].title.as_deref(), Some("主图"));
        assert_eq!(
            detail.hit.gallery_url.as_deref(),
            Some("https://cdn.modrinth.com/a.webp")
        );
        assert_eq!(
            detail.hit.author_url.as_deref(),
            Some("https://modrinth.com/user/jellysquid3")
        );
        assert_eq!(detail.hit.side, Some(Side::Client));
        assert_eq!(detail.body_format, BodyFormat::Markdown);
        assert_eq!(detail.license.as_deref(), Some("Polyform Shield 1.0.0"));
        let urls: Vec<&str> = detail.links.iter().map(|l| l.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://github.com/CaffeineMC/sodium",
                "https://ko-fi.com/x"
            ]
        );
    }

    #[test]
    fn server_hits_keep_address_players_and_modpack() {
        let json = r#"{"project_id":"OgcNPs6n","slug":"aero-server","name":"Aero SMP","summary":"Create",
            "author":"printer","game_versions":["1.21.1","26.1"],"gallery":["https://cdn.modrinth.com/a.webp"],
            "minecraft_java_server":{"address":" play.aerosmp.com ","content":{"kind":"modpack","version_id":"GDELFL7Y","project_id":"rVuIYVBN"},
            "ping":{"data":{"players_online":12,"players_max":300}}}}"#;
        let hit = serde_json::from_str::<RawServerHit>(json)
            .unwrap()
            .into_hit()
            .unwrap();
        let server = hit.server.unwrap();
        assert_eq!(server.address, "play.aerosmp.com");
        assert_eq!(server.players_online, Some(12.0));
        assert_eq!(server.modpack_version.as_deref(), Some("GDELFL7Y"));
        // 整合包可以是另一个项目（Modrinth 上 40 个热门服务器里有 3 个这样）
        assert_eq!(server.modpack_project.as_deref(), Some("rVuIYVBN"));
        assert_eq!(server.game_versions, ["26.1", "1.21.1"]);
        assert_eq!(
            hit.page_url.as_deref(),
            Some("https://modrinth.com/project/OgcNPs6n")
        );

        // 没有地址的服务器不要
        let no_address = r#"{"project_id":"x","name":"X","minecraft_java_server":{"address":""}}"#;
        assert!(
            serde_json::from_str::<RawServerHit>(no_address)
                .unwrap()
                .into_hit()
                .is_none()
        );
    }

    #[test]
    fn owner_is_preferred_over_other_members() {
        let members: Vec<RawMember> = serde_json::from_str(
            r#"[{"user":{"username":"helper"},"role":"Member","ordering":0},
                {"user":{"username":"boss"},"role":"Owner","ordering":3}]"#,
        )
        .unwrap();
        let members = members_of(members);
        assert_eq!(members[0].name, "boss");
        assert_eq!(members[0].role.as_deref(), Some("Owner"));
        assert_eq!(
            members[0].url.as_deref(),
            Some("https://modrinth.com/user/boss")
        );
        assert_eq!(members.len(), 2);
        assert!(members_of(Vec::new()).is_empty());
        // 组织的项目没有所有者：项目负责人排在维护者前面（Sodium 就是这样）
        let org: Vec<RawMember> = serde_json::from_str(
            r#"[{"user":{"username":"IMS"},"role":"Maintainer","ordering":0},
                {"user":{"username":"jellysquid3"},"role":"Project Lead","ordering":1}]"#,
        )
        .unwrap();
        assert_eq!(members_of(org)[0].name, "jellysquid3");
    }

    #[test]
    fn search_hit_takes_featured_gallery_and_author_page() {
        let json = r#"{"project_id":"x","slug":"x","title":"X","author":"bad name",
            "featured_gallery":"","gallery":["https://cdn.modrinth.com/g.png"],"date_modified":"2026-11-11T00:00:00Z"}"#;
        let hit: ProjectHit = serde_json::from_str::<RawHit>(json).unwrap().into();
        assert_eq!(
            hit.gallery_url.as_deref(),
            Some("https://cdn.modrinth.com/g.png")
        );
        // 用户名里有空格：不像正常值，不给主页地址
        assert_eq!(hit.author_url, None);
        assert_eq!(hit.updated.as_deref(), Some("2026-11-11T00:00:00Z"));
    }

    #[test]
    fn prefers_the_newest_release() {
        let picked = best_version(vec![
            version("3-beta", "beta"),
            version("2", "release"),
            version("1", "release"),
        ]);
        assert_eq!(picked.unwrap().id, "2");
        let only_beta = best_version(vec![version("3-beta", "beta"), version("2-alpha", "alpha")]);
        assert_eq!(only_beta.unwrap().id, "3-beta");
        assert!(best_version(Vec::new()).is_none());
    }

    #[test]
    fn tells_which_side_a_project_belongs_to() {
        assert_eq!(
            side_of("required", "unsupported"),
            Some(Side::Client),
            "Sodium"
        );
        assert_eq!(side_of("unsupported", "required"), Some(Side::Server));
        assert_eq!(
            side_of("required", "required"),
            Some(Side::Both),
            "Fabric API"
        );
        assert_eq!(
            side_of("optional", "optional"),
            Some(Side::Either),
            "FerriteCore"
        );
        assert_eq!(side_of("required", "optional"), Some(Side::Either));
        assert_eq!(side_of("unknown", "unknown"), None);
        assert_eq!(side_of("", ""), None, "字段缺了不乱猜");
    }

    #[test]
    fn builds_project_pages_only_from_sane_values() {
        assert_eq!(
            modrinth_page("mod", "sodium").as_deref(),
            Some("https://modrinth.com/mod/sodium")
        );
        assert_eq!(
            modrinth_page("shader", "complementary-reimagined").as_deref(),
            Some("https://modrinth.com/shader/complementary-reimagined")
        );
        assert_eq!(modrinth_page("mod", "../evil"), None);
        assert_eq!(modrinth_page("mod", "a?b=c"), None);
        assert_eq!(modrinth_page("unknown", "sodium"), None);
        assert_eq!(modrinth_page("mod", ""), None);
    }

    #[test]
    fn encodes_query_parameters() {
        assert_eq!(encode("sodium extra"), "sodium%20extra");
        assert_eq!(encode("[\"a\"]"), "%5B%22a%22%5D");
        assert_eq!(encode("光影"), "%E5%85%89%E5%BD%B1");
    }

    #[test]
    fn search_url_carries_sort_and_category() {
        let decode = |url: String| {
            url.replace("%5B", "[")
                .replace("%5D", "]")
                .replace("%22", "\"")
                .replace("%3A", ":")
                .replace("%2C", ",")
        };
        let mut facets = vec![vec!["project_type:mod".to_owned()]];
        push_category(&mut facets, Some("technology"));
        push_category(&mut facets, Some("bad\" category"));
        let url = decode(
            search_url("https://x/search", &facets, "", SearchSort::Updated, 40, 20).unwrap(),
        );
        assert!(url.contains("index=updated"), "{url}");
        assert!(url.contains("offset=40"), "{url}");
        assert!(
            url.contains(r#"[["project_type:mod"],["categories:technology"]]"#),
            "分类是单独一组（并且），不像分类名的不要：{url}"
        );
        // 「相关性」没有关键词时按下载量，有关键词时按相关度
        let empty = search_url("https://x", &facets, " ", SearchSort::Relevance, 0, 20).unwrap();
        assert!(empty.contains("index=downloads"));
        let text = search_url("https://x", &facets, "jei", SearchSort::Relevance, 0, 20).unwrap();
        assert!(text.contains("index=relevance"));
        assert!(modrinth_category("512x+").is_some());
        assert!(modrinth_category("8x-").is_some());
        assert!(modrinth_category("Tech").is_none());
    }

    #[test]
    fn curseforge_sort_fields_match_the_website() {
        assert_eq!(SearchSort::Relevance.curseforge_field(""), 6);
        assert_eq!(SearchSort::Relevance.curseforge_field("create"), 1);
        assert_eq!(SearchSort::Downloads.curseforge_field("x"), 6);
        assert_eq!(SearchSort::Updated.curseforge_field(""), 3);
        assert_eq!(SearchSort::Newest.curseforge_field(""), 11);
    }

    #[test]
    fn parses_real_search_hits() {
        let json = r#"{"hits":[{"project_id":"AANobbMI","project_type":"mod","slug":"sodium","author":"jellysquid3",
            "title":"Sodium","description":"fast","categories":["fabric"],"display_categories":["optimization"],
            "versions":["1.21.1"],"downloads":100,"follows":5,"icon_url":"https://cdn.modrinth.com/i.png"}],
            "offset":0,"limit":20,"total_hits":1}"#;
        let response: SearchResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.hits[0].project_id, "AANobbMI");
        assert_eq!(
            response.hits[0].icon_url.as_deref(),
            Some("https://cdn.modrinth.com/i.png")
        );
        assert_eq!(response.total_hits, 1);
    }

    #[test]
    fn primary_file_falls_back_to_the_first() {
        let file = |name: &str, primary: bool| VersionFile {
            url: String::new(),
            filename: name.to_owned(),
            primary,
            size: 0,
            hashes: HashMap::new(),
        };
        let mut v = version("1", "release");
        v.files = vec![file("a.jar", false), file("b.jar", true)];
        assert_eq!(v.primary_file().unwrap().filename, "b.jar");
        v.files = vec![file("a.jar", false)];
        assert_eq!(v.primary_file().unwrap().filename, "a.jar");
    }
}
