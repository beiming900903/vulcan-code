//! Owned provider transport request models shared by the host-controlled transport router and executor.
//! 供宿主控制传输路由层与执行器共享的自有 Provider 传输请求模型。

use crate::agent_runtime::provider_transport_request_budget::ProviderRequestBudgetGuard;
use crate::agent_runtime::provider_transport_retry::ProviderRetryPolicy;
use crate::agent_runtime::runtime_stream_intervention::RuntimePromptIntervention;
use crate::agent_runtime::task_runtime::TaskRuntime;
use crate::model::SessionId;
use std::sync::Arc;
use vulcan_core::types::{
    ConversationSnapshot as CoreConversationSnapshot,
    ProviderContextInjection as CoreProviderContextInjection,
    ProviderPromptLayers as CoreProviderPromptLayers,
};
use vulcan_session_core::v1::context::LockedBaselineProjection;
use vulcan_tools_manager::{ResolvedToolDefinition, VulcanToolsManager};

/// Provider transport turn-start mode selected by the launch layer.
/// 启动层选择的 Provider 传输回合启动模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderTransportTurnStartMode {
    /// The controller should append or reuse the user prompt before dispatch.
    /// 控制器应在分发前追加或复用用户提示词。
    UserPrompt,
    /// The controller should run from provider-visible input already in the snapshot.
    /// 控制器应从快照中已有的 Provider 可见输入启动。
    RestoredCheckpoint,
    /// Resume a host-admitted Continue action with its retained provider-visible history.
    /// 使用保留的 Provider 可见历史恢复宿主已准入的继续动作。
    ContinuedProviderTurn,
}

/// Owned provider transport execution request used to route one prepared runtime turn into the correct provider-specific adapter.
/// 用于把一份已准备好的运行时回合路由到正确 Provider 专用适配器的自有 Provider 传输执行请求。
/// Immutable identity of the resolved provider route that every retry attempt must preserve.
/// 每次重试尝试都必须保留的已解析 Provider 路由不可变身份。
///
/// The identity covers the full frozen route (plan §R4.5): provider, entry, connection,
/// credential profile, model, protocol, catalog revision, manifest revision, and route
/// revision. It is captured once before the retry loop and reused as the auditable retry
/// key while the already-selected adapter is replayed, so no retry re-evaluates runtime state.
/// 身份覆盖完整冻结路由（方案 §R4.5）：供应商、入口、连接、凭据档、模型、协议、
/// 目录代数、清单修订号与路由修订号。该身份在重试循环前仅捕获一次，并在重放已选
/// 适配器时作为可审计重试键复用，因此任何重试都不会重新求值运行时状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderRouteIdentity {
    /// Exact provider identifier.
    /// 精确供应商标识。
    pub(crate) provider_id: String,
    /// Exact service-entry identifier.
    /// 精确服务入口标识。
    pub(crate) service_entry_id: String,
    /// Exact credential connection identifier.
    /// 精确凭据连接标识。
    pub(crate) connection_id: String,
    /// Exact credential profile identifier.
    /// 精确凭据配置档标识。
    pub(crate) credential_profile_id: String,
    /// Exact model identifier.
    /// 精确模型标识。
    pub(crate) model_id: String,
    /// Exact protocol profile the route executes through.
    /// 路由执行所经过的精确协议 Profile。
    pub(crate) protocol_profile_id: String,
    /// Catalog generation the selection was frozen from.
    /// 选择冻结自的目录代数。
    pub(crate) catalog_generation: u64,
    /// Manifest revision the model-level binding resolved from; zero for legacy/custom routes.
    /// 模型级绑定解析自的清单修订号；旧版/自定义路由为零。
    pub(crate) manifest_revision: u64,
    /// Content-addressed route revision of the frozen snapshot.
    /// 冻结快照的内容寻址路由修订号。
    pub(crate) route_revision: String,
}

