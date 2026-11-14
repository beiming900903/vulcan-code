//! The interactive installer: one screen per choice, then a summary of every location it writes
//! (plan section 18.6). Text comes from the `setup` catalog surface; errors stay English.
//! 交互式安装界面：每个选择一屏，最后汇总将写入的每个位置（方案 18.6 节）。文案来自 `setup` 目录分面，
//! 错误保持英文。

use std::path::PathBuf;

use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use vulcan_i18n::{Translator, format_template};
use vulcan_install_core::fetch::Route;
use vulcan_install_core::names::{Channel, Platform};

use crate::args::check_port_range;
use crate::install::resolve_root;
use crate::integrate::{self, Plan};
use crate::root;
use crate::screen::{ACCENT, DANGER, MUTED, Screen, fields, location_lines, page, text};

/// The choices being edited.
/// 正在编辑的选择。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    /// Whether the desktop app is wanted.
    /// 是否需要桌面应用。
    pub gui: bool,
    /// Whether the browser component is wanted.
    /// 是否需要浏览器组件。
    pub browser: bool,
    /// Installation directory as typed.
    /// 输入的安装目录。
    pub root: String,
    /// Whether a menu entry or application bundle is wanted.
    /// 是否需要菜单项或应用程序包。
    pub menu: bool,
    /// Whether a desktop shortcut is wanted.
    /// 是否需要桌面快捷方式。
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
    /// Release channel.
    /// 发布通道。
    pub channel: Channel,
    /// Download route selected before the installer checks for updates.
    /// 安装器检查更新前选定的下载线路。
    pub route: Route,
    /// Port range as typed; empty keeps the current setting.
    /// 输入的端口范围；为空时保持当前设置。
    pub port_range: String,
}

/// One screen of the wizard.
/// 向导的一屏。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    /// Components.
    /// 组件。
    Components,
    /// Installation directory.
    /// 安装目录。
    Location,
    /// Shortcuts, PATH and the service.
    /// 快捷方式、PATH 与服务。
    Integration,
    /// Release channel.
    /// 发布通道。
    Channel,
    /// Port range.
    /// 端口范围。
    Advanced,
    /// Everything chosen, and every location written.
    /// 全部选择以及将写入的每个位置。
    Summary,
}

/// The screens, in order.
/// 各屏，按顺序排列。
const STEPS: [Step; 6] = [
    Step::Components,
    Step::Location,
    Step::Integration,
    Step::Channel,
    Step::Advanced,
    Step::Summary,
];

/// The routes and the catalog keys for their radio buttons, in display order.
/// 各下载线路及其单选项文案键，按显示顺序排列。
const ROUTES: [(Route, &str, &str); 3] = [
    (Route::Auto, "setup.route.auto", "setup.route.autoDetail"),
    (
        Route::International,
        "setup.route.international",
        "setup.route.internationalDetail",
    ),
    (Route::China, "setup.route.china", "setup.route.chinaDetail"),
];

/// One toggle of the components and integration screens.
/// 组件屏与集成屏中的一个开关。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Toggle {
    /// The desktop app.
    /// 桌面应用。
    Gui,
    /// The built-in browser.
    /// 内置浏览器。
    Browser,
    /// The menu entry or application bundle.
    /// 菜单项或应用程序包。
    Menu,
    /// The desktop shortcut.
    /// 桌面快捷方式。
    DesktopShortcut,
    /// Commands on PATH.
    /// 命令加入 PATH。
    Path,
    /// The service with the login session.
    /// 随登录会话启动的服务。
    Service,
    /// Keeping the service running after logout.
    /// 注销后继续运行服务。
    Linger,
}

