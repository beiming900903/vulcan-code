// Electron desktop GUI main process.
// Electron 桌面 GUI 主进程。
//
// The GUI owns a window and native integrations only. It starts or reuses the independent Core
// service, waits for real business readiness, and never stops that service when its window closes.
// GUI 只拥有窗口与原生集成。它启动或复用独立 Core 服务、等待真实业务就绪，并且在关闭窗口时
// 绝不停止该服务。

import { BrowserWindow, Notification, app, clipboard, dialog, ipcMain, shell } from 'electron';
import { existsSync } from 'node:fs';
import { stat } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import {
  SERVICE_CHANNELS,
  STARTUP_CHANNELS,
  CLIPBOARD_CHANNELS,
  LOCAL_BROWSER_CHANNELS,
  LOCAL_FILE_CHANNELS,
  NOTIFICATION_CHANNELS,
  NOTIFICATION_EVENT_CHANNELS,
  WINDOW_COMMAND_CHANNELS,
  WINDOW_EVENT_CHANNELS,
} from './ipc-channels.cjs';
import { LocalBrowserManager, type LocalBrowserViewport } from './local-browser.js';
import { inspectLocalFile, openLocalFileWithDefaultApp, showLocalFileInFolder } from './local-file-actions.js';
import {
  ensureServiceInstance,
  readServiceLaunchDescription,
  readLiveServiceInstance,
  resolveServiceExecutable,
  resolveWebUiUrl,
  RUNTIME_LAYOUT_ENV,
  type LiveServiceInstance,
  type ServiceStartupProgress,
} from './service-launcher.js';
import {
  loadStartupPage,
  readStartupAppearance,
  rememberStartupAppearance,
  reportStartupStage,
  showStartupFailure,
} from './startup-surface.js';
import { parseStartupAppearance, type StartupAppearance } from './startup-appearance.js';
import { resolveStableDesktopEntry } from './installed-launcher.js';
import { awaitUpdateRestart, isUpdateRestartReason, type UpdateRestartReason } from './update-restart.js';
import {
  desktopText,
  detectDesktopLocale,
  formatDesktopText,
  type DesktopLocale,
} from './desktop-text.js';
import { applyDockIcon, windowIconOptions } from './app-icon.js';
import { canUseNativeNotifications, configureGuiNotificationIdentity } from './notification-identity.js';
import { createWindowsToastXml } from './notification-toast.js';
import { registerRendererRecovery } from './renderer-recovery.js';
import { loadWorkbenchPage } from './workbench-page.js';

// Direct development launches bypass the packaged entry but keep the same identity contract.
// 直接开发启动会绕开发行入口，但仍遵循相同的身份契约。
configureGuiNotificationIdentity();

/** Initial window size used before any user resize. */
const INITIAL_WINDOW_WIDTH = 1440;
const INITIAL_WINDOW_HEIGHT = 900;

/** Minimum window size that keeps the workbench usable. */
const MINIMUM_WINDOW_WIDTH = 960;
const MINIMUM_WINDOW_HEIGHT = 600;

// TRAFFIC_LIGHT_FRAME_HEIGHT is the height of one macOS window button's frame: a 12px light with an
// invisible 2px margin above and below, the height the buttons are centred by.
// TRAFFIC_LIGHT_FRAME_HEIGHT 是 macOS 单个窗口按钮框架的高度：12px 的按钮加上下各 2px 的不可见边距，
// 按钮按此高度垂直居中。
const TRAFFIC_LIGHT_FRAME_HEIGHT = 16;

// TRAFFIC_LIGHTS_WIDTH spans the three macOS window button frames (14px each) and the two 6px gaps between them.
// TRAFFIC_LIGHTS_WIDTH 为三个 macOS 窗口按钮框架（各 14px）及其间两个 6px 间隔的总宽度。
const TRAFFIC_LIGHTS_WIDTH = 54;

// MAXIMUM_TITLE_STRIP_LENGTH bounds the renderer-reported title strip geometry, in CSS pixels.
// MAXIMUM_TITLE_STRIP_LENGTH 限定渲染端上报的标题条几何尺寸，单位为 CSS 像素。
const MAXIMUM_TITLE_STRIP_LENGTH = 512;

/** Title strip geometry the WebUI reports for the macOS window buttons, in CSS pixels. */
interface NativeButtonsPlacement {
  /** Left edge of the strip the buttons sit in, the activity rail's width. */
  left: number;
  /** Height of the title strip the buttons are centred in. */
  height: number;
}