pub(crate) struct ProviderTransportExecutionRequest {
    /// Whether this real dispatch writes complete session-local request traces.
    /// 本次真实分派是否写入完整的会话本地请求追踪。
    pub(crate) provider_request_trace_enabled: bool,
    /// Frozen opt-in for retaining failed session requests independently.
    /// 独立保留失败会话请求的冻结开关。
    pub(crate) provider_error_trace_enabled: bool,
    /// Canonical provider API family used to group transport routing and shared protocol behavior.
    /// 用于归并传输路由与共享协议行为的规范 Provider 接口族。
    /// Runtime family used to select the concrete transport adapter implementation.
    /// 用于选择具体传输适配实现的运行时家族类型。
    /// Stable provider identifier attached to request snapshots and continuation records.
    /// 附着到请求快照与 continuation 记录上的稳定 Provider 标识。
    pub(crate) provider_id: String,
    /// Human-readable runtime label surfaced when a provider family is still unsupported.
    /// 当某个 Provider 家族尚未接通时向外暴露的人类可读运行时标签。
    /// Resolved API key passed down to the low-level provider adapter.
    /// 下传给底层 Provider 适配器的已解析 API Key。
    /// Optional base URL override used by providers that support custom endpoints.
    /// 供支持自定义端点的 Provider 使用的可选基础地址覆盖值。
    /// Stable model identifier used by the target provider transport.
    /// 目标 Provider 传输层使用的稳定模型标识。
    pub(crate) model_id: String,
    /// Canonical workspace path used for host-owned generated resource materialization.
    /// 用于宿主自有生成资源落盘的规范工作区路径。
    pub(crate) workspace_path: String,
    /// Execution mode label carried to invocation-time tool guards.
    /// 传递给调用期工具守卫的执行模式标签。
    pub(crate) execution_mode: String,
    /// Exact system preamble that should be visible to the outgoing provider request.
    /// 需要暴露给出站 Provider 请求的精确系统前导提示词。
    pub(crate) system_preamble: String,
    /// Canonical typed prompt baseline proposed by the prompt owner.
    /// 由提示词 owner 提出的规范强类型提示词基线。
    pub(crate) locked_baseline: LockedBaselineProjection,
    /// Request-dynamic prompt layers kept separate from the locked baseline.
    /// 与锁定基线分离保存的请求动态提示词层。
    pub(crate) dynamic_prompt_layers: CoreProviderPromptLayers,
    /// Fixed system and developer instructions carried to the physical request head.
    /// 传递到物理请求头部的固定系统和开发者指令。
    pub(crate) fixed_instruction_layers: CoreProviderPromptLayers,
    /// Exact outgoing prompt text for the current controller turn.
    /// 当前控制器回合的精确出站提示词文本。
    pub(crate) prompt: String,
    /// Explicit turn-start mode for the host-controlled controller loop.
    /// 宿主控制控制器循环的显式回合启动模式。
    pub(crate) turn_start_mode: ProviderTransportTurnStartMode,
    /// Canonical conversation snapshot consumed by the host-owned controller loop.
    /// 供宿主自有控制器循环消费的规范会话快照。
    pub(crate) core_snapshot: CoreConversationSnapshot,
    /// Optional temperature override already normalized by the executor.
    /// 已由执行器归一化后的可选 temperature 覆盖值。
    pub(crate) temperature: Option<f64>,
    /// Effective turn budget used by the host-owned controller loop.
    /// 宿主自有控制器循环使用的生效轮次预算。
    pub(crate) max_turns: usize,
    /// Shared tools manager used to construct the host-managed runtime bridge.
    /// 用于构建宿主管理运行时桥接层的共享工具管理器。
    pub(crate) tools_manager: VulcanToolsManager,
    /// Stable session identifier propagated into tool runtime events.
    /// 传播到工具运行时事件中的稳定会话标识。
    pub(crate) session_id: SessionId,
    /// Shared resumable task runtime reused by the provider execution path.
    /// 被 Provider 执行路径复用的共享可恢复任务运行时。
    pub(crate) task_runtime: Arc<TaskRuntime>,
    /// Resolved tool definitions exposed to the provider for the current turn.
    /// 当前回合向 Provider 暴露的已解析工具定义集合。
    pub(crate) resolved_tools: Vec<ResolvedToolDefinition>,
    /// Optional provider-facing context injection overlay for the current turn.
    /// 当前回合可选的 Provider 可见上下文注入覆盖层。
    pub(crate) context_injection: Option<CoreProviderContextInjection>,
    /// Optional model-scoped interleaved reasoning capability reused by OpenAI-compatible transports.
    /// 供 OpenAI 兼容传输链路复用的可选模型级交错推理能力描述。
    /// Optional request-budget guard evaluated before the exact request is dispatched.
    /// 在精确请求分发前执行的可选请求预算守卫。
    pub(crate) request_budget_guard: Option<ProviderRequestBudgetGuard>,
    /// Runtime retry policy applied around host-controlled provider attempts.
    /// 包裹宿主控制 Provider 尝试的运行时重试策略。
    pub(crate) retry_policy: ProviderRetryPolicy,
    /// Optional host intervention invoked at replay-safe tool boundaries.
    /// 在可回放安全工具边界调用的可选宿主干预钩子。
    pub(crate) prompt_intervention: Option<RuntimePromptIntervention>,
    /// Optional reason used to reject host-managed tool requests before execution.
    /// 在执行前拒绝宿主管理工具请求时使用的可选原因。
    pub(crate) tool_request_rejection_reason: Option<String>,
    /// Whether the rejection reason applies to every host-managed tool.
    /// 拒绝原因是否应用到全部宿主管理工具。
    pub(crate) tool_request_rejection_all_tools: bool,
    /// Optional scoped tool ids rejected with the rejection reason.
    /// 使用拒绝原因拒绝的可选限定工具标识。
    pub(crate) tool_request_rejection_tool_ids: Vec<String>,
    /// Immutable route identity guarded across every retry attempt (plan §13.5.1).
    /// 每次重试尝试都受守卫的不可变路由身份（方案 §13.5.1）。
    pub(crate) route_identity: ProviderRouteIdentity,
}

