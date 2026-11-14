//! Shared provider transport entry-input preparation helpers reused by provider-specific stream entrypoints.
//! 供各 Provider 专用流式入口复用的共享传输入口输入准备辅助模块。

use crate::agent_runtime::provider_transport_request::{
    ProviderRouteIdentity, ProviderTransportExecutionRequest, ProviderTransportTurnStartMode,
};
use crate::agent_runtime::provider_transport_request_budget::ProviderRequestBudgetGuard;
use crate::agent_runtime::provider_transport_retry::ProviderRetryPolicy;
use crate::agent_runtime::provider_transport_tool_context::{
    PreparedHostToolRuntimeContext, prepare_host_tool_runtime_context,
};
use crate::agent_runtime::runtime_stream_intervention::RuntimePromptIntervention;
use crate::agent_runtime::task_runtime::TaskRuntime;
use crate::model::SessionId;
use std::sync::Arc;
use vulcan_core::types::{
    ControllerConfig as CoreControllerConfig, ConversationSnapshot as CoreConversationSnapshot,
    ProviderContextInjection as CoreProviderContextInjection,
    ProviderPromptLayers as CoreProviderPromptLayers,
};
use vulcan_session_core::v1::context::LockedBaselineProjection;
use vulcan_tools_manager::{ResolvedToolDefinition, VulcanToolsManager};

/// Prepared host-controlled stream inputs shared by provider-specific entrypoints before they build the concrete adapter.
/// 在各 Provider 专用入口构建具体适配器之前复用的宿主控制流输入对象。
pub(crate) struct PreparedProviderTransportEntryInputs {
    /// Final outgoing prompt body forwarded into the host-controlled runtime stream.
    /// 被转发到宿主控制运行时流中的最终出站提示词正文。
    pub(crate) prompt: String,
    /// Explicit turn-start mode forwarded into the host-controlled runtime stream.
    /// 被转发到宿主控制运行时流中的显式回合启动模式。
    pub(crate) turn_start_mode: ProviderTransportTurnStartMode,
    /// Prebuilt controller configuration handed to `vulcan-core`.
    /// 交给 `vulcan-core` 的预构建控制器配置。
    pub(crate) controller_config: CoreControllerConfig,
}

/// Prepared host-controlled runtime inputs shared by provider-specific entrypoints after both controller config assembly and tool-runtime preparation complete.
/// 在控制器配置装配与工具运行时准备都完成后，供各 Provider 专用入口复用的宿主控制运行时输入对象。
pub(crate) struct PreparedProviderTransportRuntimeInputs {
    /// Shared provider transport entry inputs reused by concrete adapter constructors.
    /// 被具体适配器构造过程复用的共享 Provider 传输入口输入对象。
    pub(crate) entry_inputs: PreparedProviderTransportEntryInputs,
    /// Shared host-managed tool runtime context reused by concrete adapter constructors.
    /// 被具体适配器构造过程复用的共享宿主管理工具运行时上下文。
    pub(crate) tool_context: PreparedHostToolRuntimeContext,
    /// Optional request-budget guard evaluated after exact request assembly and before provider dispatch.
    /// 在精确请求装配后、Provider 分发前执行的可选请求预算守卫。
    pub(crate) request_budget_guard: Option<ProviderRequestBudgetGuard>,
    /// Runtime retry policy applied around provider attempts.
    /// 包裹 Provider 尝试的运行时重试策略。
    pub(crate) retry_policy: ProviderRetryPolicy,
    /// Optional reason used to reject host-managed tool requests before execution.
    /// 在执行前拒绝宿主管理工具请求时使用的可选原因。
    pub(crate) tool_request_rejection_reason: Option<String>,
    /// Whether the rejection reason applies to every host-managed tool.
    /// 拒绝原因是否应用到全部宿主管理工具。
    pub(crate) tool_request_rejection_all_tools: bool,
    /// Optional scoped tool ids rejected with the rejection reason.
    /// 使用拒绝原因拒绝的可选限定工具标识。
    pub(crate) tool_request_rejection_tool_ids: Vec<String>,
    /// Immutable route key captured once and reported across every retry attempt.
    /// 仅捕获一次并在每次重试尝试中上报的不可变路由键。
    pub(crate) route_identity: ProviderRouteIdentity,
}

