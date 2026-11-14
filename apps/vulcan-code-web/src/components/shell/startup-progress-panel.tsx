import type { StartupSnapshot } from "@/types";
import { StartupLogoMark } from "./startup-logo-mark";
import { StartupDownloadProgress } from "./startup-download-progress";
import { StartupProgressBar } from "./startup-progress-bar";
import { activeStartupStep, startupProgressPercent } from "./startup-progress";

// StartupProgressLabels carries the localized startup copy, so the boot gate (minimal boot catalog) and the in-shell
// overlay (full i18n) share one panel.
// StartupProgressLabels 携带本地化启动文案，使启动门（最小启动文案表）与壳内遮罩（完整 i18n）共用同一面板。
export interface StartupProgressLabels {
  // Title shown before any startup step is active.
  // 尚无活动启动步骤时显示的标题。
  loadingWorkspace: string;
  // Detail shown before the first startup snapshot arrives.
  // 首份启动快照到达前显示的详情。
  connectingService: string;
  // Detail shown when the active step carries no message.
  // 活动步骤未携带消息时显示的详情。
  loadingProjectIndex: string;
  // Step title template containing `{step}`.
  // 包含 `{step}` 的步骤标题模板。
  loadingStep: string;
  // Completed-step template containing `{completed}` and `{total}`.
  // 包含 `{completed}` 与 `{total}` 的已完成步骤模板。
  completedSteps: string;
  // Managed-runtime download progress label.
  // 受管运行时下载进度标签。
  downloadProgress: string;
  // Text shown during a skill archive transfer, including its identity.
  // 技能归档传输期间显示的文案，包含技能标识。
  downloadingSkill?: string;
}

// StartupProgressPanel renders the startup lockup with the live progress block beneath it. The lockup is pinned to the
// panel's optical center and the progress block hangs below it, so rows appearing or disappearing never move the logo.
// StartupProgressPanel 渲染启动组合标志及其下方的实时进度区。标志固定在面板视觉中心，进度区悬挂在其下方，
// 因此行的出现或消失都不会移动标志。
// Parameters: props supplies the latest startup state and localized copy; returns prioritized step errors,
// measured or unknown progress and independent completed-step counts.
// 参数：props 提供最新启动状态与本地化文案；返回优先步骤错误、实测或未知进度及独立完成步骤计数。
export function StartupProgressPanel(props: {
  // Latest service startup snapshot, or null while the service endpoint is still being discovered.
  // 最近一次服务启动快照；服务端点仍在发现时为 null。
  startup: StartupSnapshot | null;
  // Localized startup copy.
  // 本地化启动文案。
  labels: StartupProgressLabels;
  // Host-reported detail that replaces the generic connecting text while no snapshot exists.
  // 尚无快照时替换通用连接文案的宿主上报详情。
  detail?: string | null;
}) {
  const { startup, labels } = props;
  // activeStep selects the current startup row for the progress headline.
  // activeStep 选择当前启动步骤作为进度标题。
  const activeStep = startup ? activeStartupStep(startup.steps) : null;
  // percent preserves the active step's real progress; missing upstream measurements remain unknown.
  // percent 保留活动步骤的真实进度；缺失的上游测量保持未知。
  const percent = startupProgressPercent(startup);
  // completedSteps counts finished or warning-complete steps without assigning guessed durations.
  // completedSteps 统计已完成或带警告完成的步骤，不分配猜测的耗时。
  const completedSteps = startup?.steps.filter((step) => step.status === "ready" || step.status === "warning").length ?? 0;
  // heading names the active step, or the workspace while no step is active.
  // heading 显示活动步骤名称；无活动步骤时显示工作区标题。
  const heading = activeStep ? labels.loadingStep.replace("{step}", activeStep.label) : labels.loadingWorkspace;
  // message shows the current startup phase without expanding the full step list.
  // message 展示当前启动阶段，而不展开完整步骤列表。
  const message = activeStep?.skillId && activeStep.progressPhase === "downloading" && labels.downloadingSkill
    ? labels.downloadingSkill.replace("{skill}", activeStep.skillId)
    : activeStep?.message ?? (startup ? labels.loadingProjectIndex : props.detail ?? labels.connectingService);
  // failed marks a step error so the message is announced and styled as the failure it is.
  // failed 标记步骤错误，使消息以失败身份播报并呈现。
  const failed = activeStep?.status === "error";
  return (
    <section className="startup-progress-panel" aria-live="polite" aria-busy={!startup?.ready && !failed}>
      <StartupLogoMark />
      <div className="startup-progress-content">
        <div className="startup-progress-status">
          <strong className="startup-progress-title" title={heading}>{heading}</strong>
          <p className={`startup-progress-message${failed ? " error" : ""}`} title={message} role={failed ? "alert" : undefined}>{message}</p>
        </div>
        <div className="startup-progress-meter">
          <StartupProgressBar percent={percent} label={activeStep?.label ?? labels.loadingWorkspace} />
          <div className="startup-progress-meta">
            <span>{startup && startup.steps.length > 0 ? labels.completedSteps
              .replace("{completed}", String(completedSteps)).replace("{total}", String(startup.steps.length)) : null}</span>
            <span>{percent !== null ? `${Math.round(percent)}%` : null}</span>
          </div>
        </div>
        <StartupDownloadProgress step={activeStep} label={labels.downloadProgress} />
      </div>
    </section>
  );
}
