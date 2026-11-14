//! Runtime streaming delta processing helpers and mutable loop state.
//! 运行时流式增量处理辅助逻辑与可变循环状态模块。

use crate::agent_runtime::events::{
    ExecutionStreamEvent, ProviderRequestDurabilityCompletion, RuntimeEvent, RuntimeEventKind,
};
use crate::agent_runtime::execution_models::ExecutionProviderRequestContinuation;
use crate::agent_runtime::provider_request_telemetry::{
    ProviderRequestTelemetryContext, ProviderRequestTelemetryEvent,
    ProviderRequestTelemetryEventKind, ProviderRequestTelemetryTracker,
};
use crate::agent_runtime::provider_stream::ProviderStreamDelta;
use crate::agent_runtime::provider_stream_projection::{
    is_provider_tool_boundary_runtime_event, normalize_streamed_thinking_output,
    reset_current_thinking_phase_tracking, runtime_event_to_core_event,
    take_current_thinking_phase_completed_event,
};
use crate::agent_runtime::tool_request_stream::ToolRequestStreamCoordinator;
use crate::conversation_protocol::SessionConversationState;
use crate::model::SessionId;
use std::mem;
use vulcan_ai_contract::{AiErrorCategory, AiExecutionError};
use vulcan_core::controller::VulcanCoreController;
use vulcan_core::types::{
    ProviderRequestSnapshot as CoreProviderRequestSnapshot,
    ProviderUsageSnapshot as CoreProviderUsageSnapshot,
};
use vulcan_tools_manager::ResolvedToolDefinition;

/// Owns the mutable runtime-stream loop state that changes while provider deltas are consumed.
/// 持有在消费 Provider 增量期间持续变化的运行时流式循环状态。
pub(crate) struct RuntimeStreamDeltaState {
    /// Host-owned controller used to persist streamed reasoning, outputs, and runtime events.
    /// 负责持久化流式思考、输出与运行时事件的宿主自有控制器。
    core_controller: VulcanCoreController,
    /// Raw provider reasoning buffer used before synthetic preamble normalization.
    /// 在合成前导语规范化之前累计的 Provider 原始思考缓冲区。
    thinking_raw_output: String,
    /// Visible reasoning buffer after normalization and preamble stripping.
    /// 完成规范化与前导语剥离后的可见思考缓冲区。
    thinking_visible_output: String,
    /// Visible assistant output buffer accumulated across output deltas.
    /// 跨输出增量累计得到的可见助手输出缓冲区。
    assistant_output: String,
    /// Marks whether the current thinking phase has already emitted its completion event.
    /// 标记当前思考阶段是否已经发出过完成事件。
    thinking_completed_emitted: bool,
    /// A provider summary absorbed by normalization before producing visible text; the next
    /// visible thinking delta carries its never-replay marker.
    /// 被规范化吸收、尚未产生可见文本的供应商摘要；下一条可见思考增量携带其绝不重放标记。
    thinking_display_only_pending: bool,
    /// Latest structured request snapshot captured before the most recent provider request.
    /// 最近一次 Provider 请求发出前捕获到的最新结构化请求快照。
    latest_request_snapshot: Option<CoreProviderRequestSnapshot>,
    /// Number of real provider requests observed during the current runtime stream.
    /// 当前运行时流期间观测到的真实 Provider 请求次数。
    provider_request_count: usize,
    /// Final canonical conversation state returned by the provider loop, if it has arrived.
    /// 如果已经收到，则保存 Provider 循环返回的最终规范会话状态。
    final_conversation_state: Option<SessionConversationState>,
    /// Latest successful provider usage emitted by the current provider loop.
    /// 当前 Provider 循环发出的最近一次成功 Provider 用量。
    last_successful_provider_usage: Option<CoreProviderUsageSnapshot>,
    /// Cumulative successful provider usage for the current runtime stream.
    /// 当前运行时流中成功 Provider 调用的累计用量。
    cumulative_successful_provider_usage: Option<CoreProviderUsageSnapshot>,
    /// Latest accumulated usage for the currently active physical request, including a failed partial response.
    /// 当前活动真实请求的最新累计用量，包括失败响应已产生的部分用量。
    active_provider_usage: Option<CoreProviderUsageSnapshot>,
    /// Number of successful assistant provider turns already reflected in the cumulative usage.
    /// 已经计入累计用量的成功助手 Provider 回合数量。
    cumulative_usage_accounted_assistant_turns: usize,
    /// Terminal provider error captured from the stream, if one has occurred.
    /// 如果已经发生，则保存从流中捕获到的终态 Provider 错误。
    terminal_error: Option<String>,
    /// Structured category of the latest physical request when it failed without a retry.
    /// 最近一次真实请求在不再重试地失败时的结构化类别。
    ///
    /// Every new request clears it, so it only describes the request that ended the run.
    /// 每次新请求都会清空它，因此它只描述结束本次运行的那次请求。
    terminal_provider_failure_category: Option<AiErrorCategory>,
    /// Host-owned context compaction request captured at a replay-safe boundary.
    /// 在可回放安全边界捕获的宿主上下文压缩请求。
    context_compaction_required: Option<(
        SessionConversationState,
        String,
        ExecutionProviderRequestContinuation,
    )>,
    /// Host-owned exact-route switch captured at a replay-safe boundary.
    /// 在可回放安全边界捕获的宿主精确路由切换请求。
    provider_route_switch_required: Option<(
        SessionConversationState,
        String,
        ExecutionProviderRequestContinuation,
    )>,
    /// Host-owned context-compaction policy refresh captured at a replay-safe boundary.
    /// 在可回放安全边界捕获的宿主上下文压缩策略刷新请求。
    context_compaction_policy_refresh_required: Option<(
        SessionConversationState,
        String,
        ExecutionProviderRequestContinuation,
    )>,
    /// Low-frequency physical Provider request telemetry state machine.
    /// 低频真实 Provider 请求遥测状态机。
    provider_request_telemetry: ProviderRequestTelemetryTracker,
    /// Whether one retained retry notice is waiting for the next physical-request outcome.
    /// 是否存在一条正在等待下一次真实请求结果的已保留重试提示。
    provider_retry_pending: bool,
    /// Model-side tool request stream coordinator kept separate from real tool execution.
    /// 与真实工具执行分离保存的模型侧工具请求流协调器。
    tool_request_stream: ToolRequestStreamCoordinator,
    /// Whether the provider emitted any model-side tool request during this run.
    /// 当前运行期间 Provider 是否发出过任何模型侧工具请求。
    observed_tool_request: bool,
}

impl RuntimeStreamDeltaState {
    /// Creates a new mutable runtime-stream state from the exact controller.
    /// 使用精确的控制器创建新的可变运行时流式状态。
    ///
    /// # Parameters
    /// 参数
    /// `core_controller` owns canonical conversation projection for the stream.
    /// `core_controller` 持有当前流的规范会话投影。
    ///
    /// # Returns
    /// 返回值
    /// Returns a fresh stream state.
    /// 返回新的流状态。
    pub(crate) fn new(core_controller: VulcanCoreController) -> Self {
        Self {
            core_controller,
            thinking_raw_output: String::new(),
            thinking_visible_output: String::new(),
            assistant_output: String::new(),
            thinking_completed_emitted: false,
            thinking_display_only_pending: false,
            latest_request_snapshot: None,
            provider_request_count: 0,
            final_conversation_state: None,
            last_successful_provider_usage: None,
            cumulative_successful_provider_usage: None,
            active_provider_usage: None,
            cumulative_usage_accounted_assistant_turns: 0,
            terminal_error: None,
            terminal_provider_failure_category: None,
            context_compaction_required: None,
            provider_route_switch_required: None,
            context_compaction_policy_refresh_required: None,
            provider_request_telemetry: ProviderRequestTelemetryTracker::default(),
            provider_retry_pending: false,
            tool_request_stream: ToolRequestStreamCoordinator::default(),
            observed_tool_request: false,
        }
    }

    /// Installs the frozen tool snapshot used by streamed request progress.
    /// 安装流式请求进度使用的冻结工具快照。
    ///
    /// # Parameters
    /// 参数
    /// `resolved_tools` is the frozen tool snapshot used for exact request parser routing.
    /// `resolved_tools` 是用于精确请求解析器路由的冻结工具快照。
    ///
    /// # Returns
    /// 返回值
    /// The updated runtime stream state.
    /// 返回更新后的运行时流状态。
    pub(crate) fn with_tool_request_stream(
        mut self,
        resolved_tools: &[ResolvedToolDefinition],
    ) -> Self {
        self.tool_request_stream = ToolRequestStreamCoordinator::new(resolved_tools);
        self
    }

    /// Installs immutable request telemetry context derived from execution preparation.
    /// 安装从执行准备派生的不可变请求遥测上下文。
    ///
    /// # Parameters
    /// 参数
    /// `context` contains the exact model and owner facts for this stream.
    /// `context` 包含当前流的精确模型与归属事实。
    ///
    /// # Returns
    /// 返回值
    /// Returns the updated state for launch assembly.
    /// 返回用于启动装配的更新状态。
    pub(crate) fn with_provider_request_telemetry_context(
        mut self,
        context: ProviderRequestTelemetryContext,
    ) -> Self {
        self.provider_request_telemetry.install_context(context);
        self
    }

    /// Returns the immutable host-owned controller captured by the loop state.
    /// 返回循环状态持有的不可变宿主控制器。
    pub(crate) fn controller(&self) -> &VulcanCoreController {
        &self.core_controller
    }

