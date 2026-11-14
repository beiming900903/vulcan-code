//! Provider stream projection into the unique V1 session owner.
//! Provider 流到唯一 V1 会话所有者的投影。

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use sha2::{Digest, Sha256};
use vulcan_agent_runtime::agent_runtime::events::{
    PreparedPhysicalApi, ProviderRequestPreparationCandidate, RuntimeEvent,
    ToolExecutionRecoveryBoundary,
};
use vulcan_agent_runtime::agent_runtime::provider_request_bridge_conversation::build_core_conversation_snapshot;
use vulcan_agent_runtime::agent_runtime::provider_v1_bridge::{
    ProviderV1ApiTerminalFacts, ProviderV1ExternalResourceError, ProviderV1GeneratedResourceError,
    ProviderV1RequestFacts, ProviderV1TransportObservedFacts,
    provider_v1_external_refs_from_tool_result, provider_v1_generated_attachments_from_tool_result,
};
use vulcan_agent_runtime::agent_runtime::tool_runtime_bridge::{
    PreparedToolInvocationIdentity, persistent_tool_output_from_result,
};
use vulcan_ai_contract::{
    AiMessage, ContentItem, HOST_MESSAGE_GUIDANCE, MessageRole, ProviderReplayEnvelope,
    ProviderReplayProjection, ProviderReplayRoute, ResourceKind, ResourceReference,
    ResourceSourceKind, ToolCall as AiToolCall, ToolResult as AiToolResult,
    project_provider_replay,
};
use vulcan_change_set::{PersistentChangeSetError, persistent_change_set_from_host_result};
use vulcan_context::protocol::validate_session_conversation_message_identities;
use vulcan_context::{
    SessionConversationMessage, SessionConversationPayload, SessionConversationState,
    SessionConversationToolCall, SessionProviderContinuationState,
};
use vulcan_core::types::{
    ConversationAttachment, ConversationAttachmentKind, ProviderPromptLayers,
    ProviderRequestSnapshot as CoreProviderRequestSnapshot, ToolInvocationRequest,
    ToolInvocationResult,
};
#[cfg(test)]
use vulcan_core::types::{
    ConversationAttachmentDeliveryMode, ConversationMessage, ConversationPayload,
    ConversationSnapshot, ProviderContinuation,
};
use vulcan_session_core::v1::context::{
    LockedBaselineProjection, LockedBaselineSegmentInput, LockedPromptTarget, LockedSource,
    LockedSourceKind,
};
#[cfg(test)]
use vulcan_session_core::v1::document::TurnSuccessor;
use vulcan_session_core::v1::document::{
    ApiCompletion, ApiGroupCompletionReceipt, ToolCompletion, ToolPreparation,
    TurnCompletionTransition,
};
use vulcan_session_core::v1::errors::DomainError;
#[cfg(test)]
use vulcan_session_core::v1::event::ContinuationEligibility;
use vulcan_session_core::v1::event::{
    ApiCallStatus, AssistantSemanticStatus, GeneratedResourceKind, GeneratedResourceRef,
    ProviderProjectionProof, ToolCallStatus, ToolPayload,
};
use vulcan_session_core::v1::ids::{ApiCallId, RequestStreamId, ResourceId, Seq, ToolCallId};
use vulcan_session_core::v1::projection::{ProjectedItem, ProjectedSystemRole};
use vulcan_workbench_model::SessionId;
use vulcan_workspace_store::workspace_store::{
    ProjectResourceCatalog, ProjectResourceCatalogError, ProjectResourceCatalogRegistry,
    load_project_resource_catalog,
};
#[cfg(test)]
use vulcan_workspace_store::workspace_store::{
    ProjectResourceResolution, resolve_project_resource,
};

use crate::session_runtime_registry_v1::{
    SessionFlushReason, SessionHandle, SessionMutationAdmission, SessionMutationChange,
    SessionMutationReceipt, SessionPreparedPhysicalRequest, SessionProviderProjectionSnapshot,
    SessionRuntimeError, SuccessfulTurnSettlement,
};
use crate::state::SharedServiceBridgeState;

mod answer_code;
mod compaction_source;
mod context_injection;
mod host_guidance;
#[cfg(test)]
mod memory_acceptance;
#[cfg(test)]
pub(crate) use memory_acceptance::{
    host_guided_locked_baseline, rebuild_memory_acceptance_request_for_handle,
};
mod media;
pub(crate) use media::{conversation_image_compaction_required, frozen_prompt_media_projection};

/// Returns the canonical controller identifier for a zero-based V1 projection item position.
/// 返回从零开始的 V1 投影条目位置对应的规范控制器标识。
/// The same formatter serves controller history, exact source lookup and physical memory anchors.
/// 同一格式化函数服务于控制器历史、准确来源查询和物理记忆锚点。
fn provider_projection_message_id(index: usize) -> String {
    format!("v1-provider-message-{}", index + 1)
}

/// Adds the fixed host guidance while the initial locked baseline is established.
/// 在初始锁定基线建立时加入固定宿主指引。
///
/// Registered initialized baselines are returned byte-for-byte; upgrades use a separate
/// append-only context producer instead of rewriting this one-time baseline fact.
/// 已登记的初始化基线按字节原样返回；升级使用独立追加式上下文生产者，不重写一次性基线事实。
fn initial_locked_baseline_with_host_guidance(
    baseline: &LockedBaselineProjection,
) -> Result<LockedBaselineProjection, SessionRuntimeError> {
    if host_guidance::baseline_version(baseline)
        .map_err(provider_projection_runtime_error)?
        .is_some()
    {
        return Ok(baseline.clone());
    }
    let mut segments = baseline
        .locked_sources()
        .iter()
        .map(|source| LockedBaselineSegmentInput {
            kind: source.kind,
            node_id: source.node_id.clone(),
            target: source.target,
            source_label: source.source_label.clone(),
            source_id: source.source_id.clone(),
            content: source.content.clone(),
        })
        .collect::<Vec<_>>();
    // Group system sources before developer and project context in newly locked generations.
    // 在新锁定代际中，将系统来源排在开发者指令和项目上下文之前。
    let system_end = segments
        .iter()
        .position(|source| source.target != LockedPromptTarget::System)
        .unwrap_or(segments.len());
    segments.insert(
        system_end,
        LockedBaselineSegmentInput {
            kind: LockedSourceKind::BaselineHooks,
            node_id: String::from("host-message-guidance"),
            target: LockedPromptTarget::System,
            source_label: Some(String::from("host_message_guidance")),
            source_id: Some(String::from("v1:host_message_guidance")),
            content: String::from(HOST_MESSAGE_GUIDANCE),
        },
    );
    LockedBaselineProjection::project(segments).map_err(provider_projection_runtime_error)
}

/// Projects one ordered resource list into canonical controller attachments without transport state.
/// 将一组有序资源投影为不含传输状态的规范控制器附件。
///
/// # Parameters
/// 参数
/// `catalog` owns immutable project resource metadata.
/// `catalog` 持有不可变项目资源元数据。
/// `resource_ids` preserves the occurrence order recorded on the V1 user input.
/// `resource_ids` 保留 V1 用户输入记录的资源出现顺序。
///
/// # Returns
/// 返回值
/// Returns canonical attachments in occurrence order, or an explicit missing-catalog-identity error.
/// 按出现顺序返回规范附件；目录身份缺失时返回明确错误。
fn provider_history_attachments_from_resource_ids(
    catalog: &ProjectResourceCatalog,
    resource_ids: &[ResourceId],
) -> Result<Vec<ConversationAttachment>, String> {
    resource_ids
        .iter()
        .map(|resource_id| {
            // entry is the unique immutable catalog identity referenced by this V1 occurrence.
            // entry 是本次 V1 出现所引用的唯一不可变目录身份。
            let entry = catalog
                .entries
                .iter()
                .find(|entry| entry.resource_id == *resource_id)
                .ok_or_else(|| {
                    format!(
                        "V1 Provider history references missing project resource `{}`.",
                        resource_id.value()
                    )
                })?;
            // mime is authoritative for the stable controller modality and never inferred from a filename.
            // mime 是稳定控制器模态的权威来源，绝不从文件名推断。
            let mime = entry
                .mime
                .clone()
                .unwrap_or_else(|| String::from("application/octet-stream"));
            // kind reuses the physical preparation mapper so PDF and generic-file semantics cannot drift.
            // kind 复用物理准备映射器，使 PDF 与通用文件语义不会漂移。
            let kind = media::attachment_kind(resource_kind_from_mime(&mime), Some(&mime));
            // asset_id preserves logical V1 identity without exposing one host path or Provider upload reference.
            // asset_id 保留逻辑 V1 身份，同时不暴露宿主路径或 Provider 上传引用。
            let asset_id = vulcan_agent_runtime::attachment_asset_index::project_resource_asset_id(
                resource_id.value(),
            );
            Ok(ConversationAttachment {
                kind,
                file_name: entry.original_name.clone(),
                local_path: String::new(),
                resource_path: Some(entry.entity_relative_path.clone()),
                source_path: None,
                asset_id: Some(asset_id),
                byte_len: Some(entry.size),
                source_sha256: Some(entry.sha256.clone()),
                delivery_mode: None,
                preview_local_path: None,
                mime_type: mime,
            })
        })
        .collect()
}

/// Projects the complete ordered V1 Provider history into one canonical controller state.
/// 将完整有序的 V1 Provider 历史投影为一份规范控制器状态。
///
/// # Parameters
/// 参数
/// `snapshot` is the durable V1 projection frontier shared with physical request preparation.
/// `snapshot` 是与物理请求准备共享的可靠 V1 投影前沿。
/// `resolve_attachments` resolves each user-input occurrence without reading or uploading media bytes.
/// `resolve_attachments` 在不读取或上传媒体字节的情况下解析每次用户输入的附件。
///
/// # Returns
/// 返回值
/// Returns the complete lossless controller window, or the first explicit projection error.
/// 返回完整无损的控制器窗口；投影失败时返回首个明确错误。
fn project_provider_controller_state<F>(
    snapshot: &SessionProviderProjectionSnapshot,
    mut resolve_attachments: F,
) -> Result<SessionConversationState, String>
where
    F: FnMut(usize, &[ResourceId]) -> Result<Vec<ConversationAttachment>, String>,
{
    // messages preserves the exact V1 Provider-visible ordering and deterministic position identities.
    // messages 保留精确 V1 Provider 可见顺序及确定性位置身份。
    let mut messages = Vec::with_capacity(snapshot.items.len());
    // continuation is rebuilt only from the latest durable assistant provider source.
    // continuation 仅从最近一次可靠助手 Provider 来源重建。
    let mut continuation = SessionProviderContinuationState::default();
    for (index, item) in snapshot.items.iter().enumerate() {
        // message_id uses the same stable position identity as physical request reconstruction.
        // message_id 使用与物理请求重建相同的稳定位置身份。
        let message_id = provider_projection_message_id(index);
        // payload omits system/developer layers because those remain request prompt layers, not controller messages.
        // payload 会跳过 system/developer 层，因为它们属于请求提示词层而非控制器消息。
        let payload = match item {
            ProjectedItem::SystemText {
                role: ProjectedSystemRole::HiddenUserShell,
                text,
                ..
            } => Some(SessionConversationPayload::UserText {
                text: text.clone(),
                attachments: Vec::new(),
            }),
            ProjectedItem::SystemText { .. } => None,
            ProjectedItem::UserInput { text, resource_ids } => {
                // attachments retain logical ResourceId identity while excluding every request-local delivery decision.
                // attachments 保留逻辑 ResourceId 身份，同时排除全部请求局部投递决策。
                let attachments = resolve_attachments(index, resource_ids)?;
                Some(SessionConversationPayload::UserText {
                    text: text.clone(),
                    attachments,
                })
            }
            ProjectedItem::AssistantText {
                text,
                reasoning,
                tool_calls,
                provider_source,
            } => {
                // controller_tool_calls preserves the Provider-ordered structured invocation list.
                // controller_tool_calls 保留 Provider 有序的结构化调用列表。
                let mut controller_tool_calls = Vec::with_capacity(tool_calls.len());
                for tool_call in tool_calls {
                    // call_id selects the provider-native identity when present and the stable local identity otherwise.
                    // call_id 在存在时选择 Provider 原生身份，否则选择稳定本地身份。
                    let call_id = provider_wire_tool_call_id(
                        tool_call.tool_call_id,
                        tool_call.provider_tool_call_id.as_deref(),
                    );
                    // arguments is the lossless JSON representation of the validated V1 tool payload.
                    // arguments 是已校验 V1 工具载荷的无损 JSON 表示。
                    let arguments = serde_json::to_value(&tool_call.input)
                        .map_err(|error| format!("Serialize V1 tool input failed: {error}"))?;
                    controller_tool_calls.push(SessionConversationToolCall {
                        id: format!("v1-tool-item-{}", tool_call.tool_call_id.value()),
                        tool_call_id: Some(call_id),
                        tool_name: tool_call.tool_name.clone(),
                        arguments,
                        argument_parse_error: None,
                    });
                }
                // provider_message_state retains only the typed replay envelope declared by the durable source.
                // provider_message_state 仅保留可靠来源声明的强类型重放信封。
                let provider_message_state = provider_source
                    .as_ref()
                    .and_then(|source| source.replay.as_ref())
                    .map(serde_json::to_value)
                    .transpose()
                    .map_err(|error| {
                        format!("Serialize V1 provider replay state failed: {error}")
                    })?;
                if let Some(source) = provider_source {
                    continuation.provider_id = Some(source.provider.clone());
                    continuation.protocol_profile_id = source.profile.clone();
                    continuation.model_id = Some(source.model.clone());
                    continuation.last_provider_message_id = source
                        .message_id
                        .clone()
                        .or_else(|| source.response_id.clone());
                    continuation.last_local_message_id = Some(message_id.clone());
                    continuation.context_rewrite_fingerprint =
                        Some(source.fingerprint_sha256.clone());
                }
                Some(SessionConversationPayload::Assistant {
                    text: text.clone(),
                    reasoning: reasoning.clone().unwrap_or_default(),
                    provider_message_id: provider_source.as_ref().and_then(|source| {
                        source
                            .message_id
                            .clone()
                            .or_else(|| source.response_id.clone())
                    }),
                    provider_message_state,
                    usage: None,
                    tool_calls: controller_tool_calls,
                })
            }
            ProjectedItem::ToolResult {
                tool_call_id,
                provider_tool_call_id,
                status,
                output,
                ..
            } => {
                // output is serialized once into the canonical tool-result text expected by the controller bridge.
                // output 仅序列化一次，成为控制器桥接所需的规范工具结果文本。
                let output = serde_json::to_value(output)
                    .map_err(|error| format!("Serialize V1 tool result failed: {error}"))?;
                Some(SessionConversationPayload::ToolResult {
                    tool_call_id: Some(provider_wire_tool_call_id(
                        *tool_call_id,
                        provider_tool_call_id.as_deref(),
                    )),
                    is_error: *status != ToolCallStatus::Completed,
                    text: serde_json::to_string(&output)
                        .map_err(|error| format!("Encode V1 tool result failed: {error}"))?,
                })
            }
            ProjectedItem::SubagentReturnText { text } => {
                Some(SessionConversationPayload::UserText {
                    text: text.clone(),
                    attachments: Vec::new(),
                })
            }
        };
        if let Some(payload) = payload {
            messages.push(SessionConversationMessage {
                id: message_id,
                payload,
            });
        }
    }
    // state is validated once through the canonical session protocol before either caller can observe it.
    // state 在任一调用方可观察前通过规范会话协议完成一次校验。
    let state = SessionConversationState {
        messages,
        records: Vec::new(),
        continuation,
    };
    validate_session_conversation_message_identities(&state.messages)
        .map_err(|error| format!("Validate V1 controller projection failed: {error}"))?;
    Ok(state)
}

/// Resolves the deterministic canonical controller message id for one exact V1 user-input source.
/// 为一条准确 V1 用户输入来源解析确定性的规范控制器消息标识。
///
/// # Parameters
/// 参数
/// `snapshot` is the authoritative V1 projection and `input_seq` is the durable input event seq.
/// `snapshot` 是权威 V1 投影，`input_seq` 是持久输入事件序号。
///
/// # Returns
/// 返回值
/// The exact position-derived user-message id, or an explicit ambiguity/type error.
/// 返回由准确位置派生的用户消息标识；来源歧义或类型错误时返回明确错误。
pub(crate) fn provider_projection_user_message_id_for_source(
    snapshot: &SessionProviderProjectionSnapshot,
    input_seq: Seq,
) -> Result<String, String> {
    if snapshot.items.len() != snapshot.sourced_items.len()
        || snapshot
            .items
            .iter()
            .zip(&snapshot.sourced_items)
            .any(|(item, sourced)| item != &sourced.item)
    {
        return Err(String::from(
            "V1 semantic items differ from their exact source projection.",
        ));
    }
    let mut matches = snapshot
        .sourced_items
        .iter()
        .enumerate()
        .filter(|(_, sourced)| {
            sourced.source_from_seq.value() <= input_seq.value()
                && input_seq.value() <= sourced.source_to_seq.value()
        });
    let Some((index, sourced)) = matches.next() else {
        return Err(format!(
            "V1 user input seq {} is absent from the Provider projection.",
            input_seq.value()
        ));
    };
    if matches.next().is_some() {
        return Err(format!(
            "V1 user input seq {} maps to multiple Provider projection items.",
            input_seq.value()
        ));
    }
    if !matches!(sourced.item, ProjectedItem::UserInput { .. }) {
        return Err(format!(
            "V1 source seq {} does not project to a user input.",
            input_seq.value()
        ));
    }
    Ok(provider_projection_message_id(index))
}

