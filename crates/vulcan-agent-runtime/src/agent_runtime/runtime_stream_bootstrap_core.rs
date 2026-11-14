//! Runtime stream bootstrap core-state construction and request-snapshot seeding helpers.
//! 面向运行时流 bootstrap 核心状态构建与请求快照播种的辅助模块。

use crate::agent_runtime::config::RuntimeAgentDefinition;
use crate::agent_runtime::context_request_mode::resolve_preparation_context_request_mode;
use crate::agent_runtime::execution_models::{ExecutionPreparation, ExecutionPreparationRequest};
use crate::agent_runtime::provider_request_bridge_conversation::build_core_conversation_snapshot;
use crate::agent_runtime::provider_request_bridge_messages::core_messages_to_provider_visible_history_for_model_with_system_updates_and_tool_bridges;
use crate::agent_runtime::provider_request_controller_config::build_core_controller_config_with_layers;
use crate::agent_runtime::provider_request_preview::build_provider_request_preview_markdown;
use crate::agent_runtime::provider_request_snapshot_builder::build_core_provider_request_snapshot;
use crate::agent_runtime::provider_runtime_context::{
    build_runtime_prompt_layers_for_agent, resolve_runtime_max_turns,
    runtime_context_host_event_candidates, synchronize_runtime_host_request_context,
};
use crate::agent_runtime::provider_system_update_lowering::{
    ProviderSystemUpdateLoweringPolicy, lower_provider_system_update,
    provider_system_update_repair_event,
};
use crate::agent_runtime::provider_visible_agent_messages::append_provider_visible_agent_messages_to_core_snapshot;
use crate::context_budget::{
    RequestContextCompactionBudgetSnapshotInput, build_request_context_compaction_budget_snapshot,
};
use vulcan_context::{
    ContextBaseline, RuntimeProviderLoweringRepairEvent, build_provider_request_context_assembly,
};
use vulcan_core::controller::VulcanCoreController;
use vulcan_core::types::{
    ConversationSnapshot as CoreConversationSnapshot,
    ProviderPromptLayers as CoreProviderPromptLayers,
};
use vulcan_session_core::v1::context::LockedBaselineProjection;
use vulcan_tools_manager::ResolvedToolDefinition;

/// Returns whether focused after-precheck diagnostic traces are enabled.
/// 返回 precheck 后专项诊断日志是否已启用。
pub(crate) fn after_precheck_trace_enabled() -> bool {
    std::env::var("VULCAN_WEBUI_AFTER_PRECHECK_TRACE")
        .ok()
        .map(|value| {
            let normalized = value.trim().to_ascii_lowercase();
            matches!(normalized.as_str(), "1" | "true" | "yes" | "on")
        })
        .unwrap_or(false)
}

/// Returns the current Unix timestamp in milliseconds.
/// 返回当前 Unix 毫秒时间戳。
pub(crate) fn after_precheck_unix_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

/// Returns the current RFC3339 timestamp used by WebUI trace lines.
/// 返回 WebUI 诊断日志使用的当前 RFC3339 时间戳。
fn after_precheck_trace_timestamp() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second(),
        now.millisecond()
    )
}

/// Emits one focused after-precheck diagnostic trace line from agent-runtime.
/// 从 agent-runtime 输出一条 precheck 后专项诊断日志。
pub(crate) fn after_precheck_trace(stage: &str, detail: impl AsRef<str>) {
    if !after_precheck_trace_enabled() {
        return;
    }
    eprintln!(
        "[webui-service][after-precheck-trace] timestamp={} unix_ms={} stage={} {}",
        after_precheck_trace_timestamp(),
        after_precheck_unix_millis(),
        stage,
        detail.as_ref()
    );
}

