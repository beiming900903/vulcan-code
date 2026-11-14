// Desktop GUI service coordination: description, existing-instance reuse, detached start, readiness.
// 桌面 GUI 的服务协调：启动描述、复用既有实例、分离启动与就绪等待。
//
// The GUI never rebuilds the authoritative data root and never guesses a port. It asks the service
// for its own description and only trusts discovery documents whose published identity answers
// /health.
// GUI 绝不重新拼接权威数据根，也不猜测端口。它向服务索取自身描述，并且只信任那些已发布身份能够
// 通过 /health 应答的发现文档。

import { execFile, spawn } from 'node:child_process';
import { readFile, access } from 'node:fs/promises';
import path from 'node:path';
import { promisify } from 'node:util';
import { DEFAULT_DESKTOP_LOCALE, desktopText, formatDesktopText, type DesktopText } from './desktop-text.js';

const execFileAsync = promisify(execFile);

// RUNTIME_LAYOUT_ENV names the runtime layout manifest Core and this client read (service `RUNTIME_LAYOUT_ENV`).
// RUNTIME_LAYOUT_ENV 指明 Core 与本客户端读取的运行时布局清单（服务端 `RUNTIME_LAYOUT_ENV`）。
export const RUNTIME_LAYOUT_ENV = 'VULCAN_RUNTIME_LAYOUT';

/** Startup protocol version this GUI understands. */
export const DESKTOP_SERVICE_PROTOCOL_VERSION = 1;

/** Maximum time to wait for the service description command. */
const DESCRIBE_TIMEOUT_MS = 20_000;

/** Maximum time to wait for a started service to publish ready. */
const READY_TIMEOUT_MS = 120_000;

/** Poll interval while waiting for readiness. */
const READY_POLL_INTERVAL_MS = 100;

/** Per-request timeout when probing the service health endpoint. */
const HEALTH_TIMEOUT_MS = 2_000;

/** Maximum wait for an optional startup snapshot before resuming discovery.
 * 等待可选启动快照的最长时间，到期继续发现。 */
const STARTUP_SNAPSHOT_TIMEOUT_MS = 500;

/** One validated service launch description. */
export interface ServiceLaunchDescription {
  /** Launch protocol version understood by this GUI. */
  protocolVersion: number;
  /** Stable service process name. */
  serviceName: string;
  /** Cargo package version of the service build. */
  serviceVersion: string;
  /** Cargo profile that built the service executable. */
  profile: string;
  /** Canonical path of the service executable. */
  executablePath: string;
  /** Authoritative user data root. */
  dataRoot: string;
  /** Discovery file path below the authoritative data root. */
  discoveryPath: string;
  /** Exclusive lock file path below the authoritative data root. */
  lockPath: string;
  /** WebUI mode served by this profile. */
  defaultWebUiMode: string;
  /** Vite development URL in debug builds, or null in release builds. */
  viteUrl: string | null;
  /** Launch modes supported by the compiled artifact. */
  modes: string[];
}

/** One live service instance confirmed by its own health endpoint. */
export interface LiveServiceInstance {
  /** Browser-facing base URL. */
  baseUrl: string;
  /** Operating-system process id of the owning service process. */
  pid: number;
  /** Instance start time used for identity comparison. */
  startedAt: string;
  /** Instance access token required by protected endpoints. */
  serviceToken: string;
  /** Lifecycle text published by the discovery document. */
  status: string;
}

/** One startup progress report shown on the startup window. */
export interface ServiceStartupProgress {
  /** Stable stage identifier. */
  stage: string;
  /** Localized, user-facing description of the current stage. */
  message: string;
  /** Last Core snapshot for display, with only renderer-safe startup fields.
   * 供界面显示的 Core 最新快照，仅包含启动展示字段。 */
  snapshot?: ServiceStartupSnapshot;
}

/** Renderer projection of the existing Core startup RPC.
 * 既有 Core 启动 RPC 的渲染投影。 */
