//! Runtime stream launch payload model and bootstrap-to-launch assembly helpers.
//! 面向运行时流启动载荷模型与 bootstrap 到 launch 组装转换的辅助模块。

use crate::agent_runtime::config::RuntimeAgentDefinition;
use crate::agent_runtime::events::RuntimeEvent;
use crate::agent_runtime::execution_models::{
    ExecutionPreparation, ExecutionPreparationRequest, ExecutionTurnStartMode,
    resolve_assistant_tail_restore,
};
use crate::agent_runtime::provider_request_telemetry::ProviderRequestTelemetryContext;
use crate::agent_runtime::runtime_stream_bootstrap::RuntimeStreamBootstrap;
use crate::agent_runtime::runtime_stream_delta::RuntimeStreamDeltaState;
use crate::agent_runtime::task_runtime::{TaskExecutionRuntimeContext, TaskRuntime};
use crate::model::SessionId;
use std::sync::Arc;
use vulcan_context::RuntimeProviderLoweringRepairEvent;
use vulcan_core::types::{
    ConversationPayload as CoreConversationPayload,
    ConversationSnapshot as CoreConversationSnapshot,
    ProviderPromptLayers as CoreProviderPromptLayers,
};
use vulcan_session_core::v1::context::LockedBaselineProjection;
use vulcan_tools_manager::{ResolvedToolDefinition, VulcanToolsManager};

/// Turn-start mode used by the host-controlled provider loop.
/// 宿主控制 Provider 循环使用的回合启动模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeStreamTurnStartMode {
    /// Start by appending or reusing the user-authored prompt.
    /// 通过追加或复用用户提示词启动。
    UserPrompt,
    /// Start from provider-visible messages already seeded into the snapshot.
    /// 从已经播种到快照中的 Provider 可见消息启动。
    RestoredCheckpoint,
    /// Resume an admitted Provider loop even after its one-shot Continue action has been consumed.
    /// 恢复已准入的 Provider 循环，即使对应一次性 Continue 动作已经消费。
    ContinuedProviderTurn,
}

/// Launch payload that captures everything the outer runtime stream needs after preparation and bootstrap have both succeeded.
/// 捕获准备阶段与启动阶段都成功后外层运行时流所需全部内容的启动载荷。
pub struct RuntimeStreamLaunch {
    /// Shared runtime context registered for spawn-agent child launches while this provider loop is alive.
    /// 当前 Provider 循环存活期间为 spawn_agent 子代理启动注册的共享运行时上下文。
    pub(crate) task_execution_context: Option<TaskExecutionRuntimeContext>,
    /// Session identifier owned by the current runtime run.
    /// 当前运行时执行所属的会话标识。
    pub(crate) session_id: SessionId,
    /// Trimmed effective user prompt that will be sent to the provider transport layer.
    /// 将发送给 Provider 传输层的裁剪后生效用户提示词。
    pub(crate) effective_prompt: String,
    /// Explicit turn-start mode consumed by the provider transport layer.
    /// Provider 传输层消费的显式回合启动模式。
    pub(crate) turn_start_mode: RuntimeStreamTurnStartMode,
    /// Updated execution preparation after request-snapshot seeding.
    /// 完成请求快照播种之后的更新执行准备对象。
    pub(crate) preparation: ExecutionPreparation,
    /// Active agent definition resolved from the runtime foundation for this turn.
    /// 为当前回合从运行时基础架构解析得到的激活代理定义。
    pub(crate) active_agent: Option<RuntimeAgentDefinition>,
    /// Shared resumable task runtime reused by the current execution turn.
    /// 被当前执行回合复用的共享可恢复任务运行时。
    pub(crate) task_runtime: Arc<TaskRuntime>,
    /// Host-owned unified tool manager used by the transport layer.
    /// 供传输层使用的宿主自有统一工具管理器。
    pub(crate) tools_manager: VulcanToolsManager,
    /// Final resolved tool definitions exposed to the provider for this turn.
    /// 当前回合向 Provider 暴露的最终已解析工具定义集合。
    pub(crate) resolved_tool_definitions: Vec<ResolvedToolDefinition>,
    /// Discovery events projected from enabled MCP runtime servers.
    /// 从已启用 MCP 运行时服务器投影得到的发现事件集合。
    pub(crate) discovery_events: Vec<RuntimeEvent>,
    /// Human-readable MCP discovery rows captured for the final execution report.
    /// 为最终执行报告捕获的人类可读 MCP 发现摘要行集合。
    pub(crate) discovery_rows: Vec<String>,
    /// MCP discovery errors captured for the final execution report.
    /// 为最终执行报告捕获的 MCP 发现错误集合。
    pub(crate) discovery_errors: Vec<String>,
    /// Mutable warning list accumulated before provider streaming begins.
    /// 在 Provider 流式执行开始前累计得到的可变警告列表。
    pub(crate) warnings: Vec<String>,
    /// Effective system preamble sent to the provider transport layer.
    /// 发送给 Provider 传输层的生效系统前导语。
    pub(crate) system_preamble: String,
    /// Canonical typed prompt baseline proposed for the physical request boundary.
    /// 为物理请求边界提出的规范强类型提示词基线。
    pub(crate) locked_baseline: LockedBaselineProjection,
    /// Request-dynamic prompt layers preserved for the physical Provider permit.
    /// 为物理 Provider 许可保留的请求动态提示词层。
    pub(crate) dynamic_prompt_layers: CoreProviderPromptLayers,
    /// Fixed system and developer instructions carried to the physical request head.
    /// 传递到物理请求头部的固定系统和开发者指令。
    pub(crate) fixed_instruction_layers: CoreProviderPromptLayers,
    /// Canonical conversation snapshot handed to `vulcan-core`.
    /// 交给 `vulcan-core` 的规范会话快照。
    pub(crate) core_snapshot: CoreConversationSnapshot,
    /// Mutable streaming-loop state seeded from the bootstrapped core controller.
    /// 由已启动核心控制器播种得到的可变流式循环状态。
    pub(crate) stream_state: RuntimeStreamDeltaState,
    /// Provider lowering repair events detected before the provider loop starts.
    /// Provider 循环启动前检测到的 Provider 转换修复事件。
    pub(crate) lowering_repair_events: Vec<RuntimeProviderLoweringRepairEvent>,
    /// Process-local public AI runtime installed for this launch.
    /// 为本次启动安装的进程内公共 AI 运行时。
    pub(crate) public_ai_runtime: Option<Arc<vulcan_provider_adapters::PublicAiRuntime>>,
}

/// Builds the final runtime-stream launch payload from one execution request and one completed bootstrap payload.
/// 根据一份执行请求与一份完成后的 bootstrap 载荷构建最终运行时流启动载荷。
///
/// # Parameters
/// 参数
/// `request` owns replay and Provider-continuation identity, `bootstrap` owns validated runtime facts,
/// and `task_execution_context` optionally binds a child task runtime.
/// `request` 持有重放与 Provider 延续身份，`bootstrap` 持有已校验运行时事实，
/// `task_execution_context` 可选绑定子任务运行时。
///
/// # Returns
/// 返回值
/// Returns the launch payload, or an explicit error when mutually exclusive continuation sources conflict.
/// 返回启动载荷；当互斥的延续来源冲突时返回显式错误。
pub(crate) fn build_runtime_stream_launch_payload(
    request: &ExecutionPreparationRequest,
    bootstrap: RuntimeStreamBootstrap,
    task_execution_context: Option<TaskExecutionRuntimeContext>,
) -> Result<RuntimeStreamLaunch, String> {
    let turn_start_mode = runtime_stream_turn_start_mode_for_request(request);
    let effective_prompt =
        controller_effective_prompt_for_launch(request, &bootstrap.core_snapshot)?;
    // providerTelemetryContext captures model and owner facts before preparation moves into the launch payload.
    // providerTelemetryContext 在执行准备移入启动载荷前捕获模型与归属事实。
    let provider_telemetry_context = ProviderRequestTelemetryContext::from_preparation(
        &bootstrap.preparation,
        request.provider_request_continuation.as_ref(),
    )?;
    // streamResolvedTools freezes parser capabilities before definitions move into the launch payload.
    // streamResolvedTools 在工具定义移入启动载荷前冻结解析器能力。
    let stream_resolved_tools = bootstrap.resolved_tool_definitions.clone();
    Ok(RuntimeStreamLaunch {
        task_execution_context,
        session_id: request.session_id,
        effective_prompt,
        turn_start_mode,
        preparation: bootstrap.preparation,
        active_agent: bootstrap.active_agent,
        task_runtime: bootstrap.task_runtime,
        tools_manager: bootstrap.tools_manager,
        resolved_tool_definitions: bootstrap.resolved_tool_definitions,
        discovery_events: bootstrap.discovery_events,
        discovery_rows: bootstrap.discovery_rows,
        discovery_errors: bootstrap.discovery_errors,
        warnings: bootstrap.warnings,
        system_preamble: bootstrap.system_preamble,
        dynamic_prompt_layers: bootstrap.dynamic_prompt_layers,
        fixed_instruction_layers: bootstrap.fixed_instruction_layers,
        locked_baseline: bootstrap.locked_baseline,
        core_snapshot: bootstrap.core_snapshot,
        stream_state: RuntimeStreamDeltaState::new(bootstrap.core_controller)
            .with_tool_request_stream(&stream_resolved_tools)
            .with_provider_request_telemetry_context(provider_telemetry_context),
        lowering_repair_events: bootstrap.lowering_repair_events,
        public_ai_runtime: bootstrap.public_ai_runtime,
    })
}

