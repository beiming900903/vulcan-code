//! Isolate the embedded interpreter from the parent process's Lua configuration.
//! 将内嵌解释器与父进程的 Lua 配置隔离。

/// Removes Lua startup overrides at the single-threaded Core entry point, or exits on failure.
/// 在 Core 的单线程入口移除 Lua 启动覆盖项，失败时退出。
///
/// Call this only from the entry point, before starting any worker or foreign runtime.
/// 仅在进程入口调用，且必须早于任何工作线程或外部运行时。
pub(super) fn isolate_at_entry() {
    // LuaJIT uses the unversioned names. Strip the versioned forms as well so startup
    // overrides do not reach bundled Lua subprocesses that recognize those forms.
    // LuaJIT 使用无版本名称。也移除带版本的形式，以免启动覆盖项传入使用这些名称的随包 Lua 子进程。
    for name in std::env::vars_os()
        .map(|(name, _)| name)
        .filter_map(lua_override)
    {
        #[cfg(windows)]
        if let Err(code) = remove_from_crt(&name) {
            eprintln!(
                "[vulcan-code-service] cannot isolate {name} from the Lua runtime (CRT error {code})"
            );
            std::process::exit(1);
        }
        // SAFETY: only the initial thread exists here. On Windows this also clears the
        // OS environment used by Rust subprocess creation, separately from the C runtime.
        // 安全性：此处仅有初始线程。在 Windows 上，这还单独清除 Rust 创建子进程时使用的系统进程环境。
        unsafe { std::env::remove_var(&name) };
    }
}

/// Returns the inherited key when it is a Lua path or initialization override.
/// 如果继承的键是 Lua 路径或初始化覆盖项，则返回该键。
fn lua_override(name: std::ffi::OsString) -> Option<String> {
    // These names are ASCII, and Windows environment keys are case-insensitive.
    // 这些名称均为 ASCII，且 Windows 的环境键不区分大小写。
    let name = name.into_string().ok()?;
    #[cfg(windows)]
    let normalized = name.to_ascii_uppercase();
    #[cfg(windows)]
    let key = normalized.as_str();
    #[cfg(not(windows))]
    let key = name.as_str();
    // Accept Lua's major_minor suffix form without matching unrelated variables sharing a prefix.
    // 识别 Lua 的 major_minor 后缀形式，避免误匹配共享前缀的无关变量。
    let is_override = ["LUA_PATH", "LUA_CPATH", "LUA_INIT"]
        .into_iter()
        .any(|prefix| {
            key == prefix
                || key
                    .strip_prefix(prefix)
                    .and_then(|suffix| suffix.strip_prefix('_'))
                    .and_then(|suffix| suffix.split_once('_'))
                    .is_some_and(|(major, minor)| {
                        !major.is_empty()
                            && !minor.is_empty()
                            && major
                                .bytes()
                                .chain(minor.bytes())
                                .all(|byte| byte.is_ascii_digit())
                    })
        });
    is_override.then_some(name)
}

/// Removes `name` from the MSVC CRT environment read by the statically linked LuaJIT.
/// 从静态链接的 LuaJIT 所读取的 MSVC CRT 环境中移除 `name`。
#[cfg(windows)]
fn remove_from_crt(name: &str) -> Result<(), std::ffi::c_int> {
    unsafe extern "C" {
        // Remove `name` when `value` is an empty, null-terminated C string.
        // `value` 是以空字符结尾的空 C 字符串时移除 `name`。
        fn _putenv_s(
            name: *const std::ffi::c_char,
            value: *const std::ffi::c_char,
        ) -> std::ffi::c_int;
    }
    // The input was matched against ASCII variable names and version suffixes.
    // 输入已按 ASCII 变量名及版本后缀校验。
    let name = std::ffi::CString::new(name).expect("validated Lua environment variable name");
    // SAFETY: both pointers are valid C strings and no other Core thread is running.
    // 安全性：两个指针均是有效的 C 字符串，且 Core 尚未运行其他线程。
    match unsafe { _putenv_s(name.as_ptr(), c"".as_ptr()) } {
        0 => Ok(()),
        code => Err(code),
    }
}