/// Rebuilds the complete canonical controller state from the durable V1 projection and resource catalog.
/// 从可靠 V1 投影与资源目录重建完整规范控制器状态。
///
/// # Parameters
/// 参数
/// `snapshot` is the serialized V1 frontier captured for one Continue launch.
/// `snapshot` 是为一次 Continue 启动捕获的串行化 V1 前沿。
///
/// # Returns
/// 返回值
/// Returns the complete controller state without transport decisions, or an explicit catalog/projection error.
/// 返回不含传输决策的完整控制器状态；失败时返回明确目录或投影错误。
pub(crate) fn provider_projection_controller_state(
    snapshot: &SessionProviderProjectionSnapshot,
) -> Result<SessionConversationState, String> {
    // has_resources prevents an unrelated empty-history projection from requiring project catalog I/O.
    // has_resources 避免无资源历史投影无关地要求项目资源目录 I/O。
    let has_resources = snapshot.items.iter().any(
        |item| matches!(item, ProjectedItem::UserInput { resource_ids, .. } if !resource_ids.is_empty()),
    );
    // catalog is loaded once only when the V1 projection declares logical resources.
    // catalog 仅在 V1 投影声明逻辑资源时加载一次。
    let catalog = has_resources
        .then(|| load_project_resource_catalog(&snapshot.project_directory))
        .transpose()
        .map_err(|error| format!("Load V1 Provider history resource catalog failed: {error}"))?;
    project_provider_controller_state(snapshot, |_, resource_ids| {
        if resource_ids.is_empty() {
            return Ok(Vec::new());
        }
        // catalog must exist because has_resources was derived from this exact immutable snapshot.
        // catalog 必然存在，因为 has_resources 来自同一份不可变快照。
        let catalog = catalog.as_ref().ok_or_else(|| {
            String::from(
                "V1 Provider history resource catalog was not loaded for declared resources.",
            )
        })?;
        provider_history_attachments_from_resource_ids(catalog, resource_ids)
    })
}

/// Acquires the optional V1 sink once for the complete lifetime of one stream task.
/// 为一个流任务的完整生命周期一次性获取可选 V1 sink。
///
/// # Parameters
/// 参数
/// `state` owns the process-local registry and `session_id` selects the exact registered owner.
/// `state` 持有进程内注册表，`session_id` 选择精确的已注册所有者。
///
/// # Returns
/// 返回值
/// Returns `None` only when the session has not entered V1 admission yet; every registry failure
/// is explicit so an admitted V1 session never falls back to the legacy transcript.
/// 仅当会话尚未进入 V1 准入时返回 `None`；全部注册表失败均显式返回，使已准入 V1 会话绝不降级到旧转录。
pub(crate) fn provider_v1_stream_sink_for_session(
    state: &SharedServiceBridgeState,
    session_id: SessionId,
) -> Result<Option<ProviderV1StreamSink>, String> {
    let (registry, project_resource_catalog_registry, answer_code_root) = {
        let guard = state.lock().map_err(|_| {
            String::from("Service state lock poisoned while acquiring the V1 session.")
        })?;
        (
            guard.session_runtime_registry.clone(),
            Arc::clone(&guard.project_resource_catalog_registry),
            guard.workspace_snapshot.as_ref().map(|workspace| {
                PathBuf::from(crate::workspace_path_for_session(workspace, session_id))
            }),
        )
    };
    match registry.get(session_id) {
        Ok(handle) => {
            let sink = ProviderV1StreamSink::new(handle, project_resource_catalog_registry);
            Ok(Some(match answer_code_root {
                Some(root) => sink.with_answer_code_root(root),
                None => sink,
            }))
        }
        Err(SessionRuntimeError::SessionNotRegistered(_)) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

/// Stream-local V1 projection state that retains one exact session handle for its whole lifetime.
/// 在完整生命周期内保留同一精确会话句柄的流局部 V1 投影状态。
pub(crate) struct ProviderV1StreamSink {
    /// Optional pending route authorized by the exact manual summary owner.
    /// 由精确手动摘要所有者授权的可选待生效路由。
    manual_compaction_request:
        Option<vulcan_session_core::v1::context_maintenance::ManualCompactionPendingRequest>,
    /// Frozen planning source and current local compaction attempt, absent for business execution.
    /// 冻结的规划来源及当前本地压缩尝试；业务执行时不存在。
    compaction_source: Option<compaction_source::CompactionSourceWindow>,
    /// Unique V1 owner retained by this exact stream task.
    /// 当前精确流任务保留的唯一 V1 所有者。
    handle: Arc<SessionHandle>,
    /// Project-level owner used to register generated resources before tool completion is durable.
    /// 在工具终态可靠落盘前注册生成资源的项目级 owner。
    project_resource_catalog_registry: Arc<ProjectResourceCatalogRegistry>,
    /// Sole active physical Provider request projected by this stream.
    /// 当前流投影的唯一活动物理 Provider 请求。
    active: Option<ActiveProviderV1Projection>,
    /// Durable terminal receipt of the last physical API completed by this stream.
    /// 当前流最后一次已完成物理 API 的可靠终态回执。
    api_terminal_completion: Option<SessionMutationReceipt<ApiGroupCompletionReceipt>>,
    /// Whether that durable terminal already contains the stream's failure fact.
    /// 该可靠终态是否已经包含当前流的失败事实。
    api_terminal_failure_persisted: bool,
    /// Durable logical-Turn completion produced by the final tool-free API of this stream.
    /// 当前流最终无工具 API 产生的可靠逻辑回合完成凭据。
    turn_completion: Option<SessionMutationReceipt<TurnCompletionTransition>>,
    /// Exact completed assistant semantic of the last physical API.
    /// 最后一次物理 API 的精确已完成 assistant 语义。
    completed_assistant_text: Option<ProviderV1CompletedAssistantText>,
    /// Exact locked baseline used by the last completed physical API.
    /// 最后一次已完成物理 API 使用的精确锁定基线。
    completed_locked_baseline: Option<LockedBaselineProjection>,
    /// Workspace that resolves relative `ui-code` paths, absent when the session has none.
    /// 解析相对 `ui-code` 路径的工作区；会话没有工作区时不存在。
    answer_code_root: Option<PathBuf>,
}

/// Exact persisted assistant semantic emitted by one completed physical API.
/// 一次已完成物理 API 产生的精确持久化 assistant 语义。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderV1CompletedAssistantText {
    /// Session-local physical API identity that owns the semantic.
    /// 拥有该语义的会话本地物理 API 身份。
    pub(crate) api_call_id: ApiCallId,
    /// Exact main-ledger seq of the completed assistant event.
    /// 已完成 assistant 事件在主账本中的精确 seq。
    pub(crate) seq: Seq,
    /// Exact persisted assistant body.
    /// 精确持久化的 assistant 正文。
    pub(crate) text: String,
}

/// Durable V1 facts transferred exactly once from the stream sink to terminal handling.
/// 从流 sink 向终态处理精确转移一次的可靠 V1 事实。
#[derive(Debug, Default)]
pub(crate) struct ProviderV1FinishedDurability {
    /// Exact ApiStats mutation proving that the final physical API is durable.
    /// 证明最终物理 API 已可靠落盘的精确 ApiStats 修改。
    pub(crate) api_terminal: Option<SessionMutationReceipt<ApiGroupCompletionReceipt>>,
    /// Whether the durable API terminal already persisted a controlled failure.
    /// 可靠 API 终态是否已经持久化一条受控失败。
    pub(crate) api_terminal_failure_persisted: bool,
    /// Exact logical-Turn completion and successor selection, when this API closed the Turn.
    /// 当本次 API 闭合逻辑回合时产生的精确回合完成与后继选择。
    pub(super) turn_completion: Option<SessionMutationReceipt<TurnCompletionTransition>>,
    /// Exact completed assistant semantic of the final physical API, when present.
    /// 最终物理 API 的精确已完成 assistant 语义（如存在）。
    pub(crate) completed_assistant_text: Option<ProviderV1CompletedAssistantText>,
    /// Exact locked baseline used by the final physical API, when present.
    /// 最终物理 API 使用的精确锁定基线（如存在）。
    pub(crate) locked_baseline: Option<LockedBaselineProjection>,
}

/// Exact durable result of applying one tool execution boundary.
/// 应用一个工具执行边界后的精确可靠结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProviderV1ToolBoundaryOutcome {
    /// A non-batch boundary was durably applied without a new execution identity payload.
    /// 非批次边界已可靠应用，且没有新的执行身份载荷。
    Applied,
    /// The prepared batch allocated exact local identities for every request stream.
    /// 已准备批次为每个请求流分配了精确本地身份。
    BatchPrepared(Vec<PreparedToolInvocationIdentity>),
}

/// Mutable semantic state for one active physical Provider request.
/// 单次活动物理 Provider 请求的可变语义状态。
struct ActiveProviderV1Projection {
    /// Durable V1 API identity allocated by the session document.
    /// 会话文档分配的可靠 V1 API 身份。
    api_call_id: ApiCallId,
    /// Immutable typed Provider request facts used for terminal identity validation.
    /// 用于终态身份校验的不可变强类型 Provider 请求事实。
    request_facts: ProviderV1RequestFacts,
    /// Exact validated locked baseline used to build this physical request.
    /// 构建当前物理请求所使用的精确已校验锁定基线。
    locked_baseline: LockedBaselineProjection,
    /// Exact project resource directory derived from the immutable session locator.
    /// 从不可变会话定位器派生的精确项目资源目录。
    project_directory: PathBuf,
    /// Complete reasoning text accumulated from ordered deltas.
    /// 从有序增量累积的完整思考正文。
    reasoning_text: String,
    /// Main-ledger seq of the reasoning semantic, once allocated.
    /// 推理语义分配后的主账本 seq。
    reasoning_seq: Option<Seq>,
    /// Whether the reasoning semantic already reached a terminal display status.
    /// 思考语义是否已经到达展示终态。
    reasoning_sealed: bool,
    /// Whether any reasoning delta was a provider summary; sticky so it is never replayed.
    /// 是否有任一思考增量为供应商摘要；一经置位即保持，使其绝不被重放。
    reasoning_display_only: bool,
    /// Complete assistant text accumulated from ordered deltas.
    /// 从有序增量累积的完整 assistant 正文。
    assistant_text: String,
    /// Main-ledger seq of the assistant semantic, once allocated.
    /// assistant 语义分配后的主账本 seq。
    assistant_text_seq: Option<Seq>,
    /// Provider terminal retained while a real tool batch remains pending.
    /// 真实工具批次仍待处理时保留的 Provider 终态。
    pending_terminal: Option<ProviderV1ApiTerminalFacts>,
    /// Provider-native tool identities durably prepared for this API in original batch order.
    /// 按原始批次顺序为当前 API 可靠准备的 Provider 原生工具身份。
    prepared_tools: BTreeMap<String, ActiveProviderV1Tool>,
    /// Whether the one declared Provider tool batch has crossed its durable Prepared boundary.
    /// 已声明的唯一 Provider 工具批次是否已经跨过可靠 Prepared 边界。
    tool_batch_prepared: bool,
}

/// Stream-local validation state for one durably represented V1 tool event.
/// 一条已可靠表达的 V1 工具事件的流局部校验状态。
struct ActiveProviderV1Tool {
    /// Exact Provider request retained for Started-boundary equality checks.
    /// 为 Started 边界相等性校验保留的精确 Provider 请求。
    request: ToolInvocationRequest,
    /// Exact resolved tool binding fingerprint retained at batch preparation.
    /// 在批次准备时保留的精确已解析工具绑定指纹。
    binding_fingerprint: String,
    /// Session-local identity allocated on the durable Prepared event.
    /// 在可靠 Prepared 事件上分配的会话本地身份。
    tool_call_id: ToolCallId,
    /// Current lifecycle state mirrored from the authoritative session event.
    /// 从权威会话事件镜像的当前生命周期状态。
    state: ProviderV1ToolState,
}

/// Valid stream-local transitions for one V1 tool event.
/// 一条 V1 工具事件允许的流局部转换。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderV1ToolState {
    /// The request is durable and its body has not started.
    /// 请求已经可靠落盘且主体尚未启动。
    Prepared,
    /// The tool body has started and no terminal result is durable yet.
    /// 工具主体已经启动且尚无可靠终态结果。
    Running,
    /// The exact terminal result is durable.
    /// 精确终态结果已经可靠落盘。
    Finished,
}

/// Outcome of accepting one typed Provider terminal boundary.
/// 接纳一个强类型 Provider 终态边界的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderV1TerminalOutcome {
    /// The physical API group was durably closed.
    /// 物理 API 组已经可靠闭合。
    Completed,
    /// API statistics are deferred until the declared tool batch completes.
    /// API 统计延迟到已声明工具批次完成后闭合。
    AwaitingTools,
}

/// Explicit failures emitted by the Provider-to-V1 projection boundary.
/// Provider 到 V1 投影边界发出的显式失败。
#[derive(Debug)]
pub(crate) enum ProviderV1SinkError {
    /// The V1 session owner rejected a mutation or durable flush.
    /// V1 会话所有者拒绝了一次修改或可靠 flush。
    Runtime(SessionRuntimeError),
    /// The host clock could not provide a V1 event timestamp.
    /// 宿主时钟无法提供 V1 事件时间戳。
    Timestamp(String),
    /// The stream adapter received an event without the required typed tool boundary.
    /// 流适配器收到的事件缺少必需的强类型工具边界。
    MissingToolExecutionBoundary,
    /// Durable tool completion could not update its non-transcript lifecycle surfaces.
    /// 可靠工具完成后无法更新其非转录生命周期界面。
    ToolLifecycleProjection(String),
    /// A new request arrived while another physical request remained active.
    /// 另一物理请求仍活动时到达了新请求。
    RequestAlreadyActive,
    /// A semantic or terminal event arrived without an active physical request.
    /// 没有活动物理请求时到达了语义或终态事件。
    RequestNotActive,
    /// Started telemetry did not carry the required typed V1 request facts.
    /// Started 遥测没有携带必需的强类型 V1 请求事实。
    MissingTypedRequestFacts,
    /// A legacy Started durability boundary attempted to create a second request-start owner.
    /// 旧 Started 可靠边界试图创建第二个请求启动所有者。
    UnexpectedRequestStartDurabilityBoundary,
    /// Completed telemetry did not carry the required typed V1 terminal facts.
    /// Completed 遥测没有携带必需的强类型 V1 终态事实。
    MissingTypedTerminalFacts,
    /// A terminal fact did not identify the active physical request exactly.
    /// 终态事实没有精确标识当前活动物理请求。
    RequestIdentityMismatch,
    /// A second terminal fact arrived while a tool-bearing terminal was already retained.
    /// 已保留含工具终态时又到达了第二个终态事实。
    TerminalAlreadyPending,
    /// A tool boundary arrived before the successful Provider terminal declared its batch.
    /// 成功 Provider 终态声明工具批次前到达了工具边界。
    ToolTerminalNotPending,
    /// A tool-bearing successful terminal omitted its typed Provider replay source.
    /// 含工具的成功终态缺少强类型 Provider 重放来源。
    ToolProviderSourceMissing,
    /// The Prepared batch size differs from the exact Provider terminal declaration.
    /// Prepared 批次数量与精确 Provider 终态声明不一致。
    ToolBatchSizeMismatch {
        /// Tool count declared by the Provider terminal.
        /// Provider 终态声明的工具数量。
        declared: usize,
        /// Tool count carried by the Prepared boundary.
        /// Prepared 边界携带的工具数量。
        actual: usize,
    },
    /// A Prepared batch repeats one Provider-native tool-call identity.
    /// Prepared 批次重复了一个 Provider 原生工具调用身份。
    DuplicateProviderToolCallId(String),
    /// One Provider stream identity could not enter the strict V1 local-id domain.
    /// 一个 Provider 流身份无法进入严格的 V1 本地身份域。
    InvalidRequestStreamIdentity {
        /// Exact rejected Provider stream identity.
        /// 被拒绝的精确 Provider 流身份。
        request_stream_id: String,
        /// Exact Core validation failure.
        /// Core 返回的精确校验失败。
        error: DomainError,
    },
    /// One prepared tool payload failed strict Core validation before mutation.
    /// 一个待准备工具载荷在修改前未通过严格 Core 校验。
    InvalidToolPayload {
        /// Exact tool name owning the rejected payload.
        /// 拥有被拒载荷的精确工具名。
        tool_name: String,
        /// Exact Core validation failure.
        /// Core 返回的精确校验失败。
        error: DomainError,
    },
    /// A Started or Finished boundary references no durably prepared tool.
    /// Started 或 Finished 边界引用了不存在可靠 Prepared 前驱的工具。
    ToolNotPrepared(String),
    /// A tool boundary does not match the exact request or legal lifecycle state.
    /// 工具边界与精确请求或合法生命周期状态不一致。
    ToolBoundaryMismatch(String),
    /// The canonical ChangeSet converter rejected a declared change-set.
    /// canonical ChangeSet 转换器拒绝了一份已声明的 change-set。
    ChangeSet(PersistentChangeSetError),
    /// A stateful tool returned an invalid navigation-resource contract.
    /// 有状态工具返回了无效的导航资源契约。
    ExternalResource(ProviderV1ExternalResourceError),
    /// A provider-generated attachment host contract is malformed.
    /// Provider 生成附件宿主契约畸形。
    GeneratedResource(ProviderV1GeneratedResourceError),
    /// The project resource owner rejected generated-attachment registration.
    /// 项目资源 owner 拒绝了生成附件注册。
    ResourceCatalog(ProjectResourceCatalogError),
    /// Blocking generated-resource registration could not return to the stream task.
    /// 阻塞式生成资源注册无法返回流任务。
    GeneratedResourceWorker(String),
    /// A generated attachment exposed a preview path without a typed preview resource contract.
    /// 生成附件暴露了预览路径，但缺少强类型预览资源契约。
    UntypedGeneratedPreview {
        /// Exact attachment position in producer order.
        /// 生产者顺序中的精确附件位置。
        attachment_index: usize,
        /// Stable display file name.
        /// 稳定显示文件名。
        file_name: String,
    },
    /// Project registration returned a count different from the producer attachment count.
    /// 项目注册返回的数量与生产者附件数量不同。
    GeneratedResourceCountMismatch {
        /// Number of canonical generated attachments.
        /// 规范生成附件数量。
        attachments: usize,
        /// Number of project resource ids returned by the owner.
        /// 项目 owner 返回的资源 ID 数量。
        resource_ids: usize,
    },
}

