//! Host-controlled provider streaming runtime wrapper for shared controller execution.
//! 面向共享控制器执行的宿主控制 Provider 流式运行时包装模块。

use crate::agent_runtime::events::provider_request_durability_barrier;
use crate::agent_runtime::provider_protocol_tool_definition::ProviderToolDefinition;
use crate::agent_runtime::provider_request_bridge_conversation::core_snapshot_to_session_conversation_state;
use crate::agent_runtime::provider_stream::{ProviderDeltaStream, ProviderStreamDelta};
use crate::agent_runtime::provider_transport_entry_inputs::{
    PreparedProviderTransportEntryInputs, PreparedProviderTransportRuntimeInputs,
};
use crate::agent_runtime::provider_transport_live_delta::ProviderLiveDeltaSender;
use crate::agent_runtime::provider_transport_request::{
    ProviderRouteIdentity, ProviderTransportTurnStartMode,
};
use crate::agent_runtime::provider_transport_request_budget::{
    BudgetCheckedStreamingAdapter, ProviderBudgetRequest, ProviderRequestBudgetGuard,
};
use crate::agent_runtime::provider_transport_retry::{
    ProviderRetryDecision, ProviderRetryPolicy, provider_delta_makes_retry_unsafe,
};
use crate::agent_runtime::provider_transport_stream_executor::StreamingRuntimeToolExecutor;
use crate::agent_runtime::provider_transport_tool_context::PreparedHostToolRuntimeContext;
use crate::agent_runtime::provider_transport_vulcan::OAUTH_CREDENTIAL_REFRESHED_SUBCATEGORY;
use crate::agent_runtime::runtime_stream_intervention::RuntimePromptIntervention;
use crate::agent_runtime::task_runtime::{TaskRuntime, build_task_execution_handler};
use crate::agent_runtime::tool_request_stream::{ToolRequestAbortReason, ToolRequestFragment};
use crate::agent_runtime::tool_runtime_bridge::VulcanManagedToolRuntimeBridge;
use crate::model::SessionId;
use async_stream::stream;
use serde_json::json;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::unbounded_channel;
use tokio::task::AbortHandle;
use vulcan_ai_contract::AiExecutionError;
use vulcan_core::controller::VulcanCoreController;
use vulcan_core::provider::VulcanStreamingProviderAdapter;
use vulcan_core::tool::AllowAllToolPolicy;
use vulcan_core::types::{
    ControllerConfig as CoreControllerConfig, ConversationSnapshot as CoreConversationSnapshot,
    CoreError, ProviderTurnResponse as CoreProviderTurnResponse,
};

/// Emits one failed physical-call boundary only while a Provider request is still active.
/// 仅在 Provider 请求仍处于活动状态时发出失败物理调用边界。
///
/// # Parameters
/// 参数
/// `physical_request_active` is the lifecycle state observed from typed Provider deltas; all remaining
/// fields are exact retry and failure facts already produced by the host retry state machine.
/// `physical_request_active` 是从强类型 Provider 增量观测到的生命周期状态；其余字段均为
/// 宿主重试状态机已经产生的精确重试与失败事实。
///
/// # Returns
/// 返回值
/// Success immediately when no physical request is active, or after durable terminal persistence.
/// 没有活动物理请求时立即成功；否则仅在终态记录可靠持久化后成功。
#[allow(clippy::too_many_arguments)]
async fn send_durable_request_failure(
    sender: &ProviderLiveDeltaSender,
    physical_request_active: bool,
    will_retry: bool,
    retry_number: u32,
    max_retries: u32,
    delay_ms: u64,
    error_category: String,
    error: Option<Box<AiExecutionError>>,
) -> Result<(), String> {
    if !physical_request_active {
        return Ok(());
    }
    let (completion, receiver) = provider_request_durability_barrier();
    sender
        .send(Ok(ProviderStreamDelta::RequestFailedDurable {
            will_retry,
            retry_number,
            max_retries,
            delay_ms,
            error_category,
            error,
            completion,
        }))
        .map_err(|_| String::from("Provider stream closed before terminal persistence."))?;
    receiver
        .await
        .map_err(|_| String::from("Host dropped the Provider terminal persistence barrier."))?
}

/// Prepared adapter launch inputs handed to one concrete Provider adapter constructor after the shared runtime wrapper consumes controller-only fields.
/// 在共享运行时包装器消费控制器专用字段后，交给具体 Provider 适配器构造过程的已准备输入对象。
#[derive(Clone)]
pub(crate) struct PreparedProviderTransportAdapterInputs {
    /// Mirrored provider-visible tool definitions reused by the concrete Provider adapter.
    /// 被具体 Provider 适配器复用的镜像 Provider 可见工具定义集合。
    pub(crate) provider_tool_definitions: Vec<ProviderToolDefinition>,
}

/// Aborts one spawned Provider controller task when its owning output stream disappears.
/// 当持有输出流消失时，中止对应已派生 Provider 控制器任务。
struct AbortProviderControllerOnDrop {
    /// Exact abort authority for the controller task owned by the current stream instance.
    /// 当前流实例所持控制器任务的精确中止权限。
    handle: AbortHandle,
}

impl AbortProviderControllerOnDrop {
    /// Creates one stream-scoped abort owner for a spawned Provider controller task.
    /// 为已派生 Provider 控制器任务创建一个流作用域中止所有者。
    ///
    /// # Parameters
    /// 参数
    /// `handle` identifies the exact controller task whose lifetime must not outlive the output stream.
    /// `handle` 标识生命周期不得超过输出流的精确控制器任务。
    ///
    /// # Returns
    /// 返回值
    /// A guard that aborts the task when the owning stream scope is dropped.
    /// 返回在持有流作用域被丢弃时中止任务的守卫。
    fn new(handle: AbortHandle) -> Self {
        Self { handle }
    }
}

