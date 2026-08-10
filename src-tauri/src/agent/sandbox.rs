//! 操作系统级沙箱：通过 Windows AppContainer 将 bash 子进程限制在会话 files/ 目录内。
//!
//! 原理：
//! 1. 为每个会话创建一个 AppContainer profile（容器 SID）
//! 2. 将会话 files/ 目录的 DACL 合并授权给该容器 SID（含子对象继承）
//! 3. 以「当前进程令牌 + AppContainer 安全能力属性」启动 cmd 子进程
//! 4. 子进程在 OS 层面只能访问被授权（容器 SID 允许）的文件 —— 会话目录外
//!    的一切路径（C:\Users\...、messages.db 等）默认被系统拒绝，无论命令怎么写
//!
//! 安全策略：沙箱初始化失败时拒绝执行（fail-closed），绝不静默降级。

use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr;

use windows_sys::Win32::Foundation::{
    GENERIC_ALL, GetLastError, LocalFree, SetHandleInformation, CloseHandle, HANDLE,
    HANDLE_FLAG_INHERIT,
};
use windows_sys::Win32::Security::{
    FreeSid, ACL, PSID, DACL_SECURITY_INFORMATION, SUB_CONTAINERS_AND_OBJECTS_INHERIT,
    SECURITY_CAPABILITIES,
};
use windows_sys::Win32::Security::Authorization::{
    GetNamedSecurityInfoW, SetNamedSecurityInfoW, SetEntriesInAclW, EXPLICIT_ACCESS_W, TRUSTEE_W,
    SE_FILE_OBJECT, TRUSTEE_IS_SID, TRUSTEE_IS_USER, GRANT_ACCESS,
};
use windows_sys::Win32::Security::Isolation::{
    CreateAppContainerProfile, DeriveAppContainerSidFromAppContainerName,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::Storage::FileSystem::ReadFile;
use windows_sys::Win32::System::Threading::{
    PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, STARTF_USESTDHANDLES, STARTUPINFOEXW,
    STARTUPINFOW, PROCESS_INFORMATION, EXTENDED_STARTUPINFO_PRESENT, CREATE_NO_WINDOW,
    InitializeProcThreadAttributeList, UpdateProcThreadAttribute, CreateProcessW,
    WaitForSingleObject, TerminateProcess, GetExitCodeProcess,
};

fn to_wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

fn to_wide_path(p: &Path) -> Vec<u16> {
    p.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

fn win_err(op: &str, code: u32) -> String {
    format!("{op} 失败（Windows 错误码 0x{code:08X}）")
}

/// 会话 AppContainer 沙箱
pub struct ContainerSandbox {
    _name: String,
    sid: PSID,
}

// SID 指针仅在本沙箱单线程内使用（spawn_blocking 只在一个线程执行闭包）
unsafe impl Send for ContainerSandbox {}

impl Drop for ContainerSandbox {
    fn drop(&mut self) {
        unsafe {
            FreeSid(self.sid);
        }
    }
}

impl ContainerSandbox {
    /// 创建（或复用）会话容器
    pub fn for_session(session_id: i64) -> Result<Self, String> {
        let name = format!("ChatDeepSeek.Session.{session_id}");
        let display = format!("ChatDeepSeek Session {session_id}");
        let description = "ChatDeepSeek 会话隔离沙箱";
        let name_w = to_wide(&name);
        let display_w = to_wide(&display);
        let desc_w = to_wide(&description);
        let mut sid: PSID = ptr::null_mut();
        unsafe {
            // CreateAppContainerProfile 返回 HRESULT（0=S_OK；已存在=0x800700B7）
            let ret = CreateAppContainerProfile(
                name_w.as_ptr(),
                display_w.as_ptr(),
                desc_w.as_ptr(),
                ptr::null(),
                0,
                &mut sid,
            );
            if ret as u32 == 0x800700B7u32 {
                // ERROR_ALREADY_EXISTS：复用已有容器
                let mut derived: PSID = ptr::null_mut();
                let r2 = DeriveAppContainerSidFromAppContainerName(name_w.as_ptr(), &mut derived);
                if r2 != 0 {
                    return Err(win_err("DeriveAppContainerSid", r2 as u32));
                }
                sid = derived;
            } else if ret < 0 {
                return Err(win_err("CreateAppContainerProfile", ret as u32));
            }
        }
        Ok(ContainerSandbox { _name: name, sid })
    }

    /// 将会话 files/ 目录的 DACL 合并授权给容器 SID（含子对象继承）
    pub fn grant_access(&self, dir: &Path) -> Result<(), String> {
        let dir_w = to_wide_path(dir);
        unsafe {
            let mut sd: *mut std::ffi::c_void = ptr::null_mut();
            let mut dacl: *mut ACL = ptr::null_mut();
            let ret = GetNamedSecurityInfoW(
                dir_w.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut dacl,
                ptr::null_mut(),
                &mut sd,
            );
            if ret != 0 {
                return Err(win_err("GetNamedSecurityInfoW", ret));
            }
            // 构造：容器 SID 允许 GENERIC_ALL，子容器与对象继承
            let trustee = TRUSTEE_W {
                pMultipleTrustee: ptr::null_mut(),
                MultipleTrusteeOperation: 0,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_USER,
                ptstrName: self.sid as *mut u16,
            };
            let mut access = EXPLICIT_ACCESS_W {
                grfAccessPermissions: GENERIC_ALL,
                grfAccessMode: GRANT_ACCESS,
                grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
                Trustee: trustee,
            };
            let mut new_dacl: *mut ACL = ptr::null_mut();
            let r2 = SetEntriesInAclW(1, &mut access, dacl, &mut new_dacl);
            if r2 != 0 {
                let _ = LocalFree(sd);
                return Err(win_err("SetEntriesInAclW", r2));
            }
            let r3 = SetNamedSecurityInfoW(
                dir_w.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                new_dacl,
                ptr::null_mut(),
            );
            let _ = LocalFree(new_dacl as *mut std::ffi::c_void);
            let _ = LocalFree(sd);
            if r3 != 0 {
                return Err(win_err("SetNamedSecurityInfoW", r3));
            }
        }
        Ok(())
    }

    /// 以 AppContainer 运行 cmd 命令（阻塞），返回 (stdout, stderr, 退出码)
    pub fn run(&self, cwd: &Path, command: &str) -> Result<(Vec<u8>, Vec<u8>, i32), String> {
        unsafe {
            // ---------- 输出管道（stdout/stderr 合并） ----------
            let mut out_read: HANDLE = ptr::null_mut();
            let mut out_write: HANDLE = ptr::null_mut();
            if CreatePipe(&mut out_read, &mut out_write, ptr::null(), 0) == 0 {
                return Err(win_err("CreatePipe(out)", GetLastError()));
            }
            if SetHandleInformation(out_write, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) == 0 {
                let _ = CloseHandle(out_read);
                let _ = CloseHandle(out_write);
                return Err(win_err("SetHandleInformation", GetLastError()));
            }
            // ---------- 输入管道（关闭写端 → 子进程 stdin 立即 EOF） ----------
            let mut in_read: HANDLE = ptr::null_mut();
            let mut in_write: HANDLE = ptr::null_mut();
            if CreatePipe(&mut in_read, &mut in_write, ptr::null(), 0) == 0 {
                let _ = CloseHandle(out_read);
                let _ = CloseHandle(out_write);
                return Err(win_err("CreatePipe(in)", GetLastError()));
            }
            if SetHandleInformation(in_read, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) == 0 {
                let _ = CloseHandle(out_read);
                let _ = CloseHandle(out_write);
                let _ = CloseHandle(in_read);
                let _ = CloseHandle(in_write);
                return Err(win_err("SetHandleInformation(in)", GetLastError()));
            }
            let _ = CloseHandle(in_write);

            // ---------- AppContainer 安全能力属性 ----------
            let mut sec_cap = SECURITY_CAPABILITIES {
                AppContainerSid: self.sid,
                Capabilities: ptr::null_mut::<windows_sys::Win32::Security::SID_AND_ATTRIBUTES>(),
                CapabilityCount: 0,
                Reserved: 0,
            };
            let mut attr_size: usize = 0;
            let _ = InitializeProcThreadAttributeList(
                ptr::null_mut(),
                1,
                0,
                &mut attr_size,
            );
            let mut attr_buf = vec![0u8; attr_size];
            let attr_list = attr_buf.as_mut_ptr() as *mut _;
            if InitializeProcThreadAttributeList(attr_list, 1, 0, &mut attr_size) == 0 {
                return Err(win_err("InitializeProcThreadAttributeList", GetLastError()));
            }
            if UpdateProcThreadAttribute(
                attr_list,
                0,
                PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
                &mut sec_cap as *mut SECURITY_CAPABILITIES as *const std::ffi::c_void,
                std::mem::size_of::<SECURITY_CAPABILITIES>(),
                ptr::null_mut(),
                ptr::null_mut(),
            ) == 0
            {
                return Err(win_err("UpdateProcThreadAttribute", GetLastError()));
            }

            // ---------- 启动进程 ----------
            let mut si: STARTUPINFOEXW = std::mem::zeroed();
            si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
            si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            si.StartupInfo.hStdInput = in_read;
            si.StartupInfo.hStdOutput = out_write;
            si.StartupInfo.hStdError = out_write;
            si.lpAttributeList = attr_list;

            let cmdline_w = to_wide(&format!("cmd.exe /C {command}"));
            let cwd_w = to_wide_path(cwd);
            let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
            let ok = CreateProcessW(
                ptr::null(),
                cmdline_w.as_ptr() as *mut u16,
                ptr::null(),
                ptr::null(),
                1, // TRUE: bInheritHandles（继承管道写端）
                CREATE_NO_WINDOW | EXTENDED_STARTUPINFO_PRESENT,
                ptr::null(),
                cwd_w.as_ptr(),
                &mut si as *mut STARTUPINFOEXW as *mut STARTUPINFOW,
                &mut pi,
            );
            let _ = CloseHandle(out_write);
            let _ = CloseHandle(in_read);
            if ok == 0 {
                return Err(win_err("CreateProcessW", GetLastError()));
            }

            // ---------- 读输出（独立线程阻塞读，进程退出后 EOF 结束） ----------
            let out_read_usize = out_read as usize;
            let reader = std::thread::spawn(move || {
                let out_read = out_read_usize as HANDLE;
                let mut buf = Vec::new();
                let mut tmp = [0u8; 8192];
                loop {
                    let mut read: u32 = 0;
                    if ReadFile(
                        out_read,
                        tmp.as_mut_ptr() as *mut _,
                        tmp.len() as u32,
                        &mut read,
                        ptr::null_mut(),
                    ) == 0
                    {
                        break;
                    }
                    if read == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..read as usize]);
                }
                let _ = CloseHandle(out_read);
                buf
            });

            // ---------- 等待 / 超时终止 ----------
            let wait = WaitForSingleObject(pi.hProcess, 60_000);
            let mut timed_out = false;
            if wait != 0 {
                // WAIT_TIMEOUT / FAILED：终止进程
                timed_out = true;
                let _ = TerminateProcess(pi.hProcess, 1);
                let _ = WaitForSingleObject(pi.hProcess, 5_000);
            }
            let mut code: u32 = 0;
            let _ = GetExitCodeProcess(pi.hProcess, &mut code);
            let _ = CloseHandle(pi.hProcess);
            let _ = CloseHandle(pi.hThread);
            let out_buf = reader.join().unwrap_or_default();

            let mut stderr = Vec::new();
            if timed_out {
                stderr = "命令执行超时（60 秒）".as_bytes().to_vec();
            }
            Ok((out_buf, stderr, code as i32))
        }
    }
}

/// 工具函数：把输出格式化为返回文本（含退出码）
pub fn format_output(stdout: &[u8], stderr: &[u8], code: i32) -> String {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    let mut out = String::new();
    if !stdout.trim().is_empty() {
        out.push_str(&stdout);
    }
    if !stderr.trim().is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("[stderr]\n");
        out.push_str(&stderr);
    }
    if out.is_empty() {
        out = "(命令执行完成，无输出)".into();
    }
    out.push_str(&format!("\n[退出码 {code}]"));
    out.chars().take(12000).collect()
}
