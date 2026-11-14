//! Vulcan public-contract adapter state and host execution entry points.
//! Vulcan 公共契约适配器状态与宿主执行入口。

use crate::agent_runtime::provider_protocol_tool_definition::ProviderToolDefinition;
use crate::agent_runtime::provider_stream::ProviderDeltaStream;
use crate::agent_runtime::provider_transport_entry_inputs::{
    PreparedProviderTransportStreamLaunch, prepare_provider_transport_stream_launch,
};
use crate::agent_runtime::provider_transport_live_delta::ProviderLiveDeltaSender;
use crate::agent_runtime::provider_transport_request::ProviderTransportExecutionRequest;
use crate::agent_runtime::provider_transport_stream_runtime::execute_host_controlled_stream_with_prepared_inputs;
use vulcan_ai_contract::{
    ModelParameterValue, NativeToolKind, PromptCachePolicy, ReasoningEffort, ResourceKind,
    ToolDefinition,
};
use vulcan_core::types::{
    CoreError as VulcanCoreError, ProviderPromptLayers as CoreProviderPromptLayers,
};
use vulcan_provider_adapters::{
    ProviderExecutionClient, ProviderExecutionCredentialLease, ProviderExecutionRoute,
    ResolvedPublicAiExecution,
};
use vulcan_session_core::v1::context::LockedBaselineProjection;

mod accumulator;
mod generated_resources;
mod input_resources;
mod message_projection;
mod request;
pub(super) mod streaming;
mod trace;

pub use accumulator::public_usage_to_core;
pub(crate) use request::{VulcanPublicContractRequest, project_public_tool_definitions};
use trace::ProviderApiTraceAttemptState;

/// Vulcan-owned default output budget used when the provider reports no independent output limit.
/// 当供应商未报告独立输出上限时使用的 Vulcan 本地默认输出预算。
const DEFAULT_PUBLIC_OUTPUT_TOKEN_LIMIT: u64 = 32_000;

/// Structured subcategory emitted only after the exact OAuth account secret was refreshed.
/// 仅在精确 OAuth 账号秘密已经刷新后发出的结构化子类别。
pub(crate) const OAUTH_CREDENTIAL_REFRESHED_SUBCATEGORY: &str = "oauth_credential_refreshed";

/// Structured subcategory for a successful terminal that carries no answer and no tool call.
/// 成功终态既无回答也无工具调用时使用的结构化子类别。
pub(crate) const NO_ANSWER_COMPLETION_SUBCATEGORY: &str = "no_answer_completion";