impl Drop for AbortProviderControllerOnDrop {
    /// Aborts the owned controller task when stream execution leaves its scope.
    /// 当流执行离开其作用域时中止所持控制器任务。
    ///
    /// # Returns
    /// 返回值
    /// Returns no value; aborting an already completed task is a harmless no-op.
    /// 不返回值；中止已经完成的任务是无害的空操作。
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// Projects the immutable provider route into the retry event's auditable route key.
/// 将不可变 Provider 路由投影为重试事件中可审计的路由键。
///
/// # Parameters
/// 参数
/// `route` is the exact route resolved before the transport adapter and retry loop are created.
/// `route` 是在创建传输适配器与重试循环前已经解析完成的精确路由。
///
/// # Returns
/// 返回值
/// A non-secret JSON object containing every field that must remain stable across retries.
/// 返回包含每次重试期间必须保持稳定的全部字段且不含秘密信息的 JSON 对象。
fn provider_retry_route_key_metadata(route: &ProviderRouteIdentity) -> serde_json::Value {
    json!({
        "provider_id": route.provider_id.as_str(),
        "service_entry_id": route.service_entry_id.as_str(),
        "connection_id": route.connection_id.as_str(),
        "credential_profile_id": route.credential_profile_id.as_str(),
        "model_id": route.model_id.as_str(),
        "protocol_profile_id": route.protocol_profile_id.as_str(),
        "catalog_generation": route.catalog_generation,
        "manifest_revision": route.manifest_revision,
        "route_revision": route.route_revision.as_str(),
    })
}

/// Executes one host-controlled provider stream through `vulcan-core`, forwards live deltas, and returns the final canonical state to the workbench layer.
/// 通过 `vulcan-core` 执行一次宿主控制的 Provider 流，转发实时增量，并把最终规范状态返回给工作台层。
pub(crate) fn execute_host_controlled_stream<A, F>(
    session_id: SessionId,
    task_runtime: Arc<TaskRuntime>,
    controller_config: CoreControllerConfig,
    core_snapshot: CoreConversationSnapshot,
    prompt: String,
    turn_start_mode: ProviderTransportTurnStartMode,
    provider_factory: F,
    runtime_bridge: Arc<VulcanManagedToolRuntimeBridge>,
    provider_label: String,
    request_budget_guard: Option<ProviderRequestBudgetGuard>,
    retry_policy: ProviderRetryPolicy,
    prompt_intervention: Option<RuntimePromptIntervention>,
    tool_request_rejection_reason: Option<String>,
    tool_request_rejection_all_tools: bool,
    tool_request_rejection_tool_ids: Vec<String>,
    route_identity: crate::agent_runtime::provider_transport_request::ProviderRouteIdentity,
) -> ProviderDeltaStream
where
    A: VulcanStreamingProviderAdapter<Response = CoreProviderTurnResponse>
        + Clone
        + Send
        + Sync
        + 'static,
    A::Request: ProviderBudgetRequest,
    F: Fn(ProviderLiveDeltaSender) -> A + Send + Sync + 'static,
{
    Box::pin(stream! {
        let provider_factory = Arc::new(provider_factory);
        let provider_label_for_join = provider_label.clone();
        let prompt_intervention = prompt_intervention.clone();
        let (live_sender, mut live_receiver) =
            unbounded_channel::<Result<ProviderStreamDelta, String>>();
        let task_sender = live_sender.clone();
        let controller_prompt = prompt.clone();
        let controller_task = tokio::spawn(async move {
            let mut completed_retries = 0u32;
            // retrySnapshot advances to the controller's last committed state so later physical Provider requests can retry without replaying completed tools.
            // retrySnapshot 推进到控制器最后提交的状态，使后续真实 Provider 请求可在不重放已完成工具的前提下重试。
            let mut retry_snapshot = core_snapshot;
            // retryResumesSeededInput prevents the original user prompt from being appended twice after resuming a committed snapshot.
            // retryResumesSeededInput 防止从已提交快照恢复后再次追加原始用户提示词。
            let mut retry_resumes_seeded_input = false;
            // retryRouteKey is captured once from the already-resolved execution. The concrete
            // adapter factory is fixed outside this loop, so retries replay the same protocol,
            // endpoint, request projection, manifest revision, route revision, and account
            // reference; only the credential lease may re-read a newer secret revision.
            // retryRouteKey 从已解析执行中仅捕获一次。具体适配器工厂固定在循环之外，
            // 因而重试会重放相同协议、端点、请求投影、清单修订号、路由修订号与账号引用；
            // 仅凭据租约可以重新读取更新的秘密修订版。
            let retry_route_key = route_identity;
            loop {
                let (attempt_sender, mut attempt_receiver) =
                    unbounded_channel::<Result<ProviderStreamDelta, String>>();
                // controller rejects an invalid retry snapshot through the stream instead of panicking inside the task.
                // controller 通过流拒绝无效的重试快照，而不是在任务内部触发 panic。
                let mut controller = match VulcanCoreController::new(
                    controller_config.clone(),
                    retry_snapshot.clone(),
                ) {
                    Ok(controller) => controller,
                    Err(error) => {
                        let _ = task_sender.send(Err(format!(
                            "Invalid core conversation snapshot: {error}"
                        )));
                        break;
                    }
                };
                let provider_adapter = BudgetCheckedStreamingAdapter::new(
                    provider_factory(attempt_sender.clone()),
                    request_budget_guard.clone(),
                );
                let tool_executor = StreamingRuntimeToolExecutor::new(
                    Arc::clone(&runtime_bridge),
                    attempt_sender.clone(),
                    tool_request_rejection_reason.clone(),
                    tool_request_rejection_all_tools,
                    tool_request_rejection_tool_ids.clone(),
                );
                let task_backend_guard = match task_runtime.install_backend(
                    session_id,
                    build_task_execution_handler(
                        Arc::clone(&task_runtime),
                        controller_config.clone(),
                        attempt_sender.clone(),
                    ),
                ) {
                    Ok(guard) => guard,
                    Err(error) => {
                        let _ = task_sender.send(Err(error));
                        break;
                    }
                };
                let tool_policy = AllowAllToolPolicy;
                let mut unsafe_delta_seen = false;
                // physicalRequestSeen distinguishes the retry attempt's first request from a later request reached after success.
                // physicalRequestSeen 区分本次重试尝试的首个请求与成功后继续到达的后续请求。
                let mut physical_request_seen = false;
                // physicalRequestActive prevents post-Provider tool failures from closing an already durable request again.
                // physicalRequestActive 防止 Provider 成功后的工具失败再次关闭已经持久化的请求。
                let mut physical_request_active = false;
                // activeToolRequestStreams tracks visible request drafts for attempt-scoped terminal cleanup.
                // activeToolRequestStreams 跟踪可见请求草稿，用于按尝试执行终态清理。
                let mut active_tool_request_streams = BTreeSet::<String>::new();
                let attempt_result = {
                    let checkpoint_sender = attempt_sender.clone();
                    let prompt_intervention = prompt_intervention.clone();
                    let checkpoint = move |
                        snapshot: &CoreConversationSnapshot,
                        usage: Option<&vulcan_core::types::ProviderUsageSnapshot>,
                    | {
                        // checkpoint_delta exposes a checkpoint only after its complete message namespace passes validation.
                        // checkpoint_delta 仅在完整消息命名空间通过校验后暴露检查点。
                        let checkpoint_delta = core_snapshot_to_session_conversation_state(snapshot)
                            .map(|conversation_state| {
                                ProviderStreamDelta::FinalConversationState(
                                    conversation_state,
                                    usage.cloned(),
                                )
                            });
                        let _ = checkpoint_sender.send(checkpoint_delta);
                    };
                    let intervention = move |controller: &mut VulcanCoreController| {
                        let Some(intervention) = prompt_intervention.as_ref() else {
                            return Ok(false);
                        };
                        intervention(session_id, controller).map_err(|error| match error {
                            crate::agent_runtime::runtime_stream_intervention::RuntimePromptInterventionError::ProviderRouteSwitchRequired { message } => {
                                CoreError::ProviderRouteSwitchRequired { message }
                            }
                            crate::agent_runtime::runtime_stream_intervention::RuntimePromptInterventionError::ContextCompactionPolicyRefreshRequired { message } => {
                                CoreError::ContextCompactionPolicyRefreshRequired { message }
                            }
                            other => CoreError::InvalidState {
                                message: other.to_string(),
                            },
                        })
                    };
                    let run_future = async {
                        // A transport retry must retain the already-admitted continuation mode and exact history.
                        // 传输重试必须保留已准入的继续模式及精确历史。
                        let effective_turn_start_mode = if retry_resumes_seeded_input
                            && turn_start_mode != ProviderTransportTurnStartMode::ContinuedProviderTurn
                        {
                            ProviderTransportTurnStartMode::RestoredCheckpoint
                        } else {
                            turn_start_mode
                        };
                        match effective_turn_start_mode {
                            ProviderTransportTurnStartMode::UserPrompt => {
                                controller.run_user_turn_streaming_with_checkpoints_and_interventions(
                                    &provider_adapter,
                                    &tool_executor,
                                    &tool_policy,
                                    controller_prompt.clone(),
                                    checkpoint,
                                    intervention,
                                )
                                .await
                            }
                            ProviderTransportTurnStartMode::RestoredCheckpoint => {
                                controller.run_seeded_turn_streaming_with_checkpoints_and_interventions(
                                    &provider_adapter,
                                    &tool_executor,
                                    &tool_policy,
                                    checkpoint,
                                    intervention,
                                )
                                .await
                            }
                            ProviderTransportTurnStartMode::ContinuedProviderTurn => {
                                controller.run_restored_provider_loop_streaming_with_checkpoints_and_interventions(
                                    &provider_adapter,
                                    &tool_executor,
                                    &tool_policy,
                                    None,
                                    checkpoint,
                                    intervention,
                                )
                                .await
                            }
                        }
                    };
                    tokio::pin!(run_future);
                    loop {
                        tokio::select! {
                            maybe_item = attempt_receiver.recv() => {
                                if let Some(item) = maybe_item {
                                    if let Ok(delta) = &item {
                                        observe_provider_request_retry_safety(
                                            &mut unsafe_delta_seen,
                                            delta,
                                        );
                                        observe_provider_request_retry_reset(
                                            &mut physical_request_seen,
                                            &mut completed_retries,
                                            delta,
                                        );
                                        observe_physical_request_activity(
                                            &mut physical_request_active,
                                            delta,
                                        );
                                        observe_active_tool_request_streams(
                                            &mut active_tool_request_streams,
                                            delta,
                                        );
                                    }
                                    let _ = task_sender.send(item);
                                }
                            }
                            result = &mut run_future => {
                                break result;
                            }
                        }
                    }
                };
                // backendReleaseResult is authoritative cleanup evidence required before retry or completion handling.
                // backendReleaseResult 是进入重试或完成处理前所需的权威清理证据。
                let backend_release_result = task_backend_guard.release();

                while let Ok(item) = attempt_receiver.try_recv() {
                    if let Ok(delta) = &item {
                        observe_provider_request_retry_safety(&mut unsafe_delta_seen, delta);
                        observe_provider_request_retry_reset(
                            &mut physical_request_seen,
                            &mut completed_retries,
                            delta,
                        );
                        observe_physical_request_activity(
                            &mut physical_request_active,
                            delta,
                        );
                        observe_active_tool_request_streams(
                            &mut active_tool_request_streams,
                            delta,
                        );
                    }
                    let _ = task_sender.send(item);
                }

                if let Err(error) = backend_release_result {
                    let _ = task_sender.send(Err(format!(
                        "The task backend could not be released after the provider attempt: {error}"
                    )));
                    break;
                }

                match attempt_result {
                    Ok(_) => {
                        abort_active_tool_request_streams(
                            &task_sender,
                            &mut active_tool_request_streams,
                            ToolRequestAbortReason::ProviderStreamCompletedWithoutRequestTerminal,
                        );
                        let last_successful_provider_usage =
                            controller.last_successful_provider_usage().cloned();
                        // final_delta rejects an invalid successful snapshot instead of publishing corrupt terminal state.
                        // final_delta 会拒绝无效的成功快照，而不是发布损坏的终态。
                        let final_delta =
                            core_snapshot_to_session_conversation_state(controller.snapshot()).map(
                                |conversation_state| {
                                    ProviderStreamDelta::FinalConversationState(
                                        conversation_state,
                                        last_successful_provider_usage,
                                    )
                                },
                            );
                        let _ = task_sender.send(final_delta);
                        break;
                    }
                    Err(error) => {
                        if let CoreError::ContextCompactionPolicyRefreshRequired { message } = &error {
                            abort_active_tool_request_streams(
                                &task_sender,
                                &mut active_tool_request_streams,
                                ToolRequestAbortReason::ContextCompactionPolicyRefreshRequired,
                            );
                            let last_successful_provider_usage =
                                controller.last_successful_provider_usage().cloned();
                            match core_snapshot_to_session_conversation_state(
                                controller.snapshot(),
                            ) {
                                Ok(conversation_state) => {
                                    let _ = task_sender.send(Ok(
                                        ProviderStreamDelta::FinalConversationState(
                                            conversation_state.clone(),
                                            last_successful_provider_usage,
                                        ),
                                    ));
                                    let _ = task_sender.send(Ok(
                                        ProviderStreamDelta::ContextCompactionPolicyRefreshRequired {
                                            conversation_state,
                                            reason: message.clone(),
                                        },
                                    ));
                                }
                                Err(conversion_error) => {
                                    let _ = task_sender.send(Err(format!(
                                        "Context compaction policy refresh boundary could not be serialized: {conversion_error}"
                                    )));
                                }
                            }
                            break;
                        }
                        if let CoreError::ProviderRouteSwitchRequired { message } = &error {
                            abort_active_tool_request_streams(
                                &task_sender,
                                &mut active_tool_request_streams,
                                ToolRequestAbortReason::ProviderRouteSwitchRequired,
                            );
                            let last_successful_provider_usage =
                                controller.last_successful_provider_usage().cloned();
                            match core_snapshot_to_session_conversation_state(controller.snapshot()) {
                                Ok(conversation_state) => {
                                    let _ = task_sender.send(Ok(
                                        ProviderStreamDelta::FinalConversationState(
                                            conversation_state.clone(),
                                            last_successful_provider_usage,
                                        ),
                                    ));
                                    let _ = task_sender.send(Ok(
                                        ProviderStreamDelta::ProviderRouteSwitchRequired {
                                            conversation_state,
                                            reason: message.clone(),
                                        },
                                    ));
                                }
                                Err(conversion_error) => {
                                    let _ = task_sender.send(Err(format!(
                                        "Provider route-switch boundary could not be serialized: {conversion_error}"
                                    )));
                                }
                            }
                            break;
                        }
                        if let CoreError::ContextCompactionRequired { message } = &error {
                            abort_active_tool_request_streams(
                                &task_sender,
                                &mut active_tool_request_streams,
                                ToolRequestAbortReason::ContextCompactionRequired,
                            );
                            let last_successful_provider_usage =
                                controller.last_successful_provider_usage().cloned();
                            let final_delta = core_snapshot_to_session_conversation_state(
                                controller.snapshot(),
                            )
                            .map(|conversation_state| {
                                ProviderStreamDelta::FinalConversationState(
                                    conversation_state,
                                    last_successful_provider_usage,
                                )
                            });
                            if let Err(persist_error) = send_durable_request_failure(
                                &task_sender,
                                physical_request_active,
                                false,
                                0,
                                completed_retries,
                                0,
                                String::from("context_compaction_required"),
                                None,
                            )
                            .await
                            {
                                let _ = task_sender.send(Err(persist_error));
                                break;
                            }
                            // finalDelta follows request failure so checkpoint projection cannot close the attempt as successful.
                            // finalDelta 位于请求失败之后，避免检查点投影把该 attempt 关闭为成功。
                            let _ = task_sender.send(final_delta);
                            match core_snapshot_to_session_conversation_state(controller.snapshot()) {
                                Ok(conversation_state) => {
                                    let _ = task_sender.send(Ok(
                                        ProviderStreamDelta::ContextCompactionRequired {
                                            conversation_state,
                                            reason: message.clone(),
                                        },
                                    ));
                                }
                                Err(conversion_error) => {
                                    let _ = task_sender.send(Err(format!(
                                        "Context compaction boundary could not be serialized: {conversion_error}"
                                    )));
                                }
                            }
                            break;
                        }
                        if provider_failure_requires_context_compaction(
                            &error,
                            unsafe_delta_seen,
                        ) {
                            abort_active_tool_request_streams(
                                &task_sender,
                                &mut active_tool_request_streams,
                                ToolRequestAbortReason::ProviderContextLengthExceeded,
                            );
                            let last_successful_provider_usage =
                                controller.last_successful_provider_usage().cloned();
                            let final_delta = core_snapshot_to_session_conversation_state(
                                controller.snapshot(),
                            )
                            .map(|conversation_state| {
                                ProviderStreamDelta::FinalConversationState(
                                    conversation_state,
                                    last_successful_provider_usage,
                                )
                            });
                            if let Err(persist_error) = send_durable_request_failure(
                                &task_sender,
                                physical_request_active,
                                false,
                                0,
                                completed_retries,
                                0,
                                String::from("context_length_exceeded"),
                                structured_provider_failure(&error),
                            )
                            .await
                            {
                                let _ = task_sender.send(Err(persist_error));
                                break;
                            }
                            // finalDelta follows request failure so checkpoint projection cannot close the attempt as successful.
                            // finalDelta 位于请求失败之后，避免检查点投影把该 attempt 关闭为成功。
                            let _ = task_sender.send(final_delta);
                            match core_snapshot_to_session_conversation_state(controller.snapshot()) {
                                Ok(conversation_state) => {
                                    let _ = task_sender.send(Ok(
                                        ProviderStreamDelta::ContextCompactionRequired {
                                            conversation_state,
                                            reason: String::from(
                                                "Provider rejected the replay-safe request because the declared context window was exceeded.",
                                            ),
                                        },
                                    ));
                                }
                                Err(conversion_error) => {
                                    let _ = task_sender.send(Err(format!(
                                        "Provider context-overflow boundary could not be serialized: {conversion_error}"
                                    )));
                                }
                            }
                            break;
                        }
                        // retryDecision gives a completed OAuth refresh one mandatory same-route replay before consulting generic retry policy.
                        // retryDecision 先为已完成的 OAuth 刷新提供一次强制同路由重放，再查询通用重试策略。
                        let retry_decision = refreshed_oauth_credential_retry_decision(
                            &error,
                            completed_retries,
                            unsafe_delta_seen,
                        )
                        .or_else(|| {
                            retry_policy.next_retry(
                                &error,
                                completed_retries,
                                unsafe_delta_seen,
                            )
                        });
                        abort_active_tool_request_streams(
                            &task_sender,
                            &mut active_tool_request_streams,
                            if retry_decision.is_some() {
                                ToolRequestAbortReason::ProviderRetrySuperseded
                            } else {
                                ToolRequestAbortReason::ProviderAttemptFailed
                            },
                        );
                        if let Some(decision) = retry_decision {
                            // retrySnapshot contains the user prompt, completed assistant/tool messages, and no failed Provider response.
                            // retrySnapshot 包含用户提示词及已完成的助手/工具消息，但不包含失败的 Provider 响应。
                            retry_snapshot = controller.snapshot().clone();
                            retry_resumes_seeded_input = true;
                            completed_retries = completed_retries.saturating_add(1);
                            let delay_ms = decision.delay.as_millis() as u64;
                            // delaySeconds rounds sub-second waits upward for an honest user-facing countdown value.
                            // delaySeconds 向上取整不足一秒的等待，以提供真实的用户可见倒计时数值。
                            let delay_seconds = delay_ms.div_ceil(1_000);
                            // failedRequestDelta closes the exact physical request before the retry wait begins.
                            // failedRequestDelta 在重试等待开始前关闭精确的真实请求。
                            if send_durable_request_failure(
                                &task_sender,
                                physical_request_active,
                                true,
                                decision.retry_number,
                                decision.max_retries,
                                delay_ms,
                                String::from("retryable_provider_transport_error"),
                                structured_provider_failure(&error),
                            )
                            .await
                            .is_err()
                            {
                                break;
                            }
                            // retryDetail exposes the exact finite host budget for every automatic retry.
                            // retryDetail 为每次自动重试公开精确的有限宿主预算。
                            let retry_detail = format!(
                                "{} retry {}/{} starts in {} seconds: {}",
                                provider_label,
                                decision.retry_number,
                                decision.max_retries,
                                delay_seconds,
                                decision.reason,
                            );
                            let retry_event = crate::agent_runtime::events::RuntimeEvent::new(
                                crate::agent_runtime::events::RuntimeEventKind::Retrying,
                                "Provider automatic retry",
                                retry_detail,
                            )
                            .with_metadata(Some(json!({
                                "attempt": decision.retry_number,
                                "max_retries": decision.max_retries,
                                "unbounded": decision.unbounded,
                                "delay_ms": delay_ms,
                                "delay_seconds": delay_seconds,
                                "reason": decision.reason,
                                "retry_route_key": provider_retry_route_key_metadata(&retry_route_key),
                            })));
                            // Stop retrying when the host runtime consumer has already dropped this stream.
                            // 当宿主运行时消费端已经丢弃当前流时，停止继续后台重试。
                            if task_sender
                                .send(Ok(ProviderStreamDelta::RuntimeEvent(retry_event)))
                                .is_err()
                            {
                                break;
                            }
                            tokio::time::sleep(decision.delay).await;
                            continue;
                        }

                        let last_successful_provider_usage =
                            controller.last_successful_provider_usage().cloned();
                        // failedRequestDelta closes the exact physical request before terminal stream teardown.
                        // failedRequestDelta 在终态流清理前关闭精确的真实请求。
                        if let Err(persist_error) = send_durable_request_failure(
                            &task_sender,
                            physical_request_active,
                            false,
                            0,
                            completed_retries,
                            0,
                            String::from("provider_transport_error"),
                            structured_provider_failure(&error),
                        )
                        .await
                        {
                            let _ = task_sender.send(Err(persist_error));
                            break;
                        }
                        // Preserve only the controller snapshot before the terminal error is reported.
                        // 只保留终态错误上报前的控制器快照，避免把损坏调用的流式输出写入上下文。
                        // final_delta preserves the last valid controller state only when its message identities remain canonical.
                        // final_delta 仅在消息身份仍规范时保留最后一份有效控制器状态。
                        let final_delta =
                            core_snapshot_to_session_conversation_state(controller.snapshot()).map(
                                |conversation_state| {
                                    ProviderStreamDelta::FinalConversationState(
                                        conversation_state,
                                        last_successful_provider_usage,
                                    )
                                },
                            );
                        let _ = task_sender.send(final_delta);
                        let _ = task_sender.send(Err(terminal_controller_failure_message(
                            &error, &retry_route_key,
                        )));
                        break;
                    }
                }
            }
        });
        // controllerTaskAbortGuard binds the spawned controller lifetime to this exact output stream.
        // controllerTaskAbortGuard 将已派生控制器生命周期绑定到当前精确输出流。
        let _controller_task_abort_guard =
            AbortProviderControllerOnDrop::new(controller_task.abort_handle());
        drop(live_sender);

        while let Some(item) = live_receiver.recv().await {
            yield item;
        }

        if let Err(error) = controller_task.await {
            yield Err(format!(
                "{}Controller background task failed: {}.",
                provider_label_for_join,
                error
            ));
        }
    })
}

/// Reports whether one normalized delta starts a new physical Provider request.
/// 报告一条规范化增量是否开始了一次新的真实 Provider 请求。
///
/// # Parameters
/// 参数
/// `delta` is the next normalized Provider stream delta.
/// `delta` 是下一条规范化 Provider 流增量。
///
/// # Returns
/// 返回值
/// `true` only after Started durability is confirmed.
/// 仅在 Started 可靠性得到确认后返回 `true`。
fn provider_delta_starts_physical_request(delta: &ProviderStreamDelta) -> bool {
    matches!(
        delta,
        ProviderStreamDelta::RequestStartDurabilityConfirmed { .. }
    )
}

/// Tracks whether the typed Provider stream currently owns one unterminated physical request.
/// 跟踪强类型 Provider 流当前是否持有一个尚未终结的物理请求。
///
/// # Parameters
/// 参数
/// `physical_request_active` stores the lifecycle state and `delta` is the next ordered Provider delta.
/// `physical_request_active` 保存生命周期状态，`delta` 是下一条有序 Provider 增量。
fn observe_physical_request_activity(
    physical_request_active: &mut bool,
    delta: &ProviderStreamDelta,
) {
    if provider_delta_starts_physical_request(delta) {
        *physical_request_active = true;
    } else if matches!(
        delta,
        ProviderStreamDelta::RequestSucceeded { .. }
            | ProviderStreamDelta::RequestFailedDurable { .. }
    ) {
        *physical_request_active = false;
    }
}

/// Updates retry safety at the boundary of each physical Provider request.
/// 在每次真实 Provider 请求边界更新重试安全状态。
///
/// `unsafe_delta_seen` records whether the current physical request exposed non-replayable output.
/// `unsafe_delta_seen` 记录当前真实请求是否已经暴露不可重放输出。
/// `delta` is the next normalized Provider stream delta.
/// `delta` 是下一条规范化 Provider 流增量。
/// This function mutates the supplied flag and returns no value.
/// 本函数修改传入标记且不返回值。
fn observe_provider_request_retry_safety(
    unsafe_delta_seen: &mut bool,
    delta: &ProviderStreamDelta,
) {
    if provider_delta_starts_physical_request(delta) {
        *unsafe_delta_seen = false;
        return;
    }
    if provider_delta_makes_retry_unsafe(delta) {
        *unsafe_delta_seen = true;
    }
}

/// Formats a terminal controller failure using the existing lossless public-error wire format.
/// 使用既有无损公共错误传输格式构造控制器终态失败。
/// `error` owns execution facts; `route` supplies the already-frozen provider identity.
/// `error` 持有执行事实；`route` 提供已冻结的供应商身份。
/// Returns the report diagnostic persisted even when the physical request has already closed.
/// 返回即使物理请求已闭合仍会持久化的报告诊断。
fn terminal_controller_failure_message(error: &CoreError, route: &ProviderRouteIdentity) -> String {
    if let CoreError::ToolDurabilityFailure {
        details: Some(details),
        ..
    } = error
    {
        // A closed API must not receive a second terminal; the existing display-note format retains these facts instead.
        // 已闭合 API 不能接收第二个终态；通过既有显示提示格式保留这些事实。
        let mut details = details.clone();
        details.provider_id = Some(route.provider_id.clone());
        details.entry_id = Some(route.service_entry_id.clone());
        details.model_id = Some(route.model_id.clone());
        return format!(
            "Provider streaming execution failed: provider `{}` failure: {}",
            route.provider_id,
            crate::agent_runtime::provider_transport_vulcan::public_execution_failure_message(
                &details
            ),
        );
    }
    format!("Provider streaming execution failed: {error}")
}

/// Extracts the structured provider failure retained by the controller error.
/// 提取控制器错误中保留的结构化供应商失败。
fn structured_provider_failure(
    error: &CoreError,
) -> Option<Box<vulcan_ai_contract::AiExecutionError>> {
    match error {
        CoreError::ProviderFailure { details, .. }
        | CoreError::ToolDurabilityFailure { details, .. } => details.clone(),
        CoreError::InvalidState { .. }
        | CoreError::ContextCompactionRequired { .. }
        | CoreError::ContextCompactionPolicyRefreshRequired { .. }
        | CoreError::ProviderRouteSwitchRequired { .. }
        | CoreError::ToolFailure { .. } => None,
    }
}

/// Builds one immediate retry only after the exact renewable account credential was refreshed.
/// 仅在精确可续期账号凭据已经刷新后构建一次立即重试。
///
/// # Parameters
/// 参数
///
/// `error` is the controller failure, `completed_retries` is the durable retry count before this
/// decision, and `unsafe_delta_seen` reports whether replay could repeat visible semantic output
/// or side effects.
/// `error` 是控制器失败，`completed_retries` 是本次决策前的持久重试计数，
/// `unsafe_delta_seen` 表示重放是否可能重复可见语义输出或副作用。
///
/// # Returns
/// 返回值
///
/// A one-shot zero-delay decision independent of generic transport retry settings, or `None`.
/// 返回独立于通用传输重试设置的单次零延迟决策，否则返回 `None`。
pub(crate) fn refreshed_oauth_credential_retry_decision(
    error: &CoreError,
    completed_retries: u32,
    unsafe_delta_seen: bool,
) -> Option<ProviderRetryDecision> {
    if unsafe_delta_seen {
        return None;
    }
    // structured is the exact normalized error annotated after the OAuth refresh succeeds.
    // structured 是 OAuth 刷新成功后标注的精确归一化错误。
    let structured = structured_provider_failure(error)?;
    if structured.http_status != Some(401)
        || structured.subcategory.as_deref() != Some(OAUTH_CREDENTIAL_REFRESHED_SUBCATEGORY)
        || !structured.retryable
        || structured.semantic_output_observed
        || structured.side_effect_observed
        || structured.dispatch_outcome != vulcan_ai_contract::DispatchOutcome::Dispatched
    {
        return None;
    }
    // retryNumber follows the durable host retry sequence even when generic retries happened first.
    // retryNumber 即使在通用重试已经发生后，也继续沿用宿主持久重试序列。
    let retry_number = completed_retries.saturating_add(1);
    Some(ProviderRetryDecision {
        retry_number,
        max_retries: retry_number,
        unbounded: false,
        delay: Duration::ZERO,
        reason: String::from("OAuth credential refreshed after Provider HTTP 401"),
    })
}

/// Reports whether one typed provider rejection permits a single compensating compaction.
/// 报告一个强类型供应商拒绝是否允许执行一次补偿压缩。
///
/// # Parameters
/// 参数
/// `error` retains structured provider facts and `unsafe_delta_seen` records visible non-replayable output.
/// `error` 保留结构化供应商事实，`unsafe_delta_seen` 记录可见且不可重放的输出。
///
/// # Returns
/// 返回值
/// `true` only for an exact context-window category with no semantic output, side effect, or unknown dispatch.
/// 仅对无语义输出、无副作用且分派结果非未知的精确上下文窗口类别返回 `true`。
fn provider_failure_requires_context_compaction(
    error: &CoreError,
    unsafe_delta_seen: bool,
) -> bool {
    let CoreError::ProviderFailure { details, .. } = error else {
        return false;
    };
    let Some(details) = details.as_deref() else {
        return false;
    };
    details.category == vulcan_ai_contract::AiErrorCategory::ContextLengthExceeded
        && !unsafe_delta_seen
        && !details.semantic_output_observed
        && !details.side_effect_observed
        && details.dispatch_outcome != vulcan_ai_contract::DispatchOutcome::Unknown
}

/// Resets consecutive retry accounting when the controller reaches another physical request after one successful request.
/// 当控制器在一次请求成功后进入下一次真实请求时，重置连续重试计数。
///
/// # Parameters
/// 参数
/// `physical_request_seen` records whether the current retry attempt already dispatched one physical request.
/// `physical_request_seen` 记录当前重试尝试是否已经发出过一次真实请求。
/// `completed_retries` stores consecutive failed retries since the latest successful physical request.
/// `completed_retries` 保存最近一次真实请求成功后的连续失败重试次数。
/// `delta` is the current Provider stream delta.
/// `delta` 是当前 Provider 流增量。
///
/// # Returns
/// 返回值
/// This function returns no value and mutates the retry state only at request boundaries.
/// 本函数不返回值，仅在请求边界处修改重试状态。
fn observe_provider_request_retry_reset(
    physical_request_seen: &mut bool,
    completed_retries: &mut u32,
    delta: &ProviderStreamDelta,
) {
    if !provider_delta_starts_physical_request(delta) {
        return;
    }
    if *physical_request_seen {
        *completed_retries = 0;
        return;
    }
    *physical_request_seen = true;
}

/// Updates the attempt-scoped set of visible tool requests from one normalized provider delta.
/// 根据一条规范化 Provider 增量更新尝试级可见工具请求集合。
///
/// # Parameters
/// 参数
/// `active_request_streams` owns request identifiers that have started but not reached a terminal fragment.
/// `active_request_streams` 持有已开始但尚未到达终态碎片的请求标识。
/// `delta` is the next normalized Provider stream change.
/// `delta` 是下一条规范化 Provider 流变化。
///
/// # Returns
/// 返回值
/// This function mutates the supplied set and returns no value.
/// 此函数修改传入集合，不返回值。
fn observe_active_tool_request_streams(
    active_request_streams: &mut BTreeSet<String>,
    delta: &ProviderStreamDelta,
) {
    let ProviderStreamDelta::ToolRequestFragment(fragment) = delta else {
        return;
    };
    match fragment {
        ToolRequestFragment::Detected {
            request_stream_id, ..
        } => {
            active_request_streams.insert(request_stream_id.clone());
        }
        ToolRequestFragment::Completed {
            request_stream_id, ..
        }
        | ToolRequestFragment::Invalid {
            request_stream_id, ..
        }
        | ToolRequestFragment::Aborted {
            request_stream_id, ..
        } => {
            active_request_streams.remove(request_stream_id);
        }
        ToolRequestFragment::ArgumentsDelta { .. } => {}
    }
}

/// Emits deterministic abort fragments for every request left active by a completed or failed provider attempt.
/// 为 Provider 尝试完成或失败后仍处于活动状态的每条请求发出确定性中止碎片。
///
/// # Parameters
/// 参数
/// `sender` is the outer stream channel observed by the runtime projection.
/// `sender` 是运行时投影所观察的外层流通道。
/// `active_request_streams` owns the sorted request identifiers requiring cleanup.
/// `active_request_streams` 持有需要清理的已排序请求标识。
/// `reason` is the stable cause published with every abort fragment.
/// `reason` 是随每条中止碎片发布的稳定原因。
///
/// # Returns
/// 返回值
/// This function drains the supplied set and returns no value.
/// 此函数清空传入集合，不返回值。
fn abort_active_tool_request_streams(
    sender: &ProviderLiveDeltaSender,
    active_request_streams: &mut BTreeSet<String>,
    reason: ToolRequestAbortReason,
) {
    for request_stream_id in std::mem::take(active_request_streams) {
        let _ = sender.send(Ok(ProviderStreamDelta::ToolRequestFragment(
            ToolRequestFragment::Aborted {
                request_stream_id,
                reason,
            },
        )));
    }
}

/// Executes one host-controlled provider stream from fully prepared runtime inputs, letting concrete entrypoints focus only on adapter-specific launch fields.
/// 基于完整已准备运行时输入执行一次宿主控制 Provider 流，使具体入口只关注适配器专用启动字段。
pub(crate) fn execute_host_controlled_stream_with_prepared_inputs<A, F>(
    prepared_runtime_inputs: PreparedProviderTransportRuntimeInputs,
    core_snapshot: CoreConversationSnapshot,
    provider_factory: F,
    provider_label: impl Into<String>,
    prompt_intervention: Option<RuntimePromptIntervention>,
) -> ProviderDeltaStream
where
    A: VulcanStreamingProviderAdapter<Response = CoreProviderTurnResponse>
        + Clone
        + Send
        + Sync
        + 'static,
    A::Request: ProviderBudgetRequest,
    F: Fn(ProviderLiveDeltaSender, PreparedProviderTransportAdapterInputs) -> A
        + Send
        + Sync
        + 'static,
{
    let provider_label = provider_label.into();
    let PreparedProviderTransportRuntimeInputs {
        entry_inputs:
            PreparedProviderTransportEntryInputs {
                prompt,
                turn_start_mode,
                controller_config,
            },
        tool_context:
            PreparedHostToolRuntimeContext {
                session_id,
                runtime_bridge,
                task_runtime,
                provider_tool_definitions,
                ..
            },
        request_budget_guard,
        retry_policy,
        tool_request_rejection_reason,
        tool_request_rejection_all_tools,
        tool_request_rejection_tool_ids,
        route_identity,
    } = prepared_runtime_inputs;

    let adapter_inputs = PreparedProviderTransportAdapterInputs {
        provider_tool_definitions,
    };

    execute_host_controlled_stream(
        session_id,
        task_runtime,
        controller_config,
        core_snapshot,
        prompt,
        turn_start_mode,
        move |live_sender| provider_factory(live_sender, adapter_inputs.clone()),
        runtime_bridge,
        provider_label,
        request_budget_guard,
        retry_policy,
        prompt_intervention,
        tool_request_rejection_reason,
        tool_request_rejection_all_tools,
        tool_request_rejection_tool_ids,
        route_identity,
    )
}

#[cfg(test)]
mod tests {
    //! Provider controller stream-ownership tests.
    //! Provider 控制器流所有权测试。

