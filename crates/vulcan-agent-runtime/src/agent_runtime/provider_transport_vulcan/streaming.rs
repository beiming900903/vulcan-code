//! Provider dispatch and streaming lifecycle under host durability barriers.
//! 受宿主持久化屏障约束的 Provider 分派及流生命周期。

use super::accumulator::{PublicTurnAccumulator, transient_reasoning_status_kind};
use super::generated_resources::materialize_provider_resource_event;
use super::trace::{
    ProviderApiTraceProcessingFailureSender, ProviderApiTraceResponseState,
    drain_provider_stream_for_trace, provider_trace_processing_stage_for_event,
};
use super::{
    OAUTH_CREDENTIAL_REFRESHED_SUBCATEGORY, VulcanPublicContractAdapter,
    VulcanPublicContractRequest, provider_failure, public_execution_failure, public_usage_to_core,
};
use crate::agent_runtime::events::{
    PreparedPhysicalApi, ProviderRequestPreparationCandidate, provider_request_durability_barrier,
    provider_request_preparation_barrier,
};
use crate::agent_runtime::provider_stream::ProviderStreamDelta;
use crate::agent_runtime::provider_v1_bridge::provider_v1_source_from_completed_response;
use async_trait::async_trait;
use futures_util::StreamExt;
use vulcan_ai_contract::{AiExecutionError, AiStreamEvent};
use vulcan_core::provider::{
    VulcanProviderAdapter, VulcanProviderDeltaSink, VulcanStreamingProviderAdapter,
};
use vulcan_core::types::CoreError as VulcanCoreError;
use vulcan_provider_adapters::{
    ProviderExecutionCredentialLease, ProviderWireRequestTraceDispatch,
};

/// Reports whether one public stream event is an explicit protocol terminal boundary.
/// 报告一项公共流事件是否为显式协议终态边界。
///
/// # Parameters
/// 参数
/// `event` is the normalized provider event produced by the selected protocol decoder.
/// `event` 是所选协议解码器产出的规范化 Provider 事件。
///
/// # Returns
/// 返回值
/// `true` for completed, failed, or cancelled responses; otherwise `false`.
/// 对已完成、失败或已取消响应返回 `true`，否则返回 `false`。
fn public_stream_event_is_terminal(event: &AiStreamEvent) -> bool {
    matches!(
        event,
        AiStreamEvent::ResponseCompleted
            | AiStreamEvent::ResponseFailed { .. }
            | AiStreamEvent::ResponseCancelled
    )
}

/// Marks one rejected-stream failure after its exact renewable credential was refreshed.
/// 在精确可续期凭据已刷新后标注一次被拒绝的流失败。
///
/// # Parameters
/// 参数
///
/// `error` is the normalized upstream failure owned by the adapter dispatch path.
/// `error` 是适配器分派路径持有的归一化上游失败。
///
/// # Notes
/// 说明
/// This is the sole definition of the authentication-recovery marker. The host transport recognizes a
/// retryable 401 carrying this exact subcategory, so the marker, its retryability and the credentials
/// that justify it must always change together.
/// 这是认证恢复标记的唯一来源。宿主传输仅识别携带该精确子类别的可重试 401，因此标记、可重试性
/// 与使其成立的凭据必须始终一起变化。
pub(crate) fn mark_credential_refreshed_after_unauthorized(
    error: &mut vulcan_ai_contract::AiExecutionError,
) {
    error.subcategory = Some(String::from(OAUTH_CREDENTIAL_REFRESHED_SUBCATEGORY));
    error.retryable = true;
}

/// Refreshes one exact renewable credential after a replay-safe upstream 401 response.
/// 在收到可安全重放的上游 401 响应后刷新一份精确可续期凭据。
///
/// # Parameters
/// 参数
///
/// `credential` owns the frozen account reference, `rejected_revision` identifies the token
/// used by this physical request, and `error` is the normalized upstream failure.
/// `credential` 持有冻结账号引用，`rejected_revision` 标识当前真实请求使用的 Token，
/// `error` 是归一化上游失败。
async fn recover_rejected_oauth_credential(
    credential: &ProviderExecutionCredentialLease,
    rejected_revision: u64,
    error: &mut AiExecutionError,
) {
    if error.http_status != Some(401)
        || error.semantic_output_observed
        || error.side_effect_observed
    {
        return;
    }
    match credential
        .refresh_after_unauthorized(rejected_revision)
        .await
    {
        Ok(true) => mark_credential_refreshed_after_unauthorized(error),
        Ok(false) => {}
        Err(refresh_error) => {
            error.retryable = false;
            error.message = format!(
                "{} Host OAuth credential refresh failed after the upstream 401 response: {refresh_error}",
                error.message
            );
        }
    }
}

