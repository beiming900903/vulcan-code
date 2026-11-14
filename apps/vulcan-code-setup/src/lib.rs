//! The Vulcan Code setup program: installs, updates, modifies and removes a per-user installation
//! from the signed release objects, using the same `vulcan-install-core` as the in-app update
//! (plan section 18.6).
//! Vulcan Code 安装器：从已签名的发布对象安装、更新、修改与移除每用户安装，与应用内更新共用
//! `vulcan-install-core`（方案 18.6 节）。

pub mod args;
mod install;
mod integrate;
pub mod paths;
mod prompt;
mod report;
mod root;
mod screen;
mod selfupdate;
mod session;
#[cfg(unix)]
mod shell_path;
pub mod state;
mod status;
mod uninstall;
#[cfg(windows)]
mod windows;
mod wizard;

use args::{Command, USAGE};
use vulcan_install_core::names::{Channel, Platform};

/// The error a command returns when the user declines to continue or leaves the wizard; nothing was
/// changed, so it is reported as a cancellation, not as a failure.
/// 用户拒绝继续或离开向导时命令返回的错误；未做任何改动，因此报告为取消而非失败。
pub(crate) const CANCELLED: &str = "cancelled";

/// The exit code of a cancelled command, as a shell reports an interrupted program. scripts/install.ps1
/// recognizes it and ends quietly instead of reporting a failure.
/// 被取消命令的退出码，与 shell 报告被中断程序的方式相同。scripts/install.ps1 识别它并安静结束，而不是报告失败。
pub const CANCELLED_EXIT_CODE: i32 = 130;

/// Runs the setup program with the arguments after the program name; returns the exit code.
/// 以程序名之后的参数运行安装器；返回退出码。
pub fn run(arguments: &[String]) -> i32 {
    let mut command = match args::parse(arguments) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("vulcan-code-setup: {error}\n\n{USAGE}");
            return 2;
        }
    };
    // Resolve the node before the first network request, even when a newer installer takes over.
    // 在首次联网前确定节点，即使稍后由新版安装器接管也沿用该节点。
    let updating = match &mut command {
        Command::Install(options) => Some((&mut options.common, options.channel)),
        Command::Update(common) => Some((common, None)),
        _ => None,
    };
    if let Some((common, channel)) = updating {
        match prepare(common, channel, arguments) {
            Ok(selfupdate::Outcome::Continue) => {}
            Ok(selfupdate::Outcome::Finished(code)) => return code,
            Err(error) if error == CANCELLED => return cancelled(),
            Err(error) => {
                eprintln!("vulcan-code-setup: {error}");
                return 1;
            }
        }
    }
    let result = match command {
        Command::Help => {
            println!("{USAGE}");
            Ok(())
        }
        Command::Version => {
            println!("vulcan-code-setup {}", selfupdate::SETUP_VERSION);
            Ok(())
        }
        Command::Install(options) => install::install(&options),
        Command::Update(options) => install::update(&options),
        Command::Uninstall(options) => uninstall::uninstall(&options),
        Command::Status(options) => status::status(&options),
    };
    match result {
        Ok(()) => 0,
        Err(error) if error == CANCELLED => cancelled(),
        Err(error) => {
            eprintln!("vulcan-code-setup: {error}");
            1
        }
    }
}

/// Selects a route for `common` before HTTPS, then checks for a newer installer on `channel`.
/// 在 HTTPS 请求前为 `common` 选定线路，再按 `channel` 检查新版安装器。
fn prepare(
    common: &mut args::CommonOptions,
    channel: Option<Channel>,
    arguments: &[String],
) -> Result<selfupdate::Outcome, String> {
    let platform = Platform::current().ok_or("this platform is not supported")?;
    let (channel, initial_route) = install::preferences(common, channel, platform);
    // Pass an interactive choice through a self-update restart without asking twice.
    // 将交互选择传给自更新后重启的进程，避免重复询问。
    let mut next_arguments = arguments.to_vec();
    // Unattended runs use the resolved route; interactive runs can change the highlighted choice.
    // 无人值守使用已解析线路；交互运行可修改高亮选择。
    let route = if common.route.is_none() && wizard::wanted(common.yes) {
        // This selection was absent from the original arguments.
        // 原命令行参数中没有这次选择。
        let route = wizard::choose_route(initial_route)?;
        next_arguments.push(format!("--route={}", args::route_name(route)));
        route
    } else {
        initial_route
    };
    common.route = Some(route);
    selfupdate::check(
        platform,
        channel,
        route,
        common.self_update,
        &next_arguments,
    )
}

/// Prints the cancellation outcome and returns the exit code understood by the entry scripts.
/// 输出取消结果并返回入口脚本识别的退出码。
fn cancelled() -> i32 {
    println!("Cancelled; nothing was changed.");
    CANCELLED_EXIT_CODE
}
