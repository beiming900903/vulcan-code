//! Installing and updating: a first installation is an assembly from an empty root, so it uses the
//! same verified objects and code path as the in-app update (plan section 18.6).
//! 安装与更新：首次安装就是从空根目录开始的一次组装，因此与应用内更新使用同一批已校验对象和同一条
//! 代码路径（方案 18.6 节）。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use vulcan_install_core::assemble::{Assembly, PreparedAssembly};
use vulcan_install_core::fetch::{Downloader, Route};
use vulcan_install_core::index::ComponentIndex;
use vulcan_install_core::install_root::{CURRENT_LINK, InstallLock, InstallRoot, UPDATES_DIR};
use vulcan_install_core::manifest::ReleaseManifest;
use vulcan_install_core::names::{Channel, ComponentName, Platform, ReleaseVersion};
use vulcan_install_core::record::RECORD_FILE_NAME;
use vulcan_install_core::signing::BUILT_IN_KEYS;
use vulcan_install_core::update::{find_update, read_indexes};

use crate::args::{BROWSER_COMPONENT, CommonOptions, InstallOptions, check_port_range};
use crate::integrate::{self, CLI_DIR, Plan, Recorder, SETUP_PROGRAM};
use crate::paths;
use crate::report::{Action, BACK, Decision, DesktopWay, Outcome, Plain, Reporter, Review, Stage};
use crate::root::{self, Current, Description, LEGACY_MARKER, Ownership, Version};
use crate::screen::Screen;
use crate::selfupdate;
use crate::session;
use crate::state::{Item, SERVICE_PENDING_FILE, SETUP_DIR, STATE_SCHEMA, SetupState};
use crate::wizard::{self, Draft};

/// How often the download progress is reported.
/// 下载进度的报告间隔。
const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);

/// User agent of every request.
/// 每个请求的用户代理。
pub const USER_AGENT: &str = "vulcan-code-setup";

/// Oldest glibc the Linux release runs on: it is built on the `ubuntu-22.04` runner
/// (`.github/workflows/release.yml`), whose glibc is 2.35.
/// Linux 发行版可运行的最旧 glibc：它在 `ubuntu-22.04` 运行器上构建（`.github/workflows/release.yml`），
/// 该系统的 glibc 为 2.35。
const MIN_GLIBC: (u32, u32) = (2, 35);

/// Components every installation has, as named in `scripts/release-components.json`.
/// 每个安装都有的组件，名称见 `scripts/release-components.json`。
const BASE_COMPONENTS: [&str; 2] = ["core", "tui"];

/// The choices an installation is made with.
/// 安装所依据的选择。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choices {
    /// Whether the desktop app is installed.
    /// 是否安装桌面应用。
    pub gui: bool,
    /// Release channel.
    /// 发布通道。
    pub channel: Channel,
    /// Download origins.
    /// 下载源。
    pub route: Route,
    /// Optional components.
    /// 可选组件。
    pub with: Vec<ComponentName>,
    /// Whether a menu entry or application bundle is wanted for the desktop app.
    /// 是否需要桌面应用的菜单项或应用程序包。
    pub menu: bool,
    /// Whether a desktop shortcut is wanted for the desktop app.
    /// 是否需要桌面应用的桌面快捷方式。
    pub desktop_shortcut: bool,
    /// Whether commands are wanted on PATH.
    /// 是否需要把命令加入 PATH。
    pub path: bool,
    /// Whether the Core service starts with the login session.
    /// Core 服务是否随登录会话启动。
    pub service: bool,
    /// Whether the service keeps running after logout (Linux).
    /// 服务是否在注销后继续运行（Linux）。
    pub linger: bool,
    /// The port range to set in this run; not recorded, the service settings hold it.
    /// 本次要设置的端口范围；不记录，由服务设置保存。
    pub port_range: Option<String>,
}