// parseNativeButtonsPlacement validates untrusted renderer geometry; returns null when any field is unusable.
// parseNativeButtonsPlacement 校验不受信的渲染端几何数据；任一字段不可用时返回 null。
function parseNativeButtonsPlacement(value: unknown): NativeButtonsPlacement | null {
  if (typeof value !== 'object' || value === null) return null;
  const { left, height } = value as Record<string, unknown>;
  const usable = (length: unknown): length is number =>
    typeof length === 'number' && Number.isFinite(length) && length >= 0 && length <= MAXIMUM_TITLE_STRIP_LENGTH;
  return usable(left) && usable(height) && height >= TRAFFIC_LIGHT_FRAME_HEIGHT ? { left, height } : null;
}

/** Service switches this GUI forwards to the service process on request. */
const FORWARDED_SERVICE_ARGUMENTS = new Set(['--test-root']);

/** Command-line options parsed by this GUI. */
interface DesktopGuiOptions {
  /** Explicit service executable path, or null to resolve the debug default. */
  serviceExecutable: string | null;
  /** Explicit WebUI URL, or null to use the described mode. */
  webUiUrl: string | null;
  /** Arguments forwarded to the service process and its description command. */
  forwardedArguments: string[];
}

/** Splits one `--name=value` or bare `--name` argument. */
function splitArgument(argument: string): { name: string; value: string | null } {
  const separator = argument.indexOf('=');
  if (separator < 0) {
    return { name: argument, value: null };
  }
  return { name: argument.slice(0, separator), value: argument.slice(separator + 1) };
}

/** Parses the GUI command line while preserving only service-owned pass-through arguments. */
// parseDesktopGuiOptions separates GUI-owned switches from the service allowlist. Electron and
// Chromium switches are neither consumed nor forwarded: the service rejects unknown arguments, so
// forwarding an Electron switch such as --remote-debugging-port would break its description command.
// parseDesktopGuiOptions 分离 GUI 自有开关与服务允许列表。Electron 与 Chromium 开关既不消费也不
// 转发：服务会拒绝未知参数，因此转发诸如 --remote-debugging-port 的开关会破坏其描述命令。
function parseDesktopGuiOptions(argv: string[]): DesktopGuiOptions {
  let serviceExecutable: string | null = null;
  let webUiUrl: string | null = null;
  const forwardedArguments: string[] = [];
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index] ?? '';
    const { name, value } = splitArgument(argument);
    if (name === '--service-executable') {
      serviceExecutable = value ?? argv[index + 1] ?? null;
      if (value === null) {
        index += 1;
      }
      continue;
    }
    if (name === '--webui-url') {
      webUiUrl = value ?? argv[index + 1] ?? null;
      if (value === null) {
        index += 1;
      }
      continue;
    }
    // The console switch has no value and must reach both Core description and launch.
    // 控制台开关不带参数，且必须同时到达 Core 描述与启动流程。
    if (argument === '--no-debug-console') {
      forwardedArguments.push(argument);
      continue;
    }
    if (FORWARDED_SERVICE_ARGUMENTS.has(name)) {
      if (value === null) {
        const next = argv[index + 1];
        forwardedArguments.push(next === undefined ? argument : `${name}=${next}`);
        if (next !== undefined) {
          index += 1;
        }
      } else {
        forwardedArguments.push(argument);
      }
    }
  }
  return { serviceExecutable, webUiUrl, forwardedArguments };
}

/** State shared by the IPC handlers of the current window. */
interface GuiWindowState {
  /** The single workbench window. */
  window: BrowserWindow | null;
  /** The confirmed live service instance. */
  instance: LiveServiceInstance | null;
}

const guiState: GuiWindowState = { window: null, instance: null };

// Retain pending Action Center notifications after banner timeout so click handlers stay live.
// 横幅超时后仍保留通知中心的待点击对象，使点击处理器保持有效。
const nativeNotifications = new Map<string, Notification>();
// The renderer keeps at most this many click routes; mirror its bound in the native host.
// 渲染端最多保留相同数量的点击路由；原生宿主采用一致的上限。
const MAX_RETAINED_NATIVE_NOTIFICATIONS = 512;

// retainNativeNotification holds one native click handler and evicts the oldest beyond the route bound.
// retainNativeNotification 保留一个原生点击处理器，并在超出路由上限时逐出最旧对象。
// id identifies the native toast; notification is its live Electron object; no value is returned.
// id 标识原生通知；notification 为有效的 Electron 对象；此函数不返回值。
function retainNativeNotification(id: string, notification: Notification): void {
  nativeNotifications.set(id, notification);
  while (nativeNotifications.size > MAX_RETAINED_NATIVE_NOTIFICATIONS) {
    const oldestId = nativeNotifications.keys().next().value;
    if (typeof oldestId !== 'string') break;
    nativeNotifications.delete(oldestId);
  }
}