    /// Returns the mutable host-owned controller captured by the loop state.
    /// 返回循环状态持有的可变宿主控制器。
    pub(crate) fn controller_mut(&mut self) -> &mut VulcanCoreController {
        &mut self.core_controller
    }

    /// Returns the latest structured request snapshot captured during the stream, if one exists.
    /// 返回流式执行期间最近一次捕获到的结构化请求快照（如果存在）。
    pub(crate) fn latest_request_snapshot(&self) -> Option<&CoreProviderRequestSnapshot> {
        self.latest_request_snapshot.as_ref()
    }

    /// Returns the number of real provider requests captured during the stream so far.
    /// 返回截至当前流式执行期间已经捕获到的真实 Provider 请求次数。
    pub(crate) const fn provider_request_count(&self) -> usize {
        self.provider_request_count
    }

    /// Reports whether this run produced at least one provider tool request.
    /// 返回当前运行是否至少产生过一次 Provider 工具请求。
    pub(crate) const fn observed_tool_request(&self) -> bool {
        self.observed_tool_request
    }

    /// Returns the normalized visible thinking output accumulated so far.
    /// 返回截至当前累计得到的规范化可见思考输出。
    pub(crate) fn thinking_visible_output(&self) -> &str {
        &self.thinking_visible_output
    }

    /// Drains and returns the accumulated assistant output buffer.
    /// 提取并返回累计的助手输出缓冲区。
    pub(crate) fn take_assistant_output(&mut self) -> String {
        mem::take(&mut self.assistant_output)
    }

    /// Drains and returns the final canonical conversation state captured from the stream.
    /// 提取并返回从流中捕获到的最终规范会话状态。
    pub(crate) fn take_final_conversation_state(&mut self) -> Option<SessionConversationState> {
        self.final_conversation_state.take()
    }

    /// Drains and returns the latest successful provider usage captured during this stream.
    /// 提取并返回本次流式执行期间捕获到的最近一次成功 Provider 用量。
    pub(crate) fn take_last_successful_provider_usage(
        &mut self,
    ) -> Option<CoreProviderUsageSnapshot> {
        self.last_successful_provider_usage.take()
    }

    /// Drains and returns cumulative successful provider usage captured during this stream.
    /// 提取并返回本次流式执行期间捕获到的成功 Provider 累计用量。
    pub(crate) fn take_cumulative_successful_provider_usage(
        &mut self,
    ) -> Option<CoreProviderUsageSnapshot> {
        self.cumulative_successful_provider_usage.take()
    }

    /// Drains and returns the terminal stream error, if one has been captured.
    /// 提取并返回已经捕获到的终态流错误（如果存在）。
    pub(crate) fn take_terminal_error(&mut self) -> Option<String> {
        self.terminal_error.take()
    }

    /// Drains the category of the Provider failure that ended the latest physical request.
    /// 提取结束最近一次真实请求的 Provider 失败类别。
    pub(crate) fn take_terminal_provider_failure_category(&mut self) -> Option<AiErrorCategory> {
        self.terminal_provider_failure_category.take()
    }

    /// Records the category of a request failure only when no retry follows it.
    /// 仅在请求失败后不再重试时记录其类别。
    fn record_request_failure(&mut self, will_retry: bool, error: Option<&AiExecutionError>) {
        self.terminal_provider_failure_category = if will_retry {
            None
        } else {
            error.map(|error| error.category)
        };
    }

    /// Returns whether one terminal stream error has already been captured and is waiting for finalization.
    /// 返回是否已经捕获到一条等待收尾处理的终态流错误。
    pub(crate) fn has_terminal_error(&self) -> bool {
        self.terminal_error.is_some()
    }

    /// Drains a pending host-owned context compaction request.
    /// 提取等待中的宿主上下文压缩请求。
    /// Returns the committed state, diagnostic, and next request identity, or None when absent.
    /// 返回已提交状态、诊断与下一请求身份；没有待处理请求时返回 None。
    pub(crate) fn take_context_compaction_required(
        &mut self,
    ) -> Option<(
        SessionConversationState,
        String,
        ExecutionProviderRequestContinuation,
    )> {
        self.context_compaction_required.take()
    }

    /// Drains a pending host-owned exact Provider-route switch request.
    /// 提取等待中的宿主精确 Provider 路由切换请求。
    pub(crate) fn take_provider_route_switch_required(
        &mut self,
    ) -> Option<(
        SessionConversationState,
        String,
        ExecutionProviderRequestContinuation,
    )> {
        self.provider_route_switch_required.take()
    }

    /// Drains a pending host-owned context-compaction policy refresh request.
    /// 提取等待中的宿主上下文压缩策略刷新请求。
    pub(crate) fn take_context_compaction_policy_refresh_required(
        &mut self,
    ) -> Option<(
        SessionConversationState,
        String,
        ExecutionProviderRequestContinuation,
    )> {
        self.context_compaction_policy_refresh_required.take()
    }

    /// Completes the current floating thinking phase exactly once when visible reasoning content exists.
    /// 当存在可见思考内容时，将当前浮动思考阶段恰好一次收口为完成事件。
    pub(crate) fn complete_pending_thinking(
        &mut self,
        session_id: SessionId,
    ) -> Option<ExecutionStreamEvent> {
        take_current_thinking_phase_completed_event(
            session_id,
            &self.thinking_visible_output,
            &mut self.thinking_completed_emitted,
        )
    }

    /// Resets all per-phase thinking trackers after one real tool boundary.
    /// 在一次真实工具边界后重置所有按阶段维护的思考跟踪器。
    pub(crate) fn reset_thinking_phase_tracking(&mut self) {
        reset_current_thinking_phase_tracking(
            &mut self.thinking_raw_output,
            &mut self.thinking_visible_output,
            &mut self.thinking_completed_emitted,
        );
        self.thinking_display_only_pending = false;
    }

    /// Applies one provider thinking delta to controller state and returns one visible event when new normalized text appears.
    /// 将一条 Provider 思考增量应用到控制器状态，并在出现新的规范化可见文本时返回一个可见事件。
    ///
    /// `display_only` marks a provider summary that is recorded but never replayed.
    /// `display_only` 标记记录但绝不重放的供应商摘要。
    fn process_thinking_delta(
        &mut self,
        session_id: SessionId,
        delta: String,
        display_only: bool,
    ) -> Vec<ExecutionStreamEvent> {
        let display_only = display_only || std::mem::take(&mut self.thinking_display_only_pending);
        self.core_controller.record_thinking(delta.clone());
        self.thinking_raw_output.push_str(&delta);
        let normalized_visible_output =
            normalize_streamed_thinking_output(&self.thinking_raw_output);
        let visible_delta = normalized_visible_output
            .strip_prefix(&self.thinking_visible_output)
            .unwrap_or(normalized_visible_output.as_str())
            .to_string();
        self.thinking_visible_output = normalized_visible_output;

        if visible_delta.is_empty() {
            self.thinking_display_only_pending = display_only;
            self.provider_request_telemetry.observe_first_event();
            return Vec::new();
        }
        // events preserves the first-token Hook before the visible delta projection.
        // events 保留首 Token Hook 先于可见增量投影的顺序。
        let mut events = Vec::new();
        if let Some(telemetry_event) = self.provider_request_telemetry.observe_first_token() {
            events.push(ExecutionStreamEvent::ProviderRequestTelemetry(
                telemetry_event,
            ));
        }
        events.push(ExecutionStreamEvent::ThinkingDelta {
            session_id,
            delta: visible_delta,
            display_only,
        });
        events
    }

    /// Applies one provider output delta and emits the pending thinking-completed event before the visible output delta when needed.
    /// 应用一条 Provider 输出增量，并在需要时先发出待收口的思考完成事件，再发出可见输出增量。
    fn process_output_delta(
        &mut self,
        session_id: SessionId,
        delta: String,
    ) -> Vec<ExecutionStreamEvent> {
        // events preserves the first-token Hook before visible output projection.
        // events 保留首 Token Hook 先于可见输出投影的顺序。
        let mut events = Vec::new();
        if !delta.is_empty()
            && let Some(telemetry_event) = self.provider_request_telemetry.observe_first_token()
        {
            events.push(ExecutionStreamEvent::ProviderRequestTelemetry(
                telemetry_event,
            ));
        }
        self.core_controller.record_output(delta.clone());
        if let Some(thinking_completed_event) = self.complete_pending_thinking(session_id) {
            events.push(thinking_completed_event);
        }
        self.assistant_output.push_str(&delta);
        events.push(ExecutionStreamEvent::OutputDelta { session_id, delta });
        events
    }

    /// Applies one runtime event, sealing the current thinking phase on real tool boundaries before forwarding the event.
    /// 应用一条运行时事件，并在真实工具边界上先收口当前思考阶段，再继续转发该事件。
    fn process_runtime_event(
        &mut self,
        session_id: SessionId,
        event: RuntimeEvent,
    ) -> Vec<ExecutionStreamEvent> {
        self.provider_request_telemetry.observe_first_event();
        let mut events = Vec::new();
        if is_provider_tool_boundary_runtime_event(&event) {
            if let Some(thinking_completed_event) = self.complete_pending_thinking(session_id) {
                events.push(thinking_completed_event);
            }
            self.reset_thinking_phase_tracking();
        }
        if let Some(core_event) = runtime_event_to_core_event(&event) {
            self.core_controller.push_event(core_event);
        }
        events.push(ExecutionStreamEvent::RuntimeEvent { session_id, event });
        events
    }

