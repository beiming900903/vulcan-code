use super::*;

/// Describes one startup step state exposed by the Core Service.
/// 描述 Core Service 对外暴露的单个启动步骤状态。
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StartupStepSnapshot {
    /// Stable startup step identifier used by clients to merge updates.
    /// 客户端用于合并更新的稳定启动步骤标识。
    pub id: String,
    /// User-facing startup step label.
    /// 面向用户显示的启动步骤名称。
    pub label: String,
    /// Current startup step status.
    /// 当前启动步骤状态。
    pub status: StartupStepStatus,
    /// Optional detailed startup step message.
    /// 可选的启动步骤详细消息。
    pub message: Option<String>,
    /// Optional exact completion percentage for the active step.
    /// 活动步骤的可选精确完成百分比。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_percent: Option<u8>,
    /// Optional transferred-byte count for the active download.
    /// 活动下载的可选已传输字节数。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_bytes: Option<u64>,
    /// Optional total-byte count for the active download.
    /// 活动下载的可选总字节数。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_bytes: Option<u64>,
    /// Optional download or managed-runtime progress phase.
    /// 可选的下载或受管运行时进度阶段。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_phase: Option<String>,
    /// Stable skill identity for an active ROOT archive download.
    /// 当前 ROOT 归档下载的稳定技能标识。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_id: Option<String>,
    /// Optional timestamp for the most recent status update.
    /// 最近一次状态更新的可选时间戳。
    pub updated_at: Option<OffsetDateTime>,
}

/// Represents the lifecycle status of one startup step.
/// 表示单个启动步骤的生命周期状态。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum StartupStepStatus {
    /// The startup step has not started yet.
    /// 启动步骤尚未开始。
    Pending,
    /// The startup step is currently running.
    /// 启动步骤正在运行。
    Running,
    /// The startup step completed successfully.
    /// 启动步骤已成功完成。
    Ready,
    /// The startup step completed with a warning.
    /// 启动步骤已完成但存在警告。
    Warning,
    /// The startup step failed.
    /// 启动步骤执行失败。
    Error,
}

/// Provides a complete startup snapshot for the WebUI.
/// 为 WebUI 提供完整启动快照。
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StartupSnapshot {
    /// Overall service readiness flag.
    /// 服务整体是否就绪。
    pub ready: bool,
    /// WebUI locale resolved from the persisted configuration when available.
    /// 从持久化配置解析出的 WebUI 语言，可用时用于启动前界面。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_locale: Option<String>,
    /// Current service startup phase label.
    /// 当前服务启动阶段标签。
    pub phase: String,
    /// Ordered startup steps displayed by clients.
    /// 客户端展示的有序启动步骤。
    pub steps: Vec<StartupStepSnapshot>,
    /// Recent startup log lines.
    /// 最近启动日志行。
    pub recent_logs: Vec<String>,
    /// Signed model identities that replaced conflicting user definitions during this startup.
    /// 本次启动时接管冲突用户定义的签名模型身份。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_reconciliation_notices: Vec<StartupModelReconciliationNotice>,
}

/// Identifies one provider model whose manual definition was removed on startup.
/// 标识启动期间移除手工定义的一条供应商模型。
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StartupModelReconciliationNotice {
    /// Exact provider identifier owning the signed model.
    /// 持有签名模型的精确供应商标识。
    pub provider_id: String,
    /// Exact model identifier taken over by the signed catalog.
    /// 被签名目录接管的精确模型标识。
    pub model_id: String,
}

/// Summarizes one runtime module shown in the startup and status panels.
/// 汇总启动与状态面板中展示的单个运行时模块。
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeModuleSnapshot {
    /// Stable module identifier.
    /// 稳定模块标识。
    pub id: String,
    /// User-facing module label.
    /// 面向用户显示的模块名称。
    pub label: String,
    /// Current module status.
    /// 当前模块状态。
    pub status: StartupStepStatus,
    /// Short module status message.
    /// 简短模块状态消息。
    pub message: String,
}

/// Detailed Core service lifecycle reported by the protected status endpoint.
/// 受保护状态端点报告的详细 Core 服务生命周期。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum ServiceLifecycle {
    /// Listening socket is bound and bootstrap is still running.
    /// 监听已绑定，引导仍在运行。
    Starting,
    /// Full bootstrap completed and the service accepts business work.
    /// 完整引导完成，服务接受业务工作。
    Ready,
    /// Explicit shutdown entered; new writes and leases are refused.
    /// 已进入明确关闭；拒绝新写入与新租约。
    Stopping,
    /// Shutdown drain failed and admission was reopened.
    /// 关闭排空失败，准入已重新开放。
    Recovering,
    /// Bootstrap or a required HTTP host failed.
    /// 引导或必需 HTTP 宿主失败。
    Failed,
    /// Admission recovery after a failed drain also failed.
    /// 排空失败后的准入恢复同样失败。
    RecoveryFailed,
}

/// Exact failure record retained for the protected service status endpoint.
/// 受保护服务状态端点保留的精确失败记录。
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceStatusFailure {
    /// Stable bootstrap step that failed.
    /// 失败的稳定引导步骤。
    pub stage: String,
    /// Stable machine-readable error code.
    /// 稳定的机器可读错误码。
    pub code: String,
    /// User-facing diagnostic message.
    /// 面向用户的诊断信息。
    pub message: String,
}

/// Exact activity counts exposed through the protected service status endpoint.
/// 受保护服务状态端点暴露的精确活动计数。
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ServiceActiveWorkSummary {
    /// Number of active agent stream tasks.
    /// 活动智能体流任务数。
    pub active_stream_tasks: u64,
    /// Number of running project terminals.
    /// 运行中的项目终端数。
    pub running_terminals: u64,
}

/// Detailed Core service status served by the protected status endpoint.
/// 受保护状态端点提供的详细 Core 服务状态。
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceStatusSnapshot {
    /// Detailed service lifecycle state.
    /// 详细服务生命周期状态。
    pub lifecycle: ServiceLifecycle,
    /// Owning process id.
    /// 拥有该服务的进程标识。
    pub pid: u32,
    /// UTC startup timestamp in Unix milliseconds.
    /// UTC 启动时间戳，单位为 Unix 毫秒。
    pub started_at: String,
    /// Monotonic status revision published by the service.
    /// 服务发布的单调状态版本号。
    pub revision: u64,
    /// Latest retained failure, when one exists.
    /// 最近一次保留的失败（如有）。
    pub last_failure: Option<ServiceStatusFailure>,
    /// Current activity counts without configuration or credential content.
    /// 当前活动计数，不含配置或凭据内容。
    pub active_work: ServiceActiveWorkSummary,
}
