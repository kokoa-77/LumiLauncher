//! 项目详情（资源中心点进一个项目）：介绍、图片、链接。
//!
//! 两个平台各自的接口在 [`super::modrinth::project_detail`]、[`super::curseforge::CurseForge::project_detail`]，
//! 这里只放两边共用的形状。介绍原文交给界面渲染：Modrinth 是 Markdown，CurseForge 是 HTML，
//! 界面渲染前必须先清洗（只留排版标签，去掉脚本、内嵌框架和事件属性）。

use serde::Serialize;

use super::modrinth::ProjectHit;
use super::{ContentSource, Delivery};

/// 版本发布通道（正式版 / 测试版 / 预览版）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum ReleaseType {
    Release,
    Beta,
    Alpha,
}

/// 项目的一个可安装版本（项目「版本」页签与实例「切换版本」共用）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct ContentVersion {
    pub id: String,
    pub project_id: String,
    pub source: ContentSource,
    pub name: String,
    pub version_number: String,
    pub release_type: ReleaseType,
    pub game_versions: Vec<String>,
    pub loaders: Vec<String>,
    pub published: Option<String>,
    pub downloads: Option<f64>,
    pub changelog: Option<String>,
    pub changelog_format: BodyFormat,
    pub file_name: String,
    pub file_size: f64,
    pub delivery: Delivery,
    pub page_url: Option<String>,
}

/// 介绍原文的格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum BodyFormat {
    Markdown,
    Html,
}

/// 作者放的一张图。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct GalleryImage {
    /// 大图
    pub url: String,
    /// 缩略图（列表里用，平台没给就和大图一样）
    pub thumb_url: String,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum LinkKind {
    Source,
    Issues,
    Wiki,
    Discord,
    Website,
    Donate,
}

/// 项目的外部链接（源码、问题反馈……），用系统浏览器打开。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct ProjectLink {
    pub kind: LinkKind,
    pub url: String,
}

/// 作者团队里的一个人（CurseForge 只有作者名单，没有角色）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct ProjectMember {
    pub name: String,
    /// 角色（Owner、Maintainer……，平台原样给的）
    pub role: Option<String>,
    pub avatar_url: Option<String>,
    /// 个人主页
    pub url: Option<String>,
}

/// 一个项目的详情。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct ProjectDetail {
    /// 和搜索结果同一个形状（标题、图标、作者、下载量……），详情页的页头直接用
    pub hit: ProjectHit,
    /// 介绍原文（还没清洗，见模块说明）
    pub body: String,
    pub body_format: BodyFormat,
    pub gallery: Vec<GalleryImage>,
    pub links: Vec<ProjectLink>,
    /// 许可证（例如「MIT」「保留所有权利」）
    pub license: Option<String>,
    /// 第一次发布（ISO 8601）
    pub published: Option<String>,
    /// 关注人数（Modrinth 才有）
    pub followers: Option<f64>,
    /// 支持的游戏版本：只留正式版（1.21.1、26.3 这种，快照不要），从新到旧
    pub game_versions: Vec<String>,
    /// 支持的加载器（小写：neoforge、fabric、forge、quilt……）
    pub loaders: Vec<String>,
    /// 作者团队（项目所有者在前）
    pub members: Vec<ProjectMember>,
}

/// 正式版的版本号（数字和点，至少两段），拆成数字比大小
pub(crate) fn version_key(version: &str) -> Option<Vec<u32>> {
    let parts: Vec<u32> = version
        .split('.')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    (parts.len() >= 2).then_some(parts)
}

/// 支持的游戏版本：去掉快照和预发布、去重，按版本号从新到旧排
pub fn release_versions(versions: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out: Vec<(Vec<u32>, String)> = versions
        .into_iter()
        .filter_map(|v| version_key(&v).map(|k| (k, v)))
        .collect();
    out.sort_by(|a, b| b.0.cmp(&a.0));
    out.dedup_by(|a, b| a.0 == b.0);
    out.into_iter().map(|(_, v)| v).collect()
}

/// 只留 `https://` 开头的地址：界面会把它放进图片或交给系统浏览器，别的协议一律不要。
pub fn https_only(url: String) -> Option<String> {
    url.starts_with("https://").then_some(url)
}

/// 把一组 (类型, 地址) 变成链接列表：空的、不是 https 的、重复的都去掉。
pub fn links_of(raw: impl IntoIterator<Item = (LinkKind, Option<String>)>) -> Vec<ProjectLink> {
    let mut out: Vec<ProjectLink> = Vec::new();
    for (kind, url) in raw {
        let Some(url) = url.map(|u| u.trim().to_owned()).and_then(https_only) else {
            continue;
        };
        if !out.iter().any(|l| l.url == url) {
            out.push(ProjectLink { kind, url });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_versions_drop_snapshots_and_sort_newest_first() {
        let versions = release_versions(
            [
                "1.20.1",
                "24w14a",
                "1.21.1",
                "26.3",
                "1.21.1",
                "1.21",
                "1.21.2-pre1",
                "1.9.4",
            ]
            .map(String::from),
        );
        assert_eq!(versions, ["26.3", "1.21.1", "1.21", "1.20.1", "1.9.4"]);
    }

    #[test]
    fn only_https_links_are_kept() {
        assert_eq!(
            https_only("https://a.example/x".into()).as_deref(),
            Some("https://a.example/x")
        );
        assert_eq!(https_only("http://a.example/x".into()), None);
        assert_eq!(https_only("javascript:alert(1)".into()), None);
    }

    #[test]
    fn links_drop_empty_unsafe_and_duplicates() {
        let links = links_of([
            (LinkKind::Source, Some("https://github.com/a/b".into())),
            (LinkKind::Issues, Some("https://github.com/a/b".into())),
            (LinkKind::Wiki, Some(String::new())),
            (LinkKind::Discord, Some("http://discord.gg/x".into())),
            (LinkKind::Website, None),
            (LinkKind::Donate, Some(" https://ko-fi.com/a ".into())),
        ]);
        assert_eq!(
            links,
            vec![
                ProjectLink {
                    kind: LinkKind::Source,
                    url: "https://github.com/a/b".into()
                },
                ProjectLink {
                    kind: LinkKind::Donate,
                    url: "https://ko-fi.com/a".into()
                },
            ]
        );
    }
}