    /// Records one newly completed successful provider turn usage exactly once.
    /// 将一个新完成的成功 Provider 回合用量恰好记录一次。
    ///
    /// # Parameters
    /// 参数
    /// `conversation_state` is the checkpoint state containing successful assistant turns.
    /// `conversation_state` 是包含成功助手回合的检查点状态。
    /// `usage` is the latest successful provider usage reported with that checkpoint.
    /// `usage` 是该检查点携带的最近一次成功 Provider 用量。
    fn record_successful_provider_usage_checkpoint(
        &mut self,
        conversation_state: &SessionConversationState,
        usage: &CoreProviderUsageSnapshot,
    ) {
        let successful_assistant_turns =
            successful_assistant_provider_turn_count(conversation_state);
        if successful_assistant_turns <= self.cumulative_usage_accounted_assistant_turns {
            return;
        }
        let cumulative = self
            .cumulative_successful_provider_usage
            .get_or_insert_with(CoreProviderUsageSnapshot::default);
        cumulative.accumulate_from(usage);
        self.cumulative_usage_accounted_assistant_turns = successful_assistant_turns;
    }
}

/// Counts assistant messages that represent completed provider turns in one checkpoint.
/// 统计一个检查点中代表已完成 Provider 回合的助手消息数量。
///
/// # Parameters
/// 参数
/// `conversation_state` is the canonical conversation checkpoint to inspect.
/// `conversation_state` 是需要检查的规范会话检查点。
///
/// # Returns
/// 返回值
/// The number of assistant messages that carry visible output, reasoning, tools, or usage.
/// 返回携带可见输出、思考、工具调用或用量的助手消息数量。
fn successful_assistant_provider_turn_count(
    conversation_state: &SessionConversationState,
) -> usize {
    conversation_state
        .messages
        .iter()
        .filter(|message| match &message.payload {
            crate::conversation_protocol::SessionConversationPayload::Assistant {
                text,
                reasoning,
                usage,
                tool_calls,
                ..
            } => {
                !text.trim().is_empty()
                    || !reasoning.trim().is_empty()
                    || usage.is_some()
                    || !tool_calls.is_empty()
            }
            crate::conversation_protocol::SessionConversationPayload::UserText { .. }
            | crate::conversation_protocol::SessionConversationPayload::ToolResult { .. }
            | crate::conversation_protocol::SessionConversationPayload::AgentMessage { .. } => {
                false
            }
        })
        .count()
}

/// Processes one provider-stream item, mutates the loop state in place, and returns the execution events that should be emitted immediately.
/// 处理一条 Provider 流项目，就地更新循环状态，并返回应立即发出的执行事件集合。
/// Preserves an already observed response identity when a later usage payload omits it.
/// 当后续用量载荷省略响应标识时保留已经观察到的响应标识。
///
/// # Parameters
/// 参数
/// next is the new cumulative usage snapshot and current is the previously retained snapshot.
/// next 是新的累计用量快照，current 是此前保留的快照。
fn preserve_provider_response_identity(
    next: &mut CoreProviderUsageSnapshot,
    current: Option<&CoreProviderUsageSnapshot>,
) {
    if next.response_id.is_none() {
        next.response_id = current.and_then(|usage| usage.response_id.clone());
    }
}

pub(crate) fn process_provider_stream_delta(
    session_id: SessionId,
    state: &mut RuntimeStreamDeltaState,
    item: Result<ProviderStreamDelta, String>,
) -> Vec<ExecutionStreamEvent> {
    match item {
        Ok(ProviderStreamDelta::RequestPrepared {
            candidate,
            completion,
        }) => {
            let event = match state.provider_request_telemetry.prepare_request(
                &candidate.request_snapshot,
                candidate.context_budget.as_ref(),
            ) {
                Ok(event) => event,
                Err(error) => {
                    state.terminal_error = Some(format!(
                        "Provider request preparation identity failed before the durable permit: {error}"
                    ));
                    let _ = completion.complete(Err(error));
                    return Vec::new();
                }
            };
            vec![ExecutionStreamEvent::ProviderRequestPreparationBoundary {
                event,
                candidate,
                completion,
            }]
        }
        Ok(ProviderStreamDelta::RequestStartDurabilityConfirmed {
            request_snapshot,
            context_budget,
        }) => {
            state.active_provider_usage = None;
            state.terminal_provider_failure_category = None;
            state.latest_request_snapshot = Some(request_snapshot.clone());
            state.provider_request_count = state.provider_request_count.saturating_add(1);
            let event = match state
                .provider_request_telemetry
                .confirm_prepared_request(&request_snapshot, context_budget.as_ref())
            {
                Ok(event) => event,
                Err(error) => {
                    state.terminal_error = Some(format!(
                        "Durable Provider request evidence validation failed: {error}"
                    ));
                    return Vec::new();
                }
            };
            vec![
                ExecutionStreamEvent::ProviderRequestTelemetry(event),
                ExecutionStreamEvent::RequestReady { session_id },
                ExecutionStreamEvent::RequestStarted { session_id },
            ]
        }
        Ok(ProviderStreamDelta::TransportObserved {
            transport,
            dispatched_at_ms,
            headers_at_ms,
            http_status,
            provider_request_id,
            rate_limit_headers,
            completion,
        }) => {
            let telemetry_events = state
                .provider_request_telemetry
                .observe_transport(
                    transport,
                    dispatched_at_ms,
                    headers_at_ms,
                    http_status,
                    provider_request_id,
                    rate_limit_headers,
                )
                .into_iter()
                .collect();
            provider_request_events_with_durability_boundary(telemetry_events, completion)
        }
        Ok(ProviderStreamDelta::Thinking(delta)) => {
            state.process_thinking_delta(session_id, delta, false)
        }
        Ok(ProviderStreamDelta::SummaryThinking(delta)) => {
            state.process_thinking_delta(session_id, delta, true)
        }
        Ok(ProviderStreamDelta::TransientReasoningStatusChanged(status)) => {
            vec![ExecutionStreamEvent::TransientReasoningStatusChanged { session_id, status }]
        }
        Ok(ProviderStreamDelta::Output(delta)) => state.process_output_delta(session_id, delta),
        Ok(ProviderStreamDelta::ToolRequestFragment(fragment)) => {
            state.observed_tool_request = true;
            // Tool-call arguments are generated tokens, so a tool-only reply still records its first token.
            // 工具调用参数也是生成的 Token，因此仅含工具调用的回复同样记录首 Token。
            let mut events = state
                .provider_request_telemetry
                .observe_first_token()
                .map(ExecutionStreamEvent::ProviderRequestTelemetry)
                .into_iter()
                .collect::<Vec<_>>();
            // runtimeEvents are safe low-frequency projections produced without raw arguments.
            // runtimeEvents 是在不包含原始参数的前提下生成的安全低频投影。
            let runtime_events = state.tool_request_stream.apply(fragment);
            events.extend(
                runtime_events
                    .into_iter()
                    .flat_map(|event| state.process_runtime_event(session_id, event)),
            );
            events
        }
        Ok(ProviderStreamDelta::RuntimeEvent(event)) => {
            if event.kind == RuntimeEventKind::Retrying {
                state.provider_retry_pending = true;
            }
            state.process_runtime_event(session_id, event)
        }
        Ok(ProviderStreamDelta::ResponseIdentityObserved(response_id)) => {
            let usage = state
                .active_provider_usage
                .get_or_insert_with(Default::default);
            usage.response_id = Some(response_id);
            Vec::new()
        }
        Ok(ProviderStreamDelta::UsageUpdated(mut usage)) => {
            preserve_provider_response_identity(&mut usage, state.active_provider_usage.as_ref());
            state.active_provider_usage = Some(usage);
            Vec::new()
        }
        Ok(ProviderStreamDelta::RequestSucceeded {
            mut usage,
            provider_source,
            tool_call_count,
            completion,
        }) => {
            if let Some(usage) = usage.as_mut() {
                preserve_provider_response_identity(usage, state.active_provider_usage.as_ref());
            } else if state
                .active_provider_usage
                .as_ref()
                .is_some_and(|current| current.response_id.is_some())
            {
                usage = state.active_provider_usage.clone();
            }
            state.active_provider_usage = usage;
            let events = state
                .provider_request_telemetry
                .complete_success(
                    state.active_provider_usage.as_ref(),
                    provider_source,
                    tool_call_count,
                )
                .into_iter()
                .collect::<Vec<_>>();
            let mut events = provider_request_events_with_durability_boundary(events, completion);
            events.extend(take_provider_retry_status_changed_event(
                state, session_id, true,
            ));
            events
        }
        Ok(ProviderStreamDelta::RequestFailedDurable {
            will_retry,
            retry_number,
            max_retries,
            delay_ms,
            error_category,
            error,
            completion,
        }) => {
            state.record_request_failure(will_retry, error.as_deref());
            let events = state.provider_request_telemetry.complete_failure(
                will_retry,
                retry_number,
                max_retries,
                delay_ms,
                &error_category,
                error.as_deref(),
                state.active_provider_usage.as_ref(),
            );
            let mut events = provider_request_events_with_durability_boundary(events, completion);
            events.extend(take_provider_retry_status_changed_event(
                state, session_id, false,
            ));
            events
        }
        Ok(ProviderStreamDelta::ContextCompactionRequired {
            conversation_state,
            reason,
        }) => {
            // A budget rejection may supersede a candidate before it obtains a durable permit.
            // 预算拒绝可能在候选请求获得可靠许可之前取代它。
            state.provider_request_telemetry.reject_prepared_request();
            // provider_request_continuation preserves the action consumption state across compaction.
            // provider_request_continuation 在压缩前后保留动作消费状态。
            let provider_request_continuation = match state
                .provider_request_telemetry
                .next_request_continuation_identity()
            {
                Ok(identity) => identity,
                Err(error) => {
                    state.terminal_error = Some(format!(
                        "Context compaction continuation identity was unavailable: {error}"
                    ));
                    return Vec::new();
                }
            };
            state.context_compaction_required =
                Some((conversation_state, reason, provider_request_continuation));
            Vec::new()
        }
        Ok(ProviderStreamDelta::ProviderRouteSwitchRequired {
            conversation_state,
            reason,
        }) => {
            // A configuration change can supersede a candidate before its permit; it never consumes an API attempt.
            // 配置变更可以在许可前取代候选请求；尚未派发的候选不会消耗 API 尝试次数。
            state.provider_request_telemetry.reject_prepared_request();
            // providerRequestContinuation is captured from the telemetry owner after the previous physical request closed.
            // providerRequestContinuation 在前一次物理请求收口后从遥测所有者捕获。
            let provider_request_continuation = match state
                .provider_request_telemetry
                .next_request_continuation_identity()
            {
                Ok(identity) => identity,
                Err(error) => {
                    state.terminal_error = Some(format!(
                        "Provider route-switch continuation identity was unavailable: {error}"
                    ));
                    return Vec::new();
                }
            };
            state.provider_route_switch_required =
                Some((conversation_state, reason, provider_request_continuation));
            Vec::new()
        }
        Ok(ProviderStreamDelta::ContextCompactionPolicyRefreshRequired {
            conversation_state,
            reason,
        }) => {
            // providerRequestContinuation is captured after the previous physical request closed.
            // providerRequestContinuation 在前一次真实请求收口后捕获。
            let provider_request_continuation = match state
                .provider_request_telemetry
                .next_request_continuation_identity()
            {
                Ok(identity) => identity,
                Err(error) => {
                    state.terminal_error = Some(format!(
                        "Context compaction policy refresh continuation identity was unavailable: {error}"
                    ));
                    return Vec::new();
                }
            };
            state.context_compaction_policy_refresh_required =
                Some((conversation_state, reason, provider_request_continuation));
            Vec::new()
        }
        Ok(ProviderStreamDelta::FinalConversationState(
            conversation_state,
            last_successful_provider_usage,
        )) => {
            state.final_conversation_state = Some(conversation_state.clone());
            if let Some(ref usage) = last_successful_provider_usage {
                state.last_successful_provider_usage = Some(usage.clone());
                state.record_successful_provider_usage_checkpoint(&conversation_state, usage);
            }
            state.active_provider_usage = None;
            // The adapter-owned RequestSucceeded delta is the sole successful durability boundary;
            // a replayable conversation checkpoint may arrive first and must not consume its terminal telemetry.
            // 适配器持有的 RequestSucceeded 增量是唯一成功持久化边界；
            // 可回放会话检查点可能先到达，绝不能提前消费其终态遥测。
            let mut events = Vec::new();
            events.push(ExecutionStreamEvent::ConversationCheckpoint {
                session_id,
                conversation_state,
                last_successful_provider_usage: state.last_successful_provider_usage.clone(),
                provider_request_count: state.provider_request_count(),
            });
            events.extend(take_provider_retry_status_changed_event(
                state, session_id, true,
            ));
            events
        }
        Err(error) => {
            // telemetryEvents closes a request when an adapter error bypassed the typed failure delta.
            // telemetryEvents 在适配器错误绕过类型化失败增量时关闭请求。
            let mut telemetry_events = state
                .provider_request_telemetry
                .complete_failure(
                    false,
                    0,
                    0,
                    0,
                    "provider_stream_error",
                    None,
                    state.active_provider_usage.as_ref(),
                )
                .into_iter()
                .map(ExecutionStreamEvent::ProviderRequestTelemetry)
                .collect::<Vec<_>>();
            state.terminal_error = Some(error);
            telemetry_events.extend(take_provider_retry_status_changed_event(
                state, session_id, false,
            ));
            telemetry_events
        }
    }
}

