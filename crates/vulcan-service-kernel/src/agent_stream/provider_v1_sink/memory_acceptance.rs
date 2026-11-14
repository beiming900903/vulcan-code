//! Test-only access to the production V1 physical request builder.
//! 仅供测试访问生产 V1 物理请求构建器。

use super::*;
use vulcan_agent_runtime::agent_runtime::provider_reasoning::ProviderRuntimeReasoningResolution;
use vulcan_agent_runtime::agent_runtime::provider_transport_request_budget::ProviderRequestBudgetGuard;
use vulcan_ai_contract::{AiExecutionRequest, AiOperation, ModelCapabilities};
use vulcan_core::types::{ControllerConfig, ProviderContextInjection};
use vulcan_session_core::v1::event::{ApiPurposeKind, ApiRequestSnapshot};

/// Returns the empty locked baseline completed with the fixed host guidance a real first request locks.
/// 返回补齐固定宿主指引的空锁定基线，与真实首个请求锁定的基线一致。
/// Fixtures that initialize a session baseline must use it, because rebuilds reject a baseline without guidance.
/// 初始化会话基线的夹具必须使用它，因为重建会拒绝缺少指引的基线。
pub(crate) fn host_guided_locked_baseline() -> LockedBaselineProjection {
    initial_locked_baseline_with_host_guidance(&LockedBaselineProjection::empty())
        .expect("the fixed host guidance must project onto an empty baseline")
}

/// Builds a text-only fixture candidate and passes its real V1 snapshot through production reconstruction.
/// 构造纯文本夹具候选，并将其真实 V1 快照送入生产重建。
/// Takes the authoritative snapshot and admitted overlay; returns the actual request, budget and canonical projections.
/// 接收权威快照与准入覆盖层，返回实际请求、预算及规范投影。
pub(crate) fn rebuild_memory_acceptance_request(
    snapshot: SessionProviderProjectionSnapshot,
    injection: ProviderContextInjection,
) -> Result<ProviderRequestPreparationCandidate, String> {
    let (facts, candidate) = memory_acceptance_candidate(injection);
    rebuild_memory_acceptance_candidate(snapshot, &facts, candidate)
}

/// Publishes `injection` as durable dynamic context on `handle`, then rebuilds from the committed snapshot.
/// 先把 `injection` 作为可靠动态上下文发布到 `handle`，再从已提交快照重建。
/// Persistent memory reaches the wire only through durable dynamic context, exactly as on the production path.
/// 持久记忆只经由可靠动态上下文进入线上请求，与生产路径一致。
pub(crate) fn rebuild_memory_acceptance_request_for_handle(
    handle: &SessionHandle,
    injection: ProviderContextInjection,
) -> Result<ProviderRequestPreparationCandidate, String> {
    let (facts, candidate) = memory_acceptance_candidate(injection);
    context_injection::synchronize_request_dynamic_context(handle, &facts, &candidate)
        .map_err(|error| error.to_string())?;
    let snapshot = handle
        .capture_controller_projection_snapshot()
        .map_err(|error| error.to_string())?;
    rebuild_memory_acceptance_candidate(snapshot, &facts, candidate)
}

/// Builds the fixed request facts and the text-only candidate carrying `injection`.
/// 构建固定请求事实及携带 `injection` 的纯文本候选。
fn memory_acceptance_candidate(
    injection: ProviderContextInjection,
) -> (ProviderV1RequestFacts, ProviderRequestPreparationCandidate) {
    // The fixed transport fixture carries no credentials and performs no Provider dispatch.
    // 固定传输夹具不携带凭证，也不执行 Provider 分派。
    let facts = ProviderV1RequestFacts {
        provider_call_id: "memory-acceptance".to_owned(),
        purpose: ApiPurposeKind::MainGeneration,
        continuation_action_id: None,
        request: ApiRequestSnapshot::new("provider-1", "model-1"),
        prepared_at_ms: 10,
    };
    // All history starts empty so only the supplied V1 owner can contribute conversation content.
    // 历史初始为空，只有传入的 V1 归属方可以贡献会话内容。
    let candidate = ProviderRequestPreparationCandidate {
        media_context: None,
        exact_request: AiExecutionRequest {
            operation: AiOperation::ConversationRespond,
            model_id: facts.request.model.clone(),
            cache_scope_id: None,
            continuation: None,
            messages: Vec::new(),
            tools: Vec::new(),
            native_tools: Vec::new(),
            tool_choice: Default::default(),
            parallel_tool_calls: Default::default(),
            structured_output: Default::default(),
            stop_sequences: Vec::new(),
            prompt_cache: Default::default(),
            reasoning_effort: None,
            reasoning_output: Default::default(),
            model_parameters: BTreeMap::new(),
            resources: Vec::new(),
            output_token_limit: None,
        },
        controller_config: ControllerConfig {
            provider_id: facts.request.provider.clone(),
            model_id: facts.request.model.clone(),
            temperature: facts.request.temperature,
            context_injection: Some(injection),
            ..Default::default()
        },
        controller_snapshot: ConversationSnapshot::default(),
        request_snapshot: CoreProviderRequestSnapshot {
            provider_id: facts.request.provider.clone(),
            model_id: facts.request.model.clone(),
            system_prompt: String::new(),
            prompt_layers: ProviderPromptLayers::default(),
            prompt: String::new(),
            prompt_in_history: true,
            history: Vec::new(),
            temperature: facts.request.temperature,
            max_turns: 8,
            tool_schemas: Vec::new(),
            additional_params: None,
            request_headers: None,
            continuation: ProviderContinuation::default(),
        },
        context_budget: None,
        // Match the admitted fixture route through the same frozen guard used by physical V1 tests.
        // 使用物理 V1 测试同款冻结守卫匹配已准入夹具路由。
        request_budget_guard: Some(ProviderRequestBudgetGuard::new_v1(
            facts.request.provider.clone(),
            facts.request.model.clone(),
            "test-profile",
            "sha256:test-route",
            ModelCapabilities::default(),
            ProviderRuntimeReasoningResolution::default(),
            None,
            None,
            None,
        )),
        dynamic_prompt_layers: ProviderPromptLayers::default(),
        fixed_instruction_layers: ProviderPromptLayers::default(),
        locked_baseline: host_guided_locked_baseline(),
    };
    (facts, candidate)
}

/// Passes `candidate` and its real V1 `snapshot` through production reconstruction.
/// 将 `candidate` 及其真实 V1 `snapshot` 送入生产重建。
fn rebuild_memory_acceptance_candidate(
    snapshot: SessionProviderProjectionSnapshot,
    facts: &ProviderV1RequestFacts,
    candidate: ProviderRequestPreparationCandidate,
) -> Result<ProviderRequestPreparationCandidate, String> {
    // Resource-bearing fixtures are rejected by the production builder without fabricated media delivery.
    // 带资源的夹具由生产构建器拒绝，不伪造媒体投递。
    let prepared_media = media::PreparedV1UserMedia {
        prompts: BTreeMap::new(),
        bindings: Vec::new(),
    };
    rebuild_provider_transport_candidate(snapshot, facts, candidate, prepared_media)
        .map(|prepared| prepared.transport)
        .map_err(|error| error.to_string())
}
