//! Vulcan Code independent service and user-session tray host.
//! Vulcan Code 独立服务与用户会话托盘宿主。

#![cfg_attr(
    all(target_os = "windows", not(debug_assertions), feature = "desktop-tray"),
    windows_subsystem = "windows"
)]

mod browser_dependencies;
#[cfg(all(target_os = "windows", debug_assertions, feature = "desktop-tray"))]
mod debug_console;
mod desktop_integration;
#[cfg(feature = "desktop-tray")]
mod desktop_notifications;
#[cfg(unix)]
mod inherited_descriptors;
mod launch;
mod logging;
#[allow(unsafe_code)]
mod lua_environment;
mod managed_runtimes;
mod native_diagnostics;
mod restart;
mod runtime_layout;
mod service_runtime;
mod service_settings;
mod setup_update;
#[cfg(feature = "desktop-tray")]
mod tray;
mod user_service;

/// One-shot management command: returns the summary to print, or the failure that exits with status 1.
/// 一次性管理命令：返回要打印的摘要，或以状态 1 退出的失败信息。
type OneShotCommand = fn() -> Result<String, String>;

/// Starts the Vulcan Code service process.
/// 启动 Vulcan Code 服务进程。
fn main() {
    // Descriptors inherited from the desktop client go before anything else opens one.
    // 从桌面客户端继承的描述符在其他任何内容打开描述符之前关闭。
    #[cfg(unix)]
    inherited_descriptors::close();
    lua_environment::isolate_at_entry();
    // Install the crypto provider before the tray or background service constructs any TLS client.
    // 在托盘或后台服务构造任何 TLS 客户端之前安装加密实现。
    vulcan_provider_adapters::install_process_rustls_crypto_provider()
        .expect("the process-wide Rustls CryptoProvider must install before service startup");
    // The product version is read from the distribution, never compiled in; a missing or damaged
    // `resources/product.json` stops every command here instead of surfacing later.
    // 产品版本从发行物读取而非编译进来；`resources/product.json` 缺失或损坏时所有命令都在此停止，
    // 而不是在之后才暴露。
    if let Err(error) = vulcan_product_info::product() {
        eprintln!("[vulcan-code-service] {error}");
        std::process::exit(1);
    }
    let args: Vec<String> = std::env::args().collect();
    // Finishing an update through the setup program runs after the stopped service, never as one.
    // 经安装器完成更新在已停止的服务之后运行，本身绝不作为服务运行。
    match setup_update::requested(&args) {
        Ok(Some(request)) => match setup_update::run(&request) {
            Ok(summary) => {
                println!("{summary}");
                return;
            }
            Err(error) => {
                eprintln!("[vulcan-code-service] {error}");
                std::process::exit(1);
            }
        },
        Ok(None) => {}
        Err(error) => {
            eprintln!("[vulcan-code-service] {error}");
            std::process::exit(2);
        }
    }
    // Service registration is a one-shot management command that never starts the Core itself.
    // 服务注册是一次性管理命令，本身从不启动 Core。
    match user_service::user_service_action(&args) {
        Ok(Some(action)) => match user_service::run_user_service_action(action) {
            Ok(summary) => {
                println!("{summary}");
                return;
            }
            Err(error) => {
                eprintln!("[vulcan-code-service] {error}");
                std::process::exit(1);
            }
        },
        Ok(None) => {}
        Err(error) => {
            eprintln!("[vulcan-code-service] {error}");
            std::process::exit(2);
        }
    }
    // The port range is likewise a one-shot setting that takes effect at the next start.
    // 端口范围同样是一次性设置，在下次启动时生效。
    match service_settings::requested(&args) {
        Ok(Some(value)) => match service_settings::set_port_range(&value) {
            Ok(summary) => {
                println!("{summary}");
                return;
            }
            Err(error) => {
                eprintln!("[vulcan-code-service] {error}");
                std::process::exit(1);
            }
        },
        Ok(None) => {}
        Err(error) => {
            eprintln!("[vulcan-code-service] {error}");
            std::process::exit(2);
        }
    }
    // Browser system packages and managed runtimes are likewise one-shot commands run before any Core startup.
    // 浏览器系统软件包与受管运行时同样是一次性命令，在任何 Core 启动之前执行。
    let one_shot_commands: [(&str, OneShotCommand); 2] = [
        (
            browser_dependencies::ARG_INSTALL_BROWSER_DEPENDENCIES,
            browser_dependencies::install_browser_dependencies,
        ),
        (
            managed_runtimes::ARG_PREPARE_MANAGED_RUNTIMES,
            managed_runtimes::prepare_managed_runtimes,
        ),
    ];
    for (flag, command) in one_shot_commands {
        match launch::standalone_flag_requested(&args, flag) {
            Ok(true) => match command() {
                Ok(summary) => {
                    println!("{summary}");
                    return;
                }
                Err(error) => {
                    eprintln!("[vulcan-code-service] {error}");
                    std::process::exit(1);
                }
            },
            Ok(false) => {}
            Err(error) => {
                eprintln!("[vulcan-code-service] {error}");
                std::process::exit(2);
            }
        }
    }
    // describe_requested rejects mixed launch flags before test-root or business initialization.
    // describe_requested 在测试根或业务初始化之前拒绝混用启动参数。
    let describe_requested = match launch::service_describe_requested(&args) {
        Ok(requested) => requested,
        Err(error) => {
            eprintln!("[vulcan-code-service] {error}");
            std::process::exit(2);
        }
    };
    #[cfg(feature = "test-support")]
    if let Err(error) = service_runtime::install_isolated_test_root_from_args(&args) {
        eprintln!("[vulcan-code-service] {error}");
        std::process::exit(2);
    }
    #[cfg(not(feature = "test-support"))]
    if args
        .iter()
        .skip(1)
        .any(|arg| arg == crate::launch::ARG_TEST_ROOT || arg.starts_with("--test-root="))
    {
        eprintln!("[vulcan-code-service] --test-root is available only in test-support builds");
        std::process::exit(2);
    }
    if describe_requested {
        if let Err(error) = service_runtime::print_service_description() {
            eprintln!("[vulcan-code-service] describe failed: {error}");
            std::process::exit(2);
        }
        return;
    }
    // launchMode fixes one explicit Core role before the optional Debug console is attached.
    // launchMode 在挂载可选 Debug 控制台之前固定一个明确的 Core 启动角色。
    let launch_mode = match launch::parse_service_launch_args(&args) {
        Ok(mode) => mode,
        Err(error) => {
            eprintln!("[vulcan-code-service] {error}");
            eprintln!(
                "usage: vulcan-code-service (--gui-launch | --service | --headless | --describe) [--no-debug-console]
       vulcan-code-service (--install-user-service | --uninstall-user-service)
       vulcan-code-service --set-port-range (<first>-<last> | default)
       vulcan-code-service --install-browser-dependencies
       vulcan-code-service --prepare-managed-runtimes"
            );
            std::process::exit(2);
        }
    };
    #[cfg(all(target_os = "windows", debug_assertions, feature = "desktop-tray"))]
    if matches!(launch_mode, launch::ServiceLaunchMode::GuiLaunch)
        && !args
            .iter()
            .skip(1)
            .any(|arg| arg == launch::ARG_NO_DEBUG_CONSOLE)
    {
        if let Err(error) = debug_console::ensure_gui_debug_console() {
            eprintln!("[vulcan-code-service] Debug console startup failed: {error}");
            std::process::exit(2);
        }
        eprintln!(
            "[vulcan-code-service] Debug Core console ready (pid {})",
            std::process::id()
        );
    }
    match launch_mode {
        launch::ServiceLaunchMode::Headless => service_runtime::run_headless(),
        // Both desktop launches keep only the tray event loop: the service owns no workbench
        // window, and no client window close can drive the service shutdown coordinator.
        // 两种桌面启动都只保留托盘事件循环：服务不拥有工作台窗口，
        // 任何客户端窗口关闭都不会驱动服务关闭协调器。
        #[cfg(feature = "desktop-tray")]
        launch::ServiceLaunchMode::Service | launch::ServiceLaunchMode::GuiLaunch => {
            tray::run_with_tray()
        }
        #[cfg(not(feature = "desktop-tray"))]
        launch::ServiceLaunchMode::Service | launch::ServiceLaunchMode::GuiLaunch => {
            eprintln!(
                "[vulcan-code-service] this headless build was compiled without desktop-tray; use --headless"
            );
            std::process::exit(2);
        }
    }
}
