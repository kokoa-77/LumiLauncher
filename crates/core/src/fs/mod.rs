//! 文件系统的两条安全底线：所有写入都是原子写，所有不可信路径都经过安全拼接。

pub mod atomic;
pub mod safe_path;

pub use atomic::{
    AtomicWriteError, ReadJsonError, is_win_lock_error, read_json, remove_with_win_retry,
    remove_with_win_retry_sync, rename_with_win_retry, rename_with_win_retry_sync, write_atomic,
    write_atomic_from, write_json_atomic,
};
pub use safe_path::{UnsafePath, safe_file_name, safe_join};