impl fmt::Display for ProviderV1SinkError {
    /// Formats one stable service-boundary failure.
    /// 格式化一条稳定的服务边界失败。
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Runtime(error) => write!(formatter, "{error}"),
            Self::Timestamp(error) => write!(formatter, "V1 timestamp acquisition failed: {error}"),
            Self::MissingToolExecutionBoundary => formatter.write_str("The V1 tool event has no typed execution boundary."),
            Self::ToolLifecycleProjection(error) => write!(formatter, "V1 tool lifecycle projection failed: {error}"),
            Self::RequestAlreadyActive => {
                formatter.write_str("A V1 Provider request is already active.")
            }
            Self::RequestNotActive => formatter.write_str("No V1 Provider request is active."),
            Self::MissingTypedRequestFacts => {
                formatter.write_str("The Provider Started boundary has no typed V1 request facts.")
            }
            Self::UnexpectedRequestStartDurabilityBoundary => formatter.write_str(
                "A Provider Started durability boundary cannot replace the committed V1 preparation permit.",
            ),
            Self::MissingTypedTerminalFacts => formatter
                .write_str("The Provider Completed boundary has no typed V1 terminal facts."),
            Self::RequestIdentityMismatch => formatter
                .write_str("The V1 Provider terminal does not match the active physical request."),
            Self::TerminalAlreadyPending => {
                formatter.write_str("A V1 Provider terminal is already awaiting tools.")
            }
            Self::ToolTerminalNotPending => formatter
                .write_str("The V1 tool boundary arrived before its successful Provider terminal."),
            Self::ToolProviderSourceMissing => formatter
                .write_str("The V1 tool-bearing Provider terminal has no typed replay source."),
            Self::ToolBatchSizeMismatch { declared, actual } => write!(
                formatter,
                "The V1 Prepared tool count `{actual}` does not match Provider terminal count `{declared}`."
            ),
            Self::DuplicateProviderToolCallId(tool_call_id) => write!(
                formatter,
                "The V1 Prepared batch repeats Provider tool call `{tool_call_id}`."
            ),
            Self::InvalidRequestStreamIdentity {
                request_stream_id,
                error,
            } => write!(
                formatter,
                "The V1 request stream identity `{request_stream_id}` is invalid: {error}"
            ),
            Self::InvalidToolPayload { tool_name, error } => write!(
                formatter,
                "The V1 prepared payload for tool `{tool_name}` is invalid: {error}"
            ),
            Self::ToolNotPrepared(tool_call_id) => write!(
                formatter,
                "The V1 tool boundary references unprepared Provider tool call `{tool_call_id}`."
            ),
            Self::ToolBoundaryMismatch(tool_call_id) => write!(
                formatter,
                "The V1 tool boundary does not match the prepared lifecycle for `{tool_call_id}`."
            ),
            Self::ChangeSet(error) => {
                write!(formatter, "The V1 tool ChangeSet is invalid: {error}")
            }
            Self::ExternalResource(error) => {
                write!(
                    formatter,
                    "The V1 tool external resource is invalid: {error}"
                )
            }
            Self::GeneratedResource(error) => {
                write!(formatter, "The V1 generated-resource contract is invalid: {error}")
            }
            Self::ResourceCatalog(error) => {
                write!(formatter, "The V1 project resource registration failed: {error}")
            }
            Self::GeneratedResourceWorker(error) => write!(
                formatter,
                "The V1 generated-resource registration worker failed: {error}"
            ),
            Self::UntypedGeneratedPreview {
                attachment_index,
                file_name,
            } => write!(
                formatter,
                "Generated attachment {attachment_index} (`{file_name}`) has an untyped preview path"
            ),
            Self::GeneratedResourceCountMismatch {
                attachments,
                resource_ids,
            } => write!(
                formatter,
                "Generated attachment count `{attachments}` does not match registered resource count `{resource_ids}`"
            ),
        }
    }
}

impl std::error::Error for ProviderV1SinkError {}

impl From<SessionRuntimeError> for ProviderV1SinkError {
    /// Preserves one structured runtime failure at the stream boundary.
    /// 在流边界保留一条结构化运行时失败。
    fn from(error: SessionRuntimeError) -> Self {
        Self::Runtime(error)
    }
}

impl From<PersistentChangeSetError> for ProviderV1SinkError {
    /// Preserves one canonical ChangeSet conversion failure without fallback.
    /// 在不降级的情况下保留一条 canonical ChangeSet 转换失败。
    fn from(error: PersistentChangeSetError) -> Self {
        Self::ChangeSet(error)
    }
}

impl From<ProviderV1ExternalResourceError> for ProviderV1SinkError {
    /// Preserves one exact stateful-tool resource failure without parsing display output.
    /// 在不解析展示输出的情况下保留一条精确有状态工具资源失败。
    fn from(error: ProviderV1ExternalResourceError) -> Self {
        Self::ExternalResource(error)
    }
}

impl From<ProviderV1GeneratedResourceError> for ProviderV1SinkError {
    /// Preserves one exact generated-resource payload failure without output-text inference.
    /// 在不推断输出文本的情况下保留一条精确生成资源载荷失败。
    fn from(error: ProviderV1GeneratedResourceError) -> Self {
        Self::GeneratedResource(error)
    }
}

impl From<ProjectResourceCatalogError> for ProviderV1SinkError {
    /// Preserves one project resource registration failure at the stream boundary.
    /// 在流边界保留一条项目资源注册失败。
    fn from(error: ProjectResourceCatalogError) -> Self {
        Self::ResourceCatalog(error)
    }
}

/// Registers exact generated attachments and builds their ordered durable V1 references.
/// 注册精确生成附件，并构建其有序可靠 V1 引用。
///
/// # Parameters / 参数
///
/// - `registry`: Unique process-local owner of project resource catalogs.
///   `registry`：项目资源目录的唯一进程内 owner。
/// - `project_directory`: Exact project directory from the immutable session locator.
///   `project_directory`：来自不可变会话定位器的精确项目目录。
/// - `attachments`: Producer-owned canonical attachments in output order.
///   `attachments`：按输出顺序排列的生产者自有规范附件。
///
/// # Returns / 返回值
///
/// Ordered project resource references, or one explicit producer/catalog failure.
/// 有序项目资源引用，或一条显式生产者/目录失败。
async fn register_provider_generated_resources(
    registry: Arc<ProjectResourceCatalogRegistry>,
    project_directory: PathBuf,
    attachments: Vec<ConversationAttachment>,
) -> Result<Vec<GeneratedResourceRef>, ProviderV1SinkError> {
    if attachments.is_empty() {
        return Ok(Vec::new());
    }
    let mut kinds = Vec::with_capacity(attachments.len());
    for (attachment_index, attachment) in attachments.iter().enumerate() {
        if attachment.preview_local_path.is_some() {
            return Err(ProviderV1SinkError::UntypedGeneratedPreview {
                attachment_index,
                file_name: attachment.file_name.clone(),
            });
        }
        kinds.push(match attachment.kind {
            ConversationAttachmentKind::Audio => GeneratedResourceKind::Audio,
            ConversationAttachmentKind::Image => GeneratedResourceKind::Image,
            ConversationAttachmentKind::Video => GeneratedResourceKind::Video,
            ConversationAttachmentKind::Pdf => GeneratedResourceKind::Pdf,
            ConversationAttachmentKind::File => GeneratedResourceKind::File,
        });
    }
    let attachment_count = attachments.len();
    let resource_ids = tokio::task::spawn_blocking(move || {
        registry.resolve_or_register_managed_attachments(&project_directory, &attachments)
    })
    .await
    .map_err(|error| ProviderV1SinkError::GeneratedResourceWorker(error.to_string()))??;
    if resource_ids.len() != attachment_count {
        return Err(ProviderV1SinkError::GeneratedResourceCountMismatch {
            attachments: attachment_count,
            resource_ids: resource_ids.len(),
        });
    }
    Ok(resource_ids
        .into_iter()
        .zip(kinds)
        .map(|(resource_id, kind)| GeneratedResourceRef {
            resource_id,
            kind,
            preview_resource_id: None,
        })
        .collect())
}

/// Exact frozen route identity used to accept or reject one durable replay envelope.
/// 用于接受或拒绝一份可持久化重放封套的精确冻结路由身份。
///
/// # Remarks
/// 说明
/// Every field is copied from the frozen execution this request already validated, so a replay
/// decision can never be based on a later global model selection.
/// 所有字段都复制自本请求已经校验过的冻结执行，因此重放判定绝不会基于之后的全局模型选择。
struct V1ReplayRouteIdentity {
    /// Provider definition identifier of the frozen execution.
    /// 冻结执行的供应商定义标识。
    provider_id: String,
    /// Credential connection identifier of the frozen execution.
    /// 冻结执行的凭据连接标识。
    connection_id: String,
    /// Non-secret resolved request URL of the frozen execution.
    /// 冻结执行不含秘密的已解析请求 URL。
    route_fingerprint: String,
    /// Protocol profile identifier of the frozen execution.
    /// 冻结执行的协议 Profile 标识。
    protocol_profile_id: String,
    /// Upstream model identifier of the frozen execution.
    /// 冻结执行的上游模型标识。
    model_id: String,
}

impl V1ReplayRouteIdentity {
    /// Copies the same frozen route and model identifiers used by dispatch.
    /// 复制与派发相同的冻结路由和模型标识。
    fn for_execution(
        route: &vulcan_provider_adapters::execution::ProviderExecutionRoute,
        model_id: &str,
    ) -> Self {
        Self {
            provider_id: route.provider.id.clone(),
            connection_id: route.connection.id.clone(),
            route_fingerprint: route.request_url("/responses"),
            protocol_profile_id: route.protocol_route.protocol_profile_id.clone(),
            model_id: model_id.to_owned(),
        }
    }

    /// Reports whether a durable API start proves a different route along a recorded identity dimension.
    /// 判断可靠 API 起始记录是否在已记录的身份维度上证明属于不同路由。
    fn proves_foreign_request(
        &self,
        request: &vulcan_session_core::v1::event::ApiRequestSnapshot,
    ) -> bool {
        request.provider != self.provider_id
            || request.model != self.model_id
            || request
                .connection_id
                .as_deref()
                .is_some_and(|value| value != self.connection_id)
            || request
                .profile
                .as_deref()
                .is_some_and(|value| value != self.protocol_profile_id)
    }

    /// Copies the frozen route identity from one validated preparation candidate.
    /// 从一个已校验的准备候选复制冻结路由身份。
    ///
    /// # Returns
    /// 返回值
    /// The identity of a real session request, or `None` for a state-only fixture without a frozen
    /// execution; such a fixture must never be treated as a matching route.
    /// 真实会话请求的身份；对缺少冻结执行的状态夹具返回 `None`，此类夹具绝不能被当作匹配路由。
    fn from_candidate(candidate: &ProviderRequestPreparationCandidate) -> Option<Self> {
        // execution is the same frozen carrier validated before any V1 history rebuild.
        // execution 是 V1 历史重建前已经校验过的同一冻结载体。
        let execution = &candidate.media_context.as_ref()?.execution;
        Some(Self::for_execution(&execution.route, &execution.model.id))
    }

    /// Borrows this identity as the contract-level projection route.
    /// 把本身份借用为契约层的投影路由。
    fn as_route(&self) -> ProviderReplayRoute<'_> {
        ProviderReplayRoute {
            provider_id: &self.provider_id,
            connection_id: &self.connection_id,
            route_fingerprint: &self.route_fingerprint,
            protocol_profile_id: &self.protocol_profile_id,
            model_id: &self.model_id,
        }
    }
}

/// Matches each unenveloped assistant group to the exact API that produced its first semantic event.
/// 将每个缺封套助手组匹配到确切产出其首个语义事件的 API。
pub(crate) fn unenveloped_assistant_requests(
    events: &[vulcan_session_core::v1::event::ConversationEvent],
    items: &[vulcan_session_core::v1::projection::SourcedProjectedItem],
) -> std::collections::BTreeMap<Seq, vulcan_session_core::v1::event::ApiRequestSnapshot> {
    use vulcan_session_core::v1::event::EventBody;
    // Match by API id, never by the closest selected model or adjacent request.
    // 按 API 标识匹配，绝不依据最近选择的模型或相邻请求。
    let starts: std::collections::BTreeMap<_, _> = events
        .iter()
        .filter_map(|event| {
            if let EventBody::ApiStarted(start) = &event.event {
                Some((start.api_call_id, (event, start)))
            } else {
                None
            }
        })
        .collect();
    items
        .iter()
        .filter_map(|sourced| {
            let ProjectedItem::AssistantText {
                reasoning: Some(reasoning),
                provider_source,
                ..
            } = &sourced.item
            else {
                return None;
            };
            if reasoning.is_empty()
                || provider_source
                    .as_ref()
                    .is_some_and(|source| source.replay.is_some())
            {
                return None;
            }
            // The projection range starts at the first assistant or tool semantic in this group.
            // 投影范围始于该组的首个助手或工具语义事件。
            let source_index = events
                .binary_search_by_key(&sourced.source_from_seq, |event| event.seq)
                .ok()?;
            let event = &events[source_index];
            let api_call_id = match &event.event {
                EventBody::AssistantReasoning(body) => body.api_call_id,
                EventBody::AssistantText(body) => body.api_call_id,
                EventBody::ToolCall(body) => body.api_call_id,
                _ => return None,
            };
            let (started, start) = starts.get(&api_call_id)?;
            (started.seq < event.seq && started.turn_id == event.turn_id)
                .then(|| (sourced.source_from_seq, start.request.clone()))
        })
        .collect()
}

/// Returns the proven originating request for one ordered projection item, if available.
/// 返回一个有序投影条目有证据支持的原始请求。
fn projected_assistant_request(
    snapshot: &SessionProviderProjectionSnapshot,
    index: usize,
) -> Option<&vulcan_session_core::v1::event::ApiRequestSnapshot> {
    let sourced = snapshot.sourced_items.get(index)?;
    snapshot
        .unenveloped_assistant_requests
        .get(&sourced.source_from_seq)
}