export interface ServiceStartupSnapshot {
  /** Core-owned readiness, phase, locale, and ordered step data.
   * Core 持有的就绪标记、阶段、语言及有序步骤数据。 */
  ready: boolean;
  phase: string;
  uiLocale?: string;
  steps: unknown[];
}

/** Reads startup progress from the verified instance while keeping its token in the main process.
 * 从已验证的实例读取启动进度，将令牌留在主进程内。
 * `instance` supplies the verified loopback URL and token; returns null if no snapshot is available.
 * `instance` 提供已验证的回环地址与令牌；快照不可用时返回 null。 */
async function readStartupSnapshot(instance: LiveServiceInstance): Promise<ServiceStartupSnapshot | null> {
  try {
    // Use the startup-only RPC without following a redirect or waiting on the business runtime.
    // 调用专用启动 RPC，不跟随重定向，也不等待业务运行时。
    const response = await fetch(`${instance.baseUrl}/rpc`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', 'x-vulcan-service-token': instance.serviceToken },
      body: JSON.stringify({ method: 'startup.snapshot' }),
      redirect: 'error',
      signal: AbortSignal.timeout(STARTUP_SNAPSHOT_TIMEOUT_MS),
    });
    if (!response.ok) return null;
    // Forward only fields consumed by the startup renderer.
    // 仅转发启动渲染器消费的字段。
    const envelope = await response.json() as { ok?: boolean; result?: Partial<ServiceStartupSnapshot> };
    const snapshot = envelope.result;
    if (envelope.ok !== true || !snapshot || typeof snapshot.ready !== 'boolean'
      || typeof snapshot.phase !== 'string' || !Array.isArray(snapshot.steps) || !snapshot.steps.length) return null;
    return {
      ready: snapshot.ready, phase: snapshot.phase, steps: snapshot.steps,
      ...(typeof snapshot.uiLocale === 'string' ? { uiLocale: snapshot.uiLocale } : {}),
    };
  } catch {
    // Missing progress does not change the verified health state or launch deadline.
    // 进度不可用不改变已验证的健康状态或启动截止时间。
    return null;
  }
}

/** Resolves the service executable for this build. */
// resolveServiceExecutable returns the manifest-declared service path, or the debug default when the app
// runs unpackaged from the workspace.
// resolveServiceExecutable 返回清单声明的服务路径；应用在工作区中以未打包方式运行时返回调试默认路径。
export async function resolveServiceExecutable(explicitPath: string | null, packaged: boolean): Promise<string> {
  if (explicitPath) {
    return explicitPath;
  }
  // Packaged launches resolve the distribution beside Electron's resources, including double-clicks.
  // 打包启动从 Electron 资源旁解析发行布局，包含双击启动。
  const resourcesPath = (process as NodeJS.Process & { resourcesPath?: string }).resourcesPath;
  const adjacentManifest = resourcesPath
    ? path.resolve(resourcesPath, process.platform === 'darwin' ? '../../../../..' : '../../..', 'runtime-layout.json')
    : null;
  let manifestPath = process.env[RUNTIME_LAYOUT_ENV];
  if (!manifestPath && adjacentManifest) {
    try {
      await access(adjacentManifest);
      manifestPath = adjacentManifest;
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error;
    }
  }
  if (manifestPath) {
    // Core owns the schema version and validates this same manifest strictly at startup; a second copy here went stale.
    // 架构版本由 Core 拥有，并在启动时严格校验同一份清单；此处的副本曾经过期。
    const manifest = JSON.parse(await readFile(manifestPath, 'utf8')) as {
      serviceExecutable: string; platform: string; architecture: string;
    };
    if (manifest.platform !== process.platform || manifest.architecture !== process.arch) {
      throw new Error('The desktop runtime layout does not match this platform or architecture.');
    }
    const root = path.dirname(path.resolve(manifestPath));
    const executable = path.resolve(root, manifest.serviceExecutable);
    if (path.isAbsolute(manifest.serviceExecutable) || !executable.startsWith(root + path.sep)) {
      throw new Error('The service executable escapes the runtime layout root.');
    }
    await access(executable);
    process.env[RUNTIME_LAYOUT_ENV] = path.resolve(manifestPath);
    return executable;
  }
  // A packaged app without its layout is a damaged installation, never a workspace build.
  // 缺少布局的打包应用是损坏的安装，绝不是工作区构建。
  if (packaged) {
    throw new Error(`The installation is incomplete: ${adjacentManifest ?? 'runtime-layout.json'} is missing. Reinstall Vulcan Code.`);
  }
  // The desktop application lives at apps/vulcan-code-desktop inside the workspace.
  // 桌面应用位于工作区内的 apps/vulcan-code-desktop。
  const workspaceRoot = path.resolve(import.meta.dirname, '..', '..', '..', '..');
  return path.join(workspaceRoot, 'target', 'debug', `vulcan-code-service${process.platform === 'win32' ? '.exe' : ''}`);
}