impl Toggle {
    /// Catalog keys of the label and the detail line.
    /// 标签与说明行的目录键。
    fn keys(self) -> (&'static str, &'static str) {
        match self {
            Self::Gui => ("setup.component.gui", "setup.components.guiDetail"),
            Self::Browser => ("setup.components.browser", "setup.components.browserDetail"),
            Self::Menu => ("setup.integration.menu", "setup.integration.menuDetail"),
            Self::DesktopShortcut => (
                "setup.integration.desktopShortcut",
                "setup.integration.desktopShortcutDetail",
            ),
            Self::Path => ("setup.integration.path", "setup.integration.pathDetail"),
            Self::Service => (
                "setup.integration.service",
                "setup.integration.serviceDetail",
            ),
            Self::Linger => ("setup.integration.linger", "setup.integration.lingerDetail"),
        }
    }
}

/// What a key press leads to.
/// 按键导致的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
enum Flow {
    /// Stay in the wizard.
    /// 留在向导中。
    Stay,
    /// Install with the draft.
    /// 按草稿安装。
    Finish,
    /// Leave without installing.
    /// 不安装直接离开。
    Cancel,
}

/// The wizard's state, independent of the terminal.
/// 向导的状态，与终端无关。
struct Wizard {
    /// The platform.
    /// 平台。
    platform: Platform,
    /// Whether the setup program runs as root.
    /// 安装器是否以 root 运行。
    root_user: bool,
    /// The choices.
    /// 选择。
    draft: Draft,
    /// Index into [`STEPS`].
    /// [`STEPS`] 中的下标。
    step: usize,
    /// Highlighted row of a list screen.
    /// 列表屏中高亮的行。
    row: usize,
    /// Cursor of the text input, in characters.
    /// 文本输入的光标位置，以字符计。
    cursor: usize,
    /// Why the last attempt to continue was refused.
    /// 上一次继续被拒绝的原因。
    error: Option<String>,
}

impl Wizard {
    /// Starts at the first screen.
    /// 从第一屏开始。
    fn new(platform: Platform, root_user: bool, draft: Draft) -> Self {
        let mut wizard = Self {
            platform,
            root_user,
            draft,
            step: 0,
            row: 0,
            cursor: 0,
            error: None,
        };
        wizard.enter();
        wizard
    }

    /// The current screen.
    /// 当前屏。
    fn current(&self) -> Step {
        STEPS[self.step.min(STEPS.len() - 1)]
    }

    /// The toggles of the current screen.
    /// 当前屏的开关。
    fn toggles(&self) -> Vec<Toggle> {
        match self.current() {
            Step::Components => vec![Toggle::Gui, Toggle::Browser],
            Step::Integration => {
                let mut toggles = Vec::new();
                if self.draft.gui {
                    toggles.push(Toggle::Menu);
                    if !self.platform.is_macos() {
                        toggles.push(Toggle::DesktopShortcut);
                    }
                }
                toggles.extend([Toggle::Path, Toggle::Service]);
                if self.draft.service && !self.platform.is_windows() && !self.platform.is_macos() {
                    toggles.push(Toggle::Linger);
                }
                toggles
            }
            _ => Vec::new(),
        }
    }

    /// The state of `toggle` in the draft.
    /// 草稿中 `toggle` 的状态。
    fn state(&self, toggle: Toggle) -> bool {
        match toggle {
            Toggle::Gui => self.draft.gui,
            Toggle::Browser => self.draft.browser,
            Toggle::Menu => self.draft.menu,
            Toggle::DesktopShortcut => self.draft.desktop_shortcut,
            Toggle::Path => self.draft.path,
            Toggle::Service => self.draft.service,
            Toggle::Linger => self.draft.linger,
        }
    }

    /// Number of rows of the current list screen.
    /// 当前列表屏的行数。
    fn rows(&self) -> usize {
        match self.current() {
            Step::Components | Step::Integration => self.toggles().len(),
            Step::Channel => 2,
            Step::Location | Step::Advanced | Step::Summary => 0,
        }
    }