/// Checks the target model against the same assistant replay policy used to assemble physical requests.
/// 使用组装物理请求时相同的助手回放策略检查目标模型。
/// Uses the captured history and in-flight boundary with the target route, model ID, and effective model output limit.
/// 使用捕获的历史和在途边界，以及目标路由、模型 ID 和模型的有效输出上限。
/// Returns a controlled replay error if the target cannot encode the retained reasoning.
/// 目标无法编码保留的推理时返回受控回放错误。
pub(crate) fn precheck_projected_assistant_replay(
    snapshot: &SessionProviderProjectionSnapshot,
    active_api_started_seq: Option<Seq>,
    route: &vulcan_provider_adapters::execution::ProviderExecutionRoute,
    model_id: &str,
    model_output_limit: Option<u64>,
) -> Result<(), SessionRuntimeError> {
    // The pending model takes effect at a later request boundary, after the running API can seal its envelope.
    // 待生效模型在后续请求边界生效，届时当前 API 可以先封口其封套。
    let target = V1ReplayRouteIdentity::for_execution(route, model_id);
    // Collect only the history that would reach the selected protocol, in the original order.
    // 只收集会送达所选协议的历史，保持原有顺序。
    let mut messages = Vec::new();
    let check_protocol = route.reasoning_replay_contract().is_some();
    for (index, sourced) in snapshot.sourced_items.iter().enumerate() {
        if active_api_started_seq.is_some_and(|started| sourced.source_from_seq > started) {
            continue;
        }
        match &sourced.item {
            ProjectedItem::AssistantText { .. } => {
                messages.extend(projected_assistant_wire_message(
                    snapshot,
                    index,
                    &sourced.item,
                    Some(&target),
                )?);
            }
            ProjectedItem::UserInput { text, .. } | ProjectedItem::SubagentReturnText { text }
                if check_protocol =>
            {
                messages.push(text_ai_message(MessageRole::User, text.clone()));
            }
            ProjectedItem::SystemText { role, text, .. } if check_protocol => {
                messages.push(text_ai_message(
                    projected_system_message_role(*role),
                    text.clone(),
                ));
            }
            ProjectedItem::ToolResult {
                tool_call_id,
                provider_tool_call_id,
                status,
                ..
            } if check_protocol => {
                // The replay contract needs the call-result pairing, not the recorded result payload.
                // 回放契约需要调用与结果配对，不需要已记录的工具结果载荷。
                messages.push(AiMessage {
                    role: MessageRole::Tool,
                    content: vec![ContentItem::ToolResult {
                        result: AiToolResult {
                            call_id: provider_wire_tool_call_id(
                                *tool_call_id,
                                provider_tool_call_id.as_deref(),
                            ),
                            is_error: *status != ToolCallStatus::Completed,
                            content: serde_json::Value::String(String::new()),
                        },
                    }],
                    provider_context_items: Vec::new(),
                });
            }
            _ => {}
        }
    }
    // Use the same model cap and host budget as the next physical request.
    // 使用与下一次物理请求相同的模型上限与宿主预算。
    let output_token_limit =
        vulcan_agent_runtime::agent_runtime::provider_transport_vulcan::resolved_public_output_token_limit(
            model_output_limit,
        );
    vulcan_provider_adapters::execution::validate_reasoning_replay_for_selection(
        route,
        model_id,
        output_token_limit,
        messages,
    )
    .map_err(|error| {
        if error.error.subcategory.as_deref()
            == Some(vulcan_provider_adapters::REASONING_REPLAY_UNREPLAYABLE_SUBCATEGORY)
        {
            unreplayable_replay_runtime_error(
                error
                    .error
                    .upstream_message
                    .as_deref()
                    .unwrap_or(&error.error.message),
            )
        } else {
            provider_projection_runtime_error(error)
        }
    })
}

/// Builds the exact replayable assistant message for a projected item and target route.
/// 为投影条目及目标路由构建精确可回放的助手消息。
/// Returns no message when the item has no remaining request content.
/// 条目没有可发送内容时返回空值。
fn projected_assistant_wire_message(
    snapshot: &SessionProviderProjectionSnapshot,
    index: usize,
    item: &ProjectedItem,
    route: Option<&V1ReplayRouteIdentity>,
) -> Result<Option<AiMessage>, SessionRuntimeError> {
    let ProjectedItem::AssistantText {
        text,
        reasoning,
        tool_calls,
        provider_source,
    } = item
    else {
        return Err(provider_projection_runtime_error(
            "Expected a projected assistant item.",
        ));
    };
    // Canonical content is used for visible cross-route history. The matching envelope still owns native replay.
    // 跨路由可见历史使用规范内容，匹配的封套仍持有原生回放。
    let mut content = Vec::new();
    if let Some(reasoning) = reasoning.as_ref().filter(|value| !value.is_empty()) {
        content.push(ContentItem::Reasoning {
            text: reasoning.clone(),
            signature: None,
        });
    }
    if !text.is_empty() {
        content.push(ContentItem::Text { text: text.clone() });
    }
    for tool_call in tool_calls {
        content.push(ContentItem::ToolCall {
            call: AiToolCall {
                id: provider_wire_tool_call_id(
                    tool_call.tool_call_id,
                    tool_call.provider_tool_call_id.as_deref(),
                ),
                name: tool_call.tool_name.clone(),
                arguments: serde_json::to_value(&tool_call.input)
                    .map_err(provider_projection_runtime_error)?,
                signature: None,
            },
        });
    }
    let (content, provider_context_items) = resolve_projected_assistant_replay(
        content,
        provider_source
            .as_deref()
            .and_then(|source| source.replay.as_ref()),
        projected_assistant_request(snapshot, index),
        route,
        &provider_projection_message_id(index),
    )?;
    Ok(
        (!content.is_empty() || !provider_context_items.is_empty()).then_some(AiMessage {
            role: MessageRole::Assistant,
            content,
            provider_context_items,
        }),
    )
}

/// Selects the replay result for one assistant group. An absent result keeps its canonical content.
/// 选择单个助手组的回放结果。无结果时保留其规范内容。
fn assistant_replay_projection(
    has_reasoning: bool,
    envelope: Option<&ProviderReplayEnvelope>,
    origin: Option<&vulcan_session_core::v1::event::ApiRequestSnapshot>,
    route: Option<&V1ReplayRouteIdentity>,
    projected_item_id: &str,
) -> Result<Option<ProviderReplayProjection>, SessionRuntimeError> {
    if let Some(envelope) = envelope {
        let projection = route.map_or(ProviderReplayProjection::RouteMismatch, |route| {
            project_provider_replay(envelope, &route.as_route())
        });
        if let ProviderReplayProjection::Unusable(reason) = projection {
            return Err(unreplayable_replay_runtime_error(format!(
                "projected item {projected_item_id} holds an envelope this engine cannot replay: {reason:?}"
            )));
        }
        return Ok(Some(projection));
    }
    if let Some(route) = route
        && has_reasoning
    {
        if origin.is_some_and(|request| route.proves_foreign_request(request)) {
            return Ok(Some(ProviderReplayProjection::RouteMismatch));
        }
        return Err(unreplayable_replay_runtime_error(format!(
            "projected item {projected_item_id} has reasoning but no durable replay envelope"
        )));
    }
    Ok(None)
}

/// Resolves the ordered assistant payload for one projected assistant group.
/// 为一个投影的助手组解析有序助手载荷。
///
/// # Parameters
/// 参数
/// `content` is the locally rebuilt canonical content, `envelope` is this group's durable replay
/// source when one exists, and `route` is the frozen identity of the dispatching request.
/// `content` 是本地重建的规范内容，`envelope` 是该组存在时的可持久化重放来源，
/// `route` 是正在分派请求的冻结身份。
///
/// # Returns
/// 返回值
/// One mutually exclusive payload pair. Provider items replace canonical content only for the route
/// they belong to, while an unusable envelope becomes a controlled error instead of a silent local
/// rebuild.
/// 返回一对互斥载荷。供应商原生项仅在属于其路由时替代规范内容；不可用封套会变成受控错误，
/// 而不是静默的本地重建。
/// Foreign-route reasoning is omitted while visible text and tool calls retain their order.
/// 外路由思考被省略，可见正文与工具调用保持原有顺序。
fn resolve_projected_assistant_replay(
    content: Vec<ContentItem>,
    envelope: Option<&ProviderReplayEnvelope>,
    origin: Option<&vulcan_session_core::v1::event::ApiRequestSnapshot>,
    route: Option<&V1ReplayRouteIdentity>,
    projected_item_id: &str,
) -> Result<(Vec<ContentItem>, Vec<serde_json::Value>), SessionRuntimeError> {
    // projection controls the current request only; stored messages and envelopes remain authoritative.
    // projection 只控制当前请求；存储的消息和封套仍是权威记录。
    let projection = assistant_replay_projection(
        content
            .iter()
            .any(|item| matches!(item, ContentItem::Reasoning { .. })),
        envelope,
        origin,
        route,
        projected_item_id,
    )?;
    match projection {
        // Provider items and canonical content are mutually exclusive so no tool call, no reasoning
        // segment and no body text can be written twice.
        // 供应商项与规范内容互斥，使任何工具调用、推理片段与正文都不会被写入两次。
        Some(ProviderReplayProjection::ProviderItems(items)) => Ok((Vec::new(), items)),
        Some(ProviderReplayProjection::CanonicalContent(replayed)) => Ok((replayed, Vec::new())),
        Some(ProviderReplayProjection::Unusable(reason)) => {
            Err(unreplayable_replay_runtime_error(format!(
                "projected item {projected_item_id} holds an envelope this engine cannot replay: {reason:?}"
            )))
        }
        Some(ProviderReplayProjection::RouteMismatch) => {
            // State-only projections have no frozen execution and do not prepare transport bytes.
            // 仅状态投影没有冻结执行，也不准备传输字节。
            if route.is_none() {
                return Ok((content, Vec::new()));
            }
            // Reasoning belongs to its producing route; a model switch retains only visible history.
            // 思考归产生它的路由持有；模型切换仅保留可见历史。
            // This request-only filter leaves the durable envelope and displayed history intact.
            // 此请求级过滤保留持久化封套及界面历史。
            let content = content
                .into_iter()
                .filter_map(|item| match item {
                    ContentItem::Reasoning { .. } => None,
                    ContentItem::ToolCall { mut call } => {
                        call.signature = None;
                        Some(ContentItem::ToolCall { call })
                    }
                    item => Some(item),
                })
                .collect();
            Ok((content, Vec::new()))
        }
        None => Ok((content, Vec::new())),
    }
}

/// Reports one unreplayable reasoning history with the shared English runtime diagnostic.
/// 使用共享英文运行时诊断报告一次不可回传的推理历史。
///
/// # Remarks
/// 说明
/// The copy has exactly one owning place so the session, adapter and WebUI layers cannot drift
/// apart, and the technical reason is appended as missing-evidence detail only: it never contains
/// reasoning text, a signature, a credential or a raw request body.
/// 该文案只有一个归属处，使会话层、适配器层与 WebUI 层不会各自漂移；
/// 技术原因只作为「缺失证据」附加，绝不含推理正文、签名、凭据或原始请求体。
fn unreplayable_replay_runtime_error(technical_reason: impl fmt::Display) -> SessionRuntimeError {
    SessionRuntimeError::UnreplayableReasoningHistory(format!(
        "{}\n{technical_reason}",
        vulcan_provider_adapters::REASONING_REPLAY_UNREPLAYABLE_MESSAGE
    ))
}

/// Rebuilds the only transport candidate that may leave the V1 preparation owner.
/// 重建唯一允许离开 V1 准备 owner 的传输候选。
///
/// # Parameters
/// 参数
/// `snapshot` is the immutable durable ledger projection, `facts` is the selected non-secret
/// request route, and `candidate` contributes only transport capabilities that are not ledger facts.
/// `snapshot` 是不可变可靠账本投影，`facts` 是已选非秘密请求路由，`candidate` 仅贡献不属于
/// 账本事实的传输能力。
///
/// # Returns
/// 返回值
/// Returns the rebuilt candidate and a SHA-256 proof over the exact serialized request.
/// 返回重建候选以及对精确序列化请求计算的 SHA-256 证明。
#[cfg(test)]
fn rebuild_provider_transport_candidate(
    snapshot: SessionProviderProjectionSnapshot,
    facts: &ProviderV1RequestFacts,
    candidate: ProviderRequestPreparationCandidate,
    prepared_media: media::PreparedV1UserMedia,
) -> Result<SessionPreparedPhysicalRequest<ProviderRequestPreparationCandidate>, SessionRuntimeError>
{
    rebuild_provider_transport_candidate_with_compaction(
        snapshot,
        facts,
        candidate,
        prepared_media,
        None,
    )
}

/// Rebuilds `candidate` from durable `snapshot` and optional frozen `compaction`; returns the exact proved request.
/// 根据可靠`snapshot`及可选冻结`compaction`重建`candidate`；返回带精确证明的请求。
fn rebuild_provider_transport_candidate_with_compaction(
    snapshot: SessionProviderProjectionSnapshot,
    facts: &ProviderV1RequestFacts,
    mut candidate: ProviderRequestPreparationCandidate,
    mut prepared_media: media::PreparedV1UserMedia,
    compaction: Option<&compaction_source::CompactionSourceWindow>,
) -> Result<SessionPreparedPhysicalRequest<ProviderRequestPreparationCandidate>, SessionRuntimeError>
{
    // Freeze summary semantics while retaining the current durable frontier for the final permit.
    // 冻结摘要语义，同时为最终许可保留当前可靠前沿。
    let snapshot = match compaction {
        Some(source) => source.bound_snapshot(snapshot, facts)?,
        None => snapshot,
    };
    validate_provider_transport_candidate(facts, &candidate)?;
    validate_prompt_media_preparation_frontier(&snapshot)?;
    // effectiveLockedBaseline reuses the persisted initialized generation exactly. Only a fresh
    // or refresh-pending generation may establish the candidate baseline assembled for this call.
    // effectiveLockedBaseline 精确复用已持久化的初始化 generation。只有全新或待刷新 generation
    // 才允许建立本次调用组装的候选基线。
    let effective_locked_baseline = match snapshot.initialized_locked_baseline.clone() {
        Some(initialized) => {
            let expected = initial_locked_baseline_with_host_guidance(&initialized)?;
            if expected != initialized {
                return Err(provider_projection_runtime_error(
                    "The initialized V1 locked baseline is missing the fixed host guidance.",
                ));
            }
            initialized
        }
        None => initial_locked_baseline_with_host_guidance(&candidate.locked_baseline)?,
    };
    candidate.locked_baseline = effective_locked_baseline;
    // controller_state is the shared logical V1 projection and therefore cannot contain physical delivery choices.
    // controller_state 是共享逻辑 V1 投影，因此不能包含物理投递选择。
    let controller_state = provider_projection_controller_state(&snapshot)
        .map_err(provider_projection_runtime_error)?;
    // controller_snapshot is the lossless Core representation used by request capture and later checkpoints.
    // controller_snapshot 是请求捕获与后续检查点使用的无损 Core 表示。
    let mut controller_snapshot = build_core_conversation_snapshot(&controller_state)
        .map_err(provider_projection_runtime_error)?;
    // continuation comes from the same logical projection as controller history.
    // continuation 来自与控制器历史相同的逻辑投影。
    let continuation = controller_snapshot.continuation.clone();
    let mut request_messages = Vec::new();
    let mut system_prompt_parts = Vec::new();
    let mut latest_user_prompt = String::new();

    // Render admitted overlays only after authoritative history replaces the pre-gate candidate.
    // 只有权威历史替换闸门前候选后，才渲染已准入覆盖层。
    let anchored_prefixes = context_injection::anchored_prefixes(
        &snapshot,
        candidate.controller_config.context_injection.as_ref(),
    )?;
    append_provider_request_prefix(
        &candidate.fixed_instruction_layers,
        &candidate.locked_baseline,
        &mut request_messages,
        &mut system_prompt_parts,
    )?;

    // replay_route is the frozen identity a durable envelope must match before it may replace canonical content.
    // replay_route 是可持久化封套在替代规范内容前必须匹配的冻结身份。
    let replay_route = V1ReplayRouteIdentity::from_candidate(&candidate);
    for (index, item) in snapshot.items.iter().enumerate() {
        match item {
            ProjectedItem::SystemText { role, text, .. } => {
                let message_role = projected_system_message_role(*role);
                if matches!(message_role, MessageRole::System | MessageRole::Developer) {
                    system_prompt_parts.push(text.clone());
                }
                request_messages.push(text_ai_message(message_role, text.clone()));
            }
            ProjectedItem::UserInput { text, resource_ids } => {
                // Resource-bearing prompts must use the exact prevalidated result, never an unconditional Base64 fallback.
                // 带资源提示词必须使用精确预校验结果，绝不使用无条件Base64回退。
                let mut content = if resource_ids.is_empty() {
                    vec![ContentItem::Text { text: text.clone() }]
                } else {
                    let prepared = prepared_media.prompts.remove(&index).ok_or_else(|| {
                        provider_projection_runtime_error(
                            "V1 user resources have no exact prepared prompt result",
                        )
                    })?;
                    if prepared.attachments.len() != resource_ids.len() {
                        return Err(provider_projection_runtime_error(format!(
                            "V1 prepared attachment count `{}` does not match declared resource count `{}` at prompt index `{index}`.",
                            prepared.attachments.len(),
                            resource_ids.len()
                        )));
                    }
                    prepared.content
                };
                // Prefix only the physical user message; canonical history and media occurrence identities stay intact.
                // 仅前置物理用户消息；规范历史和媒体出现身份保持不变。
                if let Some(prefix) = anchored_prefixes
                    .get(&index)
                    .filter(|prefix| !prefix.is_empty())
                {
                    content.insert(
                        0,
                        ContentItem::Text {
                            text: format!("{prefix}\n\n"),
                        },
                    );
                }
                latest_user_prompt = text.clone();
                request_messages.push(AiMessage {
                    role: MessageRole::User,
                    content,
                    provider_context_items: Vec::new(),
                });
            }
            ProjectedItem::AssistantText { .. } => request_messages.extend(
                projected_assistant_wire_message(&snapshot, index, item, replay_route.as_ref())?,
            ),
            ProjectedItem::ToolResult {
                tool_call_id,
                provider_tool_call_id,
                status,
                output,
                ..
            } => {
                let call_id =
                    provider_wire_tool_call_id(*tool_call_id, provider_tool_call_id.as_deref());
                let output =
                    serde_json::to_value(output).map_err(provider_projection_runtime_error)?;
                // Native tool media is owned by the persisted model_output field, not the host metadata object.
                // 原生工具媒体归持久model_output字段持有，而非宿主元数据对象。
                let output =
                    vulcan_agent_runtime::native_media_input::v1_native_media_content(&output)
                        .map_err(provider_projection_runtime_error)?
                        .unwrap_or(output);
                // Media read before the compaction frontier keeps only its address after compaction.
                // 压缩前沿及之前读取的媒体在压缩后仅保留地址。
                let compacted = match snapshot.compaction_frontier_seq {
                    Some(frontier) => {
                        snapshot
                            .sourced_items
                            .get(index)
                            .ok_or_else(|| {
                                provider_projection_runtime_error(
                                    "V1 tool result has no exact source projection.",
                                )
                            })?
                            .source_from_seq
                            <= frontier
                    }
                    None => false,
                };
                let summarized_image = compaction.is_some_and(|source| {
                    let seq = snapshot.sourced_items[index].source_from_seq;
                    source
                        .image_excerpts
                        .iter()
                        .any(|record| record.source_seqs.contains(&seq))
                })
                    && vulcan_agent_runtime::native_media_input::compaction_inline_image(&output)
                        .map_err(provider_projection_runtime_error)?
                        .is_some();
                let output = if compacted || summarized_image {
                    vulcan_agent_runtime::native_media_input::compacted_native_media_content(
                        &output,
                    )
                    .map_err(provider_projection_runtime_error)?
                    .unwrap_or(output)
                } else {
                    output
                };
                request_messages.push(AiMessage {
                    role: MessageRole::Tool,
                    content: vec![ContentItem::ToolResult {
                        result: AiToolResult {
                            call_id: call_id.clone(),
                            is_error: *status != ToolCallStatus::Completed,
                            content: output.clone(),
                        },
                    }],
                    provider_context_items: Vec::new(),
                });
            }
            ProjectedItem::SubagentReturnText { text } => {
                latest_user_prompt = text.clone();
                request_messages.push(text_ai_message(MessageRole::User, text.clone()));
            }
        }
    }

    // Summary instructions belong to this physical request and never replace business producer state.
    // 摘要指令属于本次物理请求，绝不替换业务生产者状态。
    for message in context_injection::compaction_request_messages(facts, &candidate)? {
        let vulcan_core::types::ConversationPayload::UserText { text, .. } = &message.payload
        else {
            return Err(provider_projection_runtime_error(
                "Compaction controls must be User messages.",
            ));
        };
        latest_user_prompt = text.clone();
        request_messages.push(text_ai_message(MessageRole::User, text.clone()));
        controller_snapshot.messages.push(message);
    }

    // Dynamic layers now live in durable SystemContext history and must not be rendered again by the transport candidate.
    // 动态层现在位于可靠 SystemContext 历史中，不能再由传输候选重复渲染。
    candidate.dynamic_prompt_layers = ProviderPromptLayers::default();
    candidate.exact_request.model_id = facts.request.model.clone();
    vulcan_agent_runtime::native_media_input::project_native_media_messages(
        &mut request_messages,
        candidate.media_context.as_ref(),
    )
    .map_err(provider_projection_runtime_error)?;
    candidate.exact_request.messages = request_messages;
    candidate.exact_request.resources.clear();
    candidate.exact_request.continuation = None;
    candidate.controller_snapshot = controller_snapshot.clone();
    candidate.request_snapshot = CoreProviderRequestSnapshot {
        provider_id: facts.request.provider.clone(),
        model_id: facts.request.model.clone(),
        system_prompt: system_prompt_parts.join("\n\n"),
        prompt_layers: candidate.dynamic_prompt_layers.clone(),
        prompt: latest_user_prompt,
        prompt_in_history: true,
        // Budgeting and request diagnostics include physical prefixes while controller history stays canonical.
        // 预算及请求诊断包含物理前缀，而控制器历史保持规范内容。
        history: context_injection::request_history_with_anchors(
            &controller_snapshot,
            &anchored_prefixes,
        )?,
        temperature: facts.request.temperature,
        max_turns: candidate.request_snapshot.max_turns,
        tool_schemas: candidate.request_snapshot.tool_schemas.clone(),
        additional_params: candidate.request_snapshot.additional_params.clone(),
        request_headers: candidate.request_snapshot.request_headers.clone(),
        continuation,
    };
    let request_bytes =
        serde_json::to_vec(&candidate.exact_request).map_err(provider_projection_runtime_error)?;
    let fingerprint_sha256 = sha256_hex(&request_bytes);
    let projection = ProviderProjectionProof {
        segment_generation: snapshot.segment_generation,
        document_revision: snapshot.document_revision,
        retained_from_seq: snapshot.retained_from_seq,
        through_seq: snapshot.through_seq,
        fingerprint_sha256,
    };
    let budget_decision = candidate
        .finalize_v1_request_budget(&projection, snapshot.provider_usage_budget_anchor.as_ref())
        .map_err(provider_projection_runtime_error)?;
    // Stop before ApiStarted so the continuation action remains available to the compacted request.
    // 在 ApiStarted 前停止，使延续动作仍可供压缩后的请求使用。
    if budget_decision.host_compaction_required {
        return Err(SessionRuntimeError::ProviderContextCompactionRequired(
            format!(
                "The final V1 request reached the session compaction line: {}.",
                budget_decision.context_budget.summary_note()
            ),
        ));
    }
    Ok(SessionPreparedPhysicalRequest {
        user_prompt_media: prepared_media.bindings,
        transport: candidate,
        projection,
        request_budget_evidence: budget_decision.evidence,
    })
}