    use super::*;
    use crate::agent_runtime::events::{
        ProviderRequestPreparationCandidate, provider_request_preparation_barrier,
    };
    use tokio::sync::Notify;
    use vulcan_session_core::v1::context::LockedBaselineProjection;

    /// Verifies retry telemetry carries every field that defines the immutable resolved route.
    /// 验证重试遥测携带定义不可变已解析路由的全部字段。
    #[test]
    fn retry_route_key_metadata_preserves_protocol_and_manifest_identity() {
        // route models one session-frozen selection before the adapter is constructed.
        // route 模拟适配器构造前已经由会话冻结的一次选择。
        let route = ProviderRouteIdentity {
            provider_id: String::from("provider-a"),
            service_entry_id: String::from("entry-a"),
            connection_id: String::from("connection-a"),
            credential_profile_id: String::from("credential-a"),
            model_id: String::from("model-a"),
            protocol_profile_id: String::from("responses-a"),
            catalog_generation: 17,
            manifest_revision: 23,
            route_revision: String::from("sha256:route-a"),
        };

        let metadata = provider_retry_route_key_metadata(&route);

        assert_eq!(metadata["provider_id"], "provider-a");
        assert_eq!(metadata["service_entry_id"], "entry-a");
        assert_eq!(metadata["connection_id"], "connection-a");
        assert_eq!(metadata["credential_profile_id"], "credential-a");
        assert_eq!(metadata["model_id"], "model-a");
        assert_eq!(metadata["protocol_profile_id"], "responses-a");
        assert_eq!(metadata["catalog_generation"], 17);
        assert_eq!(metadata["manifest_revision"], 23);
        assert_eq!(metadata["route_revision"], "sha256:route-a");
    }