/// Resolves the requested output budget while treating the effective model output limit as an upper bound.
/// 在将有效模型输出限制视为上限的前提下解析请求输出预算。
///
/// # Parameters
/// 参数
/// `model_limit` is the effective model maximum after direct model settings were applied.
/// `model_limit` 是应用直接模型设置后的有效模型最大值。
///
/// # Returns
/// 返回值
/// The default request budget capped by the model maximum when present.
/// 返回默认请求预算；存在模型最大值时以其封顶。
pub fn resolved_public_output_token_limit(model_limit: Option<u64>) -> u64 {
    model_limit
        .map(|maximum| DEFAULT_PUBLIC_OUTPUT_TOKEN_LIMIT.min(maximum))
        .unwrap_or(DEFAULT_PUBLIC_OUTPUT_TOKEN_LIMIT)
}
/// Executes one public-contract provider run through the existing host controller.
/// 通过现有宿主控制器执行一次公共契约 Provider 运行。
///
/// # Parameters
/// 参数
/// `request` owns host controller inputs and `resolved` freezes the account, protocol, model,
/// secret, cache, and model-parameter route.
/// `request` 持有宿主控制器输入，`resolved` 冻结账号、协议、模型、秘密、缓存与模型参数路由。
///
/// # Returns
/// 返回值
/// A stream of existing workbench deltas projected from public events.
/// 返回从公共事件投影出的现有工作台增量流。
pub(crate) fn execute_vulcan_public_stream(
    request: ProviderTransportExecutionRequest,
    resolved: ResolvedPublicAiExecution,
    reasoning_effort: Option<ReasoningEffort>,
) -> ProviderDeltaStream {
    // Share the same resolved execution with V1 preparation instead of reconstructing a second route or policy.
    // 与V1准备共享同一已解析执行，不重建第二条路线或策略。
    let resolved = std::sync::Arc::new(resolved);
    let provider_request_trace_enabled = request.provider_request_trace_enabled;
    // Freeze the independent error-recording decision before moving the launch request.
    // 在移动启动请求前冻结独立错误记录决策。
    let provider_error_trace_enabled = request.provider_error_trace_enabled;
    let cache_scope_id = request.session_id.to_string();
    let workspace_path = request.workspace_path.clone();
    let output_token_limit =
        resolved_public_output_token_limit(resolved.model.capabilities.output_limit);
    let prompt_cache = resolved.overrides.prompt_cache.clone().unwrap_or_default();
    let PreparedProviderTransportStreamLaunch {
        runtime_inputs,
        dynamic_prompt_layers,
        fixed_instruction_layers,
        locked_baseline,
        core_snapshot,
        prompt_intervention,
    } = prepare_provider_transport_stream_launch(request);
    execute_host_controlled_stream_with_prepared_inputs(
        runtime_inputs,
        core_snapshot,
        move |live_sender, adapter_inputs| {
            VulcanPublicContractAdapter::new(
                provider_request_trace_enabled,
                resolved.route.clone(),
                resolved.model.id.clone(),
                cache_scope_id.clone(),
                resolved.credential.clone(),
                resolved.client.clone(),
                adapter_inputs.provider_tool_definitions,
                resolved.model.capabilities.provider_native_tools.clone(),
                output_token_limit,
                prompt_cache.clone(),
                reasoning_effort,
                resolved.overrides.model_parameters.clone(),
                resolved.model.capabilities.output_resources.clone(),
                workspace_path.clone(),
                live_sender,
                dynamic_prompt_layers.clone(),
                fixed_instruction_layers.clone(),
                locked_baseline.clone(),
                std::sync::Arc::clone(&resolved),
            )
            .with_provider_error_trace_enabled(provider_error_trace_enabled)
        },
        "Vulcan public contract",
        prompt_intervention,
    )
}