/// Validates that pre-gate route facts do not drift from the selected V1 request configuration.
/// 校验闸门前路由事实没有偏离已选 V1 请求配置。
fn validate_provider_transport_candidate(
    facts: &ProviderV1RequestFacts,
    candidate: &ProviderRequestPreparationCandidate,
) -> Result<(), SessionRuntimeError> {
    if candidate.exact_request.model_id != facts.request.model
        || candidate.request_snapshot.provider_id != facts.request.provider
        || candidate.request_snapshot.model_id != facts.request.model
        || candidate.request_snapshot.temperature != facts.request.temperature
    {
        return Err(SessionRuntimeError::ProviderProjectionInvariant(
            String::from(
                "pre-gate transport route differs from the selected V1 request configuration",
            ),
        ));
    }
    if let Some(context) = &candidate.media_context {
        // The carrier must be the same frozen execution as this request, not a new global model selection.
        // 载体必须来自本请求相同的冻结执行，而非新的全局模型选择。
        let execution = &context.execution;
        if execution.model.id != facts.request.model
            || execution.route.provider.id != facts.request.provider
            || Some(execution.route.connection.id.as_str())
                != facts.request.connection_id.as_deref()
            || Some(execution.route.protocol_route.protocol_profile_id.as_str())
                != facts.request.profile.as_deref()
            || candidate.exact_request.cache_scope_id.as_deref()
                != Some(context.session_scope_id.as_str())
        {
            return Err(provider_projection_runtime_error(
                "V1 media execution differs from the frozen request route or session scope",
            ));
        }
    }
    Ok(())
}

/// Validates Core-owned new prompt eligibility in `snapshot` before any resource I/O can begin.
/// 在任何资源IO开始前校验`snapshot`中由Core持有的新提示词资格。
/// Returns an explicit invariant error for reordered, missing or non-pending eligible prompt identities.
/// 对乱序、缺失或并非待准备的合格提示词身份返回明确不变量错误。
fn validate_prompt_media_preparation_frontier(
    snapshot: &SessionProviderProjectionSnapshot,
) -> Result<(), SessionRuntimeError> {
    // Eligibility is chronological and must point to one exact user prompt occurrence, never a resource-cache owner.
    // 资格遵循时序，必须指向一次精确用户提示词出现，而非资源缓存所有者。
    if snapshot.preparing_user_prompt_media_seqs.windows(2).any(|pair| pair[0] >= pair[1])
        || snapshot.preparing_user_prompt_media_seqs.iter().any(|seq| {
            !snapshot.sourced_items.iter().any(|item| {
                item.source_from_seq == *seq && item.source_to_seq == *seq
                    && matches!(&item.item, ProjectedItem::UserInput { resource_ids, .. } if !resource_ids.is_empty())
            && item.user_prompt_parameters.as_ref().is_some_and(|parameters| matches!(parameters.media_delivery, vulcan_session_core::v1::UserPromptMediaDelivery::Pending | vulcan_session_core::v1::UserPromptMediaDelivery::Preparing { .. }))
            })
        })
    {
        return Err(provider_projection_runtime_error("V1 prompt media preparation frontier does not match chronological source projection"));
    }
    Ok(())
}

/// Appends execution-fixed and locked instructions before project context and history.
/// 在项目上下文和历史之前追加本次执行固定的指令和锁定指令。
/// `fixed` comes from the composer and `baseline` from the session lock; both output buffers
/// follow the physical order. Unsupported locked targets return a projection error.
/// `fixed` 来自组合器，`baseline` 来自会话锁；两个输出缓冲区遵循实际顺序。
/// 遇到不支持的锁定目标时返回投影错误。
fn append_provider_request_prefix(
    fixed: &ProviderPromptLayers,
    baseline: &LockedBaselineProjection,
    messages: &mut Vec<AiMessage>,
    system_prompt_parts: &mut Vec<String>,
) -> Result<(), SessionRuntimeError> {
    for (role, target, fixed_text) in [
        (
            MessageRole::System,
            LockedPromptTarget::System,
            &fixed.system,
        ),
        (
            MessageRole::Developer,
            LockedPromptTarget::Developer,
            &fixed.developer,
        ),
    ] {
        if !fixed_text.is_empty() {
            system_prompt_parts.push(fixed_text.clone());
            messages.push(text_ai_message(role, fixed_text.clone()));
        }
        for source in baseline
            .locked_sources()
            .iter()
            .filter(|source| source.target == target)
        {
            system_prompt_parts.push(source.content.clone());
            messages.push(text_ai_message(
                locked_prompt_message_role(source)?,
                source.content.clone(),
            ));
        }
    }
    for source in baseline.locked_sources().iter().filter(|source| {
        !matches!(
            source.target,
            LockedPromptTarget::System | LockedPromptTarget::Developer
        )
    }) {
        messages.push(text_ai_message(
            locked_prompt_message_role(source)?,
            source.content.clone(),
        ));
    }
    Ok(())
}

/// Converts one locked baseline target into its exact Provider-neutral message role.
/// 把一个锁定基线目标转换为精确的 Provider 中立消息角色。
fn locked_prompt_message_role(source: &LockedSource) -> Result<MessageRole, SessionRuntimeError> {
    match source.target {
        LockedPromptTarget::System => Ok(MessageRole::System),
        LockedPromptTarget::Developer => Ok(MessageRole::Developer),
        LockedPromptTarget::User => Ok(MessageRole::User),
        LockedPromptTarget::ToolMetadata => {
            Err(SessionRuntimeError::ProviderProjectionInvariant(format!(
                "locked baseline node `{}` targets tool metadata without a frozen transport mapping",
                source.node_id
            )))
        }
    }
}

/// Converts one dynamic system role into the exact Provider-neutral message role.
/// 把一个动态系统角色转换为精确的 Provider 中立消息角色。
const fn projected_system_message_role(role: ProjectedSystemRole) -> MessageRole {
    match role {
        ProjectedSystemRole::System => MessageRole::System,
        ProjectedSystemRole::Developer => MessageRole::Developer,
        ProjectedSystemRole::HiddenUserShell => MessageRole::User,
    }
}

/// Builds one text-only Provider-neutral message without native replay state.
/// 构建一条不携带原生重放状态的纯文本 Provider 中立消息。
fn text_ai_message(role: MessageRole, text: String) -> AiMessage {
    AiMessage {
        role,
        content: vec![ContentItem::Text { text }],
        provider_context_items: Vec::new(),
    }
}

/// Maps a validated MIME value to the Provider-neutral resource category.
/// 把经过校验的 MIME 值映射到 Provider 中立资源类别。
fn resource_kind_from_mime(mime: &str) -> ResourceKind {
    // MIME classification follows the case-insensitive target registry without changing the persisted source MIME.
    // MIME分类遵循不区分大小写的目标注册表，不改变持久化来源MIME。
    let normalized = mime.to_ascii_lowercase();
    if normalized.starts_with("image/") {
        ResourceKind::Image
    } else if normalized.starts_with("audio/") {
        ResourceKind::Audio
    } else if normalized.starts_with("video/") {
        ResourceKind::Video
    } else {
        ResourceKind::File
    }
}

/// Computes the canonical lowercase SHA-256 hexadecimal digest of exact request bytes.
/// 计算精确请求字节的规范小写 SHA-256 十六进制摘要。
fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .flat_map(|byte| {
            [
                char::from_digit(u32::from(byte >> 4), 16)
                    .expect("a four-bit nibble is always hexadecimal"),
                char::from_digit(u32::from(byte & 0x0f), 16)
                    .expect("a four-bit nibble is always hexadecimal"),
            ]
        })
        .collect()
}

/// Returns the exact wire-visible tool id without relying on Provider order or tool name.
/// 返回精确线路可见工具 id，不依赖 Provider 顺序或工具名。
fn provider_wire_tool_call_id(local_id: ToolCallId, provider_tool_call_id: Option<&str>) -> String {
    provider_tool_call_id
        .map(str::to_owned)
        .unwrap_or_else(|| format!("v1-tool-{}", local_id.value()))
}

/// Wraps one request-projection failure in the structured session-owner error.
/// 把一次请求投影失败包装为结构化会话 owner 错误。
fn provider_projection_runtime_error(error: impl fmt::Display) -> SessionRuntimeError {
    SessionRuntimeError::ProviderProjectionInvariant(error.to_string())
}

impl ProviderV1StreamSink {
    /// Returns the exact active API for a transient request preview; never allocates an identity.
    /// 返回瞬态请求预览所属的精确活动 API；绝不分配身份。
    /// No parameters; returns the active API or an explicit inactive-request error.
    /// 无参数；返回活动 API，或明确的请求未活动错误。
    pub(crate) fn request_preview_api_id(&self) -> Result<u64, ProviderV1SinkError> {
        self.active
            .as_ref()
            .map(|active| active.api_call_id.value())
            .ok_or(ProviderV1SinkError::RequestNotActive)
    }

    /// Binds an unbound tool-request formation event to the active API required by projection.
    /// 将尚未绑定的工具请求构造事件绑定到投影所需的活动 API。
    /// `event` is the runtime event about to be projected; other event kinds are left untouched.
    /// `event` 是即将投影的运行时事件；其他事件类型保持不变。
    /// Preserves an existing host binding and returns an inactive-request error if a new binding has no active API.
    /// 保留已有的宿主绑定；新绑定没有活动 API 时返回请求未活动错误。
    pub(crate) fn bind_tool_request_preview(
        &self,
        event: &mut RuntimeEvent,
    ) -> Result<(), ProviderV1SinkError> {
        use vulcan_agent_runtime::agent_runtime::events::RuntimeEventKind;
        if !matches!(
            event.kind,
            RuntimeEventKind::ToolRequestStarted
                | RuntimeEventKind::ToolRequestProgress
                | RuntimeEventKind::ToolRequestPreviewUpdated
                | RuntimeEventKind::ToolRequestReady
        ) {
            return Ok(());
        }
        // The formation producer leaves this field unset. A forwarded event already carries its owner's API.
        // 构造事件的生产方不设置此字段。转发事件已携带其所有者的 API。
        if let Some(serde_json::Value::Object(metadata)) = event.metadata.as_mut()
            && let serde_json::map::Entry::Vacant(binding) = metadata.entry("api_call_id")
        {
            binding.insert(serde_json::json!(self.request_preview_api_id()?));
        }
        Ok(())
    }

    /// Publishes queued steering before the next request projection without touching active work.
    /// 在下一次请求投影前发布插队消息，不触碰当前执行。
    /// `runtime` owns message delivery; returns the number durably consumed or the exact failure.
    /// `runtime` 持有消息投递状态；返回可靠消费数量或精确失败。
    pub(crate) fn consume_subagent_steering(
        &self,
        runtime: &vulcan_agent_runtime::agent_runtime::task_runtime::TaskRuntime,
    ) -> Result<usize, String> {
        runtime.consume_task_steering_messages(self.handle.session_id(), |delivery_id, text| {
            let action = vulcan_session_core::v1::ids::ClientActionId::try_new(delivery_id.to_string())
                .map_err(|error| error.to_string())?;
            let text = text.to_string();
            let now_ms = crate::current_v1_unix_millis()?;
            self.handle.mutate_durable_if_changed(
                SessionMutationAdmission::ExistingTurn, SessionFlushReason::InputAccepted,
                move |document| document.append_in_turn_guidance_before_api(action, text, now_ms)
                    .map(|result| match result {
                        vulcan_session_core::v1::document::IdempotentMutation::Committed(seq) => SessionMutationChange::Changed(seq),
                        vulcan_session_core::v1::document::IdempotentMutation::Idempotent(seq) => SessionMutationChange::Unchanged(seq),
                    }),
            ).map(|_| ()).map_err(|error| error.to_string())
        })
    }