    /// Verifies final error projection preserves proven file effects without asserting preview effects.
    /// 验证终态错误投影保留已证明的文件副作用，且不将预览认作副作用。
    #[test]
    fn tool_persistence_failure_preserves_applied_effects() {
        // The frozen route supplies only identity, without inventing another physical API attempt.
        // 冻结路由仅提供身份，不虚构另一次物理 API 尝试。
        let route = ProviderRouteIdentity {
            provider_id: String::from("provider-a"),
            service_entry_id: String::from("entry-a"),
            connection_id: String::from("connection-a"),
            credential_profile_id: String::from("credential-a"),
            model_id: String::from("model-a"),
            protocol_profile_id: String::from("responses-a"),
            catalog_generation: 17,
            manifest_revision: 23,
            route_revision: String::from("sha256:route-a"),
        };
        for (mode, applied) in [("applied", true), ("preview", false)] {
            // The fixture follows the canonical host envelope used by the real streaming executor.
            // 夹具遵循真实流式执行器使用的规范宿主信封。
            let result = vulcan_core::types::ToolInvocationResult {
                tool_call_id: String::from("patch-call"),
                tool_name: String::from("apply_patch"),
                model_output: String::from("receipt"),
                host_result: Some(serde_json::json!({
                    "kind": "change_set", "payload": { "mode": mode }
                })),
                is_error: false,
            };
            // Exercise the production error builder and the final stream extraction together.
            // 联合验证生产错误构建与最终流错误提取。
            let error = crate::agent_runtime::provider_transport_stream_executor::tool_result_persistence_failure(
                &result, String::from("duplicate resource identifier"),
            );
            let details = structured_provider_failure(&error).expect("durability details");
            assert_eq!(details.side_effect_observed, applied);
            assert!(details.terminal);
            assert!(!details.retryable);
            assert_eq!(
                details.upstream_code.as_deref(),
                Some("tool_result_persistence_failed")
            );
            assert_eq!(
                details.upstream_message.as_deref(),
                Some("duplicate resource identifier")
            );
            assert_eq!(details.message.contains("already applied"), applied);
            assert!(refreshed_oauth_credential_retry_decision(&error, 0, false).is_none());
            // Reports still carry the typed facts when RequestSucceeded has already closed telemetry.
            // RequestSucceeded 已关闭遥测时，报告仍携带强类型事实。
            let report = terminal_controller_failure_message(&error, &route);
            let retained: vulcan_ai_contract::AiExecutionError = serde_json::from_str(
                report
                    .strip_prefix(
                        "Provider streaming execution failed: provider `provider-a` failure: ",
                    )
                    .expect("registered public error prefix"),
            )
            .expect("lossless report diagnostic");
            assert_eq!(retained.side_effect_observed, applied);
            assert_eq!(retained.provider_id.as_deref(), Some("provider-a"));
            assert_eq!(retained.model_id.as_deref(), Some("model-a"));
            assert_eq!(retained.message, details.message);
        }
    }