    /// The text field of the current screen, when it has one.
    /// 当前屏的文本字段（若有）。
    fn field(&mut self) -> Option<&mut String> {
        match self.current() {
            Step::Location => Some(&mut self.draft.root),
            Step::Advanced => Some(&mut self.draft.port_range),
            _ => None,
        }
    }

    /// Highlights the current choice, or puts the cursor at the end of the text, when a screen
    /// is entered.
    /// 进入某屏时高亮当前选择，或把光标放到文本末尾。
    fn enter(&mut self) {
        self.error = None;
        self.row = match self.current() {
            Step::Channel => usize::from(self.draft.channel == Channel::Preview),
            _ => 0,
        };
        self.cursor = self.field().map_or(0, |field| field.chars().count());
    }

    /// Moves the highlight; on the channel screen it also selects.
    /// 移动高亮；在通道屏中同时选中。
    fn move_row(&mut self, down: bool) {
        let rows = self.rows();
        if rows == 0 {
            return;
        }
        self.row = if down {
            (self.row + 1) % rows
        } else {
            (self.row + rows - 1) % rows
        };
        if self.current() == Step::Channel {
            self.draft.channel = if self.row == 0 {
                Channel::Stable
            } else {
                Channel::Preview
            };
        }
    }

    /// Flips the highlighted toggle; turning the service off also turns lingering off.
    /// 切换高亮的开关；关闭服务时同时关闭驻留。
    fn toggle(&mut self) {
        let Some(toggle) = self.toggles().get(self.row).copied() else {
            return;
        };
        let draft = &mut self.draft;
        match toggle {
            Toggle::Gui => draft.gui = !draft.gui,
            Toggle::Browser => draft.browser = !draft.browser,
            Toggle::Menu => draft.menu = !draft.menu,
            Toggle::DesktopShortcut => draft.desktop_shortcut = !draft.desktop_shortcut,
            Toggle::Path => draft.path = !draft.path,
            Toggle::Service => {
                draft.service = !draft.service;
                draft.linger &= draft.service;
            }
            Toggle::Linger => draft.linger = !draft.linger,
        }
    }

    /// Continues to the next screen after checking the current one.
    /// 检查当前屏后继续到下一屏。
    fn advance(&mut self) -> Flow {
        let checked = match self.current() {
            Step::Location => check_location(self.platform, &self.draft.root).map(|_| ()),
            Step::Advanced => match self.draft.port_range.trim() {
                "" => Ok(()),
                range => check_port_range(range),
            },
            Step::Summary => return Flow::Finish,
            _ => Ok(()),
        };
        if let Err(error) = checked {
            self.error = Some(error);
            return Flow::Stay;
        }
        self.step += 1;
        self.enter();
        Flow::Stay
    }

    /// Returns to the previous screen, or leaves from the first one.
    /// 返回上一屏；在第一屏时离开。
    fn back(&mut self) -> Flow {
        if self.step == 0 {
            return Flow::Cancel;
        }
        self.step -= 1;
        self.enter();
        Flow::Stay
    }

    /// Handles one key press.
    /// 处理一次按键。
    fn handle(&mut self, key: KeyEvent) -> Flow {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Flow::Cancel;
        }
        if matches!(self.current(), Step::Location | Step::Advanced) {
            return self.edit(key);
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.move_row(false),
            KeyCode::Down | KeyCode::Char('j') => self.move_row(true),
            KeyCode::Char(' ') => self.toggle(),
            KeyCode::Enter => return self.advance(),
            KeyCode::Esc | KeyCode::Backspace | KeyCode::Left => return self.back(),
            KeyCode::Char('q') => return Flow::Cancel,
            _ => {}
        }
        Flow::Stay
    }

    /// Edits the text field of the current screen.
    /// 编辑当前屏的文本字段。
    fn edit(&mut self, key: KeyEvent) -> Flow {
        match key.code {
            KeyCode::Enter => return self.advance(),
            KeyCode::Esc => return self.back(),
            _ => {}
        }
        let mut cursor = self.cursor;
        let Some(field) = self.field() else {
            return Flow::Stay;
        };
        let mut chars: Vec<char> = field.chars().collect();
        match key.code {
            KeyCode::Left => cursor = cursor.saturating_sub(1),
            KeyCode::Right => cursor = (cursor + 1).min(chars.len()),
            KeyCode::Home => cursor = 0,
            KeyCode::End => cursor = chars.len(),
            KeyCode::Backspace if cursor > 0 => {
                cursor -= 1;
                chars.remove(cursor);
            }
            KeyCode::Delete if cursor < chars.len() => {
                chars.remove(cursor);
            }
            KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                chars.insert(cursor, character);
                cursor += 1;
            }
            _ => return Flow::Stay,
        }
        *field = chars.into_iter().collect();
        self.cursor = cursor;
        self.error = None;
        Flow::Stay
    }
}