    /// Tells the model, before its next request, which background shell jobs of this session finished.
    /// 在下一次请求前告知模型本会话哪些后台 shell 任务已结束。
    ///
    /// # Returns
    /// 返回值
    /// Returns the number of notices recorded; notices that could not be recorded stay pending.
    /// 返回已记录的通知数量；无法记录的通知保持待送达。
    pub(crate) fn consume_shell_job_notices(&self) -> Result<usize, String> {
        let runtime = vulcan_host_tools::shell_job::ShellJobRuntime::global();
        let owner = vulcan_agent_runtime::tooling::native::canonical_shell_job_session_id(
            &self.handle.session_id().to_string(),
        );
        let mut notices = runtime.take_notices(&owner).into_iter();
        let mut recorded = 0;
        while let Some(notice) = notices.next() {
            let result = (|| {
                let action = vulcan_session_core::v1::ids::ClientActionId::try_new(format!(
                    "shell-job-notice-{}",
                    notice.job_id
                ))
                .map_err(|error| error.to_string())?;
                let text = shell_job_notice_text(&notice);
                let now_ms = crate::current_v1_unix_millis()?;
                self.handle
                    .mutate_durable_if_changed(
                        SessionMutationAdmission::ExistingTurn,
                        SessionFlushReason::InputAccepted,
                        move |document| {
                            document
                                .append_in_turn_guidance_before_api(action, text, now_ms)
                                .map(|result| match result {
                                    vulcan_session_core::v1::document::IdempotentMutation::Committed(seq) => SessionMutationChange::Changed(seq),
                                    vulcan_session_core::v1::document::IdempotentMutation::Idempotent(seq) => SessionMutationChange::Unchanged(seq),
                                })
                        },
                    )
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            })();
            if let Err(error) = result {
                runtime.requeue_notices(&owner, std::iter::once(notice).chain(notices).collect());
                return Err(error);
            }
            recorded += 1;
        }
        Ok(recorded)
    }

    /// Creates one stream-local sink around the exact registered session handle.
    /// 围绕精确注册的会话句柄创建一个流局部 sink。
    ///
    /// # Parameters
    /// 参数
    /// `handle` is the unique session owner retained until the stream task exits;
    /// `project_resource_catalog_registry` owns project-level generated-resource registration.
    /// `handle` 是保留到流任务退出的唯一会话所有者；`project_resource_catalog_registry`
    /// 持有项目级生成资源注册。
    ///
    /// # Returns
    /// 返回值
    /// Returns an idle sink ready for one typed request boundary.
    /// 返回一个等待强类型请求边界的空闲 sink。
    pub(crate) fn new(
        handle: Arc<SessionHandle>,
        project_resource_catalog_registry: Arc<ProjectResourceCatalogRegistry>,
    ) -> Self {
        Self {
            compaction_source: None,
            manual_compaction_request: None,
            handle,
            project_resource_catalog_registry,
            active: None,
            api_terminal_completion: None,
            api_terminal_failure_persisted: false,
            turn_completion: None,
            completed_assistant_text: None,
            completed_locked_baseline: None,
            answer_code_root: None,
        }
    }

    /// Returns this sink with the optional manual summary `authority` for physical admission.
    /// 返回带可选手动摘要 `authority` 的接收器，用于物理请求准入。
    pub(crate) fn with_manual_compaction_request(
        mut self,
        authority: Option<
            vulcan_session_core::v1::context_maintenance::ManualCompactionPendingRequest,
        >,
    ) -> Self {
        self.manual_compaction_request = authority;
        self
    }

    /// Resolves relative `ui-code` paths of this stream's replies against `root`.
    /// 以 `root` 解析当前流回答中的相对 `ui-code` 路径。
    pub(crate) fn with_answer_code_root(mut self, root: PathBuf) -> Self {
        self.answer_code_root = Some(root);
        self
    }

    /// Transfers all durable V1 terminal facts to the finished-stream owner exactly once.
    /// 将全部可靠 V1 终态事实向流结束所有者精确转移一次。
    pub(crate) fn take_finished_durability(&mut self) -> ProviderV1FinishedDurability {
        ProviderV1FinishedDurability {
            api_terminal: self.api_terminal_completion.take(),
            api_terminal_failure_persisted: std::mem::take(
                &mut self.api_terminal_failure_persisted,
            ),
            turn_completion: self.turn_completion.take(),
            completed_assistant_text: self.completed_assistant_text.take(),
            locked_baseline: self.completed_locked_baseline.take(),
        }
    }

    /// Reports whether the current request has a durable terminal receipt without consuming it.
    /// 报告当前请求是否已有可靠终态回执，不消费该回执。
    /// No parameters; returns true only after the entire physical request, including its tools, settled.
    /// 无参数；仅整个物理请求及其工具均结算后返回 true。
    pub(crate) fn has_durable_api_terminal(&self) -> bool {
        self.active.is_none() && self.api_terminal_completion.is_some()
    }

    /// Returns the exact final assistant semantic retained for parent subagent return projection.
    /// 返回为父级子代理回流投影保留的精确最终 assistant 语义。
    ///
    /// # Returns / 返回值
    ///
    /// The latest completed assistant fact without consuming stream-finished ownership.
    /// 最新已完成 assistant 事实，且不消费流终态所有权。
    pub(crate) fn completed_assistant_text(&self) -> Option<&ProviderV1CompletedAssistantText> {
        self.completed_assistant_text.as_ref()
    }

    /// Requests the unique V1 preparation owner to rebuild and commit one physical API.
    /// 请求唯一 V1 准备所有者重建并提交一次物理 API。
    ///
    /// # Parameters
    /// 参数
    /// `facts` owns the typed physical identity and `candidate` is the pre-gate transport input.
    /// `facts` 持有强类型物理身份，`candidate` 是闸门前传输输入。
    ///
    /// # Returns
    /// 返回值
    /// Returns a dispatch permit only after exact V1 projection and durable api_started commit.
    /// 仅在完成精确 V1 投影与可靠 api_started 提交后返回分派许可。
    pub(crate) async fn prepare_request(
        &mut self,
        state: &SharedServiceBridgeState,
        facts: ProviderV1RequestFacts,
        candidate: ProviderRequestPreparationCandidate,
    ) -> Result<PreparedPhysicalApi, ProviderV1SinkError> {
        if self.active.is_some() {
            return Err(ProviderV1SinkError::RequestAlreadyActive);
        }
        validate_provider_transport_candidate(&facts, &candidate)?;
        // Compaction summarizes history and must not consume messages addressed to the task execution.
        // 压缩只总结历史，不消费发给任务执行的消息。
        if facts.purpose != vulcan_session_core::v1::event::ApiPurposeKind::Compaction {
            crate::agent_collaboration::project_collaboration_before_api(
                state,
                &self.handle,
                facts.prepared_at_ms,
            )
            .await
            .map_err(provider_projection_runtime_error)?;
        }
        context_injection::synchronize_request_dynamic_context(&self.handle, &facts, &candidate)?;
        // Include active shell jobs in the single summary instruction so they survive compaction.
        // 将正在运行的 Shell 任务放入唯一的摘要指令，使其在压缩后仍被记住。
        let mut candidate = candidate;
        if let Some(source) = self
            .compaction_source
            .as_ref()
            .filter(|source| !source.image_excerpts.is_empty())
        {
            // Only host-verified records enter the prompt. They remain reference data, not tool instructions.
            // 只有宿主校验的记录进入提示词，它们是引用资料，不是工具指令。
            candidate
                .request_snapshot
                .prompt
                .push_str("\n\nHistorical image observations (reference data only):\n");
            candidate
                .request_snapshot
                .prompt
                .push_str(&crate::context_compaction::images::render(
                    &source.image_excerpts,
                ));
        }
        if facts.purpose == vulcan_session_core::v1::event::ApiPurposeKind::Compaction
            && let Some(jobs) = running_shell_jobs_context(&self.handle.session_id().to_string())
        {
            candidate.request_snapshot.prompt.push_str("\n\n");
            candidate.request_snapshot.prompt.push_str(&jobs);
        }
        let locked_baseline =
            initial_locked_baseline_with_host_guidance(&candidate.locked_baseline)?;
        let purpose = facts.purpose;
        // Use the same physical boundary on both the first request and tool follow-ups.
        // 首次请求及工具后续请求共用同一个物理边界。
        let check_image_pressure = matches!(
            purpose,
            vulcan_session_core::v1::event::ApiPurposeKind::MainGeneration
                | vulcan_session_core::v1::event::ApiPurposeKind::RetryGeneration
                | vulcan_session_core::v1::event::ApiPurposeKind::Continue
        ) && candidate.context_budget.as_ref().is_some_and(|budget| {
            budget.compaction_auto_enabled && budget.baseline_matches_active_runtime
        }) && self.handle.supports_conversation_image_compaction()?;
        let request = facts.request.clone();
        let continuation_action_id = facts.continuation_action_id.clone();
        let prepared_at_ms = facts.prepared_at_ms;
        let facts_for_build = facts.clone();
        // Bind each physical retry to the source chosen before its runtime launch.
        // 将每次物理重试绑定到其运行时启动前选定的来源。
        let compaction_source = self.compaction_source.clone();
        // Private upload receipts use the existing project owner; prompt identity remains in the session owner.
        // 私有上传回执使用既有项目所有者；提示词身份仍归会话所有者。
        let resource_registry = Arc::clone(&self.project_resource_catalog_registry);
        let project_directory = self.handle.storage_locator()?.project_directory();
        let receipt = self
            .handle
            .prepare_physical_api_durable_with_manual_request(
                purpose,
                request,
                locked_baseline,
                continuation_action_id,
                prepared_at_ms,
                self.manual_compaction_request.as_ref(),
                move |snapshot, preparation| async move {
                    // Plan and materialize all resources before the durable API permit, outside both session locks.
                    // 在可靠API许可之前、两把会话锁之外规划和物化全部资源。
                    // Only source-owned native media participates; minimal transcripts carry textual references.
                    // 仅来源持有的原生媒体参与准备；最小转录只携带文本引用。
                    let media_snapshot = match compaction_source.as_ref() {
                        Some(source) => {
                            source.bound_snapshot(snapshot.clone(), &facts_for_build)?
                        }
                        None => snapshot.clone(),
                    };
                    if check_image_pressure
                        && let Some(context) = candidate.media_context.as_ref()
                        && media::conversation_image_compaction_required(&media_snapshot, context)
                            .await?
                    {
                        return Err(SessionRuntimeError::ProviderContextCompactionRequired(
                            String::from(
                                crate::context_compaction::CONVERSATION_IMAGE_PRESSURE_REASON,
                            ),
                        ));
                    }
                    let prepared_media = if let Some(source) = compaction_source
                        .as_ref()
                        .filter(|source| !source.image_excerpts.is_empty())
                    {
                        media::prepare_image_summary_media(
                            &media_snapshot,
                            candidate.media_context.as_ref(),
                            resource_registry,
                            &source.image_excerpts,
                        )
                        .await?
                    } else {
                        media::prepare_v1_user_media_durable(
                            &media_snapshot,
                            candidate.media_context.as_ref(),
                            resource_registry,
                            Arc::clone(&preparation),
                        )
                        .await?
                    };
                    // Blocking resource reads and encoding run outside the session writer and publisher locks.
                    // 阻塞资源读取与编码在会话写入器、发布器锁之外运行。
                    tokio::task::spawn_blocking(move || {
                        // Rebuild from a real durable recapture after our own prompt writes; external changes still require a fresh request.
                        // 自身提示词写入后从真实可靠重捕获重建；外部变化仍要求全新请求。
                        let Some(snapshot) = preparation.recapture()? else {
                            return Ok(None);
                        };
                        rebuild_provider_transport_candidate_with_compaction(
                            snapshot,
                            &facts_for_build,
                            candidate,
                            prepared_media,
                            compaction_source.as_ref(),
                        )
                        .map(Some)
                    })
                    .await
                    .map_err(|error| SessionRuntimeError::BlockingTaskJoin(error.to_string()))?
                },
            )
            .await;
        // A compaction boundary is control flow, not a failed or committed physical request.
        // 压缩边界属于控制流，不是失败或已提交的物理请求。
        let receipt = match receipt {
            Err(SessionRuntimeError::ProviderContextCompactionRequired(reason)) => {
                return Ok(PreparedPhysicalApi::ContextCompactionRequired { reason });
            }
            result => result?,
        };
        let Some(receipt) = receipt else {
            return Ok(PreparedPhysicalApi::RequestConfigurationChanged);
        };
        let prepared = receipt.value;
        let core_start = prepared.core_start;
        let locked_baseline = prepared.locked_baseline;
        let transport = prepared.transport;
        // A newly prepared API supersedes the previous physical terminal as the stream's final API.
        // 新准备的 API 会取代上一物理终态，成为当前流的最终物理 API。
        self.api_terminal_completion = None;
        self.api_terminal_failure_persisted = false;
        self.turn_completion = None;
        self.completed_assistant_text = None;
        self.completed_locked_baseline = None;
        self.active = Some(ActiveProviderV1Projection {
            api_call_id: core_start.api_call_id,
            request_facts: facts,
            locked_baseline,
            project_directory,
            reasoning_text: String::new(),
            reasoning_seq: None,
            reasoning_sealed: false,
            reasoning_display_only: false,
            assistant_text: String::new(),
            assistant_text_seq: None,
            pending_terminal: None,
            prepared_tools: BTreeMap::new(),
            tool_batch_prepared: false,
        });
        Ok(PreparedPhysicalApi::SessionV1 {
            exact_request: transport.exact_request,
            controller_config: transport.controller_config,
            controller_snapshot: transport.controller_snapshot,
            request_snapshot: transport.request_snapshot,
            context_budget: transport.context_budget,
            core_start,
            provider_api_trace_store: self.handle.provider_api_trace_store(),
        })
    }

    /// Durably binds one real wire dispatch to the active local V1 API identity.
    /// 把一次真实线路分派可靠绑定到当前本地 V1 API 身份。
    ///
    /// # Parameters
    /// 参数
    /// `facts` carries the exact prepared request identity and typed dispatch timestamp.
    /// `facts` 携带精确已准备请求身份与强类型分派时间戳。
    ///
    /// # Returns
    /// 返回值
    /// Returns only after the transport-observed fact is durable; an exact duplicate remains
    /// idempotent and still confirms any previously dirty revision.
    /// 仅在传输已观测事实可靠落盘后返回；精确重复保持幂等，并仍确认此前任意脏修订。
    pub(crate) async fn mark_transport_observed(
        &mut self,
        facts: ProviderV1TransportObservedFacts,
    ) -> Result<(), ProviderV1SinkError> {
        let active = self
            .active
            .as_ref()
            .ok_or(ProviderV1SinkError::RequestNotActive)?;
        if active.request_facts != facts.request_facts {
            return Err(ProviderV1SinkError::RequestIdentityMismatch);
        }
        self.handle
            .mark_api_transport_observed_durable(active.api_call_id, facts.dispatched_at_ms)
            .await?;
        Ok(())
    }

    /// Appends one ordered reasoning delta to the in-memory V1 semantic event.
    /// 把一条有序思考增量追加到内存 V1 语义事件。
    ///
    /// # Parameters
    /// 参数
    /// `delta` is the exact Provider fragment, `display_only` marks a provider summary that is
    /// recorded but never replayed, and `now_ms` is its host observation time.
    /// `delta` 是精确 Provider 片段，`display_only` 标记记录但绝不重放的供应商摘要，`now_ms` 是宿主观测时间。
    ///
    /// # Returns
    /// 返回值
    /// Returns after the authoritative in-memory event contains the complete accumulated text.
    /// 在权威内存事件包含完整累积正文后返回。
    pub(crate) fn append_reasoning_delta(
        &mut self,
        delta: &str,
        display_only: bool,
        now_ms: i64,
    ) -> Result<(), ProviderV1SinkError> {
        let active = self
            .active
            .as_mut()
            .ok_or(ProviderV1SinkError::RequestNotActive)?;
        if active.pending_terminal.is_some() {
            return Err(ProviderV1SinkError::TerminalAlreadyPending);
        }
        let mut next_text = active.reasoning_text.clone();
        next_text.push_str(delta);
        // A summary taints the whole event: its text can no longer be split back into model reasoning.
        // 摘要会污染整条事件：其正文无法再拆分回模型思考。
        let next_display_only = active.reasoning_display_only || display_only;
        let receipt = self.handle.update_reasoning_with_display_only(
            active.api_call_id,
            next_text.clone(),
            AssistantSemanticStatus::Streaming,
            next_display_only,
            now_ms,
        )?;
        active.reasoning_seq = Some(receipt.value);
        active.reasoning_text = next_text;
        active.reasoning_display_only = next_display_only;
        active.reasoning_sealed = false;
        Ok(())
    }

    /// Seals the accumulated reasoning semantic without forcing a disk flush.
    /// 封口已累积思考语义，但不强制磁盘 flush。
    ///
    /// # Parameters
    /// 参数
    /// `now_ms` is the exact completion observation time.
    /// `now_ms` 是精确完成观测时间。
    ///
    /// # Returns
    /// 返回值
    /// Returns after a non-empty reasoning event reaches Completed.
    /// 在非空思考事件到达 Completed 后返回。
    pub(crate) fn complete_reasoning(&mut self, now_ms: i64) -> Result<(), ProviderV1SinkError> {
        let active = self
            .active
            .as_mut()
            .ok_or(ProviderV1SinkError::RequestNotActive)?;
        if active.reasoning_text.is_empty() || active.reasoning_sealed {
            return Ok(());
        }
        let receipt = self.handle.update_reasoning_with_display_only(
            active.api_call_id,
            active.reasoning_text.clone(),
            AssistantSemanticStatus::Completed,
            active.reasoning_display_only,
            now_ms,
        )?;
        active.reasoning_seq = Some(receipt.value);
        active.reasoning_sealed = true;
        Ok(())
    }

