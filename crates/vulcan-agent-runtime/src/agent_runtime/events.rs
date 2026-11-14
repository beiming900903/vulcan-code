//! Runtime event definitions used by future agent execution surfaces.
//! 供后续代理执行界面使用的运行时事件定义。

use crate::agent_runtime::execution_models::{
    ExecutionPreparation, ExecutionProviderRequestContinuation, ExecutionRunReport,
};
use crate::agent_runtime::provider_request_telemetry::ProviderRequestTelemetryEvent;
use crate::agent_runtime::provider_transport_request_budget::{
    ProviderRequestBudgetDecision, ProviderRequestBudgetGuard,
};
use crate::context_budget::ContextBudgetSnapshot;
use crate::conversation_protocol::SessionConversationState;
use crate::model::SessionId;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
use vulcan_ai_contract::AiExecutionRequest;
use vulcan_context::RuntimeProviderLoweringRepairEvent;
use vulcan_core::types::{
    ControllerConfig, ConversationSnapshot, ProviderPromptLayers,
    ProviderRequestSnapshot as CoreProviderRequestSnapshot,
    ProviderUsageSnapshot as CoreProviderUsageSnapshot,
};
use vulcan_session_core::v1::context::LockedBaselineProjection;
use vulcan_session_core::v1::document::PreparedApiStart;
use vulcan_session_core::v1::{ProviderProjectionProof, ProviderUsageBudgetAnchor};
use vulcan_workspace_store::workspace_store::ProviderApiTraceStore;

/// Shared single-use sender backing one Provider durability completion handle.
/// 支撑单个 Provider 可靠完成句柄的共享一次性发送端。
type SharedProviderRequestDurabilitySender =
    Arc<Mutex<Option<oneshot::Sender<Result<(), String>>>>>;

/// Shared single-use sender backing one typed Provider preparation permit.
/// 支撑单个强类型 Provider 准备许可的共享一次性发送端。
type SharedProviderRequestPreparationSender =
    Arc<Mutex<Option<oneshot::Sender<Result<PreparedPhysicalApi, String>>>>>;

/// Immutable media execution captured by the selected adapter before V1 preparation.
/// 在V1准备前由选中适配器捕获的不可变媒体执行。
#[derive(Clone)]
pub struct ProviderRequestMediaContext {
    /// The exact execution already selected by this adapter, including policy, capabilities and credential lease.
    /// 此适配器已经选择的精确执行，包含策略、能力及凭据租约。
    pub execution: Arc<vulcan_provider_adapters::ResolvedPublicAiExecution>,
    /// Original session scope retained for upload contracts without reading mutable global selection.
    /// 上传契约保留的原会话作用域，不读取可变全局选择。
    pub session_scope_id: String,
    /// Names of the tools this request exposes; a metadata-only media listing discloses its path only when none can take it.
    /// 本请求暴露的工具名称；仅元数据媒体清单仅在没有工具能处理时披露路径。
    pub tool_names: Vec<String>,
}

impl std::fmt::Debug for ProviderRequestMediaContext {
    /// Formats only non-secret identities into `formatter`; never prints credentials, route URLs or media.
    /// 仅向`formatter`格式化非秘密身份，绝不打印凭据、路线URL或媒体。
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderRequestMediaContext")
            .field("model_id", &self.execution.model.id)
            .field("session_scope_id", &self.session_scope_id)
            .finish_non_exhaustive()
    }
}

