//! Command line of the setup program (plan section 18.6).
//! 安装器的命令行（方案 18.6 节）。

use std::path::PathBuf;

use vulcan_install_core::fetch::Route;
use vulcan_install_core::names::{Channel, ComponentName};

/// Usage text printed by `--help` and after an argument error.
/// `--help` 及参数错误后打印的用法说明。
pub const USAGE: &str = "\
usage: vulcan-code-setup [install] [--yes] [--gui | --no-gui] [--with browser]
                         [--menu | --no-menu] [--desktop-shortcut | --no-desktop-shortcut]
                         [--path | --no-path] [--service | --no-service] [--linger | --no-linger]
                         [--port-range <first>-<last> | --port-range default]
                         [--channel stable|preview] [--route auto|international|china]
                         [--prefix <dir>] [--self-update | --no-self-update]
       vulcan-code-setup update [--yes] [--prefix <dir>] [--route auto|international|china]
                         [--self-update | --no-self-update]
       vulcan-code-setup uninstall [--yes] [--purge] [--prefix <dir>]
       vulcan-code-setup status [--prefix <dir>]
       vulcan-code-setup --version

install    installs Vulcan Code, or refreshes and updates an existing installation; choices not
           given keep the ones recorded at installation
update     installs the newest version with the choices recorded at installation
uninstall  removes everything the setup program created; --purge also removes ~/.vulcan
status     shows what is installed and how to uninstall it

--yes        runs without asking; required when there is no terminal
--gui        also installs the desktop app (on Linux it needs a desktop environment); --no-gui
             installs the terminal client and the service only. The default is the desktop app
             where a graphical session runs
--with       adds an optional component; `browser` provides the built-in browser
--menu       a Start menu or application menu entry for the desktop app (default with --gui)
--desktop-shortcut
             a desktop shortcut for the desktop app (Windows and Linux; off by default)
--path       puts the installed command shortcuts on PATH (default)
--service    starts the Core service with your login session, as your own user, so no
             administrator rights are needed (off by default)
--linger     Linux: keeps the service running after you log out (loginctl enable-linger)
--port-range ports the service may listen on, such as 18000-18029; `default` restores the
             built-in range; the service uses them from its next start
--channel    stable (default) or preview
--route      download origins: auto, international, or china; when omitted, an interactive run
             offers a choice, otherwise the recorded route or auto is used
--prefix     installation directory; the platform default is used when omitted
--self-update     updates and restarts this setup program automatically (default)
--no-self-update  skips looking for a newer setup program
--mode, --no-shortcut
             kept from setup 0.1: --mode desktop, terminal or service means --gui, --no-gui, or
             --no-gui --service; --no-shortcut means --no-menu

To uninstall, run `vulcan-code-setup uninstall`, which is also in <install directory>/setup;
on Windows, Settings > Apps > Vulcan Code does the same.";

/// What the setup program was asked to do.
/// 安装器被要求执行的操作。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Install, or refresh and update an existing installation.
    /// 安装，或刷新并更新已有安装。
    Install(InstallOptions),
    /// Install the newest version with the recorded choices.
    /// 以记录的选择安装最新版本。
    Update(CommonOptions),
    /// Remove the installation.
    /// 移除安装。
    Uninstall(UninstallOptions),
    /// Show the installation.
    /// 显示安装。
    Status(CommonOptions),
    /// Print the usage.
    /// 打印用法。
    Help,
    /// Print the version of the setup program.
    /// 打印安装器的版本。
    Version,
}

/// Whether `install` and `update` first replace the setup program with a newer published one.
/// `install` 与 `update` 是否先把安装器替换为已发布的更新版本。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SelfUpdate {
    /// Update without asking.
    /// 不询问直接更新。
    #[default]
    Always,
    /// Do not look for a newer setup program.
    /// 不查找更新的安装器。
    Never,
}

/// Shared command options; some are only accepted by `install` and `update`.
/// 通用命令选项，其中部分选项仅供 `install` 与 `update` 使用。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommonOptions {
    /// Run without asking.
    /// 不询问直接执行。
    pub yes: bool,
    /// Installation directory, when not the platform default.
    /// 安装目录（不使用平台默认值时）。
    pub prefix: Option<PathBuf>,
    /// Whether to update the setup program first; only `install` and `update` accept a choice.
    /// 是否先更新安装器；只有 `install` 与 `update` 接受此选择。
    pub self_update: SelfUpdate,
    /// Download route for `install` and `update`; omitted to use the recorded choice or default.
    /// `install` 和 `update` 的下载线路；省略时沿用记录的选择或默认值。
    pub route: Option<Route>,
}