/// Builds one owned provider transport execution request from already resolved runtime inputs.
/// 根据已经解析完成的运行时输入构建一份自有的 Provider 传输执行请求。
///
/// # Parameters
/// 参数
/// `provider_request_trace_enabled` is the persisted system-setting decision frozen for this dispatch; the remaining values are the already resolved route, prompt, controller, tool, retry, and session inputs.
/// `provider_request_trace_enabled` 是为本次分派冻结的持久化系统设置决策；其余值是已经解析的路由、提示词、控制器、工具、重试与会话输入。
/// `provider_error_trace_enabled` independently retains failed attempts in the session error ring.
/// `provider_error_trace_enabled` 独立保留会话错误环中的失败尝试。
///
/// # Returns
/// 返回值
/// One fully owned request ready for protocol-specific stream preparation.
/// 返回一份可直接进入协议专用流准备阶段的完整自有请求。
pub(crate) fn build_provider_transport_execution_request(
    provider_request_trace_enabled: bool,
    provider_error_trace_enabled: bool,
    provider_id: String,
    model_id: &str,
    workspace_path: &str,
    execution_mode: &str,
    system_preamble: &str,
    dynamic_prompt_layers: CoreProviderPromptLayers,
    fixed_instruction_layers: CoreProviderPromptLayers,
    locked_baseline: LockedBaselineProjection,
    prompt: &str,
    turn_start_mode: ProviderTransportTurnStartMode,
    core_snapshot: CoreConversationSnapshot,
    temperature: Option<f64>,
    max_turns: usize,
    task_runtime: Arc<TaskRuntime>,
    tools_manager: VulcanToolsManager,
    session_id: SessionId,
    resolved_tools: Vec<ResolvedToolDefinition>,
    context_injection: Option<CoreProviderContextInjection>,
    request_budget_guard: Option<ProviderRequestBudgetGuard>,
    retry_policy: ProviderRetryPolicy,
    prompt_intervention: Option<RuntimePromptIntervention>,
    tool_request_rejection_reason: Option<String>,
    tool_request_rejection_all_tools: bool,
    tool_request_rejection_tool_ids: Vec<String>,
    route_identity: ProviderRouteIdentity,
) -> ProviderTransportExecutionRequest {
    ProviderTransportExecutionRequest {
        provider_request_trace_enabled,
        provider_error_trace_enabled,
        provider_id,
        model_id: model_id.to_string(),
        workspace_path: workspace_path.to_string(),
        execution_mode: execution_mode.to_string(),
        system_preamble: system_preamble.to_string(),
        dynamic_prompt_layers,
        fixed_instruction_layers,
        locked_baseline,
        prompt: prompt.to_string(),
        turn_start_mode,
        core_snapshot,
        temperature,
        max_turns,
        task_runtime,
        tools_manager,
        session_id,
        resolved_tools,
        context_injection,
        request_budget_guard,
        retry_policy,
        prompt_intervention,
        tool_request_rejection_reason,
        tool_request_rejection_all_tools,
        tool_request_rejection_tool_ids,
        route_identity,
    }
}
