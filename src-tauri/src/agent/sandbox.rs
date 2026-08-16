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
//!
//! 非 Windows 平台无 AppContainer 可用：退化为普通子进程执行（无系统级隔离，
//! 但仍保留调用前的权限确认与 60 秒超时）。

#[cfg(target_os = "windows")]
mod imp {
    use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr;

use windows_sys::Win32::Foundation::{
    GENERIC_ALL, GENERIC_EXECUTE, GENERIC_READ, GetLastError, LocalFree, SetHandleInformation,
    CloseHandle, HANDLE, HANDLE_FLAG_INHERIT,
};
use windows_sys::Win32::Security::{
    FreeSid, ACL, PSID, DACL_SECURITY_INFORMATION, SUB_CONTAINERS_AND_OBJECTS_INHERIT,
    SECURITY_CAPABILITIES, SID_AND_ATTRIBUTES, DeriveCapabilitySidsFromName,
    CreateWellKnownSid, WinLowLabelSid, TOKEN_MANDATORY_LABEL, LABEL_SECURITY_INFORMATION,
    TokenIntegrityLevel, TOKEN_ADJUST_DEFAULT, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE,
    TOKEN_QUERY, DuplicateTokenEx, SetTokenInformation, SecurityImpersonation, TokenPrimary,
    SECURITY_MAX_SID_SIZE, AddMandatoryAce, GetLengthSid, InitializeAcl, ACL_REVISION_DS,
};
use windows_sys::Win32::Security::Authorization::{
    GetNamedSecurityInfoW, SetNamedSecurityInfoW, SetEntriesInAclW, EXPLICIT_ACCESS_W, TRUSTEE_W,
    SE_FILE_OBJECT, TRUSTEE_IS_SID, TRUSTEE_IS_USER, GRANT_ACCESS,
};
use windows_sys::Win32::System::SystemServices::{
    SE_GROUP_ENABLED, SE_GROUP_INTEGRITY, SYSTEM_MANDATORY_LABEL_NO_WRITE_UP,
};
use windows_sys::Win32::Security::Isolation::{
    CreateAppContainerProfile, DeriveAppContainerSidFromAppContainerName,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::Storage::FileSystem::ReadFile;
use windows_sys::Win32::System::Threading::{
    PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, STARTF_USESTDHANDLES, STARTUPINFOEXW,
    STARTUPINFOW, PROCESS_INFORMATION, EXTENDED_STARTUPINFO_PRESENT, CREATE_NO_WINDOW,
    CREATE_UNICODE_ENVIRONMENT, InitializeProcThreadAttributeList, UpdateProcThreadAttribute,
    DeleteProcThreadAttributeList, CreateProcessW, CreateProcessAsUserW, OpenProcessToken,
    GetCurrentProcess, WaitForSingleObject, TerminateProcess, GetExitCodeProcess,
};

/// bash 命令执行超时：开发场景（npm install / cargo build / pytest 等）
/// 普遍超过 60 秒，统一放宽到 5 分钟
pub const BASH_TIMEOUT_SECS: u32 = 300;
pub const BASH_TIMEOUT_MS: u32 = BASH_TIMEOUT_SECS * 1000;

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
    /// 网络 capability SID（指向 _cap_arrs 数组内的内存，随数组释放失效，
    /// Drop 顺序保证先于数组释放不被使用）。不含 capability 的 AppContainer
    /// 无任何网络访问权，pip/npm 等全部失败
    cap_sids: Vec<PSID>,
    /// DeriveCapabilitySidsFromName 返回的 SID 数组（LocalFree 释放；
    /// SID 结构内嵌于数组内存，数组必须在进程创建期间保持存活）
    _cap_arrs: Vec<*mut PSID>,
}

// SID 指针仅在本沙箱单线程内使用（spawn_blocking 只在一个线程执行闭包）
unsafe impl Send for ContainerSandbox {}

impl Drop for ContainerSandbox {
    fn drop(&mut self) {
        unsafe {
            for &a in &self._cap_arrs {
                if !a.is_null() {
                    LocalFree(a as *mut std::ffi::c_void);
                }
            }
            FreeSid(self.sid);
        }
    }
}

impl ContainerSandbox {
    /// 全局共享容器：所有会话复用同一个 AppContainer。
    /// 会话间隔离仍成立——每个会话只把「自己的」目录授权给容器 SID，
    /// 其他会话目录无对应 ACE 互相不可见；同时避免「每会话一个容器」
    /// 使用户工具目录的 DACL 随会话数无限累积容器 SID 条目
    pub fn shared() -> Result<Self, String> {
        let name = "ChatDeepSeek.DevSandbox";
        let display = "ChatDeepSeek Dev Sandbox";
        let description = "ChatDeepSeek 编程沙箱（全局）";
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
                    return Err(win_err("DeriveAppContainerSidFromAppContainerName", r2 as u32));
                }
                sid = derived;
            } else if ret < 0 {
                return Err(win_err("CreateAppContainerProfile", ret as u32));
            }
        }
        // 派生网络 capability SID。注意 API 返回 BOOL（非零=成功，0=失败）。
        // internetClientServer 含出站连接与本地监听，覆盖开发场景：
        // pip/npm 联网下载、启动 dev server。
        // 返回两组数组：capability 组 SID + capability SID，均需加入进程属性
        let mut cap_sids: Vec<PSID> = Vec::new();
        let mut cap_arrs: Vec<*mut PSID> = Vec::new();
        unsafe {
            let mut group_arr: *mut PSID = ptr::null_mut();
            let mut group_count: u32 = 0;
            let mut cap_arr: *mut PSID = ptr::null_mut();
            let mut cap_count: u32 = 0;
            if DeriveCapabilitySidsFromName(
                to_wide("internetClientServer").as_ptr(),
                &mut group_arr,
                &mut group_count,
                &mut cap_arr,
                &mut cap_count,
            ) == 0
            {
                log::warn!(
                    "[sandbox] 派生网络 capability 失败（错误码 0x{:08X}），沙箱内将无法联网",
                    GetLastError()
                );
            } else {
                // 仅取 capability SID 数组（AppAuthority，S-1-15-3-…）。
                // 实测 group SID（NT Authority）放入 SECURITY_CAPABILITIES.
                // Capabilities 会导致 CreateProcessW 返回
                // ERROR_INVALID_PARAMETER(0x57)，不能混入
                for i in 0..cap_count as usize {
                    let s = *cap_arr.add(i);
                    if !s.is_null() {
                        cap_sids.push(s);
                    }
                }
                // 两个数组都由本沙箱持有并统一 LocalFree（SID 内嵌于数组内存）
                if !group_arr.is_null() {
                    cap_arrs.push(group_arr);
                }
                if !cap_arr.is_null() {
                    cap_arrs.push(cap_arr);
                }
            }
        }
        Ok(ContainerSandbox {
            _name: name.into(),
            sid,
            cap_sids,
            _cap_arrs: cap_arrs,
        })
    }

    /// 是否成功派生网络 capability（诊断用：false 表示沙箱内无网络）
    pub fn has_network_capability(&self) -> bool {
        !self.cap_sids.is_empty()
    }

    /// 将目录的 DACL 合并授权给容器 SID（含子对象继承）
    pub fn grant_access(&self, dir: &Path) -> Result<(), String> {
        self.grant_with(dir, GENERIC_ALL)
    }

    /// 授予目录读 + 执行（不给写）：用于开发工具链目录（PATH 条目、
    /// rustup 工具链等），让沙箱能启动工具而不获得篡改能力
    pub fn grant_read_execute(&self, dir: &Path) -> Result<(), String> {
        self.grant_with(dir, GENERIC_READ | GENERIC_EXECUTE)
    }

    fn grant_with(&self, dir: &Path, permissions: u32) -> Result<(), String> {
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
            // 构造：容器 SID 授权，子容器与对象继承
            let trustee = TRUSTEE_W {
                pMultipleTrustee: ptr::null_mut(),
                MultipleTrusteeOperation: 0,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_USER,
                ptstrName: self.sid as *mut u16,
            };
            let mut access = EXPLICIT_ACCESS_W {
                grfAccessPermissions: permissions,
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

    /// 授权开发工具链目录（读 + 执行）：PATH 中非系统目录 + 常见用户级工具位置。
    /// AppContainer 默认只能访问会话目录与系统目录（ALL APPLICATION PACKAGES），
    /// 用户安装的 node/python/cargo 等一律"拒绝访问"——此函数解决工具不可用问题。
    /// 返回授权失败的目录列表：常见原因目录由提权安装器创建（Owner=
    /// Administrators），非提权进程无 WRITE_DAC（错误 5）；PATH 条目授权失败
    /// 意味着对应工具在容器内不可用，调用方应降级为低完整性模式
    pub fn grant_dev_tool_dirs(&self) -> Vec<String> {
        let mut dirs: Vec<std::path::PathBuf> = Vec::new();
        // 常见用户级工具位置
        if let Ok(home) = std::env::var("USERPROFILE") {
            dirs.push(std::path::PathBuf::from(&home).join(".cargo"));
            dirs.push(std::path::PathBuf::from(&home).join(".rustup"));
            dirs.push(std::path::PathBuf::from(&home).join("go").join("bin"));
            dirs.push(std::path::PathBuf::from(&home).join("scoop"));
        }
        if let Ok(lad) = std::env::var("LOCALAPPDATA") {
            dirs.push(std::path::PathBuf::from(&lad).join("Programs"));
            dirs.push(std::path::PathBuf::from(&lad).join("Microsoft").join("WindowsApps"));
        }
        // PATH 中用户自定义目录（系统目录已有 ALL APPLICATION PACKAGES 权限，跳过
        // 并避免修改系统目录 ACL）
        let mut failed: Vec<String> = Vec::new();
        if let Ok(path) = std::env::var("PATH") {
            for p in path.split(';') {
                let p = p.trim();
                if p.is_empty() {
                    continue;
                }
                let pb = std::path::PathBuf::from(p);
                if !is_system_dir(&pb) {
                    dirs.push(pb);
                }
            }
        }
        let mut seen = std::collections::HashSet::new();
        for d in dirs {
            if !d.is_dir() || !seen.insert(d.to_string_lossy().to_lowercase()) {
                continue;
            }
            if let Err(e) = self.grant_read_execute(&d) {
                log::warn!("[sandbox] 授权工具目录失败 {}: {e}", d.display());
                failed.push(d.display().to_string());
            }
        }
        failed
    }

    /// 以 AppContainer 运行 cmd 命令（阻塞），返回 (stdout, stderr, 退出码)。
    /// env_overrides 在继承父进程环境的基础上覆盖指定变量（TEMP/TMP 重定向、
    /// 包管理器缓存目录重定向等，AppContainer 无法写用户目录下的默认位置）
    pub fn run(
        &self,
        cwd: &Path,
        command: &str,
        env_overrides: &[(String, String)],
    ) -> Result<(Vec<u8>, Vec<u8>, i32), String> {
        unsafe {
            // ---------- std 管道（与低完整性路径共用） ----------
            let (out_read, out_write, in_read) = make_std_pipes()?;

            // ---------- AppContainer 安全能力属性（含网络 capability） ----------
            let mut cap_attrs: Vec<SID_AND_ATTRIBUTES> = self
                .cap_sids
                .iter()
                .map(|&s| SID_AND_ATTRIBUTES {
                    Sid: s,
                    Attributes: SE_GROUP_ENABLED as u32,
                })
                .collect();
            let mut sec_cap = SECURITY_CAPABILITIES {
                AppContainerSid: self.sid,
                Capabilities: if cap_attrs.is_empty() {
                    ptr::null_mut()
                } else {
                    cap_attrs.as_mut_ptr()
                },
                CapabilityCount: cap_attrs.len() as u32,
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
                let _ = CloseHandle(out_read);
                let _ = CloseHandle(out_write);
                let _ = CloseHandle(in_read);
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
                let _ = CloseHandle(out_read);
                let _ = CloseHandle(out_write);
                let _ = CloseHandle(in_read);
                return Err(win_err("UpdateProcThreadAttribute", GetLastError()));
            }

            // ---------- 环境块（继承父进程 + overrides 覆盖，UTF-16） ----------
            let env_block = build_env_block(env_overrides);

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
                CREATE_NO_WINDOW | EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                env_block.as_ptr() as *const std::ffi::c_void,
                cwd_w.as_ptr(),
                &mut si as *mut STARTUPINFOEXW as *mut STARTUPINFOW,
                &mut pi,
            );
            let _ = CloseHandle(out_write);
            let _ = CloseHandle(in_read);
            if ok == 0 {
                let _ = CloseHandle(out_read);
                return Err(win_err("CreateProcessW", GetLastError()));
            }

            // ---------- 读输出 + 等待 / 超时终止（与低完整性路径共用） ----------
            let reader = spawn_pipe_reader(out_read);
            let result = wait_process_collect(&pi, reader);
            // 释放属性列表占用的系统资源（MSDN 要求与 Initialize 配对调用）
            let _ = DeleteProcThreadAttributeList(attr_list);
            result
        }
    }
}