    /// Appends one ordered assistant delta to the in-memory V1 semantic event.
    /// 把一条有序 assistant 增量追加到内存 V1 语义事件。
    ///
    /// # Parameters
    /// 参数
    /// `delta` is the exact Provider fragment and `now_ms` is its host observation time.
    /// `delta` 是精确 Provider 片段，`now_ms` 是宿主观测时间。
    ///
    /// # Returns
    /// 返回值
    /// Returns after reasoning is sealed and the complete assistant text is updated.
    /// 在思考封口且完整 assistant 正文更新后返回。
    pub(crate) fn append_assistant_delta(
        &mut self,
        delta: &str,
        now_ms: i64,
    ) -> Result<(), ProviderV1SinkError> {
        self.complete_reasoning(now_ms)?;
        let active = self
            .active
            .as_mut()
            .ok_or(ProviderV1SinkError::RequestNotActive)?;
        if active.pending_terminal.is_some() {
            return Err(ProviderV1SinkError::TerminalAlreadyPending);
        }
        let mut next_text = active.assistant_text.clone();
        next_text.push_str(delta);
        let receipt = self.handle.update_assistant_text(
            active.api_call_id,
            next_text.clone(),
            AssistantSemanticStatus::Streaming,
            now_ms,
        )?;
        active.assistant_text_seq = Some(receipt.value);
        active.assistant_text = next_text;
        Ok(())
    }

    /// Consumes the same terminal telemetry used by global API statistics before Turn settlement.
    /// 在 Turn 结算前消费与全局 API 统计相同的终态遥测。
    /// Validates request ownership first; normalized usage is accumulated without waiting for tools.
    /// 先校验请求归属；规范化用量无需等待工具即可累计。
    pub(crate) fn record_telemetry_terminal(
        &mut self,
        event: &vulcan_agent_runtime::agent_runtime::provider_request_telemetry::ProviderRequestTelemetryEvent,
        app: &vulcan_service_api::ServiceEventEmitter,
        state: &SharedServiceBridgeState,
    ) -> Result<(), ProviderV1SinkError> {
        use vulcan_agent_runtime::agent_runtime::provider_v1_bridge::ProviderV1LifecycleFacts;
        let Some(ProviderV1LifecycleFacts::ApiTerminal(terminal)) = event.v1_lifecycle.as_ref()
        else {
            return Err(ProviderV1SinkError::MissingTypedTerminalFacts);
        };
        let active = self
            .active
            .as_ref()
            .ok_or(ProviderV1SinkError::RequestNotActive)?;
        if active.request_facts != terminal.request_facts
            || event.session_id != self.handle.session_id()
        {
            return Err(ProviderV1SinkError::RequestIdentityMismatch);
        }
        if active.pending_terminal.is_some() {
            return Err(ProviderV1SinkError::TerminalAlreadyPending);
        }
        // Read the normalized typed envelope, never plugin JSON or a later Turn report.
        // 读取规范化强类型信封，不读取插件 JSON 或后续 Turn 报告。
        let usage = event.completed_usage.as_ref().map_or_else(
            vulcan_session_core::v1::UsageSnapshot::default,
            |usage| vulcan_session_core::v1::UsageSnapshot {
                input_tokens: usage.input_tokens.map(|value| value as u64),
                uncached_input_tokens: usage.uncached_input_tokens.map(|value| value as u64),
                cache_read_tokens: usage.cached_input_tokens.map(|value| value as u64),
                cache_write_tokens: usage.cache_creation_input_tokens.map(|value| value as u64),
                output_tokens: usage.output_tokens.map(|value| value as u64),
                reasoning_tokens: usage.reasoning_tokens.map(|value| value as u64),
                total_tokens: usage.total_tokens.map(|value| value as u64),
                audio_input_tokens: usage.audio_input_tokens.map(|value| value as u64),
                audio_output_tokens: usage.audio_output_tokens.map(|value| value as u64),
                billing_tier: None,
            },
        );
        match vulcan_session_core::v1::lifetime_usage::SessionUsageBucket::from_api(
            &usage,
            &terminal.timing,
        ) {
            Ok(increment) => self
                .handle
                .record_lifetime_usage(&increment, terminal.completed_at_ms),
            Err(error) => tracing::warn!("[session-usage] terminal: {error}"),
        }
        crate::session_plan_runtime::emit_lifetime_usage_changed(app, state, &self.handle);
        Ok(())
    }

    /// Records terminal usage before any tool-free Turn settlement and then accepts its V1 facts.
    /// 在无工具回合结算前记录终态用量，然后接纳其 V1 事实。
    /// Returns the terminal outcome; accounting errors are separately reported by the owner.
    /// 返回终态结果；统计错误由所有者单独报告。
    pub(crate) async fn accept_telemetry_terminal(
        &mut self,
        event: &vulcan_agent_runtime::agent_runtime::provider_request_telemetry::ProviderRequestTelemetryEvent,
        app: &vulcan_service_api::ServiceEventEmitter,
        state: &SharedServiceBridgeState,
    ) -> Result<ProviderV1TerminalOutcome, ProviderV1SinkError> {
        self.record_telemetry_terminal(event, app, state)?;
        let Some(vulcan_agent_runtime::agent_runtime::provider_v1_bridge::ProviderV1LifecycleFacts::ApiTerminal(terminal)) = &event.v1_lifecycle else {
            return Err(ProviderV1SinkError::MissingTypedTerminalFacts);
        };
        // API usage becomes observable only after the canonical terminal facts commit.
        // 规范终态事实提交后，API 用量才可供界面观测。
        let outcome = self.accept_terminal(terminal.clone()).await?;
        if outcome == ProviderV1TerminalOutcome::Completed {
            self.publish_context_usage(app, state);
        }
        Ok(outcome)
    }

    /// Accepts one typed API terminal and durably closes a tool-free group.
    /// 接纳一个强类型 API 终态，并可靠闭合无工具调用组。
    ///
    /// # Parameters
    /// 参数
    /// `terminal` is the exact terminal fact attached to the existing durability boundary.
    /// `terminal` 是附着到既有可靠性边界的精确终态事实。
    ///
    /// # Returns
    /// 返回值
    /// Returns `AwaitingTools` without writing `api_stats` when tool calls were declared.
    /// 声明了工具调用时返回 `AwaitingTools`，且不写入 `api_stats`。
    pub(crate) async fn accept_terminal(
        &mut self,
        terminal: ProviderV1ApiTerminalFacts,
    ) -> Result<ProviderV1TerminalOutcome, ProviderV1SinkError> {
        let active = self
            .active
            .as_ref()
            .ok_or(ProviderV1SinkError::RequestNotActive)?;
        if active.request_facts != terminal.request_facts {
            return Err(ProviderV1SinkError::RequestIdentityMismatch);
        }
        if active.pending_terminal.is_some() {
            return Err(ProviderV1SinkError::TerminalAlreadyPending);
        }
        // Quoted file lines are captured once, as the reply stands when it is sealed.
        // 引用的文件行只抓取一次，即回答封口时的状态。
        let code_snapshots = match self.active.as_ref() {
            Some(active) if active.assistant_text_seq.is_some() => {
                answer_code::capture_answer_code_snapshots(
                    self.answer_code_root.clone(),
                    &active.assistant_text,
                )
                .await
            }
            _ => Vec::new(),
        };
        self.seal_semantics_for_terminal(
            terminal.status,
            terminal.completed_at_ms,
            code_snapshots,
        )?;
        if terminal.tool_call_count > 0 {
            self.active
                .as_mut()
                .ok_or(ProviderV1SinkError::RequestNotActive)?
                .pending_terminal = Some(terminal);
            return Ok(ProviderV1TerminalOutcome::AwaitingTools);
        }
        // Auxiliary question execution has its own closure and must not finish the business turn.
        // 辅助答疑通过独立入口结束，不能关闭业务回合。
        let closes_logical_turn = terminal.status == ApiCallStatus::Completed
            && !matches!(
                terminal.request_facts.purpose,
                vulcan_session_core::v1::event::ApiPurposeKind::Compaction
                    | vulcan_session_core::v1::event::ApiPurposeKind::QuestionExecution
            );
        let completed_at_ms = terminal.completed_at_ms;
        let api_terminal = self.complete_api(terminal).await?;
        let has_promoted_in_turn_inputs = !api_terminal.value.promoted_input_seqs.is_empty();
        self.api_terminal_completion = Some(api_terminal);
        if closes_logical_turn && !has_promoted_in_turn_inputs {
            // Settlement atomically chooses closure or child-return continuation, without a read/write gap.
            // 结算原子选择关闭或子任务回流续跑，不留下读写间隙。
            let receipt = self
                .handle
                .complete_successful_turn(completed_at_ms)
                .await?;
            match receipt.value {
                SuccessfulTurnSettlement::Completed(value) => {
                    self.turn_completion = Some(SessionMutationReceipt {
                        value,
                        revision: receipt.revision,
                        changed_event_seqs: receipt.changed_event_seqs,
                        flush_reason: receipt.flush_reason,
                    });
                }
                SuccessfulTurnSettlement::PendingInTurn(promoted_input_seqs) => {
                    // Publish the exact durable input boundary for whichever continuation dispatches first.
                    // 为首先分发的续跑发布精确持久输入边界。
                    // A child-return continuation can take priority and must still read every promoted input.
                    // 子任务回流续跑可以优先分发，但仍必须读取全部已提升输入。
                    let api_terminal = self.api_terminal_completion.as_mut().ok_or_else(|| {
                        provider_projection_runtime_error(
                            "Insertion settlement requires the preceding durable API terminal receipt.",
                        )
                    })?;
                    api_terminal.value.promoted_input_seqs = promoted_input_seqs;
                }
                // Terminal handling reads pending returns from the owner after releasing admission.
                // 终态处理释放准入后从所有者读取待回流结果，不保留可能过期的快照。
                SuccessfulTurnSettlement::SubagentReturn
                | SuccessfulTurnSettlement::Collaboration
                | SuccessfulTurnSettlement::Deferred => {}
            }
        }
        Ok(ProviderV1TerminalOutcome::Completed)
    }

    /// Commits one V1 tool event, then updates lifecycle surfaces before its caller acknowledges it.
    /// 提交一个 V1 工具事件，然后在调用方确认前更新生命周期界面。
    ///
    /// Parameters: app emits lifecycle deltas, state owns live surfaces, event is the executor's exact event,
    /// and now_ms timestamps the V1 mutation. Returns the durable boundary outcome or an explicit error.
    /// 参数：app 发射生命周期增量，state 持有实时界面，event 是执行器的精确事件，
    /// now_ms 为 V1 修改提供时间戳。返回可靠边界结果或显式错误。
    pub(crate) async fn apply_tool_event(
        &mut self,
        app: &vulcan_service_api::ServiceEventEmitter,
        state: &SharedServiceBridgeState,
        event: &RuntimeEvent,
        now_ms: i64,
    ) -> Result<ProviderV1ToolBoundaryOutcome, ProviderV1SinkError> {
        // boundary is the sole typed durability source, not reconstructed display metadata.
        // boundary 是唯一的强类型可靠性来源，不是由显示元数据重建的内容。
        let boundary = event
            .tool_execution_boundary
            .as_ref()
            .ok_or(ProviderV1SinkError::MissingToolExecutionBoundary)?;
        // outcome is available only after persistence succeeds; failed boundaries must not publish lifecycle changes.
        // outcome 仅在持久化成功后可用；失败边界不得发布生命周期变更。
        let outcome = self.apply_tool_boundary(boundary, now_ms).await?;
        // The final tool commits deferred ApiStats; earlier tool boundaries have no new context usage.
        // 最后一个工具提交延迟的 ApiStats；更早的工具边界没有新上下文用量。
        if self.active.is_none() {
            self.publish_context_usage(app, state);
        }
        crate::session_plan_runtime::emit_lifetime_usage_changed(app, state, &self.handle);
        // This projector owns only live resources and request progress, never legacy transcript persistence.
        // 此投影器仅拥有实时资源和请求进度，不负责旧转录持久化。
        // Keeping it inside the public V1 entry prevents callers' early returns from skipping close/spawn deltas.
        // 将其放在公开 V1 入口内，防止调用方提前返回时跳过关闭或创建增量。
        super::append_runtime_event_projection(app, state, self.handle.session_id(), event)
            .map_err(ProviderV1SinkError::ToolLifecycleProjection)?;
        Ok(outcome)
    }

    /// Refreshes the selected composer's usage through `app` and `state` after an API commit.
    /// 在 API 提交后通过 `app` 和 `state` 刷新选中会话输入区的用量。
    /// Returns nothing; a display failure is logged without rejecting an already durable API result.
    /// 无返回值；显示失败会记录日志，不拒绝已可靠保存的 API 结果。
    fn publish_context_usage(
        &self,
        app: &vulcan_service_api::ServiceEventEmitter,
        state: &SharedServiceBridgeState,
    ) {
        if let Err(error) =
            crate::emit_session_context_usage_changed(app, state, self.handle.session_id())
        {
            tracing::warn!("[context-usage] publish: {error}");
        }
    }

    /// Applies one exact tool boundary through Prepared, Running, and terminal on one event seq.
    /// 在同一事件 seq 上按 Prepared、Running 与终态应用一个精确工具边界。
    ///
    /// # Parameters
    /// 参数
    /// `boundary` is the strongly typed runtime boundary emitted by the tool executor.
    /// `boundary` 是工具执行器发出的强类型运行时边界。
    ///
    /// # Returns
    /// 返回值
    /// Returns only after the exact existing execution barrier is represented by a durable V1 revision.
    /// 仅在精确的既有执行边界被一个可靠 V1 修订表达后返回。
    async fn apply_tool_boundary(
        &mut self,
        boundary: &ToolExecutionRecoveryBoundary,
        now_ms: i64,
    ) -> Result<ProviderV1ToolBoundaryOutcome, ProviderV1SinkError> {
        match boundary {
            ToolExecutionRecoveryBoundary::BatchPrepared(requests) => self
                .prepare_tool_batch(requests, now_ms)
                .await
                .map(ProviderV1ToolBoundaryOutcome::BatchPrepared),
            ToolExecutionRecoveryBoundary::Started {
                request,
                binding_fingerprint,
            } => {
                self.start_prepared_tool(request, binding_fingerprint, now_ms)
                    .await?;
                Ok(ProviderV1ToolBoundaryOutcome::Applied)
            }
            ToolExecutionRecoveryBoundary::Finished(result) => {
                self.finish_tool(result, now_ms).await?;
                Ok(ProviderV1ToolBoundaryOutcome::Applied)
            }
        }
    }