/// Core provider-facing runtime bootstrap state derived from canonical conversation inputs.
/// 从规范会话输入推导得到的面向 Provider 的运行时 bootstrap 核心状态。
pub(crate) struct RuntimeBootstrapCore {
    /// Effective system preamble sent to the provider transport layer.
    /// 发送给 Provider 传输层的生效系统前导语。
    pub system_preamble: String,
    /// Structured prompt layers sent through `vulcan-core` request snapshots.
    /// 通过 `vulcan-core` 请求快照发送的结构化提示词层。
    pub prompt_layers: CoreProviderPromptLayers,
    /// Request-dynamic prompt layers excluded from the locked context baseline.
    /// 从锁定上下文基线中排除的请求动态提示词层。
    pub dynamic_prompt_layers: CoreProviderPromptLayers,
    /// Fixed system and developer instructions carried to the physical request head.
    /// 传递到物理请求头部的固定系统和开发者指令。
    pub fixed_instruction_layers: CoreProviderPromptLayers,
    /// Canonical locked baseline derived from the same typed prompt composition.
    /// 从同一强类型提示词组合推导出的规范锁定基线。
    pub locked_baseline: LockedBaselineProjection,
    /// Canonical conversation snapshot handed to `vulcan-core`.
    /// 交给 `vulcan-core` 的规范会话快照。
    pub core_snapshot: CoreConversationSnapshot,
    /// Host-owned controller that tracks streaming deltas and execution events.
    /// 负责跟踪流式增量与执行事件的宿主自有控制器。
    pub core_controller: VulcanCoreController,
    /// Provider lowering repair events detected during bootstrap projection.
    /// 在 bootstrap 投影期间检测到的 Provider 转换修复事件。
    pub lowering_repair_events: Vec<RuntimeProviderLoweringRepairEvent>,
}

/// Builds the provider-facing runtime bootstrap core from the canonical conversation state and execution preparation.
/// 根据规范会话状态与执行准备构建面向 Provider 的运行时 bootstrap 核心对象。
///
/// # Parameters
/// # 参数
/// - `preparation`: Validated execution preparation containing provider, workspace, and prompt inputs.
/// - `preparation`：包含 Provider、工作区与提示输入的已验证执行准备。
/// - `request`: Canonical conversation request projected into the provider-facing core snapshot.
/// - `request`：投影为面向 Provider 核心快照的规范会话请求。
/// - `active_agent`: Optional resolved agent definition supplying agent-scoped runtime settings.
/// - `active_agent`：提供代理级运行时设置的可选已解析代理定义。
///
/// # Returns
/// # 返回值
/// Returns a complete bootstrap core, or the first exact prompt-source, projection, or controller validation failure.
/// 返回完整 bootstrap 核心，或首个精确的提示来源、投影或控制器验证失败。
pub(crate) fn build_runtime_bootstrap_core(
    preparation: &ExecutionPreparation,
    request: &ExecutionPreparationRequest,
    active_agent: Option<&RuntimeAgentDefinition>,
) -> Result<RuntimeBootstrapCore, String> {
    // runtime_prompt is complete only after every configured project prompt source loads successfully.
    // runtime_prompt 仅在所有已配置项目提示来源均成功加载后才是完整结果。
    let runtime_prompt = build_runtime_prompt_layers_for_agent(preparation, active_agent)?;
    let system_preamble = runtime_prompt.fallback_text.clone();
    let mut core_snapshot = build_core_conversation_snapshot(&request.conversation_state)?;
    let bridge_tool_ids = preparation
        .tool_descriptors
        .iter()
        .map(|tool| tool.id.clone())
        .collect::<Vec<_>>();
    let system_update_policy = ProviderSystemUpdateLoweringPolicy;
    let lowering_repair_events = collect_system_update_lowering_repairs(
        preparation,
        &request.runtime_system_updates,
        system_update_policy,
    );
    core_snapshot.messages =
        core_messages_to_provider_visible_history_for_model_with_system_updates_and_tool_bridges(
            &core_snapshot.messages,
            Some(preparation.model_blueprint.protocol_profile_id.as_str()),
            Some(&preparation.model_blueprint.public_capabilities),
            &bridge_tool_ids,
            &preparation.runtime_system_updates,
            system_update_policy,
        )?;
    append_provider_visible_agent_messages_to_core_snapshot(
        &mut core_snapshot,
        &request.provider_visible_agent_messages,
    )?;
    let mut host_events = runtime_prompt.host_events.clone();
    if let Some(injection) = request.context_injection.as_ref() {
        host_events.extend(runtime_context_host_event_candidates(injection));
    }
    // Only the same resolved UserPrompt launch mode owns a new unsent user insertion boundary.
    // 仅相同解析结果的 UserPrompt 启动模式拥有新的未发送用户插入边界。
    let has_new_user_boundary = matches!(
        crate::agent_runtime::runtime_stream_launch_payload::runtime_stream_turn_start_mode_for_request(request),
        crate::agent_runtime::runtime_stream_launch_payload::RuntimeStreamTurnStartMode::UserPrompt
    );
    let current_user_message_id = request
        .conversation_state
        .messages
        .last()
        .filter(|message| {
            has_new_user_boundary
                && !message
                    .id
                    .starts_with(vulcan_context::runtime_host::RUNTIME_HOST_MESSAGE_PREFIX)
                && matches!(
                    &message.payload,
                    vulcan_context::SessionConversationPayload::UserText { .. }
                )
        })
        .map(|message| message.id.as_str());
    synchronize_runtime_host_request_context(
        &mut core_snapshot,
        &host_events,
        current_user_message_id,
        preparation.prompt_purpose,
    )?;
    // Match the actual transport entry: only the stable preamble belongs to the top-level controller layers.
    // 与实际传输入口保持一致：只有稳定前导语属于控制器顶部提示层。
    let stable_prompt_layers =
        CoreProviderPromptLayers::from_system_prompt(system_preamble.clone());
    let controller_config = build_core_controller_config_with_layers(
        preparation,
        &system_preamble,
        stable_prompt_layers.clone(),
        resolve_runtime_max_turns(active_agent, preparation.prompt_purpose),
        None,
    );
    let core_controller = VulcanCoreController::new(controller_config, core_snapshot.clone())
        .map_err(|error| format!("Invalid core conversation snapshot: {error}"))?;

    Ok(RuntimeBootstrapCore {
        system_preamble,
        prompt_layers: stable_prompt_layers,
        dynamic_prompt_layers: runtime_prompt.dynamic_prompt_layers,
        fixed_instruction_layers: runtime_prompt.fixed_instruction_layers,
        locked_baseline: runtime_prompt.locked_baseline,
        core_snapshot,
        core_controller,
        lowering_repair_events,
    })
}

