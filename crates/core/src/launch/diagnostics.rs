//! 从同一次启动的记录判断常见问题，并导出经过脱敏的本地诊断包。

use std::fs::File;
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};
use std::path::Path;

use serde::Serialize;
use serde_json::json;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

use super::record::{LaunchOutcome, LaunchRecord, QuickPlayRecord, record_file, records_dir};
use crate::redact::redact;

const LOG_LIMIT: u64 = 2 * 1024 * 1024;
const REPORT_LIMIT: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum CrashKind {
    Java,
    Memory,
    Dependency,
    Conflict,
    Files,
    Unknown,
}

pub fn classify(text: &str, error_code: Option<&str>) -> CrashKind {
    let text = text.to_ascii_lowercase();
    let has = |signals: &[&str]| signals.iter().any(|signal| text.contains(signal));
    if error_code == Some("java.missing")
        || text
            .lines()
            .any(|line| line.contains("hard_dep") && line.contains("depends java @"))
        || has(&[
            "unsupportedclassversionerror",
            "unsupported class file major version",
            "compiled by a more recent version of the java runtime",
            "could not create the java virtual machine: java version",
        ])
    {
        CrashKind::Java
    } else if has(&[
        "outofmemoryerror",
        "could not reserve enough space for",
        "insufficient memory for the java runtime environment",
        "native memory allocation (malloc) failed",
        "gc overhead limit exceeded",
    ]) {
        CrashKind::Memory
    } else if has(&[
        "hard_dep_no_candidate",
        "missing or unsupported mandatory dependencies",
        "missing mandatory dependencies",
        "missingmods exception",
        "missingmodsexception",
        "missing dependencies:",
        "which is missing!",
        "which is missing.",
        "but it is missing",
        "但没有安装它",
    ]) && (text.contains("mod")
        || text.contains("模组")
        || text.contains("mandatory dependencies"))
    {
        CrashKind::Dependency
    } else if has(&[
        "neg_hard_dep",
        "hard_break",
        "duplicatemodsfoundexception",
        "found duplicate mods",
        "duplicate mod id",
        "conflicts with",
        "incompatible with",
        "some of your mods are incompatible",
        "但存在匹配的版本",
        "模组冲突",
    ]) && (text.contains("mod") || text.contains("模组"))
    {
        CrashKind::Conflict
    } else if error_code == Some("install.files_invalid")
        || has(&[
            "invalid or corrupt jarfile",
            "could not find or load main class net.minecraft.client.main.main",
        ])
        || (has(&[
            "zip end header not found",
            "invalid loc header",
            "zip file is empty",
        ]) && text
            .lines()
            .any(|line| line.contains(".jar") && has_game_file_path(line)))
    {
        CrashKind::Files
    } else {
        CrashKind::Unknown
    }
}

fn has_game_file_path(line: &str) -> bool {
    ["libraries", "versions", "assets"].iter().any(|folder| {
        line.contains(&format!("/{folder}/")) || line.contains(&format!("\\{folder}\\"))
    })
}

fn open_record_file(dir: &Path, id: &str, suffix: &str) -> io::Result<File> {
    let base = records_dir(dir);
    let file = record_file(&base, id, suffix).map_err(io::Error::other)?;
    let root = dir.canonicalize()?;
    let base = base.canonicalize()?;
    let file = file.canonicalize()?;
    if !base.starts_with(root) || file.parent() != Some(&base) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "启动记录路径超出实例",
        ));
    }
    File::open(file)
}

fn read_record(dir: &Path, id: &str) -> io::Result<LaunchRecord> {
    let file = open_record_file(dir, id, ".json")?;
    let record: LaunchRecord =
        serde_json::from_reader(file.take(REPORT_LIMIT)).map_err(io::Error::other)?;
    if record.id != id {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "启动记录编号不一致",
        ));
    }
    Ok(record)
}

fn read_text(
    dir: &Path,
    id: &str,
    suffix: &str,
    limit: u64,
    tail: bool,
) -> io::Result<Option<String>> {
    let mut file = match open_record_file(dir, id, suffix) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let size = file.metadata()?.len();
    let clipped = size > limit;
    if tail && clipped {
        file.seek(SeekFrom::Start(size - limit))?;
    }
    let mut bytes = Vec::new();
    file.take(limit).read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    let text = match (clipped, tail) {
        (true, true) => {
            let rest = text.split_once('\n').map_or("", |(_, rest)| rest);
            format!("[只保留最近的部分输出]\n{rest}")
        }
        (true, false) => {
            let prefix = text.rsplit_once('\n').map_or("", |(prefix, _)| prefix);
            format!("{prefix}\n[崩溃报告超出导出大小限制]")
        }
        _ => text.into_owned(),
    };
    Ok(Some(text))
}

