import React from "react";
import { RotateCw, X } from "lucide-react";
import { StartupLogoMark } from "@/components/shell/startup-logo-mark";
import { readElectronDesktopBridge } from "@/host-shell/electron-bridge";
import { StartupProgressPanel, type StartupProgressLabels } from "@/components/shell/startup-progress-panel";
import type { StartupSnapshot } from "@/types";
import { resolveAppLocale, type LocaleCode } from "@/i18n-locales";
import defaultBootCatalog from "@catalogs/boot/en-US.json";

// BootText stores the minimal boot-screen labels: the shared progress copy plus the boot failure copy.
// BootText 保存最小启动屏文案：共用进度文案加上启动失败文案。
export interface BootText extends StartupProgressLabels {
  // Localized skill download wording is available before the main language catalog loads.
  // 完整语言目录加载前即可使用本地化技能下载文案。
  downloadingSkill: string;
  // Startup failure prefix.
  // 启动失败前缀。
  failedPrefix: string;
  // Label for optional raw failure details, keeping the recovery action visible.
  // 可选原始错误详情的标签，让恢复操作保持可见。
  errorDetails: string;
  // Explicit recovery action shown after a startup failure.
  // 启动失败后显示的显式恢复操作。
  reloadApplication: string;
  // Accessible label of the desktop startup page's close button.
  // 桌面启动页关闭按钮的无障碍标签。
  closeWindow: string;
}

// DEFAULT_BOOT_TEXT is the English boot catalog; assigning it proves the shared catalog covers BootText.
// DEFAULT_BOOT_TEXT 是英文启动目录；该赋值证明共享目录覆盖 BootText。
const DEFAULT_BOOT_TEXT: BootText = defaultBootCatalog;

// BOOT_CATALOGS bundles the small shared boot catalogs without importing the full application catalogs.
// BOOT_CATALOGS 打包体积很小的共享启动目录，而不导入完整应用目录。
const BOOT_CATALOGS = new Map(
  Object.entries(import.meta.glob<Partial<BootText>>("@catalogs/boot/*.json", { eager: true, import: "default" }))
    .map(([path, catalog]) => [/([a-z]{2}-[A-Z]{2})\.json$/u.exec(path)?.[1], catalog] as const),
);

// bootText returns the boot labels of one language, completing missing keys from English.
// bootText 返回一种语言的启动文案，并用英文补全缺失的键。
export function bootText(locale: LocaleCode): BootText {
  return { ...DEFAULT_BOOT_TEXT, ...BOOT_CATALOGS.get(locale) };
}

// STARTUP_SURFACE_FADE_MS is the single duration of the startup surface hand-off fade; the stylesheet reads it
// through --startup-surface-fade.
// STARTUP_SURFACE_FADE_MS 是启动层交接淡出的唯一时长；样式表通过 --startup-surface-fade 读取。
export const STARTUP_SURFACE_FADE_MS = 320;

// BootLoadingScreen renders the persistent startup surface with real startup progress. WebuiApp keeps exactly one
// instance mounted from the first probe until the workbench takes over, so the logo motion plays once per page load;
// on hand-off it fades out over the freshly mounted workbench and then reports that it has left.
// BootLoadingScreen 渲染带真实启动进度的常驻启动层。WebuiApp 从首次探测到工作台接管期间只挂载一个实例，
// 使标志动效每次页面加载只播放一次；交接时它在刚挂载的工作台上方淡出，随后报告已离场。
export function BootLoadingScreen(props: {
  // Startup snapshot received before the workbench bundle is loaded.
  // 工作台包加载前收到的启动快照。
  startup?: StartupSnapshot | null;
  // Language used while no snapshot carries one, such as the desktop startup page before Core answers.
  // 尚无快照携带语言时使用的语言，例如 Core 应答前的桌面启动页。
  uiLocale?: string | null;
  // Host-reported detail shown while no snapshot exists yet.
  // 尚无快照时显示的宿主上报详情。
  detail?: string | null;
  // True once the workbench owns the screen; the surface fades out and stops intercepting input.
  // 工作台接管屏幕后为 true；启动层淡出并停止拦截输入。
  leaving?: boolean;
  // Called after the fade-out so the owner can unmount the surface.
  // 淡出结束后调用，使持有者可以卸载启动层。
  onLeft?: () => void;
}) {
  const { leaving = false, onLeft } = props;
  // text stores localized labels for the boot screen without importing the full translation catalog.
  // text 保存启动屏本地化文案，而不导入完整翻译表。
  const text = bootText(bootLocale(props.startup?.uiLocale ?? props.uiLocale));

  React.useEffect(() => {
    if (!leaving || !onLeft) {
      return;
    }
    const timer = window.setTimeout(onLeft, STARTUP_SURFACE_FADE_MS);
    return () => window.clearTimeout(timer);
  }, [leaving, onLeft]);

  return (
    <main
      className="loading-screen startup-surface"
      data-leaving={leaving ? "" : undefined}
      aria-hidden={leaving ? true : undefined}
      style={{ "--startup-surface-fade": `${STARTUP_SURFACE_FADE_MS}ms` } as React.CSSProperties}
    >
      <StartupProgressPanel startup={props.startup ?? null} labels={text} detail={props.detail} />
    </main>
  );
}