/// Prepared one-shot launch data split from a routed provider transport execution request.
/// 从已路由 Provider 传输执行请求中拆分出来的一次性启动数据。
pub(crate) struct PreparedProviderTransportStreamLaunch {
    /// Shared runtime inputs consumed by the host-controlled streaming runtime.
    /// 由宿主控制流式运行时消费的共享运行时输入。
    pub(crate) runtime_inputs: PreparedProviderTransportRuntimeInputs,
    /// Canonical typed prompt baseline proposed by the prompt owner.
    /// 由提示词 owner 提出的规范强类型提示词基线。
    pub(crate) locked_baseline: LockedBaselineProjection,
    /// Request-dynamic prompt layers carried to the typed preparation candidate.
    /// 传递到强类型准备候选的请求动态提示词层。
    pub(crate) dynamic_prompt_layers: CoreProviderPromptLayers,
    /// Fixed system and developer instructions carried to the physical request head.
    /// 传递到物理请求头部的固定系统和开发者指令。
    pub(crate) fixed_instruction_layers: CoreProviderPromptLayers,
    /// Canonical conversation snapshot consumed by the host-owned controller loop.
    /// 由宿主自有控制器循环消费的规范会话快照。
    pub(crate) core_snapshot: CoreConversationSnapshot,
    /// Resolved API key passed down to the concrete provider adapter constructor.
    /// 下传给具体 Provider 适配器构造函数的已解析 API Key。
    /// Optional normalized base URL override used by providers that support custom endpoints.
    /// 供支持自定义端点的 Provider 使用的可选规范化基础地址覆盖值。
    /// Human-readable runtime label preserved for transport diagnostics.
    /// 为传输诊断保留的人类可读运行时标签。
    /// Optional model-scoped interleaved reasoning capability reused by compatible transports.
    /// 兼容传输链路复用的可选模型级交错推理能力描述。
    /// Optional host intervention invoked at replay-safe tool boundaries.
    /// 在可回放安全工具边界调用的可选宿主干预钩子。
    pub(crate) prompt_intervention: Option<RuntimePromptIntervention>,
}

/// Prepares the shared provider transport entry inputs from lightweight borrowed arguments.
/// 根据轻量借用参数准备共享 Provider 传输入口输入对象。
fn prepare_provider_transport_entry_inputs(
    provider_id: &str,
    model_id: &str,
    system_preamble: &str,
    prompt: &str,
    turn_start_mode: ProviderTransportTurnStartMode,
    temperature: Option<f64>,
    max_turns: usize,
    context_injection: Option<CoreProviderContextInjection>,
) -> PreparedProviderTransportEntryInputs {
    let provider_id = provider_id.to_string();
    let model_id = model_id.to_string();

    PreparedProviderTransportEntryInputs {
        prompt: prompt.to_string(),
        turn_start_mode,
        controller_config: CoreControllerConfig {
            provider_id,
            model_id,
            system_prompt: system_preamble.to_string(),
            prompt_layers: CoreProviderPromptLayers::from_system_prompt(system_preamble),
            temperature,
            max_turns,
            additional_params: None,
            context_injection,
        },
    }
}