#[async_trait]
impl VulcanStreamingProviderAdapter for VulcanPublicContractAdapter {
    async fn execute_request_streaming<S>(
        &self,
        request: Self::Request,
        sink: &mut S,
    ) -> Result<Self::Response, VulcanCoreError>
    where
        S: VulcanProviderDeltaSink,
    {
        let VulcanPublicContractRequest {
            public,
            controller_config,
            controller_snapshot,
            budget,
            context_budget,
            request_budget_guard,
        } = request;
        let (prepared_completion, prepared_receiver) = provider_request_preparation_barrier();
        self.live_sender
            .send(Ok(ProviderStreamDelta::RequestPrepared {
                candidate: ProviderRequestPreparationCandidate {
                    media_context: Some(
                        crate::agent_runtime::events::ProviderRequestMediaContext {
                            execution: std::sync::Arc::clone(&self.media_execution),
                            session_scope_id: self.cache_scope_id.clone(),
                            tool_names: public.tools.iter().map(|tool| tool.name.clone()).collect(),
                        },
                    ),
                    exact_request: public,
                    controller_config,
                    controller_snapshot,
                    request_snapshot: budget,
                    context_budget,
                    request_budget_guard,
                    locked_baseline: self.locked_baseline.clone(),
                    dynamic_prompt_layers: self.dynamic_prompt_layers.clone(),
                    fixed_instruction_layers: self.fixed_instruction_layers.clone(),
                },
                completion: prepared_completion,
            }))
            .map_err(|_| {
                provider_failure(
                    self.provider_id(),
                    "The provider request stream closed before dispatch.",
                )
            })?;
        let (
            public,
            controller_snapshot,
            request_snapshot,
            context_budget,
            provider_api_trace_context,
        ) = match prepared_receiver
            .await
            .map_err(|_| {
                provider_failure(
                    self.provider_id(),
                    "The host dropped the typed Provider preparation permit before dispatch.",
                )
            })?
            .map_err(|error| {
                provider_failure(
                    self.provider_id(),
                    &format!(
                        "The Provider request was not dispatched because the unique preparation owner rejected it: {error}"
                    ),
                )
            })? {
            PreparedPhysicalApi::ContextCompactionRequired { reason } => {
                return Err(VulcanCoreError::ContextCompactionRequired { message: reason });
            }
            PreparedPhysicalApi::RequestConfigurationChanged => {
                return Err(VulcanCoreError::ProviderRouteSwitchRequired {
                    message: String::from("The captured session inputs or request configuration changed before dispatch."),
                });
            }
            PreparedPhysicalApi::SessionV1 {
                exact_request,
                controller_config,
                controller_snapshot,
                request_snapshot,
                context_budget,
                core_start,
                provider_api_trace_store,
            } => {
                let _ = controller_config;
                (
                    exact_request,
                    controller_snapshot,
                    request_snapshot,
                    context_budget,
                    provider_api_trace_store.map(|store| (core_start.api_call_id, store)),
                )
            }
            PreparedPhysicalApi::ExcludedNonSession {
                exact_request,
                controller_config,
                controller_snapshot,
                request_snapshot,
                context_budget,
            } => {
                let _ = controller_config;
                (
                    exact_request,
                    controller_snapshot,
                    request_snapshot,
                    context_budget,
                    None,
                )
            }
        };
        let trace_turn_anchor_message_id = controller_snapshot
            .messages
            .last()
            .map(|message| message.id.clone());
        self.live_sender
            .send(Ok(ProviderStreamDelta::RequestStartDurabilityConfirmed {
                request_snapshot,
                context_budget,
            }))
            .map_err(|_| {
                provider_failure(
                    self.provider_id(),
                    "The provider request stream closed after start persistence and before dispatch.",
                )
            })?;
        // secret is re-read after the durable start boundary and immediately before this physical dispatch.
        // secret 在可靠起始边界之后、当前真实分派之前立即重新读取。
        let secret = self
            .credential
            .resolve_for_request()
            .await
            .map_err(|error| {
                provider_failure(
                    self.provider_id(),
                    &format!(
                        "The Provider credential could not be resolved before dispatch: {error}"
                    ),
                )
            })?;
        // rejectedSecretRevision binds any later 401 recovery to the exact token used on the wire.
        // rejectedSecretRevision 将后续任何 401 恢复绑定到线上实际使用的精确 Token 修订版。
        let rejected_secret_revision = secret.revision();
        let execution_id = uuid::Uuid::new_v4().to_string();
        let trace_attempt =
            if let Some((api_call_id, trace_store)) = provider_api_trace_context.as_ref() {
                match self.claim_provider_api_trace_attempt(*api_call_id, trace_store) {
                    Ok(Some(reservation)) => Some((reservation, trace_store.clone())),
                    Ok(None) => None,
                    Err(error) => {
                        eprintln!(
                            "[provider-api-trace] session={} reservation_failed={error}",
                            self.cache_scope_id
                        );
                        None
                    }
                }
            } else {
                if self.provider_trace_enabled() {
                    eprintln!(
                        "[provider-api-trace] session={} skipped=no_session_v1_trace_owner",
                        self.cache_scope_id
                    );
                }
                None
            };
        // wireCaptures pair the exact dispatched request with bytes observed before local response parsing.
        // wireCaptures 将实际分派请求与本地响应解析前观察到的字节成对关联。
        let wire_captures = if let Some((trace_reservation, trace_store)) = trace_attempt.as_ref() {
            let (request_sender, request_receiver) = tokio::sync::oneshot::channel();
            let (response_sender, response_receiver) = tokio::sync::oneshot::channel();
            let (processing_failure_sender, processing_failure_receiver) =
                tokio::sync::oneshot::channel();
            let trace_adapter = self.clone();
            let trace_execution_id = execution_id.clone();
            let trace_reservation = trace_reservation.clone();
            let trace_store = trace_store.clone();
            let trace_turn_anchor_message_id = trace_turn_anchor_message_id.clone();
            tokio::spawn(async move {
                let Ok(ProviderWireRequestTraceDispatch {
                    request: captured_request,
                    persistence_result,
                }) = request_receiver.await
                else {
                    // A closed capture channel means encoding failed before an API request existed.
                    // 捕获通道关闭表示编码在 API 请求形成前失败，因此没有请求可保存。
                    return;
                };
                // The session-owned store already durably reserved trace_sequence before wire
                // dispatch; complete input/output artifacts publish only after capture finishes.
                // 会话级存储已在线路分派前可靠预留 trace_sequence；完整 input/output 制品只在捕获
                // 结束后发布。
                if persistence_result.send(Ok(())).is_err() {
                    return;
                }
                // A closed response channel remains explicit in the trace instead of discarding the request.
                // 响应通道关闭会在追踪中显式记录，而不是丢弃请求。
                let response_state = response_receiver.await.map_or(
                    ProviderApiTraceResponseState::CaptureChannelClosed,
                    ProviderApiTraceResponseState::Captured,
                );
                // processingFailure closes only after the public accumulator has accepted or rejected the captured response.
                // processingFailure 仅在公共累积器接受或拒绝已捕获响应后关闭。
                let processing_failure = processing_failure_receiver.await.ok();
                if let Err(error) = trace_adapter
                    .persist_provider_api_trace_snapshot(
                        trace_store,
                        trace_reservation,
                        trace_execution_id.as_str(),
                        trace_turn_anchor_message_id.as_deref(),
                        captured_request,
                        response_state,
                        processing_failure,
                    )
                    .await
                {
                    eprintln!(
                        "[provider-api-trace] session={} execution={} failed={error}",
                        trace_adapter.cache_scope_id, trace_execution_id
                    );
                }
            });
            Some((request_sender, response_sender, processing_failure_sender))
        } else {
            None
        };
        let (mut stream, processing_failure_sender) =
            if let Some((request_sender, response_sender, processing_failure_sender)) =
                wire_captures
            {
                (
                    self.client.execute_with_wire_api_trace_capture(
                        self.route.clone(),
                        public.clone(),
                        secret.clone(),
                        execution_id,
                        request_sender,
                        response_sender,
                    ),
                    Some(processing_failure_sender),
                )
            } else {
                (
                    self.client
                        .execute(self.route.clone(), public.clone(), secret, execution_id),
                    None,
                )
            };
        let mut trace_processing_failure =
            ProviderApiTraceProcessingFailureSender::new(processing_failure_sender);
        // routeFingerprint binds replay signatures to the exact resolved endpoint used by this request.
        // routeFingerprint 将重放签名绑定到本次请求使用的精确已解析端点。
        let route_fingerprint = self.route.request_url("/responses");
        let transient_reasoning_kind = transient_reasoning_status_kind(
            self.provider_id(),
            self.route.response_projection_profiles.as_slice(),
        )?;
        let mut response = PublicTurnAccumulator::new(
            self.provider_id(),
            self.route.connection.id.as_str(),
            route_fingerprint.as_str(),
            self.route.protocol_route.protocol_profile_id.as_str(),
            self.model_id(),
            transient_reasoning_kind,
        );
        while let Some(item) = stream.next().await {
            // event preserves the normalized provider item after credential recovery annotates failures.
            // event 在凭据恢复标注失败后保留归一化供应商事件。
            let event = match item {
                Ok(event) => event,
                Err(mut failure) => {
                    attach_execution_route_context(
                        &mut failure.error,
                        self.provider_id(),
                        self.route.entry.id.as_str(),
                        self.model_id(),
                        self.route.protocol_route.responses_recovery_policy(),
                    );
                    recover_rejected_oauth_credential(
                        &self.credential,
                        rejected_secret_revision,
                        &mut failure.error,
                    )
                    .await;
                    return Err(public_execution_failure(self.provider_id(), *failure.error));
                }
            };
            let mut event = match materialize_provider_resource_event(
                self.provider_id(),
                self.workspace_path.as_str(),
                event,
            )
            .await
            {
                Ok(event) => event,
                Err(error) => {
                    trace_processing_failure.observe_failure("response_projection", &error);
                    return Err(error);
                }
            };
            if let AiStreamEvent::ResponseFailed { error } = &mut event {
                attach_execution_route_context(
                    error,
                    self.provider_id(),
                    self.route.entry.id.as_str(),
                    self.model_id(),
                    self.route.protocol_route.responses_recovery_policy(),
                );
                recover_rejected_oauth_credential(
                    &self.credential,
                    rejected_secret_revision,
                    error,
                )
                .await;
            }
            let event = match event {
                AiStreamEvent::TransportObserved {
                    transport,
                    dispatched_at_ms,
                    headers_at_ms,
                    http_status,
                    provider_request_id,
                    rate_limit_headers,
                } => {
                    let (dispatched_completion, dispatched_receiver) =
                        provider_request_durability_barrier();
                    self.live_sender
                        .send(Ok(ProviderStreamDelta::TransportObserved {
                            transport,
                            dispatched_at_ms,
                            headers_at_ms,
                            http_status,
                            provider_request_id,
                            rate_limit_headers,
                            completion: dispatched_completion,
                        }))
                        .map_err(|_| {
                            provider_failure(
                                self.provider_id(),
                                "The provider request stream closed before dispatch persistence.",
                            )
                        })?;
                    dispatched_receiver
                        .await
                        .map_err(|_| {
                            provider_failure(
                                self.provider_id(),
                                "The host dropped the Provider dispatch persistence barrier.",
                            )
                        })?
                        .map_err(|error| {
                            provider_failure(
                                self.provider_id(),
                                &format!(
                                    "The Provider request was dispatched but its dispatched state could not be persisted: {error}"
                                ),
                            )
                        })?;
                    continue;
                }
                event => event,
            };
            // terminalEvent closes the normalized response at its explicit protocol boundary instead of waiting for transport EOF.
            // terminalEvent 在显式协议终态边界关闭规范化响应，而不是继续等待传输 EOF。
            let terminal_event = public_stream_event_is_terminal(&event);
            // responseIdentity captures response.created before a later terminal stream error can end this attempt.
            // responseIdentity 在后续终态流错误结束本次尝试前捕获 response.created。
            let response_identity = match &event {
                AiStreamEvent::ResponseMetadataUpdated { continuation } => {
                    Some(continuation.provider_message_id.clone())
                }
                _ => None,
            };
            let usage_updated = matches!(&event, AiStreamEvent::UsageUpdated { .. });
            let processing_stage = provider_trace_processing_stage_for_event(&event);
            // applyResult is separated from `?` so trace mode can retain the rest of a failed API body.
            // applyResult 与 `?` 分离，使追踪模式可以保留失败 API 正文的剩余部分。
            let apply_result =
                response.apply_event(self.provider_id(), &self.live_sender, sink, event);
            if let Err(error) = apply_result {
                trace_processing_failure.observe_failure(processing_stage, &error);
                if self.provider_trace_enabled() {
                    drain_provider_stream_for_trace(stream);
                }
                return Err(error);
            }
            if usage_updated && let Some(usage) = response.usage.as_ref() {
                let _ = self.live_sender.send(Ok(ProviderStreamDelta::UsageUpdated(
                    public_usage_to_core(usage),
                )));
            }
            if let Some(response_id) = response_identity {
                let _ = self
                    .live_sender
                    .send(Ok(ProviderStreamDelta::ResponseIdentityObserved(
                        response_id,
                    )));
            }
            if terminal_event {
                if self.provider_trace_enabled() {
                    // terminalTraceDrain preserves transport bytes that follow the semantic terminal event
                    // without delaying tool execution or projecting any later semantic event.
                    // terminalTraceDrain 会保留语义终态事件之后的传输字节，且不会延迟工具执行或投影任何
                    // 后续语义事件。
                    drain_provider_stream_for_trace(stream);
                }
                break;
            }
        }
        let final_usage = response.usage.as_ref().map(public_usage_to_core);
        let finish_stage = if response.has_pending_tool_request() {
            "tool_request_coordinator"
        } else {
            "response_projection"
        };
        let final_response = match response.finish() {
            Ok(response) => response,
            Err(mut error) => {
                // Typed completion failures carry the same exact route as stream failures.
                // 强类型完成失败携带与流失败相同的精确路由。
                if let VulcanCoreError::ProviderFailure {
                    details: Some(details),
                    ..
                } = &mut error
                {
                    attach_execution_route_context(
                        details,
                        self.provider_id(),
                        self.route.entry.id.as_str(),
                        self.model_id(),
                        self.route.protocol_route.responses_recovery_policy(),
                    );
                }
                trace_processing_failure.observe_failure(finish_stage, &error);
                return Err(error);
            }
        };
        // toolCallCount is frozen from the exact completed response before the terminal durability gate.
        // toolCallCount 在终态可靠性闸门前从精确完成响应中冻结。
        let tool_call_count = final_response.tool_calls.len();
        // providerSource is extracted from the completed typed response before any telemetry exists.
        // providerSource 在任何遥测产生前从已完成的强类型响应提取。
        let provider_source =
            provider_v1_source_from_completed_response(&final_response).map_err(|error| {
                provider_failure(
                    self.provider_id(),
                    &format!("The completed Provider replay source is invalid: {error}"),
                )
            })?;
        let (completed_completion, completed_receiver) = provider_request_durability_barrier();
        self.live_sender
            .send(Ok(ProviderStreamDelta::RequestSucceeded {
                usage: final_usage,
                provider_source,
                tool_call_count,
                completion: completed_completion,
            }))
            .map_err(|_| {
                provider_failure(
                    self.provider_id(),
                    "The provider request stream closed before terminal persistence.",
                )
            })?;
        completed_receiver
            .await
            .map_err(|_| {
                provider_failure(
                    self.provider_id(),
                    "The host dropped the Provider terminal persistence barrier.",
                )
            })?
            .map_err(|error| {
                provider_failure(
                    self.provider_id(),
                    &format!(
                        "The Provider response completed but its terminal record could not be persisted: {error}"
                    ),
                )
            })?;
        self.credential.mark_request_succeeded();
        Ok(final_response)
    }
}

