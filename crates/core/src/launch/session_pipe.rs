//! 给 Java 进程的游戏参数只经过本次启动的标准输入管道。

use std::io;
use std::path::Path;
use std::time::Duration;

use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::process::Child;
use tokio_util::sync::CancellationToken;

use super::args::LaunchArguments;
use crate::redact::Secret;

pub(super) const MAIN: &str = "com.lumilauncher.bootstrap.SessionPipe";
const CLASS: &[u8] = include_bytes!("../../data/java/SessionPipe.class");
const MAX_ARGUMENTS: usize = 4096;
const MAX_BYTES: usize = 1024 * 1024;
const HANDOFF_TIMEOUT: Duration = Duration::from_secs(15);

/// Java 从此目录加载 Lumi 自己的类。目录只含发布时嵌入的 class，不写游戏参数。
pub(super) fn stage() -> io::Result<TempDir> {
    let dir = tempfile::Builder::new().prefix("lumi-game-").tempdir()?;
    let package = dir.path().join("com/lumilauncher/bootstrap");
    std::fs::create_dir_all(&package)?;
    std::fs::write(package.join("SessionPipe.class"), CLASS)?;
    Ok(dir)
}

/// 最后一个显式类路径是 Java 实际使用的类路径。加入引导类后保留原条目顺序。
pub(super) fn jvm_args(launch: &LaunchArguments, bootstrap: &Path) -> Vec<String> {
    let args = &launch.jvm;
    let mut result = args.clone();
    let mut options = args.iter().enumerate().rev();
    let bridge = bootstrap.to_string_lossy();
    let separator = if cfg!(windows) { ';' } else { ':' };
    if let Some((index, flag)) = options.find(|(_, arg)| {
        matches!(arg.as_str(), "-cp" | "-classpath" | "--class-path")
            || arg.starts_with("--class-path=")
    }) {
        if let Some(classpath) = flag.strip_prefix("--class-path=") {
            result[index] = format!("--class-path={bridge}{separator}{classpath}");
        } else if let Some(classpath) = result.get_mut(index + 1) {
            *classpath = format!("{bridge}{separator}{classpath}");
        }
    } else {
        result.extend(["-cp".to_owned(), format!("{bridge}{separator}.")]);
    }
    if uses_legacy_forge_module(launch, separator) {
        result.push(
            "--add-exports=cpw.mods.bootstraplauncher/cpw.mods.bootstraplauncher=ALL-UNNAMED"
                .to_owned(),
        );
    }
    result.push(MAIN.to_owned());
    result
}

/// Forge 1.18.2 的 1.0.0 入口包没有导出，其他已支持的 BootstrapLauncher 已导出该包。
fn uses_legacy_forge_module(launch: &LaunchArguments, separator: char) -> bool {
    if launch.main != "cpw.mods.bootstraplauncher.BootstrapLauncher" {
        return false;
    }
    launch
        .jvm
        .iter()
        .enumerate()
        .filter_map(|(index, arg)| {
            if matches!(arg.as_str(), "-p" | "--module-path") {
                launch.jvm.get(index + 1).map(String::as_str)
            } else {
                arg.strip_prefix("--module-path=")
            }
        })
        .flat_map(|paths| paths.split(separator))
        .any(|path| {
            Path::new(path)
                .ends_with("cpw/mods/bootstraplauncher/1.0.0/bootstraplauncher-1.0.0.jar")
        })
}

pub(super) fn payload(launch: &LaunchArguments) -> io::Result<Secret<Vec<u8>>> {
    let args = launch.game.expose();
    let values = std::iter::once(&launch.main).chain(args);
    let size = values.clone().fold(4usize, |size, value| {
        size.saturating_add(4).saturating_add(value.len())
    });
    if args.len() > MAX_ARGUMENTS || size > MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "game arguments exceed the launch pipe limit",
        ));
    }
    let mut bytes = Vec::with_capacity(size);
    bytes.extend_from_slice(&(args.len() as u32).to_be_bytes());
    for value in values {
        bytes.extend_from_slice(&(value.len() as u32).to_be_bytes());
        bytes.extend_from_slice(value.as_bytes());
    }
    Ok(Secret::new(bytes))
}

pub(super) async fn send(
    child: &mut Child,
    payload: &Secret<Vec<u8>>,
    stop: &CancellationToken,
) -> io::Result<()> {
    let mut pipe = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("missing game launch pipe"))?;
    let write = async {
        pipe.write_all(payload.expose()).await?;
        pipe.shutdown().await
    };
    tokio::select! {
        biased;
        () = stop.cancelled() => Err(io::ErrorKind::Interrupted.into()),
        result = tokio::time::timeout(HANDOFF_TIMEOUT, write) => {
            result.map_err(|_| io::ErrorKind::TimedOut)?
        }
    }
}
