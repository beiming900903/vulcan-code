//! Runtime stream bootstrap helpers for provider-visible history, tool resolution, and request-snapshot seeding.
//! 面向 Provider 可见历史、工具解析与请求快照播种的运行时流式 bootstrap 辅助模块。

use crate::agent_runtime::config::RuntimeAgentDefinition;
use crate::agent_runtime::events::RuntimeEvent;
use crate::agent_runtime::execution_models::{ExecutionPreparation, ExecutionPreparationRequest};
use crate::agent_runtime::execution_preparation::PreparedToolSnapshot;
use crate::agent_runtime::runtime::RuntimeFoundation;
use crate::agent_runtime::runtime_stream_bootstrap_core::{
    RuntimeBootstrapCore, after_precheck_trace, after_precheck_unix_millis,
    build_runtime_bootstrap_core, seed_runtime_request_snapshot,
};
use crate::agent_runtime::runtime_stream_bootstrap_resources::{
    RuntimeBootstrapResources, resolve_runtime_bootstrap_resources,
};
use crate::agent_runtime::task_runtime::TaskRuntime;
use crate::luaskills_runtime::LuaSkillsRuntimeHandle;
use std::sync::Arc;
use vulcan_context::RuntimeProviderLoweringRepairEvent;
use vulcan_core::controller::VulcanCoreController;
use vulcan_core::types::{
    ConversationSnapshot as CoreConversationSnapshot,
    ProviderPromptLayers as CoreProviderPromptLayers,
};
use vulcan_session_core::v1::context::LockedBaselineProjection;
use vulcan_tools_manager::{ResolvedToolDefinition, VulcanToolsManager};

/// Bootstrap payload that captures the fully prepared runtime turn before provider streaming begins.
/// 在 Provider 流式执行开始前捕获完整运行时回合准备结果的启动载荷。
pub(crate) struct RuntimeStreamBootstrap {
    /// Updated execution preparation after request-snapshot seeding.
    /// 完成请求快照播种之后的更新执行准备对象。
    pub preparation: ExecutionPreparation,
    /// Active agent definition resolved from the runtime foundation for this turn.
    /// 为当前回合从运行时基础架构解析得到的激活代理定义。
    pub active_agent: Option<RuntimeAgentDefinition>,
    /// Shared resumable task runtime reused by the current execution turn.
    /// 被当前执行回合复用的共享可恢复任务运行时。
    pub task_runtime: Arc<TaskRuntime>,
    /// Host-owned unified tool manager used by the transport layer.
    /// 供传输层使用的宿主自有统一工具管理器。
    pub tools_manager: VulcanToolsManager,
    /// Final resolved tool definitions exposed to the provider for this turn.
    /// 当前回合向 Provider 暴露的最终已解析工具定义集合。
    pub resolved_tool_definitions: Vec<ResolvedToolDefinition>,
    /// Discovery events projected from enabled MCP runtime servers.
    /// 从已启用 MCP 运行时服务器投影得到的发现事件集合。
    pub discovery_events: Vec<RuntimeEvent>,
    /// Human-readable MCP discovery rows captured for the final execution report.
    /// 为最终执行报告捕获的人类可读 MCP 发现摘要行集合。
    pub discovery_rows: Vec<String>,
    /// MCP discovery errors captured for the final execution report.
    /// 为最终执行报告捕获的 MCP 发现错误集合。
    pub discovery_errors: Vec<String>,
    /// Mutable warning list accumulated before provider streaming begins.
    /// 在 Provider 流式执行开始前累计得到的可变警告列表。
    pub warnings: Vec<String>,
    /// Effective system preamble sent to the provider transport layer.
    /// 发送给 Provider 传输层的生效系统前导语。
    pub system_preamble: String,
    /// Canonical typed prompt baseline proposed for a fresh context generation.
    /// 为新上下文代际提出的规范强类型提示词基线。
    pub locked_baseline: LockedBaselineProjection,
    /// Request-dynamic prompt layers preserved independently from the locked baseline.
    /// 与锁定基线独立保留的请求动态提示词层。
    pub dynamic_prompt_layers: CoreProviderPromptLayers,
    /// Fixed system and developer instructions carried to the physical request head.
    /// 传递到物理请求头部的固定系统和开发者指令。
    pub fixed_instruction_layers: CoreProviderPromptLayers,
    /// Canonical conversation snapshot handed to `vulcan-core`.
    /// 交给 `vulcan-core` 的规范会话快照。
    pub core_snapshot: CoreConversationSnapshot,
    /// Host-owned controller that tracks streaming deltas and execution events.
    /// 负责跟踪流式增量与执行事件的宿主自有控制器。
    pub core_controller: VulcanCoreController,
    /// Provider lowering repair events detected before streaming starts.
    /// 流式执行开始前检测到的 Provider 转换修复事件。
    pub lowering_repair_events: Vec<RuntimeProviderLoweringRepairEvent>,
    /// Process-local public AI runtime installed by the service.
    /// 由服务安装的进程内公共 AI 运行时。
    pub public_ai_runtime: Option<Arc<vulcan_provider_adapters::PublicAiRuntime>>,
}