/// Resolves how the provider loop should start the next controller turn.
/// 解析 Provider 循环应如何启动下一次控制器回合。
pub(crate) fn runtime_stream_turn_start_mode_for_request(
    request: &ExecutionPreparationRequest,
) -> RuntimeStreamTurnStartMode {
    // Collaboration input was durably projected before launch and must not append a human prompt.
    // 协作输入在启动前已可靠投影，不能再追加人类提示词。
    if matches!(
        request.prompt_purpose,
        crate::agent_runtime::execution_models::ExecutionPromptPurpose::QuestionExecution
            | crate::agent_runtime::execution_models::ExecutionPromptPurpose::AgentTask
    ) {
        return RuntimeStreamTurnStartMode::RestoredCheckpoint;
    }
    if let Some(mode) = request.turn_start_mode {
        return match mode {
            ExecutionTurnStartMode::UserPrompt => RuntimeStreamTurnStartMode::UserPrompt,
            ExecutionTurnStartMode::RestoredCheckpoint => {
                // Recovery survives API binding; only an admitted loop may restore an assistant tail.
                // 恢复能力在 API 绑定后仍保留；只有已准入循环可以恢复助手尾部。
                if resolve_assistant_tail_restore(
                    request.continuation_action_id.as_ref(),
                    request.provider_request_continuation.as_ref(),
                ) {
                    RuntimeStreamTurnStartMode::ContinuedProviderTurn
                } else {
                    RuntimeStreamTurnStartMode::RestoredCheckpoint
                }
            }
        };
    }
    if request.provider_visible_agent_messages.is_empty() {
        RuntimeStreamTurnStartMode::UserPrompt
    } else {
        RuntimeStreamTurnStartMode::RestoredCheckpoint
    }
}

/// Resolves the prompt text handed to `vulcan-core` so preseeded user messages are not duplicated after provider projection.
/// 解析交给 `vulcan-core` 的提示词文本，避免 Provider 投影后重复追加已预播种的用户消息。
///
/// # Parameters
/// 参数
/// `request` is the original execution request carrying the user-authored prompt.
/// `request` 是携带用户原始提示词的执行请求。
/// `core_snapshot` is the provider-visible snapshot already prepared for the controller.
/// `core_snapshot` 是已经为控制器准备好的 Provider 可见快照。
///
/// # Returns
/// 返回值
/// The projected latest user text when present, otherwise the raw request prompt.
/// 当存在时返回投影后的最新用户文本，否则返回原始请求提示词。
fn controller_effective_prompt_for_launch(
    request: &ExecutionPreparationRequest,
    core_snapshot: &CoreConversationSnapshot,
) -> Result<String, String> {
    if !request.provider_visible_agent_messages.is_empty() {
        let prompt = request
            .provider_visible_agent_messages
            .iter()
            .map(|message| {
                CoreConversationPayload::render_agent_message_text(
                    &message.delivery_id,
                    message.kind,
                    &message.sender,
                    &message.recipient,
                    &message.task_id,
                    &message.payload,
                )
            })
            .collect::<Result<Vec<_>, _>>()?
            .join("\n\n");
        return Ok(prompt);
    }
    let raw_prompt = request.prompt.trim().to_string();
    // A resource-only input reaches the controller as its projected note, never as a blank prompt.
    // 仅资源输入以其投影说明进入控制器，绝不作为空白提示词。
    let raw_prompt = if raw_prompt.is_empty() && current_turn_is_resource_only(core_snapshot) {
        String::from(vulcan_session_core::v1::projection::RESOURCE_ONLY_INPUT_TEXT)
    } else {
        raw_prompt
    };
    Ok(core_snapshot
        .messages
        .last()
        .and_then(|message| match &message.payload {
            CoreConversationPayload::UserText { text, .. } => Some(text.trim().to_string()),
            _ => None,
        })
        .filter(|text| projected_user_prompt_matches_request(text, &raw_prompt))
        .unwrap_or(raw_prompt))
}

/// Returns whether the trailing user messages of the controller snapshot hold a resource-only input.
/// 判断控制器快照末尾的用户消息中是否包含仅资源输入。
///
/// # Parameters
/// 参数
/// `core_snapshot` is the provider-visible snapshot already prepared for the controller.
/// `core_snapshot` 是已经为控制器准备好的 Provider 可见快照。
///
/// # Returns
/// 返回值
/// `true` when a user message after the last non-user message carries the resource-only note.
/// 当最后一条非用户消息之后的某条用户消息带有仅资源说明时返回 `true`。
fn current_turn_is_resource_only(core_snapshot: &CoreConversationSnapshot) -> bool {
    core_snapshot
        .messages
        .iter()
        .rev()
        .map_while(|message| match &message.payload {
            CoreConversationPayload::UserText { text, .. } => Some(text),
            _ => None,
        })
        .any(|text| {
            projected_user_prompt_matches_request(
                text,
                vulcan_session_core::v1::projection::RESOURCE_ONLY_INPUT_TEXT,
            )
        })
}

/// Returns whether one projected user prompt can safely stand in for the raw request prompt.
/// 判断一条投影后的用户提示词是否可以安全代表原始请求提示词。
///
/// # Parameters
/// 参数
/// `projected_prompt` is the latest user text already present in the controller snapshot.
/// `projected_prompt` 是控制器快照中已经存在的最新用户文本。
/// `raw_prompt` is the trimmed user prompt carried by the execution request.
/// `raw_prompt` 是执行请求携带的裁剪后用户提示词。
///
/// # Returns
/// 返回值
/// `true` when the projected prompt keeps the raw prompt as one complete paragraph.
/// 当投影提示词仍把原始提示词保留为一个完整段落时返回 `true`。
fn projected_user_prompt_matches_request(projected_prompt: &str, raw_prompt: &str) -> bool {
    if projected_prompt.is_empty() || raw_prompt.is_empty() {
        return false;
    }
    // A multi-paragraph prompt spans several split pieces, so it must also match as a whole run of
    // paragraphs.
    // 多段提示词跨越多个拆分片段，因此还须作为一组完整连续段落匹配。
    projected_prompt
        .split("\n\n")
        .any(|paragraph| paragraph.trim() == raw_prompt)
        || projected_prompt == raw_prompt
        || projected_prompt.starts_with(&format!("{raw_prompt}\n\n"))
        || projected_prompt.ends_with(&format!("\n\n{raw_prompt}"))
        || projected_prompt.contains(&format!("\n\n{raw_prompt}\n\n"))
}

#[cfg(test)]
mod tests {
    //! Launch-payload assembly tests for runtime stream orchestration.
    //! 面向运行时流编排的启动载荷组装测试。

    mod continuation;

    use super::*;
    use crate::agent_runtime::conversation_hooks::ConversationHookRuntime;
    use crate::agent_runtime::execution_models::ExecutionProviderVisibleAgentMessage;
    use crate::agent_runtime::provider::ProviderRuntimeBlueprint;
    use crate::agent_runtime::runtime_stream_bootstrap_core::build_runtime_bootstrap_core;
    use crate::conversation_protocol::SessionConversationState;
    use crate::workbench_prompt_turn::{
        SharedPromptTurnContinueInput, SharedPromptTurnRuntimeBeginInput,
        begin_shared_prompt_turn_with_runtime_state,
        continue_shared_prompt_turn_after_preparation_with_runtime_state,
    };
    use std::sync::Arc;
    use uuid::Uuid;
    use vulcan_core::controller::VulcanCoreController;
    use vulcan_core::types::{
        ControllerConfig, ConversationMessage as CoreConversationMessage,
        ConversationPayload as CoreConversationPayload,
    };
    use vulcan_core::types::{ConversationAttachment, ConversationAttachmentKind};
    use vulcan_workbench_model::{PendingSessionPromptSource, SessionRuntimeStateEntry};

    /// Builds one minimal execution preparation used by launch-payload tests.
    /// 构建一份供启动载荷测试使用的最小执行准备对象。
    fn build_test_preparation() -> ExecutionPreparation {
        ExecutionPreparation {
            session_id: Uuid::nil(),
            session_storage_directory: None,
            conversation_hook_turn_id: None,
            continuation_action_id: None,
            session_title: String::from("测试会话"),
            agent_id: String::from("default-agent"),
            agent_name: String::from("默认代理"),
            agent_kind: String::from("内置"),
            agent_prompt_snapshot: None,
            dedicated_session: None,
            subagent_prompt_context: None,
            model_blueprint: ProviderRuntimeBlueprint {
                provider_id: String::from("openai"),
                service_entry_id: String::from("public-api"),
                connection_id: String::from("test-connection"),
                provider_name: String::from("OpenAI"),
                runtime_label: String::from("OpenAI"),
                model_id: String::from("gpt-test"),
                model_name: String::from("GPT Test"),
                protocol_profile_id: String::from(vulcan_llm_core::OPENAI_CHAT_PROFILE_ID),
                manifest_revision: 0,
                frozen_selection: None,
                route_revision: String::from("sha256:test-route"),
                secondary_route: Default::default(),
                public_capabilities: vulcan_ai_contract::ModelCapabilities::default(),
                base_url: Some(String::from("https://api.example.test/v1")),
                supports_remote_model_refresh: true,
                credential_source: String::from("环境变量"),
                reasoning: Default::default(),
            },
            prompt_preview: String::from("请分析代码"),
            workspace_path: String::from(r"D:\workspace"),
            conversation_attachments: Vec::new(),
            context_history_count: 1,
            execution_mode: String::from("Build"),
            temperature: Some(0.2),
            session_request_options: Default::default(),
            prompt_configuration: crate::provider_config::PersistedPromptConfiguration::default(),
            output_style_snapshot: None,
            prompt_purpose: crate::agent_runtime::ExecutionPromptPurpose::UserTurn,
            prompt_skills: Vec::new(),
            prompt_skill_user_root: None,
            additional_developer_segments: Vec::new(),
            skill_names: Vec::new(),
            tool_descriptors: Vec::new(),
            tool_request_rejection_reason: None,
            tool_request_rejection_all_tools: false,
            tool_request_rejection_tool_ids: Vec::new(),
            delegate_agents: Vec::new(),
            enabled_mcp_servers: Vec::new(),
            warnings: Vec::new(),
            provider_request_preview_markdown: String::new(),
            provider_request_snapshot_json: None,
            context_budget: None,
            context_compaction_policy: None,
            context_budget_override: None,
            context_assembly: None,
            context_baseline: None,
            context_request_mode: None,
            context_injection: None,
            system_prompt_hook_blocks: Vec::new(),
            project_vmm: Default::default(),
            session_vmm: Default::default(),
            vmm_runtime_ready: false,
            boundary_events: Vec::new(),
            runtime_system_updates: Vec::new(),
            session_goal_context: None,
            session_plan_context: None,
            embedded_turn_usage_segments: Vec::new(),
        }
    }

