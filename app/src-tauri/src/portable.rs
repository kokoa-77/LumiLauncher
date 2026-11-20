//! 便携模式由程序旁的标记启用，所有应用数据走同一目录。

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

const MARKER: &str = "Lumi.portable";
const DATA_DIR: &str = "LumiData";
const LOCK_FILE: &str = ".launcher.lock";

#[derive(Debug)]
pub struct PortableProfile {
    data_dir: PathBuf,
    // 进程退出时由操作系统释放。保留锁文件，避免新进程锁到另一个文件。
    _lock: File,
}

impl PortableProfile {
    pub fn detect() -> io::Result<Option<Self>> {
        Self::open_next_to(&std::env::current_exe()?)
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    fn open_next_to(exe: &Path) -> io::Result<Option<Self>> {
        let folder = exe
            .parent()
            .filter(|folder| folder.is_absolute())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "找不到启动器所在目录"))?;
        match fs::symlink_metadata(folder.join(MARKER)) {
            Ok(marker) if marker.is_file() && !is_link(&marker) => {}
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Lumi.portable 必须是启动器旁的普通文件",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        }

        let data_dir = folder.join(DATA_DIR);
        let lock = lock_data_dir(&data_dir)?;
        Ok(Some(Self {
            data_dir,
            _lock: lock,
        }))
    }
}

fn lock_data_dir(data_dir: &Path) -> io::Result<File> {
    match fs::create_dir(data_dir) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let metadata = fs::symlink_metadata(data_dir)?;
    if !metadata.is_dir() || is_link(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "LumiData 必须是启动器旁的普通文件夹",
        ));
    }

    let lock_path = data_dir.join(LOCK_FILE);
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x1;
        const FILE_SHARE_WRITE: u32 = 0x2;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        // 锁持有期间可以尝试打开同一个文件，禁止删除它或跟随其重解析点。
        options
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let lock = options.open(lock_path)?;
    let metadata = lock.metadata()?;
    if !metadata.is_file() || is_link(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "LumiData 中的锁文件无效",
        ));
    }
    match lock.try_lock() {
        Ok(()) => Ok(lock),
        Err(TryLockError::WouldBlock) => Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "该文件夹里的 Lumi 便携版已经在运行",
        )),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        // Windows 目录联接也会把数据引向其他位置。
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::{fs, io};

    use super::PortableProfile;

    #[test]
    fn unmarked_executable_uses_installed_mode_without_creating_data() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            PortableProfile::open_next_to(&dir.path().join("Lumi.exe"))
                .unwrap()
                .is_none()
        );
        assert!(!dir.path().join("LumiData").exists());
    }

    #[test]
    fn marker_selects_data_next_to_executable() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Lumi.portable"), []).unwrap();
        let profile = PortableProfile::open_next_to(&dir.path().join("Lumi.exe"))
            .unwrap()
            .unwrap();
        let data_dir = dir.path().join("LumiData");
        assert_eq!(profile.data_dir(), data_dir);
        assert!(data_dir.is_dir());
    }

    #[test]
    fn marker_directory_is_a_configuration_error() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("Lumi.portable")).unwrap();
        assert_eq!(
            PortableProfile::open_next_to(&dir.path().join("Lumi.exe"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn blocked_data_directory_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Lumi.portable"), []).unwrap();
        fs::write(dir.path().join("LumiData"), b"existing file").unwrap();
        assert!(PortableProfile::open_next_to(&dir.path().join("Lumi.exe")).is_err());
        assert_eq!(
            fs::read(dir.path().join("LumiData")).unwrap(),
            b"existing file"
        );
    }

    #[test]
    fn one_process_at_a_time_can_open_the_same_portable_profile() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Lumi.portable"), []).unwrap();
        let exe = dir.path().join("Lumi.exe");
        let first = PortableProfile::open_next_to(&exe).unwrap().unwrap();
        assert_eq!(
            PortableProfile::open_next_to(&exe).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        drop(first);
        assert!(PortableProfile::open_next_to(&exe).unwrap().is_some());
    }
}