/// Checks that `text` names a directory the setup program may install into.
/// 检查 `text` 是否指向安装器可以安装到的目录。
fn check_location(platform: Platform, text: &str) -> Result<PathBuf, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err(String::from("enter a directory"));
    }
    let root = resolve_root(Some(&PathBuf::from(text)), platform)?;
    root::installable(&root)?;
    Ok(root)
}

/// Runs the wizard on `screen`, from the summary when `at_summary`; `None` when the person left
/// without installing.
/// 在 `screen` 上运行向导，`at_summary` 时从汇总屏开始；用户未安装就离开时为 `None`。
pub fn run(
    screen: &mut Screen,
    platform: Platform,
    draft: Draft,
    at_summary: bool,
) -> Result<Option<Draft>, String> {
    let mut wizard = Wizard::new(platform, integrate::running_as_root(), draft);
    if at_summary {
        wizard.step = STEPS.len() - 1;
        wizard.enter();
    }
    loop {
        screen.draw(|frame, translator| render(frame, &wizard, translator))?;
        let key = match event::read() {
            Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => key,
            Ok(_) => continue,
            Err(error) => return Err(format!("cannot read the keyboard: {error}")),
        };
        match wizard.handle(key) {
            Flow::Stay => {}
            Flow::Finish => return Ok(Some(wizard.draft)),
            Flow::Cancel => return Ok(None),
        }
    }
}

/// Draws the current screen.
/// 绘制当前屏。
fn render(frame: &mut Frame<'_>, wizard: &Wizard, translator: &Translator) {
    let t = |key| text(translator, key);
    let step = wizard.current();
    let title = match step {
        Step::Components => "setup.components.title",
        Step::Location => "setup.location.title",
        Step::Integration => "setup.integration.title",
        Step::Channel => "setup.channel.title",
        Step::Advanced => "setup.advanced.title",
        Step::Summary => "setup.summary.title",
    };
    let counter = format_template(
        t("setup.step"),
        &[
            ("current", &(wizard.step + 1).to_string()),
            ("total", &STEPS.len().to_string()),
        ],
    );
    let mut lines = match step {
        Step::Components | Step::Integration => {
            let toggles = wizard.toggles();
            // Resolve the detail here so the same renderer accepts descriptions with arguments.
            // 在此翻译说明，让同一渲染器也能接收带参数的说明。
            let options: Vec<_> = toggles
                .iter()
                .map(|toggle| {
                    let (label, detail) = toggle.keys();
                    (label, t(detail))
                })
                .collect();
            let states: Vec<bool> = toggles.iter().map(|toggle| wizard.state(*toggle)).collect();
            choice_lines(translator, &options, wizard.row, Some(&states))
        }
        Step::Channel => choice_lines(
            translator,
            &[
                ("setup.channel.stable", t("setup.channel.stableDetail")),
                ("setup.channel.preview", t("setup.channel.previewDetail")),
            ],
            wizard.row,
            None,
        ),
        Step::Location => input_lines(
            t("setup.location.detail"),
            &wizard.draft.root,
            wizard.cursor,
        ),
        Step::Advanced => {
            let mut lines = vec![Line::styled(
                t("setup.advanced.ports"),
                Style::default().add_modifier(Modifier::BOLD),
            )];
            lines.extend(input_lines(
                t("setup.advanced.portsDetail"),
                &wizard.draft.port_range,
                wizard.cursor,
            ));
            lines
        }
        Step::Summary => summary_lines(translator, wizard),
    };
    if let Some(error) = &wizard.error {
        lines.push(Line::default());
        lines.push(Line::styled(error.clone(), Style::default().fg(DANGER)));
    }
    let hint = match step {
        Step::Channel => "setup.hint.choose",
        Step::Components | Step::Integration => "setup.hint.toggle",
        Step::Location => "setup.hint.input",
        Step::Advanced => "setup.hint.ports",
        Step::Summary => "setup.hint.summary",
    };
    page(frame, translator, &counter, t(title), lines, t(hint), 0);
}

