//! 用真实的 1.21.1 版本描述文件验证解析和 rules 判定。

// 测试代码允许直接 unwrap（clippy 不把测试文件里的辅助函数算作测试）
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;

use launcher_core::meta::{Environment, VersionJson, rules_allow};

fn windows_x64() -> Environment {
    Environment {
        os_name: "windows".into(),
        os_arch: "x86_64".into(),
        os_version: "10.0".into(),
        features: BTreeMap::new(),
    }
}

fn load() -> VersionJson {
    VersionJson::parse("1.21.1", include_bytes!("fixtures/1.21.1.json")).unwrap()
}

#[test]
fn parses_the_key_fields() {
    let v = load();
    assert_eq!(v.id, "1.21.1");
    assert_eq!(v.kind.as_deref(), Some("release"));
    assert_eq!(
        v.main_class.as_deref(),
        Some("net.minecraft.client.main.Main")
    );
    assert_eq!(v.required_java(), 21);
    assert_eq!(
        v.java_version.as_ref().and_then(|j| j.component.as_deref()),
        Some("java-runtime-delta")
    );
    assert_eq!(v.asset_index.as_ref().unwrap().id, "17");
    assert_eq!(v.libraries.len(), 97);
    let client = v
        .downloads
        .as_ref()
        .and_then(|d| d.client.as_ref())
        .unwrap();
    assert!(client.sha1.is_some() && client.size.is_some());
    let logging = v.logging.as_ref().and_then(|l| l.client.as_ref()).unwrap();
    assert_eq!(logging.file.id, "client-1.12.xml");
}

#[test]
fn windows_gets_only_windows_natives() {
    let v = load();
    let env = windows_x64();
    let allowed: Vec<&str> = v
        .libraries
        .iter()
        .filter(|l| rules_allow(l.rules.as_deref(), &env))
        .map(|l| l.name.as_str())
        .collect();

    assert!(
        allowed.len() < v.libraries.len(),
        "其他系统的库应该被过滤掉"
    );
    assert!(allowed.iter().any(|n| n.ends_with(":natives-windows")));
    assert!(
        !allowed
            .iter()
            .any(|n| n.contains("natives-linux") || n.contains("natives-macos"))
    );
    assert!(
        !allowed
            .iter()
            .any(|n| n.starts_with("ca.weblite:java-objc-bridge")),
        "这是 macOS 专用的"
    );
}

#[test]
fn every_windows_library_can_be_downloaded_and_verified() {
    let v = load();
    let env = windows_x64();
    for lib in v
        .libraries
        .iter()
        .filter(|l| rules_allow(l.rules.as_deref(), &env))
    {
        let artifact = lib
            .downloads
            .as_ref()
            .and_then(|d| d.artifact.as_ref())
            .unwrap_or_else(|| panic!("{} 缺少下载信息", lib.name));
        assert!(
            artifact.url.starts_with("https://libraries.minecraft.net/"),
            "{}",
            artifact.url
        );
        assert!(artifact.sha1.is_some(), "{} 缺少 SHA-1", lib.name);
        let coord = lib
            .coord()
            .unwrap_or_else(|| panic!("{} 坐标解析失败", lib.name));
        assert_eq!(
            artifact.path.as_deref(),
            Some(coord.path().as_str()),
            "Maven 路径应该和官方给的一致"
        );
    }
}

#[test]
fn round_trips_through_serde() {
    let v = load();
    let again: VersionJson = serde_json::from_str(&serde_json::to_string(&v).unwrap()).unwrap();
    assert_eq!(again, v);
}