    /// Builds one typed context-window provider failure for compensation-safety tests.
    /// 构建一个用于补偿安全测试的强类型上下文窗口供应商失败。
    ///
    /// # Returns
    /// 返回值
    /// A dispatched rejection with no semantic output or side effect.
    /// 返回一个已分派且没有语义输出或副作用的拒绝。
    fn context_window_failure() -> CoreError {
        CoreError::ProviderFailure {
            provider_id: String::from("openai"),
            message: String::from("context length exceeded"),
            details: Some(Box::new(vulcan_ai_contract::AiExecutionError {
                category: vulcan_ai_contract::AiErrorCategory::ContextLengthExceeded,
                subcategory: Some(String::from("invalid_request_error")),
                stage: String::from("response_headers"),
                http_status: Some(400),
                transport: Some(vulcan_ai_contract::AiTransportKind::Http),
                websocket_close_code: None,
                upstream_code: Some(String::from("context_length_exceeded")),
                upstream_type: Some(String::from("invalid_request_error")),
                upstream_parameter: Some(String::from("input")),
                upstream_message: None,
                request_id: None,
                provider_id: Some(String::from("openai")),
                entry_id: Some(String::from("api")),
                model_id: Some(String::from("gpt")),
                message: String::from("request context exceeded"),
                dispatch_outcome: vulcan_ai_contract::DispatchOutcome::Dispatched,
                semantic_output_observed: false,
                side_effect_observed: false,
                terminal: true,
                retryable: false,
                retry_after: None,
            })),
        }
    }