/// Builds one display-only terminal update for the latest pending Provider retry notice.
/// 为最近一条待定 Provider 重试提示构建一条纯展示终态更新。
///
/// `state` owns the exact pending-retry marker for this serialized stream.
/// `state` 持有当前串行流的精确待定重试标记。
/// `session_id` identifies the session that owns the serialized Provider request.
/// `session_id` 标识持有该序列化 Provider 请求的会话。
/// `succeeded` is the authoritative physical-request terminal outcome.
/// `succeeded` 是真实请求的权威终态结果。
/// Returns one runtime event that updates an existing retry note when present.
/// 返回一条在重试提示存在时原位更新它的运行时事件。
fn take_provider_retry_status_changed_event(
    state: &mut RuntimeStreamDeltaState,
    session_id: SessionId,
    succeeded: bool,
) -> Option<ExecutionStreamEvent> {
    if !std::mem::take(&mut state.provider_retry_pending) {
        return None;
    }
    let (title, display_state) = if succeeded {
        ("Provider retry succeeded", "succeeded")
    } else {
        ("Provider retry failed", "failed")
    };
    Some(ExecutionStreamEvent::RuntimeEvent {
        session_id,
        event: RuntimeEvent::new(
            RuntimeEventKind::ProviderRetryStatusChanged,
            title,
            String::new(),
        )
        .with_metadata(Some(serde_json::json!({
            "provider_retry_state": display_state,
        }))),
    })
}

/// Converts one started or terminal telemetry event into a durable execution barrier.
/// 将一条开始或终态遥测事件转换为可靠执行屏障。
///
/// # Parameters
/// 参数
/// `telemetry_events` are the ordered events produced by the physical-call tracker.
/// `telemetry_events` 是物理调用跟踪器产生的有序事件。
/// `completion` is released by the service after session-state persistence.
/// `completion` 在服务完成会话状态持久化后释放。
///
/// # Returns
/// 返回值
/// Returns ordered execution events with exactly one durable started or terminal boundary.
/// 返回包含恰好一个可靠开始或终态边界的有序执行事件。
fn provider_request_events_with_durability_boundary(
    telemetry_events: Vec<ProviderRequestTelemetryEvent>,
    completion: ProviderRequestDurabilityCompletion,
) -> Vec<ExecutionStreamEvent> {
    let boundary_index = telemetry_events.iter().rposition(|event| {
        matches!(
            event.kind,
            ProviderRequestTelemetryEventKind::Started
                | ProviderRequestTelemetryEventKind::Dispatched
                | ProviderRequestTelemetryEventKind::Completed
        )
    });
    let Some(boundary_index) = boundary_index else {
        let _ = completion.complete(Err(String::from(
            "Provider durability boundary did not contain a prepared, dispatched, or terminal telemetry event.",
        )));
        return telemetry_events
            .into_iter()
            .map(ExecutionStreamEvent::ProviderRequestTelemetry)
            .collect();
    };
    telemetry_events
        .into_iter()
        .enumerate()
        .map(|(index, event)| {
            if index == boundary_index {
                ExecutionStreamEvent::ProviderRequestDurabilityBoundary {
                    event,
                    completion: completion.clone(),
                }
            } else {
                ExecutionStreamEvent::ProviderRequestTelemetry(event)
            }
        })
        .collect()
}

/// Builds one pre-permit request delta with a valid single-use completion handle for crate-local tests.
/// 为 crate 内测试构建一条带有效一次性完成句柄的许可前请求增量。
///
/// # Parameters
/// 参数
/// `request_snapshot` is the candidate request state observed before V1 reconstruction.
/// `request_snapshot` 是 V1 重建前观测到的候选请求状态。
///
/// # Returns
/// 返回值
/// Returns one immutable preparation candidate that cannot be dispatched directly.
/// 返回一份不可直接分派的不可变准备候选。
#[cfg(test)]
pub(crate) fn test_request_prepared_delta(
    request_snapshot: CoreProviderRequestSnapshot,
) -> ProviderStreamDelta {
    let (completion, _receiver) =
        crate::agent_runtime::events::provider_request_preparation_barrier();
    ProviderStreamDelta::RequestPrepared {
        candidate: crate::agent_runtime::events::ProviderRequestPreparationCandidate {
            media_context: None,
            exact_request: serde_json::from_value(serde_json::json!({
                "operation": "conversation_respond",
                "model_id": request_snapshot.model_id
            }))
            .expect("minimal Provider request candidate must be valid"),
            controller_config: vulcan_core::types::ControllerConfig::default(),
            controller_snapshot: vulcan_core::types::ConversationSnapshot::default(),
            request_snapshot,
            context_budget: None,
            request_budget_guard: None,
            locked_baseline: vulcan_session_core::v1::context::LockedBaselineProjection::empty(),
            dynamic_prompt_layers: vulcan_core::types::ProviderPromptLayers::default(),
            fixed_instruction_layers: Default::default(),
        },
        completion,
    }
}

/// Starts one physical request through the durable preparation and permit boundaries for crate-local tests.
/// 为 crate 内测试通过可靠准备与许可边界开始一次物理请求。
///
/// # Parameters
/// 参数
/// `session_id` identifies the stream, `state` must carry an installed telemetry context, and
/// `request_snapshot` is both the candidate and the permitted request.
/// `session_id` 标识运行时流，`state` 必须已安装遥测上下文，`request_snapshot` 同时作为候选与已许可请求。
///
/// # Returns
/// 返回值
/// Returns the events emitted when Started durability is confirmed.
/// 返回 Started 可靠性得到确认时发出的事件。
#[cfg(test)]
pub(crate) fn test_start_durable_request(
    session_id: SessionId,
    state: &mut RuntimeStreamDeltaState,
    request_snapshot: CoreProviderRequestSnapshot,
) -> Vec<ExecutionStreamEvent> {
    let preparation_events = process_provider_stream_delta(
        session_id,
        state,
        Ok(test_request_prepared_delta(request_snapshot.clone())),
    );
    assert!(
        matches!(
            preparation_events.as_slice(),
            [ExecutionStreamEvent::ProviderRequestPreparationBoundary { .. }]
        ),
        "durable preparation must reserve exactly one request identity"
    );
    process_provider_stream_delta(
        session_id,
        state,
        Ok(ProviderStreamDelta::RequestStartDurabilityConfirmed {
            request_snapshot,
            context_budget: None,
        }),
    )
}

