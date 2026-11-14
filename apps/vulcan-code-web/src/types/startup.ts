// StartupStepStatus mirrors the Rust service API startup status enum.
// StartupStepStatus 对齐 Rust service API 的启动状态枚举。
export type StartupStepStatus = "pending" | "running" | "ready" | "warning" | "error";

// MainAreaId describes selectable top-level WebUI areas.
// MainAreaId 描述 WebUI 顶层可选择区域。
export type MainAreaId = "chat" | "browser" | "agent-skills" | "skills" | "agents" | "memory" | "settings";

// SettingsSubAreaId describes secondary settings pages exposed by the WebUI settings shell.
// SettingsSubAreaId 描述 WebUI 设置壳暴露的设置二级页面。
export type SettingsSubAreaId =
  | "openvulcan-account"
  | "model-providers"
  | "credential-management"
  | "model-list"
  | "context-basics"
  | "ai-capabilities"
  | "ai-vendors"
  | "ai-web-search"
  | "ai-web-extract"
  | "ai-understand-image"
  | "ai-understand-video"
  | "ai-understand-audio"
  | "ai-image-generation"
  | "ai-image-edit"
  | "ai-video-generation"
  | "ai-video-edit"
  | "ai-video-extend"
  | "ai-speech-synthesis"
  | "ai-speech-transcribe"
  | "ai-music-generation"
  | "ai-music-cover"
  | "runtime-diagnostics"
  | "visual-lab"
  | "notifications"
  | "appearance"
  | "git-commit"
  | "permission-judge"
  | "prompt-composer"
  | "app-update"
  | "changelog"
  | "feedback-messages"
  | "about-licenses"
  | "archived-restore"
  | "subagent-models"
  | "mcp-service";

// StartupStepSnapshot describes one startup step displayed by the startup panel.
// StartupStepSnapshot 描述启动面板展示的单个启动步骤。
export interface StartupStepSnapshot {
  // Stable startup step identifier.
  // 稳定启动步骤标识。
  id: string;
  // User-facing startup step label.
  // 面向用户显示的启动步骤名称。
  label: string;
  // Current startup step status.
  // 当前启动步骤状态。
  status: StartupStepStatus;
  // Optional startup detail message.
  // 可选启动详情消息。
  message?: string | null;
  // Exact completion percentage for the active step when available.
  // 可用时活动步骤的精确完成百分比。
  progressPercent?: number | null;
  // Transferred bytes for the active download when available.
  // 可用时活动下载的已传输字节数。
  completedBytes?: number | null;
  // Total bytes for the active download when available.
  // 可用时活动下载的总字节数。
  totalBytes?: number | null;
  // Native managed-runtime progress phase when applicable.
  // 适用时的原生受管运行时进度阶段。
  progressPhase?: string | null;
  // Stable skill identity on the active ROOT archive download.
  // 当前 ROOT 归档下载的稳定技能标识。
  skillId?: string | null;
}

// StartupSnapshot describes the full startup status returned by the service.
// StartupSnapshot 描述服务返回的完整启动状态。
export interface StartupSnapshot {
  // Overall readiness flag.
  // 整体就绪标记。
  ready: boolean;
  // WebUI locale resolved before the main app mounts.
  // 主应用挂载前解析出的 WebUI 语言。
  uiLocale?: string | null;
  // Current startup phase.
  // 当前启动阶段。
  phase: string;
  // Ordered startup steps.
  // 有序启动步骤。
  steps: StartupStepSnapshot[];
  // Recent startup log lines.
  // 最近启动日志行。
  recentLogs: string[];
  // Signed identities that replaced local manual definitions during this service startup.
  // 本次服务启动时接管本地手工定义的签名身份。
  modelReconciliationNotices?: { providerId: string; modelId: string }[];
}

// RuntimeModuleSnapshot describes one runtime module card.
// RuntimeModuleSnapshot 描述单个运行时模块卡片。
export interface RuntimeModuleSnapshot {
  // Stable module identifier.
  // 稳定模块标识。
  id: string;
  // User-facing module label.
  // 面向用户显示的模块名称。
  label: string;
  // Current module status.
  // 当前模块状态。
  status: StartupStepStatus;
  // Short module message.
  // 简短模块消息。
  message: string;
}