/// Options of `install`; `None` keeps the recorded choice or the default.
/// `install` 的选项；`None` 表示沿用记录的选择或默认值。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InstallOptions {
    /// Options every command accepts.
    /// 每个命令都接受的选项。
    pub common: CommonOptions,
    /// Whether to install the desktop app.
    /// 是否安装桌面应用。
    pub gui: Option<bool>,
    /// Optional components to add.
    /// 要添加的可选组件。
    pub with: Vec<ComponentName>,
    /// Release channel.
    /// 发布通道。
    pub channel: Option<Channel>,
    /// Whether to create the menu entry or application bundle.
    /// 是否创建菜单项或应用程序包。
    pub menu: Option<bool>,
    /// Whether to create the desktop shortcut.
    /// 是否创建桌面快捷方式。
    pub desktop_shortcut: Option<bool>,
    /// Whether to put the commands on PATH.
    /// 是否把命令加入 PATH。
    pub path: Option<bool>,
    /// Whether to start the Core service with the login session.
    /// 是否让 Core 服务随登录会话启动。
    pub service: Option<bool>,
    /// Whether the service keeps running after logout (Linux).
    /// 服务是否在注销后继续运行（Linux）。
    pub linger: Option<bool>,
    /// The port range to set, or `default`; checked by the Core service.
    /// 要设置的端口范围或 `default`；由 Core 服务校验。
    pub port_range: Option<String>,
}

/// Options of `uninstall`.
/// `uninstall` 的选项。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UninstallOptions {
    /// Options every command accepts.
    /// 每个命令都接受的选项。
    pub common: CommonOptions,
    /// Also remove the user data in `~/.vulcan`.
    /// 同时删除 `~/.vulcan` 中的用户数据。
    pub purge: bool,
}

/// The built-in browser component (`scripts/release-components.json`).
/// 内置浏览器组件（`scripts/release-components.json`）。
pub const BROWSER_COMPONENT: &str = "browser";

/// Components that `--with` may add; the desktop app has `--gui`, the others are always installed.
/// `--with` 可以添加的组件；桌面应用使用 `--gui`，其余组件总是安装。
pub const OPTIONAL_COMPONENTS: [&str; 1] = [BROWSER_COMPONENT];

/// Value of `--port-range` that restores the built-in range.
/// 恢复内置范围的 `--port-range` 值。
pub const DEFAULT_PORT_RANGE: &str = "default";

/// Sets a choice once; a second flag for the same choice is refused.
/// 只设置一次选择；同一选择的第二个参数被拒绝。
fn choose(slot: &mut Option<bool>, value: bool, flag: &str) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("{flag} repeats or contradicts an earlier option"));
    }
    *slot = Some(value);
    Ok(())
}

/// Checks the form of a `--port-range` value: `default`, one port, or `<first>-<last>`; the
/// Core service checks the numbers.
/// 检查 `--port-range` 值的形式：`default`、单个端口或 `<first>-<last>`；数值由 Core 服务检查。
pub fn check_port_range(value: &str) -> Result<(), String> {
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    let valid = value == DEFAULT_PORT_RANGE
        || match value.split_once('-') {
            Some((first, last)) => digits(first) && digits(last),
            None => digits(value),
        };
    if valid {
        Ok(())
    } else {
        Err(format!(
            "--port-range accepts <first>-<last>, one port, or {DEFAULT_PORT_RANGE}, not {value}"
        ))
    }
}