#[cfg(test)]
mod tests {
    //! Runtime-stream delta tests for host-owned provider loop projection behavior.
    //! 面向宿主自有 Provider 循环投影行为的运行时流式增量测试。

    use super::*;
    use crate::agent_runtime::events::RuntimeEventKind;
    use crate::agent_runtime::provider_request_telemetry::ProviderRequestCompletedUsage;
    use crate::agent_runtime::tool_request_stream::ToolRequestFragment;
    use serde_json::{Value, json};
    use uuid::Uuid;
    use vulcan_core::types::{ControllerConfig, ConversationSnapshot};

    /// Builds one minimal controller-backed delta state for unit tests.
    /// 为单元测试构建一份最小控制器支撑的增量状态。
    fn build_delta_state() -> RuntimeStreamDeltaState {
        RuntimeStreamDeltaState::new(
            VulcanCoreController::new(
                ControllerConfig {
                    provider_id: String::from("mock-provider"),
                    model_id: String::from("mock-model"),
                    system_prompt: String::from("system"),
                    prompt_layers: vulcan_core::types::ProviderPromptLayers::from_system_prompt(
                        String::from("system"),
                    ),
                    temperature: Some(0.1),
                    max_turns: 4,
                    additional_params: None,
                    context_injection: None,
                },
                ConversationSnapshot::default(),
            )
            .expect("valid core conversation snapshot"),
        )
    }

    /// Builds one delta state with exact request telemetry identities installed.
    /// 构建一份已安装精确请求遥测身份的增量状态。
    fn build_delta_state_with_telemetry() -> RuntimeStreamDeltaState {
        build_delta_state()
            .with_provider_request_telemetry_context(ProviderRequestTelemetryContext::for_test())
    }

    /// Verifies terminal retry updates are emitted exactly once and only after a retry notice.
    /// 验证重试终态更新仅在重试提示之后发出，并且恰好发出一次。
    #[test]
    fn provider_retry_terminal_status_requires_one_pending_retry_notice() {
        // session_id identifies the single serialized Provider stream under test.
        // session_id 标识当前测试中的单一串行 Provider 流。
        let session_id = Uuid::nil();
        // state owns the authoritative pending-retry marker.
        // state 持有权威的待定重试标记。
        let mut state = build_delta_state();

        assert!(take_provider_retry_status_changed_event(&mut state, session_id, true).is_none());

        let retry_events = process_provider_stream_delta(
            session_id,
            &mut state,
            Ok(ProviderStreamDelta::RuntimeEvent(RuntimeEvent::new(
                RuntimeEventKind::Retrying,
                "Vulcan 公共契约 第 1/5 次重试将在 3 秒后开始",
                "Provider unavailable",
            ))),
        );
        assert!(retry_events.iter().any(|event| matches!(
            event,
            ExecutionStreamEvent::RuntimeEvent { event, .. }
                if event.kind == RuntimeEventKind::Retrying
        )));

        let terminal_event = take_provider_retry_status_changed_event(&mut state, session_id, true)
            .expect("pending retry status update");
        assert!(matches!(
            terminal_event,
            ExecutionStreamEvent::RuntimeEvent { event, .. }
                if event.kind == RuntimeEventKind::ProviderRetryStatusChanged
                    && event.metadata
                        .as_ref()
                        .and_then(|metadata| metadata.get("provider_retry_state"))
                        .and_then(serde_json::Value::as_str)
                        == Some("succeeded")
        ));
        assert!(take_provider_retry_status_changed_event(&mut state, session_id, true).is_none());
    }

    /// Verifies consecutive durable requests retain one logical request identity and advance attempts.
    /// 验证连续可靠请求保留同一逻辑请求身份并推进 attempt。
    #[test]
    fn durable_requests_preserve_logical_identity_and_advance_attempts() {
        // sessionId and state own two sequential physical requests in the same logical execution.
        // sessionId 与 state 持有同一逻辑执行中的两次连续物理请求。
        let session_id = Uuid::nil();
        let mut state = build_delta_state_with_telemetry();
        // firstEvents contains the first Started fact at attempt zero.
        // firstEvents 包含 attempt 为零的首个 Started 事实。
        let first_events =
            test_start_durable_request(session_id, &mut state, build_request_snapshot());
        let first_identity = started_request_identity(&first_events);
        assert_eq!(
            first_identity,
            (json!("execution-1"), json!("turn-1"), json!(0))
        );

        // failure closes the first active request without creating another physical attempt.
        // failure 关闭首个活动请求，且不会创建另一物理 attempt。
        let _ = process_provider_stream_delta(
            session_id,
            &mut state,
            Ok(build_request_failed_delta("test_terminal")),
        );
        // secondEvents must retain the logical identities while advancing only the physical index.
        // secondEvents 必须保留逻辑身份，同时仅推进物理索引。
        let second_events =
            test_start_durable_request(session_id, &mut state, build_request_snapshot());
        let second_identity = started_request_identity(&second_events);
        assert_eq!(
            second_identity,
            (json!("execution-1"), json!("turn-1"), json!(1))
        );
    }

    /// Verifies the durable permit is the only boundary that publishes Started and increments usage accounting.
    /// 验证可靠许可是唯一发布 Started 并递增用量计数的边界。
    #[test]
    fn durable_permit_promotes_prepared_identity_once() {
        let session_id = Uuid::nil();
        let mut state = build_delta_state_with_telemetry();
        let candidate_snapshot = build_request_snapshot();
        let preparation_events = process_provider_stream_delta(
            session_id,
            &mut state,
            Ok(test_request_prepared_delta(candidate_snapshot.clone())),
        );

        assert_eq!(preparation_events.len(), 1);
        assert!(matches!(
            preparation_events.as_slice(),
            [ExecutionStreamEvent::ProviderRequestPreparationBoundary { event, .. }]
                if event.kind == ProviderRequestTelemetryEventKind::Started
        ));
        assert_eq!(state.provider_request_count(), 0);
        assert!(state.latest_request_snapshot().is_none());

        let mut exact_snapshot = candidate_snapshot;
        exact_snapshot.prompt = String::from("durable-ledger-request");
        let confirmed_events = process_provider_stream_delta(
            session_id,
            &mut state,
            Ok(ProviderStreamDelta::RequestStartDurabilityConfirmed {
                request_snapshot: exact_snapshot.clone(),
                context_budget: None,
            }),
        );

        assert_eq!(state.provider_request_count(), 1);
        let latest_snapshot = state
            .latest_request_snapshot()
            .expect("confirmed permit must publish the exact request snapshot");
        assert_eq!(latest_snapshot.provider_id, exact_snapshot.provider_id);
        assert_eq!(latest_snapshot.model_id, exact_snapshot.model_id);
        assert_eq!(latest_snapshot.prompt, exact_snapshot.prompt);
        assert!(matches!(
            confirmed_events.as_slice(),
            [
                ExecutionStreamEvent::ProviderRequestTelemetry(event),
                ExecutionStreamEvent::RequestReady { .. },
                ExecutionStreamEvent::RequestStarted { .. }
            ] if event.kind == ProviderRequestTelemetryEventKind::Started
        ));
    }

    /// Verifies a rejected preparation creates no telemetry terminal and leaves the next attempt reusable.
    /// 验证被拒绝的准备不会创建遥测终态，并保留下一尝试编号可复用。
    #[test]
    fn rejected_preparation_creates_no_started_or_terminal_fact() {
        let session_id = Uuid::nil();
        let mut state = build_delta_state_with_telemetry();
        let snapshot = build_request_snapshot();
        let preparation_events = process_provider_stream_delta(
            session_id,
            &mut state,
            Ok(test_request_prepared_delta(snapshot.clone())),
        );
        assert_eq!(preparation_events.len(), 1);

        let rejected_events = process_provider_stream_delta(
            session_id,
            &mut state,
            Err(String::from("durable permit rejected")),
        );
        assert!(
            rejected_events
                .iter()
                .all(|event| !matches!(event, ExecutionStreamEvent::ProviderRequestTelemetry(_)))
        );
        assert_eq!(state.provider_request_count(), 0);

        let retry_events = process_provider_stream_delta(
            session_id,
            &mut state,
            Ok(test_request_prepared_delta(snapshot)),
        );
        assert!(matches!(
            retry_events.as_slice(),
            [ExecutionStreamEvent::ProviderRequestPreparationBoundary { event, .. }]
                if event.payload["attempt_index"] == 0
        ));
    }

    /// Verifies an accumulated usage update survives a later physical request failure.
    /// 验证累计用量更新会在后续真实请求失败时保留下来。
    #[test]
    fn failed_stream_request_emits_partial_usage_telemetry() {
        let session_id = Uuid::nil();
        let mut state = build_delta_state_with_telemetry();
        test_start_durable_request(session_id, &mut state, build_request_snapshot());
        process_provider_stream_delta(
            session_id,
            &mut state,
            Ok(ProviderStreamDelta::UsageUpdated(
                CoreProviderUsageSnapshot {
                    input_tokens: Some(21),
                    output_tokens: Some(4),
                    total_tokens: Some(25),
                    ..CoreProviderUsageSnapshot::default()
                },
            )),
        );

        let events = process_provider_stream_delta(
            session_id,
            &mut state,
            Ok(build_request_failed_delta("provider_transport_error")),
        );

        let completed_usage = terminal_completed_usage(events);
        assert_eq!(
            completed_usage.and_then(|usage| usage.total_tokens),
            Some(25)
        );
    }