/// Lines of a list screen: radio buttons when `states` is `None`, check boxes otherwise.
/// 列表屏的各行：`states` 为 `None` 时为单选按钮，否则为复选框。
fn choice_lines<'a>(
    translator: &'a Translator,
    options: &[(&'a str, &'a str)],
    row: usize,
    states: Option<&[bool]>,
) -> Vec<Line<'a>> {
    let mut lines = Vec::new();
    for (index, (label, detail)) in options.iter().enumerate() {
        let highlighted = index == row;
        let mark = match states {
            None if highlighted => "(•)",
            None => "( )",
            Some(states) if states.get(index).copied().unwrap_or(false) => "[x]",
            Some(_) => "[ ]",
        };
        let style = if highlighted {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let pointer = if highlighted { "›" } else { " " };
        lines.push(Line::from(vec![
            Span::styled(format!("{pointer} {mark} "), style),
            Span::styled(text(translator, label), style),
        ]));
        lines.push(Line::styled(
            format!("      {detail}"),
            Style::default().fg(MUTED),
        ));
        lines.push(Line::default());
    }
    lines
}

/// Opens the selection page with `initial` highlighted and returns the chosen route or cancellation.
/// 打开线路选择页，默认高亮 `initial`，返回选中的线路或取消结果。
pub fn choose_route(initial: Route) -> Result<Route, String> {
    // Keep the highlighted row aligned with the route used if Enter is pressed immediately.
    // 让高亮行与直接按 Enter 时采用的线路一致。
    let mut row = ROUTES
        .iter()
        .position(|(route, ..)| *route == initial)
        .expect("every download route has a radio button");
    // Reuse the installation screen and return the terminal before the self-update can restart.
    // 复用安装器界面，并在安装器可能自更新重启前归还终端。
    let mut screen = Screen::open()?;
    loop {
        screen.draw(|frame, translator| render_routes(frame, translator, row))?;
        // Ignore non-key events and key release events, as the main wizard does.
        // 与主向导一样，忽略非按键事件和按键释放事件。
        let key = match event::read() {
            Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => key,
            Ok(_) => continue,
            Err(error) => return Err(format!("cannot read the keyboard: {error}")),
        };
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => row = (row + ROUTES.len() - 1) % ROUTES.len(),
            KeyCode::Down | KeyCode::Char('j') => row = (row + 1) % ROUTES.len(),
            KeyCode::Enter => return Ok(ROUTES[row].0),
            KeyCode::Esc | KeyCode::Char('q') => return Err(String::from(crate::CANCELLED)),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Err(String::from(crate::CANCELLED));
            }
            _ => {}
        }
    }
}