    /// Builds one typed 401 failure after the exact OAuth credential was refreshed.
    /// 构建一个精确 OAuth 凭据已经刷新后的强类型 401 失败。
    ///
    /// # Returns
    /// 返回值
    ///
    /// A dispatched replay-safe authentication failure carrying the host refresh marker.
    /// 返回一个带宿主刷新标记、已分派且可安全重放的鉴权失败。
    fn refreshed_oauth_failure() -> CoreError {
        // failure reuses the complete structured fixture before replacing only authoritative facts.
        // failure 复用完整结构化夹具，再仅替换权威事实。
        let mut failure = context_window_failure();
        let CoreError::ProviderFailure {
            details: Some(details),
            ..
        } = &mut failure
        else {
            panic!("context-window fixture must retain structured provider details");
        };
        details.category = vulcan_ai_contract::AiErrorCategory::Authentication;
        details.subcategory = Some(String::from(OAUTH_CREDENTIAL_REFRESHED_SUBCATEGORY));
        details.http_status = Some(401);
        details.retryable = true;
        details.semantic_output_observed = false;
        details.side_effect_observed = false;
        failure
    }

    /// Verifies refreshed OAuth credentials receive one mandatory zero-delay same-route replay.
    /// 验证已刷新的 OAuth 凭据会获得一次强制零延迟同路由重放。
    #[test]
    fn refreshed_oauth_credential_retry_is_independent_of_generic_policy() {
        // failure is the exact adapter marker produced only after refresh succeeds.
        // failure 是仅在刷新成功后由适配器产生的精确标记。
        let failure = refreshed_oauth_failure();
        // decision proves credential recovery does not depend on user transport retry settings.
        // decision 证明凭据恢复不依赖用户传输重试设置。
        let decision = refreshed_oauth_credential_retry_decision(&failure, 3, false)
            .expect("refreshed OAuth credential should replay once");
        assert_eq!(decision.retry_number, 4);
        assert_eq!(decision.max_retries, 4);
        assert_eq!(decision.delay, Duration::ZERO);
        assert!(
            refreshed_oauth_credential_retry_decision(&failure, 3, true).is_none(),
            "visible semantic output must still block replay"
        );
    }