    /// Verifies response.created identity survives an omitted usage identity and a later stream failure.
    /// 验证 response.created 标识在后续用量省略标识并最终流失败时仍被保留。
    #[test]
    fn response_identity_survives_usage_update_and_failure() {
        let session_id = Uuid::nil();
        let mut state = build_delta_state_with_telemetry();
        test_start_durable_request(session_id, &mut state, build_request_snapshot());
        process_provider_stream_delta(
            session_id,
            &mut state,
            Ok(ProviderStreamDelta::ResponseIdentityObserved(String::from(
                "resp-created-1",
            ))),
        );
        process_provider_stream_delta(
            session_id,
            &mut state,
            Ok(ProviderStreamDelta::UsageUpdated(
                CoreProviderUsageSnapshot {
                    input_tokens: Some(7),
                    ..CoreProviderUsageSnapshot::default()
                },
            )),
        );

        let events = process_provider_stream_delta(
            session_id,
            &mut state,
            Ok(build_request_failed_delta("provider_transport_error")),
        );
        let completed_usage = terminal_completed_usage(events);
        assert_eq!(
            completed_usage
                .as_ref()
                .and_then(|usage| usage.response_id.as_deref()),
            Some("resp-created-1")
        );
        assert_eq!(
            completed_usage.and_then(|usage| usage.input_tokens),
            Some(7)
        );
    }

    /// Keeps successful terminal telemetry available when a replayable checkpoint arrives before the durable success delta.
    /// 当可回放检查点先于持久成功增量到达时，保留成功终态遥测供持久化边界使用。
    #[test]
    fn final_state_before_request_succeeded_preserves_terminal_durability_event() {
        let session_id = Uuid::nil();
        let mut state = build_delta_state_with_telemetry();
        test_start_durable_request(session_id, &mut state, build_request_snapshot());

        let checkpoint_events = process_provider_stream_delta(
            session_id,
            &mut state,
            Ok(ProviderStreamDelta::FinalConversationState(
                build_successful_conversation_state(&["压缩摘要"]),
                Some(build_usage_snapshot(200, 20, 5, 225, 100)),
            )),
        );
        assert!(matches!(
            checkpoint_events.as_slice(),
            [ExecutionStreamEvent::ConversationCheckpoint { .. }]
        ));

        let terminal_events = process_provider_stream_delta(
            session_id,
            &mut state,
            Ok(build_request_succeeded_delta(Some(build_usage_snapshot(
                200, 20, 5, 225, 100,
            )))),
        );
        assert!(matches!(
            terminal_events.as_slice(),
            [ExecutionStreamEvent::ProviderRequestDurabilityBoundary { event, .. }]
                if event.kind == ProviderRequestTelemetryEventKind::Completed
        ));
    }

    /// Builds one minimal request snapshot so provider-request counting uses the real event shape.
    /// 构建一份最小请求快照，让 Provider 请求计数使用真实事件形态。
    fn build_request_snapshot() -> CoreProviderRequestSnapshot {
        CoreProviderRequestSnapshot {
            provider_id: String::from("mock-provider"),
            model_id: String::from("mock-model"),
            system_prompt: String::from("system"),
            prompt_layers: vulcan_core::types::ProviderPromptLayers::from_system_prompt(
                String::from("system"),
            ),
            prompt: String::from("prompt"),
            prompt_in_history: false,
            history: Vec::new(),
            temperature: Some(0.1),
            max_turns: 4,
            tool_schemas: Vec::new(),
            additional_params: None,
            request_headers: None,
            continuation: vulcan_core::types::ProviderContinuation::default(),
        }
    }

    /// Builds one durable terminal success delta for the active physical request.
    /// 为当前活动物理请求构建一条可靠成功终态增量。
    fn build_request_succeeded_delta(
        usage: Option<CoreProviderUsageSnapshot>,
    ) -> ProviderStreamDelta {
        let (completion, _receiver) =
            crate::agent_runtime::events::provider_request_durability_barrier();
        ProviderStreamDelta::RequestSucceeded {
            usage,
            provider_source:
                crate::agent_runtime::provider_v1_bridge::test_provider_v1_source_input(),
            tool_call_count: 0,
            completion,
        }
    }

    /// Builds one durable terminal failure delta that schedules no retry.
    /// 构建一条不安排重试的可靠失败终态增量。
    fn build_request_failed_delta(error_category: &str) -> ProviderStreamDelta {
        let (completion, _receiver) =
            crate::agent_runtime::events::provider_request_durability_barrier();
        ProviderStreamDelta::RequestFailedDurable {
            will_retry: false,
            retry_number: 0,
            max_retries: 0,
            delay_ms: 0,
            error_category: String::from(error_category),
            error: None,
            completion,
        }
    }

    /// Extracts the logical execution, logical turn, and physical attempt from one Started fact.
    /// 从一条 Started 事实中提取逻辑执行、逻辑回合与物理 attempt。
    fn started_request_identity(events: &[ExecutionStreamEvent]) -> (Value, Value, Value) {
        let payload = events
            .iter()
            .find_map(|event| match event {
                ExecutionStreamEvent::ProviderRequestTelemetry(event)
                    if event.kind == ProviderRequestTelemetryEventKind::Started =>
                {
                    Some(&event.payload)
                }
                _ => None,
            })
            .expect("a confirmed durable permit must publish one Started fact");
        (
            payload["execution_id"].clone(),
            payload["turn_id"].clone(),
            payload["attempt_index"].clone(),
        )
    }

    /// Returns the completed usage carried by the durable terminal boundary.
    /// 返回可靠终态边界携带的完成用量。
    fn terminal_completed_usage(
        events: Vec<ExecutionStreamEvent>,
    ) -> Option<ProviderRequestCompletedUsage> {
        events.into_iter().find_map(|event| match event {
            ExecutionStreamEvent::ProviderRequestDurabilityBoundary { event, .. }
                if event.kind == ProviderRequestTelemetryEventKind::Completed =>
            {
                event.completed_usage
            }
            _ => None,
        })
    }

    /// Builds one provider usage snapshot for cumulative accounting tests.
    /// 为累计统计测试构建一份 Provider 用量快照。
    fn build_usage_snapshot(
        input_tokens: usize,
        output_tokens: usize,
        reasoning_tokens: usize,
        total_tokens: usize,
        cached_input_tokens: usize,
    ) -> CoreProviderUsageSnapshot {
        CoreProviderUsageSnapshot {
            input_tokens: Some(input_tokens),
            uncached_input_tokens: Some(input_tokens.saturating_sub(cached_input_tokens)),
            output_tokens: Some(output_tokens),
            reasoning_tokens: Some(reasoning_tokens),
            total_tokens: Some(total_tokens),
            cached_input_tokens: Some(cached_input_tokens),
            cache_creation_input_tokens: None,
            ..CoreProviderUsageSnapshot::default()
        }
    }

    /// Builds one canonical conversation checkpoint with visible successful assistant turns.
    /// 构建一份包含可见成功助手回合的规范会话检查点。
    fn build_successful_conversation_state(assistant_texts: &[&str]) -> SessionConversationState {
        SessionConversationState {
            messages: assistant_texts
                .iter()
                .enumerate()
                .map(
                    |(index, text)| crate::conversation_protocol::SessionConversationMessage {
                        id: format!("assistant-{index}"),
                        payload:
                            crate::conversation_protocol::SessionConversationPayload::Assistant {
                                text: (*text).to_string(),
                                reasoning: String::new(),
                                provider_message_id: None,
                                provider_message_state: None,
                                usage: None,
                                tool_calls: Vec::new(),
                            },
                    },
                )
                .collect(),
            records: Vec::new(),
            continuation: Default::default(),
        }
    }

    /// Summary thinking reaches the visible stream with its never-replay marker; model thinking does not.
    /// 摘要思考携带绝不重放标记进入可见流；模型思考不携带该标记。
    #[test]
    fn summary_thinking_carries_display_only_marker() {
        let session_id = Uuid::nil();
        let mut state = build_delta_state();

        let summary_events = process_provider_stream_delta(
            session_id,
            &mut state,
            Ok(ProviderStreamDelta::SummaryThinking(String::from(
                "summary",
            ))),
        );
        assert!(matches!(
            summary_events.as_slice(),
            [ExecutionStreamEvent::ThinkingDelta { delta, display_only: true, .. }]
                if delta == "summary"
        ));
        let thinking_events = process_provider_stream_delta(
            session_id,
            &mut state,
            Ok(ProviderStreamDelta::Thinking(String::from(" model"))),
        );
        assert!(matches!(
            thinking_events.as_slice(),
            [ExecutionStreamEvent::ThinkingDelta {
                display_only: false,
                ..
            }]
        ));
    }

    /// Emits the pending thinking-completed event before the first visible output delta once reasoning has already become visible.
    /// 一旦思考内容已经变为可见，就会在首个正式输出增量之前先发出待收口的思考完成事件。
    #[test]
    fn output_delta_seals_visible_thinking_before_streaming_output() {
        let session_id = Uuid::nil();
        let mut state = build_delta_state();

        let thinking_events = process_provider_stream_delta(
            session_id,
            &mut state,
            Ok(ProviderStreamDelta::Thinking(String::from("实际思考内容"))),
        );
        assert!(matches!(
            thinking_events.as_slice(),
            [ExecutionStreamEvent::ThinkingDelta { delta, .. }] if delta == "实际思考内容"
        ));

        let output_events = process_provider_stream_delta(
            session_id,
            &mut state,
            Ok(ProviderStreamDelta::Output(String::from("正式输出"))),
        );
        assert!(matches!(
            output_events.as_slice(),
            [
                ExecutionStreamEvent::ThinkingCompleted { .. },
                ExecutionStreamEvent::OutputDelta { delta, .. }
            ] if delta == "正式输出"
        ));
    }