// BootErrorScreen renders a boot failure before the full workbench bundle is loaded.
// BootErrorScreen 在完整工作台包加载前渲染启动失败状态。
export function BootErrorScreen(props: {
  // Error message rendered after startup fails.
  // 启动失败后展示的错误消息。
  error: string;
  // Service-provided UI locale.
  // 服务提供的界面语言。
  uiLocale?: string | null;
  // onReload explicitly restarts the application only after the user chooses recovery.
  // onReload 仅在用户选择恢复后显式重新加载应用。
  onReload?: () => void;
  // False when the page already draws its own window bar, such as the desktop startup page.
  // 页面已自绘窗口条时为 false，例如桌面启动页。
  windowBar?: boolean;
}) {
  // text stores localized labels for the boot error.
  // text 保存启动错误的本地化文案。
  const text = bootText(bootLocale(props.uiLocale));
  // desktop is the frameless desktop window that needs drawn close and reload buttons, or null.
  // desktop 是需要自绘关闭与刷新按钮的无边框桌面窗口，否则为 null。
  const desktop = props.windowBar === false ? null : framelessDesktopWindow();
  return (
    <main className="loading-screen error">
      {desktop ? (
        <StartupWindowBar
          closeLabel={text.closeWindow}
          onClose={() => void desktop.closeWindow().catch(() => window.close())}
          onReload={props.onReload}
          reloadLabel={text.reloadApplication}
        />
      ) : null}
      <section className="startup-error-panel" aria-live="assertive">
        <StartupLogoMark />
        <div className="startup-error-content">
          <strong>{text.failedPrefix}</strong>
          <details className="startup-error-details">
            <summary>{text.errorDetails}</summary>
            <p className="startup-error-message">{props.error}</p>
          </details>
          {props.onReload ? <button className="startup-reload-button" type="button" onClick={props.onReload}>
            {text.reloadApplication}
          </button> : null}
        </div>
      </section>
    </main>
  );
}

// NATIVE_WINDOW_BUTTONS is true on macOS, whose frameless desktop window keeps its native buttons.
// NATIVE_WINDOW_BUTTONS 在 macOS 上为 true，其无边框桌面窗口保留原生按钮。
export const NATIVE_WINDOW_BUTTONS = typeof navigator !== "undefined" && /Mac/u.test(navigator.userAgent);

// framelessDesktopWindow returns the desktop bridge when this page fills a desktop window without native buttons.
// framelessDesktopWindow 在本页占满无原生按钮的桌面窗口时返回桌面桥。
function framelessDesktopWindow() {
  return NATIVE_WINDOW_BUTTONS ? null : readElectronDesktopBridge();
}

// StartupWindowBar is the drag strip of a frameless desktop window before the workbench draws its own titlebar: an
// optional reload button and a close button at the right edge.
// StartupWindowBar 是工作台自绘标题栏之前无边框桌面窗口的拖动条：右端可选的刷新按钮与关闭按钮。
export function StartupWindowBar(props: {
  // Accessible label of the close button.
  // 关闭按钮的无障碍标签。
  closeLabel: string;
  // Closes the window.
  // 关闭窗口。
  onClose: () => void;
  // Reloads the page; the reload button is shown only when it is given.
  // 重新加载页面；仅在提供时显示刷新按钮。
  onReload?: () => void;
  // Accessible label of the reload button.
  // 刷新按钮的无障碍标签。
  reloadLabel?: string;
}) {
  return (
    <div className="startup-window-bar">
      {props.onReload ? (
        <button aria-label={props.reloadLabel} className="startup-window-button" onClick={props.onReload} title={props.reloadLabel} type="button">
          <RotateCw />
        </button>
      ) : null}
      <button aria-label={props.closeLabel} className="startup-window-button" data-action="close" onClick={props.onClose} title={props.closeLabel} type="button">
        <X />
      </button>
    </div>
  );
}

// bootLocale resolves the saved or device language without importing the full i18n module.
// bootLocale 在不导入完整 i18n 模块的情况下解析已保存语言或设备语言。
function bootLocale(value: string | null | undefined): LocaleCode {
  return resolveAppLocale(value);
}