    /// Verifies compensation requires typed overflow plus replay-safe stream facts.
    /// 验证补偿压缩必须同时具备强类型越界与可回放安全流事实。
    #[test]
    fn context_overflow_compensation_requires_replay_safety() {
        let error = context_window_failure();
        assert!(provider_failure_requires_context_compaction(&error, false));
        assert!(!provider_failure_requires_context_compaction(&error, true));

        let CoreError::ProviderFailure {
            details: Some(details),
            ..
        } = error
        else {
            panic!("fixture must retain structured details");
        };
        let mut unsafe_details = *details;
        unsafe_details.dispatch_outcome = vulcan_ai_contract::DispatchOutcome::Unknown;
        let unsafe_error = CoreError::ProviderFailure {
            provider_id: String::from("openai"),
            message: String::from("context length exceeded"),
            details: Some(Box::new(unsafe_details)),
        };
        assert!(!provider_failure_requires_context_compaction(
            &unsafe_error,
            false
        ));
    }

    /// Builds one minimal request snapshot used to verify physical-request retry boundaries.
    /// 构建一份用于验证真实请求重试边界的最小请求快照。
    ///
    /// This function accepts no parameters and returns a valid empty request snapshot.
    /// 本函数不接收参数，并返回一份有效的空请求快照。
    fn test_request_snapshot() -> vulcan_core::types::ProviderRequestSnapshot {
        vulcan_core::types::ProviderRequestSnapshot {
            provider_id: String::from("test-provider"),
            model_id: String::from("test-model"),
            system_prompt: String::new(),
            prompt_layers: vulcan_core::types::ProviderPromptLayers::default(),
            prompt: String::from("test prompt"),
            prompt_in_history: false,
            history: Vec::new(),
            temperature: None,
            max_turns: 1,
            tool_schemas: Vec::new(),
            additional_params: None,
            request_headers: None,
            continuation: vulcan_core::types::ProviderContinuation::default(),
        }
    }

    /// Builds the durable request boundary emitted by the active public-contract path.
    /// 构建当前公共契约路径实际发出的可靠请求边界。
    ///
    /// # Returns
    /// 返回值
    /// A request-prepared delta with a valid persistence completion handle.
    /// 返回带有效持久化完成句柄的请求已准备增量。
    fn test_request_prepared_delta() -> ProviderStreamDelta {
        let (completion, _receiver) = provider_request_preparation_barrier();
        ProviderStreamDelta::RequestPrepared {
            candidate: ProviderRequestPreparationCandidate {
                media_context: None,
                request_snapshot: test_request_snapshot(),
                exact_request: serde_json::from_value(json!({
                    "operation": "conversation_respond",
                    "model_id": "test-model"
                }))
                .expect("the minimal public AI request fixture must be valid"),
                controller_config: CoreControllerConfig::default(),
                controller_snapshot: CoreConversationSnapshot::default(),
                context_budget: None,
                request_budget_guard: None,
                locked_baseline: LockedBaselineProjection::empty(),
                dynamic_prompt_layers: vulcan_core::types::ProviderPromptLayers::default(),
                fixed_instruction_layers: Default::default(),
            },
            completion,
        }
    }