/// Whether a graphical session runs, which decides whether the desktop app is preselected.
/// 是否有图形会话在运行，决定是否预选桌面应用。
pub fn graphical(platform: Platform) -> bool {
    platform.is_windows()
        || platform.is_macos()
        || std::env::var_os("DISPLAY").is_some()
        || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

impl Choices {
    /// Combines the options given with the choices recorded at the last installation, then the
    /// defaults; `graphical` preselects the desktop app.
    /// 将给出的选项与上次安装记录的选择合并，再取默认值；`graphical` 决定是否预选桌面应用。
    pub fn resolve(
        options: &InstallOptions,
        recorded: Option<&SetupState>,
        platform: Platform,
        graphical: bool,
    ) -> Result<Self, String> {
        let pick = |given: Option<bool>, recorded_value: fn(&SetupState) -> bool, default: bool| {
            given.or(recorded.map(recorded_value)).unwrap_or(default)
        };
        let gui = pick(options.gui, |state| state.gui, graphical);
        if platform.is_macos() && options.desktop_shortcut == Some(true) {
            return Err(String::from(
                "--desktop-shortcut is not available on macOS; the app is placed in Applications",
            ));
        }
        let desktop_shortcut = !platform.is_macos()
            && pick(
                options.desktop_shortcut,
                |state| state.desktop_shortcut,
                false,
            );
        let service = pick(options.service, |state| state.service, false);
        let linger = pick(options.linger, |state| state.linger, false);
        if linger && (platform.is_windows() || platform.is_macos()) {
            return Err(String::from("--linger is available on Linux only"));
        }
        if linger && !service {
            return Err(String::from(
                "--linger keeps the service running, so it needs --service",
            ));
        }
        if let Some(range) = &options.port_range {
            check_port_range(range)?;
        }
        let mut with = recorded.map(|state| state.with.clone()).unwrap_or_default();
        for component in &options.with {
            if !with.contains(component) {
                with.push(component.clone());
            }
        }
        Ok(Self {
            gui,
            channel: options
                .channel
                .or(recorded.map(|state| state.channel))
                .unwrap_or(Channel::Stable),
            route: options
                .common
                .route
                .or(recorded.map(|state| state.route))
                .unwrap_or_default(),
            with,
            menu: pick(options.menu, |state| state.menu, true),
            desktop_shortcut,
            path: pick(options.path, |state| state.path, true),
            service,
            linger,
            port_range: options.port_range.clone(),
        })
    }

    /// The components this choice installs.
    /// 此选择安装的组件。
    pub fn components(&self) -> Result<BTreeSet<ComponentName>, String> {
        let mut names: Vec<&str> = BASE_COMPONENTS.to_vec();
        if self.gui {
            names.push(vulcan_install_core::names::DESKTOP_COMPONENT);
        }
        let mut components = BTreeSet::new();
        for name in names {
            components.insert(ComponentName::new(name).map_err(|error| error.to_string())?);
        }
        components.extend(self.with.iter().cloned());
        Ok(components)
    }

    /// The channels whose pointers are read.
    /// 读取其指针的通道。
    fn channels(&self) -> &'static [Channel] {
        channels(self.channel)
    }
}

/// The channels whose pointers are read for `channel`: preview also follows a newer stable release.
/// 为 `channel` 读取其指针的通道：preview 同时跟随更新的 stable 版本。
pub fn channels(channel: Channel) -> &'static [Channel] {
    match channel {
        Channel::Stable => &[Channel::Stable],
        Channel::Preview => &[Channel::Stable, Channel::Preview],
    }
}

/// The channel and origins `install` or `update` will use, for looking up a newer setup program
/// before anything else: the options given, then the recorded choices, then the defaults.
/// `install` 或 `update` 将使用的通道与下载源，用于在其他步骤之前查找更新的安装器：依次取给出的选项、
/// 记录的选择与默认值。
pub fn preferences(
    common: &CommonOptions,
    channel: Option<Channel>,
    platform: Platform,
) -> (Channel, Route) {
    let recorded = resolve_root(common.prefix.as_deref(), platform)
        .and_then(|root| root::ownership(&root))
        .ok()
        .and_then(|ownership| match ownership {
            Ownership::Owned(state) => Some(state),
            _ => None,
        });
    (
        channel
            .or(recorded.as_ref().map(|state| state.channel))
            .unwrap_or(Channel::Stable),
        common
            .route
            .or(recorded.as_ref().map(|state| state.route))
            .unwrap_or_default(),
    )
}

/// `vulcan-code-setup install`.
/// `vulcan-code-setup install`。
pub fn install(options: &InstallOptions) -> Result<(), String> {
    let platform = Platform::current().ok_or("this platform is not supported")?;
    check_system(platform)?;
    let root = resolve_root(options.common.prefix.as_deref(), platform)?;
    let ownership = root::installable(&root)?;
    let recorded = match &ownership {
        Ownership::Owned(state) => Some(state),
        _ => None,
    };
    let start = Choices::resolve(options, recorded, platform, graphical(platform))?;
    if !wizard::wanted(options.common.yes) {
        let reporter = Plain::new(options.common.yes);
        return apply(platform, &root, &ownership, &start, &reporter)
            .map(|outcome| outcome.print());
    }
    // The wizard starts from the options given, then the recorded choices, then the defaults.
    // 向导依次以给出的选项、记录的选择与默认值为起点。
    let browser = ComponentName::new(BROWSER_COMPONENT).map_err(|error| error.to_string())?;
    let draft = Draft {
        gui: start.gui,
        browser: start.with.contains(&browser),
        root: root.display().to_string(),
        menu: start.menu,
        desktop_shortcut: start.desktop_shortcut,
        path: start.path,
        service: start.service,
        linger: start.linger,
        channel: start.channel,
        route: start.route,
        port_range: start.port_range.clone().unwrap_or_default(),
    };
    // The terminal stays in the interactive installer from the wizard to the last page; the outcome
    // is printed once it is given back.
    // 终端从向导到最后一页都保持在交互式安装界面中；归还终端后再打印结果。
    let mut screen = Screen::open()?;
    let outcome = interactive(&mut screen, platform, draft, browser);
    drop(screen);
    outcome.map(|outcome| outcome.print())
}