/// Immutable transport candidate produced before the V1 session owner rebuilds the request.
/// V1会话所有者重建请求前产生的不可变传输候选。
#[derive(Debug, Clone)]
pub struct ProviderRequestPreparationCandidate {
    /// Frozen media execution for real session requests; state-only and excluded fixtures may omit it.
    /// 真实会话请求的冻结媒体执行；仅状态和排除夹具可以省略。
    pub media_context: Option<ProviderRequestMediaContext>,
    /// Candidate request assembled from pre-gate controller state; it is never dispatched directly.
    /// 从闸门前控制器状态组装的候选请求；该对象绝不被直接分派。
    pub exact_request: AiExecutionRequest,
    /// Candidate controller configuration used only as validated non-ledger transport input.
    /// 仅作为经过校验的非账本传输输入使用的候选控制器配置。
    pub controller_config: ControllerConfig,
    /// Candidate controller snapshot retained only until the V1 owner creates the final permit.
    /// 仅保留到 V1 所有者创建最终许可为止的候选控制器快照。
    pub controller_snapshot: ConversationSnapshot,
    /// Candidate request snapshot containing route and budget facts.
    /// 包含路由与预算事实的候选请求快照。
    pub request_snapshot: CoreProviderRequestSnapshot,
    /// Candidate context-budget decision to be replaced or validated by the permit owner.
    /// 由许可所有者替换或校验的候选上下文预算判定。
    pub context_budget: Option<ContextBudgetSnapshot>,
    /// Exact request-budget guard frozen with the route before the V1 owner rebuilds history.
    /// 在 V1 owner 重建历史前随路由冻结的精确请求预算守卫。
    pub request_budget_guard: Option<ProviderRequestBudgetGuard>,
    /// Typed locked baseline provenance assembled by the unique prompt composer.
    /// 由唯一提示词组装器生成的强类型锁定基线来源。
    pub locked_baseline: LockedBaselineProjection,
    /// Request-dynamic prompt layers emitted by the same unique prompt composer.
    /// 由同一个唯一提示词组合器生成的请求动态提示词层。
    pub dynamic_prompt_layers: ProviderPromptLayers,
    /// Fixed system and developer instructions carried to the physical request head.
    /// 传递到物理请求头部的固定系统和开发者指令。
    pub fixed_instruction_layers: ProviderPromptLayers,
}

impl ProviderRequestPreparationCandidate {
    /// Recomputes and validates budget facts against the exact V1-rebuilt request.
    /// 针对精确 V1 重建请求重新计算并校验预算事实。
    ///
    /// # Parameters
    /// 参数
    /// `projection` is the exact V1 source frontier and `anchor` is the latest durable usage pair.
    /// `projection` 是精确 V1 来源前沿，`anchor` 是最近的可靠用量事实对。
    ///
    /// # Returns
    /// 返回值
    /// Stores the final budget and returns its admission decision and durable request evidence.
    /// 保存最终预算，返回其准入判定与持久化请求证据。
    pub fn finalize_v1_request_budget(
        &mut self,
        projection: &ProviderProjectionProof,
        anchor: Option<&ProviderUsageBudgetAnchor>,
    ) -> Result<ProviderRequestBudgetDecision, String> {
        let guard = self.request_budget_guard.as_ref().ok_or_else(|| {
            String::from("The V1 Provider permit is missing its frozen request-budget guard.")
        })?;
        let decision =
            guard.build_v1_budget_decision(&self.request_snapshot, projection, anchor)?;
        self.context_budget = Some(decision.context_budget.clone());
        Ok(decision)
    }

    /// Converts an explicitly excluded non-session candidate into its transport permit.
    /// 把一份明确排除的非会话候选转换为传输许可。
    ///
    /// # Returns
    /// 返回值
    /// Returns an excluded permit only after validating resource sources and projecting native tool media.
    /// 仅在校验资源来源并投影原生工具媒体后返回排除许可。
    pub fn into_excluded_non_session(mut self) -> Result<PreparedPhysicalApi, String> {
        let has_unprepared_resource = self
            .exact_request
            .resources
            .iter()
            .chain(self.exact_request.messages.iter().flat_map(|message| {
                message.content.iter().filter_map(|item| match item {
                    vulcan_ai_contract::ContentItem::Resource { resource } => Some(resource),
                    _ => None,
                })
            }))
            .any(non_session_resource_requires_prompt_owner);
        if has_unprepared_resource {
            return Err(String::from(
                "A non-session Provider request contains an unprepared host resource; route it through the session media owner or an explicit AI extension pipeline.",
            ));
        }
        // Standalone requests lack the V1 rebuild that normally separates tool receipts from media input.
        // 独立请求没有通常用于拆分工具回执与媒体输入的 V1 重建步骤。
        crate::native_media_input::project_native_media_messages(
            &mut self.exact_request.messages,
            self.media_context.as_ref(),
        )?;
        Ok(PreparedPhysicalApi::ExcludedNonSession {
            exact_request: self.exact_request,
            controller_config: self.controller_config,
            controller_snapshot: self.controller_snapshot,
            request_snapshot: self.request_snapshot,
            context_budget: self.context_budget,
        })
    }
}