/// 创建 std 管道：out（stdout/stderr 合并，写端可继承）+ in（读端可继承、
/// 写端立即关闭 → 子进程 stdin 直接 EOF）。返回 (out_read, out_write, in_read)
unsafe fn make_std_pipes() -> Result<(HANDLE, HANDLE, HANDLE), String> {
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
    Ok((out_read, out_write, in_read))
}

/// 独立线程阻塞读管道直到 EOF（进程退出关闭写端），返回全部输出
unsafe fn spawn_pipe_reader(out_read: HANDLE) -> std::thread::JoinHandle<Vec<u8>> {
    let out_read_usize = out_read as usize;
    std::thread::spawn(move || {
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
    })
}

/// 等待进程（超时终止）并汇合读线程，返回 (stdout, stderr, 退出码)
unsafe fn wait_process_collect(
    pi: &PROCESS_INFORMATION,
    reader: std::thread::JoinHandle<Vec<u8>>,
) -> Result<(Vec<u8>, Vec<u8>, i32), String> {
    let wait = WaitForSingleObject(pi.hProcess, BASH_TIMEOUT_MS);
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
        stderr = format!("命令执行超时（{} 秒）", BASH_TIMEOUT_MS / 1000)
            .as_bytes()
            .to_vec();
    }
    Ok((out_buf, stderr, code as i32))
}