/// The wizard, then the run with its pages; going back from the review returns to the wizard's
/// summary.
/// 先运行向导，再运行带各页的安装；从审阅返回时回到向导的汇总屏。
fn interactive(
    screen: &mut Screen,
    platform: Platform,
    mut draft: Draft,
    browser: ComponentName,
) -> Result<Outcome, String> {
    let mut at_summary = false;
    loop {
        let Some(chosen) = wizard::run(screen, platform, draft, at_summary)? else {
            return Err(String::from(crate::CANCELLED));
        };
        draft = chosen;
        let root = resolve_root(Some(Path::new(draft.root.trim())), platform)?;
        let ownership = root::installable(&root)?;
        let port_range = Some(draft.port_range.trim().to_owned()).filter(|range| !range.is_empty());
        let choices = Choices {
            gui: draft.gui,
            channel: draft.channel,
            route: draft.route,
            with: if draft.browser {
                vec![browser.clone()]
            } else {
                Vec::new()
            },
            menu: draft.menu,
            desktop_shortcut: draft.desktop_shortcut,
            path: draft.path,
            service: draft.service,
            linger: draft.linger,
            port_range,
        };
        match session::run(screen, true, |reporter| {
            apply(platform, &root, &ownership, &choices, reporter)
        }) {
            Err(error) if error == BACK => at_summary = true,
            result => return result,
        }
    }
}

/// `vulcan-code-setup update`.
/// `vulcan-code-setup update`。
pub fn update(options: &CommonOptions) -> Result<(), String> {
    let platform = Platform::current().ok_or("this platform is not supported")?;
    check_system(platform)?;
    let root = resolve_root(options.prefix.as_deref(), platform)?;
    let ownership = root::installable(&root)?;
    let Ownership::Owned(state) = &ownership else {
        return Err(format!(
            "{} was not installed by the setup program; run `vulcan-code-setup install`",
            root.display()
        ));
    };
    let choices = Choices::resolve(
        &InstallOptions {
            common: options.clone(),
            ..InstallOptions::default()
        },
        Some(state),
        platform,
        false,
    )?;
    if !wizard::wanted(options.yes) {
        let reporter = Plain::new(options.yes);
        return apply(platform, &root, &ownership, &choices, &reporter)
            .map(|outcome| outcome.print());
    }
    let mut screen = Screen::open()?;
    let outcome = session::run(&mut screen, false, |reporter| {
        apply(platform, &root, &ownership, &choices, reporter)
    });
    drop(screen);
    outcome.map(|outcome| outcome.print())
}

/// The install root: `--prefix` or the platform default, absolute and usable in launchers.
/// 安装根目录：`--prefix` 或平台默认值，为绝对路径且可用于启动器。
pub fn resolve_root(prefix: Option<&Path>, platform: Platform) -> Result<PathBuf, String> {
    let root = match prefix {
        Some(prefix) => paths::absolute(prefix)?,
        None => paths::default_root(platform)?,
    };
    if !platform.is_windows() {
        let text = root.to_str().ok_or_else(|| {
            format!(
                "the installation directory must be valid UTF-8: {}",
                root.display()
            )
        })?;
        if !embeddable(text) {
            return Err(format!(
                "the installation directory must not contain quotes, dollar signs, backquotes or \
                 backslashes: {text}"
            ));
        }
    }
    Ok(root)
}

/// Whether `text` can be embedded in double quotes in launchers and menu entries.
/// `text` 能否嵌入启动器与菜单项的双引号中。
fn embeddable(text: &str) -> bool {
    !text.contains(['"', '$', '`', '\\', '\n'])
}

/// Refuses systems the release cannot run on before anything is downloaded.
/// 在下载任何内容之前拒绝发行版无法运行的系统。
fn check_system(platform: Platform) -> Result<(), String> {
    if platform.is_windows() || platform.is_macos() {
        return Ok(());
    }
    let output = std::process::Command::new("getconf")
        .arg("GNU_LIBC_VERSION")
        .output();
    let found = output
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| parse_glibc(&String::from_utf8_lossy(&output.stdout)));
    let (major, minor) = MIN_GLIBC;
    match found {
        Some(version) if version >= MIN_GLIBC => Ok(()),
        Some((found_major, found_minor)) => Err(format!(
            "Vulcan Code needs glibc {major}.{minor} or newer; this system has {found_major}.\
             {found_minor}"
        )),
        None => Err(format!(
            "Vulcan Code needs glibc {major}.{minor} or newer, and this system does not report \
             one (musl-based systems such as Alpine are not supported)"
        )),
    }
}