/// Draws the route page with row `row` selected and URLs from the downloader's route table.
/// 绘制下载线路页，高亮第 `row` 行，网址来自下载器的线路表。
fn render_routes(frame: &mut Frame<'_>, translator: &Translator, row: usize) {
    // Auto has two origins; an explicit node uses its single configured origin.
    // 自动线路有两个来源，显式节点使用其唯一配置的来源。
    let details = ROUTES.map(|(route, _, detail)| {
        if route == Route::Auto {
            text(translator, detail).to_owned()
        } else {
            format_template(text(translator, detail), &[("domain", &route.origins()[0])])
        }
    });
    // Borrow the translated descriptions for this frame.
    // 在本帧借用翻译后的说明。
    let options = std::array::from_fn::<_, { ROUTES.len() }, _>(|index| {
        (ROUTES[index].1, details[index].as_str())
    });
    page(
        frame,
        translator,
        "",
        text(translator, "setup.route.title"),
        choice_lines(translator, &options, row, None),
        text(translator, "setup.route.hint"),
        0,
    );
}

/// Lines of a text input, with the cursor shown as a reversed cell.
/// 文本输入的各行，光标以反色单元显示。
fn input_lines<'a>(detail: &'a str, value: &str, cursor: usize) -> Vec<Line<'a>> {
    let chars: Vec<char> = value.chars().collect();
    let before: String = chars[..cursor.min(chars.len())].iter().collect();
    let at: String = chars
        .get(cursor)
        .map_or_else(|| String::from(" "), |character| character.to_string());
    let after: String = chars.iter().skip(cursor + 1).collect();
    vec![
        Line::styled(detail, Style::default().fg(MUTED)),
        Line::default(),
        Line::from(vec![
            Span::styled("› ", Style::default().fg(ACCENT)),
            Span::raw(before),
            Span::styled(at, Style::default().add_modifier(Modifier::REVERSED)),
            Span::raw(after),
        ]),
    ]
}

/// Lines of the summary screen.
/// 汇总屏的各行。
fn summary_lines<'a>(translator: &'a Translator, wizard: &Wizard) -> Vec<Line<'a>> {
    let t = |key| text(translator, key);
    let draft = &wizard.draft;
    let mut components = vec![t("setup.component.core"), t("setup.component.tui")];
    if draft.gui {
        components.push(t("setup.component.gui"));
    }
    if draft.browser {
        components.push(t("setup.components.browser"));
    }
    let channel = match draft.channel {
        Channel::Stable => t("setup.channel.stable"),
        Channel::Preview => t("setup.channel.preview"),
    };
    // Display the selected route beside the release channel in the review.
    // 在确认页的发布通道旁显示选中的下载线路。
    let (_, route, _) = ROUTES
        .iter()
        .find(|(route, ..)| *route == draft.route)
        .expect("every download route has a radio button");
    let ports = match draft.port_range.trim() {
        "" => t("setup.summary.portsKeep").to_owned(),
        range => range.to_owned(),
    };
    let root = check_location(wizard.platform, &draft.root)
        .unwrap_or_else(|_| PathBuf::from(draft.root.trim()));
    let mut lines = fields(vec![
        (t("setup.summary.components"), components.join(", ")),
        (t("setup.summary.directory"), root.display().to_string()),
        (t("setup.summary.channel"), channel.to_owned()),
        (t("setup.route.title"), t(route).to_owned()),
        (t("setup.summary.ports"), ports),
    ]);
    lines.push(Line::default());
    lines.push(Line::styled(
        t("setup.summary.writes"),
        Style::default().fg(MUTED),
    ));
    let version = String::new();
    let plan = Plan {
        platform: wizard.platform,
        root: &root,
        version: &version,
        gui: draft.gui,
        menu: draft.menu,
        desktop_shortcut: draft.desktop_shortcut,
        path: draft.path,
        service: draft.service,
        linger: draft.linger,
        port_range: None,
    };
    let locations = plan.locations().unwrap_or_default();
    lines.extend(location_lines(translator, wizard.platform, &locations));
    if wizard.root_user {
        lines.push(Line::default());
        lines.push(Line::styled(
            t("setup.summary.root"),
            Style::default().fg(DANGER),
        ));
    }
    lines.push(Line::default());
    lines.push(Line::styled(
        t("setup.summary.detail"),
        Style::default().fg(MUTED),
    ));
    lines
}