    /// Builds one minimal execution request used by launch-payload tests.
    /// 构建一份供启动载荷测试使用的最小执行请求。
    fn build_test_request() -> ExecutionPreparationRequest {
        ExecutionPreparationRequest {
            session_id: Uuid::nil(),
            session_storage_directory: None,
            execution_scope: vulcan_session_core::ExecutionScope::MainSession {
                session_id: vulcan_session_core::SessionNodeId::from(Uuid::nil()),
            },
            runtime_policy: vulcan_session_core::ExecutionRuntimePolicy::main_session(),
            conversation_hook_turn_id: Some(String::from("test-hook-turn")),
            continuation_action_id: None,
            provider_request_continuation: None,
            execution_agent_id: None,
            agent_prompt_snapshot: None,
            dedicated_session: None,
            provider_id_override: None,
            model_id_override: None,
            connection_id_override: None,
            subagent_model_policy: None,
            provider_route_lease_identity: None,
            provider_tool_selections_override: Default::default(),
            provider_tool_media_understanding_selections_override: Default::default(),
            provider_tool_enabled_capabilities_override: Default::default(),
            provider_extension_target_sets_override: None,
            provider_extension_modes_override: None,
            subagent_prompt_context: None,
            session_title: String::from("测试会话"),
            workspace_path: String::from(r"D:\workspace"),
            project_id: None,
            prompt: String::from("  请分析主入口文件  "),
            output_style_snapshot: None,
            provider_visible_agent_messages: Vec::new(),
            turn_start_mode: None,
            conversation_state: SessionConversationState::default(),
            context_compaction_policy: None,
            context_window_selection: None,
            context_budget_override: None,
            context_window_prefill_input_tokens: None,
            execution_mode: String::from("Build"),
            temperature: Some(0.2),
            session_request_options: Default::default(),
            reasoning_effort: crate::provider_config::ReasoningEffortLevel::Off,
            tool_exposure_override: Default::default(),
            captured_child_tool_admission: None,
            tool_request_rejection_reason: None,
            tool_request_rejection_all_tools: false,
            tool_request_rejection_tool_ids: Vec::new(),
            tool_request_hidden_tool_ids: Vec::new(),
            prompt_purpose: crate::agent_runtime::ExecutionPromptPurpose::UserTurn,
            prompt_skill_policy_override: Default::default(),
            project_prompt_skills: Default::default(),
            session_prompt_skill_overrides: Default::default(),
            context_injection: None,
            system_prompt_hook_blocks: Vec::new(),
            project_vmm: Default::default(),
            session_vmm: Default::default(),
            vmm_runtime_ready: true,
            boundary_events: Vec::new(),
            runtime_system_updates: Vec::new(),
            controller_runtime: None,
            native_goal_runtime: None,
            native_browser_runtime: None,
            native_agent_collaboration_runtime: None,
            supplemental_mcp_tools: Vec::new(),
            media_resource_runtime: None,
            knowledge_access: None,
            session_goal_context: None,
            session_plan_context: None,
            embedded_turn_usage_segments: Vec::new(),
        }
    }