/// Parses the output of `getconf GNU_LIBC_VERSION`, such as `glibc 2.35`.
/// 解析 `getconf GNU_LIBC_VERSION` 的输出，例如 `glibc 2.35`。
fn parse_glibc(output: &str) -> Option<(u32, u32)> {
    let version = output.trim().strip_prefix("glibc ")?;
    let mut parts = version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

/// What an installation run does with the versions.
/// 一次安装运行对版本所做的事。
enum Work {
    /// Assemble and switch to a newer version.
    /// 组装并切换到更新的版本。
    Assemble {
        /// Its verified manifest.
        /// 其已校验的清单。
        manifest: Box<ReleaseManifest>,
        /// Indexes of the chosen components.
        /// 所选组件的索引。
        indexes: Vec<ComponentIndex>,
    },
    /// Keep the current version and refresh the integration.
    /// 保留当前版本并刷新集成。
    Refresh(ReleaseVersion),
}

/// Installs or refreshes `root` with `choices`, reporting to `reporter`. Nothing is downloaded or
/// switched before the review, which shows the exact download, says to go on; going back or
/// cancelling leaves the root as it was found.
/// 以 `choices` 安装或刷新 `root`，并向 `reporter` 报告。审阅（显示确切的下载量）同意继续之前，不下载也
/// 不切换任何内容；返回或取消时根目录保持原样。
fn apply(
    platform: Platform,
    root_path: &Path,
    ownership: &Ownership,
    choices: &Choices,
    reporter: &dyn Reporter,
) -> Result<Outcome, String> {
    let root = InstallRoot::new(root_path);
    let current = if root_path.exists() {
        root::current(&root)?
    } else {
        Current::None
    };
    let installed = match &current {
        Current::Version(version) => Some(version.clone()),
        _ => None,
    };
    let components = choices.components()?;
    let floor = match &installed {
        Some(version) => version.clone(),
        None => "0.0.0"
            .parse::<ReleaseVersion>()
            .map_err(|error| error.to_string())?,
    };
    let mut downloader =
        Downloader::new(choices.route.origins(), USER_AGENT).map_err(|error| error.to_string())?;
    let cancel = downloader.cancel_flag();
    reporter.cancel_with(Arc::clone(&cancel));
    reporter.stage(Stage::Checking(choices.channel));
    let offer = find_update(
        &mut downloader,
        BUILT_IN_KEYS,
        platform,
        choices.channels(),
        &floor,
    )
    .map_err(|error| failure(&cancel, error))?;
    let work = match (offer, &installed) {
        (Some(offer), _) => {
            let own = selfupdate::version()?;
            if let Some(minimum) = offer
                .manifest
                .min_setup_version
                .as_ref()
                .filter(|minimum| **minimum > own)
            {
                return Err(format!(
                    "{} needs setup program {minimum} or newer, and this one is {own}; update the \
                     setup program first (it updates itself unless --no-self-update is given)",
                    offer.manifest.version
                ));
            }
            let indexes = read_indexes(&mut downloader, &offer.manifest, &components)
                .map_err(|error| failure(&cancel, error))?;
            Work::Assemble {
                manifest: Box::new(offer.manifest),
                indexes,
            }
        }
        (None, Some(version)) => {
            let present = root::installed_components(&root, version)?;
            if present != components {
                return Err(format!(
                    "{version} is installed with {}; it cannot change to {} until a newer version \
                     is published, or after `vulcan-code-setup uninstall`",
                    join(&present),
                    join(&components)
                ));
            }
            Work::Refresh(version.clone())
        }
        (None, None) => {
            return Err(format!(
                "no release is published for {} on the {} channel",
                platform.as_str(),
                choices.channel.as_str()
            ));
        }
    };
    let version = match &work {
        Work::Assemble { manifest, .. } => manifest.version.clone(),
        Work::Refresh(version) => version.clone(),
    };
    let version_text = version.to_string();
    let plan = Plan {
        platform,
        root: root_path,
        version: &version_text,
        gui: choices.gui,
        menu: choices.menu,
        desktop_shortcut: choices.desktop_shortcut,
        path: choices.path,
        service: choices.service,
        linger: choices.linger,
        port_range: choices.port_range.as_deref(),
    };
    let action = match (&work, &installed) {
        (Work::Refresh(_), _) => Action::Refresh,
        (Work::Assemble { .. }, Some(from)) => Action::Update {
            from: from.to_string(),
        },
        (Work::Assemble { .. }, None) => Action::Install,
    };
    let review = Review {
        platform,
        action: action.clone(),
        version: version_text.clone(),
        channel: choices.channel,
        components: components.iter().cloned().collect(),
        directory: root_path.to_path_buf(),
        download: 0,
        installed_size: match (&work, &installed) {
            (Work::Assemble { indexes, .. }, None) => {
                Some(indexes.iter().map(ComponentIndex::installed_size).sum())
            }
            _ => None,
        },
        locations: plan.locations()?,
        port_range: choices.port_range.clone(),
    };

    // What this run creates before the switch is removed again when the review is declined.
    // 审阅被拒绝时，移除本次运行在切换之前创建的内容。
    let undo = Undo::note(&root, &version);
    std::fs::create_dir_all(root_path)
        .map_err(|error| format!("cannot create {}: {error}", root_path.display()))?;
    let lock = root.lock().map_err(|error| error.to_string())?;
    let switched = match assemble(
        &root,
        &lock,
        work,
        &version,
        review,
        reporter,
        &mut downloader,
        &cancel,
    ) {
        Ok(switched) => switched,
        Err(error) => {
            drop(lock);
            if error == crate::CANCELLED || error == BACK {
                undo.run();
            }
            return Err(error);
        }
    };
    if *ownership == Ownership::Legacy {
        let _ = std::fs::remove_file(root_path.join(LEGACY_MARKER));
    }

    reporter.stage(Stage::Integrating);
    let installed_version = Version::read(&root.version_dir(&version))?;
    let previous_items: &[Item] = match ownership {
        Ownership::Owned(state) => &state.items,
        _ => &[],
    };
    let mut recorder = Recorder::new(previous_items);
    if switched && plan.service {
        // Marked before anything can fail, so that a repeated run registers the new version.
        // 在任何步骤可能失败之前标记，使重跑会注册新版本。
        let dir = root_path.join(SETUP_DIR);
        std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::write(dir.join(SERVICE_PENDING_FILE), b""))
            .map_err(|error| format!("cannot write {}: {error}", dir.display()))?;
    }
    // The setup program is copied first, so the service registered below never outlives it.
    // 先复制安装器，使下面注册的服务绝不会比安装器存在得更久。
    let integrated = copy_self(root_path).and_then(|()| {
        integrate::integrate(&plan, &installed_version, switched, &mut recorder, reporter)
    });
    let mut state = SetupState {
        schema: STATE_SCHEMA,
        channel: choices.channel,
        route: choices.route,
        gui: choices.gui,
        with: choices.with.clone(),
        menu: choices.menu,
        desktop_shortcut: choices.desktop_shortcut,
        path: choices.path,
        service: choices.service,
        linger: choices.linger,
        items: Vec::new(),
    };
    // Items of an earlier run that this run did not create are removed; when this run failed they
    // are kept so that uninstalling still finds them.
    // 更早一次运行创建、而本次未创建的条目会被移除；本次失败时保留它们，使卸载仍能找到。
    let mut leftovers = Vec::new();
    for item in previous_items {
        if !recorder.items.contains(item) {
            if integrated.is_ok() {
                leftovers.push(item.clone());
            } else {
                state.add(item.clone());
            }
        }
    }
    for item in &recorder.items {
        state.add(item.clone());
    }
    let mut problems = Vec::new();
    for item in leftovers.iter().rev() {
        if let Err(error) = integrate::remove(item, root_path, Some(&installed_version), reporter) {
            problems.push(error);
            state.add(item.clone());
        }
    }
    state.write(root_path)?;
    integrated?;

    reporter.stage(Stage::Cleaning);
    let removed = root
        .prune_versions(&lock, &version)
        .map_err(|error| error.to_string())?;
    let keep = match &current {
        Current::Legacy(name) if switched => Some(name.as_str()),
        _ => None,
    };
    root::remove_legacy_generations(&root, keep);
    drop(lock);

    if !choices.gui {
        integrate::headless_notes(&plan, &installed_version, &mut recorder);
    }
    Ok(Outcome {
        platform,
        action,
        version: version_text,
        directory: root_path.to_path_buf(),
        terminal: terminal_command(platform, root_path, choices.path, &installed_version),
        desktop: desktop_way(
            platform,
            root_path,
            choices,
            &recorder.items,
            &installed_version,
        ),
        setup: setup_command(root_path, choices.path),
        removed: removed.iter().map(ToString::to_string).collect(),
        browser_dependencies: choices
            .with
            .iter()
            .any(|name| name.as_str() == BROWSER_COMPONENT)
            && !platform.is_windows()
            && !platform.is_macos(),
        notes: recorder.notes.iter().chain(&problems).cloned().collect(),
    })
}