/// Returns whether one resource still requires a session media owner or dedicated extension preparation.
/// 返回一项资源是否仍需会话媒体owner或专用扩展准备。
fn non_session_resource_requires_prompt_owner(
    resource: &vulcan_ai_contract::ResourceReference,
) -> bool {
    resource.source_kind == vulcan_ai_contract::ResourceSourceKind::HostResolved
        || resource.local_path.is_some()
}

/// Exact physical Provider request released by the unique preparation owner.
/// 由唯一准备所有者放行的精确物理 Provider 请求。
#[derive(Debug, Clone)]
pub enum PreparedPhysicalApi {
    /// Final V1 budget requires host compaction before any API identity or Continue action is consumed.
    /// 最终 V1 预算要求在消费任何 API 身份或 Continue 动作之前进行宿主压缩。
    ContextCompactionRequired {
        /// Final calibrated budget explanation forwarded to the existing compaction restart path.
        /// 传给既有压缩重启路径的最终校准预算说明。
        reason: String,
    },
    /// Newer session inputs or configuration require rebuilding this undispatched candidate on the same logical turn.
    /// 较新的会话输入或配置要求在同一逻辑回合内重建尚未派发的候选请求。
    RequestConfigurationChanged,
    /// A V1 session request rebuilt from the durable event ledger and committed before dispatch.
    /// 从可靠事件账本重建并在分派前完成提交的 V1 会话请求。
    SessionV1 {
        /// Exact request that is allowed to reach the transport.
        /// 允许进入传输层的精确请求。
        exact_request: AiExecutionRequest,
        /// Canonical controller configuration derived for subsequent tool-result turns.
        /// 为后续工具结果回合派生的规范控制器配置。
        controller_config: ControllerConfig,
        /// Canonical controller snapshot projected from the same V1 source.
        /// 从同一 V1 来源投影出的规范控制器快照。
        controller_snapshot: ConversationSnapshot,
        /// Exact request snapshot used by telemetry and budget accounting.
        /// 遥测与预算记账使用的精确请求快照。
        request_snapshot: CoreProviderRequestSnapshot,
        /// Context-budget decision computed from the exact permitted request.
        /// 根据精确许可请求计算的上下文预算判定。
        context_budget: Option<ContextBudgetSnapshot>,
        /// Durable Core receipt proving api_started was committed before dispatch.
        /// 证明 api_started 已在分派前可靠提交的 Core 回执。
        core_start: PreparedApiStart,
        /// Unique session-scoped diagnostic trace owner; absent only for non-production fixtures.
        /// 唯一的会话级诊断追踪 owner；仅非生产测试夹具允许为空。
        provider_api_trace_store: Option<ProviderApiTraceStore>,
    },
    /// A typed non-session request that is intentionally outside the V1 event ledger.
    /// 明确位于 V1 事件账本之外的强类型非会话请求。
    ExcludedNonSession {
        /// Exact request that is allowed to reach the transport.
        /// 允许进入传输层的精确请求。
        exact_request: AiExecutionRequest,
        /// Exact controller configuration retained for the physical run.
        /// 为物理运行保留的精确控制器配置。
        controller_config: ControllerConfig,
        /// Exact controller snapshot retained for the physical run.
        /// 为物理运行保留的精确控制器快照。
        controller_snapshot: ConversationSnapshot,
        /// Exact request snapshot used by telemetry and budget accounting.
        /// 遥测与预算记账使用的精确请求快照。
        request_snapshot: CoreProviderRequestSnapshot,
        /// Context-budget decision associated with the excluded request.
        /// 与排除请求关联的上下文预算判定。
        context_budget: Option<ContextBudgetSnapshot>,
    },
}

/// Cloneable completion half of one typed Provider preparation barrier.
/// 单次强类型 Provider 准备屏障的可克隆完成端。
#[derive(Debug, Clone)]
pub struct ProviderRequestPreparationCompletion {
    /// Single-use completion sender shared across cloned stream events.
    /// 在克隆流事件之间共享的一次性完成发送端。
    sender: SharedProviderRequestPreparationSender,
}