fn reason(record: &LaunchRecord, log: Option<&str>, report: Option<&str>) -> CrashKind {
    if !matches!(
        record.outcome,
        LaunchOutcome::Failed | LaunchOutcome::Crashed
    ) {
        return CrashKind::Unknown;
    }
    let text = [
        record.error.as_deref().unwrap_or_default(),
        report.unwrap_or_default(),
        log.unwrap_or_default(),
    ]
    .join("\n");
    classify(&text, record.error_code.as_deref())
}

pub fn diagnose(dir: &Path, id: &str) -> io::Result<CrashKind> {
    let record = read_record(dir, id)?;
    let log = read_text(dir, id, ".log", LOG_LIMIT, true)?;
    let report = read_text(dir, id, ".crash.txt", REPORT_LIMIT, false)?;
    Ok(reason(&record, log.as_deref(), report.as_deref()))
}

/// 只打包所选启动的摘要和输出。存档、模组文件、账户和配置不参与读取。
pub fn export(dir: &Path, id: &str, home: Option<&Path>) -> io::Result<Vec<u8>> {
    let record = read_record(dir, id)?;
    let log = read_text(dir, id, ".log", LOG_LIMIT, true)?;
    let report = read_text(dir, id, ".crash.txt", REPORT_LIMIT, false)?;
    let kind = reason(&record, log.as_deref(), report.as_deref());
    let server = match &record.quick_play {
        Some(QuickPlayRecord::Server { address }) => Some(address.as_str()),
        _ => None,
    };
    let sanitize = |text: &str| sanitize(text, dir, home, server);
    let mods: Vec<_> = record
        .mods
        .iter()
        .map(|item| {
            json!({
                "title": sanitize(&item.title),
                "version": item.version.as_deref().map(sanitize),
                "fileName": sanitize(&item.file_name),
                "enabled": item.enabled,
                "source": item.source,
            })
        })
        .collect();
    let summary = json!({
        "launcherVersion": env!("CARGO_PKG_VERSION"),
        "startedAt": record.started_at,
        "endedAt": record.ended_at,
        "gameVersion": sanitize(&record.game_version),
        "loader": record.loader.as_deref().map(sanitize),
        "javaVersion": record.java_version.as_deref().map(sanitize),
        "memoryMb": record.memory_mb,
        "outcome": record.outcome,
        "exitCode": record.exit_code,
        "reachedWorld": record.reached_world,
        "error": record.error.as_deref().map(sanitize),
        "errorCode": record.error_code.as_deref().map(sanitize),
        "diagnosis": kind,
        "mods": mods,
    });
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    zip.start_file("record.json", options)
        .map_err(io::Error::other)?;
    serde_json::to_writer_pretty(&mut zip, &summary).map_err(io::Error::other)?;
    if let Some(text) = log {
        zip.start_file("game.log", options)
            .map_err(io::Error::other)?;
        zip.write_all(sanitize(&text).as_bytes())?;
    }
    if let Some(text) = report {
        zip.start_file("crash-report.txt", options)
            .map_err(io::Error::other)?;
        zip.write_all(sanitize(&text).as_bytes())?;
    }
    Ok(zip.finish().map_err(io::Error::other)?.into_inner())
}

fn sanitize(text: &str, dir: &Path, home: Option<&Path>, server: Option<&str>) -> String {
    let mut text = redact(text).into_owned();
    if text.contains("[CHAT]") {
        text = text
            .lines()
            .filter(|line| !line.contains("[CHAT]"))
            .collect::<Vec<_>>()
            .join("\n");
    }
    for (path, mask) in [(Some(dir), "<game-dir>"), (home, "<home>")] {
        if let Some(path) = path.and_then(Path::to_str).filter(|path| !path.is_empty()) {
            text = text.replace(path, mask);
            text = text.replace(&path.replace('\\', "/"), mask);
            text = text.replace(&path.replace('\\', "\\\\"), mask);
        }
    }
    if let Some(server) = server.filter(|server| !server.is_empty()) {
        text = text.replace(server, "<server>");
    }
    text
}