/// Prepares the assembly of `work`, asks for `review` with the exact download, then downloads and
/// switches to `version`; returns whether the current version changed.
/// 准备 `work` 的组装，以确切的下载量请求 `review`，然后下载并切换到 `version`；返回当前版本是否改变。
#[allow(clippy::too_many_arguments)]
fn assemble(
    root: &InstallRoot,
    lock: &InstallLock,
    work: Work,
    version: &ReleaseVersion,
    mut review: Review,
    reporter: &dyn Reporter,
    downloader: &mut Downloader,
    cancel: &AtomicBool,
) -> Result<bool, String> {
    let (manifest, indexes) = match work {
        Work::Refresh(_) => {
            decide(reporter, &review)?;
            return Ok(false);
        }
        Work::Assemble { manifest, indexes } => (manifest, indexes),
    };
    let version_dir = root.version_dir(version);
    // A version assembled by an earlier run that did not switch is complete already.
    // 更早一次未切换的运行已组装好的版本已经完整。
    let prepared = if version_dir.join(RECORD_FILE_NAME).is_file() {
        None
    } else {
        reporter.stage(Stage::Preparing);
        let prepared = Assembly::new(root, lock, &manifest, indexes)
            .and_then(Assembly::prepare)
            .map_err(|error| error.to_string())?;
        Some(prepared)
    };
    review.download = prepared
        .as_ref()
        .map_or(0, PreparedAssembly::download_bytes);
    decide(reporter, &review)?;
    if let Some(prepared) = prepared {
        reporter.stage(Stage::Downloading);
        let total = prepared.download_bytes();
        let counter = downloader.received_counter();
        let start = counter.load(Ordering::Relaxed);
        let done = AtomicBool::new(false);
        let result = std::thread::scope(|scope| {
            if total > 0 {
                scope.spawn(|| {
                    loop {
                        let finished = done.load(Ordering::Relaxed);
                        let received = counter.load(Ordering::Relaxed).saturating_sub(start);
                        reporter.progress(received.min(total), total);
                        if finished {
                            break;
                        }
                        std::thread::sleep(PROGRESS_INTERVAL);
                    }
                });
            }
            let result = prepared.complete(downloader);
            done.store(true, Ordering::Relaxed);
            result
        });
        result.map_err(|error| failure(cancel, error))?;
        if let Err(error) = check_release_build(&version_dir) {
            let _ = std::fs::remove_dir_all(&version_dir);
            return Err(error);
        }
    }
    reporter.stage(Stage::Switching);
    root.switch_to(lock, version)
        .map_err(|error| error.to_string())?;
    Ok(true)
}