/// 构造 Low 完整性 SID（S-1-16-4096）
unsafe fn low_integrity_sid() -> Result<[u8; SECURITY_MAX_SID_SIZE as usize], String> {
    let mut sid = [0u8; SECURITY_MAX_SID_SIZE as usize];
    let mut size = sid.len() as u32;
    if CreateWellKnownSid(
        WinLowLabelSid,
        ptr::null_mut(),
        sid.as_mut_ptr() as PSID,
        &mut size,
    ) == 0
    {
        return Err(win_err("CreateWellKnownSid(WinLowLabelSid)", GetLastError()));
    }
    Ok(sid)
}

/// 将单个路径打上 Low 完整性标签。正确方式：构造含单个
/// SYSTEM_MANDATORY_LABEL_ACE 的 SACL（AddMandatoryAce），经 pSacl +
/// LABEL_SECURITY_INFORMATION 写入。此前误把标签 SID 传 psidOwner——
/// 未设置 OWNER flag 时该参数被忽略，标签实际未生效（实测子进程全部
/// 写入被拒）。设置 label 不需要 SE_SECURITY_NAME 特权，对象所有者即可
unsafe fn label_path_low(p: &Path, sid: PSID) -> Result<(), String> {
    let w = to_wide_path(p);
    // ACL 头 + ACE 头(4B) + policy u32 + SID，u16 对齐满足 ACL 结构要求
    let sid_len = GetLengthSid(sid) as usize;
    let acl_len = std::mem::size_of::<ACL>() + 4 + 4 + sid_len;
    let mut acl_buf = vec![0u16; acl_len.div_ceil(2)];
    let acl = acl_buf.as_mut_ptr() as *mut ACL;
    if InitializeAcl(acl, acl_len as u32, ACL_REVISION_DS) == 0 {
        return Err(win_err("InitializeAcl(label)", GetLastError()));
    }
    // NO_WRITE_UP：IL 低于标签的对象禁止写。工作区标签 = Low，Low 子进程
    // 不低于它 → 可写；工作区外对象为 Medium+，Low 子进程低于它 → 拒写
    if AddMandatoryAce(acl, ACL_REVISION_DS, 0, SYSTEM_MANDATORY_LABEL_NO_WRITE_UP, sid) == 0 {
        return Err(win_err("AddMandatoryAce", GetLastError()));
    }
    let r = SetNamedSecurityInfoW(
        w.as_ptr(),
        SE_FILE_OBJECT,
        LABEL_SECURITY_INFORMATION,
        ptr::null_mut(),
        ptr::null_mut(),
        ptr::null_mut(),
        acl,
    );
    if r != 0 {
        return Err(win_err("SetNamedSecurityInfoW(label)", r));
    }
    Ok(())
}