impl ProviderRequestPreparationCompletion {
    /// Completes the preparation barrier exactly once with the permit-owner result.
    /// 使用许可所有者的结果恰好一次完成准备屏障。
    ///
    /// # Parameters
    /// 参数
    /// `result` is successful only when the returned request is allowed to reach the wire.
    /// `result` 仅在返回请求获准进入线路时才成功。
    ///
    /// # Returns
    /// 返回值
    /// Returns `true` for the first completion and `false` for duplicates or poisoned state.
    /// 首次完成返回 `true`，重复完成或锁中毒时返回 `false`。
    pub fn complete(&self, result: Result<PreparedPhysicalApi, String>) -> bool {
        let Ok(mut guard) = self.sender.lock() else {
            return false;
        };
        let Some(sender) = guard.take() else {
            return false;
        };
        sender.send(result).is_ok()
    }
}

/// Builds the completion and wait halves of one typed Provider preparation barrier.
/// 构建单次强类型 Provider 准备屏障的完成端与等待端。
///
/// # Returns
/// 返回值
/// Returns one cloneable host completion handle and the single Provider-side permit receiver.
/// 返回一个可克隆的宿主完成句柄与单个 Provider 侧许可接收端。
pub(crate) fn provider_request_preparation_barrier() -> (
    ProviderRequestPreparationCompletion,
    oneshot::Receiver<Result<PreparedPhysicalApi, String>>,
) {
    let (sender, receiver) = oneshot::channel();
    (
        ProviderRequestPreparationCompletion {
            sender: Arc::new(Mutex::new(Some(sender))),
        },
        receiver,
    )
}

/// Classifies one non-persistent reasoning status line.
/// 对单条非持久化推理状态行进行分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransientReasoningStatusKind {
    /// Opaque encrypted reasoning represented only by progress metadata.
    /// 仅通过进度元数据表示的不透明加密推理。
    Encrypted,
}

/// Represents one replace-in-place reasoning status that never enters transcript state.
/// 表示一条原地替换且绝不进入转录状态的推理状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransientReasoningStatus {
    /// Presentation kind selected by the frozen response profile.
    /// 由冻结响应 Profile 选择的展示类型。
    pub kind: TransientReasoningStatusKind,
    /// Latest cumulative reasoning-token count when reported.
    /// 供应商报告时的最新累计推理 Token 数。
    pub reasoning_tokens: Option<u64>,
}

/// Cloneable completion half of one Provider-call durable persistence barrier.
/// 单次 Provider 调用可靠持久化屏障的可克隆完成端。
#[derive(Debug, Clone)]
pub struct ProviderRequestDurabilityCompletion {
    /// Single-use completion sender shared across cloned stream events.
    /// 在克隆流事件之间共享的一次性完成发送端。
    sender: SharedProviderRequestDurabilitySender,
}

impl ProviderRequestDurabilityCompletion {
    /// Completes the durable barrier exactly once with the host persistence result.
    /// 使用宿主持久化结果恰好一次完成可靠屏障。
    ///
    /// # Parameters
    /// 参数
    /// `result` is success only after the session state write is durable.
    /// `result` 仅在会话状态写入可靠完成后才为成功。
    ///
    /// # Returns
    /// 返回值
    /// Returns `true` for the first completion and `false` for duplicates or poisoned state.
    /// 首次完成返回 `true`，重复完成或锁中毒时返回 `false`。
    pub fn complete(&self, result: Result<(), String>) -> bool {
        let Ok(mut guard) = self.sender.lock() else {
            return false;
        };
        let Some(sender) = guard.take() else {
            return false;
        };
        sender.send(result).is_ok()
    }
}

/// Builds the completion and wait halves of one Provider-call durable persistence barrier.
/// 构建单次 Provider 调用可靠持久化屏障的完成端与等待端。
///
/// # Returns
/// 返回值
/// Returns a cloneable host completion handle and a single Provider-side receiver.
/// 返回可克隆的宿主完成句柄与单个 Provider 侧接收端。
pub(crate) fn provider_request_durability_barrier() -> (
    ProviderRequestDurabilityCompletion,
    oneshot::Receiver<Result<(), String>>,
) {
    let (sender, receiver) = oneshot::channel();
    (
        ProviderRequestDurabilityCompletion {
            sender: Arc::new(Mutex::new(Some(sender))),
        },
        receiver,
    )
}