/// Bridges Vulcan public requests and events into the host-owned controller.
/// 将 Vulcan 公共请求与事件桥接到宿主控制器。
#[derive(Clone)]
pub(crate) struct VulcanPublicContractAdapter {
    /// Shared authority for the exact media policy and route used by host-owned V1 preparation.
    /// 宿主V1准备用到的精确媒体策略与路线共享权威。
    media_execution: std::sync::Arc<ResolvedPublicAiExecution>,
    /// Frozen system-setting decision for grouped request and raw-response tracing.
    /// 当前控制器运行期间冻结的分组请求与原始返回追踪系统设置决策。
    provider_request_trace_enabled: bool,
    /// Frozen preference for keeping failed requests in the independent error ring.
    /// 将失败请求保存在独立错误环中的冻结偏好。
    provider_error_trace_enabled: bool,
    /// Shared physical-attempt index state keyed by the durable V1 API identity.
    /// 按可靠 V1 API 身份索引的共享物理尝试下标状态。
    provider_api_trace_attempt_state:
        std::sync::Arc<std::sync::Mutex<ProviderApiTraceAttemptState>>,
    /// Immutable provider execution route.
    /// 不可变供应商执行路由。
    route: ProviderExecutionRoute,
    /// Exact model identifier fixed for this controller execution.
    /// 为本次控制器执行固定的精确模型标识。
    model_id: String,
    /// Stable session cache scope shared by every physical request in this controller run.
    /// 本次控制器运行中每个物理请求共享的稳定会话缓存作用域。
    cache_scope_id: String,
    /// Exact account lease that resolves the latest secret before every physical request.
    /// 在每次真实请求前解析最新秘密的精确账号租约。
    credential: ProviderExecutionCredentialLease,
    /// Shared public-contract execution client.
    /// 共享公共契约执行客户端。
    client: ProviderExecutionClient,
    /// Model-visible host tool definitions.
    /// 模型可见的宿主工具定义。
    tools: Vec<ToolDefinition>,
    /// Source kind and label kept in the same order as the projected tool definitions.
    /// 与投影后工具定义保持同序的来源类型与标签。
    tool_sources: Vec<(String, String)>,
    /// Provider-native tools declared by the exact discovered model route.
    /// 精确发现模型路由声明的供应商原生工具。
    provider_native_tools: Vec<NativeToolKind>,
    /// Resolved output limit for every controller turn.
    /// 每个控制器回合使用的已解析输出限制。
    output_token_limit: u64,
    /// Resolved prompt-cache policy snapshot.
    /// 已解析提示词缓存策略快照。
    prompt_cache: PromptCachePolicy,
    /// Optional user-selected reasoning effort validated against the active catalog.
    /// 根据当前目录校验后的可选用户推理强度。
    reasoning_effort: Option<ReasoningEffort>,
    /// Validated model-specific values.
    /// 已校验模型独立参数值。
    model_parameters: std::collections::BTreeMap<String, ModelParameterValue>,
    /// Output resource kinds declared by the exact discovered model.
    /// 精确发现模型声明的输出资源类别。
    output_resources: Vec<ResourceKind>,
    /// Canonical workspace path that owns materialized provider-generated resources.
    /// 拥有供应商生成资源落盘文件的规范工作区路径。
    workspace_path: String,
    /// Live host sender for tool request construction.
    /// 工具请求构造使用的宿主实时发送器。
    live_sender: ProviderLiveDeltaSender,
    /// Request-dynamic prompt layers emitted by the unique prompt composer.
    /// 由唯一提示词组合器生成的请求动态提示词层。
    dynamic_prompt_layers: CoreProviderPromptLayers,
    /// Fixed system and developer instructions carried to the physical request head.
    /// 传递到物理请求头部的固定系统和开发者指令。
    fixed_instruction_layers: CoreProviderPromptLayers,
    /// Canonical typed prompt baseline proposed for V1 request preparation.
    /// 为 V1 请求准备提出的规范强类型提示词基线。
    locked_baseline: LockedBaselineProjection,
}

impl VulcanPublicContractAdapter {
    /// Freezes the independent error-capture preference and returns the configured adapter.
    /// 冻结独立错误捕获偏好，并返回配置后的适配器。
    /// The enabled flag applies to subsequent physical attempts in this run.
    /// enabled 开关应用于本次运行的后续真实尝试。
    fn with_provider_error_trace_enabled(mut self, enabled: bool) -> Self {
        self.provider_error_trace_enabled = enabled;
        self
    }

    /// Returns whether either explicit diagnostic preference requires bounded wire capture.
    /// 返回任一显式诊断偏好是否要求进行有界线上捕获。
    fn provider_trace_enabled(&self) -> bool {
        self.provider_request_trace_enabled || self.provider_error_trace_enabled
    }

