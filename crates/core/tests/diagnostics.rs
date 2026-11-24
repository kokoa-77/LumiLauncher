use std::io::{Cursor, Read};

use launcher_core::launch::diagnostics::{CrashKind, classify, diagnose, export};
use launcher_core::launch::{
    LaunchOutcome, LaunchRecord, LogLevel, LogLine, QuickPlayRecord, RecordEnd, RecordWriter,
};

#[test]
fn recognizes_five_failures_and_keeps_an_unknown_fallback() {
    for (text, expected) in [
        (
            "java.lang.UnsupportedClassVersionError: loader was compiled by a more recent version",
            CrashKind::Java,
        ),
        (
            "java.lang.OutOfMemoryError: Java heap space",
            CrashKind::Memory,
        ),
        (
            "Mod 'Example' requires any version of core, which is missing!",
            CrashKind::Dependency,
        ),
        (
            "Incompatible mods found!\n[HARD_DEP_NO_CANDIDATE mod_needs depends mod_missing]\n模组 mod_needs 需要 mod_missing，但没有安装它！",
            CrashKind::Dependency,
        ),
        (
            "Mod 'Example' is incompatible with any version of other",
            CrashKind::Conflict,
        ),
        (
            "Error: Could not find or load main class net.minecraft.client.main.Main",
            CrashKind::Files,
        ),
        (
            "Failed to load; caused by java.util.zip.ZipException: zip END header not found in mods/example.jar",
            CrashKind::Unknown,
        ),
        (
            "java.lang.IllegalStateException: Something went wrong",
            CrashKind::Unknown,
        ),
    ] {
        assert_eq!(classify(text, None), expected, "{text}");
    }
    assert_eq!(
        classify("下载 Java 失败", Some("java.download_failed")),
        CrashKind::Unknown
    );
    assert_eq!(
        classify("找不到可用的 Java", Some("java.missing")),
        CrashKind::Java
    );
    assert_eq!(
        classify("描述文件格式不对", Some("install.files_invalid")),
        CrashKind::Files
    );
}

#[test]
fn exports_only_the_chosen_record_with_no_launcher_secrets() {
    let root = tempfile::tempdir().unwrap();
    let record = LaunchRecord::start(
        1.0,
        "1.21.1".into(),
        Some("Fabric 0.19.0".into()),
        Some(QuickPlayRecord::Server {
            address: "private-server.invalid".into(),
        }),
        &[],
    );
    let mut writer = RecordWriter::begin(root.path(), record).unwrap();
    writer.set_launch(&root.path().join("bin/java.exe"), "21.0.1", 4096);
    writer.append(&[
        LogLine {
            level: LogLevel::Error,
            thread: None,
            message: "java.lang.OutOfMemoryError: Java heap space".into(),
        },
        LogLine {
            level: LogLevel::Info,
            thread: None,
            message: r#"Authorization: XBL3.0 x=priv-user;priv-xbl"#.into(),
        },
        LogLine {
            level: LogLevel::Info,
            thread: None,
            message: r#"[--accessToken, priv-token, --username, test-player]"#.into(),
        },
        LogLine {
            level: LogLevel::Info,
            thread: None,
            message: r#"{"accessToken":"part\"priv-json"}"#.into(),
        },
        LogLine {
            level: LogLevel::Info,
            thread: None,
            message: format!(
                "Log at {}/test.log\n[CHAT] private-message",
                root.path().display()
            ),
        },
    ]);
    let record = writer.finish(RecordEnd {
        ended_at: 2.0,
        outcome: LaunchOutcome::Crashed,
        exit_code: Some(1),
        reached_world: false,
        error: None,
        crash_report: None,
    });
    assert_eq!(
        diagnose(root.path(), &record.id).unwrap(),
        CrashKind::Memory
    );
    assert!(export(root.path(), "../untrusted", None).is_err());
    let bytes = export(root.path(), &record.id, None).unwrap();
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    assert_eq!(zip.len(), 2);
    let mut summary = String::new();
    zip.by_name("record.json")
        .unwrap()
        .read_to_string(&mut summary)
        .unwrap();
    let mut log = String::new();
    zip.by_name("game.log")
        .unwrap()
        .read_to_string(&mut log)
        .unwrap();
    let data = format!("{summary}\n{log}");
    assert!(data.contains("OutOfMemoryError") && data.contains("4096"));
    for private in [
        "private-server.invalid",
        "priv-user",
        "priv-xbl",
        "priv-token",
        "priv-json",
        "private-message",
        &root.path().display().to_string(),
    ] {
        assert!(!data.contains(private), "诊断包含有 {private}");
    }
}