/// Exposes the real durability barrier to cross-crate host tests; returns its completion and error-preserving waiter.
/// 向跨 crate 宿主测试公开真实可靠屏障；返回完成端与保留错误的等待器。
#[cfg(feature = "test-support")]
pub fn test_provider_request_durability_barrier() -> (
    ProviderRequestDurabilityCompletion,
    impl std::future::Future<Output = Result<(), String>>,
) {
    // Keep test observations on the same one-shot channel used by real Provider dispatch.
    // 让测试观察使用真实 Provider 分派的同一一次性通道。
    let (completion, receiver) = provider_request_durability_barrier();
    (completion, async move {
        receiver.await.map_err(|error| error.to_string())?
    })
}

/// High-level runtime event kinds emitted by the compose foundation.
/// compose 基础架构发出的高层运行时事件类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeEventKind {
    /// Token-level model streaming delta.
    /// 模型流式输出的 Token 增量事件。
    TokenDelta,
    /// Model-side tool request was identified and is still receiving arguments.
    /// 已识别模型侧工具请求，当前仍在接收参数。
    ToolRequestStarted,
    /// Model-side tool request argument progress changed.
    /// 模型侧工具请求参数进度发生变化。
    ToolRequestProgress,
    /// Sanitized parser preview for the active tool request changed.
    /// 活动工具请求的净化解析器预览发生变化。
    ToolRequestPreviewUpdated,
    /// Model-side tool request arguments completed and are ready for host dispatch.
    /// 模型侧工具请求参数已完成，可进入宿主调度。
    ToolRequestReady,
    /// Model-side tool request became definitively invalid before host execution.
    /// 模型侧工具请求在宿主执行前已确定无效。
    ToolRequestInvalid,
    /// Model-side tool request stopped before host execution began.
    /// 模型侧工具请求在宿主执行开始前停止。
    ToolRequestAborted,
    /// One complete tool batch is waiting for its durable recovery boundary.
    /// 一批完整工具请求正在等待可靠恢复边界。
    ToolBatchPrepared,
    /// Native or MCP tool invocation started.
    /// 原生工具或 MCP 工具调用开始事件。
    ToolStarted,
    /// Native or MCP tool invocation finished.
    /// 原生工具或 MCP 工具调用结束事件。
    ToolFinished,
    /// Delegate or sub-agent invocation started.
    /// 委派代理或子代理调用开始事件。
    DelegateStarted,
    /// Delegate or sub-agent invocation finished.
    /// 委派代理或子代理调用结束事件。
    DelegateFinished,
    /// Execution pipeline failed.
    /// 执行链路失败事件。
    Failed,
    /// Provider execution is waiting before an automatic retry.
    /// Provider 执行正在等待一次自动重试。
    Retrying,
    /// One retained Provider retry notice changed to a terminal display state.
    /// 一条已保留的 Provider 重试提示变更为终态展示状态。
    ProviderRetryStatusChanged,
    /// Provider request was dispatched but has not produced its first stream event yet.
    /// Provider 请求已经发出，但尚未产生首个流事件。
    ProviderWaiting,
}

/// Binds one exact tool request to the resolved definition fingerprint used by its original batch.
/// 将一条精确工具请求绑定到其原始批次使用的已解析定义指纹。
#[derive(Debug, Clone)]
pub struct ToolExecutionRecoveryRequest {
    /// Exact Provider stream-item identity used by live draft projection.
    /// 实时草稿投影使用的精确 Provider 流 item 身份。
    pub request_stream_id: String,
    /// Exact original tool invocation request.
    /// 精确原始工具调用请求。
    pub request: vulcan_core::types::ToolInvocationRequest,
    /// Stable resolved descriptor, schema, and execution-policy fingerprint.
    /// 稳定的已解析描述、Schema 与执行策略指纹。
    pub binding_fingerprint: String,
}

