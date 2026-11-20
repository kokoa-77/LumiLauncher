//! 整理内存（D17h）：让后台程序把暂时不用的内存交出来，给游戏腾地方。
//!
//! **这是本项目唯一允许写 `unsafe` 的地方**（开发合同第 8 节第 9 条）：要调的 Windows 接口没有现成的
//! 安全封装。这里只做两件事，每个 `unsafe` 块只放一个调用，并写明为什么安全：
//!
//! 1. [`trim`]：在管理员权限下，让所有程序把工作集里暂时不用的内存交出来（系统的「清空工作集」），
//!    再把其中改过的写进页面文件（「写出已修改页面」）。这样任务管理器里的「可用」会变多。
//!    **不清待机内存**（系统的文件缓存）：它本来就算在「可用」里，清了数字不变，只会让切回别的程序更慢。
//!    代价：被整理的程序下次用到那部分内存时要从硬盘读回来，切回去的头一两秒会慢一点。
//! 2. [`run_elevated`]：用「以管理员身份运行」启动一个程序并等它结束（会弹出 Windows 的管理员确认）。
//!    启动器用它以管理员身份再开一次自己，只做 [`trim`] 这一件事就退出。

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrimError {
    /// 没有管理员权限
    NotAdmin,
    /// 系统接口返回了失败（NTSTATUS 或 Win32 错误码）
    Failed(i64),
    /// 不是 Windows
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ElevateError {
    /// 用户在 Windows 的管理员确认里点了「否」
    Cancelled,
    /// 启动失败（Win32 错误码）
    Failed(i64),
    /// 等太久还没结束
    Timeout,
    /// 不是 Windows
    Unsupported,
}

/// 把参数拼成 `ShellExecuteExW` 要的一行：带空格或引号的参数用双引号包起来。
/// 只用于启动器自己的固定参数（不含用户输入）。
pub fn join_args(args: &[&str]) -> String {
    args.iter()
        .map(|arg| {
            if arg.is_empty() || arg.contains([' ', '\t', '"']) {
                format!("\"{}\"", arg.replace('"', "\\\""))
            } else {
                (*arg).to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(windows)]
pub use imp::{maximize_process_window, run_elevated, set_process_window_title, trim};

#[cfg(not(windows))]
pub fn trim() -> Result<(), TrimError> {
    Err(TrimError::Unsupported)
}

#[cfg(not(windows))]
pub fn set_process_window_title(_pid: u32, _title: &str) -> bool {
    false
}

#[cfg(not(windows))]
pub fn maximize_process_window(_pid: u32) -> bool {
    false
}

#[cfg(not(windows))]
pub fn run_elevated(
    _exe: &std::path::Path,
    _args: &[&str],
    _timeout: std::time::Duration,
) -> Result<u32, ElevateError> {
    Err(ElevateError::Unsupported)
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::ptr::{null, null_mut};
    use std::time::Duration;

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_CANCELLED, ERROR_NOT_ALL_ASSIGNED, HANDLE, HWND, LPARAM, LUID,
        WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::Security::{
        AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED,
        TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetExitCodeProcess, OpenProcessToken, WaitForSingleObject,
    };
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, IsWindowVisible, SW_HIDE, SW_MAXIMIZE,
        SetWindowTextW, ShowWindow,
    };

    use super::{ElevateError, TrimError, join_args};

    /// `SystemMemoryListInformation`：系统内存列表的命令
    const SYSTEM_MEMORY_LIST_INFORMATION: i32 = 80;
    /// 清空所有进程的工作集
    const MEMORY_EMPTY_WORKING_SETS: i32 = 2;
    /// 把「已修改」的页面写进页面文件（之后它们变成待机，算进「可用」）
    const MEMORY_FLUSH_MODIFIED_LIST: i32 = 3;
    /// `STATUS_PRIVILEGE_NOT_HELD`
    const STATUS_PRIVILEGE_NOT_HELD: i32 = 0xC000_0061_u32 as i32;
    /// 内存列表命令要这个权限（管理员才有）
    const PROFILE_PRIVILEGE: &str = "SeProfileSingleProcessPrivilege";

    // windows-sys 没有收录这个函数（它在 ntdll 里，文档写在 Windows 驱动开发包里），自己声明。
    // raw-dylib：不需要 ntdll.lib 导入库
    // SAFETY: 签名和 Windows 驱动开发包文档里的一致（类别、指针、长度，返回 NTSTATUS）
    #[link(name = "ntdll", kind = "raw-dylib")]
    unsafe extern "system" {
        fn NtSetSystemInformation(class: i32, information: *const c_void, length: u32) -> i32;
    }

    /// 用完自动关闭的句柄
    struct Handle(HANDLE);

    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: 句柄是系统刚给我们的、还没关过，这里只关一次
                unsafe { CloseHandle(self.0) };
            }
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    fn last_error() -> i64 {
        i64::from(std::io::Error::last_os_error().raw_os_error().unwrap_or(0))
    }

    /// 打开当前进程令牌里的一个权限（管理员令牌里有，但默认是关着的）
    fn enable_privilege(name: &str) -> Result<(), TrimError> {
        let mut token: HANDLE = null_mut();
        // SAFETY: 无参数，返回当前进程的伪句柄（不需要关闭）
        let process = unsafe { GetCurrentProcess() };
        // SAFETY: process 是有效的伪句柄；token 指向一个本地变量，系统往里写句柄
        let opened =
            unsafe { OpenProcessToken(process, TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token) };
        if opened == 0 {
            return Err(TrimError::Failed(last_error()));
        }
        let token = Handle(token);

        let name = wide(name);
        let mut luid = LUID {
            LowPart: 0,
            HighPart: 0,
        };
        // SAFETY: name 以 0 结尾、在调用期间一直有效；luid 指向一个本地变量
        let found = unsafe { LookupPrivilegeValueW(null(), name.as_ptr(), &mut luid) };
        if found == 0 {
            return Err(TrimError::Failed(last_error()));
        }
        let privileges = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES {
                Luid: luid,
                Attributes: SE_PRIVILEGE_ENABLED,
            }],
        };
        // SAFETY: token 是刚打开的有效令牌；privileges 在调用期间有效、只被读取；不要旧值，后两个参数为空
        let adjusted =
            unsafe { AdjustTokenPrivileges(token.0, 0, &privileges, 0, null_mut(), null_mut()) };
        // 不是管理员时这个函数也返回成功，只是错误码是「没有全部分配」
        let error = last_error();
        if adjusted == 0 || error == i64::from(ERROR_NOT_ALL_ASSIGNED) {
            return Err(TrimError::NotAdmin);
        }
        Ok(())
    }

    fn memory_list_command(command: i32) -> Result<(), TrimError> {
        let length = u32::try_from(size_of::<i32>()).unwrap_or(4);
        // SAFETY: 第二个参数指向一个本地的 i32（系统要求的「内存列表命令」），长度如实给出，系统只读它
        let status = unsafe {
            NtSetSystemInformation(
                SYSTEM_MEMORY_LIST_INFORMATION,
                (&raw const command).cast(),
                length,
            )
        };
        match status {
            0.. => Ok(()),
            STATUS_PRIVILEGE_NOT_HELD => Err(TrimError::NotAdmin),
            other => Err(TrimError::Failed(i64::from(other))),
        }
    }

    /// 整理内存。要在管理员权限下运行（否则返回 [`TrimError::NotAdmin`]），耗时一两秒。
    pub fn trim() -> Result<(), TrimError> {
        enable_privilege(PROFILE_PRIVILEGE)?;
        memory_list_command(MEMORY_EMPTY_WORKING_SETS)?;
        memory_list_command(MEMORY_FLUSH_MODIFIED_LIST)
    }

    /// 以管理员身份运行 `exe`（会弹出 Windows 的管理员确认），不显示窗口，等它结束，返回它的退出码。
    pub fn run_elevated(exe: &Path, args: &[&str], timeout: Duration) -> Result<u32, ElevateError> {
        let file: Vec<u16> = exe.as_os_str().encode_wide().chain(Some(0)).collect();
        let verb = wide("runas");
        let parameters = wide(&join_args(args));
        let mut info = SHELLEXECUTEINFOW {
            cbSize: u32::try_from(size_of::<SHELLEXECUTEINFOW>()).unwrap_or(0),
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
            lpVerb: verb.as_ptr(),
            lpFile: file.as_ptr(),
            lpParameters: parameters.as_ptr(),
            nShow: SW_HIDE,
            ..Default::default()
        };
        // SAFETY: info 里的三个字符串都以 0 结尾、在调用期间一直有效，cbSize 如实；系统往 hProcess 里写句柄
        let ok = unsafe { ShellExecuteExW(&mut info) };
        if ok == 0 {
            let error = last_error();
            return Err(if error == i64::from(ERROR_CANCELLED) {
                ElevateError::Cancelled
            } else {
                ElevateError::Failed(error)
            });
        }
        if info.hProcess.is_null() {
            return Err(ElevateError::Failed(0));
        }
        let process = Handle(info.hProcess);
        let millis = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1);
        // SAFETY: process 是 ShellExecuteExW 刚给的有效进程句柄
        let waited = unsafe { WaitForSingleObject(process.0, millis) };
        match waited {
            WAIT_OBJECT_0 => {}
            WAIT_TIMEOUT => return Err(ElevateError::Timeout),
            _ => return Err(ElevateError::Failed(last_error())),
        }
        let mut code = 0u32;
        // SAFETY: process 有效；code 指向一个本地变量
        let got = unsafe { GetExitCodeProcess(process.0, &mut code) };
        if got == 0 {
            return Err(ElevateError::Failed(last_error()));
        }
        Ok(code)
    }

    struct TitleTarget {
        pid: u32,
        title: Vec<u16>,
        updated: bool,
    }

    unsafe extern "system" fn enum_window_set_title(hwnd: HWND, lparam: LPARAM) -> i32 {
        // SAFETY: lparam 由下方 set_process_window_title 同步传入，指向栈上有效的 TitleTarget，
        // 且 EnumWindows 在当前线程同步执行，不存在并发访问或悬垂指针。
        let target = unsafe { &mut *(lparam as *mut TitleTarget) };
        let mut window_pid = 0u32;
        // SAFETY: hwnd 是系统枚举传入的有效顶层窗口句柄，window_pid 是本地栈变量。
        unsafe { GetWindowThreadProcessId(hwnd, &mut window_pid) };
        if window_pid != target.pid {
            return 1;
        }
        // SAFETY: hwnd 是系统枚举传入的有效窗口句柄。
        if unsafe { IsWindowVisible(hwnd) } == 0 {
            return 1;
        }
        // SAFETY: hwnd 有效，target.title 是以 NUL 结尾的 UTF-16 缓冲区。
        if unsafe { SetWindowTextW(hwnd, target.title.as_ptr()) } != 0 {
            target.updated = true;
        }
        1
    }

    /// 设置指定进程可见顶层窗口的标题（D26c 自定义游戏窗口标题）。
    pub fn set_process_window_title(pid: u32, title: &str) -> bool {
        if pid == 0 || title.trim().is_empty() {
            return false;
        }
        let mut target = TitleTarget {
            pid,
            title: wide(title),
            updated: false,
        };
        let ptr = (&mut target as *mut TitleTarget) as LPARAM;
        // SAFETY: 回调在当前线程同步遍历顶层窗口，期间 target 始终在栈上有效。
        unsafe { EnumWindows(Some(enum_window_set_title), ptr) };
        target.updated
    }

    struct MaximizeTarget {
        pid: u32,
        maximized: bool,
    }

    unsafe extern "system" fn enum_window_maximize(hwnd: HWND, lparam: LPARAM) -> i32 {
        // SAFETY: lparam 由下方 maximize_process_window 同步传入，指向栈上有效的 MaximizeTarget。
        let target = unsafe { &mut *(lparam as *mut MaximizeTarget) };
        let mut window_pid = 0u32;
        // SAFETY: hwnd 是系统枚举传入的有效顶层窗口句柄，window_pid 是本地栈变量。
        unsafe { GetWindowThreadProcessId(hwnd, &mut window_pid) };
        if window_pid != target.pid {
            return 1;
        }
        // SAFETY: hwnd 是系统枚举传入的有效窗口句柄。
        if unsafe { IsWindowVisible(hwnd) } == 0 {
            return 1;
        }
        // SAFETY: hwnd 是有效的可见顶层窗口句柄，SW_MAXIMIZE 仅改变窗口显示状态。
        unsafe { ShowWindow(hwnd, SW_MAXIMIZE) };
        target.maximized = true;
        0
    }

    /// 最大化指定进程的首个可见顶层窗口（D26c 启动时最大化窗口模式）。
    pub fn maximize_process_window(pid: u32) -> bool {
        if pid == 0 {
            return false;
        }
        let mut target = MaximizeTarget {
            pid,
            maximized: false,
        };
        let ptr = (&mut target as *mut MaximizeTarget) as LPARAM;
        // SAFETY: 回调在当前线程同步遍历顶层窗口，期间 target 始终在栈上有效。
        unsafe { EnumWindows(Some(enum_window_maximize), ptr) };
        target.maximized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_fixed_arguments() {
        assert_eq!(join_args(&["--clean-memory"]), "--clean-memory");
        assert_eq!(join_args(&["--a", "b c"]), "--a \"b c\"");
        assert_eq!(join_args(&[""]), "\"\"");
        assert_eq!(join_args(&[]), "");
    }
}