/// 将目录树打上 Low 完整性标签：目录打标后新建子项自动继承，
/// 已存在的子项需逐个补标（条目上限防止超大目录卡死）。
/// Low 完整性子进程只能写入 Low 标签对象——工作区因此可写，
/// 用户目录/系统目录（Medium+）被 OS 拒绝写入
pub fn label_dir_tree_low(dir: &Path) -> Result<(), String> {
    unsafe {
        let sid = low_integrity_sid()?;
        let sid_ptr = sid.as_ptr() as PSID;
        let mut count = 0usize;
        fn walk(p: &Path, sid_ptr: PSID, count: &mut usize) -> Result<(), String> {
            const MAX_ITEMS: usize = 5000;
            if *count >= MAX_ITEMS {
                return Ok(());
            }
            unsafe {
                label_path_low(p, sid_ptr)?;
                *count += 1;
            }
            if p.is_dir() {
                let entries = match std::fs::read_dir(p) {
                    Ok(e) => e,
                    Err(_) => return Ok(()),
                };
                for e in entries.filter_map(|e| e.ok()) {
                    if *count >= MAX_ITEMS {
                        break;
                    }
                    let _ = walk(&e.path(), sid_ptr, count);
                }
            }
            Ok(())
        }
        walk(dir, sid_ptr, &mut count)
    }
}

