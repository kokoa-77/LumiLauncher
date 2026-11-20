//! 下载源改写：官方地址 → BMCLAPI 镜像地址（Minecraft）、MCIM 镜像地址（模组文件，D18b）。
//!
//! 替换表来自现有启动器源码和 BMCLAPI 文档（bmclapidoc.bangbang93.com），已对 1.21.1 的
//! 客户端、资源文件、库文件实测可用。以后做成可远程更新的配置，现在先放代码里。

use crate::config::DownloadSource;

const BMCLAPI: &str = "https://bmclapi2.bangbang93.com";

/// 官方地址前缀 → 镜像地址前缀
const BMCLAPI_RULES: &[(&str, &str)] = &[
    ("https://piston-meta.mojang.com", BMCLAPI),
    ("https://launchermeta.mojang.com", BMCLAPI),
    ("https://piston-data.mojang.com", BMCLAPI),
    ("https://launcher.mojang.com", BMCLAPI),
    (
        "https://resources.download.minecraft.net",
        "https://bmclapi2.bangbang93.com/assets",
    ),
    (
        "https://libraries.minecraft.net",
        "https://bmclapi2.bangbang93.com/maven",
    ),
    (
        "https://maven.minecraftforge.net",
        "https://bmclapi2.bangbang93.com/maven",
    ),
    (
        "https://maven.neoforged.net/releases",
        "https://bmclapi2.bangbang93.com/maven",
    ),
    (
        "https://meta.fabricmc.net",
        "https://bmclapi2.bangbang93.com/fabric-meta",
    ),
    (
        "https://maven.fabricmc.net",
        "https://bmclapi2.bangbang93.com/maven",
    ),
];

/// MCIM 的文件镜像（D18b，见 www.mcimirror.top/guide）：只换域名，路径不变
const MCIM_FILES: &str = "https://mod.mcimirror.top";
/// 模组文件的官方 CDN → MCIM。`mediafilez.forgecdn.net` 按 MCIM 的说明不换
const MCIM_FILE_HOSTS: &[(&str, ModPlatform)] = &[
    ("https://cdn.modrinth.com", ModPlatform::Modrinth),
    ("https://edge.forgecdn.net", ModPlatform::CurseForge),
];

/// 模组文件来自哪个平台（决定按哪一行下载源设置换镜像）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModPlatform {
    Modrinth,
    CurseForge,
}

/// 模组文件的 MCIM 地址；不是 Modrinth / CurseForge 官方 CDN 的返回 `None`
pub fn mod_file_mirror(url: &str) -> Option<(ModPlatform, String)> {
    MCIM_FILE_HOSTS.iter().find_map(|(official, platform)| {
        let rest = url.strip_prefix(official)?;
        rest.starts_with('/')
            .then(|| (*platform, format!("{MCIM_FILES}{rest}")))
    })
}

/// 把官方地址改写成镜像地址；不在替换表里的地址返回 `None`。
///
/// 前缀后面必须紧跟 `/` 或字符串结束，防止 `libraries.minecraft.net.evil.com` 这类地址被误改写。
pub fn to_mirror(url: &str) -> Option<String> {
    BMCLAPI_RULES.iter().find_map(|(official, mirror)| {
        let rest = url.strip_prefix(official)?;
        (rest.is_empty() || rest.starts_with('/')).then(|| format!("{mirror}{rest}"))
    })
}

/// 按下载源设置给出候选地址（按尝试顺序排列）。
///
/// - 官方优先：先官方，失败再镜像
/// - 国内镜像优先：先镜像，失败再官方
/// - 只用官方：不碰镜像（D18）
/// - 自动：应用里先由 [`super::SourcePicker`] 测速换成官方优先或镜像优先；没测过时按官方优先
pub fn candidate_urls(official: &str, source: DownloadSource) -> Vec<String> {
    match (to_mirror(official), source) {
        (Some(mirror), DownloadSource::Mirror) => vec![mirror, official.to_owned()],
        (Some(mirror), DownloadSource::Auto | DownloadSource::Official) => {
            vec![official.to_owned(), mirror]
        }
        (Some(_), DownloadSource::OfficialOnly) | (None, _) => vec![official.to_owned()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_known_official_hosts() {
        assert_eq!(
            to_mirror("https://piston-data.mojang.com/v1/objects/abc/client.jar").as_deref(),
            Some("https://bmclapi2.bangbang93.com/v1/objects/abc/client.jar")
        );
        assert_eq!(
            to_mirror("https://resources.download.minecraft.net/ab/abcdef").as_deref(),
            Some("https://bmclapi2.bangbang93.com/assets/ab/abcdef")
        );
        assert_eq!(
            to_mirror(
                "https://libraries.minecraft.net/com/mojang/brigadier/1.3.10/brigadier-1.3.10.jar"
            )
            .as_deref(),
            Some(
                "https://bmclapi2.bangbang93.com/maven/com/mojang/brigadier/1.3.10/brigadier-1.3.10.jar"
            )
        );
        assert_eq!(
            to_mirror("https://maven.neoforged.net/releases/net/neoforged/neoforge/x.jar")
                .as_deref(),
            Some("https://bmclapi2.bangbang93.com/maven/net/neoforged/neoforge/x.jar")
        );
    }

    #[test]
    fn leaves_unknown_and_look_alike_hosts_alone() {
        assert_eq!(to_mirror("https://cdn.modrinth.com/data/x.jar"), None);
        assert_eq!(
            to_mirror("https://libraries.minecraft.net.evil.com/x.jar"),
            None
        );
        assert_eq!(
            to_mirror("http://libraries.minecraft.net/x.jar"),
            None,
            "只改写 https"
        );
    }

    #[test]
    fn mod_files_only_change_the_host() {
        assert_eq!(
            mod_file_mirror("https://cdn.modrinth.com/data/AANobbMI/versions/x/sodium.jar"),
            Some((
                ModPlatform::Modrinth,
                "https://mod.mcimirror.top/data/AANobbMI/versions/x/sodium.jar".to_owned()
            ))
        );
        assert_eq!(
            mod_file_mirror("https://edge.forgecdn.net/files/1/2/jei.jar"),
            Some((
                ModPlatform::CurseForge,
                "https://mod.mcimirror.top/files/1/2/jei.jar".to_owned()
            ))
        );
        assert_eq!(
            mod_file_mirror("https://mediafilez.forgecdn.net/files/1/2/jei.jar"),
            None,
            "MCIM 说这个不换"
        );
        assert_eq!(mod_file_mirror("https://cdn.modrinth.com.evil.com/x"), None);
        assert_eq!(mod_file_mirror("https://github.com/a/b.jar"), None);
    }

    #[test]
    fn candidate_order_follows_the_setting() {
        let url = "https://libraries.minecraft.net/a.jar";
        let mirror = "https://bmclapi2.bangbang93.com/maven/a.jar";

        assert_eq!(candidate_urls(url, DownloadSource::Auto), [url, mirror]);
        assert_eq!(candidate_urls(url, DownloadSource::Official), [url, mirror]);
        assert_eq!(candidate_urls(url, DownloadSource::Mirror), [mirror, url]);
        assert_eq!(candidate_urls(url, DownloadSource::OfficialOnly), [url]);
        assert_eq!(
            candidate_urls("https://cdn.modrinth.com/x", DownloadSource::Mirror),
            ["https://cdn.modrinth.com/x"]
        );
    }
}