/// Carries an internal exact tool boundary without exposing model output through UI metadata.
/// 携带内部精确工具边界，同时避免通过 UI 元数据暴露模型输出。
#[derive(Debug, Clone)]
pub enum ToolExecutionRecoveryBoundary {
    /// The complete original Provider batch is durable before any tool body starts.
    /// 在任何工具主体启动前可靠保留的完整原始 Provider 批次。
    BatchPrepared(Vec<ToolExecutionRecoveryRequest>),
    /// The exact request is durable and the tool body has not started yet.
    /// 精确请求已经可靠保留，工具主体尚未启动。
    Started {
        /// Exact original invocation request.
        /// 精确原始调用请求。
        request: vulcan_core::types::ToolInvocationRequest,
        /// Stable fingerprint of the resolved tool descriptor, schema, and execution policy.
        /// 已解析工具描述、Schema 与执行策略的稳定指纹。
        binding_fingerprint: String,
    },
    /// The exact terminal result must become durable before Core may advance.
    /// Core 推进前必须可靠保留的精确终态结果。
    Finished(vulcan_core::types::ToolInvocationResult),
}

/// Structured runtime event record reserved for future telemetry surfaces.
/// 为后续遥测界面预留的结构化运行时事件记录。
#[derive(Debug, Clone)]
pub struct RuntimeEvent {
    /// Event kind.
    /// 事件类型。
    pub kind: RuntimeEventKind,
    /// Short event headline.
    /// 事件简短标题。
    pub title: String,
    /// Additional event detail text.
    /// 事件补充说明文本。
    pub detail: String,
    /// Optional structured metadata reserved for transcript projection and host-side UI enrichments.
    /// 为转录投影与宿主侧界面增强保留的可选结构化元数据。
    pub metadata: Option<Value>,
    /// Optional internal tool-recovery boundary consumed only by the host service.
    /// 仅由宿主服务消费的可选内部工具恢复边界。
    pub tool_execution_boundary: Option<ToolExecutionRecoveryBoundary>,
}

impl RuntimeEvent {
    /// Creates a new runtime event from strongly typed parts.
    /// 使用强类型片段创建新的运行时事件。
    pub fn new(
        kind: RuntimeEventKind,
        title: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            title: title.into(),
            detail: detail.into(),
            metadata: None,
            tool_execution_boundary: None,
        }
    }

    /// Attaches optional structured metadata without changing the visible runtime headline or detail.
    /// 在不改变可见运行时标题与正文的前提下附着可选结构化元数据。
    pub fn with_metadata(mut self, metadata: Option<Value>) -> Self {
        self.metadata = metadata;
        self
    }

    /// Attaches one exact internal tool boundary without changing user-visible projection metadata.
    /// 附着一条精确内部工具边界，而不改变用户可见的投影元数据。
    pub fn with_tool_execution_boundary(mut self, boundary: ToolExecutionRecoveryBoundary) -> Self {
        self.tool_execution_boundary = Some(boundary);
        self
    }
}