/** Reads and validates the service launch description. */
// readServiceLaunchDescription asks the service to describe itself without starting business logic.
// readServiceLaunchDescription 向服务索取自身描述，且不启动业务逻辑。
export async function readServiceLaunchDescription(
  executable: string,
  forwardedArguments: string[],
): Promise<ServiceLaunchDescription> {
  const { stdout } = await execFileAsync(
    executable,
    ['--describe', ...forwardedArguments],
    { timeout: DESCRIBE_TIMEOUT_MS, windowsHide: true, maxBuffer: 1024 * 1024 },
  );
  let description: ServiceLaunchDescription;
  try {
    description = JSON.parse(stdout) as ServiceLaunchDescription;
  } catch (error) {
    throw new Error(
      `The service printed an unreadable launch description: ${String(error)}`,
    );
  }
  if (description.protocolVersion !== DESKTOP_SERVICE_PROTOCOL_VERSION) {
    throw new Error(
      `Unsupported service launch protocol ${description.protocolVersion}; this client requires ${DESKTOP_SERVICE_PROTOCOL_VERSION}.`,
    );
  }
  if (!description.discoveryPath) {
    throw new Error('The service launch description did not name a discovery path.');
  }
  return description;
}

/** Probes one candidate discovery document and confirms the instance is alive. */
// readLiveServiceInstance returns the instance only when its own /health endpoint confirms it.
// readLiveServiceInstance 仅在实例自身的 /health 端点确认时返回该实例。
export async function readLiveServiceInstance(
  discoveryPath: string,
): Promise<LiveServiceInstance | null> {
  let document: Record<string, unknown>;
  try {
    document = JSON.parse(await readFile(discoveryPath, 'utf8')) as Record<string, unknown>;
  } catch {
    return null;
  }
  if (!document || typeof document !== 'object' || Array.isArray(document)
    || document.host !== '127.0.0.1'
    || typeof document.port !== 'number' || !Number.isInteger(document.port)
    || document.port < 1 || document.port > 65535
    || document.baseUrl !== `http://127.0.0.1:${document.port}`
    || typeof document.pid !== 'number' || !Number.isSafeInteger(document.pid)
    || document.pid < 1 || typeof document.startedAt !== 'string'
    || !document.startedAt || typeof document.serviceToken !== 'string'
    || !document.serviceToken) {
    return null;
  }
  // Core publishes one exact loopback origin. Reject rewritten paths, ports and remote origins
  // before any fetch or privileged workbench navigation can use the discovery document.
  // Core 只发布一个精确的回环来源；在网络请求或特权工作台导航使用发现文档前，拒绝改写的路径、端口和远程来源。
  const baseUrl = document.baseUrl;
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), HEALTH_TIMEOUT_MS);
  try {
    const response = await fetch(`${baseUrl}/health`, {
      cache: 'no-store',
      redirect: 'manual',
      signal: controller.signal,
    });
    if (!response.ok) {
      return null;
    }
    const health = (await response.json()) as Record<string, unknown>;
    // A discovery file is only a claim; the published identity must answer for itself.
    // 发现文件只是一份声明；已发布身份必须能自我应答。
    if (!health || typeof health !== 'object' || Array.isArray(health)
      || health.baseUrl !== baseUrl || health.host !== document.host
      || health.port !== document.port || health.pid !== document.pid
      || health.serviceToken !== document.serviceToken
      || health.startedAt !== document.startedAt
      || typeof health.status !== 'string' || !health.status) {
      return null;
    }
    return {
      baseUrl,
      pid: document.pid,
      startedAt: document.startedAt,
      serviceToken: document.serviceToken,
      status: health.status,
    };
  } catch {
    return null;
  } finally {
    clearTimeout(timer);
  }
}