/// Attaches the exact resolved route to one structured execution failure.
/// 为一个结构化执行失败附加精确解析后的执行路由。
///
/// # Parameters
/// 参数
/// `error` is the normalized failure; the remaining values identify the immutable physical route.
/// `error` 是归一化失败；其余值标识不可变的物理执行路由。
fn attach_execution_route_context(
    error: &mut vulcan_ai_contract::AiExecutionError,
    provider_id: &str,
    entry_id: &str,
    model_id: &str,
    recovery_policy: vulcan_provider_adapters::ResponsesRecoveryPolicy,
) {
    error.provider_id = Some(provider_id.to_string());
    error.entry_id = Some(entry_id.to_string());
    error.model_id = Some(model_id.to_string());
    // AgentSpace reconciliation is classified only at the route-aware boundary; the generic
    // OpenAI decoder cannot authorize a provider-specific recovery state.
    // AgentSpace 对账仅在感知路由的边界分类；通用 OpenAI 解码器无权授予供应商专属恢复状态。
    if recovery_policy
        == vulcan_provider_adapters::ResponsesRecoveryPolicy::AgentSpaceResultAndUsageReconciliationV1
        && provider_id == "agentspace"
        && entry_id == "chatgpt-plan"
        && error.upstream_code.as_deref() == Some("reconciliation_required")
        && error.dispatch_outcome == vulcan_ai_contract::DispatchOutcome::Dispatched
    {
        error.subcategory = Some(String::from(
            vulcan_ai_contract::AGENTSPACE_RECONCILIATION_REQUIRED_SUBCATEGORY,
        ));
        error.terminal = true;
        error.retryable = false;
    }
}

#[cfg(test)]
mod tests;