    /// Seals and resets the floating thinking phase when one real tool-boundary runtime event arrives.
    /// 当收到真实工具边界运行时事件时，会收口并重置当前浮动思考阶段。
    #[test]
    fn tool_boundary_runtime_event_seals_and_resets_thinking_phase() {
        let session_id = Uuid::nil();
        let mut state = build_delta_state();

        let _ = process_provider_stream_delta(
            session_id,
            &mut state,
            Ok(ProviderStreamDelta::Thinking(String::from("先准备参数"))),
        );

        let runtime_events = process_provider_stream_delta(
            session_id,
            &mut state,
            Ok(ProviderStreamDelta::RuntimeEvent(
                RuntimeEvent::new(
                    RuntimeEventKind::ToolStarted,
                    "Tool call · read_file",
                    "开始调用工具",
                )
                .with_metadata(Some(json!({"tool_call_id":"call-1"}))),
            )),
        );
        assert!(matches!(
            runtime_events.as_slice(),
            [
                ExecutionStreamEvent::ThinkingCompleted { .. },
                ExecutionStreamEvent::RuntimeEvent { event, .. }
            ] if event.title == "Tool call · read_file"
        ));
        assert!(state.thinking_visible_output().is_empty());

        let follow_up_events = process_provider_stream_delta(
            session_id,
            &mut state,
            Ok(ProviderStreamDelta::Thinking(String::from("继续分析"))),
        );
        assert!(matches!(
            follow_up_events.as_slice(),
            [ExecutionStreamEvent::ThinkingDelta { delta, .. }] if delta == "继续分析"
        ));
    }

    /// Verifies request start seals thinking once while later request progress does not create another boundary.
    /// 验证请求开始只收口一次思考，而后续请求进度不会再次创建边界。
    #[test]
    fn tool_request_start_seals_thinking_only_once() {
        // state first receives visible reasoning that must end when the tool identity becomes known.
        // state 首先接收可见思考，并在工具身份明确时结束该思考。
        let session_id = Uuid::nil();
        let mut state = build_delta_state();
        let _ = process_provider_stream_delta(
            session_id,
            &mut state,
            Ok(ProviderStreamDelta::Thinking(String::from(
                "正在组织长参数",
            ))),
        );
        let started = RuntimeEvent::new(
            RuntimeEventKind::ToolRequestStarted,
            "Tool request · pending-1 · submit_plan",
            String::new(),
        );
        let started_events = process_provider_stream_delta(
            session_id,
            &mut state,
            Ok(ProviderStreamDelta::RuntimeEvent(started)),
        );
        assert!(matches!(
            started_events.as_slice(),
            [
                ExecutionStreamEvent::ThinkingCompleted { .. },
                ExecutionStreamEvent::RuntimeEvent { event, .. }
            ] if event.kind == RuntimeEventKind::ToolRequestStarted
        ));
        let progress = RuntimeEvent::new(
            RuntimeEventKind::ToolRequestProgress,
            "Tool request · pending-1 · submit_plan",
            String::new(),
        );
        let progress_events = process_provider_stream_delta(
            session_id,
            &mut state,
            Ok(ProviderStreamDelta::RuntimeEvent(progress)),
        );
        assert!(matches!(
            progress_events.as_slice(),
            [ExecutionStreamEvent::RuntimeEvent { event, .. }]
                if event.kind == RuntimeEventKind::ToolRequestProgress
        ));
    }

    /// Verifies a tool-call-only reply records its first token exactly once.
    /// 验证仅含工具调用的回复恰好记录一次首 Token。
    #[test]
    fn tool_only_reply_records_first_token_once() {
        let session_id = Uuid::nil();
        let mut state = build_delta_state_with_telemetry();
        test_start_durable_request(session_id, &mut state, build_request_snapshot());
        let count_first_tokens = |events: &[ExecutionStreamEvent]| {
            events
                .iter()
                .filter(|event| {
                    matches!(
                        event,
                        ExecutionStreamEvent::ProviderRequestTelemetry(event)
                            if event.kind == ProviderRequestTelemetryEventKind::FirstToken
                    )
                })
                .count()
        };
        let detected_events = process_provider_stream_delta(
            session_id,
            &mut state,
            Ok(ProviderStreamDelta::ToolRequestFragment(
                ToolRequestFragment::Detected {
                    request_stream_id: String::from("request-1"),
                    tool_call_id: String::from("call-1"),
                    tool_name: String::from("submit_plan"),
                    initial_arguments: String::new(),
                    argument_assembler: None,
                },
            )),
        );
        assert_eq!(count_first_tokens(&detected_events), 1);
        let delta_events = process_provider_stream_delta(
            session_id,
            &mut state,
            Ok(ProviderStreamDelta::ToolRequestFragment(
                ToolRequestFragment::ArgumentsDelta {
                    request_stream_id: String::from("request-1"),
                    delta: String::from("{}"),
                },
            )),
        );
        assert_eq!(count_first_tokens(&delta_events), 0);
    }

    /// Accumulates successful provider usage across provider turns without double-counting duplicate checkpoints.
    /// 跨 Provider 回合累计成功用量，同时避免重复检查点造成重复计数。
    #[test]
    fn final_state_accumulates_provider_usage_once_per_successful_assistant_turn() {
        let session_id = Uuid::nil();
        let mut state = build_delta_state_with_telemetry();
        let usage_one = build_usage_snapshot(20_000, 120, 30, 20_150, 19_500);
        let usage_two = build_usage_snapshot(22_000, 160, 40, 22_200, 21_200);

        test_start_durable_request(session_id, &mut state, build_request_snapshot());
        let first_checkpoint = build_successful_conversation_state(&["第一次响应"]);
        let _ = process_provider_stream_delta(
            session_id,
            &mut state,
            Ok(ProviderStreamDelta::FinalConversationState(
                first_checkpoint.clone(),
                Some(usage_one.clone()),
            )),
        );
        let _ = process_provider_stream_delta(
            session_id,
            &mut state,
            Ok(ProviderStreamDelta::FinalConversationState(
                first_checkpoint,
                Some(usage_one.clone()),
            )),
        );
        let _ = process_provider_stream_delta(
            session_id,
            &mut state,
            Ok(build_request_succeeded_delta(Some(usage_one))),
        );

        test_start_durable_request(session_id, &mut state, build_request_snapshot());
        let final_events = process_provider_stream_delta(
            session_id,
            &mut state,
            Ok(ProviderStreamDelta::FinalConversationState(
                build_successful_conversation_state(&["第一次响应", "第二次响应"]),
                Some(usage_two),
            )),
        );
        assert!(matches!(
            final_events.as_slice(),
            [ExecutionStreamEvent::ConversationCheckpoint {
                provider_request_count,
                ..
            }] if *provider_request_count == 2
        ));

        let cumulative_usage = state
            .take_cumulative_successful_provider_usage()
            .expect("successful provider usage should be accumulated");
        assert_eq!(cumulative_usage.input_tokens, Some(42_000));
        assert_eq!(cumulative_usage.output_tokens, Some(280));
        assert_eq!(cumulative_usage.reasoning_tokens, Some(70));
        assert_eq!(cumulative_usage.total_tokens, Some(42_350));
        assert_eq!(cumulative_usage.cached_input_tokens, Some(40_700));
    }

    /// Captures terminal errors while final conversation states also emit a safe workbench checkpoint.
    /// 捕获终态错误，同时让最终会话状态额外发出安全工作台检查点。
    #[test]
    fn terminal_error_is_buffered_and_final_state_emits_checkpoint() {
        let session_id = Uuid::nil();
        let mut state = build_delta_state_with_telemetry();

        test_start_durable_request(session_id, &mut state, build_request_snapshot());
        let final_state_events = process_provider_stream_delta(
            session_id,
            &mut state,
            Ok(ProviderStreamDelta::FinalConversationState(
                SessionConversationState::default(),
                None,
            )),
        );
        assert!(matches!(
            final_state_events.as_slice(),
            [ExecutionStreamEvent::ConversationCheckpoint {
                provider_request_count,
                ..
            }] if *provider_request_count == 1
        ));
        assert!(state.take_final_conversation_state().is_some());

        let error_events = process_provider_stream_delta(
            session_id,
            &mut state,
            Err(String::from("provider failed")),
        );
        // The adapter error closes the still-active request exactly once as a failed terminal.
        // 适配器错误将仍处于活动状态的请求恰好一次收口为失败终态。
        assert!(matches!(
            error_events.as_slice(),
            [ExecutionStreamEvent::ProviderRequestTelemetry(event)]
                if event.kind == ProviderRequestTelemetryEventKind::Completed
                    && event.payload["status"] == "failed"
        ));
        assert_eq!(
            state.take_terminal_error().as_deref(),
            Some("provider failed")
        );
    }