/** Reports one startup progress stage to the waiting caller. */
export type ServiceStartupReporter = (progress: ServiceStartupProgress) => void;

/** Ensures exactly one service instance is running and returns it. */
// ensureServiceInstance reuses a live instance, otherwise starts one detached and waits for ready.
// ensureServiceInstance 复用活动实例，否则分离启动一个实例并等待就绪。
export async function ensureServiceInstance(
  description: ServiceLaunchDescription,
  forwardedArguments: string[],
  report: ServiceStartupReporter,
  text: DesktopText = desktopText(DEFAULT_DESKTOP_LOCALE),
): Promise<LiveServiceInstance> {
  report({ stage: 'checking-instance', message: text.checkingInstance });
  const existing = await readLiveServiceInstance(description.discoveryPath);
  if (existing?.status === 'ready') {
    report({ stage: 'reusing-instance', message: formatDesktopText(text.reusingInstance, { pid: existing.pid }) });
    return existing;
  }

  // A live but starting owner keeps the lock; wait for that owner instead of launching competitors.
  // 活着但启动中的所有者持有锁；等待该所有者，避免启动竞争进程。
  let exitDiagnostic = '';
  // launched records that this GUI already started its one detached service process.
  // launched 记录本 GUI 已经启动过唯一一个分离服务进程。
  let launched = false;
  // lostOwnership records that the started Core exited because another process holds the data directory.
  // lostOwnership 记录已启动的 Core 因另一进程占用数据目录而退出。
  let lostOwnership = false;
  // launchService starts one detached Core and records its pre-readiness failure.
  // launchService 启动一个分离 Core，并记录其就绪前失败。
  const launchService = (): void => {
    launched = true;
    report({ stage: 'starting-service', message: text.startingService });
    // The service is started detached so that closing this GUI never terminates Core.
    // 服务以分离方式启动，因此关闭本 GUI 绝不会终止 Core。
    // Debug Core owns one visible diagnostic console; Release Core stays windowless.
    // Debug Core 持有一个可见诊断控制台；Release Core 保持无窗口。
    const showDebugConsole = process.platform === 'win32' && description.profile === 'debug'
      && !forwardedArguments.includes('--no-debug-console');
    const child = spawn(
      description.executablePath,
      ['--gui-launch', ...forwardedArguments],
      { detached: true, stdio: 'ignore', windowsHide: !showDebugConsole },
    );
    child.unref();
    child.once('error', (error) => {
      exitDiagnostic = formatDesktopText(text.launchFailed, { error: error.message });
    });
    child.once('exit', (code, signal) => {
      // A non-zero exit before readiness is a real failure; a zero exit means another owner won.
      // 就绪前非零退出是真实失败；退出码为零表示另一个所有者胜出。
      if (code === null) {
        exitDiagnostic = formatDesktopText(text.signaledBeforeReady, { signal: String(signal) });
      } else if (code !== 0) {
        exitDiagnostic = formatDesktopText(text.exitedBeforeReady, { code: String(code) });
      } else {
        lostOwnership = true;
      }
    });
  };
  if (!existing) {
    launchService();
  }
  report({ stage: 'awaiting-ready', message: text.awaitingReady });
  // The deadline also bounds reuse of an owner that has not finished bootstrap.
  // 此截止时间同样限制对尚未完成初始化的既有所有者的等待。
  const deadline = Date.now() + READY_TIMEOUT_MS;
  // Keep the last published display fields so unchanged probes do not rerender.
  // 保留上次发布的展示字段，避免无变化的探测触发重渲染。
  let previousSnapshot = '';
  for (;;) {
    const instance = await readLiveServiceInstance(description.discoveryPath);
    if (instance?.status === 'ready') {
      report({ stage: 'ready', message: formatDesktopText(text.serviceReady, { pid: instance.pid }) });
      return instance;
    }
    if (instance && instance.status !== 'starting') {
      throw new Error(formatDesktopText(text.serviceUnavailable, { status: instance.status }));
    }
    if (instance?.status === 'starting') {
      // Only a Core identity verified by /health is allowed to supply progress.
      // 仅允许经过 /health 验证的 Core 身份提供进度。
      const snapshot = await readStartupSnapshot(instance);
      if (snapshot) {
        // Ignore step timestamps when checking whether the display has changed.
        // 判断展示是否变化时忽略步骤时间戳。
        const fingerprint = JSON.stringify(snapshot, (key, value: unknown) => key === 'updatedAt' ? undefined : value);
        if (fingerprint !== previousSnapshot) {
          previousSnapshot = fingerprint;
          report({ stage: snapshot.phase, message: text.awaitingReady, snapshot });
        }
      }
    }
    // A starting owner that disappears before readiness no longer holds the lock, so launch once ourselves.
    // 启动中的所有者在就绪前消失后不再持有锁，因此由本 GUI 自行启动一次。
    if (!instance && !launched) {
      launchService();
    }
    if (exitDiagnostic) {
      throw new Error(exitDiagnostic);
    }
    if (Date.now() >= deadline) {
      // An owner that won the lock yet never answered is a stuck process the user has to end.
      // 抢到锁却始终未应答的所有者是卡住的进程，需要用户结束它。
      if (lostOwnership && !instance) {
        throw new Error(formatDesktopText(text.ownerUnresponsive, {
          seconds: Math.round(READY_TIMEOUT_MS / 1000),
        }));
      }
      throw new Error(formatDesktopText(text.readyTimeout, {
        seconds: Math.round(READY_TIMEOUT_MS / 1000),
        path: description.discoveryPath,
      }));
    }
    await new Promise((resolve) => setTimeout(resolve, READY_POLL_INTERVAL_MS));
  }
}

/** Resolves which WebUI URL this GUI must load. */
// resolveWebUiUrl accepts a service-owned explicit page, then the described mode or service origin.
// resolveWebUiUrl 接受服务所有的显式页面，否则使用描述中的模式或服务同源地址。
export function resolveWebUiUrl(
  description: ServiceLaunchDescription,
  instance: LiveServiceInstance,
  explicitWebUiUrl: string | null,
): string {
  if (explicitWebUiUrl) {
    // The workbench preload grants native capabilities to its main frame. An explicit launch
    // override may select a known service-owned page, but must not authorize another origin.
    // 工作台预加载向主框架授予原生能力。显式启动覆盖只能选择已知服务页面，不能授权其他来源。
    if (explicitWebUiUrl !== instance.baseUrl && explicitWebUiUrl !== description.viteUrl) {
      throw new Error('The desktop WebUI URL must match the selected Core or its declared Vite URL.');
    }
    return explicitWebUiUrl;
  }
  if (description.defaultWebUiMode === 'vite') {
    if (!description.viteUrl) {
      throw new Error('The service selected the Vite WebUI mode but published no Vite URL.');
    }
    return description.viteUrl;
  }
  return instance.baseUrl;
}