/// Asks for `review`; going back and cancelling become their errors.
/// 请求 `review`；返回与取消转为各自的错误。
fn decide(reporter: &dyn Reporter, review: &Review) -> Result<(), String> {
    match reporter.review(review)? {
        Decision::Continue => Ok(()),
        Decision::Back => Err(String::from(BACK)),
        Decision::Cancel => Err(String::from(crate::CANCELLED)),
    }
}

/// The error of a transfer: a cancellation once `cancel` is set, `error` otherwise.
/// 传输的错误：`cancel` 已设置时为取消，否则为 `error`。
fn failure(cancel: &AtomicBool, error: impl std::fmt::Display) -> String {
    if cancel.load(Ordering::Relaxed) {
        String::from(crate::CANCELLED)
    } else {
        error.to_string()
    }
}

/// What was missing before a run created anything, so that a declined review leaves the root as it
/// was found. A failed run keeps what it downloaded, so the next run resumes.
/// 运行创建任何内容之前缺少的部分，使被拒绝的审阅让根目录保持原样。失败的运行保留已下载的内容，以便
/// 下一次运行续装。
struct Undo {
    /// The install root.
    /// 安装根目录。
    root: PathBuf,
    /// Whether the root itself was missing.
    /// 根目录本身是否缺失。
    created_root: bool,
    /// Missing directories inside the root, innermost first.
    /// 根目录内缺失的目录，最内层在前。
    created: Vec<PathBuf>,
}

impl Undo {
    /// Notes what is missing for assembling `version` in `root`.
    /// 记下在 `root` 中组装 `version` 所缺少的内容。
    fn note(root: &InstallRoot, version: &ReleaseVersion) -> Self {
        let path = root.path().to_path_buf();
        let created = [root.staging_dir(version), path.join(UPDATES_DIR)]
            .into_iter()
            .filter(|dir| !dir.exists())
            .collect();
        Self {
            created_root: !path.exists(),
            root: path,
            created,
        }
    }

