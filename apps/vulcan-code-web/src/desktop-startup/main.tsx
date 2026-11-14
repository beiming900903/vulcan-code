import React from "react";
import ReactDOM from "react-dom/client";
import {
  BootErrorScreen,
  BootLoadingScreen,
  bootText,
  NATIVE_WINDOW_BUTTONS,
  StartupWindowBar,
} from "@/app/boot-startup-screen";
import { resolveAppLocale } from "@/i18n-locales";
import type { StartupSnapshot } from "@/types";
import {
  APPEARANCE_TEXT_TIERS,
  APPEARANCE_THEMES,
  applyAppearance,
  DEFAULT_APPEARANCE,
  type AppearanceMotion,
} from "@/ui-kit/appearance";
import "../boot-styles.css";

// The desktop startup page: the WebUI startup surface shown from local files in the desktop window while the desktop
// shell starts or reuses Core. The shell drives it through `window.vulcanDesktopStartup` and then loads the WebUI in the
// same window, whose startup surface continues where this page stops.
// 桌面启动页：桌面壳启动或复用 Core 期间，在桌面窗口中从本地文件显示的 WebUI 启动层。桌面壳通过
// `window.vulcanDesktopStartup` 驱动它，随后在同一窗口载入 WebUI，由 WebUI 的启动层接续本页。

// StartupPageState is what the shell has reported so far.
// StartupPageState 是桌面壳迄今上报的状态。
interface StartupPageState {
  // Latest real stage message, or null before the first stage.
  // 最近的真实阶段消息；首个阶段之前为 null。
  detail: string | null;
  // Latest Core state forwarded by the desktop main process.
  // 桌面主进程转发的最新 Core 状态。
  startup: StartupSnapshot | null;
  // Startup failure message; once set the page shows the failure instead of progress.
  // 启动失败消息；一旦设置，页面显示失败而非进度。
  failure: string | null;
}

// DesktopStartupHost is the page interface the shell calls with `executeJavaScript`.
// DesktopStartupHost 是桌面壳通过 `executeJavaScript` 调用的页面接口。
interface DesktopStartupHost {
  // Displays the launcher message and any Core snapshot supplied for this stage.
  // 显示启动器消息及此阶段提供的 Core 快照。
  progress: (progress: { message: string; snapshot?: Omit<StartupSnapshot, "recentLogs"> }) => void;
  // Shows the startup failure; the page keeps it until the window closes.
  // 显示启动失败；页面保持显示直至窗口关闭。
  failed: (message: string) => void;
}

// state and listeners form the small store the shell writes and React reads.
// state 与 listeners 组成桌面壳写入、React 读取的小型存储。
let state: StartupPageState = { detail: null, startup: null, failure: null };
const listeners = new Set<() => void>();

// update replaces the state and re-renders the page.
// update 替换状态并重新渲染页面。
function update(next: Partial<StartupPageState>): void {
  state = { ...state, ...next };
  listeners.forEach((listener) => listener());
}

(window as unknown as { vulcanDesktopStartup: DesktopStartupHost }).vulcanDesktopStartup = {
  progress: ({ message, snapshot }) => update({
    detail: message,
    startup: snapshot ? { ...snapshot, recentLogs: [] } : null,
  }),
  failed: (message) => update({ failure: message }),
};

// parameters carry the language and appearance the WebUI last reported to the shell.
// parameters 携带 WebUI 最近上报给桌面壳的语言与外观。
const parameters = new URLSearchParams(window.location.search);
// accepted returns the parameter when it is one of the accepted values, otherwise the fallback.
// accepted 在参数属于可接受取值时返回它，否则返回回退值。
function accepted<T extends string>(name: string, values: readonly T[], fallback: T): T {
  const value = parameters.get(name);
  return value !== null && (values as readonly string[]).includes(value) ? (value as T) : fallback;
}
const locale = resolveAppLocale(parameters.get("locale"));
document.documentElement.lang = locale;
applyAppearance({
  ...DEFAULT_APPEARANCE,
  theme: accepted("theme", APPEARANCE_THEMES, DEFAULT_APPEARANCE.theme),
  text: accepted("text", APPEARANCE_TEXT_TIERS, DEFAULT_APPEARANCE.text),
  motion: accepted<AppearanceMotion>("motion", ["system", "reduce"], DEFAULT_APPEARANCE.motion),
});
// DesktopStartupPage renders the startup surface or the startup failure under a drag strip.
// DesktopStartupPage 在拖动条下渲染启动层或启动失败。
function DesktopStartupPage() {
  const current = React.useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    () => state,
  );
  const closeLabel = bootText(locale).closeWindow;
  return (
    <>
      {current.failure === null
        ? <BootLoadingScreen uiLocale={locale} detail={current.detail} startup={current.startup} />
        : <BootErrorScreen error={current.failure} uiLocale={locale} windowBar={false} />}
      {/* macOS keeps its native window buttons on the frameless window, so only other systems need a close button. */}
      {/* macOS 在无边框窗口上保留原生窗口按钮，因此只有其他系统需要关闭按钮。 */}
      {NATIVE_WINDOW_BUTTONS ? null : <StartupWindowBar closeLabel={closeLabel} onClose={() => window.close()} />}
    </>
  );
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <DesktopStartupPage />
  </React.StrictMode>,
);