    /// Builds the exact post-permit request-start confirmation used by state-only tests.
    /// 构建纯状态测试使用的精确许可后请求开始确认。
    ///
    /// # Returns
    /// 返回值
    /// A confirmation carrying the final request snapshot and no context-budget decision.
    /// 返回携带最终请求快照且不包含上下文预算判定的确认。
    fn test_request_start_durability_confirmed_delta() -> ProviderStreamDelta {
        ProviderStreamDelta::RequestStartDurabilityConfirmed {
            request_snapshot: test_request_snapshot(),
            context_budget: None,
        }
    }

    /// Verifies only a durability-confirmed Provider request resets retry safety.
    /// 验证只有可靠性已确认的 Provider 请求才会重置重试安全状态。
    #[test]
    fn confirmed_physical_request_boundary_resets_retry_safety() {
        // unsafeDeltaSeen models visible output emitted by one earlier successful physical request.
        // unsafeDeltaSeen 模拟更早一次成功真实请求已经发出的可见输出。
        let mut unsafe_delta_seen = false;
        observe_provider_request_retry_safety(
            &mut unsafe_delta_seen,
            &ProviderStreamDelta::Output(String::from("earlier response")),
        );
        assert!(unsafe_delta_seen);

        observe_provider_request_retry_safety(
            &mut unsafe_delta_seen,
            &test_request_prepared_delta(),
        );
        assert!(unsafe_delta_seen);

        observe_provider_request_retry_safety(
            &mut unsafe_delta_seen,
            &test_request_start_durability_confirmed_delta(),
        );
        assert!(!unsafe_delta_seen);
    }

    /// Verifies a successful physical request resets retry numbering before the next request starts.
    /// 验证一次真实请求成功后，下一次请求开始前会重置重试序号。
    #[test]
    fn next_physical_request_resets_consecutive_retry_count() {
        // physicalRequestSeen starts false for each outer retry attempt.
        // physicalRequestSeen 在每次外层重试尝试开始时为 false。
        let mut physical_request_seen = false;
        // completedRetries models three consecutive failures before the current attempt succeeds.
        // completedRetries 模拟当前尝试成功前已经发生三次连续失败。
        let mut completed_retries = 3;

        observe_provider_request_retry_reset(
            &mut physical_request_seen,
            &mut completed_retries,
            &test_request_prepared_delta(),
        );
        assert_eq!(completed_retries, 3);

        observe_provider_request_retry_reset(
            &mut physical_request_seen,
            &mut completed_retries,
            &test_request_start_durability_confirmed_delta(),
        );
        assert_eq!(completed_retries, 3);

        observe_provider_request_retry_reset(
            &mut physical_request_seen,
            &mut completed_retries,
            &test_request_start_durability_confirmed_delta(),
        );
        assert_eq!(completed_retries, 0);
    }

    /// Verifies a successful Provider boundary clears the active physical-request lifecycle marker.
    /// 验证 Provider 成功边界会清除活动物理请求生命周期标记。
    #[test]
    fn successful_provider_boundary_clears_active_physical_request() {
        // physicalRequestActive begins without an in-flight Provider call.
        // physicalRequestActive 初始时不存在正在进行的 Provider 调用。
        let mut physical_request_active = false;
        observe_physical_request_activity(
            &mut physical_request_active,
            &test_request_prepared_delta(),
        );
        assert!(!physical_request_active);

        observe_physical_request_activity(
            &mut physical_request_active,
            &test_request_start_durability_confirmed_delta(),
        );
        assert!(physical_request_active);

        // completion supplies the typed success boundary without requiring host persistence in this state-only test.
        // completion 为纯状态测试提供强类型成功边界，无需执行宿主持久化。
        let (completion, _receiver) = provider_request_durability_barrier();
        observe_physical_request_activity(
            &mut physical_request_active,
            &ProviderStreamDelta::RequestSucceeded {
                usage: None,
                provider_source:
                    crate::agent_runtime::provider_v1_bridge::test_provider_v1_source_input(),
                tool_call_count: 0,
                completion,
            },
        );
        assert!(!physical_request_active);
    }

    /// Verifies a rejected Started persistence boundary cannot create a synthetic terminal call.
    /// 验证被拒绝的 Started 持久化边界不能生成虚假的调用终态。
    #[tokio::test]
    async fn unconfirmed_request_preparation_skips_terminal_boundary() {
        // physicalRequestActive remains false until the adapter receives durable host acknowledgement.
        // physicalRequestActive 在适配器收到宿主可靠确认前保持为 false。
        let mut physical_request_active = false;
        observe_physical_request_activity(
            &mut physical_request_active,
            &test_request_prepared_delta(),
        );
        assert!(!physical_request_active);

        // sender exposes any incorrectly synthesized Completed boundary.
        // sender 用于暴露任何错误合成的 Completed 边界。
        let (sender, mut receiver) = unbounded_channel();
        send_durable_request_failure(
            &sender,
            physical_request_active,
            false,
            0,
            0,
            0,
            String::from("start_persistence_rejected"),
            None,
        )
        .await
        .expect("unconfirmed preparation must not need terminal persistence");
        assert!(receiver.try_recv().is_err());
    }

    /// Verifies a post-Provider controller failure emits no duplicate durable request terminal.
    /// 验证 Provider 成功后的控制器失败不会发出重复的可靠请求终态。
    #[tokio::test]
    async fn inactive_physical_request_skips_duplicate_failure_boundary() {
        // sender and receiver expose whether the helper incorrectly emits a terminal delta.
        // sender 与 receiver 用于观测辅助函数是否错误发出终态增量。
        let (sender, mut receiver) = unbounded_channel();
        send_durable_request_failure(
            &sender,
            false,
            false,
            0,
            0,
            0,
            String::from("tool_execution_error"),
            None,
        )
        .await
        .expect("an inactive request must not require another durability boundary");
        assert!(receiver.try_recv().is_err());
    }

    /// Verifies dropping the stream-scoped guard cancels its still-running controller task.
    /// 验证丢弃流作用域守卫会取消其仍在运行的控制器任务。
    ///
    /// # Returns
    /// 返回值
    /// This test returns no value and fails if the controller task survives owner teardown.
    /// 此测试无返回值；若控制器任务在所有者销毁后仍存活则测试失败。
    #[tokio::test]
    async fn provider_controller_abort_guard_cancels_owned_task_on_drop() {
        // taskStarted confirms the spawned controller reached its pending body before cancellation.
        // taskStarted 确认已派生控制器在取消前已经进入等待主体。
        let task_started = Arc::new(Notify::new());
        // taskStartedSignal transfers the exact start notification into the spawned task.
        // taskStartedSignal 将精确启动通知转移到已派生任务中。
        let task_started_signal = Arc::clone(&task_started);
        // controllerTask models a controller that would run forever without stream-owned cancellation.
        // controllerTask 模拟在没有流所有权取消时会永久运行的控制器。
        let controller_task = tokio::spawn(async move {
            task_started_signal.notify_one();
            std::future::pending::<()>().await;
        });
        // controllerTaskAbortGuard owns cancellation for the exact spawned task.
        // controllerTaskAbortGuard 持有精确已派生任务的取消责任。
        let controller_task_abort_guard =
            AbortProviderControllerOnDrop::new(controller_task.abort_handle());
        task_started.notified().await;

        drop(controller_task_abort_guard);
        // joinError proves the owned task terminated through cancellation rather than normal completion.
        // joinError 证明所持任务通过取消终止，而不是正常完成。
        let join_error = controller_task
            .await
            .expect_err("dropping the stream owner must cancel the controller task");
        assert!(join_error.is_cancelled());
    }
}