    /// Creates one immutable public-contract controller adapter.
    /// 创建一个不可变公共契约控制器适配器。
    ///
    /// # Parameters
    /// 参数
    /// `provider_request_trace_enabled` freezes the persisted diagnostic switch; `route`,
    /// `credential`, and `client` identify the exact execution path; `tools` are host definitions;
    /// `output_token_limit`, `prompt_cache`, and `model_parameters` are resolved before dispatch;
    /// `live_sender` projects tool construction to the WebUI.
    /// `provider_request_trace_enabled` 冻结持久化诊断开关；`route`、`credential` 与 `client`
    /// 标识精确执行路径；`tools` 是宿主工具定义；`output_token_limit`、`prompt_cache` 与
    /// `model_parameters` 在分派前解析；`live_sender` 将工具构造投影到 WebUI。
    /// `media_execution` retains this exact resolved route, model, credential lease and media policy for V1 preparation.
    /// `media_execution`为V1准备保留本精确已解析路线、模型、凭据租约和媒体策略。
    ///
    /// # Returns
    /// 返回值
    /// A reusable adapter whose route remains fixed for the controller run.
    /// 返回在控制器运行期间路由保持固定的可复用适配器。
    pub(crate) fn new(
        provider_request_trace_enabled: bool,
        route: ProviderExecutionRoute,
        model_id: String,
        cache_scope_id: String,
        credential: ProviderExecutionCredentialLease,
        client: ProviderExecutionClient,
        tools: Vec<ProviderToolDefinition>,
        provider_native_tools: Vec<NativeToolKind>,
        output_token_limit: u64,
        prompt_cache: PromptCachePolicy,
        reasoning_effort: Option<ReasoningEffort>,
        model_parameters: std::collections::BTreeMap<String, ModelParameterValue>,
        output_resources: Vec<ResourceKind>,
        workspace_path: String,
        live_sender: ProviderLiveDeltaSender,
        dynamic_prompt_layers: CoreProviderPromptLayers,
        fixed_instruction_layers: CoreProviderPromptLayers,
        locked_baseline: LockedBaselineProjection,
        media_execution: std::sync::Arc<ResolvedPublicAiExecution>,
    ) -> Self {
        // toolSources preserves exact source ownership before protocol projection removes host metadata.
        // toolSources 在协议投影移除宿主元数据前保留精确来源归属。
        let tool_sources = tools
            .iter()
            .map(|tool| (tool.source_kind.clone(), tool.source_label.clone()))
            .collect();
        // publicTools applies the route-bound schema projection.
        // publicTools 应用路由绑定的 Schema 投影。
        let public_tools = project_public_tool_definitions(&route, tools);
        Self {
            media_execution,
            provider_request_trace_enabled,
            provider_error_trace_enabled: false,
            provider_api_trace_attempt_state: std::sync::Arc::new(std::sync::Mutex::new(
                ProviderApiTraceAttemptState::default(),
            )),
            route,
            model_id,
            cache_scope_id,
            credential,
            client,
            tools: public_tools,
            tool_sources,
            provider_native_tools,
            output_token_limit,
            prompt_cache,
            reasoning_effort,
            model_parameters,
            output_resources,
            workspace_path,
            live_sender,
            dynamic_prompt_layers,
            fixed_instruction_layers,
            locked_baseline,
        }
    }
}

/// Builds a Core provider failure that retains the normalized public error.
/// 构建一项保留归一化公共错误的 Core Provider 失败。
///
/// # Parameters
/// 参数
/// `provider_id` identifies the controller route and `error` is the structured failure.
/// `provider_id` 标识控制器路由，`error` 是结构化失败。
///
/// # Returns
/// 返回值
/// A Core error carrying both display text and typed details.
/// 返回同时承载展示文本与强类型细节的 Core 错误。
fn public_execution_failure(
    provider_id: &str,
    error: vulcan_ai_contract::AiExecutionError,
) -> VulcanCoreError {
    VulcanCoreError::ProviderFailure {
        provider_id: provider_id.to_string(),
        message: public_execution_failure_message(&error),
        details: Some(Box::new(error)),
    }
}

/// Serializes one public execution error as JSON for lossless user-facing diagnostics.
/// 将一个公共执行错误序列化为 JSON，以便面向用户的诊断信息无损传递。
pub(crate) fn public_execution_failure_message(
    error: &vulcan_ai_contract::AiExecutionError,
) -> String {
    // AiExecutionError contains only serde-supported scalar, option, enum, and nested struct fields.
    // AiExecutionError 仅包含 serde 支持的标量、可选值、枚举与嵌套结构字段。
    serde_json::to_string(error).expect("AiExecutionError serialization is infallible")
}

/// Creates one controller provider failure.
/// 创建一个控制器供应商失败。
fn provider_failure(provider_id: &str, message: &str) -> VulcanCoreError {
    VulcanCoreError::ProviderFailure {
        provider_id: provider_id.to_string(),
        message: message.to_string(),
        details: None,
    }
}

#[cfg(test)]
mod tests;