    /// Runs real lazy launch preparation on a fixed small stack in an isolated process.
    /// 在独立进程的固定小栈上运行真实惰性启动准备。
    /// A missing agent makes the result deterministic before any Provider request or user data access.
    /// 使用不存在的代理，在任何供应商请求或用户数据访问前获得确定结果。
    #[test]
    fn runtime_stream_preparation_fits_small_stack() {
        // Child mode isolates fatal stack overflow from the parent test harness.
        // 子进程模式将致命栈溢出与父测试框架隔离。
        const CHILD_MODE: &str = "VULCAN_RUNTIME_STACK_TEST_CHILD";
        // Completion evidence prevents an empty child test selection from passing silently.
        // 完成证据防止子进程未选中任何测试却静默通过。
        const COMPLETED: &str = "bounded runtime preparation completed";
        if std::env::var_os(CHILD_MODE).is_none() {
            // Trace settings must not select a different execution or cancellation path.
            // 追踪设置不得选择不同的执行或取消路径。
            for trace_enabled in ["0", "1"] {
                // Exact test selection prevents recursive execution of unrelated tests in the child.
                // 精确测试选择避免子进程递归执行其他测试。
                let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "agent_runtime::runtime_stream_launch_payload::tests::runtime_stream_preparation_fits_small_stack",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(CHILD_MODE, "1")
                .env("VULCAN_WEBUI_AFTER_PRECHECK_TRACE", trace_enabled)
                .output()
                .unwrap();
                assert!(
                    output.status.success(),
                    "launch on the bounded stack failed: trace={trace_enabled} status={}\n{}\n{}",
                    output.status,
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                assert!(
                    String::from_utf8_lossy(&output.stdout).contains(COMPLETED),
                    "the child must execute and complete the selected stack regression test"
                );
            }
            return;
        }
        // One MiB leaves headroom below the production worker stack for host task wrappers.
        // 一 MiB 为生产工作线程栈上的宿主任务包装保留余量。
        std::thread::Builder::new()
            .name("runtime-launch-stack-test".into())
            .stack_size(1024 * 1024)
            .spawn(|| {
                use futures_util::StreamExt;
                // Runtime has no external services and uses the real first-poll execution path.
                // runtime 不含外部服务，并使用真实首次轮询执行路径。
                let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
                runtime.block_on(async {
                    // Request fails inside execution preparation, at the function seen in the crash dump.
                    // request 在崩溃转储中对应的执行准备函数内失败。
                    let mut request = build_test_request();
                    request.execution_agent_id = Some("stack-test-missing-agent".into());
                    // Stream keeps the production entrypoint, without installing a network runtime.
                    // stream 保持生产入口，不安装网络运行时。
                    let mut stream = Box::pin(crate::agent_runtime::runtime_stream::stream_runtime_run(
                        crate::provider_config::ProviderConfigurationFile::default(),
                        crate::agent_profile::AgentProfileCatalog::default(),
                        None, None,
                        Arc::new(crate::agent_runtime::task_runtime::TaskRuntime::new()),
                        Arc::new(crate::agent_runtime::question_runtime::QuestionRuntime::new()),
                        Arc::new(crate::tooling::CommandSessionRuntime::new()),
                        request,
                    ));
                    let Some(crate::agent_runtime::events::ExecutionStreamEvent::Finished(report)) = stream.next().await else {
                        panic!("the missing agent must produce exactly one terminal preparation failure");
                    };
                    assert!(report.error.as_deref().unwrap().contains("stack-test-missing-agent"));
                    assert_eq!(report.provider_request_count, 0);
                    assert!(stream.next().await.is_none());
                });
                // Exercise the prepared driver too, with no installed transport or network request.
                // 同时执行已准备的驱动，不安装传输运行时或发送网络请求。
                runtime.block_on(async {
                    // Launch reuses the typed in-memory fixture and normal start-event projection.
                    // launch 复用类型化内存夹具及正常启动事件投影。
                    let launch = build_runtime_stream_launch_payload(
                        &build_test_request(), build_test_bootstrap(), None,
                    ).expect("in-memory launch should assemble");
                    // Driver must reach its original terminal error within the same bounded stack.
                    // driver 必须在相同受限栈内到达原有终态错误。
                    let mut driver = Box::pin(crate::agent_runtime::runtime_stream_driver::drive_runtime_stream_launch(
                        crate::provider_config::ProviderConfigurationFile::default(), launch, None,
                    ));
                    assert!(matches!(driver.next().await, Some(crate::agent_runtime::events::ExecutionStreamEvent::Prepared(_))));
                    // Terminal count prevents silent truncation or duplicate final reports.
                    // 终态计数防止静默截断或重复最终报告。
                    let mut terminal_count = 0;
                    while let Some(event) = driver.next().await {
                        if let crate::agent_runtime::events::ExecutionStreamEvent::Finished(report) = event {
                            terminal_count += 1;
                            assert_eq!(report.error.as_deref(), Some("The service has not installed the public AI runtime."));
                            assert_eq!(report.provider_request_count, 0);
                            assert!(driver.next().await.is_none());
                            break;
                        }
                    }
                    assert_eq!(terminal_count, 1);
                });
            })
            .unwrap()
            .join()
            .unwrap();
        println!("{COMPLETED}");
    }

    /// Verifies actual bootstrap exposes dynamic instructions once in User history and keeps both top projections stable.
    /// 验证真实 bootstrap 将动态指令仅投影一次到 User 历史，且两份顶部投影均保持稳定。
    #[test]
    fn bootstrap_keeps_request_agent_instructions_out_of_top_layers() {
        // Use the production composer and bootstrap, rather than a manually correct protocol fixture.
        // 使用生产组合器与 bootstrap，而不是手工构造正确的协议夹具。
        let mut preparation = build_test_preparation();
        preparation.workspace_path = String::new();
        preparation.agent_prompt_snapshot =
            crate::agent_runtime::execution_models::ExecutionAgentPromptSnapshot::fixed_parent_runtime(
                "dynamic-bootstrap-instructions",
            );
        let request = build_test_request();
        let bootstrap = build_runtime_bootstrap_core(&preparation, &request, None)
            .expect("production bootstrap should build");
        let controller = bootstrap.core_controller.config();
        assert_eq!(controller.system_prompt, bootstrap.system_preamble);
        assert_eq!(controller.prompt_layers, bootstrap.prompt_layers);
        assert_eq!(
            controller.prompt_layers.render_fallback_system_prompt(),
            bootstrap.system_preamble
        );
        assert!(
            !bootstrap
                .system_preamble
                .contains("dynamic-bootstrap-instructions")
        );
        assert!(
            bootstrap
                .dynamic_prompt_layers
                .system
                .contains("dynamic-bootstrap-instructions")
        );
        assert_eq!(
            bootstrap
                .core_snapshot
                .messages
                .iter()
                .filter(|message| matches!(
                    &message.payload,
                    CoreConversationPayload::UserText { text, .. }
                        if text.contains("dynamic-bootstrap-instructions")
                            && text.contains("<vulcan_context_update ")
                ))
                .count(),
            1
        );
        // Restored history may end in a user message that has already been sent; additions must follow it.
        // 恢复历史末尾可能是已经发送的用户消息；新增内容必须位于其后。
        let mut restored_request = build_test_request();
        restored_request.conversation_state = crate::agent_runtime::provider_request_bridge_conversation::core_snapshot_to_session_conversation_state(&bootstrap.core_snapshot).unwrap();
        vulcan_context::append_user_text_message(
            &mut restored_request.conversation_state,
            "already-sent-user",
            "previous user",
            Vec::new(),
        )
        .unwrap();
        restored_request.turn_start_mode = Some(ExecutionTurnStartMode::RestoredCheckpoint);
        let restored_prefix = restored_request.conversation_state.messages.clone();
        preparation.agent_prompt_snapshot = None;
        let withdrawn =
            build_runtime_bootstrap_core(&preparation, &restored_request, None).unwrap();
        let withdrawn_state = crate::agent_runtime::provider_request_bridge_conversation::core_snapshot_to_session_conversation_state(&withdrawn.core_snapshot).unwrap();
        assert_eq!(
            serde_json::to_value(&withdrawn_state.messages[..restored_prefix.len()]).unwrap(),
            serde_json::to_value(&restored_prefix).unwrap()
        );
        let ledger =
            vulcan_context::runtime_host::read_runtime_host_ledger(&withdrawn_state).unwrap();
        let original = ledger
            .iter()
            .find(|row| {
                !row.expires_source && row.envelope.body.contains("dynamic-bootstrap-instructions")
            })
            .unwrap();
        assert!(
            ledger
                .iter()
                .any(|row| row.expires_source && row.occurrence_id == original.occurrence_id)
        );
        // Auxiliary Q&A sees only its local preparation and cannot withdraw the original business inventory.
        // 辅助答疑只持有局部准备，不能撤下原业务清单。
        preparation.prompt_purpose =
            crate::agent_runtime::execution_models::ExecutionPromptPurpose::QuestionExecution;
        restored_request.prompt_purpose = preparation.prompt_purpose;
        let auxiliary =
            build_runtime_bootstrap_core(&preparation, &restored_request, None).unwrap();
        let auxiliary_state = crate::agent_runtime::provider_request_bridge_conversation::core_snapshot_to_session_conversation_state(&auxiliary.core_snapshot).unwrap();
        let auxiliary_ledger =
            vulcan_context::runtime_host::read_runtime_host_ledger(&auxiliary_state).unwrap();
        assert!(
            !auxiliary_ledger
                .iter()
                .any(|row| row.expires_source && row.occurrence_id == original.occurrence_id)
        );
    }

    /// Builds one minimal canonical snapshot used to verify controller seeding during launch assembly.
    /// 构建一份最小规范快照，用于验证启动组装期间的控制器播种行为。
    fn build_test_core_snapshot() -> CoreConversationSnapshot {
        CoreConversationSnapshot {
            messages: vec![CoreConversationMessage {
                id: String::from("user-1"),
                payload: CoreConversationPayload::UserText {
                    text: String::from("请分析主入口文件"),
                    attachments: Vec::new(),
                },
            }],
            ..CoreConversationSnapshot::default()
        }
    }

    /// Builds one minimal bootstrap payload used by launch-payload tests.
    /// 构建一份供启动载荷测试使用的最小 bootstrap 载荷。
    fn build_test_bootstrap() -> RuntimeStreamBootstrap {
        let core_snapshot = build_test_core_snapshot();
        let core_controller = VulcanCoreController::new(
            ControllerConfig {
                provider_id: String::from("openai"),
                model_id: String::from("gpt-test"),
                system_prompt: String::from("system"),
                prompt_layers: vulcan_core::types::ProviderPromptLayers::from_system_prompt(
                    "system",
                ),
                temperature: Some(0.2),
                max_turns: 4,
                additional_params: None,
                context_injection: None,
            },
            core_snapshot.clone(),
        )
        .expect("valid core conversation snapshot");

        RuntimeStreamBootstrap {
            preparation: build_test_preparation(),
            active_agent: None,
            public_ai_runtime: None,
            task_runtime: Arc::new(TaskRuntime::new()),
            tools_manager: VulcanToolsManager::default(),
            resolved_tool_definitions: Vec::new(),
            discovery_events: Vec::new(),
            discovery_rows: vec![String::from("mcp-row")],
            discovery_errors: vec![String::from("mcp-error")],
            warnings: vec![String::from("warning")],
            system_preamble: String::from("system preamble"),
            dynamic_prompt_layers: CoreProviderPromptLayers::default(),
            fixed_instruction_layers: CoreProviderPromptLayers::default(),
            locked_baseline: LockedBaselineProjection::empty(),
            core_snapshot,
            core_controller,
            lowering_repair_events: Vec::new(),
        }
    }

    /// Recovery preserves original identity across listener rejection and immediate post-handoff cancellation.
    /// 恢复在监听拒绝及交接后立即取消时保留原身份。
    /// Uses the real launch assembler and durable domain transitions without invoking a provider.
    /// 使用真实启动组装器及可靠领域转换，不调用供应商。
    #[tokio::test]
    async fn recovered_child_launch_preserves_identity_across_listener_and_stop() {
        use crate::agent_profile::AgentProfileCatalog;
        use crate::agent_runtime::runtime::RuntimeFoundation;
        use crate::agent_runtime::task_runtime::restore_child_business_request;
        use vulcan_session_core::v1::child_execution::{
            ChildExecutionProfile, ChildExecutionPromptContext, ChildExecutionRuntimeIdentity,
        };
        use vulcan_session_core::v1::collaboration::{
            CollaborationExecutionId, CollaborationOperationId, CollaborationRunControl,
            SetRunControlInput,
        };
        use vulcan_session_core::v1::document::{ClientInputSubmission, SessionDocument};
        use vulcan_session_core::v1::{ClientActionId, SubagentTaskId};
        use vulcan_session_core::{
            ExecutionRuntimePolicy, ProviderModelSnapshot, SessionEffectiveConfig,
            TaskInvocationConfig, TaskSessionResolvedConfig,
        };
        // Each case owns a fresh runtime; the successful handoff is stopped before this thread yields.
        // 每种情况持有全新运行时；成功交接在本线程让出执行之前被停止。
        for (reject_listener, stop_during_admission) in
            [(true, false), (false, false), (true, true)]
        {
            // The original business identity is deliberately unrelated to mutable display labels.
            // 原业务身份故意与可变展示标签无关。
            let root_id = Uuid::new_v4();
            let mut document =
                SessionDocument::new(Uuid::nil().to_string(), "project", "Edited title", 1);
            let turn = document
                .submit_root_input(
                    ClientInputSubmission::new(
                        ClientActionId::try_new("original-execution").unwrap(),
                        "请分析主入口文件",
                        vec![],
                    ),
                    2,
                )
                .unwrap()
                .into_value();
            let effective = SessionEffectiveConfig {
                provider_model: ProviderModelSnapshot {
                    provider_id: Some("openai".into()),
                    model_id: Some("gpt-test".into()),
                    reasoning_effort: Some("off".into()),
                    execution_mode: Some("build".into()),
                },
                ..SessionEffectiveConfig::default()
            };
            let profile = ChildExecutionProfile {
                version: vulcan_session_core::v1::child_execution::CHILD_EXECUTION_PROFILE_VERSION,
                root_session_id: root_id.to_string(),
                parent_session_id: root_id.to_string(),
                task_id: SubagentTaskId::try_new(Uuid::nil().to_string()).unwrap(),
                segment_generation: 1,
                business_turn_id: turn,
                business_execution_id: "original-execution".into(),
                runtime_identity: ChildExecutionRuntimeIdentity {
                    public_task_id: "original-reviewer".into(),
                    display_title: "Original review".into(),
                    description: Some("Review source".into()),
                    subagent: None,
                    depth: 1,
                },
                invocation_config: TaskInvocationConfig::main("请分析主入口文件"),
                resolved_config: TaskSessionResolvedConfig {
                    requested_config: Default::default(),
                    effective_config: effective.clone(),
                    functional_system_prompt_layers: vec![],
                    runtime_policy: ExecutionRuntimePolicy::task_session(),
                },
                effective_config: effective,
                runtime_policy: ExecutionRuntimePolicy::task_session(),
                execution_agent_id: Some("default-agent".into()),
                connection_id: "test-connection".into(),
                temperature: Some(0.2),
                admitted_tools: Default::default(),
                request_overrides: Default::default(),
                fixed_agent_prompt: Some("Original fixed prompt".into()),
                prompt_context: ChildExecutionPromptContext {
                    identity: "original-reviewer".into(),
                    delegated_task: "请分析主入口文件".into(),
                    delegation: None,
                    functional_system_prompt: None,
                },
            };
            document
                .capture_child_execution_profile(profile.clone(), 3)
                .unwrap();
            document
                .set_collaboration_run_control(SetRunControlInput {
                    operation_id: CollaborationOperationId::try_new("running").unwrap(),
                    state: CollaborationRunControl::Running,
                    changed_at_ms: 4,
                })
                .unwrap();
            let revision = document
                .prepare_flush_candidates()
                .unwrap()
                .unwrap()
                .revision;
            document.advance_durable(revision).unwrap();
            let execution_id = CollaborationExecutionId::try_new("original-execution").unwrap();
            let before = document.clone();
            let mut request = build_test_request();
            request.prompt.clear();
            request.conversation_hook_turn_id = None;
            request.prompt_purpose = crate::agent_runtime::ExecutionPromptPurpose::AgentTask;
            restore_child_business_request(&mut request, &document, &execution_id).unwrap();
            let config = crate::provider_config::ProviderConfigurationFile::default();
            let foundation = Arc::new(RuntimeFoundation::build_from_optional_mcp_access(
                &config,
                &AgentProfileCatalog::default(),
                None,
                None,
            ));
            let runtime = foundation.task_runtime();
            let context =
                TaskExecutionRuntimeContext::new(config, foundation, None, request.clone());
            // Retain the exact preparation for a retry against the same runtime and durable document.
            // 为同一运行时及可靠文档上的重试保留精确准备。
            let retry_context = context.clone();
            let mut bootstrap = build_test_bootstrap();
            bootstrap.task_runtime = runtime.clone();
            bootstrap.preparation.prompt_purpose =
                crate::agent_runtime::ExecutionPromptPurpose::AgentTask;
            let launch =
                build_runtime_stream_launch_payload(&request, bootstrap, Some(context)).unwrap();
            let admitted = std::cell::Cell::new(false);
            let result = runtime.start_recovered_child_business(
                launch,
                &document,
                &execution_id,
                |invocation| {
                    admitted.set(true);
                    assert_eq!(invocation.public_task_id, "original-reviewer");
                    assert_eq!(invocation.execution_id, "original-execution");
                    assert_eq!(invocation.display_title, "Original review");
                    assert_eq!(invocation.invocation_config, profile.invocation_config);
                    assert_eq!(invocation.snapshot.messages.len(), 1);
                    assert!(runtime.is_task_running(invocation.task_id).unwrap());
                    if stop_during_admission {
                        assert!(runtime.stop_task(invocation.task_id).unwrap());
                    }
                    if reject_listener {
                        Err(String::from("test listener rejected before provider"))
                    } else {
                        Ok(())
                    }
                },
            );
            assert!(admitted.get());
            if reject_listener {
                assert!(
                    result
                        .unwrap_err()
                        .contains("test listener rejected before provider")
                );
                assert!(!runtime.is_task_running(Uuid::nil()).unwrap());
                if !stop_during_admission {
                    // Unwinding must release the same admission before an ordinary retry can succeed.
                    // 栈展开必须释放同一准入，之后普通重试才能成功。
                    let mut panic_bootstrap = build_test_bootstrap();
                    panic_bootstrap.task_runtime = runtime.clone();
                    panic_bootstrap.preparation.prompt_purpose =
                        crate::agent_runtime::ExecutionPromptPurpose::AgentTask;
                    let panic_launch = build_runtime_stream_launch_payload(
                        &request,
                        panic_bootstrap,
                        Some(retry_context.clone()),
                    )
                    .unwrap();
                    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        runtime.start_recovered_child_business(
                            panic_launch,
                            &document,
                            &execution_id,
                            |_| panic!("listener unwound before handoff"),
                        )
                    }));
                    assert!(unwound.is_err());
                    assert!(!runtime.is_task_running(Uuid::nil()).unwrap());
                    assert!(
                        runtime
                            .subagent_registry_snapshot()
                            .unwrap()
                            .get_for_parent(root_id.into(), "original-reviewer")
                            .is_none()
                    );
                }
                // Reuse both original identities: generating another task would not prove rollback.
                // 复用两个原身份：生成另一个任务不能证明回滚。
                let mut retry_bootstrap = build_test_bootstrap();
                retry_bootstrap.task_runtime = runtime.clone();
                retry_bootstrap.preparation.prompt_purpose =
                    crate::agent_runtime::ExecutionPromptPurpose::AgentTask;
                let retry_launch = build_runtime_stream_launch_payload(
                    &request,
                    retry_bootstrap,
                    Some(retry_context),
                )
                .unwrap();
                let retry = runtime.start_recovered_child_business(
                    retry_launch,
                    &document,
                    &execution_id,
                    |_| {
                        assert!(
                            !stop_during_admission,
                            "stopped admission must not reach a listener"
                        );
                        Ok(())
                    },
                );
                if stop_during_admission {
                    assert!(
                        retry
                            .unwrap_err()
                            .contains("already has a resident task record")
                    );
                    assert_eq!(
                        runtime
                            .subagent_registry_snapshot()
                            .unwrap()
                            .get_for_parent(root_id.into(), "original-reviewer")
                            .unwrap()
                            .status,
                        vulcan_session_core::SubagentStatus::Stopped,
                    );
                } else {
                    retry.unwrap();
                }
            } else {
                result.unwrap();
            }
            if !stop_during_admission {
                assert!(runtime.is_task_running(Uuid::nil()).unwrap());
                // The current-thread runtime cannot poll provider work before this immediate abort.
                // 当前线程运行时在此立即中止前无法轮询 Provider 工作。
                assert!(runtime.stop_task(Uuid::nil()).unwrap());
                tokio::time::timeout(std::time::Duration::from_secs(3), async {
                    while runtime.is_task_running(Uuid::nil()).unwrap() {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .unwrap();
            }
            assert!(!runtime.is_task_running(Uuid::nil()).unwrap());
            assert_eq!(
                serde_json::to_value((&document.metadata, document.events())).unwrap(),
                serde_json::to_value((&before.metadata, before.events())).unwrap()
            );
        }
    }

    /// Only an explicit restored action selects continuation, leaving new input and normal retry modes unchanged.
    /// 只有明确的恢复动作选择继续模式，新输入和普通重试模式保持不变。
    /// No inputs; verifies the host action identity is not inferred from prompt text or assistant history.
    /// 无输入；验证宿主动作身份不从提示词或助手历史推测。
    #[test]
    fn launch_mode_requires_explicit_restored_continuation_action() {
        // Start from the existing complete request fixture, with no model or service mutation.
        // 从现有完整请求夹具开始，不修改模型或服务。
        let mut request = build_test_request();
        assert_eq!(
            runtime_stream_turn_start_mode_for_request(&request),
            RuntimeStreamTurnStartMode::UserPrompt
        );
        request.turn_start_mode = Some(ExecutionTurnStartMode::RestoredCheckpoint);
        assert_eq!(
            runtime_stream_turn_start_mode_for_request(&request),
            RuntimeStreamTurnStartMode::RestoredCheckpoint
        );
        request.continuation_action_id = Some(
            vulcan_session_core::v1::ClientActionId::try_new("admitted-continue-action")
                .expect("valid host action identity"),
        );
        assert_eq!(
            runtime_stream_turn_start_mode_for_request(&request),
            RuntimeStreamTurnStartMode::ContinuedProviderTurn
        );
        request.turn_start_mode = Some(ExecutionTurnStartMode::UserPrompt);
        assert_eq!(
            runtime_stream_turn_start_mode_for_request(&request),
            RuntimeStreamTurnStartMode::UserPrompt
        );
        request.turn_start_mode = None;
        assert_eq!(
            runtime_stream_turn_start_mode_for_request(&request),
            RuntimeStreamTurnStartMode::UserPrompt
        );
    }

    /// Builds a fresh tracker from `request` through the production preparation-to-telemetry bridge.
    /// 根据 `request` 通过生产准备到遥测桥接链路构建全新跟踪器。
    /// Returns an idle tracker carrying the request's exact Continue and host-restart identities.
    /// 返回空闲跟踪器，其中携带请求的精确 Continue 与宿主重启身份。
    fn continuation_test_tracker(
        request: &ExecutionPreparationRequest,
    ) -> crate::agent_runtime::provider_request_telemetry::ProviderRequestTelemetryTracker {
        // preparation reproduces the authoritative identity copy performed by execution preparation.
        // preparation 复现执行准备中权威身份字段的复制。
        let mut preparation = build_test_preparation();
        preparation.continuation_action_id = request.continuation_action_id.clone();
        preparation.conversation_hook_turn_id = request.conversation_hook_turn_id.clone();
        // context exercises the real template and attempt-seed construction for every replacement stream.
        // context 为每个替代流执行真实模板与尝试种子的构建。
        let context = crate::agent_runtime::provider_request_telemetry::ProviderRequestTelemetryContext::from_preparation(
            &preparation,
            request.provider_request_continuation.as_ref(),
        )
        .expect("host continuation should build an exact telemetry context");
        // tracker starts empty so no prior stream's in-memory attempt state can mask the regression.
        // tracker 从空状态开始，避免旧流的内存尝试状态掩盖回归。
        let mut tracker =
            crate::agent_runtime::provider_request_telemetry::ProviderRequestTelemetryTracker::default();
        tracker.install_context(context);
        tracker
    }

    /// Verifies host handoffs consume Continue once while retaining only the admitted loop's recovery capability.
    /// 验证宿主交接只消费 Continue 一次，同时仅保留已准入循环的恢复能力。
    /// Takes no arguments; returns no value and checks both successful and failed permitted requests.
    /// 无参数、无返回值；同时检查已许可请求的成功与失败情形。
    #[test]
    fn continued_host_restarts_separate_action_consumption_from_tail_recovery() {
        use crate::agent_runtime::provider_v1_bridge::ProviderV1LifecycleFacts;
        use vulcan_session_core::v1::event::ApiPurposeKind;

        // request_failed covers both terminal outcomes after the same durable permission boundary.
        // request_failed 覆盖相同可靠许可边界之后的两种终态。
        for request_failed in [false, true] {
            // action identifies the single user-authorized Continue throughout all replacement streams.
            // action 标识全部替代流中唯一一次用户授权的 Continue。
            let action = vulcan_session_core::v1::ClientActionId::try_new("host-restart-continue")
                .expect("valid Continue action");
            // request is the actual host request mutated by every restart handoff.
            // request 是每次重启交接实际修改的宿主请求。
            let mut request = build_test_request();
            request.continuation_action_id = Some(action.clone());
            request.turn_start_mode = Some(ExecutionTurnStartMode::RestoredCheckpoint);
            // snapshot supplies an offline physical request without provider or credential access.
            // snapshot 提供离线物理请求，不访问供应商或凭据。
            let snapshot = vulcan_core::types::ProviderRequestSnapshot {
                provider_id: String::from("openai"),
                model_id: String::from("gpt-test"),
                system_prompt: String::new(),
                prompt_layers: Default::default(),
                prompt: String::from("continue"),
                prompt_in_history: false,
                history: Vec::new(),
                temperature: None,
                max_turns: 1,
                tool_schemas: Vec::new(),
                additional_params: None,
                request_headers: None,
                continuation: Default::default(),
            };
            // tracker owns the real reserve, permit, terminal, and restart lifecycle.
            // tracker 持有真实的预留、许可、终态与重启生命周期。
            let mut tracker = continuation_test_tracker(&request);
            tracker
                .prepare_request(&snapshot, None)
                .expect("reserve candidate");
            tracker.reject_prepared_request();
            // pending captures a pre-permit restart without consuming the user action or attempt.
            // pending 捕获许可前重启，不消耗用户动作或尝试编号。
            let pending = tracker
                .next_request_continuation_identity()
                .expect("safe pre-permit boundary");
            assert_eq!(pending.pending_continuation_action_id, Some(action.clone()));
            assert!(pending.allows_assistant_tail_restore);
            assert_eq!(pending.next_provider_attempt_index, 0);
            request.replace_retry_replay_with_provider_request_continuation(pending);
            assert_eq!(
                runtime_stream_turn_start_mode_for_request(&request),
                RuntimeStreamTurnStartMode::ContinuedProviderTurn
            );
            tracker = continuation_test_tracker(&request);
            // prepared proves the surviving action still binds the first actual request.
            // prepared 证明保留下来的动作仍绑定首次实际请求。
            let prepared = tracker
                .prepare_request(&snapshot, None)
                .expect("prepare continued request");
            assert!(
                matches!(prepared.v1_lifecycle, Some(ProviderV1LifecycleFacts::RequestPrepared(ref facts))
                if facts.purpose == ApiPurposeKind::Continue && facts.continuation_action_id.as_ref() == Some(&action))
            );
            tracker
                .confirm_prepared_request(&snapshot, None)
                .expect("confirm durable permit");
            assert!(tracker.next_request_continuation_identity().is_err());
            if request_failed {
                assert_eq!(
                    tracker
                        .complete_failure(false, 0, 0, 0, "context_length_exceeded", None, None)
                        .len(),
                    1
                );
            } else {
                tracker
                    .complete_success(
                        None,
                        crate::agent_runtime::provider_v1_bridge::test_provider_v1_source_input(),
                        1,
                    )
                    .expect("complete continued request");
            }

            // Repeat the handoff to prove a replacement stream cannot resurrect a consumed action.
            // 重复交接，证明替代流无法复活已消费的动作。
            for _ in 0..2 {
                // continuation must advance the physical attempt without retaining the old action.
                // continuation 必须推进物理尝试编号，同时不保留旧动作。
                let continuation = tracker
                    .next_request_continuation_identity()
                    .expect("safe completed boundary");
                assert_eq!(continuation.pending_continuation_action_id, None);
                assert!(continuation.allows_assistant_tail_restore);
                assert_eq!(continuation.next_provider_attempt_index, 1);
                assert_eq!(
                    continuation.logical_turn_id,
                    request.conversation_hook_turn_id
                );
                request.replace_retry_replay_with_provider_request_continuation(continuation);
                assert_eq!(request.continuation_action_id, None);
                assert_eq!(
                    runtime_stream_turn_start_mode_for_request(&request),
                    RuntimeStreamTurnStartMode::ContinuedProviderTurn
                );
                tracker = continuation_test_tracker(&request);
                // followup uses the replacement stream's first slot, which previously rebound Continue.
                // followup 使用替代流的首个槽位，旧逻辑会在此重新绑定 Continue。
                let followup = tracker
                    .prepare_request(&snapshot, None)
                    .expect("prepare ordinary followup");
                assert_eq!(followup.payload["attempt_index"], 1);
                assert!(
                    matches!(followup.v1_lifecycle, Some(ProviderV1LifecycleFacts::RequestPrepared(ref facts))
                    if facts.purpose == ApiPurposeKind::MainGeneration && facts.continuation_action_id.is_none())
                );
                tracker.reject_prepared_request();
            }

            // Explicit new input still owns its own boundary even when a prior loop could restore a tail.
            // 即使之前循环可恢复尾部，明确的新输入仍持有自己的边界。
            request.turn_start_mode = Some(ExecutionTurnStartMode::UserPrompt);
            assert_eq!(
                runtime_stream_turn_start_mode_for_request(&request),
                RuntimeStreamTurnStartMode::UserPrompt
            );
            request.turn_start_mode = Some(ExecutionTurnStartMode::RestoredCheckpoint);
            // purpose keeps auxiliary execution strict even if a caller carries the admitted loop state.
            // purpose 保持辅助执行的严格边界，即使调用方携带已准入循环状态。
            for purpose in [
                crate::agent_runtime::execution_models::ExecutionPromptPurpose::QuestionExecution,
                crate::agent_runtime::execution_models::ExecutionPromptPurpose::AgentTask,
            ] {
                request.prompt_purpose = purpose;
                assert_eq!(
                    runtime_stream_turn_start_mode_for_request(&request),
                    RuntimeStreamTurnStartMode::RestoredCheckpoint
                );
            }
        }
    }

    /// Auxiliary questions start from their already-projected canonical history without a user prompt.
    /// 辅助问题从已投影的规范历史启动，不追加用户提示词。
    #[test]
    fn question_launch_mode_restores_canonical_history() {
        let mut request = build_test_request();
        request.prompt.clear();
        request.prompt_purpose =
            crate::agent_runtime::execution_models::ExecutionPromptPurpose::QuestionExecution;
        assert!(request.provider_visible_agent_messages.is_empty());
        assert!(request.turn_start_mode.is_none());
        let launch =
            build_runtime_stream_launch_payload(&request, build_test_bootstrap(), None).unwrap();
        assert_eq!(
            launch.turn_start_mode,
            RuntimeStreamTurnStartMode::RestoredCheckpoint
        );
        assert!(launch.effective_prompt.is_empty());
    }

    /// Claimed agent work restores canonical history even when a caller supplies user-start mode.
    /// 即使调用方传入用户启动模式，已认领代理任务也只恢复规范历史。
    #[test]
    fn agent_task_launch_mode_never_appends_human_prompt() {
        // The durable tagged input is already in history; this adapter must not append it twice.
        // 可靠带标签输入已经在历史中；此适配层不能重复追加它。
        let mut request = build_test_request();
        request.prompt_purpose =
            crate::agent_runtime::execution_models::ExecutionPromptPurpose::AgentTask;
        request.turn_start_mode = Some(ExecutionTurnStartMode::UserPrompt);
        let launch =
            build_runtime_stream_launch_payload(&request, build_test_bootstrap(), None).unwrap();
        assert_eq!(
            launch.turn_start_mode,
            RuntimeStreamTurnStartMode::RestoredCheckpoint
        );
        assert!(request.prompt_purpose.runs_completed_turn_hooks());
        assert!(request.prompt_purpose.supports_user_turn_tooling());
    }

    /// Trims the effective prompt and preserves the bootstrapped controller snapshot inside the launch delta state.
    /// 裁剪生效提示词，并在启动增量状态中保留已 bootstrap 的控制器快照。
    #[test]
    fn launch_payload_assembly_trims_prompt_and_preserves_controller_seed() {
        let request = build_test_request();
        let launch = build_runtime_stream_launch_payload(&request, build_test_bootstrap(), None)
            .expect("valid runtime stream launch payload");

        assert_eq!(launch.session_id, request.session_id);
        assert_eq!(
            launch.turn_start_mode,
            RuntimeStreamTurnStartMode::UserPrompt
        );
        assert_eq!(launch.effective_prompt, "请分析主入口文件");
        assert_eq!(launch.discovery_rows, vec![String::from("mcp-row")]);
        assert_eq!(launch.discovery_errors, vec![String::from("mcp-error")]);
        assert_eq!(launch.warnings, vec![String::from("warning")]);
        assert_eq!(launch.core_snapshot.messages.len(), 1);
        assert_eq!(
            launch.stream_state.controller().snapshot().messages.len(),
            1
        );
        assert!(launch.stream_state.controller().events().is_empty());
    }

    /// Uses seeded agent messages as the controller turn start without creating a user prompt.
    /// 使用已播种智能体消息作为控制器回合起点，不创建用户提示词。
    #[test]
    fn launch_payload_uses_seeded_agent_messages_without_user_prompt() {
        let mut request = build_test_request();
        request.prompt.clear();
        request.provider_visible_agent_messages = vec![ExecutionProviderVisibleAgentMessage {
            delivery_id: String::from("delivery-1"),
            kind: vulcan_ai_contract::HostMessageKind::TaskResult,
            message_type: String::from("FINAL_ANSWER"),
            task_id: String::from("task-1"),
            task_name: String::from("quick_agent"),
            sender: String::from("quick_agent"),
            recipient: String::from("parent:00000000-0000-0000-0000-000000000000"),
            payload: String::from("abc"),
            trigger_turn: true,
            timestamp_ms: 1,
            metadata: None,
        }];
        let preparation = build_test_preparation();
        let runtime_core = build_runtime_bootstrap_core(&preparation, &request, None)
            .expect("runtime bootstrap core should seed agent messages");
        // Dynamic host context may follow the agent message; its typed identity must remain unique and unchanged.
        // 动态宿主上下文可位于代理消息之后；代理的强类型身份必须唯一且保持不变。
        let agent_messages = runtime_core
            .core_snapshot
            .messages
            .iter()
            .filter(|message| {
                matches!(
                    &message.payload,
                    CoreConversationPayload::AgentMessage { .. }
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(agent_messages.len(), 1);
        assert!(
            matches!(&agent_messages[0].payload, CoreConversationPayload::AgentMessage { payload, .. } if payload == "abc")
        );
        assert!(runtime_core.core_snapshot.messages.iter().all(|message| {
            matches!(
                &message.payload,
                CoreConversationPayload::AgentMessage { .. }
            ) || message
                .id
                .starts_with(vulcan_context::runtime_host::RUNTIME_HOST_MESSAGE_PREFIX)
        }));
        let seeded_messages = serde_json::to_value(&runtime_core.core_snapshot.messages).unwrap();
        let crate::agent_runtime::runtime_stream_bootstrap_core::RuntimeBootstrapCore {
            system_preamble,
            dynamic_prompt_layers,
            fixed_instruction_layers,
            locked_baseline,
            core_snapshot,
            core_controller,
            ..
        } = runtime_core;
        let bootstrap = RuntimeStreamBootstrap {
            preparation,
            active_agent: None,
            public_ai_runtime: None,
            task_runtime: Arc::new(TaskRuntime::new()),
            tools_manager: VulcanToolsManager::default(),
            resolved_tool_definitions: Vec::new(),
            discovery_events: Vec::new(),
            discovery_rows: Vec::new(),
            discovery_errors: Vec::new(),
            warnings: Vec::new(),
            system_preamble,
            dynamic_prompt_layers,
            fixed_instruction_layers,
            locked_baseline,
            core_snapshot,
            core_controller,
            lowering_repair_events: Vec::new(),
        };

        let launch = build_runtime_stream_launch_payload(&request, bootstrap, None)
            .expect("valid runtime stream launch payload");

        assert_eq!(
            launch.turn_start_mode,
            RuntimeStreamTurnStartMode::RestoredCheckpoint
        );
        assert!(launch.effective_prompt.contains("<vulcan_agent_message"));
        assert_eq!(
            serde_json::to_value(&launch.stream_state.controller().snapshot().messages).unwrap(),
            seeded_messages
        );
    }

    /// Uses projected latest user text as the controller prompt so attachment bridge text does not duplicate the raw user message.
    /// 使用投影后的最新用户文本作为控制器提示词，避免附件桥接文本重复追加原始用户消息。
    #[test]
    fn launch_payload_uses_projected_latest_user_prompt_for_controller_reuse() {
        let request = build_test_request();
        let mut bootstrap = build_test_bootstrap();
        bootstrap.core_snapshot.messages = vec![CoreConversationMessage {
            id: String::from("user-projected"),
            payload: CoreConversationPayload::UserText {
                text: String::from(
                    "INFO: Attachment \"image.png\" is kept locally.\n\n请分析主入口文件",
                ),
                attachments: Vec::new(),
            },
        }];

        let launch = build_runtime_stream_launch_payload(&request, bootstrap, None)
            .expect("valid runtime stream launch payload");

        assert_eq!(
            launch.effective_prompt,
            "INFO: Attachment \"image.png\" is kept locally.\n\n请分析主入口文件"
        );
    }

    /// A blank request whose turn holds the resource-only note reaches the controller as that note.
    /// 回合含仅资源说明的空白请求以该说明进入控制器。
    #[test]
    fn launch_payload_uses_resource_only_note_for_blank_prompt() {
        let note = vulcan_session_core::v1::projection::RESOURCE_ONLY_INPUT_TEXT;
        for trailing_host_message in [false, true] {
            let mut request = build_test_request();
            request.prompt = String::new();
            let mut bootstrap = build_test_bootstrap();
            bootstrap.core_snapshot.messages = vec![CoreConversationMessage {
                id: String::from("v1-item-0"),
                payload: CoreConversationPayload::UserText {
                    text: String::from(note),
                    attachments: Vec::new(),
                },
            }];
            if trailing_host_message {
                bootstrap
                    .core_snapshot
                    .messages
                    .push(CoreConversationMessage {
                        id: String::from("runtime-host:context"),
                        payload: CoreConversationPayload::UserText {
                            text: String::from("<vulcan_context_update />"),
                            attachments: Vec::new(),
                        },
                    });
            }

            let launch = build_runtime_stream_launch_payload(&request, bootstrap, None)
                .expect("valid runtime stream launch payload");

            assert_eq!(launch.effective_prompt, note);
        }
    }

    /// A blank request without a resource-only turn stays blank so the controller rejects it.
    /// 没有仅资源回合的空白请求保持空白，由控制器拒绝。
    #[test]
    fn launch_payload_keeps_blank_prompt_without_resources() {
        let mut request = build_test_request();
        request.prompt = String::new();
        let mut bootstrap = build_test_bootstrap();
        bootstrap.core_snapshot.messages = vec![CoreConversationMessage {
            id: String::from("v1-item-0"),
            payload: CoreConversationPayload::UserText {
                text: String::new(),
                attachments: Vec::new(),
            },
        }];

        let launch = build_runtime_stream_launch_payload(&request, bootstrap, None)
            .expect("valid runtime stream launch payload");

        assert!(launch.effective_prompt.is_empty());
    }

    /// Falls back to the request prompt when the latest projected user text belongs to history.
    /// 当最新投影用户文本属于历史记录时回退使用请求提示词。
    #[test]
    fn launch_payload_falls_back_when_projected_user_prompt_mismatches_request() {
        let request = build_test_request();
        let mut bootstrap = build_test_bootstrap();
        bootstrap.core_snapshot.messages = vec![CoreConversationMessage {
            id: String::from("historical-user"),
            payload: CoreConversationPayload::UserText {
                text: String::from("历史用户输入"),
                attachments: Vec::new(),
            },
        }];

        let launch = build_runtime_stream_launch_payload(&request, bootstrap, None)
            .expect("valid runtime stream launch payload");

        assert_eq!(launch.effective_prompt, "请分析主入口文件");
    }

    /// Reuses projected text when an unsupported attachment note follows the raw prompt.
    /// 当不支持的附件提示跟随原始提示词时复用投影文本。
    #[test]
    fn launch_payload_reuses_projected_prompt_when_suffix_note_follows_request() {
        let request = build_test_request();
        let mut bootstrap = build_test_bootstrap();
        bootstrap.core_snapshot.messages = vec![CoreConversationMessage {
            id: String::from("user-projected-suffix"),
            payload: CoreConversationPayload::UserText {
                text: String::from(
                    "请分析主入口文件\n\nERROR: Attachment \"image.png\" cannot be uploaded by this API path.",
                ),
                attachments: Vec::new(),
            },
        }];

        let launch = build_runtime_stream_launch_payload(&request, bootstrap, None)
            .expect("valid runtime stream launch payload");

        assert_eq!(
            launch.effective_prompt,
            "请分析主入口文件\n\nERROR: Attachment \"image.png\" cannot be uploaded by this API path."
        );
    }

    /// Reuses projected text when bridge and downgrade notes wrap the raw prompt.
    /// 当桥接与降级提示包裹原始提示词时复用投影文本。
    #[test]
    fn launch_payload_reuses_projected_prompt_when_notes_wrap_request() {
        let request = build_test_request();
        let mut bootstrap = build_test_bootstrap();
        bootstrap.core_snapshot.messages = vec![CoreConversationMessage {
            id: String::from("user-projected-wrapped"),
            payload: CoreConversationPayload::UserText {
                text: String::from(
                    "INFO: Attachment \"image.png\" is kept locally.\n\n请分析主入口文件\n\nERROR: Attachment \"demo.pdf\" cannot be uploaded by this API path.",
                ),
                attachments: Vec::new(),
            },
        }];

        let launch = build_runtime_stream_launch_payload(&request, bootstrap, None)
            .expect("valid runtime stream launch payload");

        assert_eq!(
            launch.effective_prompt,
            "INFO: Attachment \"image.png\" is kept locally.\n\n请分析主入口文件\n\nERROR: Attachment \"demo.pdf\" cannot be uploaded by this API path."
        );
    }

    /// Covers the workbench begin/continue/launch chain so the preseeded user prompt remains the controller prompt.
    /// 覆盖工作台 begin/continue/launch 链路，确保预播种用户提示词仍作为控制器提示词。
    #[test]
    fn launch_payload_preserves_workbench_preseeded_user_prompt_through_shared_turn_flow() {
        let session_id = Uuid::nil();
        let prompt = "请分析主入口文件";
        let attachment = ConversationAttachment {
            kind: ConversationAttachmentKind::Image,
            file_name: String::from("image.png"),
            local_path: String::from(r"D:\workspace\image.png"),
            resource_path: None,
            source_path: None,
            asset_id: None,
            byte_len: Some(1024),
            source_sha256: None,
            delivery_mode: None,
            preview_local_path: None,
            mime_type: String::from("image/png"),
        };
        let mut runtime_state = SessionRuntimeStateEntry::default();
        let mut hook_runtime = ConversationHookRuntime::new();
        let start = begin_shared_prompt_turn_with_runtime_state(
            &mut runtime_state,
            &mut hook_runtime,
            SharedPromptTurnRuntimeBeginInput {
                session_id,
                session_title: "测试会话",
                workspace_path: r"D:\workspace",
                prompt_entry_id: String::from("prompt-entry-1"),
                continuation_action_id: None,
                prompt,
                hidden_model_guidance: None,
                provider_visible_agent_messages: Vec::new(),
                attachments: vec![attachment],
                provider_attachments: None,
                existing_canonical_user_message_id: None,
                append_canonical_user_message: true,
                append_visible_user_prompt_record: true,
                source: PendingSessionPromptSource::UserFollowup,
                vmm_runtime_ready: true,
                vmm_precheck_ready: true,
                execution_mode: String::from("Build"),
                reasoning_effort: None,
                owning_project_id: None,
            },
        )
        .expect("begin shared prompt turn");
        let precheck_outcome = start.precheck_request.clone().skip();
        // profile_outcome keeps profile preparation distinct in this transport-focused test.
        // profile_outcome 在该传输链路测试中保持画像准备的独立性。
        let profile_outcome = start.profile_request.clone().skip();
        // vmm_session_memory models the Core controller candidate for this launch path.
        // vmm_session_memory 模拟该启动链路中的 Core 控制器候选状态。
        let mut vmm_session_memory =
            vulcan_vmm::VmmSessionMemoryState::new(session_id.to_string(), "1")
                .expect("test VMM session owner should be valid");
        let continuation = continue_shared_prompt_turn_after_preparation_with_runtime_state(
            &mut runtime_state,
            &mut vmm_session_memory,
            SharedPromptTurnContinueInput {
                vmm_context_epoch: Some(
                    vulcan_vmm::VmmContextEpoch::new(1, None).expect("fixture context epoch"),
                ),
                vmm_profile_read_source: None,
                start,
                precheck_outcome,
                profile_outcome,
                project_configuration: Default::default(),
                runtime_reasoning_effort: None,
                provider_reasoning_effort: None,
                patch_visible_user_prompt_record: true,
            },
        )
        .expect("continue shared prompt turn");
        let request = continuation.request;
        let latest_request_message = request
            .conversation_state
            .messages
            .last()
            .expect("request should contain preseeded user message");
        assert!(matches!(
            &latest_request_message.payload,
            crate::conversation_protocol::SessionConversationPayload::UserText {
                text,
                attachments,
            } if text == prompt && attachments.len() == 1
        ));

        let mut preparation = build_test_preparation();
        preparation.session_id = session_id;
        preparation.session_title = String::from("测试会话");
        preparation.workspace_path = String::from(r"D:\workspace");
        let runtime_core = build_runtime_bootstrap_core(&preparation, &request, None)
            .expect("runtime bootstrap core should build");
        let crate::agent_runtime::runtime_stream_bootstrap_core::RuntimeBootstrapCore {
            system_preamble,
            prompt_layers: _,
            dynamic_prompt_layers,
            fixed_instruction_layers,
            locked_baseline,
            core_snapshot,
            core_controller,
            lowering_repair_events: _,
        } = runtime_core;
        let latest_projected_message = core_snapshot
            .messages
            .last()
            .expect("projected snapshot should contain latest user message");
        let CoreConversationPayload::UserText {
            text: projected_text,
            attachments: projected_attachments,
        } = &latest_projected_message.payload
        else {
            panic!("latest projected message should be user text");
        };
        assert!(projected_text.contains(prompt));
        assert!(projected_text.contains("ATTACHMENT:"));
        assert_ne!(projected_text, prompt);
        assert!(projected_attachments.is_empty());
        let expected_effective_prompt = projected_text.clone();
        // Only one actual human input may remain; host User envelopes are separate typed records.
        // 只能保留一个真实人类输入；宿主 User 信封属于不同的强类型记录。
        assert_eq!(
            core_snapshot
                .messages
                .iter()
                .filter(|message| matches!(
                    &message.payload,
                    CoreConversationPayload::UserText { .. }
                ) && !message
                    .id
                    .starts_with(vulcan_context::runtime_host::RUNTIME_HOST_MESSAGE_PREFIX))
                .count(),
            1
        );
        assert_eq!(latest_projected_message.id, latest_request_message.id);
        let seeded_messages = serde_json::to_value(&core_snapshot.messages).unwrap();
        let bootstrap = RuntimeStreamBootstrap {
            preparation,
            active_agent: None,
            public_ai_runtime: None,
            task_runtime: Arc::new(TaskRuntime::new()),
            tools_manager: VulcanToolsManager::default(),
            resolved_tool_definitions: Vec::new(),
            discovery_events: Vec::new(),
            discovery_rows: Vec::new(),
            discovery_errors: Vec::new(),
            warnings: Vec::new(),
            system_preamble,
            dynamic_prompt_layers,
            fixed_instruction_layers,
            locked_baseline,
            core_snapshot,
            core_controller,
            lowering_repair_events: Vec::new(),
        };

        let launch = build_runtime_stream_launch_payload(&request, bootstrap, None)
            .expect("valid runtime stream launch payload");

        assert_eq!(launch.effective_prompt, expected_effective_prompt);
        assert_eq!(
            serde_json::to_value(&launch.stream_state.controller().snapshot().messages).unwrap(),
            seeded_messages
        );
    }

    /// Builds one request-dynamic system hook block without baseline provenance.
    /// 构建一个不声明基线来源的请求动态系统 Hook 块。
    fn dynamic_request_hook(content: &str) -> vulcan_hooks::SystemPromptHookBlock {
        vulcan_hooks::SystemPromptHookBlock {
            tag: String::from("vmm_profile"),
            description: String::from("Dynamic profile details"),
            content: content.to_string(),
            source: Some(String::from("adjacent-request-test")),
            phase: vulcan_hooks::SystemPromptHookPhase::FirstSession,
            baseline_provenance: None,
        }
    }

    /// Returns the indexes of projected runtime-host User items whose envelope carries `marker`.
    /// 返回渲染信封中携带 `marker` 的已投影运行时宿主 User 条目下标。
    fn runtime_host_user_item_indexes(
        messages: &[CoreConversationMessage],
        marker: &str,
    ) -> Vec<usize> {
        messages
            .iter()
            .enumerate()
            .filter(|(_, message)| {
                message
                    .id
                    .starts_with(vulcan_context::runtime_host::RUNTIME_HOST_MESSAGE_PREFIX)
                    && matches!(
                        &message.payload,
                        CoreConversationPayload::UserText { text, .. }
                            if text.contains(marker) && text.contains("<vulcan_context_update ")
                    )
            })
            .map(|(index, _)| index)
            .collect()
    }

    /// Verifies two adjacent real requests keep one stable fixed top and only append tagged User items.
    /// 验证相邻两次真实请求保持唯一稳定的固定顶部，且仅追加带标签 User 条目。
    ///
    /// The second request reuses the first request's produced history as its canonical state, so this
    /// compares two real assemblies rather than one encoder fixture. Every dynamic producer body
    /// changes between the pair while the environment producer stays identical.
    /// 第二次请求复用第一次请求产出的历史作为规范状态，因此比较两次真实装配而不是单次编码器夹具。
    /// 两次请求之间每个动态生产者正文都发生变化，环境生产者保持相同。
    #[test]
    fn two_adjacent_requests_keep_fixed_top_and_append_dynamic_user_items() {
        // A workspace-free preparation isolates this test from project-owned prompt sources.
        // 无工作区的执行准备把本测试与项目自有提示来源隔离。
        let mut preparation = build_test_preparation();
        preparation.workspace_path = String::new();
        preparation.agent_prompt_snapshot =
            crate::agent_runtime::execution_models::ExecutionAgentPromptSnapshot::fixed_parent_runtime(
                "dynamic agent instructions V1",
            );
        preparation.session_plan_context = Some(String::from("dynamic plan context V1"));
        preparation.system_prompt_hook_blocks = vec![dynamic_request_hook("dynamic hook body V1")];

        // The first real user input is admitted but not yet sent, so host context precedes it.
        // 第一条真实用户输入已准入但尚未发送，因此宿主上下文位于其前方。
        let mut first_request = build_test_request();
        vulcan_context::append_user_text_message(
            &mut first_request.conversation_state,
            "user-1",
            "first real user input",
            Vec::new(),
        )
        .expect("first real user input should be admitted");

        let first = build_runtime_bootstrap_core(&preparation, &first_request, None)
            .expect("first request should assemble through the production bootstrap");
        let superseded_markers = [
            "dynamic agent instructions V1",
            "dynamic plan context V1",
            "dynamic hook body V1",
        ];
        for marker in superseded_markers {
            assert_eq!(
                runtime_host_user_item_indexes(&first.core_snapshot.messages, marker).len(),
                1,
                "`{marker}` must appear once as a tagged User item"
            );
            assert!(
                !first.system_preamble.contains(marker),
                "the fixed top must not carry `{marker}`"
            );
        }
        // The environment producer is unchanged between both requests and must stay single.
        // 环境生产者在两次请求之间保持不变，必须保持唯一。
        assert_eq!(
            runtime_host_user_item_indexes(
                &first.core_snapshot.messages,
                "Current execution mode: Build"
            )
            .len(),
            1
        );
        // Exactly one real human input is visible; every other item is a typed host record.
        // 恰好一条真实人类输入可见；其余条目都是强类型宿主记录。
        assert_eq!(
            first
                .core_snapshot
                .messages
                .iter()
                .filter(|message| !message
                    .id
                    .starts_with(vulcan_context::runtime_host::RUNTIME_HOST_MESSAGE_PREFIX))
                .count(),
            1
        );

        // The next request reuses that history and changes every dynamic producer body.
        // 下一次请求复用该历史，并改变每个动态生产者正文。
        let mut second_state =
            crate::agent_runtime::provider_request_bridge_conversation::core_snapshot_to_session_conversation_state(
                &first.core_snapshot,
            )
            .expect("produced history should convert back to canonical state");
        vulcan_context::append_user_text_message(
            &mut second_state,
            "user-2",
            "second real user input",
            Vec::new(),
        )
        .expect("second real user input should be admitted");
        let mut second_request = build_test_request();
        second_request.conversation_state = second_state;
        preparation.agent_prompt_snapshot =
            crate::agent_runtime::execution_models::ExecutionAgentPromptSnapshot::fixed_parent_runtime(
                "dynamic agent instructions V2",
            );
        preparation.session_plan_context = Some(String::from("dynamic plan context V2"));
        preparation.system_prompt_hook_blocks = vec![dynamic_request_hook("dynamic hook body V2")];

        let second = build_runtime_bootstrap_core(&preparation, &second_request, None)
            .expect("second request should assemble through the production bootstrap");

        // The fixed top and the locked baseline stay byte-identical across the adjacent pair.
        // 相邻两次请求的固定顶部与锁定基线保持字节一致。
        assert_eq!(second.system_preamble, first.system_preamble);
        assert_eq!(second.locked_baseline, first.locked_baseline);

        // Already emitted messages are never rewritten; every update follows them.
        // 已发出的消息绝不被改写；每个更新都跟随其后。
        let emitted_len = first.core_snapshot.messages.len();
        assert_eq!(
            serde_json::to_value(&second.core_snapshot.messages[..emitted_len]).unwrap(),
            serde_json::to_value(&first.core_snapshot.messages).unwrap(),
            "the already emitted request prefix must stay byte-identical"
        );
        let appended = &second.core_snapshot.messages[emitted_len..];
        let updated_markers = [
            "dynamic agent instructions V2",
            "dynamic plan context V2",
            "dynamic hook body V2",
        ];
        for updated_marker in updated_markers {
            assert_eq!(
                runtime_host_user_item_indexes(appended, updated_marker).len(),
                1,
                "the updated `{updated_marker}` must be appended exactly once"
            );
            assert!(
                !second.system_preamble.contains(updated_marker),
                "the fixed top must not carry `{updated_marker}`"
            );
        }
        // Superseded bodies are retained; only an explicit withdrawal or compaction may invalidate them.
        // 被替代的旧正文被保留；只有显式撤下或压缩才可使其失效。
        for marker in superseded_markers {
            assert_eq!(
                runtime_host_user_item_indexes(&second.core_snapshot.messages, marker).len(),
                1,
                "the superseded `{marker}` body must be retained"
            );
        }
        assert_eq!(
            runtime_host_user_item_indexes(
                &second.core_snapshot.messages,
                "Current execution mode: Build"
            )
            .len(),
            1,
            "the unchanged environment producer must not be duplicated"
        );
        // Every appended item is either one updated dynamic producer or the next real user input.
        // 每条追加条目要么是一条已更新的动态生产者，要么是下一条真实用户输入。
        for message in appended {
            let CoreConversationPayload::UserText { text, .. } = &message.payload else {
                panic!("appended request material must be user-role text");
            };
            let is_dynamic_update = updated_markers.iter().any(|marker| text.contains(marker))
                && text.contains("<vulcan_context_update ");
            assert!(
                is_dynamic_update || text.contains("second real user input"),
                "unexpected appended request material: {text}"
            );
        }
    }

    /// A multi-paragraph request still matches the projected prompt that appends attachment notes.
    /// 多段请求仍与追加了附件说明的投影提示词匹配。
    #[test]
    fn multi_paragraph_request_matches_projected_prompt() {
        let raw = "first paragraph\n\nsecond paragraph";
        assert!(projected_user_prompt_matches_request(raw, raw));
        assert!(projected_user_prompt_matches_request(
            &format!("{raw}\n\n[attachment note]"),
            raw
        ));
        assert!(!projected_user_prompt_matches_request(
            "first paragraph\n\nother text",
            raw
        ));
    }
}