/// Builds the full bootstrap payload required before provider streaming starts.
/// 构建 Provider 流式执行开始前所需的完整启动载荷。
pub(crate) async fn bootstrap_runtime_stream(
    foundation: &RuntimeFoundation,
    luaskills_runtime: Option<LuaSkillsRuntimeHandle>,
    request: &ExecutionPreparationRequest,
    mut preparation: ExecutionPreparation,
    tool_snapshot: PreparedToolSnapshot,
) -> Result<RuntimeStreamBootstrap, String> {
    let bootstrap_started_at = after_precheck_unix_millis();
    let resources_started_at = after_precheck_unix_millis();
    let RuntimeBootstrapResources {
        active_agent,
        task_runtime,
        tools_manager,
        resolved_tool_definitions,
        discovery_events,
        discovery_rows,
        discovery_errors,
        mut warnings,
    } = resolve_runtime_bootstrap_resources(
        foundation,
        luaskills_runtime,
        request,
        &preparation,
        tool_snapshot,
    );
    after_precheck_trace(
        "after_precheck_27_bootstrap_resources_ready",
        format!(
            "session={} tools={} mcp_discovery_events={} elapsed_ms={}",
            preparation.session_id,
            resolved_tool_definitions.len(),
            discovery_events.len(),
            after_precheck_unix_millis().saturating_sub(resources_started_at)
        ),
    );
    let core_started_at = after_precheck_unix_millis();
    let RuntimeBootstrapCore {
        system_preamble,
        prompt_layers,
        dynamic_prompt_layers,
        fixed_instruction_layers,
        locked_baseline,
        core_snapshot,
        core_controller,
        lowering_repair_events,
    } = build_runtime_bootstrap_core(&preparation, request, active_agent.as_ref())?;
    after_precheck_trace(
        "after_precheck_28_bootstrap_core_ready",
        format!(
            "session={} history={} lowering_repairs={} elapsed_ms={}",
            preparation.session_id,
            core_snapshot.messages.len(),
            lowering_repair_events.len(),
            after_precheck_unix_millis().saturating_sub(core_started_at)
        ),
    );
    let prompt_purpose = preparation.prompt_purpose;
    let snapshot_seed_started_at = after_precheck_unix_millis();
    seed_runtime_request_snapshot(
        &mut preparation,
        request,
        &system_preamble,
        prompt_layers,
        &resolved_tool_definitions,
        crate::agent_runtime::provider_runtime_context::resolve_runtime_max_turns(
            active_agent.as_ref(),
            prompt_purpose,
        ),
        &mut warnings,
    )
    .await;
    after_precheck_trace(
        "after_precheck_29_request_snapshot_seeded",
        format!(
            "session={} elapsed_ms={} elapsed_ms_since_bootstrap={}",
            preparation.session_id,
            after_precheck_unix_millis().saturating_sub(snapshot_seed_started_at),
            after_precheck_unix_millis().saturating_sub(bootstrap_started_at)
        ),
    );

    Ok(RuntimeStreamBootstrap {
        preparation,
        active_agent,
        task_runtime,
        tools_manager,
        resolved_tool_definitions,
        discovery_events,
        discovery_rows,
        discovery_errors,
        warnings,
        system_preamble,
        dynamic_prompt_layers,
        fixed_instruction_layers,
        locked_baseline,
        core_snapshot,
        core_controller,
        lowering_repair_events,
        public_ai_runtime: foundation.public_ai_runtime(),
    })
}
