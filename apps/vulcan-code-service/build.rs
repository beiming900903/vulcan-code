//! Native executable resources for the independent Core service.
//! 独立 Core 服务的原生可执行文件资源。

// Embed the executable icon and version metadata; MSVC link arguments live in `.cargo/config.toml`.
// 嵌入可执行文件图标和版本元数据；MSVC 链接参数位于 `.cargo/config.toml`。
//
// On Linux and macOS, Core also exports the Lua ABI used by bundled native modules.
// Linux 和 macOS 上的 Core 还会导出随包原生模块使用的 Lua ABI。
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // Inspect the target rather than the build host so cross-compilation selects the right resources.
    // 检查编译目标而非构建宿主，使交叉编译选择正确的资源。
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").expect("Cargo target OS");
    if matches!(target_os.as_str(), "linux" | "macos") {
        // The two platforms share Lua API names, but Mach-O prefixes C symbols with an underscore.
        // 两个平台共用 Lua API 名称，但 Mach-O 会为 C 符号添加下划线前缀。
        if target_os == "macos" {
            println!("cargo:rustc-link-arg-bin=vulcan-code-service=-Wl,-export_dynamic");
        }
        // Root each native import in the static LuaJIT archive and expose it to the dynamic loader.
        // 将每个原生模块导入保留为静态 LuaJIT 归档的链接根，并向动态加载器导出。
        for symbol in include_str!("lua-native-symbols.txt").lines() {
            if target_os == "macos" {
                println!("cargo:rustc-link-arg-bin=vulcan-code-service=-Wl,-u,_{symbol}");
            } else {
                println!("cargo:rustc-link-arg-bin=vulcan-code-service=-Wl,--undefined={symbol}");
                println!(
                    "cargo:rustc-link-arg-bin=vulcan-code-service=-Wl,--export-dynamic-symbol={symbol}"
                );
            }
        }
        println!("cargo:rerun-if-changed=lua-native-symbols.txt");
    }
    if target_os == "windows" {
        // LuaJIT expands executable-relative package paths through GetModuleFileNameA.
        // Give this process a UTF-8 code page so those bytes match the host's Rust paths.
        // LuaJIT 通过 GetModuleFileNameA 展开相对于可执行文件的包路径。
        // 让本进程使用 UTF-8 代码页，使得到的字节与 Rust 宿主路径一致。
        println!("cargo:rerun-if-changed=windows-utf8.manifest");
        // Link the manifest into both the desktop and headless service executable.
        // 将清单嵌入桌面版和无头版服务程序。
        let manifest =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("windows-utf8.manifest");
        println!("cargo:rustc-link-arg-bin=vulcan-code-service=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bin=vulcan-code-service=/MANIFESTINPUT:{}",
            manifest.display()
        );
        #[cfg(feature = "desktop-tray")]
        {
            println!("cargo:rerun-if-changed=icons/icon.ico");
            winresource::WindowsResource::new()
                .set_icon("icons/icon.ico")
                .set("ProductName", "Vulcan Code")
                .set("FileDescription", "Vulcan Code Service")
                .compile()
                .expect("compile Core executable icon and version resources");
        }
    }
}