/// 【低完整性降级模式】当 AppContainer 无法授权工具目录（目录 Owner=
/// Administrators，非提权进程无 WRITE_DAC）时使用：
/// 子进程令牌降到 Low 完整性（复制自身令牌，无需额外特权）——
/// - 可正常执行任意用户级工具（node/python/go 无需改 ACL）
/// - 可联网（无 capability 限制）
/// - 只能写入 Low 标签对象：工作区目录（调用前需 label_dir_tree_low），
///   用户目录/系统目录被强制完整性策略拒绝写入（读不受限）
pub fn run_low_integrity(
    cwd: &Path,
    command: &str,
    env_overrides: &[(String, String)],
) -> Result<(Vec<u8>, Vec<u8>, i32), String> {
    unsafe {
        let (out_read, out_write, in_read) = make_std_pipes()?;

        // ---------- 令牌：复制自身主令牌并降完整性 ----------
        let mut self_tok: HANDLE = ptr::null_mut();
        if OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ADJUST_DEFAULT | TOKEN_ASSIGN_PRIMARY,
            &mut self_tok,
        ) == 0
        {
            let _ = CloseHandle(out_read);
            let _ = CloseHandle(out_write);
            let _ = CloseHandle(in_read);
            return Err(win_err("OpenProcessToken", GetLastError()));
        }
        let mut primary: HANDLE = ptr::null_mut();
        if DuplicateTokenEx(
            self_tok,
            TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ADJUST_DEFAULT | TOKEN_ASSIGN_PRIMARY,
            ptr::null(),
            SecurityImpersonation,
            TokenPrimary,
            &mut primary,
        ) == 0
        {
            let _ = CloseHandle(self_tok);
            let _ = CloseHandle(out_read);
            let _ = CloseHandle(out_write);
            let _ = CloseHandle(in_read);
            return Err(win_err("DuplicateTokenEx", GetLastError()));
        }
        let _ = CloseHandle(self_tok);
        let sid = low_integrity_sid()?;
        let tml = TOKEN_MANDATORY_LABEL {
            Label: SID_AND_ATTRIBUTES {
                Sid: sid.as_ptr() as PSID,
                Attributes: SE_GROUP_INTEGRITY as u32,
            },
        };
        if SetTokenInformation(
            primary,
            TokenIntegrityLevel,
            &tml as *const _ as *const std::ffi::c_void,
            std::mem::size_of::<TOKEN_MANDATORY_LABEL>() as u32,
        ) == 0
        {
            let _ = CloseHandle(primary);
            let _ = CloseHandle(out_read);
            let _ = CloseHandle(out_write);
            let _ = CloseHandle(in_read);
            return Err(win_err("SetTokenInformation(TokenIntegrityLevel)", GetLastError()));
        }

        // ---------- 启动进程（复制自自身令牌，无需 SeAssignPrimaryTokenPrivilege） ----------
        let env_block = build_env_block(env_overrides);
        let mut si: STARTUPINFOW = std::mem::zeroed();
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        si.dwFlags = STARTF_USESTDHANDLES;
        si.hStdInput = in_read;
        si.hStdOutput = out_write;
        si.hStdError = out_write;

        let mut cmdline_w = to_wide(&format!("cmd.exe /C {command}"));
        let cwd_w = to_wide_path(cwd);
        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
        let ok = CreateProcessAsUserW(
            primary,
            ptr::null(),
            cmdline_w.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            1,
            CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT,
            env_block.as_ptr() as *const std::ffi::c_void,
            cwd_w.as_ptr(),
            &si,
            &mut pi,
        );
        let _ = CloseHandle(primary);
        let _ = CloseHandle(out_write);
        let _ = CloseHandle(in_read);
        if ok == 0 {
            let _ = CloseHandle(out_read);
            return Err(win_err("CreateProcessAsUserW", GetLastError()));
        }

        let reader = spawn_pipe_reader(out_read);
        wait_process_collect(&pi, reader)
    }
}