    /// Durably reserves every Provider-ordered tool event before any tool body starts.
    /// 在任何工具主体启动前按 Provider 顺序可靠预留全部工具事件。
    async fn prepare_tool_batch(
        &mut self,
        requests: &[vulcan_agent_runtime::agent_runtime::events::ToolExecutionRecoveryRequest],
        now_ms: i64,
    ) -> Result<Vec<PreparedToolInvocationIdentity>, ProviderV1SinkError> {
        let active = self
            .active
            .as_ref()
            .ok_or(ProviderV1SinkError::RequestNotActive)?;
        let terminal = active
            .pending_terminal
            .as_ref()
            .ok_or(ProviderV1SinkError::ToolTerminalNotPending)?;
        let provider_source = terminal
            .provider_source
            .clone()
            .ok_or(ProviderV1SinkError::ToolProviderSourceMissing)?;
        if terminal.tool_call_count != requests.len() {
            return Err(ProviderV1SinkError::ToolBatchSizeMismatch {
                declared: terminal.tool_call_count,
                actual: requests.len(),
            });
        }
        let mut provider_ids = BTreeSet::new();
        for request in requests {
            if !provider_ids.insert(request.request.tool_call_id.clone()) {
                return Err(ProviderV1SinkError::DuplicateProviderToolCallId(
                    request.request.tool_call_id.clone(),
                ));
            }
        }
        let api_call_id = active.api_call_id;
        let durable_requests = requests.to_vec();
        let request_stream_ids = durable_requests
            .iter()
            .map(|request| {
                RequestStreamId::try_new(request.request_stream_id.clone()).map_err(|error| {
                    ProviderV1SinkError::InvalidRequestStreamIdentity {
                        request_stream_id: request.request_stream_id.clone(),
                        error,
                    }
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let preparations = durable_requests
            .iter()
            .zip(request_stream_ids.iter().cloned())
            .map(|(request, request_stream_id)| {
                let tool_name = request.request.tool_name.clone();
                // Unparseable Provider arguments have no object to keep; the terminal result records the rejection.
                // 无法解析的 Provider 参数没有可保留的对象；由终态结果记录该拒绝。
                let input = if request.request.argument_parse_error.is_some() {
                    ToolPayload::empty()
                } else {
                    ToolPayload::try_new(request.request.arguments.clone()).map_err(|error| {
                        ProviderV1SinkError::InvalidToolPayload {
                            tool_name: tool_name.clone(),
                            error,
                        }
                    })?
                };
                Ok(ToolPreparation {
                    request_stream_id,
                    tool_name: tool_name.clone(),
                    input,
                    provider_tool_call_id: Some(request.request.tool_call_id.clone()),
                })
            })
            .collect::<Result<Vec<_>, ProviderV1SinkError>>()?;
        let prepared = self
            .handle
            .prepare_tool_batch_with_provider_source_durable(
                api_call_id,
                preparations,
                provider_source,
                now_ms,
            )
            .await?;
        // The durable receipt distinguishes a new batch from an idempotent replay.
        // 可靠回执区分新批次与幂等重放。
        if prepared.changed {
            self.handle.record_lifetime_usage(
                &vulcan_session_core::v1::lifetime_usage::SessionUsageBucket {
                    tool_call_count: prepared.value.tools.len() as u64,
                    ..Default::default()
                },
                now_ms,
            );
        }
        let prepared = prepared.value;
        if prepared.api_call_id != api_call_id || prepared.tools.len() != durable_requests.len() {
            return Err(ProviderV1SinkError::ToolBoundaryMismatch(String::from(
                "prepared_tool_batch_receipt",
            )));
        }
        let active = self
            .active
            .as_mut()
            .ok_or(ProviderV1SinkError::RequestNotActive)?;
        let mut identities = Vec::with_capacity(prepared.tools.len());
        for ((request, request_stream_id), receipt) in durable_requests
            .into_iter()
            .zip(request_stream_ids)
            .zip(prepared.tools)
        {
            if receipt.request_stream_id != request_stream_id || receipt.api_call_id != api_call_id
            {
                return Err(ProviderV1SinkError::ToolBoundaryMismatch(
                    request.request.tool_call_id,
                ));
            }
            let provider_tool_call_id = request.request.tool_call_id.clone();
            let next = ActiveProviderV1Tool {
                request: request.request,
                binding_fingerprint: request.binding_fingerprint,
                tool_call_id: receipt.tool_call_id,
                state: ProviderV1ToolState::Prepared,
            };
            if let Some(existing) = active.prepared_tools.get(&provider_tool_call_id) {
                if existing.request != next.request
                    || existing.binding_fingerprint != next.binding_fingerprint
                    || existing.tool_call_id != next.tool_call_id
                {
                    return Err(ProviderV1SinkError::ToolBoundaryMismatch(
                        provider_tool_call_id,
                    ));
                }
            } else {
                active.prepared_tools.insert(provider_tool_call_id, next);
            }
            identities.push(receipt);
        }
        active.tool_batch_prepared = true;
        Ok(identities)
    }

    /// Promotes one durably prepared tool to Running on its existing seq.
    /// 在既有 seq 上把一条可靠 Prepared 工具提升为 Running。
    async fn start_prepared_tool(
        &mut self,
        request: &ToolInvocationRequest,
        binding_fingerprint: &str,
        now_ms: i64,
    ) -> Result<(), ProviderV1SinkError> {
        let active = self
            .active
            .as_ref()
            .ok_or(ProviderV1SinkError::RequestNotActive)?;
        let prepared = active
            .prepared_tools
            .get(&request.tool_call_id)
            .ok_or_else(|| ProviderV1SinkError::ToolNotPrepared(request.tool_call_id.clone()))?;
        if !matches!(
            prepared.state,
            ProviderV1ToolState::Prepared | ProviderV1ToolState::Running
        ) || prepared.request != *request
            || prepared.binding_fingerprint != binding_fingerprint
        {
            return Err(ProviderV1SinkError::ToolBoundaryMismatch(
                request.tool_call_id.clone(),
            ));
        }
        let api_call_id = active.api_call_id;
        let tool_call_id = prepared.tool_call_id;
        Arc::clone(&self.handle)
            .mutate_durable_if_changed_async(
                SessionMutationAdmission::ExistingTurn,
                SessionFlushReason::ToolBoundary,
                move |document| {
                    document
                        .start_prepared_tool(api_call_id, tool_call_id, now_ms)
                        .map(|changed| {
                            if changed {
                                SessionMutationChange::Changed(())
                            } else {
                                SessionMutationChange::Unchanged(())
                            }
                        })
                },
            )
            .await?;
        self.active
            .as_mut()
            .and_then(|active| active.prepared_tools.get_mut(&request.tool_call_id))
            .ok_or_else(|| ProviderV1SinkError::ToolNotPrepared(request.tool_call_id.clone()))?
            .state = ProviderV1ToolState::Running;
        Ok(())
    }

    /// Persists one exact tool result and closes the API exactly once after the final result.
    /// 持久化一个精确工具结果，并在最后一个结果后恰好一次关闭 API。
    async fn finish_tool(
        &mut self,
        result: &ToolInvocationResult,
        now_ms: i64,
    ) -> Result<(), ProviderV1SinkError> {
        let (output, changeset) = persistent_tool_completion_payloads(result)?;
        let external_refs = provider_v1_external_refs_from_tool_result(result)?;
        let generated_attachments = provider_v1_generated_attachments_from_tool_result(result)?;
        let active = self
            .active
            .as_ref()
            .ok_or(ProviderV1SinkError::RequestNotActive)?;
        let prepared = active
            .prepared_tools
            .get(&result.tool_call_id)
            .ok_or_else(|| ProviderV1SinkError::ToolNotPrepared(result.tool_call_id.clone()))?;
        if prepared.state != ProviderV1ToolState::Running
            || prepared.request.tool_name != result.tool_name
        {
            return Err(ProviderV1SinkError::ToolBoundaryMismatch(
                result.tool_call_id.clone(),
            ));
        }
        let closes_api = active.prepared_tools.values().all(|tool| {
            tool.state == ProviderV1ToolState::Finished
                || tool.tool_call_id == prepared.tool_call_id
        });
        let terminal = if closes_api {
            Some(
                active
                    .pending_terminal
                    .clone()
                    .ok_or(ProviderV1SinkError::ToolTerminalNotPending)?,
            )
        } else {
            None
        };
        let api_call_id = active.api_call_id;
        let tool_call_id = prepared.tool_call_id;
        let project_directory = active.project_directory.clone();
        let provider_tool_call_id = result.tool_call_id.clone();
        let durable_status = self
            .handle
            .read(|document| document.tool_status(api_call_id, tool_call_id))
            .map_err(ProviderV1SinkError::Runtime)?
            .map_err(|error| {
                ProviderV1SinkError::Runtime(SessionRuntimeError::Domain(Box::new(error)))
            })?;
        // A rejected Ask is durably Cancelled before its suspended future returns the canonical
        // error result. The generic ToolFinished boundary must confirm that exact terminal fact
        // instead of attempting a second Failed transition on the same tool event.
        // 被拒绝的 Ask 会在挂起 future 返回规范错误结果前可靠进入 Cancelled；通用 ToolFinished
        // 边界必须确认该精确终态，而不是在同一工具事件上再次尝试 Failed 转换。
        let terminal_status = if durable_status == ToolCallStatus::Cancelled {
            ToolCallStatus::Cancelled
        } else if result.is_error {
            ToolCallStatus::Failed
        } else {
            ToolCallStatus::Completed
        };
        let generated_resources = register_provider_generated_resources(
            Arc::clone(&self.project_resource_catalog_registry),
            project_directory,
            generated_attachments,
        )
        .await?;
        self.handle.finish_tool(
            api_call_id,
            tool_call_id,
            ToolCompletion {
                output: Some(output),
                changeset,
                status: terminal_status,
                finished_at_ms: now_ms,
                external_refs,
                generated_resources,
            },
        )?;
        if let Some(terminal) = terminal {
            let api_terminal = self.complete_api(terminal).await?;
            self.api_terminal_completion = Some(api_terminal);
        } else {
            self.active
                .as_mut()
                .and_then(|active| active.prepared_tools.get_mut(&provider_tool_call_id))
                .ok_or_else(|| ProviderV1SinkError::ToolNotPrepared(provider_tool_call_id.clone()))?
                .state = ProviderV1ToolState::Finished;
        }
        Ok(())
    }

    /// Seals reasoning and assistant semantics according to one exact API terminal.
    /// 根据一个精确 API 终态封口思考与 assistant 语义。
    ///
    /// # Parameters
    /// 参数
    /// `status` selects Completed or Interrupted and `now_ms` is the terminal observation time;
    /// `code_snapshots` are the `ui-code` captures attached by the sealing revision.
    /// `status` 选择 Completed 或 Interrupted，`now_ms` 是终态观测时间；
    /// `code_snapshots` 是随封口修订附着的 `ui-code` 抓取结果。
    ///
    /// # Returns
    /// 返回值
    /// Returns after every existing semantic event has its terminal display status.
    /// 在每条既有语义事件获得展示终态后返回。
    fn seal_semantics_for_terminal(
        &mut self,
        status: ApiCallStatus,
        now_ms: i64,
        code_snapshots: Vec<vulcan_session_core::v1::answer_components::AnswerCodeSnapshot>,
    ) -> Result<(), ProviderV1SinkError> {
        let semantic_status = if status == ApiCallStatus::Completed {
            AssistantSemanticStatus::Completed
        } else {
            AssistantSemanticStatus::Interrupted
        };
        let active = self
            .active
            .as_mut()
            .ok_or(ProviderV1SinkError::RequestNotActive)?;
        if !active.reasoning_text.is_empty() && !active.reasoning_sealed {
            let receipt = self.handle.update_reasoning_with_display_only(
                active.api_call_id,
                active.reasoning_text.clone(),
                semantic_status,
                active.reasoning_display_only,
                now_ms,
            )?;
            active.reasoning_seq = Some(receipt.value);
            active.reasoning_sealed = true;
        }
        // Seal every allocated semantic event, including an exact empty physical response.
        // 封口每个已分配语义事件，包括真实的空物理回复。
        if active.assistant_text_seq.is_some() {
            let receipt = self.handle.seal_assistant_text(
                active.api_call_id,
                active.assistant_text.clone(),
                semantic_status,
                code_snapshots,
                now_ms,
            )?;
            active.assistant_text_seq = Some(receipt.value);
        }
        Ok(())
    }

    /// Durably appends the unique API statistics event and returns the sink to idle.
    /// 可靠追加唯一 API 统计事件，并把 sink 恢复为空闲状态。
    ///
    /// # Parameters
    /// 参数
    /// `terminal` owns the exact request, usage, status, error, and timing facts.
    /// `terminal` 持有精确请求、用量、状态、错误与时间事实。
    ///
    /// # Returns
    /// 返回值
    /// Returns only after the terminal revision is durable.
    /// 仅在终态修订可靠落盘后返回。
    async fn complete_api(
        &mut self,
        terminal: ProviderV1ApiTerminalFacts,
    ) -> Result<SessionMutationReceipt<ApiGroupCompletionReceipt>, ProviderV1SinkError> {
        let active = self
            .active
            .as_ref()
            .ok_or(ProviderV1SinkError::RequestNotActive)?;
        let api_call_id = active.api_call_id;
        let completed_assistant_text = if terminal.status == ApiCallStatus::Completed {
            active
                .assistant_text_seq
                .map(|seq| ProviderV1CompletedAssistantText {
                    api_call_id,
                    seq,
                    text: active.assistant_text.clone(),
                })
        } else {
            None
        };
        // completedLockedBaseline is the exact baseline admitted with this physical request.
        // completedLockedBaseline 是与当前物理请求一同准入的精确基线。
        let completed_locked_baseline = active.locked_baseline.clone();
        // api_terminal_failure_persisted is derived only from the typed terminal fact that will be committed below.
        // api_terminal_failure_persisted 仅从下方即将提交的强类型终态事实派生。
        let api_terminal_failure_persisted = terminal.error.is_some();
        let completion = api_completion_from_terminal(terminal);
        let receipt = Arc::clone(&self.handle)
            .mutate_durable_async(
                SessionMutationAdmission::ExistingTurn,
                SessionFlushReason::ApiCompleted,
                move |document| document.complete_api_group(api_call_id, completion),
            )
            .await?;
        self.completed_assistant_text = completed_assistant_text;
        self.completed_locked_baseline = Some(completed_locked_baseline);
        self.api_terminal_failure_persisted = api_terminal_failure_persisted;
        self.active = None;
        Ok(receipt)
    }
}

/// Converts one typed Provider terminal into the authoritative session completion input.
/// 把一份强类型 Provider 终态转换为权威会话完成输入。
fn api_completion_from_terminal(terminal: ProviderV1ApiTerminalFacts) -> ApiCompletion {
    ApiCompletion {
        request: terminal.request_facts.request,
        usage: terminal.usage,
        status: terminal.status,
        end_reason: terminal.end_reason,
        error: terminal.error,
        continuation_eligibility: terminal.continuation_eligibility,
        timing: terminal.timing,
        provider_source: terminal.provider_source,
        completed_at_ms: terminal.completed_at_ms,
    }
}

/// Converts only the exact host result of a ToolFinished boundary to the persistent ChangeSet.
/// 仅把 ToolFinished 边界的精确宿主结果转换为持久化 ChangeSet。
///
/// # Parameters
/// 参数
/// `result` is the strongly typed tool result emitted by the runtime.
/// `result` 是运行时发出的强类型工具结果。
///
/// # Returns
/// 返回值
/// Returns `None` for non-change-set results and an explicit error for malformed declared payloads.
/// 非 change-set 结果返回 `None`，畸形已声明载荷返回显式错误。
fn persistent_change_set_from_tool_result(
    result: &ToolInvocationResult,
) -> Result<Option<vulcan_change_set::PersistentChangeSet>, ProviderV1SinkError> {
    let Some(host_result) = result.host_result.as_ref() else {
        return Ok(None);
    };
    Ok(persistent_change_set_from_host_result(host_result)?)
}

/// Builds the paired durable output and changeset with exactly one authoritative diff body.
/// 构建配对的可靠输出与变更集，仅保留一份权威差异正文。
/// Takes the runtime result; returns validated persistence fields or an explicit contract error.
/// 接收运行时结果，返回已校验持久化字段或明确的契约错误。
fn persistent_tool_completion_payloads(
    result: &ToolInvocationResult,
) -> Result<(ToolPayload, Option<vulcan_change_set::PersistentChangeSet>), ProviderV1SinkError> {
    // Extraction must succeed before removing the redundant host copy.
    // 必须先成功提取，才能移除重复宿主副本。
    let changeset = persistent_change_set_from_tool_result(result)?;
    // Preserve every other host contract, including file-read coordinates and error facts.
    // 保留其他所有宿主契约，包括文件读取坐标和错误事实。
    let output = if changeset.is_some() {
        ToolPayload::try_new(serde_json::json!({
            "tool_call_id": result.tool_call_id,
            "tool_name": result.tool_name,
            "model_output": result.model_output,
            "host_result": null,
            "is_error": result.is_error,
        }))
    } else {
        persistent_tool_output_from_result(result)
    }
    .map_err(|error| ProviderV1SinkError::Runtime(SessionRuntimeError::Domain(Box::new(error))))?;
    Ok((output, changeset))
}

/// Longest command text in the running-jobs list of a summary request, in characters.
/// 摘要请求中运行中任务列表里命令文本的最大长度（字符数）。
const RUNNING_SHELL_JOB_COMMAND_CHARS: usize = 200;

/// Lists the shell jobs still running in one session for a summary request; `None` when none run.
/// 为摘要请求列出某会话仍在运行的 shell 任务；没有运行中的任务时为 `None`。
fn running_shell_jobs_context(session_id: &str) -> Option<String> {
    let jobs = vulcan_host_tools::shell_job::ShellJobRuntime::global().running_jobs(
        &vulcan_agent_runtime::tooling::native::canonical_shell_job_session_id(session_id),
    );
    if jobs.is_empty() {
        return None;
    }
    let mut text = String::from(
        "Shell jobs still running in this session. Keep each job id and purpose in the summary; the \
         jobs keep running, and `shell_job` reads, waits for or stops them:",
    );
    for job in jobs {
        let command: String = job
            .command()
            .chars()
            .take(RUNNING_SHELL_JOB_COMMAND_CHARS)
            .collect();
        text.push_str(&format!(
            "\n- {}: {} (command: {})",
            job.id(),
            job.purpose(),
            command.split_whitespace().collect::<Vec<_>>().join(" ")
        ));
    }
    Some(text)
}

/// Model-facing text of one finished background shell job.
/// 一个已结束后台 shell 任务面向模型的文本。
fn shell_job_notice_text(notice: &vulcan_host_tools::shell_job::ShellJobNotice) -> String {
    vulcan_session_core::v1::shell_job_notice::ShellJobNoticeFacts {
        job_id: notice.job_id.clone(),
        purpose: notice.purpose.clone(),
        command: notice.command.clone(),
        status: notice.outcome.status.as_str().to_string(),
        duration_ms: notice.outcome.duration_ms,
        exit_code: notice.outcome.exit_code,
        failure_reason: notice.outcome.failure_reason.clone(),
    }
    .render()
}

#[cfg(test)]
mod tests;