/// Parses the arguments after the program name.
/// 解析程序名之后的参数。
pub fn parse(arguments: &[String]) -> Result<Command, String> {
    let mut rest = arguments.iter().map(String::as_str).peekable();
    let verb = match rest.peek() {
        Some(word) if !word.starts_with('-') => {
            let verb = *word;
            rest.next();
            verb
        }
        _ => "install",
    };
    let mut install = InstallOptions::default();
    let mut purge = false;
    // Keep explicit switches distinct from the default so status and uninstall still reject them.
    // 区分显式开关与默认值，确保 status 和 uninstall 仍拒绝这两个选项。
    let mut self_update = None;
    while let Some(argument) = rest.next() {
        let (flag, inline) = match argument.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value)),
            _ => (argument, None),
        };
        let mut value = |name: &str| -> Result<String, String> {
            match inline {
                Some(value) => Ok(value.to_owned()),
                None => rest
                    .next()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("{name} needs a value")),
            }
        };
        let switch_only = |flag: &str| match inline {
            Some(_) => Err(format!("{flag} takes no value")),
            None => Ok(()),
        };
        // A switch pair: `--name` turns the choice on and `--no-name` turns it off.
        // 开关对：`--name` 打开选择，`--no-name` 关闭选择。
        let on = !flag.starts_with("--no-");
        match flag {
            "-h" | "--help" => return Ok(Command::Help),
            "-V" | "--version" => return Ok(Command::Version),
            "--self-update" | "--no-self-update" => {
                switch_only(flag)?;
                choose(&mut self_update, on, flag)?;
            }
            "-y" | "--yes" => {
                switch_only(flag)?;
                install.common.yes = true;
            }
            "--prefix" => install.common.prefix = Some(PathBuf::from(value(flag)?)),
            "--gui" | "--no-gui" => {
                switch_only(flag)?;
                choose(&mut install.gui, on, flag)?;
            }
            "--menu" | "--no-menu" => {
                switch_only(flag)?;
                choose(&mut install.menu, on, flag)?;
            }
            "--no-shortcut" => {
                switch_only(flag)?;
                choose(&mut install.menu, false, flag)?;
            }
            "--desktop-shortcut" | "--no-desktop-shortcut" => {
                switch_only(flag)?;
                choose(&mut install.desktop_shortcut, on, flag)?;
            }
            "--path" | "--no-path" => {
                switch_only(flag)?;
                choose(&mut install.path, on, flag)?;
            }
            "--service" | "--no-service" => {
                switch_only(flag)?;
                choose(&mut install.service, on, flag)?;
            }
            "--linger" | "--no-linger" => {
                switch_only(flag)?;
                choose(&mut install.linger, on, flag)?;
            }
            "--mode" => match value(flag)?.as_str() {
                "desktop" => choose(&mut install.gui, true, flag)?,
                "terminal" => choose(&mut install.gui, false, flag)?,
                "service" => {
                    choose(&mut install.gui, false, flag)?;
                    choose(&mut install.service, true, flag)?;
                }
                _ => return Err(String::from("--mode accepts desktop, terminal or service")),
            },
            "--port-range" => {
                if install.port_range.is_some() {
                    return Err(String::from("--port-range is given more than once"));
                }
                let range = value(flag)?;
                check_port_range(&range)?;
                install.port_range = Some(range);
            }
            "--with" => {
                let name = value(flag)?;
                if !OPTIONAL_COMPONENTS.contains(&name.as_str()) {
                    return Err(format!(
                        "--with accepts {}, not {name}; the desktop app is chosen with --gui",
                        OPTIONAL_COMPONENTS.join(", ")
                    ));
                }
                let component = ComponentName::new(name).map_err(|error| error.to_string())?;
                if !install.with.contains(&component) {
                    install.with.push(component);
                }
            }
            "--channel" => {
                install.channel = Some(
                    value(flag)?
                        .parse()
                        .map_err(|_| String::from("--channel accepts stable or preview"))?,
                );
            }
            "--route" => install.common.route = Some(parse_route(&value(flag)?)?),
            "--purge" => {
                switch_only(flag)?;
                purge = true;
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let install_only = install.gui.is_some()
        || !install.with.is_empty()
        || install.channel.is_some()
        || install.menu.is_some()
        || install.desktop_shortcut.is_some()
        || install.path.is_some()
        || install.service.is_some()
        || install.linger.is_some()
        || install.port_range.is_some();
    if self_update == Some(false) {
        install.common.self_update = SelfUpdate::Never;
    }
    match verb {
        "install" if !purge => Ok(Command::Install(install)),
        "update" if !install_only && !purge => Ok(Command::Update(install.common)),
        "status"
            if !install_only
                && !purge
                && self_update.is_none()
                && install.common.route.is_none() =>
        {
            Ok(Command::Status(install.common))
        }
        "uninstall" if !install_only && self_update.is_none() && install.common.route.is_none() => {
            Ok(Command::Uninstall(UninstallOptions {
                common: install.common,
                purge,
            }))
        }
        "install" | "update" | "status" | "uninstall" => {
            Err(format!("an option given is not accepted by {verb}"))
        }
        other => Err(format!("unknown command {other}")),
    }
}

/// Whether a standalone option consumes the following argument in `parse`.
/// 独立选项在 `parse` 中是否使用紧随其后的参数作为值。
pub(crate) fn takes_separate_value(argument: &str) -> bool {
    matches!(
        argument,
        "--prefix" | "--mode" | "--port-range" | "--with" | "--channel" | "--route"
    )
}

/// Parses a `--route` value.
/// 解析 `--route` 的值。
fn parse_route(value: &str) -> Result<Route, String> {
    match value {
        "auto" => Ok(Route::Auto),
        "international" => Ok(Route::International),
        "china" => Ok(Route::China),
        _ => Err(String::from("--route accepts auto, international or china")),
    }
}

/// Returns the CLI value for `route` so a restarted installer can repeat the same choice.
/// 返回 `route` 对应的命令行值，让重启后的安装器沿用同一选择。
pub(crate) const fn route_name(route: Route) -> &'static str {
    match route {
        Route::Auto => "auto",
        Route::International => "international",
        Route::China => "china",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses words separated by spaces.
    /// 解析以空格分隔的单词。
    fn parse_words(line: &str) -> Result<Command, String> {
        parse(
            &line
                .split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>(),
        )
    }

    /// The install options of `line`.
    /// `line` 的安装选项。
    fn install(line: &str) -> InstallOptions {
        match parse_words(line) {
            Ok(Command::Install(options)) => options,
            other => panic!("install expected, got {other:?}"),
        }
    }

    #[test]
    fn install_is_the_default_command_and_values_may_be_inline() {
        let options = install(
            "--yes --gui --with browser --channel preview --prefix /opt/v --service \
             --no-menu --desktop-shortcut --port-range=18000-18029",
        );
        assert!(options.common.yes);
        assert_eq!(options.common.self_update, SelfUpdate::Always);
        assert_eq!(options.gui, Some(true));
        assert_eq!(options.with.len(), 1);
        assert_eq!(options.channel, Some(Channel::Preview));
        assert_eq!(options.common.prefix, Some(PathBuf::from("/opt/v")));
        assert_eq!(options.service, Some(true));
        assert_eq!(options.menu, Some(false));
        assert_eq!(options.desktop_shortcut, Some(true));
        assert_eq!(options.port_range.as_deref(), Some("18000-18029"));
        assert_eq!(options.path, None);
    }

    #[test]
    fn options_of_setup_0_1_keep_their_meaning() {
        let service = install("--mode service --no-shortcut");
        assert_eq!(
            (service.gui, service.service, service.menu),
            (Some(false), Some(true), Some(false))
        );
        assert_eq!(install("--mode=desktop").gui, Some(true));
        assert_eq!(install("--mode terminal").gui, Some(false));
        assert!(parse_words("--mode desktop --no-gui").is_err());
        assert!(parse_words("--mode cloud").is_err());
    }

    #[test]
    fn port_ranges_are_checked_for_form_only() {
        assert!(check_port_range("18000-18029").is_ok());
        assert!(check_port_range("18000").is_ok());
        assert!(check_port_range(DEFAULT_PORT_RANGE).is_ok());
        assert!(check_port_range("18000-").is_err());
        assert!(check_port_range("a-b").is_err());
        assert!(parse_words("--port-range 1-2 --port-range 3-4").is_err());
    }

    #[test]
    fn options_of_another_command_are_refused() {
        assert!(parse_words("uninstall --gui").is_err());
        assert!(parse_words("update --service").is_err());
        assert!(parse_words("status --purge").is_err());
        assert!(parse_words("install --purge").is_err());
        assert!(parse_words("--with core").is_err());
        assert!(parse_words("--with gui").is_err());
        assert!(parse_words("--yes=1").is_err());
        assert!(parse_words("--gui=1").is_err());
        assert!(parse_words("--mode").is_err());
        assert!(parse_words("--gui --no-gui").is_err());
        assert!(parse_words("repair").is_err());
        assert!(parse_words("status --self-update").is_err());
        assert!(parse_words("status --route china").is_err());
        assert!(parse_words("uninstall --route china").is_err());
        assert!(parse_words("--self-update --no-self-update").is_err());
        let Ok(Command::Update(options)) = parse_words("update --no-self-update --route=china")
        else {
            panic!("update expected");
        };
        assert_eq!(options.self_update, SelfUpdate::Never);
        assert_eq!(options.route, Some(Route::China));
        assert_eq!(
            parse_words("uninstall --purge --yes"),
            Ok(Command::Uninstall(UninstallOptions {
                common: CommonOptions {
                    yes: true,
                    prefix: None,
                    self_update: SelfUpdate::Always,
                    route: None,
                },
                purge: true
            }))
        );
    }
}