/// Incremental execution events streamed from the real provider runtime into the workbench.
/// 从真实 Provider 运行时流式发送到工作台的增量执行事件。
#[derive(Debug, Clone)]
pub enum ExecutionStreamEvent {
    /// Execution preparation completed and validated.
    /// 执行准备已完成且通过校验。
    Prepared(ExecutionPreparation),
    /// Provider-neutral request snapshot has been built before transport-specific serialization.
    /// Provider 中立请求快照已在传输层专用序列化前构建完成。
    RequestReady {
        /// Session identifier owning the provider request.
        /// 拥有该 Provider 请求的会话标识。
        session_id: SessionId,
    },
    /// One real provider request snapshot has just been sent and the workbench should show a lightweight waiting hint until visible output arrives.
    /// 一次真实 Provider 请求快照刚刚发出，工作台应显示轻量等待提示，直到出现可见输出为止。
    RequestStarted {
        /// Session identifier owning the provider request.
        /// 拥有该 Provider 请求的会话标识。
        session_id: SessionId,
    },
    /// Low-frequency structured telemetry for one physical Provider request.
    /// 单次真实 Provider 请求的低频结构化遥测。
    ProviderRequestTelemetry(ProviderRequestTelemetryEvent),
    /// Provider telemetry boundary that must be durably persisted before execution may continue.
    /// Provider 遥测边界；执行继续前必须可靠持久化。
    ProviderRequestDurabilityBoundary {
        /// Exact started or terminal physical-call telemetry event.
        /// 精确的物理调用开始或终态遥测事件。
        event: ProviderRequestTelemetryEvent,
        /// Completion handle released by the service only after durable persistence.
        /// 仅在可靠持久化后由服务释放的完成句柄。
        completion: ProviderRequestDurabilityCompletion,
    },
    /// Candidate request boundary that only the unique session owner may turn into a dispatch permit.
    /// 仅允许唯一会话所有者把候选请求转换为分派许可的边界。
    ProviderRequestPreparationBoundary {
        /// Started telemetry carrying the immutable physical request identity.
        /// 携带不可变物理请求身份的 Started 遥测。
        event: ProviderRequestTelemetryEvent,
        /// Pre-gate candidate that must never be dispatched directly for a V1 session.
        /// 对 V1 会话绝不能直接分派的闸门前候选。
        candidate: ProviderRequestPreparationCandidate,
        /// Typed completion released with the exact permitted physical request.
        /// 使用精确获准物理请求释放的强类型完成句柄。
        completion: ProviderRequestPreparationCompletion,
    },
    /// Visible assistant thinking delta that should stay outside future context.
    /// 应保持在未来上下文之外的可见思考增量。
    ThinkingDelta {
        /// Session identifier receiving the thinking delta.
        /// 接收该思考增量的会话标识。
        session_id: SessionId,
        /// Incremental thinking text emitted by the provider.
        /// Provider 发出的增量思考文本。
        delta: String,
        /// Whether the text is a provider summary that is recorded but never replayed.
        /// 正文是否为记录但绝不重放的供应商摘要。
        display_only: bool,
    },
    /// Replaces or clears the non-persistent reasoning status line.
    /// 替换或清除非持久化推理状态行。
    TransientReasoningStatusChanged {
        /// Session identifier owning the transient status.
        /// 拥有瞬态状态的会话标识。
        session_id: SessionId,
        /// Latest status, or `None` to remove the line immediately.
        /// 最新状态；为 `None` 时立即移除该状态行。
        status: Option<TransientReasoningStatus>,
    },
    /// Visible assistant thinking phase completed and should dock back into the main transcript immediately.
    /// 可见助手思考阶段已完成，并应立即收回主转录正文中。
    ThinkingCompleted {
        /// Session identifier whose thinking phase completed.
        /// 思考阶段已经完成的会话标识。
        session_id: SessionId,
    },
    /// Visible assistant final-output delta that should stay inside future context.
    /// 应保留在未来上下文之内的可见正式输出增量。
    OutputDelta {
        /// Session identifier receiving the output delta.
        /// 接收该输出增量的会话标识。
        session_id: SessionId,
        /// Incremental assistant output text emitted by the provider.
        /// Provider 发出的助手正式输出增量文本。
        delta: String,
    },
    /// Runtime event emitted while the run is still in progress.
    /// 在运行仍进行中时发出的运行时事件。
    RuntimeEvent {
        /// Session identifier owning the runtime event.
        /// 拥有该运行时事件的会话标识。
        session_id: SessionId,
        /// Structured runtime event payload to project into host UI.
        /// 需要投影到宿主界面的结构化运行时事件载荷。
        event: RuntimeEvent,
    },
    /// Provider lowering repair audit event emitted by a protocol adapter.
    /// 协议适配器发出的 Provider 转换修复审计事件。
    ProviderLoweringRepair {
        /// Session identifier owning the lowering repair event.
        /// 拥有该转换修复事件的会话标识。
        session_id: SessionId,
        /// Structured lowering repair payload to persist into runtime audit.
        /// 需要持久化到运行时审计中的结构化转换修复载荷。
        event: RuntimeProviderLoweringRepairEvent,
    },
    /// Safe canonical context checkpoint emitted after one provider/tool cycle becomes replayable.
    /// 在一次 Provider/工具循环已经可回放后发出的安全规范上下文检查点。
    ConversationCheckpoint {
        /// Session identifier owning the checkpoint.
        /// 拥有该检查点的会话标识。
        session_id: SessionId,
        /// Canonical conversation state that can be used by the next request.
        /// 可供下一次请求使用的规范会话状态。
        conversation_state: SessionConversationState,
        /// Latest successful provider usage captured at this checkpoint.
        /// 该检查点捕获到的最近一次成功 Provider 用量。
        last_successful_provider_usage: Option<CoreProviderUsageSnapshot>,
        /// Number of provider requests observed by this run when the checkpoint was emitted.
        /// 发出该检查点时本轮已观测到的 Provider 请求次数。
        provider_request_count: usize,
    },
    /// Requests the host to compact the committed conversation and restart the active turn.
    /// 请求宿主压缩已提交会话并重启当前回合。
    ContextCompactionRequired {
        /// Session identifier owning the active turn.
        /// 拥有当前活动回合的会话标识。
        session_id: SessionId,
        /// Replay-safe conversation state produced at the tool boundary.
        /// 工具边界处生成的可安全回放会话状态。
        conversation_state: SessionConversationState,
        /// Verified context-budget diagnostic produced before dispatch.
        /// 分派前生成的已验证上下文预算诊断。
        reason: String,
        /// Next request identity and unconsumed Continue action retained across compaction.
        /// 跨压缩保留的下一请求身份与尚未消费的 Continue 动作。
        provider_request_continuation: ExecutionProviderRequestContinuation,
    },
    /// Requests the host to activate a pending exact model route and restart the active turn.
    /// 请求宿主激活待生效精确模型路由并重启当前活动回合。
    ProviderRouteSwitchRequired {
        /// Session identifier owning the active turn.
        /// 拥有当前活动回合的会话标识。
        session_id: SessionId,
        /// Replay-safe conversation state produced after the complete tool batch.
        /// 完整工具批次之后生成的可安全回放会话状态。
        conversation_state: SessionConversationState,
        /// Internal diagnostic produced at the pre-Provider intervention boundary.
        /// 在 Provider 前干预边界生成的内部诊断。
        reason: String,
        /// Exact logical identity and physical attempt reserved for the replacement Provider stream.
        /// 为替代 Provider 流保留的精确逻辑身份与物理尝试索引。
        provider_request_continuation: ExecutionProviderRequestContinuation,
    },
    /// Requests the host to refresh the session compaction policy and restart the active turn.
    /// 请求宿主刷新会话压缩策略并重启当前活动回合。
    ContextCompactionPolicyRefreshRequired {
        /// Session identifier owning the active turn.
        /// 拥有当前活动回合的会话标识。
        session_id: SessionId,
        /// Replay-safe conversation state produced after the complete tool batch.
        /// 完整工具批次之后生成的可安全回放会话状态。
        conversation_state: SessionConversationState,
        /// Internal diagnostic produced at the pre-Provider intervention boundary.
        /// 在 Provider 前干预边界生成的内部诊断。
        reason: String,
        /// Telemetry continuation reserved for the replacement physical Provider request.
        /// 为替代真实 Provider 请求保留的遥测延续身份。
        provider_request_continuation: ExecutionProviderRequestContinuation,
    },
    /// Terminal execution report emitted exactly once at the end of the run.
    /// 在运行结束时恰好发出一次的最终执行报告。
    Finished(ExecutionRunReport),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Non-session permits accept only already prepared standard sources and reject every host-owned source.
    /// 非会话许可仅接纳已经准备的标准来源，并拒绝全部宿主持有来源。
    #[test]
    fn non_session_resources_require_explicit_preparation() {
        // resource builds one minimal reference whose source semantics are the only tested dimension.
        // resource 构建一个最小引用，来源语义是唯一被测维度。
        let resource = |source_kind, local_path| vulcan_ai_contract::ResourceReference {
            id: String::from("resource"),
            kind: vulcan_ai_contract::ResourceKind::Image,
            media_type: Some(String::from("image/png")),
            source_kind,
            file_name: None,
            image_detail: None,
            resource_path: None,
            local_path,
            url: Some(String::from("https://safe.example/image.png")),
        };
        assert!(non_session_resource_requires_prompt_owner(&resource(
            vulcan_ai_contract::ResourceSourceKind::HostResolved,
            None,
        )));
        assert!(non_session_resource_requires_prompt_owner(&resource(
            vulcan_ai_contract::ResourceSourceKind::RemoteUrl,
            Some(String::from("C:/private/image.png")),
        )));
        assert!(!non_session_resource_requires_prompt_owner(&resource(
            vulcan_ai_contract::ResourceSourceKind::RemoteUrl,
            None,
        )));
        assert!(!non_session_resource_requires_prompt_owner(&resource(
            vulcan_ai_contract::ResourceSourceKind::InlineData,
            None,
        )));
    }
}