// activateNativeNotification routes a click only for a toast delivered by this live GUI.
// activateNativeNotification 只路由由当前有效 GUI 投递的通知点击。
// id is the validated native tag; no value is returned.
// id 为已验证的原生标签；此函数不返回值。
function activateNativeNotification(id: string): void {
  if (!nativeNotifications.has(id)) return;
  const window = guiState.window;
  if (!window || window.isDestroyed()) return;
  nativeNotifications.delete(id);
  if (window.isMinimized()) window.restore();
  window.show();
  // A user click in Windows Action Center can leave the notification pane above a restored GUI.
  // 用户从 Windows 通知中心点击后，通知面板可能仍压在已还原的 GUI 上方。
  window.moveTop();
  window.focus();
  window.webContents.send(NOTIFICATION_EVENT_CHANNELS.activated, { notificationId: id });
}

// parseWindowsNotificationActivation accepts only one exact body-click tag from our toast XML.
// parseWindowsNotificationActivation 只接受本应用通知 XML 中唯一且精确的正文点击标签。
// details are Electron's Windows activation arguments; returns the valid tag or null.
// details 为 Electron 的 Windows 激活参数；返回有效标签或空值。
function parseWindowsNotificationActivation(details: { type: string; arguments: string }): string | null {
  if (details.type !== 'click') return null;
  const parameters = new URLSearchParams(details.arguments);
  const types = parameters.getAll('type');
  const tags = parameters.getAll('tag');
  if (types.length !== 1 || types[0] !== 'click' || tags.length !== 1) return null;
  const tag = tags[0];
  return tag && /^[A-Za-z0-9:._-]{1,64}$/.test(tag) ? tag : null;
}

/** Returns the language the WebUI last reported, or the device language before any report. */
function startupLocale(appearance: StartupAppearance | null): DesktopLocale {
  return appearance?.locale ?? detectDesktopLocale(app.getPreferredSystemLanguages());
}

// DEFAULT_WINDOW_BACKGROUND paints the window before the WebUI ever reported its ground color.
// DEFAULT_WINDOW_BACKGROUND 在 WebUI 尚未上报底色时绘制窗口背景。
const DEFAULT_WINDOW_BACKGROUND = '#101215';

/** The workbench window and the origin it may navigate to once the WebUI address is known. */
interface WorkbenchWindow {
  window: BrowserWindow;
  /** Allows navigation to the WebUI origin; before this the window stays on the startup page. */
  allowWebUi: (webUiUrl: string) => void;
}

