// Desktop startup page shown in the workbench window while Core starts.
// Core 启动期间显示在工作台窗口中的桌面启动页。
//
// The page is the WebUI startup screen built on its own (`apps/vulcan-code-web/vite.desktop-startup.config.ts`), so the
// window shows the same startup surface from local files before Core serves the WebUI and then loads the WebUI in place.
// Every stage and failure shown is a real reported value. The language and appearance come from what the WebUI last
// reported, remembered in a small file beside Electron's user data; the file is a display cache that a later report
// rewrites, so an unreadable or unknown file is ignored.
// 该页面是单独构建的 WebUI 启动界面（`apps/vulcan-code-web/vite.desktop-startup.config.ts`），使窗口在 Core 提供
// WebUI 之前即可从本地文件显示同样的启动层，随后原位载入 WebUI。显示的每个阶段与失败都是真实上报值。语言与外观来自
// WebUI 最近一次上报，记在 Electron 用户数据旁的小文件中；该文件是显示缓存，之后的上报会重写它，因此无法读取或未知的
// 文件会被忽略。

import { app, type BrowserWindow } from 'electron';
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import path from 'node:path';

import type { DesktopLocale } from './desktop-text.js';
import type { ServiceStartupProgress } from './service-launcher.js';
import { APPEARANCE_FILE_VERSION, parseStartupAppearance, type StartupAppearance } from './startup-appearance.js';

// STARTUP_PAGE_FILE is the page `scripts/desktop-runtime.mjs` copies into `startup/` beside `gui/`.
// STARTUP_PAGE_FILE 是 `scripts/desktop-runtime.mjs` 复制到 `gui/` 旁 `startup/` 中的页面。
export const STARTUP_PAGE_FILE = 'desktop-startup.html';

// startupPageCandidates lists where the page may be: beside `gui/` when packaged, then the WebUI build output for a
// development build running from `apps/vulcan-code-desktop/dist/gui`.
// startupPageCandidates 列出页面可能的位置：打包时位于 `gui/` 旁，其次是从 `apps/vulcan-code-desktop/dist/gui`
// 运行的开发构建所用的 WebUI 构建产物。
// Parameters: guiDirectory is the directory of the compiled GUI modules.
// 参数：guiDirectory 为已编译 GUI 模块所在目录。
export function startupPageCandidates(guiDirectory: string): string[] {
  return [
    path.join(guiDirectory, '..', 'startup', STARTUP_PAGE_FILE),
    path.join(guiDirectory, '..', '..', '..', 'vulcan-code-web', 'dist', 'desktop-startup', STARTUP_PAGE_FILE),
  ];
}

// appearanceFile names the remembered appearance file in Electron's user data directory.
// appearanceFile 指明 Electron 用户数据目录中所记外观的文件。
function appearanceFile(): string {
  return path.join(app.getPath('userData'), 'startup-appearance.json');
}

// readStartupAppearance returns the remembered appearance, or null when none is usable.
// readStartupAppearance 返回所记外观；没有可用记录时返回 null。
export function readStartupAppearance(): StartupAppearance | null {
  try {
    const stored = JSON.parse(readFileSync(appearanceFile(), 'utf8')) as Record<string, unknown>;
    return stored.version === APPEARANCE_FILE_VERSION ? parseStartupAppearance(stored) : null;
  } catch {
    return null;
  }
}

// rememberStartupAppearance replaces the remembered appearance atomically.
// rememberStartupAppearance 以原子方式替换所记外观。
// Parameters: appearance is a validated report.
// 参数：appearance 为已校验的上报。
export function rememberStartupAppearance(appearance: StartupAppearance): void {
  const file = appearanceFile();
  const pending = `${file}.tmp`;
  mkdirSync(path.dirname(file), { recursive: true });
  writeFileSync(pending, JSON.stringify({ version: APPEARANCE_FILE_VERSION, ...appearance }), 'utf8');
  renameSync(pending, file);
}

// loadStartupPage shows the startup page in the window with the remembered language and appearance.
// loadStartupPage 以所记语言与外观在窗口中显示启动页。
// Parameters: window is the workbench window; locale is the language the shell resolved; appearance may be null.
// 参数：window 为工作台窗口；locale 为桌面壳解析出的语言；appearance 可为 null。
// Throws when the page is missing, which means the desktop app was packaged without it.
// 页面缺失时抛出错误，这意味着桌面应用打包时遗漏了它。
export async function loadStartupPage(
  window: BrowserWindow,
  locale: DesktopLocale,
  appearance: StartupAppearance | null,
): Promise<void> {
  const candidates = startupPageCandidates(import.meta.dirname);
  const page = candidates.find((candidate) => existsSync(candidate));
  if (!page) throw new Error(`The desktop startup page is missing; looked in ${candidates.join(', ')}.`);
  const query: Record<string, string> = { locale };
  if (appearance) Object.assign(query, { theme: appearance.theme, text: appearance.text, motion: appearance.motion });
  await window.loadFile(page, { query });
}

// isStartupPageShown reports whether the window currently shows the startup page.
// isStartupPageShown 报告窗口当前是否显示启动页。
function isStartupPageShown(window: BrowserWindow): boolean {
  const url = window.webContents.getURL();
  return url.startsWith('file:') && new URL(url).pathname.endsWith(`/${STARTUP_PAGE_FILE}`);
}

// callStartupPage runs one call of the page's `window.vulcanDesktopStartup` interface with a JSON-encoded message.
// callStartupPage 以 JSON 编码的消息调用页面 `window.vulcanDesktopStartup` 接口的一个方法。
function callStartupPage(window: BrowserWindow, method: 'progress' | 'failed', payload: ServiceStartupProgress | string): Promise<unknown> {
  return window.webContents.executeJavaScript(
    `window.vulcanDesktopStartup && window.vulcanDesktopStartup.${method}(${JSON.stringify(payload)});`,
    true,
  );
}

// reportStartupStage shows one real startup stage on the startup page.
// reportStartupStage 在启动页显示一个真实启动阶段。
// Parameters: window owns the startup page and progress carries Core state or localized launcher text.
// 参数：window 持有启动页，progress 携带 Core 状态或本地化启动器文本。
export function reportStartupStage(window: BrowserWindow, progress: ServiceStartupProgress): void {
  if (window.isDestroyed() || !isStartupPageShown(window)) return;
  void callStartupPage(window, 'progress', progress);
}

// showStartupFailure shows the failure on the startup page, returning to the page when the window had already left it
// (a failed WebUI load).
// showStartupFailure 在启动页显示失败；若窗口已离开启动页（WebUI 载入失败），则先回到启动页。
// Parameters: window is the workbench window; locale and appearance reload the page; message is the failure.
// 参数：window 为工作台窗口；locale 与 appearance 用于重新载入页面；message 为失败内容。
export async function showStartupFailure(
  window: BrowserWindow,
  locale: DesktopLocale,
  appearance: StartupAppearance | null,
  message: string,
): Promise<void> {
  if (window.isDestroyed()) return;
  if (!isStartupPageShown(window)) await loadStartupPage(window, locale, appearance);
  await callStartupPage(window, 'failed', message);
  window.show();
}