/// Whether the wizard should run: no `--yes`, and both ends are a terminal.
/// 是否运行向导：没有 `--yes`，且输入输出都是终端。
pub fn wanted(yes: bool) -> bool {
    use std::io::IsTerminal;
    !yes && std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use vulcan_i18n::SETUP;

    fn platform(name: &str) -> Platform {
        Platform::ALL
            .into_iter()
            .find(|platform| platform.as_str() == name)
            .expect("platform")
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn draft(root: &str) -> Draft {
        Draft {
            gui: true,
            browser: false,
            root: root.to_owned(),
            menu: true,
            desktop_shortcut: false,
            path: true,
            service: false,
            linger: false,
            channel: Channel::Stable,
            route: Route::Auto,
            port_range: String::new(),
        }
    }

    /// Types `text` into the current text field.
    /// 在当前文本字段中输入 `text`。
    fn type_text(wizard: &mut Wizard, text: &str) {
        for character in text.chars() {
            wizard.handle(key(KeyCode::Char(character)));
        }
    }

    #[test]
    fn integration_toggles_follow_the_desktop_app_and_the_service() {
        let mut linux = Wizard::new(platform("linux-x64"), false, draft("/opt/vulcan"));
        linux.step = 2;
        assert_eq!(
            linux.toggles(),
            [
                Toggle::Menu,
                Toggle::DesktopShortcut,
                Toggle::Path,
                Toggle::Service
            ]
        );
        linux.row = 3;
        linux.toggle();
        assert_eq!(linux.toggles().last(), Some(&Toggle::Linger));
        linux.row = 4;
        linux.toggle();
        assert!(linux.draft.linger);
        // Turning the service off also turns lingering off.
        // 关闭服务时同时关闭驻留。
        linux.row = 3;
        linux.toggle();
        assert!(!linux.draft.service && !linux.draft.linger);
        let mut macos = Wizard::new(platform("macos-arm64"), false, draft("/Users/a/Vulcan"));
        macos.step = 2;
        macos.draft.service = true;
        assert_eq!(
            macos.toggles(),
            [Toggle::Menu, Toggle::Path, Toggle::Service]
        );
        macos.draft.gui = false;
        assert_eq!(macos.toggles(), [Toggle::Path, Toggle::Service]);
    }

    #[test]
    fn keys_walk_the_screens_and_edit_the_draft() {
        // The directory is checked by the rules of the machine running the test.
        // 目录按运行测试的机器的规则检查。
        let host = Platform::current().expect("supported host");
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path().join("vulcan").display().to_string();
        let mut wizard = Wizard::new(host, false, draft(&root));
        assert_eq!(wizard.current(), Step::Components);
        wizard.handle(key(KeyCode::Char(' ')));
        assert!(!wizard.draft.gui);
        wizard.handle(key(KeyCode::Down));
        wizard.handle(key(KeyCode::Char(' ')));
        assert!(wizard.draft.browser);
        wizard.handle(key(KeyCode::Enter));
        assert_eq!(wizard.current(), Step::Location);
        // Typing `q` edits the path instead of quitting.
        // 输入 `q` 编辑路径而不是退出。
        wizard.handle(key(KeyCode::Char('q')));
        assert!(wizard.draft.root.ends_with('q'));
        wizard.handle(key(KeyCode::Backspace));
        assert_eq!(wizard.draft.root, root);
        wizard.handle(key(KeyCode::Enter));
        // Without the desktop app there is no shortcut, so the first toggle is PATH.
        // 没有桌面应用就没有快捷方式，因此第一个开关是 PATH。
        assert_eq!(wizard.toggles().first(), Some(&Toggle::Path));
        wizard.handle(key(KeyCode::Char(' ')));
        assert!(!wizard.draft.path);
        wizard.handle(key(KeyCode::Enter));
        wizard.handle(key(KeyCode::Down));
        assert_eq!(wizard.draft.channel, Channel::Preview);
        wizard.handle(key(KeyCode::Enter));
        assert_eq!(wizard.current(), Step::Advanced);
        type_text(&mut wizard, "18000-");
        assert_eq!(wizard.handle(key(KeyCode::Enter)), Flow::Stay);
        assert!(wizard.error.is_some());
        type_text(&mut wizard, "18029");
        wizard.handle(key(KeyCode::Enter));
        assert_eq!(wizard.draft.port_range, "18000-18029");
        assert_eq!(wizard.current(), Step::Summary);
        assert_eq!(wizard.handle(key(KeyCode::Enter)), Flow::Finish);
    }

    #[test]
    fn a_refused_directory_keeps_the_location_screen() {
        let temp = tempfile::tempdir().expect("temp dir");
        std::fs::write(temp.path().join("notes.txt"), "mine").expect("foreign file");
        let mut wizard = Wizard::new(
            platform("windows-x64"),
            false,
            draft(&temp.path().display().to_string()),
        );
        assert_eq!(wizard.current(), Step::Components);
        wizard.handle(key(KeyCode::Enter));
        assert_eq!(wizard.handle(key(KeyCode::Enter)), Flow::Stay);
        assert_eq!(wizard.current(), Step::Location);
        assert!(wizard.error.is_some());
        wizard.handle(key(KeyCode::Esc));
        assert_eq!(wizard.current(), Step::Components);
        assert_eq!(wizard.handle(key(KeyCode::Esc)), Flow::Cancel);
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(wizard.handle(ctrl_c), Flow::Cancel);
    }

    #[test]
    fn every_screen_renders_catalog_text_in_every_language() {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path().join("vulcan").display().to_string();
        for name in ["linux-x64", "macos-arm64", "windows-x64"] {
            for locale in vulcan_i18n::LOCALES {
                let mut translator = Translator::new(&[&SETUP]);
                assert!(translator.set_locale(locale.code));
                let mut start = draft(&root);
                start.service = true;
                start.linger = !name.starts_with("windows") && !name.starts_with("macos");
                start.desktop_shortcut = true;
                let mut wizard = Wizard::new(platform(name), true, start);
                let mut terminal = Terminal::new(TestBackend::new(60, 40)).expect("terminal");
                // Render the standalone route chooser as well as the numbered wizard pages.
                // 除带编号的向导页外，也渲染独立的下载线路选择页。
                terminal
                    .draw(|frame| render_routes(frame, &translator, 0))
                    .expect("draw routes");
                // Read the rendered cells to ensure both origins and formatted catalog text are visible.
                // 读取渲染单元，验证两个来源及替换后的目录文案都已显示。
                let routes: String = terminal
                    .backend()
                    .buffer()
                    .content()
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect();
                assert!(!routes.contains("setup.") && !routes.contains("{domain}"));
                assert!(routes.contains(vulcan_install_core::layout::PRIMARY_ORIGIN));
                assert!(routes.contains(vulcan_install_core::layout::CHINA_ORIGIN));
                for _ in 0..STEPS.len() {
                    terminal
                        .draw(|frame| render(frame, &wizard, &translator))
                        .expect("draw");
                    // A key missing from the catalog would be drawn as the key itself.
                    // 目录中缺少的键会以键本身绘制出来。
                    let screen: String = terminal
                        .backend()
                        .buffer()
                        .content()
                        .iter()
                        .map(|cell| cell.symbol())
                        .collect();
                    assert!(
                        !screen.contains("setup."),
                        "{name} {} {:?} shows a raw key",
                        locale.code,
                        wizard.current()
                    );
                    wizard.advance();
                }
            }
        }
    }
}