/// Collects audit repair events for runtime system updates that need fallback or rejection.
/// 收集需要降级或拒绝的 runtime system update 审计修复事件。
fn collect_system_update_lowering_repairs(
    preparation: &ExecutionPreparation,
    updates: &[vulcan_context::RuntimeSystemUpdateEvent],
    policy: ProviderSystemUpdateLoweringPolicy,
) -> Vec<RuntimeProviderLoweringRepairEvent> {
    let provider_family = preparation
        .model_blueprint
        .protocol_profile_key()
        .to_string();
    updates
        .iter()
        .filter_map(|update| {
            let lowered = lower_provider_system_update(update, policy);
            provider_system_update_repair_event(
                update,
                &lowered,
                &preparation.model_blueprint.provider_id,
                &preparation.model_blueprint.model_id,
                &provider_family,
            )
        })
        .collect()
}

/// Seeds the final request preview and structured request snapshot onto the mutable execution preparation object.
/// 将最终请求预览与结构化请求快照播种到可变执行准备对象上。
pub(crate) async fn seed_runtime_request_snapshot(
    preparation: &mut ExecutionPreparation,
    request: &ExecutionPreparationRequest,
    system_preamble: &str,
    prompt_layers: CoreProviderPromptLayers,
    resolved_tool_definitions: &[ResolvedToolDefinition],
    runtime_max_turns: Option<usize>,
    warnings: &mut Vec<String>,
) {
    let seed_started_at = after_precheck_unix_millis();
    let snapshot_started_at = after_precheck_unix_millis();
    match build_core_provider_request_snapshot(
        preparation,
        &request.conversation_state,
        system_preamble,
        prompt_layers,
        request.prompt.trim(),
        &request.provider_visible_agent_messages,
        resolved_tool_definitions,
        runtime_max_turns,
    )
    .await
    {
        Ok(request_snapshot) => {
            after_precheck_trace(
                "after_precheck_29a_core_request_snapshot_ready",
                format!(
                    "session={} elapsed_ms={}",
                    preparation.session_id,
                    after_precheck_unix_millis().saturating_sub(snapshot_started_at)
                ),
            );
            let budget_started_at = after_precheck_unix_millis();
            // Native media counts as attachments, never as Base64 prompt characters, exactly like the transport guard.
            // 原生媒体按附件计数，绝不按 Base64 提示词字符计数，与传输守卫完全一致。
            preparation.context_budget = match crate::native_media_input::budget_snapshot(
                &request_snapshot,
            ) {
                Ok(media_snapshot) => Some(build_request_context_compaction_budget_snapshot(
                    RequestContextCompactionBudgetSnapshotInput {
                        provider_id: &preparation.model_blueprint.provider_id,
                        model_id: &preparation.model_blueprint.model_id,
                        protocol_profile_id: &preparation.model_blueprint.protocol_profile_id,
                        capabilities: &preparation.model_blueprint.public_capabilities,
                        reasoning: &preparation.model_blueprint.reasoning,
                        request_snapshot: media_snapshot.as_ref(),
                        compaction_policy: preparation.context_compaction_policy,
                        context_budget_override: preparation.context_budget_override,
                        observed_usage: None,
                        accounted_input_tokens: None,
                        accounting_source:
                            crate::context_budget::ContextBudgetAccountingSource::CharacterEstimate,
                        accounting_warning_code: Some(
                            "provider_usage_baseline_pending_request_assembly",
                        ),
                        provider_anchor_call_id: None,
                        prefill_input_tokens: request.context_window_prefill_input_tokens,
                    },
                )),
                // The transport guard rejects the same malformed media before dispatch.
                // 传输守卫会在分派前拒绝同一份畸形媒体。
                Err(error) => {
                    warnings.push(format!(
                        "Failed to project native media for the context budget: {error}"
                    ));
                    None
                }
            };
            after_precheck_trace(
                "after_precheck_29b_context_budget_ready",
                format!(
                    "session={} elapsed_ms={}",
                    preparation.session_id,
                    after_precheck_unix_millis().saturating_sub(budget_started_at)
                ),
            );
            let assembly_started_at = after_precheck_unix_millis();
            let context_assembly = build_provider_request_context_assembly(&request_snapshot);
            preparation.context_baseline = Some(ContextBaseline::from_assembly(&context_assembly));
            preparation.context_assembly = Some(context_assembly);
            preparation.context_request_mode = Some(resolve_preparation_context_request_mode(
                preparation,
                &request_snapshot,
            ));
            after_precheck_trace(
                "after_precheck_29c_context_assembly_ready",
                format!(
                    "session={} elapsed_ms={}",
                    preparation.session_id,
                    after_precheck_unix_millis().saturating_sub(assembly_started_at)
                ),
            );
            let preview_started_at = after_precheck_unix_millis();
            preparation.provider_request_preview_markdown =
                build_provider_request_preview_markdown(preparation, &request_snapshot);
            after_precheck_trace(
                "after_precheck_29d_preview_ready",
                format!(
                    "session={} chars={} elapsed_ms={}",
                    preparation.session_id,
                    preparation
                        .provider_request_preview_markdown
                        .chars()
                        .count(),
                    after_precheck_unix_millis().saturating_sub(preview_started_at)
                ),
            );
            let json_started_at = after_precheck_unix_millis();
            preparation.provider_request_snapshot_json =
                serde_json::to_value(&request_snapshot).ok();
            after_precheck_trace(
                "after_precheck_29e_snapshot_json_ready",
                format!(
                    "session={} success={} elapsed_ms={} elapsed_ms_since_seed={}",
                    preparation.session_id,
                    preparation.provider_request_snapshot_json.is_some(),
                    after_precheck_unix_millis().saturating_sub(json_started_at),
                    after_precheck_unix_millis().saturating_sub(seed_started_at)
                ),
            );
        }
        Err(error) => {
            preparation.provider_request_preview_markdown = format!(
                "# Last real API request snapshot\n\nFailed to build the request snapshot: {}",
                error
            );
            preparation.provider_request_snapshot_json = None;
            warnings.push(format!("Failed to build the request snapshot: {error}"));
            after_precheck_trace(
                "after_precheck_29x_request_snapshot_failed",
                format!(
                    "session={} elapsed_ms_since_seed={} error={}",
                    preparation.session_id,
                    after_precheck_unix_millis().saturating_sub(seed_started_at),
                    error
                ),
            );
        }
    }
}