/// 系统目录（Windows / Program Files / ProgramData）：
/// 默认 ACL 已含 ALL APPLICATION PACKAGES 读执行权限，无需也不应重复授权
fn is_system_dir(p: &Path) -> bool {
    let s = p.to_string_lossy().to_lowercase();
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| "c:\\windows".into()).to_lowercase();
    s.starts_with(&windir)
        || s.starts_with("c:\\program files")
        || s.starts_with("c:\\programdata")
        // 系统盘根下的公共工具目录也可能是系统级，仅排除明确的系统目录
}

/// 构造 UTF-16 环境块：继承当前进程全部环境变量，overrides 覆盖同名项（大小写不敏感）。
/// Windows 要求环境块按名称排序且以双 \0 结尾，并配合 CREATE_UNICODE_ENVIRONMENT
fn build_env_block(overrides: &[(String, String)]) -> Vec<u16> {
    let mut vars: Vec<(String, String)> = std::env::vars().collect();
    for (k, v) in overrides {
        if let Some(item) = vars.iter_mut().find(|(ek, _)| ek.eq_ignore_ascii_case(k)) {
            item.1 = v.clone();
        } else {
            vars.push((k.clone(), v.clone()));
        }
    }
    vars.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    let mut block: Vec<u16> = Vec::new();
    for (k, v) in &vars {
        block.extend(format!("{k}={v}").encode_utf16());
        block.push(0);
    }
    block.push(0);
    block
}
}

#[cfg(not(target_os = "windows"))]
mod imp {
    use std::path::Path;
    use std::process::Command;

    /// 与 Windows 侧保持一致的超时（见 Windows 实现处说明）
    pub const BASH_TIMEOUT_SECS: u32 = 300;

    /// 非 Windows 平台：无 AppContainer 可用，退化为普通子进程执行。
    /// 调用前的权限确认与超时仍在 tools.rs 层生效，但无操作系统级隔离。
    pub struct ContainerSandbox {
        _session_id: i64,
    }

    impl ContainerSandbox {
        pub fn shared() -> Result<Self, String> {
            Ok(ContainerSandbox {
                _session_id: 0,
            })
        }

        /// 非 Windows 无 capability 限制，网络可用
        pub fn has_network_capability(&self) -> bool {
            true
        }

        pub fn grant_access(&self, _dir: &Path) -> Result<(), String> {
            Ok(())
        }

        pub fn grant_read_execute(&self, _dir: &Path) -> Result<(), String> {
            Ok(())
        }

        /// 非 Windows 无 AppContainer，工具目录本就可访问，无失败
        pub fn grant_dev_tool_dirs(&self) -> Vec<String> {
            Vec::new()
        }