/// Prepares the full shared runtime inputs for one host-controlled provider stream, including controller config plus host tool runtime state.
/// 为一次宿主控制 Provider 流准备完整共享运行时输入，包括控制器配置与宿主工具运行时状态。
fn prepare_provider_transport_runtime_inputs(
    provider_id: &str,
    model_id: &str,
    system_preamble: &str,
    prompt: &str,
    turn_start_mode: ProviderTransportTurnStartMode,
    temperature: Option<f64>,
    max_turns: usize,
    context_injection: Option<CoreProviderContextInjection>,
    tools_manager: VulcanToolsManager,
    resolved_tools: &[ResolvedToolDefinition],
    session_id: SessionId,
    execution_mode: &str,
    task_runtime: Arc<TaskRuntime>,
    request_budget_guard: Option<ProviderRequestBudgetGuard>,
    retry_policy: ProviderRetryPolicy,
    tool_request_rejection_reason: Option<String>,
    tool_request_rejection_all_tools: bool,
    tool_request_rejection_tool_ids: Vec<String>,
    route_identity: ProviderRouteIdentity,
) -> PreparedProviderTransportRuntimeInputs {
    PreparedProviderTransportRuntimeInputs {
        entry_inputs: prepare_provider_transport_entry_inputs(
            provider_id,
            model_id,
            system_preamble,
            prompt,
            turn_start_mode,
            temperature,
            max_turns,
            context_injection,
        ),
        tool_context: prepare_host_tool_runtime_context(
            tools_manager,
            resolved_tools,
            session_id,
            execution_mode,
            task_runtime,
        ),
        request_budget_guard,
        retry_policy,
        tool_request_rejection_reason,
        tool_request_rejection_all_tools,
        tool_request_rejection_tool_ids,
        route_identity,
    }
}

/// Splits one owned routed provider execution request into shared runtime inputs plus provider-specific launch fields; returns data ready for a concrete stream entrypoint.
/// 将一份自有的已路由 Provider 执行请求拆分为共享运行时输入与 Provider 专用启动字段；返回可供具体流式入口使用的数据。
pub(crate) fn prepare_provider_transport_stream_launch(
    request: ProviderTransportExecutionRequest,
) -> PreparedProviderTransportStreamLaunch {
    let ProviderTransportExecutionRequest {
        provider_request_trace_enabled: _,
        provider_error_trace_enabled: _,
        provider_id,
        model_id,
        workspace_path: _,
        execution_mode,
        system_preamble,
        dynamic_prompt_layers,
        fixed_instruction_layers,
        locked_baseline,
        prompt,
        turn_start_mode,
        core_snapshot,
        temperature,
        max_turns,
        tools_manager,
        session_id,
        task_runtime,
        resolved_tools,
        context_injection,
        request_budget_guard,
        retry_policy,
        prompt_intervention,
        tool_request_rejection_reason,
        tool_request_rejection_all_tools,
        tool_request_rejection_tool_ids,
        route_identity,
    } = request;
    let runtime_inputs = prepare_provider_transport_runtime_inputs(
        provider_id.as_str(),
        model_id.as_str(),
        system_preamble.as_str(),
        prompt.as_str(),
        turn_start_mode,
        temperature,
        max_turns,
        context_injection,
        tools_manager,
        &resolved_tools,
        session_id,
        execution_mode.as_str(),
        task_runtime,
        request_budget_guard,
        retry_policy,
        tool_request_rejection_reason,
        tool_request_rejection_all_tools,
        tool_request_rejection_tool_ids,
        route_identity,
    );
    PreparedProviderTransportStreamLaunch {
        runtime_inputs,
        dynamic_prompt_layers,
        fixed_instruction_layers,
        locked_baseline,
        core_snapshot,
        prompt_intervention,
    }
}

#[cfg(test)]
mod tests {
    //! Provider transport entry-input regression tests.
    //! Provider 传输入口输入回归测试。

    use super::*;

    #[test]
    fn entry_inputs_preserve_context_injection_overlay() {
        let injection = CoreProviderContextInjection {
            persistent_blocks: vec![String::from("<perm>\n固定\n</perm>")],
            transient_prefix: String::from("<temp>\n临时\n</temp>"),
            fingerprint: String::from("fingerprint"),
            ..CoreProviderContextInjection::default()
        };

        let inputs = prepare_provider_transport_entry_inputs(
            "openai",
            "gpt-test",
            "system",
            "prompt",
            ProviderTransportTurnStartMode::UserPrompt,
            Some(0.2),
            4,
            Some(injection.clone()),
        );

        let captured = inputs
            .controller_config
            .context_injection
            .expect("context injection should be preserved");
        assert_eq!(captured.persistent_blocks, injection.persistent_blocks);
        assert_eq!(captured.transient_prefix, injection.transient_prefix);
        assert_eq!(captured.fingerprint, injection.fingerprint);
    }
}