    /// Removes what the run created.
    /// 移除本次运行创建的内容。
    fn run(self) {
        if self.created_root {
            let _ = std::fs::remove_dir_all(&self.root);
            return;
        }
        for dir in self.created {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// `path` in double quotes, as a command to type.
/// 置于双引号中的 `path`，作为要输入的命令。
fn quoted(path: &Path) -> String {
    format!("\"{}\"", path.display())
}

/// The command that starts the terminal client: its name on PATH, its full path otherwise.
/// 启动终端客户端的命令：已加入 PATH 时为其名称，否则为完整路径。
fn terminal_command(platform: Platform, root: &Path, on_path: bool, version: &Version) -> String {
    let program = if on_path {
        None
    } else if platform.is_windows() {
        Some(root.join(CLI_DIR).join(integrate::TUI_FORWARDER))
    } else {
        version
            .layout
            .tui_executable
            .as_ref()
            .map(|tui| root.join(CURRENT_LINK).join(tui))
    };
    program.map_or_else(
        || String::from(integrate::TUI_COMMAND),
        |path| quoted(&path),
    )
}

/// How the desktop app is opened: the entry this run created, or the program itself.
/// 桌面应用的打开方式：本次运行创建的入口，或程序本身。
fn desktop_way(
    platform: Platform,
    root: &Path,
    choices: &Choices,
    items: &[Item],
    version: &Version,
) -> Option<DesktopWay> {
    if !choices.gui {
        return None;
    }
    if let Some(bundle) = items.iter().find_map(|item| match item {
        Item::Bundle { path } => Some(path.clone()),
        _ => None,
    }) {
        return Some(DesktopWay::Bundle(bundle));
    }
    if choices.menu && !platform.is_macos() {
        return Some(if platform.is_windows() {
            DesktopWay::StartMenu
        } else {
            DesktopWay::AppMenu
        });
    }
    if choices.desktop_shortcut {
        return Some(DesktopWay::Shortcut);
    }
    if platform.is_windows() {
        return Some(DesktopWay::Program(quoted(
            &root.join(integrate::ROOT_LAUNCHER),
        )));
    }
    if choices.path {
        return Some(DesktopWay::Program(String::from(
            integrate::DESKTOP_COMMAND,
        )));
    }
    let desktop = version.layout.desktop_executable.as_ref()?;
    Some(DesktopWay::Program(quoted(
        &root.join(CURRENT_LINK).join(desktop),
    )))
}

/// The command that runs the setup program again: its name on PATH, its full path otherwise.
/// 再次运行安装器的命令：已加入 PATH 时为其名称，否则为完整路径。
pub fn setup_command(root: &Path, on_path: bool) -> String {
    if on_path {
        String::from(integrate::SETUP_COMMAND)
    } else {
        quoted(&root.join(SETUP_DIR).join(SETUP_PROGRAM))
    }
}

/// How to uninstall: the command on PATH when it was put there, the full path otherwise.
/// 如何卸载：命令已加入 PATH 时给出命令，否则给出完整路径。
pub fn uninstall_hint(root: &Path, on_path: bool) -> String {
    let command = setup_command(root, on_path);
    let mut hint = format!(
        "To uninstall: {command} uninstall (add --purge to also delete your sessions and settings)"
    );
    if cfg!(windows) {
        hint.push_str(", or Settings > Apps > Vulcan Code");
    }
    hint
}

/// Requires the assembled version to be a release build that loads here.
/// 要求组装好的版本是能在本机加载的发行构建。
fn check_release_build(version_dir: &Path) -> Result<(), String> {
    if Description::read(version_dir)?.profile != "release" {
        return Err(format!("{} is not a release build", version_dir.display()));
    }
    Ok(())
}

/// Copies the running setup program into `<root>/setup` unless it already runs from there.
/// 将正在运行的安装器复制到 `<root>/setup`，除非它已从那里运行。
fn copy_self(root: &Path) -> Result<(), String> {
    let running = std::env::current_exe()
        .map_err(|error| format!("cannot find the setup program: {error}"))?;
    let dir = root.join(SETUP_DIR);
    let target = dir.join(SETUP_PROGRAM);
    let same = match (running.canonicalize(), target.canonicalize()) {
        (Ok(running), Ok(target)) => running == target,
        _ => false,
    };
    if same {
        return Ok(());
    }
    let fail = |error: std::io::Error| format!("cannot write {}: {error}", target.display());
    std::fs::create_dir_all(&dir).map_err(fail)?;
    let mut partial = target.as_os_str().to_owned();
    partial.push(".partial");
    std::fs::copy(&running, &partial).map_err(fail)?;
    if target.exists() {
        // A running copy cannot be replaced on Windows, but it can be renamed.
        // Windows 上无法替换正在运行的副本，但可以改名。
        let mut aside = target.as_os_str().to_owned();
        aside.push(".old");
        let _ = std::fs::remove_file(&aside);
        let _ = std::fs::rename(&target, &aside);
    }
    std::fs::rename(&partial, &target).map_err(fail)
}

/// Component names separated by commas.
/// 以逗号分隔的组件名。
fn join(components: &BTreeSet<ComponentName>) -> String {
    components
        .iter()
        .map(ComponentName::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::tests::plain_state;

    fn platform(name: &str) -> Platform {
        Platform::ALL
            .into_iter()
            .find(|platform| platform.as_str() == name)
            .expect("platform")
    }

    /// Component names of `choices`.
    /// `choices` 的组件名。
    fn names(choices: &Choices) -> Vec<String> {
        choices
            .components()
            .expect("components")
            .iter()
            .map(|name| name.as_str().to_owned())
            .collect()
    }

    #[test]
    fn choices_follow_the_options_then_the_recorded_state_then_the_defaults() {
        let linux = platform("linux-x64");
        let headless =
            Choices::resolve(&InstallOptions::default(), None, linux, false).expect("headless");
        assert!(!headless.gui && headless.menu && headless.path && !headless.service);
        assert_eq!(names(&headless), ["core", "tui"]);
        let desktop =
            Choices::resolve(&InstallOptions::default(), None, linux, true).expect("desktop");
        assert_eq!(names(&desktop), ["core", "gui", "tui"]);
        let options = InstallOptions {
            service: Some(true),
            menu: Some(false),
            port_range: Some(String::from("18000-18029")),
            ..InstallOptions::default()
        };
        let choices = Choices::resolve(&options, None, linux, true).expect("choices");
        assert_eq!(choices.channel, Channel::Stable);
        assert!(choices.gui && choices.service && !choices.menu && choices.path);
        let recorded = SetupState {
            channel: Channel::Preview,
            route: Route::China,
            gui: false,
            with: vec![ComponentName::new("browser").expect("name")],
            service: true,
            ..plain_state()
        };
        // A recorded installation keeps its choices whatever session runs the setup program.
        // 已记录的安装保持其选择，与运行安装器的会话无关。
        let again = Choices::resolve(&InstallOptions::default(), Some(&recorded), linux, true)
            .expect("again");
        assert!(!again.gui && again.service && !again.path);
        assert_eq!(again.channel, Channel::Preview);
        assert_eq!(again.route, Route::China);
        assert_eq!(again.port_range, None);
        assert_eq!(names(&again), ["browser", "core", "tui"]);
    }

    #[test]
    fn choices_are_refused_where_they_do_not_exist() {
        let given = |options: InstallOptions, name: &str| {
            Choices::resolve(&options, None, platform(name), true)
        };
        let linger = InstallOptions {
            service: Some(true),
            linger: Some(true),
            ..InstallOptions::default()
        };
        assert!(given(linger.clone(), "linux-x64").is_ok());
        assert!(given(linger.clone(), "macos-arm64").is_err());
        assert!(
            given(
                InstallOptions {
                    service: Some(false),
                    ..linger
                },
                "linux-x64"
            )
            .is_err()
        );
        let shortcut = InstallOptions {
            desktop_shortcut: Some(true),
            ..InstallOptions::default()
        };
        assert!(given(shortcut.clone(), "windows-x64").is_ok());
        assert!(given(shortcut, "macos-arm64").is_err());
        // The desktop app and the service are offered on every platform.
        // 每个平台都提供桌面应用与服务。
        for name in ["windows-x64", "macos-arm64", "linux-x64"] {
            let both = InstallOptions {
                gui: Some(true),
                service: Some(true),
                ..InstallOptions::default()
            };
            assert!(names(&given(both, name).expect(name)).contains(&String::from("gui")));
        }
    }

    #[test]
    fn the_uninstall_hint_names_the_program_when_it_is_not_on_path() {
        let root = Path::new("/opt/vulcan");
        assert!(uninstall_hint(root, true).contains("vulcan-code-setup uninstall"));
        assert!(
            uninstall_hint(root, false).contains(
                &root
                    .join(SETUP_DIR)
                    .join(SETUP_PROGRAM)
                    .display()
                    .to_string()
            )
        );
    }

    #[test]
    fn a_declined_review_leaves_the_root_as_it_was_found() {
        let temp = tempfile::tempdir().expect("temp dir");
        let version: ReleaseVersion = "1.2.3".parse().expect("version");
        // A root this run creates goes away completely.
        // 本次运行创建的根目录会被完全移除。
        let fresh = InstallRoot::new(temp.path().join("fresh"));
        let undo = Undo::note(&fresh, &version);
        std::fs::create_dir_all(fresh.staging_dir(&version)).expect("staging");
        undo.run();
        assert!(!fresh.path().exists());
        // In an existing root only the directories this run created go, not an earlier download.
        // 在已有的根目录中只移除本次运行创建的目录，不移除更早的下载。
        let existing = InstallRoot::new(temp.path().join("existing"));
        std::fs::create_dir_all(existing.path().join("versions")).expect("versions");
        let undo = Undo::note(&existing, &version);
        std::fs::create_dir_all(existing.staging_dir(&version)).expect("staging");
        undo.run();
        assert!(existing.path().join("versions").is_dir());
        assert!(!existing.path().join(UPDATES_DIR).exists());
        std::fs::create_dir_all(existing.staging_dir(&version)).expect("earlier download");
        let undo = Undo::note(&existing, &version);
        undo.run();
        assert!(existing.staging_dir(&version).is_dir());
    }

    #[test]
    fn commands_are_named_on_path_and_quoted_otherwise() {
        let root = Path::new("/opt/vulcan");
        assert_eq!(setup_command(root, true), integrate::SETUP_COMMAND);
        let full = setup_command(root, false);
        assert!(full.starts_with('"') && full.ends_with('"'));
        assert!(full.contains(SETUP_PROGRAM));
    }

    #[test]
    fn glibc_versions_are_read_from_getconf() {
        assert_eq!(parse_glibc("glibc 2.35\n"), Some((2, 35)));
        assert_eq!(parse_glibc("glibc 2.39"), Some((2, 39)));
        assert_eq!(parse_glibc("musl"), None);
        assert!((2, 31) < MIN_GLIBC && (2, 40) > MIN_GLIBC);
    }

    #[test]
    fn launcher_paths_refuse_shell_characters() {
        assert!(embeddable("/opt/vulcan code"));
        assert!(!embeddable("/opt/$HOME"));
        assert!(!embeddable("/opt/\"x"));
    }
}