        /// 以系统 shell 执行命令（阻塞），返回 (stdout, stderr, 退出码)
        pub fn run(
            &self,
            cwd: &Path,
            command: &str,
            env_overrides: &[(String, String)],
        ) -> Result<(Vec<u8>, Vec<u8>, i32), String> {
            run_low_integrity(cwd, command, env_overrides)
        }
    }

    /// 非 Windows：无完整性级别概念，退化为普通子进程执行
    pub fn label_dir_tree_low(_dir: &Path) -> Result<(), String> {
        Ok(())
    }

    /// 非 Windows：无 AppContainer/低完整性之分，普通执行
    pub fn run_low_integrity(
        cwd: &Path,
        command: &str,
        env_overrides: &[(String, String)],
    ) -> Result<(Vec<u8>, Vec<u8>, i32), String> {
        let output = Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(cwd)
            .envs(env_overrides.iter().cloned())
            .output()
            .map_err(|e| format!("执行命令失败: {e}"))?;
        Ok((
            output.stdout,
            output.stderr,
            output.status.code().unwrap_or(-1),
        ))
    }
}

pub use imp::{ContainerSandbox, BASH_TIMEOUT_SECS, label_dir_tree_low, run_low_integrity};

#[cfg(test)]
mod tests {
    use super::*;

    /// 沙箱全链路验证：容器创建 → capability 派生（BOOL 语义修复后必须非空，
    /// 否则沙箱无网络）→ 目录授权 → AppContainer 进程创建 → 管道输出读取
    /// → 环境变量注入。任一环节断裂（安全策略限制 / API 误用）都会在此暴露
    #[test]
    fn sandbox_full_chain() {
        let dir = std::env::temp_dir().join("cds_sandbox_test");
        std::fs::create_dir_all(&dir).unwrap();
        let sb = ContainerSandbox::shared().expect("AppContainer profile 创建失败");
        assert!(
            sb.has_network_capability(),
            "网络 capability 未派生成功——沙箱内将无法联网"
        );
        sb.grant_access(&dir).expect("目录授权失败");
        let env = vec![("CDS_SANDBOX_TEST".to_string(), "injected".to_string())];
        let (out, err, code) = sb
            .run(&dir, "echo hello & echo %CDS_SANDBOX_TEST%", &env)
            .expect("沙箱执行失败");
        let stdout = String::from_utf8_lossy(&out);
        let stderr = String::from_utf8_lossy(&err);
        assert_eq!(code, 0, "退出码异常 stderr={stderr}");
        assert!(stdout.contains("hello"), "stdout 缺少命令输出: {stdout}");
        assert!(
            stdout.contains("injected"),
            "环境变量注入未生效: {stdout}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 低完整性降级路径验证：打标 → 执行 → 环境注入 →
    /// 写工作区（Low 标签）成功 + 写工作区外（Medium+）被 OS 拒绝。
    /// 这是 AppContainer 无法授权工具目录时编程功能可用的兜底保障
    #[test]
    fn low_integrity_chain() {
        let dir = std::env::temp_dir().join("cds_lowil_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        label_dir_tree_low(&dir).expect("Low 完整性打标失败");
        let env = vec![("CDS_LOWIL_TEST".to_string(), "injected".to_string())];
        let (out, err, code) = run_low_integrity(
            &dir,
            // 注意：被拒写（%TEMP%，Medium 完整性）放在中间——cmd /C 退出码取
            // 最后一条命令，末尾放成功命令保证 code=0；被拒本身由下方断言验证
            "echo lowil & echo %CDS_LOWIL_TEST% & echo bad > %TEMP%\\cds_lowil_out.txt & echo ok > in_ws.txt",
            &env,
        )
        .expect("低完整性执行失败");
        let stdout = String::from_utf8_lossy(&out);
        let stderr = String::from_utf8_lossy(&err);
        assert_eq!(code, 0, "退出码异常 stderr={stderr}");
        assert!(stdout.contains("lowil") && stdout.contains("injected"), "输出异常: {stdout}");
        assert!(dir.join("in_ws.txt").exists(), "工作区（Low 标签）写入应成功");
        assert!(
            !std::env::temp_dir().join("cds_lowil_out.txt").exists(),
            "工作区外（Medium 完整性）写入应被 OS 拒绝"
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(std::env::temp_dir().join("cds_lowil_out.txt"));
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