/** Creates the single workbench window: it shows the startup page first and the WebUI once Core is ready. */
function createWorkbenchWindow(background: string): WorkbenchWindow {
  const window = new BrowserWindow({
    width: INITIAL_WINDOW_WIDTH,
    height: INITIAL_WINDOW_HEIGHT,
    minWidth: MINIMUM_WINDOW_WIDTH,
    minHeight: MINIMUM_WINDOW_HEIGHT,
    resizable: true,
    // The WebUI renders its own title bar and window controls, so the native frame is removed.
    // WebUI 自行渲染标题栏与窗口控制，因此移除原生边框。
    frame: false,
    titleBarStyle: process.platform === 'darwin' ? 'hidden' : 'default',
    // Only WebUI provides visible application chrome and controls.
    // 可见应用边框与控件全部由 WebUI 提供。
    autoHideMenuBar: true,
    // The window appears with the startup page's first paint, so it never shows an empty frame.
    // 窗口随启动页首帧出现，因此绝不显示空白框架。
    show: false,
    backgroundColor: background,
    ...windowIconOptions(),
    webPreferences: {
      // The preload is CommonJS because a sandboxed renderer cannot load an ESM preload.
      // 预加载为 CommonJS，因为沙箱渲染进程无法加载 ESM 预加载。
      preload: path.join(import.meta.dirname, '..', 'preload.cjs'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
    },
  });
  window.setMenu(null);
  window.once('ready-to-show', () => window.show());
  // Page-initiated navigation stays within the WebUI origin; none is allowed while the startup page is shown.
  // 页面发起的导航限于 WebUI 源；显示启动页期间不允许任何导航。
  let webUiOrigin: string | null = null;
  const restrictNavigation = (event: Electron.Event, url: string) => {
    if (webUiOrigin === null || new URL(url).origin !== webUiOrigin) {
      event.preventDefault();
    }
  };
  window.webContents.on('will-navigate', restrictNavigation);
  window.webContents.on('will-redirect', restrictNavigation);
  // Linux window managers apply maximize and restore asynchronously, so isMaximized() can still report the old
  // state during the resize; the state events fire once the change landed and make the WebUI read it again.
  // Linux 窗口管理器异步应用最大化与还原，resize 期间 isMaximized() 仍可能返回旧状态；状态事件在变化
  // 完成后触发，让 WebUI 重新读取。
  const notifyResized = () => {
    if (!window.isDestroyed()) {
      window.webContents.send(WINDOW_EVENT_CHANNELS.resized);
    }
  };
  window.on('resize', notifyResized);
  window.on('maximize', notifyResized);
  window.on('unmaximize', notifyResized);
  window.on('restore', notifyResized);
  // The WebUI drops its window-button reservation in full screen, so both transitions make it read the state again.
  // WebUI 在全屏时取消窗口按钮预留，因此两次切换都让它重新读取状态。
  window.on('enter-full-screen', notifyResized);
  window.on('leave-full-screen', notifyResized);
  if (process.platform === 'darwin') convertTitlebarZoomToFullScreen(window);
  window.on('focus', () => {
    if (!window.isDestroyed()) {
      window.webContents.send(WINDOW_EVENT_CHANNELS.focusChanged);
    }
  });
  window.on('blur', () => {
    if (!window.isDestroyed()) {
      window.webContents.send(WINDOW_EVENT_CHANNELS.focusChanged);
    }
  });
  // External links open in the system browser instead of inside the application frame.
  // 外部链接在系统浏览器中打开，而不是在应用框架内打开。
  window.webContents.setWindowOpenHandler(({ url }) => {
    if (['http:', 'https:'].includes(new URL(url).protocol)) {
      void shell.openExternal(url);
    }
    return { action: 'deny' };
  });
  return { window, allowWebUi: (webUiUrl) => { webUiOrigin = new URL(webUiUrl).origin; } };
}

// convertTitlebarZoomToFullScreen makes a title strip double click act like the green button. Chromium answers a
// double click on a drag region itself with the system zoom and the page never sees it; Electron only reports the
// finished zoom as `maximize`, so the window then enters native full screen, and leaving it undoes the zoom.
// convertTitlebarZoomToFullScreen 使标题条双击与绿色按钮效果相同。Chromium 自行以系统缩放响应拖动区域上的双击，
// 页面收不到该事件；Electron 只把完成的缩放报告为 `maximize`，因此窗口随即进入原生全屏，离开全屏时撤销缩放。
// window is the macOS workbench window; the listeners live as long as it does.
// window 为 macOS 工作台窗口；监听器与其生命周期相同。
function convertTitlebarZoomToFullScreen(window: BrowserWindow): void {
  // zoomedIntoFullScreen marks a full screen entered from a zoom, which leaving full screen must undo.
  // zoomedIntoFullScreen 标记由缩放进入的全屏，离开全屏时必须撤销该缩放。
  let zoomedIntoFullScreen = false;
  window.on('maximize', () => {
    if (window.isDestroyed() || window.isFullScreen()) return;
    zoomedIntoFullScreen = true;
    window.setFullScreen(true);
  });
  window.on('leave-full-screen', () => {
    if (!zoomedIntoFullScreen || window.isDestroyed()) return;
    zoomedIntoFullScreen = false;
    if (window.isMaximized()) window.unmaximize();
  });
}

/** Serializes maximize toggles so rapid mixed clicks cannot interleave. */
// A rapid sequence of toggle commands must converge to a single well-defined state instead of
// racing two native maximize transitions.
// 快速连续的切换命令必须收敛到单一明确状态，而不是让两次原生最大化转换互相竞争。
let maximizeQueue: Promise<void> = Promise.resolve();

// RESTART_WAIT_MS bounds how long the client waits for the old Core to exit (the service's `OWNER_EXIT_WAIT`).
// RESTART_WAIT_MS 限定客户端等待旧 Core 退出的时长（服务端 `OWNER_EXIT_WAIT`）。
const RESTART_WAIT_MS = 120_000;

// SETUP_WAIT_MS bounds how long the client waits for the setup program to finish an update and
// for the service started after it; the client relaunches when it runs out.
// SETUP_WAIT_MS 限定客户端等待安装器完成更新及其之后启动的服务的时长；超时后客户端照常重新启动。
const SETUP_WAIT_MS = 15 * 60_000;

// restartForUpdate ends this client once the restart announced by the Core allows it (see
// awaitUpdateRestart): an update or a setup run relaunches the client from the stable entry, so the
// new version starts; an installer starts the app itself when it finishes, so the client only quits.
// restartForUpdate 在 Core 宣布的重启允许时结束本客户端（见 awaitUpdateRestart）：更新或安装器运行会从
// 稳定入口重新启动客户端以运行新版本；安装包结束时会自行启动应用，因此客户端只退出。
// Parameters: reason comes from the service event, discoveryPath names the authoritative discovery file.
// 参数：reason 来自服务事件，discoveryPath 指明权威发现文件。
async function restartForUpdate(reason: UpdateRestartReason, discoveryPath: string): Promise<void> {
  const relaunch = await awaitUpdateRestart(
    reason,
    guiState.instance?.pid ?? null,
    async () => (await readLiveServiceInstance(discoveryPath).catch(() => null))?.pid ?? null,
    {
      exitWaitMs: RESTART_WAIT_MS,
      setupWaitMs: SETUP_WAIT_MS,
      pollMs: 500,
      now: Date.now,
      sleep: (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
    },
  );
  if (relaunch) relaunchFromStableEntry();
  app.exit(0);
}

// relaunchFromStableEntry starts the client again once this process exits, from the entry that
// always runs the current version.
// relaunchFromStableEntry 在本进程退出后再次启动客户端，使用始终运行当前版本的入口。
function relaunchFromStableEntry(): void {
  const entry = resolveStableDesktopEntry(process.execPath, process.platform, existsSync);
  // The relaunched client inherits this environment; the layout this version recorded there would
  // keep the new version on this version's manifest, so the entry reads the one beside itself.
  // 重新启动的客户端会继承本进程环境；本版本记录在其中的布局会让新版本沿用本版本的清单，因此改由入口
  // 读取其自身旁边的清单。
  if (entry) {
    delete process.env[RUNTIME_LAYOUT_ENV];
    app.relaunch({ execPath: entry, args: [] });
  } else {
    app.relaunch();
  }
}

/** Registers every main-process handler for one window. */
function registerWindowIpcHandlers(window: BrowserWindow, webUiUrl: string, discoveryPath: string, serviceBaseUrl: string): void {
  // blockedBrowserOrigins grows with confirmed Core generations so a restarted Core cannot be
  // opened as an untrusted temporary page when it binds a different port.
  // blockedBrowserOrigins 随已确认的 Core 代次增长，避免重启后换端口的 Core 被临时页面打开。
  const blockedBrowserOrigins = new Set([new URL(webUiUrl).origin, new URL(serviceBaseUrl).origin]);
  // Every bridge handler accepts only the selected workbench main frame, including after navigation.
  // 每个桥处理器仅接受选定工作台主框架，导航后同样核对。
  const handle = <Args extends unknown[]>(channel: string, listener: (event: Electron.IpcMainInvokeEvent, ...args: Args) => unknown): void => {
    ipcMain.handle(channel, (event, ...args) => {
      if (event.sender !== window.webContents || event.senderFrame !== window.webContents.mainFrame
        || new URL(event.senderFrame.url).origin !== new URL(webUiUrl).origin) {
        throw new Error('Desktop capability is restricted to the workbench main frame.');
      }
      return listener(event, ...args as Args);
    });
  };
  // The owner-checked IPC boundary lets the renderer edit text without unrestricted native access.
  // 已核对所有者的 IPC 边界让渲染端编辑文本，同时不开放无限制的原生访问。
  handle(CLIPBOARD_CHANNELS.readText, () => clipboard.readText());
  handle(CLIPBOARD_CHANNELS.writeText, (_event, value: string) => {
    if (typeof value !== 'string') throw new TypeError('Clipboard text must be a string.');
    clipboard.writeText(value);
  });
  handle(CLIPBOARD_CHANNELS.paste, () => window.webContents.paste());
  handle(CLIPBOARD_CHANNELS.readFileReferences, async () => {
    // Read only native file references and metadata; directory contents and file bytes stay on disk.
    // 内读取原生文件引用及元数据，目录内容与文件字节留在磁盘。
    if (!await clipboard.has('text/uri-list')) return [];
    const items = await clipboard.read();
    const lists = await Promise.all(items.filter(item => item.types.includes('text/uri-list'))
      .map(async item => {
        const payload = await item.getType('text/uri-list');
        if (!('text' in payload)) throw new TypeError('Native file references must be a URI list.');
        return payload.text();
      }));
    const paths = lists.flatMap(list => list.split(/\r?\n/).filter(line => line.startsWith('file://')).map(uri => fileURLToPath(uri)));
    return Promise.all(paths.map(async filePath => ({
      path: filePath,
      name: path.basename(filePath),
      directory: await stat(filePath).then(info => info.isDirectory(), () => false),
    })));
  });
  handle(SERVICE_CHANNELS.connection, async () => {
    // Re-read the authoritative discovery path on reconnect; never search another user namespace.
    // 重连时重读权威发现路径，绝不搜索另一个用户命名空间。
    const instance = await readLiveServiceInstance(discoveryPath);
    if (!instance || instance.status !== 'ready') {
      throw new Error('The selected Core service is not ready.');
    }
    blockedBrowserOrigins.add(new URL(instance.baseUrl).origin);
    guiState.instance = instance;
    return instance;
  });
  // The WebUI reports its language and appearance for the next startup page; the window background follows at once.
  // WebUI 为下一次启动页上报其语言与外观；窗口背景随即跟随。
  handle(STARTUP_CHANNELS.reportAppearance, (_event, report: unknown) => {
    const appearance = parseStartupAppearance(report);
    if (!appearance) throw new TypeError('Invalid startup appearance.');
    rememberStartupAppearance(appearance);
    if (!window.isDestroyed()) window.setBackgroundColor(appearance.ground);
  });
  handle(SERVICE_CHANNELS.restartForUpdate, (_event, reason: unknown) => {
    if (!isUpdateRestartReason(reason)) throw new TypeError('Unknown restart reason.');
    void restartForUpdate(reason, discoveryPath);
  });
  // Temporary browser pages run in a separate in-memory partition with no workbench preload.
  // 临时浏览页面运行在独立的内存分区中，不带工作台预加载桥。
  const localBrowser = new LocalBrowserManager(
    window,
    blockedBrowserOrigins,
    (snapshot) => window.webContents.send(LOCAL_BROWSER_CHANNELS.changed, snapshot),
  );
  window.on('close', () => localBrowser.dispose());
  handle(LOCAL_BROWSER_CHANNELS.initialize, () => localBrowser.initialize());
  handle(LOCAL_BROWSER_CHANNELS.setOwner, (_event, ownerSessionId: string | null) => localBrowser.setOwner(ownerSessionId));
  handle(LOCAL_BROWSER_CHANNELS.snapshot, () => localBrowser.snapshot());
  handle(LOCAL_BROWSER_CHANNELS.open, (_event, url: string) => localBrowser.open(url));
  handle(LOCAL_BROWSER_CHANNELS.navigate, (_event, id: string, url: string) => localBrowser.navigate(id, url));
  handle(LOCAL_BROWSER_CHANNELS.history, (_event, id: string, action: 'back' | 'forward' | 'reload' | 'stop') => localBrowser.history(id, action));
  handle(LOCAL_BROWSER_CHANNELS.move, (_event, id: string, targetId: string) => localBrowser.move(id, targetId));
  handle(LOCAL_BROWSER_CHANNELS.openDeveloperTools, (_event, id: string) => localBrowser.openDeveloperTools(id));
  handle(LOCAL_BROWSER_CHANNELS.pickElement, (_event, id: string) => localBrowser.pickElement(id));
  handle(LOCAL_BROWSER_CHANNELS.cancelElementPick, (_event, id: string) => localBrowser.cancelElementPick(id));
  handle(LOCAL_BROWSER_CHANNELS.openExternal, (_event, id: string) => localBrowser.openExternal(id));
  handle(LOCAL_BROWSER_CHANNELS.openExternalUrl, (_event, url: string) => localBrowser.openExternalUrl(url));
  handle(LOCAL_BROWSER_CHANNELS.select, (_event, id: string) => localBrowser.select(id));
  handle(LOCAL_BROWSER_CHANNELS.close, (_event, id: string) => localBrowser.close(id));
  // The trusted workbench owns confirmation UI; the native manager still validates and isolates the owner.
  // 可信工作台负责确认界面；原生管理器仍校验并隔离会话归属。
  handle(LOCAL_BROWSER_CHANNELS.closeOwner, (_event, ownerSessionId: string, confirmed: boolean) => {
    if (typeof confirmed !== 'boolean') throw new TypeError('Browser close confirmation must be a boolean.');
    return localBrowser.closeOwner(ownerSessionId, async () => confirmed);
  });
  handle(LOCAL_BROWSER_CHANNELS.viewport, (_event, viewport: LocalBrowserViewport | null) =>
    localBrowser.setViewport(viewport));
  // File references reach the OS only after the main process rechecks the path and refuses runnable files.
  // 文件引用只有在主进程复核路径并拒绝可运行文件后才交给操作系统。
  handle(LOCAL_FILE_CHANNELS.inspect, (_event, filePath: unknown) => inspectLocalFile(filePath));
  handle(LOCAL_FILE_CHANNELS.openWithDefaultApp, (_event, filePath: unknown) =>
    openLocalFileWithDefaultApp(filePath, (target) => shell.openPath(target)));
  handle(LOCAL_FILE_CHANNELS.showInFolder, (_event, filePath: unknown) =>
    showLocalFileInFolder(filePath, (target) => shell.showItemInFolder(target)));
  // The shared handler guard limits DevTools activation to this workbench's trusted main frame.
  // 共用处理器校验将开发者工具的打开权限限定于此工作台的可信主框架。
  handle(WINDOW_COMMAND_CHANNELS.openDeveloperTools, () => {
    if (window.isDestroyed()) {
      throw new Error('The workbench window is no longer available.');
    }
    window.webContents.openDevTools({ mode: 'detach', activate: true });
  });
  handle(WINDOW_COMMAND_CHANNELS.minimize, () => {
    window.minimize();
  });
  handle(WINDOW_COMMAND_CHANNELS.toggleMaximize, () => {
    maximizeQueue = maximizeQueue.then(() => {
      if (window.isDestroyed()) {
        return;
      }
      if (window.isMaximized()) {
        window.unmaximize();
      } else {
        window.maximize();
      }
    });
    return maximizeQueue;
  });
  handle(WINDOW_COMMAND_CHANNELS.isMaximized, () => !window.isDestroyed() && window.isMaximized());
  handle(WINDOW_COMMAND_CHANNELS.toggleFullScreen, () => {
    if (!window.isDestroyed()) window.setFullScreen(!window.isFullScreen());
  });
  handle(WINDOW_COMMAND_CHANNELS.isFullScreen, () => !window.isDestroyed() && window.isFullScreen());
  // The buttons sit right of the activity rail, centred in the title strip, with the same gap on either side;
  // the answer is their right edge so the WebUI clears exactly that much. Electron renders at a zoom factor of
  // one, so CSS pixels equal window points.
  // 按钮位于活动轨道右侧，在标题条中垂直居中，两侧间距相同；返回其右边缘，使 WebUI 恰好让出该宽度。
  // Electron 以缩放系数 1 渲染，因此 CSS 像素等于窗口点数。
  handle(WINDOW_COMMAND_CHANNELS.placeNativeButtons, (_event, report: unknown) => {
    if (process.platform !== 'darwin') throw new Error('Native window buttons exist only on macOS.');
    const placement = parseNativeButtonsPlacement(report);
    if (!placement) throw new TypeError('Invalid native window button placement.');
    const inset = Math.floor((placement.height - TRAFFIC_LIGHT_FRAME_HEIGHT) / 2);
    const x = Math.round(placement.left) + inset;
    if (!window.isDestroyed()) window.setWindowButtonPosition({ x, y: inset });
    return x + TRAFFIC_LIGHTS_WIDTH + inset;
  });
  handle(WINDOW_COMMAND_CHANNELS.isMinimized, () => !window.isDestroyed() && window.isMinimized());
  handle(WINDOW_COMMAND_CHANNELS.isFocused, () => !window.isDestroyed() && window.isFocused());
  handle(WINDOW_COMMAND_CHANNELS.isVisible, () => !window.isDestroyed() && window.isVisible());
  handle(WINDOW_COMMAND_CHANNELS.close, () => {
    if (!window.isDestroyed()) {
      // Closing the GUI window is local; the independent Core service keeps running.
      // 关闭 GUI 窗口是本地的；独立 Core 服务继续运行。
      window.close();
    }
  });
  handle(NOTIFICATION_CHANNELS.permission, () =>
    canUseNativeNotifications() && Notification.isSupported() ? 'granted' : 'unsupported',
  );
  handle(NOTIFICATION_CHANNELS.requestPermission, () =>
    canUseNativeNotifications() && Notification.isSupported() ? 'granted' : 'unsupported',
  );
  handle(
    NOTIFICATION_CHANNELS.show,
    async (_event, request: { id: string; title: string; body: string }) => {
      if (!canUseNativeNotifications() || !Notification.isSupported()) {
        throw new Error('Native notifications are not supported on this host.');
      }
      if (typeof request?.id !== 'string' || !/^[A-Za-z0-9:._-]{1,64}$/.test(request.id)
        || typeof request.title !== 'string' || request.title.length > 256
        || typeof request.body !== 'string' || request.body.length > 2048) {
        throw new Error('Invalid native notification request.');
      }
      // The stable native id preserves the WebUI's exact activation correlation key.
      // 稳定的原生 ID 保留 WebUI 用于激活关联的精确键。
      const notification = new Notification({
        id: request.id, title: request.title, body: request.body,
        ...(process.platform === 'win32' ? {
          toastXml: createWindowsToastXml(request.id, request.title, request.body),
        } : {}),
      });
      retainNativeNotification(request.id, notification);
      // Electron can deliver an in-process Windows body click only through the instance event;
      // COM activation calls the process-wide handler as well. The route removes its id on first use.
      // Electron 可能仅通过对象事件投递进程内的 Windows 正文点击；COM 激活也会调用进程级处理器。
      // 路由首次使用时移除 ID，因此两条路径不会重复打开业务内容。
      notification.on('click', () => activateNativeNotification(request.id));
      // Windows also emits close when only the banner times out while the same toast remains in
      // Action Center. Releasing the object here would discard its later click callback.
      // Windows 仅横幅超时、通知仍在通知中心时也会发出 close；此处释放对象会丢失后续点击回调。
      // Windows builds the toast asynchronously; the IPC call succeeds only after native delivery.
      // Windows 异步创建通知；IPC 仅在原生投递得到确认后返回成功。
      await new Promise<void>((resolve, reject) => {
        // timeout prevents a native backend that emits neither result from hanging the settings UI.
        // timeout 防止原生后端不发出任何结果时设置界面永久等待。
        const timeout = setTimeout(() => {
          nativeNotifications.delete(request.id);
          reject(new Error('Native notification delivery was not confirmed.'));
        }, 10_000);
        notification.once('show', () => {
          clearTimeout(timeout);
          resolve();
        });
        notification.once('failed', (_failedEvent, error) => {
          clearTimeout(timeout);
          nativeNotifications.delete(request.id);
          reject(new Error(`Native notification delivery failed: ${error}`));
        });
        try {
          notification.show();
        } catch (error) {
          clearTimeout(timeout);
          nativeNotifications.delete(request.id);
          reject(error);
        }
      });
    },
  );
}

/** Runs the GUI startup sequence and reports real failures to the user. */
async function runDesktopGui(): Promise<void> {
  const options = parseDesktopGuiOptions(process.argv.slice(1));
  const appearance = readStartupAppearance();
  const locale = startupLocale(appearance);
  const text = desktopText(locale);
  const { window, allowWebUi } = createWorkbenchWindow(appearance?.ground ?? DEFAULT_WINDOW_BACKGROUND);
  guiState.window = window;
  window.once('closed', () => {
    guiState.window = null;
  });
  try {
    await loadStartupPage(window, locale, appearance);
  } catch (error) {
    // Without its startup page the window has nothing to show, so the failure goes to a native dialog.
    // 缺少启动页时窗口无内容可显示，因此失败改由原生对话框呈现。
    dialog.showErrorBox('Vulcan Code', error instanceof Error ? error.message : String(error));
    app.quit();
    return;
  }
  try {
    const executable = await resolveServiceExecutable(options.serviceExecutable, app.isPackaged);
    const description = await readServiceLaunchDescription(executable, options.forwardedArguments);
    const instance = await ensureServiceInstance(
      description,
      options.forwardedArguments,
      (progress: ServiceStartupProgress) => reportStartupStage(window, progress),
      text,
    );
    guiState.instance = instance;
    const webUiUrl = resolveWebUiUrl(description, instance, options.webUiUrl);
    allowWebUi(webUiUrl);
    registerWindowIpcHandlers(window, webUiUrl, description.discoveryPath, instance.baseUrl);
    try {
      await loadWorkbenchPage(window, webUiUrl);
    } catch (error) {
      throw new Error(formatDesktopText(text.workbenchLoadFailed, { url: webUiUrl, error: String(error) }));
    }
    // Startup failures use the startup page; subsequent renderer loss uses native recovery.
    // 启动失败使用启动页；后续渲染进程退出使用原生恢复入口。
    registerRendererRecovery(window, webUiUrl, description.dataRoot, locale);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    // The process stays alive so the user can read the real failure instead of a silent exit.
    // 进程保持存活，使用户能读到真实失败信息，而不是静默退出。
    await showStartupFailure(window, locale, appearance, message).catch((pageError: unknown) => {
      dialog.showErrorBox('Vulcan Code', `${message}\n\n${String(pageError)}`);
    });
    throw error;
  }
}

// The GUI itself is single-instance: a second launch focuses the existing window.
// GUI 自身为单实例：第二次启动会聚焦既有窗口。
if (!app.requestSingleInstanceLock()) {
  app.quit();
} else {
  // quitting marks a client that is closing, so a launch arriving meanwhile starts it again.
  // quitting 标记正在关闭的客户端，使期间到达的启动请求能再次启动它。
  let quitting = false;
  app.on('before-quit', () => {
    quitting = true;
  });
  app.on('second-instance', () => {
    // The one window shows the startup page, its failure or the WebUI, so a relaunch always brings it forward.
    // 唯一的窗口显示启动页、启动失败或 WebUI，因此再次启动总会把它带到前台。
    const window = guiState.window;
    if (!window || window.isDestroyed()) {
      // The launching process has already exited with its request handed over, so a client that
      // is closing starts again from the stable entry instead of dropping the click.
      // 发起启动的进程已交出请求并退出，因此正在关闭的客户端改为从稳定入口再次启动，而不是丢弃这次点击。
      process.stderr.write(`[vulcan-code-desktop] launch request without a window (quitting: ${quitting})\n`);
      if (quitting) relaunchFromStableEntry();
      return;
    }
    if (window.isMinimized()) {
      window.restore();
    }
    window.show();
    window.focus();
  });
  app.whenReady().then(() => {
    applyDockIcon();
    if (process.platform === 'win32') {
      // Register before the workbench starts so Electron can replay a pending native activation.
      // 在工作台启动前注册，使 Electron 能重放待处理的原生激活事件。
      Notification.handleActivation((details) => {
        const id = parseWindowsNotificationActivation(details);
        if (id) activateNativeNotification(id);
      });
    }
    void runDesktopGui().catch((error: unknown) => {
      process.stderr.write(`[vulcan-code-desktop] startup failed: ${String(error)}\n`);
    });
  });
  app.on('window-all-closed', () => {
    // Closing the last GUI window ends the client process only, never the Core service.
    // 关闭最后一个 GUI 窗口只结束客户端进程，绝不结束 Core 服务。
    app.quit();
  });
}