    /// Checks the Continue identity carried by one host restart before and after durable permission.
    /// 检查一种宿主重启在可靠许可前后携带的 Continue 身份。
    /// `restart_delta` creates the control event, `take_pending` drains its state, and `prepare_candidate`
    /// models a pre-permit candidate; returns no value and asserts exact action and attempt retention.
    /// `restart_delta` 创建控制事件，`take_pending` 提取对应状态，`prepare_candidate` 模拟许可前候选；
    /// 无返回值，并断言动作与尝试编号被精确保留。
    fn assert_continue_at_host_restart(
        restart_delta: fn() -> ProviderStreamDelta,
        take_pending: fn(
            &mut RuntimeStreamDeltaState,
        ) -> Option<(
            SessionConversationState,
            String,
            ExecutionProviderRequestContinuation,
        )>,
        prepare_candidate: bool,
    ) {
        // confirmed distinguishes an unconsumed action from one already bound to a completed API.
        // confirmed 区分尚未消费的动作与已经绑定已完成 API 的动作。
        for confirmed in [false, true] {
            // action is the user-authorized Continue that must never bind two physical requests.
            // action 是用户授权的 Continue，绝不能绑定两次物理请求。
            let action =
                vulcan_session_core::v1::ClientActionId::try_new("restart-boundary-continue")
                    .expect("valid Continue action");
            // context freezes action admission and recovery capability before telemetry starts.
            // context 在遥测启动前冻结动作准入信息与恢复能力。
            let context = ProviderRequestTelemetryContext::for_continued_test(action.clone());
            // state consumes the same provider deltas as the production stream driver.
            // state 消费与生产流驱动相同的 Provider 增量。
            let mut state = build_delta_state().with_provider_request_telemetry_context(context);
            if confirmed {
                test_start_durable_request(Uuid::nil(), &mut state, build_request_snapshot());
                process_provider_stream_delta(
                    Uuid::nil(),
                    &mut state,
                    Ok(build_request_succeeded_delta(None)),
                );
            }
            if prepare_candidate {
                state
                    .provider_request_telemetry
                    .prepare_request(&build_request_snapshot(), None)
                    .expect("reserve a candidate before the restart");
            }
            // events must contain no spurious failure for the interrupted pre-permit candidate.
            // events 不得为被中断的许可前候选包含虚假失败。
            let events =
                process_provider_stream_delta(Uuid::nil(), &mut state, Ok(restart_delta()));
            assert!(events.is_empty());
            assert!(!state.has_terminal_error());
            // continuation is the authoritative identity forwarded to the replacement stream.
            // continuation 是转交给替代流的权威身份。
            let (_, _, continuation) =
                take_pending(&mut state).expect("host restart remains pending");
            assert_eq!(continuation.logical_request_id, "execution-1");
            assert!(continuation.allows_assistant_tail_restore);
            assert_eq!(
                continuation.next_provider_attempt_index,
                u32::from(confirmed)
            );
            assert_eq!(
                continuation.pending_continuation_action_id,
                if confirmed { None } else { Some(action) }
            );
        }
    }

    /// Verifies compaction forwards only an unconsumed Continue action, including pre-permit rejection.
    /// 验证压缩只转交尚未消费的 Continue 动作，并覆盖许可前拒绝。
    /// Takes no arguments and returns no value; checks both sides of the durable permission boundary.
    /// 无参数、无返回值；检查可靠许可边界的前后两种状态。
    #[test]
    fn continued_context_compaction_preserves_action_consumption() {
        assert_continue_at_host_restart(
            || ProviderStreamDelta::ContextCompactionRequired {
                conversation_state: SessionConversationState::default(),
                reason: String::from("context budget exceeded"),
            },
            RuntimeStreamDeltaState::take_context_compaction_required,
            true,
        );
    }

    /// Verifies route changes forward only an unconsumed Continue action before replacing the stream.
    /// 验证路由变更在替换流前只转交尚未消费的 Continue 动作。
    /// Takes no arguments and returns no value; checks both sides of the durable permission boundary.
    /// 无参数、无返回值；检查可靠许可边界的前后两种状态。
    #[test]
    fn continued_route_switch_preserves_action_consumption() {
        assert_continue_at_host_restart(
            || ProviderStreamDelta::ProviderRouteSwitchRequired {
                conversation_state: SessionConversationState::default(),
                reason: String::from("exact route changed"),
            },
            RuntimeStreamDeltaState::take_provider_route_switch_required,
            true,
        );
    }

    /// Verifies policy refreshes forward only an unconsumed Continue action at a safe tool boundary.
    /// 验证策略刷新在安全工具边界只转交尚未消费的 Continue 动作。
    /// Takes no arguments and returns no value; checks both sides of the durable permission boundary.
    /// 无参数、无返回值；检查可靠许可边界的前后两种状态。
    #[test]
    fn continued_policy_refresh_preserves_action_consumption() {
        assert_continue_at_host_restart(
            || ProviderStreamDelta::ContextCompactionPolicyRefreshRequired {
                conversation_state: SessionConversationState::default(),
                reason: String::from("compaction policy changed"),
            },
            RuntimeStreamDeltaState::take_context_compaction_policy_refresh_required,
            false,
        );
    }

    /// Verifies compaction cannot restart a stream while its durable physical request is still active.
    /// 验证可靠物理请求仍活动时，压缩不能重启流。
    /// Takes no arguments and returns no value; an unsafe boundary must remain a terminal error.
    /// 无参数、无返回值；不安全边界必须保持为终态错误。
    #[test]
    fn context_compaction_rejects_active_request_boundary() {
        // state retains an active API rather than a superseded preparation candidate.
        // state 保留活动 API，而非已被取代的准备候选。
        let mut state = build_delta_state_with_telemetry();
        test_start_durable_request(Uuid::nil(), &mut state, build_request_snapshot());
        process_provider_stream_delta(
            Uuid::nil(),
            &mut state,
            Ok(ProviderStreamDelta::ContextCompactionRequired {
                conversation_state: SessionConversationState::default(),
                reason: String::from("context budget exceeded"),
            }),
        );
        assert!(state.has_terminal_error());
        assert!(state.take_context_compaction_required().is_none());
    }

    /// Verifies a superseded first candidate keeps attempt zero and never becomes a terminal failure.
    /// 验证被新选择取代的首次候选保持尝试编号零，不会成为终止失败。
    #[test]
    fn provider_route_switch_before_first_permit_preserves_attempt_identity() {
        // Only preparation has happened; no durable API or network dispatch exists.
        // 此时仅完成候选准备，尚无可靠 API 或网络派发。
        let mut state = build_delta_state_with_telemetry();
        state
            .provider_request_telemetry
            .prepare_request(&build_request_snapshot(), None)
            .unwrap();
        let events = process_provider_stream_delta(
            Uuid::nil(),
            &mut state,
            Ok(ProviderStreamDelta::ProviderRouteSwitchRequired {
                conversation_state: SessionConversationState::default(),
                reason: String::from("request configuration changed"),
            }),
        );
        assert!(events.is_empty());
        assert!(!state.has_terminal_error());
        assert_eq!(state.provider_request_count, 0);
        let (_, _, continuation) = state.take_provider_route_switch_required().unwrap();
        assert_eq!(continuation.logical_request_id, "execution-1");
        assert_eq!(continuation.next_provider_attempt_index, 0);
    }

    /// Verifies the exact-route switch delta is retained as host control state rather than a stream failure.
    /// 验证精确路由切换增量会保留为宿主控制状态，而不是流失败。
    #[test]
    fn provider_route_switch_delta_is_buffered_without_terminal_error() {
        // sessionId and conversationState identify the replay-safe boundary under test.
        // sessionId 与 conversationState 标识本测试使用的可回放安全边界。
        let session_id = Uuid::nil();
        let mut state = build_delta_state_with_telemetry();
        let conversation_state = build_successful_conversation_state(&["tool result retained"]);
        // requestEvents advance the same telemetry tracker used by production before the route switch is requested.
        // requestEvents 在请求路由切换前推进生产环境使用的同一个遥测跟踪器。
        let request_events =
            test_start_durable_request(session_id, &mut state, build_request_snapshot());
        assert!(matches!(
            request_events.first(),
            Some(ExecutionStreamEvent::ProviderRequestTelemetry(event))
                if event.kind == ProviderRequestTelemetryEventKind::Started
        ));
        let terminal_events = process_provider_stream_delta(
            session_id,
            &mut state,
            Ok(build_request_succeeded_delta(None)),
        );
        assert!(matches!(
            terminal_events.as_slice(),
            [ExecutionStreamEvent::ProviderRequestDurabilityBoundary { event, .. }]
                if event.kind == ProviderRequestTelemetryEventKind::Completed
        ));

        // The next candidate may already be waiting at the permit when the user switches models.
        // 用户切换模型时，下一候选可能已经在等待许可。
        state
            .provider_request_telemetry
            .prepare_request(&build_request_snapshot(), None)
            .unwrap();
        let events = process_provider_stream_delta(
            session_id,
            &mut state,
            Ok(ProviderStreamDelta::ProviderRouteSwitchRequired {
                conversation_state: conversation_state.clone(),
                reason: String::from("exact session route changed"),
            }),
        );

        assert!(events.is_empty());
        assert!(!state.has_terminal_error());
        let (retained_state, retained_reason, retained_continuation) = state
            .take_provider_route_switch_required()
            .expect("route switch should remain pending for the host driver");
        assert_eq!(
            retained_state.messages.len(),
            conversation_state.messages.len()
        );
        assert_eq!(
            retained_state.records.len(),
            conversation_state.records.len()
        );
        assert_eq!(retained_reason, "exact session route changed");
        assert_eq!(retained_continuation.logical_request_id, "execution-1");
        assert_eq!(
            retained_continuation.logical_turn_id.as_deref(),
            Some("turn-1")
        );
        assert_eq!(retained_continuation.next_provider_attempt_index, 1);
    }
}
