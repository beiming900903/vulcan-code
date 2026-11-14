mod answer_mode;
mod compaction_budget;
mod compaction_continuation;
mod context_usage;
mod host_guidance;
mod insertion_continuation;
mod model_selection;
mod native_media;
mod permission_mode;
mod provider_tools;
mod queued_child_returns;
mod terminal_return_owner;
use super::*;

/// Proves the real persistence pair retains source once and leaves non-diff metadata intact.
/// 证明真实持久化字段对只保留一份源码，且不修改非差异元数据。
#[test]
fn tool_completion_stores_changeset_body_once() {
    // This is the exact lifecycle shape produced by native file creation.
    // 这是原生文件创建产出的准确生命周期结构。
    let result = ToolInvocationResult {
        tool_call_id: "create-file".into(),
        tool_name: "file".into(),
        model_output: "# File changes applied\n\ncreate: `new.txt`".into(),
        host_result: Some(json!({"kind": "change_set", "payload": {
            "mode": "applied", "files": [{
                "resource_id": "resource-new", "change": "create", "path": "new.txt",
                "content_mode": "full", "content": "UNIQUE_DIFF_BODY", "total_line_count": 1,
                "hunks": []
            }]
        }})),
        is_error: false,
    };
    // Paired fields are precisely what finish_tool passes into the durable event.
    // 配对字段正是 finish_tool 写入可靠事件的内容。
    let (output, changeset) = persistent_tool_completion_payloads(&result).expect("valid pair");
    let encoded = serde_json::to_string(&json!({"output": output, "changeset": changeset}))
        .expect("serialize event fields");
    assert_eq!(encoded.matches("UNIQUE_DIFF_BODY").count(), 1);
    assert!(!encoded.contains("\"kind\":\"change_set\""));
    // Read mapping still has a consumer and must not be removed.
    // 读取映射仍有消费者，不得删除。
    let read_result = ToolInvocationResult {
        host_result: Some(json!({"kind": "file_access", "payload": {"range": {
            "source_start_line": 5, "output_start_line": 3, "line_count": 1
        }}})),
        ..result
    };
    let (read_output, read_changeset) =
        persistent_tool_completion_payloads(&read_result).expect("read output");
    assert!(read_changeset.is_none());
    assert!(
        serde_json::to_string(&read_output)
            .expect("read encoding")
            .contains("source_start_line")
    );
}
use serde_json::json;
use vulcan_agent_runtime::agent_runtime::events::ToolExecutionRecoveryRequest;
use vulcan_agent_runtime::agent_runtime::provider_reasoning::ProviderRuntimeReasoningResolution;
use vulcan_agent_runtime::agent_runtime::provider_transport_request_budget::ProviderRequestBudgetGuard;
use vulcan_agent_runtime::agent_runtime::tool_runtime_bridge::build_tool_failure_invocation_result;
use vulcan_ai_contract::{
    AiExecutionRequest, AiOperation, ModelCapabilities, ProviderReplayEnvelope,
};
use vulcan_session_core::v1::context::LockedBaselineProjection;
use vulcan_session_core::v1::document::{
    ClientInputSubmission, FlushCandidate, PrepareAndStartApiInput, SessionDocument,
    SubagentReturnBuildRequest,
};
use vulcan_session_core::v1::event::{
    ApiEndReason, ApiErrorCategory, ApiPurposeKind, ApiRequestSnapshot, ApiTimings,
    ConversationEvent, EventBody, ExternalResourceKind, ProviderProjectionProof, RedactedApiError,
};
use vulcan_session_core::v1::ids::{ClientActionId, Seq, SubagentTaskId};
use vulcan_session_core::v1::projection::provider_visible_fingerprint;
use vulcan_session_core::v1::stats::UsageSnapshot;
use vulcan_workbench_model::SessionId;
use vulcan_workspace_store::workspace_store::{
    ProjectResourceRegistration, SessionV1FlushResult, SessionV1StoreError,
};

use crate::session_runtime_registry_v1::{
    SessionFlushReason, SessionMutationAdmission, SessionMutationChange, SessionPersistenceBackend,
};

/// Adds host guidance exactly once to a fresh baseline projection.
/// 验证宿主指引只在全新基线投影中加入一次。
#[test]
fn fresh_baseline_contains_host_guidance_once() {
    let baseline = LockedBaselineProjection::empty();
    let first = initial_locked_baseline_with_host_guidance(&baseline)
        .expect("fresh baseline projection succeeds");
    let guidance = first
        .locked_sources()
        .iter()
        .find(|source| source.node_id == "host-message-guidance")
        .expect("host guidance source");
    assert_eq!(guidance.target, LockedPromptTarget::System);
    assert_eq!(guidance.content, HOST_MESSAGE_GUIDANCE);
    assert_eq!(
        first
            .locked_sources()
            .iter()
            .filter(|source| source.node_id == "host-message-guidance")
            .count(),
        1
    );
    assert_eq!(
        initial_locked_baseline_with_host_guidance(&first)
            .expect("initialized baseline remains stable"),
        first
    );
}

/// Deterministic successful backend for sink state-machine tests.
/// 用于 sink 状态机测试的确定性成功后端。
struct TestPersistenceBackend;

impl SessionPersistenceBackend for TestPersistenceBackend {
    /// Accepts one complete candidate and publishes its exact metadata.
    /// 接纳一份完整候选并发布其精确 metadata。
    fn flush(
        &self,
        candidate: &FlushCandidate,
    ) -> Result<SessionV1FlushResult, SessionV1StoreError> {
        Ok(SessionV1FlushResult {
            written_file_count: candidate.dirty.len(),
            published_metadata: Some(candidate.metadata.clone()),
        })
    }
}

/// Creates one published V1 session with an open root Turn.
/// 创建一个已发布且拥有开放根回合的 V1 会话。
fn published_open_turn() -> Arc<SessionHandle> {
    let session_id = SessionId::new_v4();
    let document = SessionDocument::new(session_id.to_string(), "project-1", "title", 1);
    let handle =
        SessionHandle::with_backend(session_id, document, Arc::new(TestPersistenceBackend));
    handle
        .submit_root_input(
            ClientInputSubmission::new(
                ClientActionId::try_new("provider-v1-sink-root").unwrap(),
                "root",
                Vec::new(),
            ),
            2,
        )
        .expect("publish root turn");
    handle
}

/// Creates a real isolated storage owner for return paths that require full durable reads.
/// 为需要完整持久化读取的回流路径创建真实隔离存储所有者。
/// Returns the temporary directory guard and its published open-turn handle.
/// 返回临时目录守卫及已发布开放回合的句柄。
fn published_stored_open_turn() -> (tempfile::TempDir, Arc<SessionHandle>) {
    let directory = tempfile::tempdir().unwrap();
    let id = SessionId::new_v4();
    let locator = vulcan_workspace_store::workspace_store::create_session_v1_storage_fixture(
        directory.path(),
        &id.to_string(),
        1,
    )
    .unwrap();
    let mut document = vulcan_workspace_store::workspace_store::load_session_v1_full(&locator)
        .unwrap()
        .document;
    // Request-message assertions assume no component guidance unless a test switches modes.
    // 请求消息断言假定没有组件指引，除非测试主动切换模式。
    document.metadata.config.answer_mode.mode =
        vulcan_session_core::v1::config::AnswerMode::Standard;
    let handle = SessionHandle::new(id, locator, document);
    handle.initialize_root_usage(true, 1);
    handle
        .submit_root_input(
            ClientInputSubmission::new(
                ClientActionId::try_new("stored-input").unwrap(),
                "root",
                Vec::new(),
            ),
            2,
        )
        .unwrap();
    (directory, handle)
}

/// Commits one exact prepared API and mirrors its durable receipt into the stream-local sink.
/// 提交一次精确的已准备 API，并把其可靠回执镜像到流局部 sink。
fn prepare_sink_request(handle: &SessionHandle, sink: &mut ProviderV1StreamSink) {
    prepare_sink_request_with_facts(handle, sink, request_facts());
}

/// Prepares an isolated physical request with the explicit execution purpose under test.
/// 使用被测执行用途准备隔离的物理请求；参数为持久 owner、sink 和精确请求事实，无返回值。
fn prepare_sink_request_with_facts(
    handle: &SessionHandle,
    sink: &mut ProviderV1StreamSink,
    facts: ProviderV1RequestFacts,
) {
    let projection = handle
        .read(|document| {
            let through_seq = document.events().last().map(|event| event.seq).unwrap();
            ProviderProjectionProof {
                segment_generation: document.segment_generation(),
                document_revision: document.memory_revision(),
                retained_from_seq: Seq::try_new(1).unwrap(),
                through_seq,
                fingerprint_sha256: provider_visible_fingerprint(document.events(), through_seq)
                    .unwrap(),
            }
        })
        .expect("build exact provider projection proof");
    let prepared = handle
        .prepare_and_start_api_durable(PrepareAndStartApiInput {
            purpose: facts.purpose,
            request: facts.request.clone(),
            request_budget_evidence:
                crate::session_runtime_registry_v1::test_provider_request_budget_evidence(),
            projection,
            locked_baseline: LockedBaselineProjection::empty(),
            continuation_action_id: None,
            now_ms: facts.prepared_at_ms,
        })
        .expect("durably prepare provider API");
    assert!(
        prepared.changed,
        "first physical API preparation must commit"
    );
    sink.active = Some(ActiveProviderV1Projection {
        api_call_id: prepared.value.api_call_id,
        request_facts: facts,
        locked_baseline: LockedBaselineProjection::empty(),
        project_directory: std::env::temp_dir()
            .join(format!("vulcan-provider-v1-test-{}", handle.session_id())),
        reasoning_text: String::new(),
        reasoning_sealed: false,
        reasoning_display_only: false,
        reasoning_seq: None,
        assistant_text: String::new(),
        assistant_text_seq: None,
        pending_terminal: None,
        prepared_tools: BTreeMap::new(),
        tool_batch_prepared: false,
    });
}

/// Returns deterministic typed request facts for one sink test.
/// 为一个 sink 测试返回确定性的强类型请求事实。
fn request_facts() -> ProviderV1RequestFacts {
    ProviderV1RequestFacts {
        provider_call_id: String::from("provider-call-1"),
        purpose: ApiPurposeKind::MainGeneration,
        continuation_action_id: None,
        request: ApiRequestSnapshot {
            provider: String::from("provider-1"),
            profile: Some(String::from("profile-1")),
            model: String::from("model-1"),
            connection_id: Some(String::from("connection-1")),
            thinking_level: Some(String::from("high")),
            temperature: Some(0.2),
        },
        prepared_at_ms: 3,
    }
}

/// A terminal interrupted V1 tool result remains the canonical tail of the complete controller state.
/// 终态中断的 V1 工具结果会保留为完整控制器状态的规范尾部。
#[test]
fn interrupted_tool_result_projects_in_complete_controller_state()
-> Result<(), Box<dyn std::error::Error>> {
    // snapshot contains one durable failed tool result exactly as observed by Continue.
    // snapshot 包含 Continue 所观察到的一条可靠失败工具结果。
    let snapshot = SessionProviderProjectionSnapshot {
        unenveloped_assistant_requests: Default::default(),
        preparing_user_prompt_media_seqs: Vec::new(),
        compaction_frontier_seq: None,
        image_replay_from_seq: None,
        segment_generation: 3,
        open_turn_id: None,
        child_business_execution_id: None,
        current_compaction_id: None,
        document_revision: 19,
        retained_from_seq: Seq::try_new(1)?,
        through_seq: Seq::try_new(12)?,
        items: vec![ProjectedItem::ToolResult {
            tool_call_id: ToolCallId::try_new(4)?,
            provider_tool_call_id: Some(String::from("provider-video-call")),
            tool_name: String::from("video_generate"),
            status: ToolCallStatus::Interrupted,
            output: ToolPayload::try_new(json!({
                "error": {
                    "kind": "interrupted",
                    "message": "Tool execution was interrupted before completion."
                },
                "ok": false
            }))?,
        }],
        sourced_items: Vec::new(),
        project_directory: PathBuf::new(),
        provider_usage_budget_anchor: None,
        initialized_locked_baseline: None,
    };

    // state must represent the whole V1 window even when that window currently has one item.
    // state 必须表示完整 V1 窗口，即使该窗口当前只有一项。
    let state = provider_projection_controller_state(&snapshot)?;
    assert_eq!(state.messages.len(), 1);
    assert_eq!(state.messages[0].id, "v1-provider-message-1");
    let SessionConversationPayload::ToolResult {
        tool_call_id,
        is_error,
        text,
    } = &state.messages[0].payload
    else {
        panic!("interrupted V1 tool result must remain a controller ToolResult");
    };
    assert_eq!(tool_call_id.as_deref(), Some("provider-video-call"));
    assert!(*is_error);
    assert!(text.contains("Tool execution was interrupted before completion."));
    Ok(())
}

/// The shared logical projector preserves complete V1 order and accepts an attachment-only tail without transport state.
/// 共享逻辑投影器保留完整 V1 顺序，并接纳不含传输状态的仅附件尾部。
#[test]
fn complete_v1_history_projects_attachment_only_tail_without_transport_state()
-> Result<(), Box<dyn std::error::Error>> {
    // project owns one real immutable V1 resource catalog used by the production projector.
    // project 持有生产投影器使用的一份真实不可变 V1 资源目录。
    let project = tempfile::tempdir()?;
    // registry publishes the exact video resource through the workspace-store contract.
    // registry 通过 workspace-store 契约发布精确视频资源。
    let registry = ProjectResourceCatalogRegistry::default();
    // resource_ids are the exact logical identities carried by the final attachment-only user occurrence.
    // resource_ids 是最终仅附件用户出现所携带的精确逻辑身份。
    let resource_ids = registry.resolve_or_register_project_resources(
        project.path(),
        &[ProjectResourceRegistration {
            original_name: String::from("clip.mp4"),
            mime: Some(String::from("video/mp4")),
            content: vec![1, 2, 3, 4],
            provider_bindings: Vec::new(),
        }],
    )?;
    // resource_id selects the single resource published by this isolated catalog.
    // resource_id 选择该隔离目录发布的唯一资源。
    let resource_id = resource_ids[0];
    // tool_call_id pairs the projected assistant invocation with its interrupted result.
    // tool_call_id 将投影助手调用与其中断结果精确配对。
    let tool_call_id = ToolCallId::try_new(4)?;
    // snapshot spans prior user, assistant and tool-result history before the final attachment-only input.
    // snapshot 在最终仅附件输入前覆盖先前用户、助手与工具结果历史。
    let snapshot = SessionProviderProjectionSnapshot {
        unenveloped_assistant_requests: Default::default(),
        preparing_user_prompt_media_seqs: Vec::new(),
        compaction_frontier_seq: None,
        image_replay_from_seq: None,
        segment_generation: 7,
        open_turn_id: None,
        child_business_execution_id: None,
        current_compaction_id: None,
        document_revision: 31,
        retained_from_seq: Seq::try_new(1)?,
        through_seq: Seq::try_new(20)?,
        items: vec![
            ProjectedItem::UserInput {
                text: String::from("root prompt"),
                resource_ids: Vec::new(),
            },
            ProjectedItem::AssistantText {
                text: String::from("calling tool"),
                reasoning: None,
                tool_calls: vec![vulcan_session_core::v1::projection::ProjectedToolCall {
                    tool_call_id,
                    provider_tool_call_id: Some(String::from("provider-tool-4")),
                    tool_name: String::from("video_generate"),
                    input: ToolPayload::try_new(json!({"prompt": "lake"}))?,
                }],
                provider_source: None,
            },
            ProjectedItem::ToolResult {
                tool_call_id,
                provider_tool_call_id: Some(String::from("provider-tool-4")),
                tool_name: String::from("video_generate"),
                status: ToolCallStatus::Interrupted,
                output: ToolPayload::try_new(json!({"ok": false}))?,
            },
            ProjectedItem::UserInput {
                text: String::new(),
                resource_ids: vec![resource_id],
            },
        ],
        sourced_items: Vec::new(),
        project_directory: project.path().to_path_buf(),
        provider_usage_budget_anchor: None,
        initialized_locked_baseline: None,
    };
    // state resolves only the declared resource occurrence and never adds a delivery decision.
    // state 仅解析已声明的资源出现，绝不添加投递决策。
    let state = provider_projection_controller_state(&snapshot)?;

    assert_eq!(state.messages.len(), 4);
    assert_eq!(state.messages[0].id, "v1-provider-message-1");
    assert_eq!(state.messages[1].id, "v1-provider-message-2");
    assert_eq!(state.messages[2].id, "v1-provider-message-3");
    assert_eq!(state.messages[3].id, "v1-provider-message-4");
    let SessionConversationPayload::UserText { text, attachments } = &state.messages[3].payload
    else {
        panic!("attachment-only V1 tail must remain a controller UserText");
    };
    assert!(text.is_empty());
    assert_eq!(attachments.len(), 1);
    // resource_path stays project-relative while asset_id carries the stable logical ResourceId.
    // resource_path 保持项目相对路径，asset_id 携带稳定逻辑 ResourceId。
    let resource_path = attachments[0]
        .resource_path
        .as_deref()
        .expect("canonical V1 attachment must retain a project-relative path");
    // expected_asset_id is the stable non-secret identity retained beside the relative path.
    // expected_asset_id 是与相对路径并存保留的稳定非秘密身份。
    let expected_asset_id = format!("project-resource-{}", resource_id.value());
    assert!(!std::path::Path::new(resource_path).is_absolute());
    assert_eq!(
        attachments[0].asset_id.as_deref(),
        Some(expected_asset_id.as_str())
    );
    assert!(attachments[0].local_path.is_empty());
    assert_eq!(attachments[0].kind, ConversationAttachmentKind::Video);
    assert_eq!(attachments[0].byte_len, Some(4));
    assert_eq!(attachments[0].delivery_mode, None);
    // core_snapshot proves the complete state passes the shared bridge and seeded-input contract.
    // core_snapshot 证明完整状态通过共享桥接及播种输入契约。
    let core_snapshot = build_core_conversation_snapshot(&state)?;
    assert!(
        vulcan_core::controller::conversation_snapshot_has_seeded_provider_input(&core_snapshot)
    );
    Ok(())
}

/// Verifies exact pending prompt eligibility is checked before resource access, including missing or ranged identities.
/// 验证资源访问前检查精确待准备提示词资格，包括缺失或区间身份。
#[test]
fn prompt_media_frontier_rejects_non_exact_source_before_io()
-> Result<(), Box<dyn std::error::Error>> {
    // Only one exact source event owns this resource occurrence and its pending parameters.
    // 只有一个精确来源事件持有此次资源出现及待准备参数。
    let seq = Seq::try_new(2)?;
    let resources = vec![ResourceId::try_new(1)?];
    let item = ProjectedItem::UserInput {
        text: "prompt".into(),
        resource_ids: resources.clone(),
    };
    let mut snapshot = SessionProviderProjectionSnapshot {
        unenveloped_assistant_requests: Default::default(),
        current_compaction_id: None,
        preparing_user_prompt_media_seqs: vec![seq],
        compaction_frontier_seq: None,
        image_replay_from_seq: None,
        open_turn_id: None,
        child_business_execution_id: None,
        segment_generation: 1,
        document_revision: 1,
        retained_from_seq: Seq::try_new(1)?,
        through_seq: seq,
        items: vec![item.clone()],
        sourced_items: vec![vulcan_session_core::v1::projection::SourcedProjectedItem {
            user_prompt_parameters:
                vulcan_session_core::v1::UserPromptParameters::for_new_resources(&resources),
            source_from_seq: seq,
            source_to_seq: seq,
            item,
        }],
        project_directory: PathBuf::new(),
        provider_usage_budget_anchor: None,
        initialized_locked_baseline: None,
    };
    validate_prompt_media_preparation_frontier(&snapshot)?;
    snapshot.sourced_items[0].source_to_seq = Seq::try_new(3)?;
    assert!(validate_prompt_media_preparation_frontier(&snapshot).is_err());
    snapshot.sourced_items[0].source_to_seq = seq;
    snapshot.sourced_items[0].user_prompt_parameters = None;
    assert!(validate_prompt_media_preparation_frontier(&snapshot).is_err());
    snapshot.sourced_items[0].user_prompt_parameters =
        vulcan_session_core::v1::UserPromptParameters::for_new_resources(&resources);
    snapshot.preparing_user_prompt_media_seqs = vec![seq, seq];
    assert!(validate_prompt_media_preparation_frontier(&snapshot).is_err());
    snapshot.preparing_user_prompt_media_seqs = vec![Seq::try_new(9)?];
    assert!(validate_prompt_media_preparation_frontier(&snapshot).is_err());
    Ok(())
}

/// Verifies an exact durable user-input source resolves to the same position-derived controller identity used during projection.
/// 验证精确的持久用户输入来源会解析为投影阶段使用的同一位置派生控制器身份。
#[test]
fn user_input_source_resolves_exact_projected_controller_message_id()
-> Result<(), Box<dyn std::error::Error>> {
    // first and second preserve source order so the selected input must resolve to message position two.
    // first 与 second 保留来源顺序，因此选中的输入必须解析为第二个消息位置。
    let first_seq = Seq::try_new(1)?;
    let second_seq = Seq::try_new(2)?;
    let first = ProjectedItem::UserInput {
        text: String::from("first"),
        resource_ids: Vec::new(),
    };
    let second = ProjectedItem::UserInput {
        text: String::from("edited second"),
        resource_ids: Vec::new(),
    };
    let snapshot = SessionProviderProjectionSnapshot {
        unenveloped_assistant_requests: Default::default(),
        preparing_user_prompt_media_seqs: Vec::new(),
        compaction_frontier_seq: None,
        image_replay_from_seq: None,
        segment_generation: 1,
        current_compaction_id: None,
        document_revision: 2,
        open_turn_id: None,
        child_business_execution_id: None,
        retained_from_seq: first_seq,
        through_seq: second_seq,
        items: vec![first.clone(), second.clone()],
        sourced_items: vec![
            vulcan_session_core::v1::projection::SourcedProjectedItem {
                user_prompt_parameters: None,
                source_from_seq: first_seq,
                source_to_seq: first_seq,
                item: first,
            },
            vulcan_session_core::v1::projection::SourcedProjectedItem {
                user_prompt_parameters: None,
                source_from_seq: second_seq,
                source_to_seq: second_seq,
                item: second,
            },
        ],
        project_directory: PathBuf::new(),
        provider_usage_budget_anchor: None,
        initialized_locked_baseline: None,
    };

    assert_eq!(
        provider_projection_user_message_id_for_source(&snapshot, second_seq)?,
        "v1-provider-message-2"
    );
    Ok(())
}

/// Returns one successful terminal fact for the supplied tool count.
/// 为给定工具数量返回一份成功终态事实。
fn terminal_facts(tool_call_count: usize) -> ProviderV1ApiTerminalFacts {
    ProviderV1ApiTerminalFacts {
        request_facts: request_facts(),
        usage: UsageSnapshot {
            input_tokens: Some(10),
            output_tokens: Some(5),
            total_tokens: Some(15),
            ..UsageSnapshot::default()
        },
        status: ApiCallStatus::Completed,
        end_reason: Some(ApiEndReason::CompletedSuccessfully),
        error: None,
        continuation_eligibility: ContinuationEligibility::Forbidden,
        timing: ApiTimings {
            prepared_at_ms: Some(3),
            completed_at_ms: Some(8),
            ..ApiTimings::default()
        },
        provider_source: Some(vulcan_session_core::v1::event::ProviderSourceInput {
            provider: String::from("provider-1"),
            profile: Some(String::from("profile-1")),
            model: String::from("model-1"),
            connection_id: Some(String::from("connection-1")),
            message_id: Some(String::from("message-1")),
            response_id: None,
            replay: Some(ProviderReplayEnvelope::new(
                "provider-1",
                "connection-1",
                "https://provider.invalid/v1",
                "profile-1",
                "model-1",
                Vec::new(),
                Vec::new(),
            )),
        }),
        tool_call_count,
        completed_at_ms: 8,
    }
}

/// Proves that pre-gate legacy messages cannot reach the physical Provider permit.
/// 证明闸门前旧消息无法进入物理 Provider 许可。
#[test]
fn provider_transport_rebuild_replaces_pre_gate_conversation() {
    let facts = request_facts();
    let legacy_message = text_ai_message(MessageRole::User, String::from("legacy-message"));
    let candidate_locked_baseline =
        LockedBaselineProjection::project(vec![LockedBaselineSegmentInput {
            kind: LockedSourceKind::AgentsMd,
            node_id: String::from("candidate-baseline"),
            target: LockedPromptTarget::Developer,
            source_label: None,
            source_id: None,
            content: String::from("candidate-baseline-content"),
        }])
        .expect("candidate baseline");
    let initialized_locked_baseline = LockedBaselineProjection::project(vec![
        LockedBaselineSegmentInput {
            kind: LockedSourceKind::AgentsMd,
            node_id: String::from("persisted-baseline"),
            target: LockedPromptTarget::User,
            source_label: None,
            source_id: None,
            content: String::from("persisted-baseline-content"),
        },
        LockedBaselineSegmentInput {
            kind: LockedSourceKind::BaselineHooks,
            node_id: String::from("host-message-guidance"),
            target: LockedPromptTarget::System,
            source_label: Some(String::from("host_message_guidance")),
            source_id: Some(String::from("v1:host_message_guidance")),
            content: String::from(HOST_MESSAGE_GUIDANCE),
        },
    ])
    .expect("persisted initialized baseline");
    let candidate = ProviderRequestPreparationCandidate {
        media_context: None,
        exact_request: AiExecutionRequest {
            operation: AiOperation::ConversationRespond,
            model_id: facts.request.model.clone(),
            cache_scope_id: None,
            continuation: None,
            messages: vec![legacy_message],
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
        controller_config: vulcan_core::types::ControllerConfig {
            provider_id: facts.request.provider.clone(),
            model_id: facts.request.model.clone(),
            temperature: facts.request.temperature,
            // Declared overlays must survive final reconstruction without becoming canonical transcript.
            // 已声明覆盖层必须跨最终重建保留，但不能成为规范转录。
            context_injection: Some(vulcan_core::types::ProviderContextInjection {
                persistent_blocks: vec![String::from("admitted-profile-delta")],
                cache_epoch_id: Some(
                    vulcan_vmm::VmmContextEpoch::new(4, None)
                        .expect("context")
                        .cache_epoch_id(),
                ),
                anchored_prefixes: vec![vulcan_core::types::ProviderContextAnchor {
                    anchor_id: String::from("admitted-memory-anchor"),
                    target_message_id: provider_projection_message_id(0),
                    prefix: String::from("admitted-memory-body"),
                }],
                ..Default::default()
            }),
            ..Default::default()
        },
        controller_snapshot: ConversationSnapshot {
            messages: vec![ConversationMessage {
                id: String::from("legacy-controller-message"),
                payload: ConversationPayload::UserText {
                    text: String::from("legacy-message"),
                    attachments: Vec::new(),
                },
            }],
            ..ConversationSnapshot::default()
        },
        request_snapshot: CoreProviderRequestSnapshot {
            provider_id: facts.request.provider.clone(),
            model_id: facts.request.model.clone(),
            system_prompt: String::from("legacy-system"),
            prompt_layers: ProviderPromptLayers::default(),
            prompt: String::from("legacy-message"),
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
        dynamic_prompt_layers: ProviderPromptLayers {
            system: String::from("dynamic-system"),
            developer: String::from("dynamic-developer"),
            tool_guidance: String::from("dynamic-tool-guidance"),
            runtime_context: String::from("dynamic-runtime-context"),
        },
        fixed_instruction_layers: ProviderPromptLayers {
            system: String::from("fixed-base-system\n\nfixed-agent-system"),
            developer: String::from("fixed-capability-guide"),
            ..Default::default()
        },
        locked_baseline: candidate_locked_baseline,
    };
    // project owns the immutable resource identity shared by physical reconstruction and logical projection.
    // project 持有物理重建与逻辑投影共享的不可变资源身份。
    let project = tempfile::tempdir().expect("create isolated resource project");
    // registry publishes one exact resource through the production workspace-store contract.
    // registry 通过生产 workspace-store 契约发布一项精确资源。
    let registry = ProjectResourceCatalogRegistry::default();
    // resource_ids preserve the exact occurrence identity consumed by both reconstruction stages.
    // resource_ids 保留两个重建阶段共同消费的精确出现身份。
    let resource_ids = registry
        .resolve_or_register_project_resources(
            project.path(),
            &[ProjectResourceRegistration {
                original_name: String::from("reference.png"),
                mime: Some(String::from("image/png")),
                content: vec![1, 2, 3, 4],
                provider_bindings: Vec::new(),
            }],
        )
        .expect("publish isolated project resource");
    // resource_id selects the one published resource occurrence.
    // resource_id 选择唯一已发布资源出现。
    let resource_id = resource_ids[0];
    let snapshot = SessionProviderProjectionSnapshot {
        unenveloped_assistant_requests: Default::default(),
        preparing_user_prompt_media_seqs: Vec::new(),
        compaction_frontier_seq: None,
        image_replay_from_seq: None,
        segment_generation: 4,
        open_turn_id: None,
        child_business_execution_id: None,
        current_compaction_id: None,
        document_revision: 7,
        retained_from_seq: Seq::try_new(1).expect("valid retained seq"),
        through_seq: Seq::try_new(3).expect("valid through seq"),
        items: vec![ProjectedItem::UserInput {
            text: String::from("durable-v1-message"),
            resource_ids: vec![resource_id],
        }],
        sourced_items: Vec::new(),
        project_directory: project.path().to_path_buf(),
        provider_usage_budget_anchor: None,
        initialized_locked_baseline: Some(initialized_locked_baseline.clone()),
    };
    // prepared_media carries one request-local delivery decision that must not enter controller history.
    // prepared_media 携带一个不得进入控制器历史的请求局部投递决策。
    let prepared_media = media::PreparedV1UserMedia {
        prompts: BTreeMap::from([(
            0,
            media::PreparedV1PromptMedia {
                content: vec![ContentItem::Text {
                    text: String::from("durable-v1-message"),
                }],
                attachments: vec![ConversationAttachment {
                    kind: ConversationAttachmentKind::Image,
                    file_name: String::from("reference.png"),
                    local_path: String::from("D:/project/reference.png"),
                    resource_path: Some(String::from("project-resource://1")),
                    source_path: None,
                    asset_id: None,
                    byte_len: Some(4),
                    source_sha256: Some(String::from("abcd")),
                    delivery_mode: Some(ConversationAttachmentDeliveryMode::UploadReference),
                    preview_local_path: None,
                    mime_type: String::from("image/png"),
                }],
            },
        )]),
        bindings: Vec::new(),
    };

    let rebuilt = rebuild_provider_transport_candidate(snapshot, &facts, candidate, prepared_media)
        .expect("rebuild the exact V1 request");
    let serialized = serde_json::to_string(&rebuilt.transport.exact_request.messages)
        .expect("serialize rebuilt request messages");
    assert!(serialized.contains("durable-v1-message"));
    assert!(!serialized.contains("dynamic-system"));
    assert!(!serialized.contains("dynamic-developer"));
    assert!(!serialized.contains("dynamic-tool-guidance"));
    assert!(!serialized.contains("dynamic-runtime-context"));
    assert_eq!(serialized.matches("admitted-profile-delta").count(), 0);
    assert_eq!(serialized.matches("admitted-memory-body").count(), 1);
    // Physical prefix placement is checked directly, not inferred from a request-wide text match.
    // 直接检查物理前缀位置，而非从整个请求的文本匹配推断。
    let physical_user = rebuilt
        .transport
        .exact_request
        .messages
        .last()
        .expect("physical user");
    assert_eq!(physical_user.role, MessageRole::User);
    assert_eq!(physical_user.content.len(), 2);
    assert!(
        matches!(&physical_user.content[0], ContentItem::Text { text } if text == "admitted-memory-body\n\n")
    );
    assert!(
        matches!(&physical_user.content[1], ContentItem::Text { text } if text == "durable-v1-message")
    );
    assert!(serialized.contains("persisted-baseline-content"));
    assert!(rebuilt.transport.exact_request.messages.starts_with(&[
        text_ai_message(
            MessageRole::System,
            "fixed-base-system\n\nfixed-agent-system".into(),
        ),
        text_ai_message(MessageRole::System, HOST_MESSAGE_GUIDANCE.into()),
        text_ai_message(MessageRole::Developer, "fixed-capability-guide".into()),
        text_ai_message(MessageRole::User, "persisted-baseline-content".into()),
    ]));
    // Verify both an in-band prefix and a protocol with dedicated system instructions.
    // 同时验证消息内前缀和使用独立系统指令字段的协议。
    let chat = vulcan_llm_core::ProtocolRegistry::first_batch()
        .expect("registered protocols")
        .require(vulcan_llm_core::OPENAI_CHAT_PROFILE_ID)
        .expect("registered Chat Completions profile")
        .encode(&rebuilt.transport.exact_request)
        .expect("encode exact request for Chat Completions");
    assert_eq!(chat.body["messages"][0]["role"], "system");
    assert_eq!(
        chat.body["messages"][0]["content"],
        "fixed-base-system\n\nfixed-agent-system"
    );
    let responses =
        vulcan_llm_core::encode_codex_responses_request(&rebuilt.transport.exact_request)
            .expect("encode exact request for Codex Responses");
    assert!(
        responses.body["instructions"]
            .as_str()
            .unwrap()
            .starts_with("fixed-base-system\n\nfixed-agent-system")
    );
    assert_eq!(responses.body["input"][0]["role"], "developer");
    assert_eq!(
        responses.body["input"][0]["content"][0]["text"],
        "fixed-capability-guide"
    );
    assert!(!serialized.contains("candidate-baseline-content"));
    assert!(!serialized.contains("legacy-message"));
    assert_eq!(
        rebuilt.transport.locked_baseline,
        initialized_locked_baseline
    );
    assert_eq!(
        rebuilt.transport.request_snapshot.prompt_layers,
        ProviderPromptLayers::default()
    );
    // The request-only memory prefix never contaminates durable controller message text.
    // 请求专属记忆前缀绝不污染持久控制器消息正文。
    assert!(
        !serde_json::to_string(&rebuilt.transport.controller_snapshot)
            .expect("canonical controller snapshot")
            .contains("admitted-memory-body")
    );
    assert!(matches!(
        &rebuilt.transport.request_snapshot.history[0].payload,
        ConversationPayload::UserText { text, .. }
            if text == "admitted-memory-body\n\ndurable-v1-message"
    ));
    // Compare through the actual budget guard, not a separate test-only text length calculation.
    // 通过实际预算守卫比较，而非另写测试专属文本长度计算。
    let mut unprefixed_snapshot = rebuilt.transport.request_snapshot.clone();
    if let ConversationPayload::UserText { text, .. } = &mut unprefixed_snapshot.history[0].payload
    {
        *text = String::from("durable-v1-message");
    } else {
        panic!("fixture requires the exact user history item");
    }
    // Only the memory prefix differs; route, canonical history length and persistent layers stay identical.
    // 唯一差异是记忆前缀；路由、规范历史长度及持久层保持一致。
    let unprefixed_budget = rebuilt
        .transport
        .request_budget_guard
        .as_ref()
        .expect("frozen budget guard")
        .build_v1_budget_decision(&unprefixed_snapshot, &rebuilt.projection, None)
        .expect("unprefixed budget comparison");
    assert!(
        rebuilt
            .request_budget_evidence
            .request_text_measurement
            .character_count
            > unprefixed_budget
                .evidence
                .request_text_measurement
                .character_count
    );
    assert_eq!(
        rebuilt.transport.request_snapshot.prompt,
        "durable-v1-message"
    );
    let ConversationPayload::UserText { attachments, .. } =
        &rebuilt.transport.controller_snapshot.messages[0].payload
    else {
        panic!("rebuilt V1 controller history must preserve the user resource occurrence");
    };
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].delivery_mode, None);
    assert!(attachments[0].local_path.is_empty());
    assert!(
        attachments[0]
            .resource_path
            .as_deref()
            .is_some_and(|path| !std::path::Path::new(path).is_absolute())
    );
    assert!(
        rebuilt.transport.context_budget.is_some(),
        "the exact V1 transport request must pass the final budget gate"
    );
    assert!(
        rebuilt
            .request_budget_evidence
            .request_text_measurement
            .character_count
            > 0,
        "the persisted evidence must measure the rebuilt request instead of the legacy candidate"
    );
    assert_eq!(
        rebuilt
            .request_budget_evidence
            .request_text_measurement
            .history_message_count,
        u64::try_from(rebuilt.transport.request_snapshot.history.len())
            .expect("fixture history count fits u64")
    );
    assert_eq!(rebuilt.projection.fingerprint_sha256.len(), 64);
    assert_eq!(rebuilt.projection.document_revision, 7);
}

/// Returns one exact durable-recovery request for a Provider tool call.
/// 为一条 Provider 工具调用返回精确的可靠恢复请求。
fn recovery_request(
    tool_call_id: &str,
    tool_name: &str,
    arguments: serde_json::Value,
) -> ToolExecutionRecoveryRequest {
    // requestStreamId is the stable Provider item identity paired with this execution identity.
    // requestStreamId 是与该执行身份精确配对的稳定 Provider item 身份。
    let request_stream_id = format!("request-stream-{tool_call_id}");
    ToolExecutionRecoveryRequest {
        request_stream_id: request_stream_id.clone(),
        request: ToolInvocationRequest {
            request_stream_id,
            tool_call_id: tool_call_id.to_string(),
            tool_name: tool_name.to_string(),
            arguments,
            argument_parse_error: None,
            raw_arguments: Default::default(),
            user_request: Default::default(),
        },
        binding_fingerprint: format!("binding-{tool_call_id}"),
    }
}

/// Captures navigation pushes and checks they never precede durable V1 tool completion.
/// 捕获导航推送，并检查推送不会早于可靠的 V1 工具完成。
struct ToolLifecycleEventSink {
    /// Parent ledger that must already contain the completed tool when closure is published.
    /// 发布关闭状态时必须已包含完成工具的父账本。
    handle: Arc<SessionHandle>,
    /// Navigation states observed without opening or refreshing any child panel.
    /// 未打开或刷新任何子面板时观察到的导航状态。
    states: std::sync::Mutex<Vec<String>>,
}

impl vulcan_service_api::ServiceEventSink for ToolLifecycleEventSink {
    /// Records each child status; parameters contain the service event and no value is returned.
    /// 记录每个子任务状态；参数包含服务事件，无返回值。
    fn emit_service_event(&self, event: vulcan_service_api::ServiceEvent) {
        if let vulcan_service_api::ServiceEvent::WorkbenchDelta(envelope) = event
            && let vulcan_service_api::WorkbenchDelta::SubagentNavigationChanged(delta) =
                envelope.delta
            && let Some(navigation) = delta.subagent_navigation
        {
            for task in navigation.tasks {
                if task.run_state == "closed" {
                    self.handle.read(|document| {
                            assert_eq!(document.memory_revision(), document.durable_revision());
                            assert!(document.events().iter().any(|event| matches!(
                                &event.event, EventBody::ToolCall(tool) if tool.status == ToolCallStatus::Completed
                            )));
                        }).expect("closed navigation must follow durable tool completion");
                }
                self.states
                    .lock()
                    .expect("captured states")
                    .push(task.run_state);
            }
        }
    }
}

/// Exercises the real V1 event handler after closing a live child, including pending-peer and rejected-boundary cases.
/// 关闭实时子任务后执行真实 V1 事件处理入口，包含同批工具未完成与边界被拒绝的情况。
/// Parameters: tool_call_count selects one or two tools; start_close_tool controls whether completion is admissible.
/// 参数：tool_call_count 选择一个或两个工具；start_close_tool 控制完成边界是否满足准入条件。
/// Returns: none; asserts navigation and cache changes only follow successful durable tool completion.
/// 返回值：无；断言导航与缓存变化只能发生在工具可靠完成后。
async fn assert_v1_close_agent_navigation(tool_call_count: usize, start_close_tool: bool) {
    use crate::agent_stream::{
        ProviderTelemetryHookGuard, handle_webui_detached_child_execution_event,
    };
    use crate::tests::test_support::{
        seed_test_live_subagent_registry_from_panel_states, test_service_state_with_parent_session,
    };
    use vulcan_agent_runtime::agent_runtime::events::{
        ExecutionStreamEvent, RuntimeEvent, RuntimeEventKind,
    };
    use vulcan_agent_runtime::agent_runtime::provider_tool_event_format::{
        attach_tool_name_to_runtime_metadata, build_tool_result_runtime_metadata,
    };
    use vulcan_workbench_model::{SubagentTaskPanelState, SubagentTaskRunState};

    // handle and sink share the real V1 persistence owner for this execution.
    // handle 和 sink 共享本次执行的真实 V1 持久化所有者。
    let handle = published_open_turn();
    let mut sink = ProviderV1StreamSink::new(
        Arc::clone(&handle),
        Arc::new(ProjectResourceCatalogRegistry::default()),
    );
    prepare_sink_request(&handle, &mut sink);
    assert!((1..=2).contains(&tool_call_count));
    sink.accept_terminal(terminal_facts(tool_call_count))
        .await
        .expect("tool-bearing terminal");
    // request is prepared and started before close_agent changes the live registry.
    // request 在 close_agent 改变实时注册表前完成准备和启动。
    let request = recovery_request("close-1", "close_agent", json!({"target": "standby-agent"}));
    // requests can include an unfinished sibling, so closure must not wait for the whole API batch.
    // requests 可包含尚未完成的同批工具，因此关闭同步不得等待整批 API 完成。
    let mut requests = vec![request.clone()];
    if tool_call_count == 2 {
        requests.push(recovery_request("list-2", "list_agents", json!({})));
    }
    sink.apply_tool_boundary(&ToolExecutionRecoveryBoundary::BatchPrepared(requests), 9)
        .await
        .expect("prepared close");
    if start_close_tool {
        sink.apply_tool_boundary(
            &ToolExecutionRecoveryBoundary::Started {
                request: request.request.clone(),
                binding_fingerprint: request.binding_fingerprint.clone(),
            },
            10,
        )
        .await
        .expect("running close");
    }
    // child is completed but still open, exactly matching the reported standby state.
    // child 已完成但仍开放，与报告中的待命状态一致。
    let child = SubagentTaskPanelState {
        task_id: SessionId::new_v4(),
        parent_session_id: handle.session_id(),
        public_task_id: String::from("standby-agent"),
        subagent_label: String::from("standby-agent"),
        description: None,
        run_state: SubagentTaskRunState::Completed,
        result_text: String::from("online"),
        conversation_state: None,
    };
    // service_state seeds the production runtime registry, not a synthetic close projection.
    // service_state 填充生产运行时注册表，而不是合成关闭投影。
    let mut service_state = test_service_state_with_parent_session(handle.session_id());
    seed_test_live_subagent_registry_from_panel_states(
        &mut service_state,
        handle.session_id(),
        std::slice::from_ref(&child),
    );
    service_state.subagent_surface_detail_cache.insert(
        child.task_id,
        crate::state::WebuiSubagentSurfaceDetailCacheEntry {
            task_state: child.clone(),
            last_access_ms: 0,
        },
    );
    service_state
        .detached_subagent_task_listeners
        .insert(child.task_id, String::from("original-close-execution"));
    // runtime performs the actual close; state retains the same live cache used by the event handler.
    // runtime 执行真实关闭；state 保留事件处理器使用的同一实时缓存。
    let runtime = service_state.agent_runtime.clone().expect("seeded runtime");
    let state = Arc::new(std::sync::Mutex::new(service_state));
    // capture observes the same service deltas consumed by the frontend list.
    // capture 观察前端列表消费的同一类服务增量。
    let capture = Arc::new(ToolLifecycleEventSink {
        handle: Arc::clone(&handle),
        states: std::sync::Mutex::new(Vec::new()),
    });
    // app forwards service deltas to the test observer without any navigation refresh.
    // app 将服务增量转发给测试观察器，不触发任何导航刷新。
    let app = vulcan_service_api::ServiceEventEmitter::new(vec![capture.clone()]);
    crate::emit_webui_subagent_navigation_changed_delta_for_parent(
        &app,
        &state,
        handle.session_id(),
    )
    .expect("initial standby navigation");
    // closed is the real tool result produced after the registry archive commits.
    // closed 是注册表归档提交后产出的真实工具结果。
    let closed = runtime
        .task_runtime
        .invoke_close_agent(handle.session_id(), json!({"target": "standby-agent"}))
        .await
        .expect("close actual child");
    // host_result retains the runtime producer's exact kind and payload without synthesizing closure fields.
    // host_result 保留运行时生产者的精确 kind 和 payload，不合成关闭字段。
    let host_result = closed.host_result.expect("close host result");
    // result preserves the production executor's typed tool completion envelope.
    // result 保留生产执行器的强类型工具完成信封。
    let result = ToolInvocationResult {
        tool_call_id: String::from("close-1"),
        tool_name: String::from("close_agent"),
        model_output: closed.model_output.to_string(),
        host_result: Some(json!({"kind": host_result.kind, "payload": host_result.payload})),
        is_error: false,
    };
    // metadata is generated with the same formatter as StreamingRuntimeToolExecutor.
    // metadata 由 StreamingRuntimeToolExecutor 使用的同一格式化器生成。
    let metadata = attach_tool_name_to_runtime_metadata(
        Some(build_tool_result_runtime_metadata(
            "close-1",
            result.host_result.as_ref(),
            "completed",
        )),
        "close_agent",
    );
    // event carries both the typed V1 boundary and the executor's canonical display metadata.
    // event 同时携带强类型 V1 边界与执行器的规范显示元数据。
    let event = RuntimeEvent::new(
        RuntimeEventKind::ToolFinished,
        "Tool result · close-1",
        "closed",
    )
    .with_tool_execution_boundary(ToolExecutionRecoveryBoundary::Finished(result))
    .with_metadata(metadata);
    // telemetry uses the same service state as the production detached-event entry.
    // telemetry 使用与生产独立事件入口相同的服务状态。
    let mut telemetry = ProviderTelemetryHookGuard::new_with_persistence(Arc::clone(&state), None);
    // handled must complete all lifecycle projection before returning to the executor.
    // handled 必须在返回执行器前完成全部生命周期投影。
    let handled = handle_webui_detached_child_execution_event(
        &app,
        &state,
        &runtime,
        handle.session_id(),
        "close-execution",
        &mut telemetry,
        &mut sink,
        ExecutionStreamEvent::RuntimeEvent {
            session_id: handle.session_id(),
            event,
        },
    )
    .await;
    if !start_close_tool {
        handled.expect_err("an unstarted close cannot be durably completed");
        assert_eq!(
            *capture.states.lock().expect("captured navigation states"),
            vec![String::from("completed")]
        );
        assert!(
            state
                .lock()
                .expect("service state")
                .subagent_surface_detail_cache
                .contains_key(&child.task_id)
        );
        return;
    }
    handled.expect("real V1 event handler");
    assert_eq!(
        *capture.states.lock().expect("captured navigation states"),
        vec![String::from("completed"), String::from("closed")]
    );
    assert!(
        !state
            .lock()
            .expect("service state")
            .subagent_surface_detail_cache
            .contains_key(&child.task_id)
    );
    assert_eq!(
        state
            .lock()
            .expect("service state")
            .detached_subagent_task_listeners
            .get(&child.task_id)
            .map(String::as_str),
        Some("original-close-execution"),
        "a close navigation projection must retain the original terminal listener"
    );
    assert_eq!(sink.api_terminal_completion.is_none(), tool_call_count == 2);
    assert_eq!(sink.active.is_some(), tool_call_count == 2);
}

/// Verifies closure is pushed immediately through the real stream entry without opening the child panel.
/// 验证关闭通过真实流入口立即推送，无需打开子面板。
/// Parameters: none. Returns: none; fails if the navigation remains in standby.
/// 参数：无。返回值：无；若导航仍为待命则失败。
#[tokio::test]
async fn v1_close_agent_event_pushes_closed_navigation_without_reopening_panel() {
    assert_v1_close_agent_navigation(1, true).await;
}

/// Verifies a closed child is published while another tool in the same physical API remains unfinished.
/// 验证同一物理 API 中仍有未完成工具时，也会发布子任务关闭状态。
/// Parameters: none. Returns: none; fails if projection waits for the remaining tool or API completion.
/// 参数：无。返回值：无；若投影等待剩余工具或 API 完成则失败。
#[tokio::test]
async fn v1_close_agent_event_pushes_closed_navigation_before_peer_tool_finishes() {
    assert_v1_close_agent_navigation(2, true).await;
}

/// Verifies a rejected V1 completion does not publish closure or evict the child detail cache.
/// 验证被拒绝的 V1 完成边界不会发布关闭状态或清除子任务详情缓存。
/// Parameters: none. Returns: none; fails if lifecycle changes precede durable completion.
/// 参数：无。返回值：无；若生命周期变化早于可靠完成则失败。
#[tokio::test]
async fn v1_close_agent_event_rejects_navigation_projection_when_boundary_fails() {
    assert_v1_close_agent_navigation(1, false).await;
}

/// Verifies one summary delta keeps the whole reasoning event out of replay, through completion.
/// 验证一条摘要增量会使整条思考事件在完成后仍排除在重放之外。
#[tokio::test]
async fn summary_reasoning_delta_marks_the_event_display_only() {
    let handle = published_open_turn();
    let mut sink = ProviderV1StreamSink::new(
        Arc::clone(&handle),
        Arc::new(ProjectResourceCatalogRegistry::default()),
    );
    prepare_sink_request(&handle, &mut sink);
    sink.append_reasoning_delta("summary", true, 4)
        .expect("summary delta");
    sink.append_reasoning_delta(" model", false, 5)
        .expect("model delta");
    sink.complete_reasoning(6).expect("reasoning complete");

    let reasoning = handle
        .read(|document| {
            document
                .events()
                .iter()
                .find_map(|event| match &event.event {
                    EventBody::AssistantReasoning(reasoning) => Some(reasoning.clone()),
                    _ => None,
                })
        })
        .expect("read reasoning")
        .expect("reasoning event");
    assert_eq!(reasoning.text, "summary model");
    assert_eq!(reasoning.status, AssistantSemanticStatus::Completed);
    assert!(reasoning.display_only);
}

/// Verifies api_started is durable before semantic streaming and terminal closure.
/// 验证 api_started 在语义流与终态闭合前已经可靠落盘。
#[tokio::test]
async fn tool_free_request_persists_ordered_v1_lifecycle() {
    let handle = published_open_turn();
    let mut sink = ProviderV1StreamSink::new(
        Arc::clone(&handle),
        Arc::new(ProjectResourceCatalogRegistry::default()),
    );
    prepare_sink_request(&handle, &mut sink);
    let (memory_revision, durable_revision, bodies) = handle
        .read(|document| {
            (
                document.memory_revision(),
                document.durable_revision(),
                document
                    .events()
                    .iter()
                    .map(|event| event.event.clone())
                    .collect::<Vec<_>>(),
            )
        })
        .expect("read api start");
    assert_eq!(memory_revision, durable_revision);
    assert!(matches!(bodies.last(), Some(EventBody::ApiStarted(_))));
    // Capture the actual API start before later host-only lifecycle events are appended.
    // 在追加后续宿主专属生命周期事件前捕获真实 API 起点。
    let api_start_index = bodies.len() - 1;

    sink.append_reasoning_delta("reason", false, 4)
        .expect("reasoning delta");
    sink.complete_reasoning(5).expect("reasoning complete");
    sink.append_assistant_delta("answer", 6)
        .expect("assistant delta");
    assert_eq!(
        sink.accept_terminal(terminal_facts(0))
            .await
            .expect("terminal boundary"),
        ProviderV1TerminalOutcome::Completed
    );
    let bodies = handle
        .read(|document| {
            document
                .events()
                .iter()
                .map(|event| event.event.clone())
                .collect::<Vec<_>>()
        })
        .expect("read complete lifecycle");
    // Natural idle is part of the same durable closure, after the ordered API and Turn statistics.
    // 自然空闲属于同一次可靠闭合，位于有序 API 及回合统计之后。
    let lifecycle = &bodies[api_start_index..];
    assert_eq!(lifecycle.len(), 7, "unexpected lifecycle: {lifecycle:?}");
    assert!(matches!(&lifecycle[0], EventBody::ApiStarted(_)));
    assert!(matches!(&lifecycle[1], EventBody::AssistantReasoning(_)));
    assert!(matches!(&lifecycle[2], EventBody::AssistantText(_)));
    assert!(matches!(&lifecycle[3], EventBody::ApiStats(_)));
    assert!(matches!(&lifecycle[4], EventBody::TurnBoundary(_)));
    assert!(matches!(&lifecycle[5], EventBody::TurnStats(_)));
    assert!(matches!(&lifecycle[6], EventBody::Collaboration(event)
            if matches!(&event.after_state,
                vulcan_session_core::v1::collaboration::CollaborationAfterState::RunControl { transition }
                if transition.state == vulcan_session_core::v1::collaboration::CollaborationRunControl::NaturallyIdle)));
    let durability = sink.take_finished_durability();
    assert!(
        durability.api_terminal.is_some(),
        "the physical API terminal must remain available to finished handling"
    );
    assert!(!durability.api_terminal_failure_persisted);
    let receipt = durability
        .turn_completion
        .expect("final assistant should produce one durable Turn receipt");
    assert_eq!(receipt.value.successor, TurnSuccessor::None);
    let consumed = sink.take_finished_durability();
    assert!(consumed.api_terminal.is_none());
    assert!(!consumed.api_terminal_failure_persisted);
    assert!(consumed.turn_completion.is_none());
}

/// Verifies display-free summaries still persist API facts, answer barriers and preserve business ownership.
/// 验证无展示摘要仍持久化 API 事实、响应屏障并保留业务归属。
/// No parameters or return value; assertions exercise valid and rejected summary payloads through the shared receiver.
/// 无参数及返回值；断言通过共用接收器验证有效和被拒绝的摘要载荷。
#[tokio::test]
async fn compaction_without_root_display_keeps_durable_boundaries() {
    use vulcan_agent_runtime::agent_runtime::events::{
        ExecutionStreamEvent, test_provider_request_durability_barrier,
    };
    use vulcan_agent_runtime::agent_runtime::provider_request_telemetry::{
        ProviderRequestTelemetryEvent, ProviderRequestTelemetryEventKind,
    };
    use vulcan_agent_runtime::agent_runtime::provider_v1_bridge::ProviderV1LifecycleFacts;

    for (valid, output) in [
        (
            true,
            "<summary>Retain the original task identity.</summary>",
        ),
        (false, "<analysis>No final summary was produced.</analysis>"),
        (false, ""),
        (false, "   "),
    ] {
        // This session deliberately has no root workspace presentation row.
        // 此会话故意不拥有根工作区展示行。
        let handle = published_open_turn();
        // A different visible root proves persistence does not depend on display selection.
        // 不同的可见根会话证明持久化不依赖展示选择。
        let service_state =
            crate::tests::test_support::test_service_state_with_parent_session(SessionId::new_v4());
        service_state
            .session_runtime_registry
            .register(Arc::clone(&handle))
            .unwrap();
        // The real service registry and telemetry guard are retained throughout this physical request.
        // 在整个物理请求期间保留真实服务注册表与遥测守卫。
        let state = Arc::new(std::sync::Mutex::new(service_state));
        // No display subscribers are needed to complete the durable protocol.
        // 完成可靠协议不需要展示订阅者。
        let app = crate::ServiceEventEmitter::new(Vec::new());
        // The sink has one exact registered session owner.
        // 接收器具有一个精确注册会话所有者。
        let mut sink = ProviderV1StreamSink::new(
            Arc::clone(&handle),
            Arc::new(ProjectResourceCatalogRegistry::default()),
        );
        // Freeze compaction purpose before opening the actual API.
        // 打开实际 API 之前冻结压缩用途。
        let mut facts = request_facts();
        facts.purpose = ApiPurposeKind::Compaction;
        prepare_sink_request_with_facts(&handle, &mut sink, facts.clone());
        // Cancellation uses the production guard independently of optional root display.
        // 取消使用生产守卫，独立于可选根展示。
        let mut telemetry =
            crate::ProviderTelemetryHookGuard::new_with_persistence(Arc::clone(&state), None);
        // Local display identity stays absent while the summary text is retained for canonical normalization.
        // 局部展示身份保持不存在，而摘要文本被保留供规范化使用。
        let mut output_id = None;
        // Invalid tagged output must be rejected without keeping the API active.
        // 必须拒绝无效标签输出，并且不能让 API 保持活动。
        let mut raw = String::new();
        for event in [
            ExecutionStreamEvent::RequestStarted {
                session_id: handle.session_id(),
            },
            ExecutionStreamEvent::OutputDelta {
                session_id: handle.session_id(),
                delta: output.into(),
            },
        ] {
            assert!(
                crate::context_compaction::apply_context_compaction_stream_event(
                    &app,
                    &state,
                    None,
                    &mut output_id,
                    &mut raw,
                    &mut sink,
                    &mut telemetry,
                    event,
                )
                .await
                .unwrap()
                .is_none()
            );
        }
        assert!(output_id.is_none());
        // Observe the same one-shot barrier used by real Provider completion.
        // 观察真实 Provider 完成使用的同一一次性屏障。
        let (completion, waiter) = test_provider_request_durability_barrier();
        // Terminal facts belong to this same compaction API, never the visible root.
        // 终态事实属于同一次压缩 API，绝不属于可见根会话。
        let mut terminal = terminal_facts(0);
        terminal.request_facts = facts;
        // This envelope carries the real typed lifecycle owner through the shared receiver.
        // 此封套通过共用接收器携带真实类型化生命周期所有者。
        let event = ExecutionStreamEvent::ProviderRequestDurabilityBoundary {
            event: ProviderRequestTelemetryEvent {
                kind: ProviderRequestTelemetryEventKind::Completed,
                session_id: handle.session_id(),
                payload: json!({}),
                completed_usage: None,
                v1_lifecycle: Some(ProviderV1LifecycleFacts::ApiTerminal(terminal)),
            },
            completion,
        };
        // Check both the host return and the actual producer wake-up result.
        // 同时检查宿主返回与实际生产者唤醒结果。
        let result = crate::context_compaction::apply_context_compaction_stream_event(
            &app,
            &state,
            None,
            &mut output_id,
            &mut raw,
            &mut sink,
            &mut telemetry,
            event,
        )
        .await;
        // A receiver must never wait for a dropped or silently acknowledged error.
        // 接收方绝不能等待被丢弃或静默确认的错误。
        let receipt = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .unwrap();
        if valid {
            assert!(result.unwrap().is_none());
            receipt.unwrap();
        } else {
            assert_eq!(
                receipt,
                Err(String::from(
                    "Provider output did not contain a valid compaction summary."
                ))
            );
            assert_eq!(result.unwrap_err(), receipt.unwrap_err());
        }
        handle
            .read(|document| {
                assert!(document.active_api().is_none());
                assert!(!document.active_turn().unwrap().closed);
                assert_eq!(document.memory_revision(), document.durable_revision());
                assert!(
                    document
                        .events()
                        .iter()
                        .any(|row| matches!(row.event, EventBody::ApiStats(_)))
                );
                assert!(
                    !document
                        .events()
                        .iter()
                        .any(|row| matches!(row.event, EventBody::TurnBoundary(_)))
                );
                assert!(document.events().iter().any(|row| {
                    matches!(&row.event, EventBody::AssistantText(text)
                            if text.status == AssistantSemanticStatus::Completed)
                }));
            })
            .unwrap();
        // Rejected physical output remains auditable, but its error forbids a compaction-node commit.
        // 被拒绝的物理输出保留可审计性，但对应错误禁止提交压缩节点。
        let durability = sink.take_finished_durability();
        assert!(durability.api_terminal.is_some());
        assert!(durability.completed_assistant_text.is_some());
        assert!(durability.turn_completion.is_none());
        if let Some(assistant) = durability.completed_assistant_text {
            assert_eq!(
                assistant.text,
                if valid {
                    "Retain the original task identity."
                } else {
                    output
                }
            );
        }
        assert!(
            !state
                .lock()
                .unwrap()
                .workspace_snapshot
                .as_ref()
                .unwrap()
                .session_run_phases
                .contains_key(&handle.session_id())
        );
    }
}

/// Settles all real policy rejections before refusing a tool-bearing summary through its final barrier.
/// 通过最后屏障拒绝带工具的摘要之前，结算全部真实策略拒绝结果。
/// No parameters or return value; asserts partial-batch ownership and durable final closure.
/// 无参数和返回值；断言部分批次归属与可靠最终闭合。
#[tokio::test]
async fn compaction_tool_rejections_settle_before_final_barrier_failure() {
    use vulcan_agent_runtime::agent_runtime::events::{
        ExecutionStreamEvent, RuntimeEvent, RuntimeEventKind,
        test_provider_request_durability_barrier,
    };
    use vulcan_agent_runtime::agent_runtime::provider_request_telemetry::{
        ProviderRequestTelemetryEvent, ProviderRequestTelemetryEventKind,
    };
    use vulcan_agent_runtime::agent_runtime::provider_v1_bridge::ProviderV1LifecycleFacts;
    use vulcan_agent_runtime::agent_runtime::tool_runtime_bridge::{
        attach_test_runtime_event_projection_barrier, test_tool_rejection_invocation_result,
    };

    for output in ["", "I will call two tools."] {
        // This exact hidden child ledger owns the physical request and all its tool boundaries.
        // 此精确隐藏子账本持有物理请求及其全部工具边界。
        let handle = published_open_turn();
        // No root presentation is installed for the child request.
        // 不为子请求安装根展示。
        let service_state =
            crate::tests::test_support::test_service_state_with_parent_session(SessionId::new_v4());
        service_state
            .session_runtime_registry
            .register(Arc::clone(&handle))
            .unwrap();
        // Keep the real service owner throughout all durable writes.
        // 全部可靠写入期间保留真实服务所有者。
        let state = Arc::new(std::sync::Mutex::new(service_state));
        // No UI subscriber is required for the real persistence barriers.
        // 真实持久化屏障不需要 UI 订阅者。
        let app = crate::ServiceEventEmitter::new(Vec::new());
        // The sink remains attached to one exact child handle.
        // 接收器始终绑定一个精确子句柄。
        let mut sink = ProviderV1StreamSink::new(
            Arc::clone(&handle),
            Arc::new(ProjectResourceCatalogRegistry::default()),
        );
        // Freeze the physical purpose before its start is admitted.
        // 准入开始前冻结物理用途。
        let mut facts = request_facts();
        facts.purpose = ApiPurposeKind::Compaction;
        // The actual telemetry guard observes completion without inventing cancellation usage.
        // 实际遥测守卫观察完成，不虚构取消用量。
        let mut telemetry =
            crate::ProviderTelemetryHookGuard::new_with_persistence(Arc::clone(&state), None);
        // Retain the exact optional physical output, even though it is not a valid summary.
        // 保留精确可选物理输出，即使其不是有效摘要。
        let mut raw = output.to_owned();
        // Hidden execution must not allocate a root display identity.
        // 隐藏执行不得分配根展示身份。
        let mut output_id = None;
        // The tool coordinator emits the preview before the host adds the physical API identity.
        // 工具协调器先发出预览，再由宿主补充物理 API 身份。
        let formation = ExecutionStreamEvent::RuntimeEvent {
            session_id: handle.session_id(),
            event: RuntimeEvent::new(RuntimeEventKind::ToolRequestStarted, "", "")
                .with_metadata(Some(json!({ "request_stream_id": "rejected-one" }))),
        };
        assert!(
            crate::context_compaction::apply_context_compaction_stream_event(
                &app,
                &state,
                None,
                &mut output_id,
                &mut raw,
                &mut sink,
                &mut telemetry,
                formation.clone(),
            )
            .await
            .is_err(),
            "an unbound compaction preview must fail before handoff"
        );
        prepare_sink_request_with_facts(&handle, &mut sink, facts.clone());
        // Capture the owner that must survive forwarding through the original child listener.
        // 记录必须在原子任务监听器转发过程中保留的归属。
        let compaction_api = sink.request_preview_api_id().unwrap();
        let Some(ExecutionStreamEvent::RuntimeEvent {
            event: mut forwarded,
            ..
        }) = crate::context_compaction::apply_context_compaction_stream_event(
            &app,
            &state,
            None,
            &mut output_id,
            &mut raw,
            &mut sink,
            &mut telemetry,
            formation,
        )
        .await
        .unwrap()
        else {
            panic!("child compaction must forward its bound preview");
        };
        // The original listener can be idle while compaction owns the active API.
        // 压缩持有活动 API 时，原监听器可以处于空闲状态。
        let listener_sink = ProviderV1StreamSink::new(
            Arc::clone(&handle),
            Arc::new(ProjectResourceCatalogRegistry::default()),
        );
        listener_sink
            .bind_tool_request_preview(&mut forwarded)
            .unwrap();
        assert_eq!(
            forwarded.metadata.unwrap()["api_call_id"],
            json!(compaction_api)
        );
        // Two tools prove the first rejection cannot prematurely close the physical group.
        // 两个工具证明首个拒绝不能提前关闭物理组。
        let mut terminal = terminal_facts(2);
        terminal.request_facts = facts;
        // Use the producer's real one-shot completion barrier.
        // 使用生产者真实的一次性完成屏障。
        let (completion, waiter) = test_provider_request_durability_barrier();
        crate::context_compaction::apply_context_compaction_stream_event(
            &app,
            &state,
            None,
            &mut output_id,
            &mut raw,
            &mut sink,
            &mut telemetry,
            ExecutionStreamEvent::ProviderRequestDurabilityBoundary {
                event: ProviderRequestTelemetryEvent {
                    kind: ProviderRequestTelemetryEventKind::Completed,
                    session_id: handle.session_id(),
                    payload: json!({}),
                    completed_usage: None,
                    v1_lifecycle: Some(ProviderV1LifecycleFacts::ApiTerminal(terminal)),
                },
                completion,
            },
        )
        .await
        .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .unwrap()
            .unwrap();
        assert!(!sink.has_durable_api_terminal());
        // These are real recovery requests whose local IDs are allocated by the production batch writer.
        // 这些是真实恢复请求，其本地 ID 由生产批次写入器分配。
        let requests = vec![
            recovery_request(
                "rejected-one",
                "shell_command",
                json!({"command": "never execute"}),
            ),
            recovery_request("rejected-two", "file_edit", json!({"path": "never write"})),
        ];
        // Replay the same ordering emitted by the real rejected-tool executor.
        // 重放真实被拒绝工具执行器发出的相同顺序。
        let mut boundaries = vec![(
            RuntimeEventKind::ToolBatchPrepared,
            ToolExecutionRecoveryBoundary::BatchPrepared(requests.clone()),
        )];
        for request in &requests {
            boundaries.push((
                RuntimeEventKind::ToolStarted,
                ToolExecutionRecoveryBoundary::Started {
                    request: request.request.clone(),
                    binding_fingerprint: request.binding_fingerprint.clone(),
                },
            ));
        }
        for request in &requests {
            boundaries.push((
                RuntimeEventKind::ToolFinished,
                ToolExecutionRecoveryBoundary::Finished(test_tool_rejection_invocation_result(
                    &request.request,
                    "Tool calls are not allowed during this internal execution.",
                )),
            ));
        }
        for (index, (kind, boundary)) in boundaries.into_iter().enumerate() {
            // Every transition carries the production barrier, including the last rejection.
            // 每次转换都携带生产屏障，包括最后拒绝。
            let (event, waiter) = attach_test_runtime_event_projection_barrier(
                RuntimeEvent::new(kind, "summary policy rejection", "durable tool boundary")
                    .with_tool_execution_boundary(boundary),
            )
            .unwrap();
            // Only the final tool has permission to end this failed summary attempt.
            // 只有最后工具有权结束本次失败摘要尝试。
            let result = crate::context_compaction::apply_context_compaction_stream_event(
                &app,
                &state,
                None,
                &mut output_id,
                &mut raw,
                &mut sink,
                &mut telemetry,
                ExecutionStreamEvent::RuntimeEvent {
                    session_id: handle.session_id(),
                    event,
                },
            )
            .await;
            // Observe the actual producer receipt instead of only the host return value.
            // 观察实际生产者回执，而非仅宿主返回值。
            let receipt = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
                .await
                .unwrap();
            if index == 4 {
                assert!(
                    result
                        .unwrap_err()
                        .contains("after settling its tool batch")
                );
                assert!(
                    receipt
                        .unwrap_err()
                        .contains("after settling its tool batch")
                );
                assert!(sink.has_durable_api_terminal());
            } else {
                assert!(result.unwrap().is_none());
                receipt.unwrap();
                assert!(!sink.has_durable_api_terminal());
            }
        }
        handle.read(|document| {
                assert!(document.active_api().is_none());
                assert!(!document.active_turn().unwrap().closed);
                assert_eq!(document.memory_revision(), document.durable_revision());
                assert_eq!(document.events().iter().filter(|row| matches!(&row.event, EventBody::ApiStats(_))).count(), 1);
                assert_eq!(document.events().iter().filter(|row| matches!(&row.event,
                    EventBody::ToolCall(tool) if tool.status == ToolCallStatus::Completed && tool.output.is_some()
                )).count(), 2);
                assert!(!document.events().iter().any(|row| matches!(&row.event, EventBody::TurnBoundary(_))));
            }).unwrap();
        assert!(sink.take_finished_durability().turn_completion.is_none());
        assert!(output_id.is_none());
    }
}

/// Verifies question-purpose API completion keeps business closure with the dedicated owner.
/// 验证答疑用途 API 完成将业务闭合保留给专用 owner；无参数和返回值，失败通过断言报告。
#[tokio::test]
async fn question_api_terminal_does_not_complete_business_turn() {
    // The open ordinary turn represents an in-turn question execution at this sink boundary.
    // 此 sink 边界中的普通开放回合表示原回合内的答疑执行。
    let handle = published_open_turn();
    // The sink retains only physical-request facts and must not manufacture logical completion.
    // sink 仅保留物理请求事实，不得生成逻辑完成。
    let mut sink = ProviderV1StreamSink::new(
        Arc::clone(&handle),
        Arc::new(ProjectResourceCatalogRegistry::default()),
    );
    // The purpose is frozen before the real durable API preparation.
    // 用途在真实可靠 API 准备之前冻结。
    let mut facts = request_facts();
    facts.purpose = ApiPurposeKind::QuestionExecution;
    prepare_sink_request_with_facts(&handle, &mut sink, facts.clone());
    sink.append_assistant_delta("No additional answer.", 6)
        .expect("persist actual assistant output");
    // A final physical response without a reply tool is not a business completion receipt.
    // 没有回复工具的物理最终响应不构成业务完成回执。
    let mut terminal = terminal_facts(0);
    terminal.request_facts = facts;
    assert_eq!(
        sink.accept_terminal(terminal).await.expect("API terminal"),
        ProviderV1TerminalOutcome::Completed
    );
    handle
        .read(|document| {
            assert!(!document.active_turn().expect("original turn").closed);
            assert!(matches!(
                document.events().last().map(|event| &event.event),
                Some(EventBody::ApiStats(_))
            ));
            assert_eq!(document.memory_revision(), document.durable_revision());
        })
        .expect("read durable API-only result");
    // Physical usage is available once, while logical completion remains absent.
    // 物理用量可提取一次，而逻辑完成始终不存在。
    let durability = sink.take_finished_durability();
    assert!(durability.api_terminal.is_some());
    assert!(durability.turn_completion.is_none());
    assert!(sink.take_finished_durability().api_terminal.is_none());
}

/// Verifies failed API completion transfers an explicit durable-failure marker exactly once.
/// 验证失败 API 完成会精确转移一次明确的可靠失败标记。
///
/// # Parameters
/// 参数
/// This test has no parameters.
/// 此测试没有参数。
///
/// # Returns
/// 返回值
/// This test returns no value and fails through assertions.
/// 此测试无返回值，并通过断言报告失败。
#[tokio::test]
async fn failed_api_terminal_transfers_durable_failure_marker_once() {
    // handle owns the isolated durable session used by the failed terminal.
    // handle 持有失败终态使用的隔离可靠会话。
    let handle = published_open_turn();
    // sink owns the exact physical request and its once-only finished facts.
    // sink 持有精确物理请求及其仅可转移一次的结束事实。
    let mut sink = ProviderV1StreamSink::new(
        Arc::clone(&handle),
        Arc::new(ProjectResourceCatalogRegistry::default()),
    );
    prepare_sink_request(&handle, &mut sink);
    // terminal is a contract-valid failed fact with no replay source or continuation permission.
    // terminal 是一份契约合法的失败事实，不携带重放来源或继续权限。
    let mut terminal = terminal_facts(0);
    terminal.status = ApiCallStatus::Failed;
    terminal.end_reason = Some(ApiEndReason::Unknown);
    terminal.error = Some(RedactedApiError {
        category: ApiErrorCategory::Unknown,
        error_code: Some(String::from("test_failure")),
        message: String::from("controlled failure"),
    });
    terminal.provider_source = None;

    assert_eq!(
        sink.accept_terminal(terminal)
            .await
            .expect("failed terminal boundary"),
        ProviderV1TerminalOutcome::Completed
    );
    // durability is the first and only transfer of the persisted failed API terminal.
    // durability 是已持久化失败 API 终态的首次且唯一一次转移。
    let durability = sink.take_finished_durability();
    assert!(durability.api_terminal.is_some());
    assert!(durability.api_terminal_failure_persisted);
    // consumed proves a second transfer cannot repeat either the receipt or its failure marker.
    // consumed 证明第二次转移不会重复回执或其失败标记。
    let consumed = sink.take_finished_durability();
    assert!(consumed.api_terminal.is_none());
    assert!(!consumed.api_terminal_failure_persisted);
}

/// Verifies a child return committed during the parent API is transferred to stream-finished dispatch instead of closing the Turn.
/// 验证父级 API 期间提交的子代理回流会转交流终态分发，而不是关闭回合。
#[tokio::test]
async fn child_return_during_parent_api_defers_turn_completion_to_exact_continuation() {
    // childHandle produces the exact terminal child facts required by the parent return contract.
    // childHandle 产生父级回流契约要求的精确子会话终态事实。
    let child_handle = published_open_turn();
    let mut child_sink = ProviderV1StreamSink::new(
        Arc::clone(&child_handle),
        Arc::new(ProjectResourceCatalogRegistry::default()),
    );
    prepare_sink_request(&child_handle, &mut child_sink);
    child_sink
        .append_assistant_delta("durable child result", 5)
        .expect("append child assistant result");
    child_sink
        .accept_terminal(terminal_facts(0))
        .await
        .expect("complete child turn");
    // childDurability retains both the child final Turn and its exact assistant source seq.
    // childDurability 同时保留子会话最终回合及其精确 assistant 来源 seq。
    let child_durability = child_sink.take_finished_durability();
    let child_turn_id = child_durability
        .api_terminal
        .as_ref()
        .expect("child API terminal")
        .value
        .turn_id;
    let child_assistant_seq = child_durability
        .completed_assistant_text
        .as_ref()
        .expect("child assistant semantic")
        .seq;
    let return_input = child_handle
        .read(|document| {
            document.build_subagent_return_input(SubagentReturnBuildRequest {
                final_turn_id: child_turn_id,
                child_task_id: SubagentTaskId::try_new("child-return-during-parent-api")?,
                source_assistant_text_seq: child_assistant_seq,
            })
        })
        .expect("read child document")
        .expect("build exact child return");

    // parentSink remains physically active when the child return enters the same logical Turn.
    // parentSink 在子代理回流进入同一逻辑回合时仍持有活动物理 API。
    let parent_handle = published_open_turn();
    let mut parent_sink = ProviderV1StreamSink::new(
        Arc::clone(&parent_handle),
        Arc::new(ProjectResourceCatalogRegistry::default()),
    );
    prepare_sink_request(&parent_handle, &mut parent_sink);
    parent_sink
        .append_assistant_delta("parent still owns the API", 5)
        .expect("append parent assistant text");
    let appended_return = parent_handle
        .append_subagent_return(return_input, 12)
        .expect("append child return to active parent Turn")
        .value;
    // parentTerminal ends after the child return and releases only the physical API.
    // parentTerminal 在子代理回流后结束，且只释放物理 API。
    let mut parent_terminal = terminal_facts(0);
    parent_terminal.completed_at_ms = 20;
    parent_terminal.timing.completed_at_ms = Some(20);
    parent_sink
        .accept_terminal(parent_terminal)
        .await
        .expect("complete parent physical API");

    let durability = parent_sink.take_finished_durability();
    assert!(
        durability.turn_completion.is_none(),
        "a pending child return must keep the parent logical Turn open"
    );
    let pending_dispatch = parent_handle
        .read(|document| document.pending_subagent_return_dispatch())
        .expect("read parent document")
        .expect("validate pending child return");
    assert_eq!(pending_dispatch.as_ref(), Some(&appended_return));
}

/// Keeps the real parent sink open until owned child content and success publish together.
/// 使真实父接收器保持开放，直至所属子正文与成功一起发布。
/// Parameters: none. Returns: none; verifies durable publication, completion gating and retry.
/// 参数：无。返回值：无；验证可靠发布、完成门及重试。
#[tokio::test]
async fn owned_child_return_keeps_parent_open_until_publication() {
    use crate::session_runtime_registry_v1::{SessionFlushReason, SessionMutationAdmission};
    use vulcan_session_core::v1::collaboration::{
        ActivateChildTaskInput, CollaborationExecutionId, CollaborationOperationId,
        CollaborationParticipant, ReserveChildTaskInput,
    };
    // Real service dispatch requires the parent to exist in the authoritative SQLite index.
    // 真实服务分发要求父会话存在于权威 SQLite 索引。
    let parent_id = SessionId::new_v4();
    // Keep all indexed parent files alive through dispatch verification.
    // 在分发验证期间保持所有父索引文件存活。
    let dispatch_root = crate::tests::test_support::TestHostRuntimeRoot::new(parent_id);
    // Indexed creation must precede loading the canonical parent document.
    // 索引创建必须先于规范父文档加载。
    let dispatch_controller = dispatch_root.attached_host_runtime_handle().await;
    // The locator carries the actual shared project identity used by both participants.
    // 定位器携带双方参与者使用的实际共享项目身份。
    let parent_locator = dispatch_root.v1_storage_locator();
    // Preserve the declared project across the parent and the isolated child fixture.
    // 在父会话与隔离子夹具间保留声明项目。
    let project_id = parent_locator.project_id().to_owned();
    // Canonical loading and every parent mutation use the actual production file writer.
    // 规范加载及每项父修改均使用实际生产文件写入器。
    let parent_handle = SessionHandle::new(
        parent_id,
        parent_locator.clone(),
        vulcan_workspace_store::workspace_store::load_session_v1_full(&parent_locator)
            .unwrap()
            .document,
    );
    parent_handle
        .submit_root_input(
            ClientInputSubmission::new(
                ClientActionId::try_new("owned-parent").unwrap(),
                "root",
                vec![],
            ),
            2,
        )
        .unwrap();
    // Produce the opaque return using the real child sink's durable terminal facts.
    // 使用真实子接收器的可靠终态事实产生不透明回流。
    let child_id = SessionId::new_v4();
    // The child shares the parent's actual project while retaining the existing isolated writer seam.
    // 子会话共享父实际项目，同时保留既有隔离写入接口。
    let child_handle = SessionHandle::with_backend(
        child_id,
        SessionDocument::new(child_id.to_string(), &project_id, "child", 1),
        Arc::new(TestPersistenceBackend),
    );
    child_handle
        .submit_root_input(
            ClientInputSubmission::new(
                ClientActionId::try_new("owned-publication-execution").unwrap(),
                "root",
                vec![],
            ),
            2,
        )
        .unwrap();
    // First child preparation appends non-turn context after its root; it must not block publication.
    // 子代理首次准备在根输入后追加非回合上下文；这不能阻断结果发布。
    child_handle
        .mutate_durable(
            SessionMutationAdmission::ControlPlane,
            SessionFlushReason::MetadataChanged,
            |document| {
                document.append_system_context(
                    None,
                    "dynamic.developer",
                    vulcan_session_core::v1::event::SystemContextOperation::Upsert,
                    vulcan_session_core::v1::event::SystemContextScope::Session,
                    None,
                    Some(vulcan_session_core::v1::event::SystemRoleHint::HiddenUserShell),
                    "Return the final result to the parent agent.",
                    2,
                )
            },
        )
        .unwrap();
    let mut child_sink = ProviderV1StreamSink::new(
        Arc::clone(&child_handle),
        Arc::new(ProjectResourceCatalogRegistry::default()),
    );
    prepare_sink_request(&child_handle, &mut child_sink);
    child_sink
        .append_assistant_delta("owned child result", 5)
        .unwrap();
    child_sink.accept_terminal(terminal_facts(0)).await.unwrap();
    let child_durability = child_sink.take_finished_durability();
    let task_id = SubagentTaskId::try_new(child_handle.session_id().to_string()).unwrap();
    let input = child_handle
        .read(|document| {
            document.build_subagent_return_input(SubagentReturnBuildRequest {
                final_turn_id: child_durability
                    .api_terminal
                    .as_ref()
                    .unwrap()
                    .value
                    .turn_id,
                child_task_id: task_id.clone(),
                source_assistant_text_seq: child_durability
                    .completed_assistant_text
                    .as_ref()
                    .unwrap()
                    .seq,
            })
        })
        .unwrap()
        .unwrap();
    // Reserve and activate the exact owned child before the parent model completes.
    // 在父模型完成之前预留并激活精确所属子任务。
    let parent_turn = parent_handle
        .read(|document| document.active_turn().unwrap().turn_id)
        .unwrap();
    let execution = CollaborationExecutionId::try_new("owned-publication-execution").unwrap();
    parent_handle
        .mutate_durable(
            SessionMutationAdmission::ControlPlane,
            SessionFlushReason::MetadataChanged,
            |document| {
                document.reserve_child_task(ReserveChildTaskInput {
                    operation_id: CollaborationOperationId::try_new("owned-publication-reserve")
                        .unwrap(),
                    source_tool_name: String::from("spawn_agent"),
                    execution_id: execution.clone(),
                    task_id: task_id.clone(),
                    parent_turn_id: parent_turn,
                    child: CollaborationParticipant::new(
                        child_handle.session_id().to_string(),
                        parent_handle.session_id().to_string(),
                        project_id.as_str(),
                        1,
                        None,
                        Some(task_id.clone()),
                    )?,
                    reserved_at_ms: 3,
                })
            },
        )
        .unwrap();
    parent_handle
        .mutate_durable(
            SessionMutationAdmission::ControlPlane,
            SessionFlushReason::MetadataChanged,
            |document| {
                document.activate_child_task(ActivateChildTaskInput {
                    operation_id: CollaborationOperationId::try_new("owned-publication-activate")
                        .unwrap(),
                    execution_id: execution.clone(),
                    task_id,
                    active_at_ms: 4,
                })
            },
        )
        .unwrap();
    let mut parent_sink = ProviderV1StreamSink::new(
        Arc::clone(&parent_handle),
        Arc::new(ProjectResourceCatalogRegistry::default()),
    );
    prepare_sink_request(&parent_handle, &mut parent_sink);
    parent_sink
        .append_assistant_delta("parent physical output", 5)
        .unwrap();
    parent_sink
        .accept_terminal(terminal_facts(0))
        .await
        .unwrap();
    assert!(
        parent_sink
            .take_finished_durability()
            .turn_completion
            .is_none()
    );
    parent_handle
        .read(|document| {
            assert!(!document.active_turn().unwrap().closed);
            assert_eq!(
                document
                    .completion_obligations(parent_turn)
                    .open_child_task_ids
                    .len(),
                1
            );
            assert!(
                document
                    .pending_subagent_return_dispatch()
                    .unwrap()
                    .is_none()
            );
        })
        .unwrap();
    // One owner revision publishes both facts without quarantining the canonical handle.
    // 一个所有者修订发布两个事实，不隔离规范句柄。
    let before = parent_handle
        .read(|document| document.memory_revision())
        .unwrap();
    let published = parent_handle
        .append_execution_subagent_return(execution.clone(), input.clone(), 30)
        .unwrap();
    assert!(published.changed);
    assert_eq!(published.revision, before + 1);
    assert_eq!(published.value.parent_turn_id, parent_turn);
    parent_handle
        .read(|document| {
            assert_eq!(document.durable_revision(), published.revision);
            assert!(
                document
                    .completion_obligations(parent_turn)
                    .open_child_task_ids
                    .is_empty()
            );
            assert_eq!(
                document.pending_subagent_return_dispatch().unwrap(),
                Some(published.value.clone())
            );
            assert!(!document.active_turn().unwrap().closed);
        })
        .unwrap();
    let retry = parent_handle
        .append_execution_subagent_return(execution, input, 31)
        .unwrap();
    assert!(!retry.changed);
    assert_eq!(retry.revision, published.revision);
    // Register the existing canonical owner so no test-created replacement history participates.
    // 登记既有规范所有者，避免测试新造替代历史参与判断。
    let mut dispatch_state = crate::tests::test_support::test_service_state_with_parent_session(
        parent_handle.session_id(),
    );
    dispatch_state.controller_runtime = Some(dispatch_controller.clone());
    dispatch_state
        .session_runtime_registry
        .register(parent_handle.clone())
        .unwrap();
    // Shared state follows the production service ownership boundary.
    // 共享状态遵守生产服务所有权边界。
    let dispatch_state = Arc::new(std::sync::Mutex::new(dispatch_state));
    // Runtime remains offline; the active API branch must return before model preparation.
    // 运行时保持离线；活动 API 分支必须在模型准备之前返回。
    let dispatch_runtime = crate::tests::test_support::test_agent_runtime_bridge();
    // Real durable auxiliary requests cannot discharge the parent's child-result continuation.
    // 真实持久辅助请求不能解除父会话的子结果续跑义务。
    for purpose in [
        ApiPurposeKind::Compaction,
        ApiPurposeKind::QuestionExecution,
    ] {
        // Use the same canonical parent and accepted result across both auxiliary requests.
        // 两次辅助请求共用同一规范父会话及已接纳结果。
        let mut auxiliary_sink = ProviderV1StreamSink::new(
            Arc::clone(&parent_handle),
            Arc::new(ProjectResourceCatalogRegistry::default()),
        );
        // Purpose is supplied to the actual prepared API, rather than editing stored events.
        // 用途传给实际准备的 API，而非修改已存储事件。
        let mut auxiliary_facts = request_facts();
        auxiliary_facts.purpose = purpose;
        auxiliary_facts.prepared_at_ms = 32;
        prepare_sink_request_with_facts(&parent_handle, &mut auxiliary_sink, auxiliary_facts);
        assert!(
            !Box::pin(
                crate::webui_turn_chaining::start_exact_v1_subagent_return_turn(
                    &vulcan_service_api::ServiceEventEmitter::new(vec![]),
                    &dispatch_state,
                    &dispatch_controller,
                    &dispatch_runtime,
                    parent_handle.session_id(),
                    published.value.clone(),
                )
            )
            .await
            .unwrap(),
            "an auxiliary API is not successful business dispatch"
        );
        parent_handle
            .mutate_durable(
                SessionMutationAdmission::ExistingTurn,
                SessionFlushReason::DisplayBoundary,
                |document| document.interrupt_active_api(33),
            )
            .unwrap();
        // Exercise the production completion gate under its own canonical writer lock.
        // 在规范写锁内执行生产完成门。
        let settlement = parent_handle.complete_successful_turn(34).await.unwrap();
        assert!(matches!(
            settlement.value,
            SuccessfulTurnSettlement::SubagentReturn
        ));
        assert!(!settlement.changed);
        parent_handle
            .read(|document| {
                assert!(!document.active_turn().unwrap().closed);
                assert_eq!(
                    document.pending_subagent_return_dispatch().unwrap(),
                    Some(published.value.clone())
                );
            })
            .unwrap();
    }
    // A later ordinary business API positively proves dispatch of the same original result.
    // 后续普通业务 API 正向证明同一原结果已分发。
    let mut business_sink = ProviderV1StreamSink::new(
        parent_handle.clone(),
        Arc::new(ProjectResourceCatalogRegistry::default()),
    );
    // Use real preparation again, so this is not a fabricated event predicate test.
    // 再次使用真实准备，避免仅测试伪造事件谓词。
    let mut business_facts = request_facts();
    business_facts.prepared_at_ms = 35;
    prepare_sink_request_with_facts(&parent_handle, &mut business_sink, business_facts);
    assert!(
        Box::pin(
            crate::webui_turn_chaining::start_exact_v1_subagent_return_turn(
                &vulcan_service_api::ServiceEventEmitter::new(vec![]),
                &dispatch_state,
                &dispatch_controller,
                &dispatch_runtime,
                parent_handle.session_id(),
                published.value,
            )
        )
        .await
        .unwrap()
    );
}

/// Exercises late completion through the production listener with or without a durable child receipt.
/// 有或没有可靠子任务回执时，通过生产监听器验证迟到完成事件。
/// Parameters: durable_child selects successful persistence or an explicit missing-receipt failure.
/// 参数：durable_child 选择持久化成功或显式缺失回执失败。
/// Returns: none; verifies immediate terminal pushes, cleanup, and exact continuation input.
/// 返回值：无；验证立即终态推送、清理与精确续跑输入。
async fn assert_late_child_completion_lifecycle(durable_child: bool) {
    use crate::agent_stream::{
        ProviderTelemetryHookGuard, handle_webui_detached_subagent_task_event,
    };
    use crate::tests::test_support::{
        TestHostRuntimeRoot, seed_test_live_subagent_registry_from_panel_states,
        test_service_state_with_parent_session,
    };
    use vulcan_agent_runtime::agent_runtime::events::{RuntimeEvent, RuntimeEventKind};
    use vulcan_agent_runtime::agent_runtime::task_runtime::DetachedTaskRuntimeEvent;
    use vulcan_workbench_model::{SessionRunPhase, SubagentTaskPanelState, SubagentTaskRunState};

    // parent_handle has already durably closed the initial user turn.
    // parent_handle 已经可靠关闭最初的用户回合。
    let (_parent_directory, parent_handle) = published_stored_open_turn();
    let mut parent_sink = ProviderV1StreamSink::new(
        Arc::clone(&parent_handle),
        Arc::new(ProjectResourceCatalogRegistry::default()),
    );
    prepare_sink_request(&parent_handle, &mut parent_sink);
    parent_sink
        .append_assistant_delta("child is still working", 5)
        .unwrap();
    parent_sink
        .accept_terminal(terminal_facts(0))
        .await
        .unwrap();
    // child_sink retains the exact source receipt used by the production completion path.
    // child_sink 保留生产完成路径使用的精确来源回执。
    let (_child_directory, child_handle) = published_stored_open_turn();
    let mut child_sink = ProviderV1StreamSink::new(
        Arc::clone(&child_handle),
        Arc::new(ProjectResourceCatalogRegistry::default()),
    );
    if durable_child {
        prepare_sink_request(&child_handle, &mut child_sink);
        child_sink
            .append_assistant_delta("LATE_CHILD_OK", 5)
            .unwrap();
        child_sink.accept_terminal(terminal_facts(0)).await.unwrap();
    }
    // child is terminal in the real registry while an existing listener still makes the projection active.
    // child 在真实注册表中已终态，但现存监听器仍会让投影显示活动。
    let child = SubagentTaskPanelState {
        task_id: child_handle.session_id(),
        parent_session_id: parent_handle.session_id(),
        public_task_id: String::from("late-child"),
        subagent_label: String::from("late-child"),
        description: None,
        run_state: SubagentTaskRunState::Completed,
        result_text: String::from("LATE_CHILD_OK"),
        conversation_state: None,
    };
    let mut service_state = test_service_state_with_parent_session(parent_handle.session_id());
    seed_test_live_subagent_registry_from_panel_states(
        &mut service_state,
        parent_handle.session_id(),
        std::slice::from_ref(&child),
    );
    service_state
        .session_runtime_registry
        .register(Arc::clone(&parent_handle))
        .unwrap();
    service_state
        .detached_subagent_task_listeners
        .insert(child.task_id, String::from("late-execution"));
    // A stopped parent must retain the result without invoking an external Provider during this test.
    // 测试中的已停止父会话必须保留结果，但不调用外部 Provider。
    service_state
        .workspace_snapshot
        .as_mut()
        .unwrap()
        .session_run_phases
        .insert(parent_handle.session_id(), SessionRunPhase::Stopped);
    let runtime = service_state.agent_runtime.clone().unwrap();
    let state = Arc::new(std::sync::Mutex::new(service_state));
    let capture = Arc::new(ToolLifecycleEventSink {
        handle: Arc::clone(&parent_handle),
        states: Default::default(),
    });
    let app = vulcan_service_api::ServiceEventEmitter::new(vec![capture.clone()]);
    crate::emit_webui_subagent_navigation_changed_delta_for_parent(
        &app,
        &state,
        parent_handle.session_id(),
    )
    .unwrap();
    // controller_root owns isolated offline storage; it never touches the user's session files.
    // controller_root 持有隔离离线存储，不接触用户会话文件。
    let controller_root = TestHostRuntimeRoot::new(parent_handle.session_id());
    let controller = controller_root.host_runtime_handle();
    let mut telemetry = ProviderTelemetryHookGuard::new_with_persistence(Arc::clone(&state), None);
    // Mirror the production listener's publication gate and typed content mailbox.
    // 复现生产监听器的发布门控与强类型正文邮箱。
    runtime
        .task_runtime
        .expect_durable_task_return(child.task_id, "late-execution")
        .unwrap();
    let mailbox = vulcan_session_boundary::SubagentMailboxEvent {
        parent_thread_id: parent_handle.session_id().into(),
        parent_turn_id: None,
        child_thread_id: child.task_id.into(),
        kind: vulcan_session_boundary::SubagentMailboxEventKind::Completed,
        timestamp_ms: 8,
        payload: json!({"status":"completed","result_text":"LATE_CHILD_OK"}),
    };
    let event = RuntimeEvent::new(RuntimeEventKind::DelegateFinished, "Child finished", "Completed").with_metadata(Some(json!({
            "kind": "subagent_task", "payload": {
                "task_id": "late-child", "subagent_task_id": child.task_id.to_string(),
                "execution_id": "late-execution", "execution_status": "completed", "subagent": "late-child", "result": "LATE_CHILD_OK", "subagent_mailbox_event": mailbox
            }
        })));
    let handled = handle_webui_detached_subagent_task_event(
        &app,
        &state,
        &controller,
        &runtime,
        child.task_id,
        "late-execution",
        child_handle.as_ref(),
        &mut telemetry,
        &mut child_sink,
        DetachedTaskRuntimeEvent::ParentRuntime {
            parent_session_id: parent_handle.session_id(),
            event,
        },
    )
    .await;
    assert_eq!(
        state
            .lock()
            .unwrap()
            .detached_subagent_task_listeners
            .contains_key(&child.task_id),
        !durable_child,
        "unpublished results must retain exact recovery ownership"
    );
    if durable_child {
        assert_eq!(
            capture.states.lock().unwrap().last().map(String::as_str),
            Some("completed")
        );
        handled.expect("late completion must be persisted without a user prompt");
        let receipt = parent_handle
            .read(|doc| doc.pending_subagent_return_dispatch())
            .unwrap()
            .unwrap()
            .expect("pending return");
        let authority =
            crate::webui_prompt_lifecycle::subagent_return_dispatch_authority_from_v1_handle(
                &parent_handle,
                &receipt,
            )
            .unwrap();
        assert_eq!(authority.content, "LATE_CHILD_OK");
        prepare_sink_request(&parent_handle, &mut parent_sink);
        assert!(
            parent_handle
                .read(|doc| doc.active_api().is_some())
                .unwrap()
        );
    } else {
        assert!(
            handled
                .unwrap_err()
                .contains("no exact durable V1 final assistant receipt")
        );
    }
}

/// Verifies late successful completion publishes terminal navigation and retains a dispatchable return.
/// 验证迟到成功完成会发布终态导航并保留可分发的回流。
/// Parameters: none. Returns: none; assertions cover the production listener entry.
/// 参数：无。返回值：无；断言覆盖生产监听器入口。
#[tokio::test]
async fn late_child_completion_pushes_state_without_new_user_input() {
    assert_late_child_completion_lifecycle(true).await;
}

/// Verifies failed publication retains exact recovery ownership without claiming a durable result.
/// 验证发布失败保留精确恢复归属，不宣称已有可靠结果。
/// Parameters: none. Returns: none; asserts explicit failure and the retained listener.
/// 参数：无。返回值：无；断言明确失败及保留监听器。
#[tokio::test]
async fn late_child_completion_error_retains_publication_owner() {
    assert_late_child_completion_lifecycle(false).await;
}

/// Executes native Goal mutations while the production V1 sink owns a running tool boundary.
/// 在生产 V1 sink 持有运行中工具边界时执行原生 Goal 修改。
/// Parameters: none. Returns: none; asserts creation, completion, replacement and durable tool closure.
/// 参数：无。返回值：无；断言创建、完成、替换以及可靠工具闭合。
#[tokio::test]
async fn native_goal_creation_inside_v1_tool_api_succeeds() {
    use vulcan_agent_runtime::tooling::native::{NativeGoalStatus, NativeGoalTerminalStatus};
    // handle owns the same active physical API observed by the native Goal runtime.
    // handle 持有原生 Goal 运行时观察到的同一个活动物理 API。
    let handle = published_open_turn();
    let mut sink = ProviderV1StreamSink::new(
        Arc::clone(&handle),
        Arc::new(ProjectResourceCatalogRegistry::default()),
    );
    prepare_sink_request(&handle, &mut sink);
    sink.accept_terminal(terminal_facts(1)).await.unwrap();
    let request = recovery_request(
        "goal-create",
        "create_goal",
        json!({"objective": "test goal"}),
    );
    sink.apply_tool_boundary(
        &ToolExecutionRecoveryBoundary::BatchPrepared(vec![request.clone()]),
        9,
    )
    .await
    .unwrap();
    sink.apply_tool_boundary(
        &ToolExecutionRecoveryBoundary::Started {
            request: request.request,
            binding_fingerprint: request.binding_fingerprint,
        },
        10,
    )
    .await
    .unwrap();
    let service_state =
        crate::tests::test_support::test_service_state_with_parent_session(handle.session_id());
    service_state
        .session_runtime_registry
        .register(Arc::clone(&handle))
        .unwrap();
    let state = Arc::new(std::sync::Mutex::new(service_state));
    let runtime = crate::session_goals::session_v1_native_goal_runtime(
        state,
        vulcan_service_api::ServiceEventEmitter::new(Vec::new()),
    );
    let goal = runtime
        .create_goal(handle.session_id(), String::from("test goal"), Some(2000))
        .await
        .expect("create_goal inside its running V1 tool");
    assert_eq!(goal.status, NativeGoalStatus::Active);
    runtime
        .update_goal_status(handle.session_id(), NativeGoalTerminalStatus::Complete)
        .await
        .unwrap();
    let replacement = runtime
        .create_goal(handle.session_id(), String::from("replacement goal"), None)
        .await
        .expect("replace completed goal inside tool API");
    assert_ne!(goal.goal_id, replacement.goal_id);
    // Tool completion must still target the original active API after Goal metadata changed.
    // Goal 元数据变化后，工具完成仍必须指向原有活动 API。
    sink.apply_tool_boundary(
        &ToolExecutionRecoveryBoundary::Finished(ToolInvocationResult {
            tool_call_id: String::from("goal-create"),
            tool_name: String::from("create_goal"),
            model_output: String::from("{\"ok\":true}"),
            host_result: None,
            is_error: false,
        }),
        12,
    )
    .await
    .unwrap();
    handle
        .read(|document| {
            assert!(document.active_api().is_none());
            assert!(document.active_turn().is_some_and(|turn| !turn.closed));
            assert_eq!(document.memory_revision(), document.durable_revision());
        })
        .unwrap();
}

/// Verifies that final-answer settlement hands the complete insertion batch to one fresh queued Turn.
/// 验证最终回答结算把完整插队批次交接给一个新的排队回合；无参数，无返回值。
#[tokio::test]
async fn final_answer_terminal_hands_off_insertion_batch_before_queue() {
    // A final assistant response must transfer the entire batch before the ordinary queue.
    // 最终助手回答必须把整个批次转交到普通队列之前。
    let (_directory, handle) = published_stored_open_turn();
    let mut sink = ProviderV1StreamSink::new(
        Arc::clone(&handle),
        Arc::new(ProjectResourceCatalogRegistry::default()),
    );
    prepare_sink_request(&handle, &mut sink);
    sink.append_assistant_delta("final answer", 4)
        .expect("assistant answer before insertion");
    handle
        .mutate_durable_if_changed(
            SessionMutationAdmission::ExistingTurn,
            SessionFlushReason::InputAccepted,
            |document| {
                document.apply_atomic_transition(|document| {
                    document.enqueue_input(
                        ClientInputSubmission::new(
                            ClientActionId::try_new("ordinary-after-batch")?,
                            "ordinary",
                            Vec::new(),
                        ),
                        5,
                    )?;
                    for (position, text) in ["first insertion", "second insertion"]
                        .into_iter()
                        .enumerate()
                    {
                        // Exercise the exact queue audit that must survive cross-Turn handoff.
                        // 覆盖必须跨回合交接保留的精确队列审计。
                        let action = ClientActionId::try_new(format!("insertion-{position}"))?;
                        // Preserve the accepted identity through explicit insertion promotion.
                        // 在明确插队提升中保留已接纳身份。
                        let input_id = document
                            .enqueue_input(
                                ClientInputSubmission::new(action.clone(), text, Vec::new()),
                                6 + position as i64,
                            )?
                            .into_value()
                            .input_id;
                        document.promote_queued_input_to_pending_turn(
                            vulcan_session_core::v1::config::QueuedInputIdentity {
                                client_action_id: action,
                                input_id,
                            },
                            8 + position as i64,
                        )?;
                    }
                    Ok(SessionMutationChange::Changed(()))
                })
            },
        )
        .expect("persist both input classes");
    assert_eq!(
        sink.accept_terminal(terminal_facts(0))
            .await
            .expect("final terminal"),
        ProviderV1TerminalOutcome::Completed
    );
    // The old API receipt does not claim to consume inputs delegated to the next Turn.
    // 旧 API 回执不能声称消费已委托给下一回合的输入。
    let durability = sink.take_finished_durability();
    let api_terminal = durability.api_terminal.expect("physical terminal");
    assert!(api_terminal.value.promoted_input_seqs.is_empty());
    let completion = durability
        .turn_completion
        .expect("durable old-Turn closure");
    let TurnSuccessor::QueuedUser {
        turn_id,
        input_id,
        client_action_id,
        input_seq,
    } = completion.value.successor
    else {
        panic!("the insertion batch must open one queued Turn");
    };
    assert_ne!(turn_id, api_terminal.value.turn_id);
    // Real dispatch authority must cover every member in one preparation prompt.
    // 真实派发权威必须在一个准备提示中覆盖全部成员。
    let authority = crate::webui_prompt_lifecycle::queued_dispatch_authority_from_v1_handle(
        handle.as_ref(),
        turn_id,
        input_id,
        &client_action_id,
        input_seq,
    )
    .expect("reconstruct full-batch dispatch");
    assert_eq!(authority.text, "first insertion\n\nsecond insertion");
    // A collaboration message can arrive adjacent to the batch before its first physical request.
    // 协作消息可能在批次首次物理请求前紧邻批次到达。
    let guidance_seq = handle
        .mutate_durable(
            SessionMutationAdmission::ExistingTurn,
            SessionFlushReason::InputAccepted,
            |document| {
                document
                    .append_in_turn_guidance_before_api(
                        ClientActionId::try_new("post-batch-guidance")?,
                        "collaboration guidance".to_owned(),
                        20,
                    )
                    .map(|receipt| receipt.into_value())
            },
        )
        .expect("persist adjacent guidance")
        .value;
    assert_eq!(
        crate::webui_prompt_lifecycle::queued_dispatch_authority_from_v1_handle(
            handle.as_ref(),
            turn_id,
            input_id,
            &client_action_id,
            input_seq,
        )
        .expect("guidance must not invalidate batch admission"),
        authority,
    );
    assert!(
        handle
            .read(|document| document
                .events()
                .iter()
                .any(|event| event.seq == guidance_seq))
            .expect("read persisted guidance")
    );
    handle
        .read(|document| {
            assert_eq!(
                document.active_turn().expect("new open Turn").turn_id,
                turn_id
            );
            assert_eq!(
                document
                    .initial_queued_input_batch(turn_id, input_seq)
                    .expect("initial batch")
                    .len(),
                2
            );
            assert!(
                document
                    .metadata
                    .input_state
                    .pending_in_turn_inputs
                    .is_empty()
            );
            assert_eq!(document.metadata.input_state.queued_inputs.len(), 1);
            assert_eq!(
                document
                    .metadata
                    .input_state
                    .queued_inputs
                    .first()
                    .expect("ordinary queue")
                    .text,
                "ordinary"
            );
        })
        .expect("inspect authoritative successor");
    // Read the actual stored files to verify the atomic publication survives strict cold loading.
    // 读取真实存储文件，验证原子发布能够通过严格冷加载。
    let locator = handle.storage_locator().expect("isolated storage locator");
    let loaded = vulcan_workspace_store::workspace_store::load_session_v1_full(&locator)
        .expect("strict reload of the handed-off batch");
    assert_eq!(
        loaded
            .document
            .initial_queued_input_batch(turn_id, input_seq)
            .expect("stored initial batch")
            .len(),
        2,
    );
    // The real provider projection includes each insertion once and leaves the ordinary queue out.
    // 真实供应商投影只包含各插队一次，并把普通队列留在历史之外。
    let projection = handle
        .capture_controller_projection_snapshot()
        .expect("provider source projection");
    let insertion_texts = projection
        .items
        .iter()
        .filter_map(|item| match item {
            ProjectedItem::UserInput { text, .. }
                if text == "first insertion" || text == "second insertion" =>
            {
                Some(text.as_str())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(insertion_texts, ["first insertion", "second insertion"]);
    assert_eq!(projection.items.iter().filter(|item|
        matches!(item, ProjectedItem::UserInput { text, .. } if text == "collaboration guidance")).count(), 1);
    assert!(
        !projection.items.iter().any(
            |item| matches!(item, ProjectedItem::UserInput { text, .. } if text == "ordinary")
        )
    );
}

/// Physical terminal accounting must precede the same request's Goal budget decision.
/// 物理终态累计必须先于同一次请求的 Goal 预算判断。
#[tokio::test]
async fn terminal_usage_precedes_goal_budget_decision() {
    let handle = published_open_turn();
    handle
        .mutate_durable_if_changed(
            SessionMutationAdmission::MetadataOnly,
            SessionFlushReason::MetadataChanged,
            |document| {
                document.create_goal("finish", Some(27), "finish", 2)?;
                document.metadata.mode.goal.usage_baseline = Some(Default::default());
                Ok(SessionMutationChange::Changed(()))
            },
        )
        .unwrap();
    let service_state =
        crate::tests::test_support::test_service_state_with_parent_session(handle.session_id());
    service_state
        .session_runtime_registry
        .register(Arc::clone(&handle))
        .unwrap();
    let state = Arc::new(std::sync::Mutex::new(service_state));
    let app = crate::ServiceEventEmitter::new(Vec::new());
    let mut sink = ProviderV1StreamSink::new(
        Arc::clone(&handle),
        Arc::new(ProjectResourceCatalogRegistry::default()),
    );
    prepare_sink_request(&handle, &mut sink);
    sink.append_assistant_delta("done", 6).unwrap();
    let event = vulcan_agent_runtime::agent_runtime::provider_request_telemetry::ProviderRequestTelemetryEvent {
            kind: vulcan_agent_runtime::agent_runtime::provider_request_telemetry::ProviderRequestTelemetryEventKind::Completed,
            session_id: handle.session_id(), payload: json!({}),
            completed_usage: Some(vulcan_agent_runtime::agent_runtime::provider_request_telemetry::ProviderRequestCompletedUsage {
                uncached_input_tokens: Some(20), output_tokens: Some(7), total_tokens: Some(27), ..Default::default()
            }),
            v1_lifecycle: Some(vulcan_agent_runtime::agent_runtime::provider_v1_bridge::ProviderV1LifecycleFacts::ApiTerminal(terminal_facts(0))),
        };
    sink.accept_telemetry_terminal(&event, &app, &state)
        .await
        .unwrap();
    assert_eq!(
        handle
            .lifetime_usage_snapshot()
            .unwrap()
            .main
            .budget_tokens
            .sum,
        27
    );
    assert_eq!(
        handle
            .read(|document| document.metadata.mode.goal.state)
            .unwrap(),
        vulcan_session_core::v1::config::GoalState::BudgetExhausted
    );
    assert!(
        handle
            .read(|document| document.active_turn().is_some_and(|turn| turn.closed))
            .unwrap()
    );
}

/// Verifies one Provider-ordered batch keeps each tool seq across all lifecycle updates.
/// 验证一个 Provider 有序批次在全部生命周期更新中保持每条工具 seq 不变。
#[tokio::test]
async fn tool_batch_persists_prepared_running_finished_and_unique_stats() {
    let handle = published_open_turn();
    // Use physical terminal telemetry to prove accounting precedes deferred ApiStats.
    // 使用物理请求终态遥测，验证累计早于延迟生成的 ApiStats。
    let service_state =
        crate::tests::test_support::test_service_state_with_parent_session(handle.session_id());
    service_state
        .session_runtime_registry
        .register(Arc::clone(&handle))
        .unwrap();
    let state = Arc::new(std::sync::Mutex::new(service_state));
    let app = crate::ServiceEventEmitter::new(Vec::new());
    let mut sink = ProviderV1StreamSink::new(
        Arc::clone(&handle),
        Arc::new(ProjectResourceCatalogRegistry::default()),
    );
    prepare_sink_request(&handle, &mut sink);
    let terminal = vulcan_agent_runtime::agent_runtime::provider_request_telemetry::ProviderRequestTelemetryEvent {
            kind: vulcan_agent_runtime::agent_runtime::provider_request_telemetry::ProviderRequestTelemetryEventKind::Completed,
            session_id: handle.session_id(),
            payload: json!({}),
            completed_usage: Some(vulcan_agent_runtime::agent_runtime::provider_request_telemetry::ProviderRequestCompletedUsage {
                input_tokens: Some(20), uncached_input_tokens: Some(20), output_tokens: Some(7), total_tokens: Some(27), ..Default::default()
            }),
            v1_lifecycle: Some(vulcan_agent_runtime::agent_runtime::provider_v1_bridge::ProviderV1LifecycleFacts::ApiTerminal(terminal_facts(2))),
        };
    assert_eq!(
        sink.accept_telemetry_terminal(&terminal, &app, &state)
            .await
            .expect("defer terminal"),
        ProviderV1TerminalOutcome::AwaitingTools
    );
    let usage = handle.lifetime_usage_snapshot().unwrap();
    assert_eq!(usage.main.request_count, 1);
    assert_eq!(usage.main.total_tokens.sum, 27);
    assert_eq!(usage.main.budget_tokens.sum, 27);
    assert_eq!(usage.main.tool_call_count, 0);
    let first = recovery_request("tool-1", "shell_command", json!({"command": "echo one"}));
    let second = recovery_request("tool-2", "file_edit", json!({"path": "two.txt"}));
    sink.apply_tool_boundary(
        &ToolExecutionRecoveryBoundary::BatchPrepared(vec![first.clone(), second.clone()]),
        9,
    )
    .await
    .expect("durable Prepared batch");
    let prepared = handle
        .read(|document| {
            document
                .events()
                .iter()
                .filter_map(|event| match &event.event {
                    EventBody::ToolCall(tool) => Some((event.seq, tool.clone())),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .expect("read Prepared batch");
    assert_eq!(prepared.len(), 2);
    assert!(prepared.iter().all(|(_, tool)| {
        tool.status == ToolCallStatus::Prepared
            && tool.started_at_ms.is_none()
            && tool.finished_at_ms.is_none()
    }));
    assert!(prepared[0].1.provider_source.is_none());
    assert!(prepared[1].1.provider_source.is_some());
    let (memory_revision, durable_revision, stats_count) = handle
        .read(|document| {
            (
                document.memory_revision(),
                document.durable_revision(),
                document
                    .events()
                    .iter()
                    .filter(|event| matches!(&event.event, EventBody::ApiStats(_)))
                    .count(),
            )
        })
        .expect("read durable Prepared source boundary");
    assert_eq!(memory_revision, durable_revision);
    assert_eq!(stats_count, 0);
    // The prepared tools belong to the session's open root turn, whatever id it was allocated.
    // 已准备工具属于会话的开放根回合，无论其分配到的 id 是多少。
    let root_turn_id = handle
        .read(|document| document.active_turn().map(|turn| turn.turn_id))
        .expect("read open root turn")
        .expect("open root turn");
    let duplicate_batch = sink
        .apply_tool_boundary(
            &ToolExecutionRecoveryBoundary::BatchPrepared(vec![first.clone(), second.clone()]),
            9,
        )
        .await
        .expect("an exact repeated Prepared batch must be idempotent");
    assert_eq!(
        duplicate_batch,
        ProviderV1ToolBoundaryOutcome::BatchPrepared(
            prepared
                .iter()
                .map(|(seq, tool)| PreparedToolInvocationIdentity {
                    turn_id: root_turn_id,
                    api_call_id: tool.api_call_id,
                    tool_call_id: tool.tool_call_id,
                    request_stream_id: tool.request_stream_id.clone(),
                    tool_seq: *seq,
                })
                .collect(),
        )
    );
    assert_eq!(
        handle
            .lifetime_usage_snapshot()
            .unwrap()
            .main
            .tool_call_count,
        2
    );

    for request in [&first, &second] {
        let started = ToolExecutionRecoveryBoundary::Started {
            request: request.request.clone(),
            binding_fingerprint: request.binding_fingerprint.clone(),
        };
        sink.apply_tool_boundary(&started, 10)
            .await
            .expect("durable Running transition");
        sink.apply_tool_boundary(&started, 10)
            .await
            .expect("exact repeated Started boundary must be idempotent");
    }
    let running = handle
        .read(|document| {
            document
                .events()
                .iter()
                .filter_map(|event| match &event.event {
                    EventBody::ToolCall(tool) => Some((event.seq, tool.clone())),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .expect("read Running tools");
    assert_eq!(
        prepared.iter().map(|(seq, _)| *seq).collect::<Vec<_>>(),
        running.iter().map(|(seq, _)| *seq).collect::<Vec<_>>()
    );
    assert!(
        running
            .iter()
            .all(|(_, tool)| tool.status == ToolCallStatus::Running)
    );

    sink.apply_tool_boundary(
        &ToolExecutionRecoveryBoundary::Finished(ToolInvocationResult {
            tool_call_id: String::from("tool-1"),
            tool_name: String::from("shell_command"),
            model_output: String::from("one"),
            host_result: Some(json!({"kind": "command_result", "exit_code": 0})),
            is_error: false,
        }),
        11,
    )
    .await
    .expect("finish first tool");
    assert!(
        handle
            .read(|document| document
                .events()
                .iter()
                .all(|event| !matches!(&event.event, EventBody::ApiStats(_))))
            .expect("read deferred stats")
    );

    sink.apply_tool_boundary(
        &ToolExecutionRecoveryBoundary::Finished(ToolInvocationResult {
            tool_call_id: String::from("tool-2"),
            tool_name: String::from("file_edit"),
            model_output: String::from("two"),
            host_result: Some(json!({"kind": "plain_result", "ok": true})),
            is_error: false,
        }),
        12,
    )
    .await
    .expect("finish final tool and API");
    let (finished, stats_count) = handle
        .read(|document| {
            let tools = document
                .events()
                .iter()
                .filter_map(|event| match &event.event {
                    EventBody::ToolCall(tool) => Some((event.seq, tool.clone())),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let stats_count = document
                .events()
                .iter()
                .filter(|event| matches!(&event.event, EventBody::ApiStats(_)))
                .count();
            (tools, stats_count)
        })
        .expect("read terminal tools");
    assert_eq!(stats_count, 1);
    assert_eq!(
        prepared.iter().map(|(seq, _)| *seq).collect::<Vec<_>>(),
        finished.iter().map(|(seq, _)| *seq).collect::<Vec<_>>()
    );
    assert!(
        finished
            .iter()
            .all(|(_, tool)| tool.status == ToolCallStatus::Completed)
    );
    assert!(finished[0].1.provider_source.is_none());
    assert!(finished[1].1.provider_source.is_some());
    let duplicate_finish_error = sink
        .apply_tool_boundary(
            &ToolExecutionRecoveryBoundary::Finished(ToolInvocationResult {
                tool_call_id: String::from("tool-2"),
                tool_name: String::from("file_edit"),
                model_output: String::from("two"),
                host_result: Some(json!({"kind": "plain_result", "ok": true})),
                is_error: false,
            }),
            13,
        )
        .await
        .expect_err("a duplicate final result must fail");
    assert!(matches!(
        duplicate_finish_error,
        ProviderV1SinkError::RequestNotActive
    ));
    assert_eq!(
        handle
            .read(|document| document
                .events()
                .iter()
                .filter(|event| matches!(&event.event, EventBody::ApiStats(_)))
                .count())
            .expect("read unique terminal statistic"),
        1
    );
    let durability = sink.take_finished_durability();
    assert!(durability.api_terminal.is_some());
    assert!(
        durability.turn_completion.is_none(),
        "a tool-bearing API terminal must not close its logical Turn"
    );
}

/// Generated provider attachments are project-registered before their ToolCall terminal is durable.
/// Provider 生成附件会先注册到项目目录，再可靠提交其 ToolCall 终态。
#[tokio::test]
async fn generated_attachment_registration_precedes_tool_terminal() {
    let temporary = tempfile::tempdir().expect("temporary generated-resource root");
    let project_directory = temporary.path().join("project");
    let managed_path = temporary.path().join("generated.png");
    let content = b"generated-image";
    std::fs::write(&managed_path, content).expect("write managed generated attachment");
    let digest = hex::encode(Sha256::digest(content));
    let handle = published_open_turn();
    let mut sink = ProviderV1StreamSink::new(
        Arc::clone(&handle),
        Arc::new(ProjectResourceCatalogRegistry::default()),
    );
    prepare_sink_request(&handle, &mut sink);
    sink.active
        .as_mut()
        .expect("prepared active request")
        .project_directory = project_directory.clone();
    assert_eq!(
        sink.accept_terminal(terminal_facts(1))
            .await
            .expect("defer terminal for generated tool"),
        ProviderV1TerminalOutcome::AwaitingTools
    );
    let request = recovery_request("generated-tool-1", "image_generate", json!({}));
    sink.apply_tool_boundary(
        &ToolExecutionRecoveryBoundary::BatchPrepared(vec![request.clone()]),
        9,
    )
    .await
    .expect("prepare generated tool");
    sink.apply_tool_boundary(
        &ToolExecutionRecoveryBoundary::Started {
            request: request.request.clone(),
            binding_fingerprint: request.binding_fingerprint.clone(),
        },
        10,
    )
    .await
    .expect("start generated tool");

    sink.apply_tool_boundary(
        &ToolExecutionRecoveryBoundary::Finished(ToolInvocationResult {
            tool_call_id: String::from("generated-tool-1"),
            tool_name: String::from("image_generate"),
            model_output: json!({"resources": [{"file_name": "generated.png"}]}).to_string(),
            host_result: Some(json!({
                "kind": "generated_attachments",
                "payload": {
                    "outputs": [{
                        "kind": "image",
                        "file_name": "generated.png",
                        "local_path": managed_path.to_string_lossy(),
                        "asset_id": "generated-asset-1",
                        "byte_len": content.len(),
                        "source_sha256": digest,
                        "mime_type": "image/png"
                    }],
                    "upstream_response_id": "provider-response-1"
                }
            })),
            is_error: false,
        }),
        11,
    )
    .await
    .expect("finish generated tool after project registration");

    let generated_resources = handle
        .read(|document| {
            document
                .events()
                .iter()
                .find_map(|event| match &event.event {
                    EventBody::ToolCall(tool) if tool.tool_name == "image_generate" => {
                        Some(tool.generated_resources.clone())
                    }
                    _ => None,
                })
        })
        .expect("read generated ToolCall")
        .expect("generated ToolCall event");
    let catalog = load_project_resource_catalog(&project_directory)
        .expect("load generated project resource catalog");

    assert_eq!(generated_resources.len(), 1);
    assert_eq!(generated_resources[0].kind, GeneratedResourceKind::Image);
    assert_eq!(generated_resources[0].resource_id.value(), 1);
    assert!(generated_resources[0].preview_resource_id.is_none());
    assert!(matches!(
        resolve_project_resource(
            &project_directory,
            &catalog,
            generated_resources[0].resource_id
        ),
        Ok(ProjectResourceResolution::Available { bytes, .. }) if bytes == content
    ));
}

/// Verifies a durably cancelled Ask is only confirmed by the later generic ToolFinished boundary.
/// 验证已可靠取消的 Ask 只会由后续通用 ToolFinished 边界进行幂等确认。
#[tokio::test]
async fn cancelled_ask_finish_confirms_the_existing_terminal_tool_fact() {
    let handle = published_open_turn();
    let mut sink = ProviderV1StreamSink::new(
        Arc::clone(&handle),
        Arc::new(ProjectResourceCatalogRegistry::default()),
    );
    prepare_sink_request(&handle, &mut sink);
    assert_eq!(
        sink.accept_terminal(terminal_facts(1))
            .await
            .expect("defer terminal"),
        ProviderV1TerminalOutcome::AwaitingTools
    );
    let request = recovery_request("ask-tool", "ask", json!({"questions": []}));
    sink.apply_tool_boundary(
        &ToolExecutionRecoveryBoundary::BatchPrepared(vec![request.clone()]),
        9,
    )
    .await
    .expect("durably prepare Ask");
    sink.apply_tool_boundary(
        &ToolExecutionRecoveryBoundary::Started {
            request: request.request.clone(),
            binding_fingerprint: request.binding_fingerprint.clone(),
        },
        10,
    )
    .await
    .expect("durably start Ask");
    let (api_call_id, tool_call_id) = handle
        .read(|document| {
            document
                .events()
                .iter()
                .find_map(|event| match &event.event {
                    EventBody::ToolCall(tool) => Some((tool.api_call_id, tool.tool_call_id)),
                    _ => None,
                })
        })
        .expect("read Ask identity")
        .expect("Ask tool event");
    handle
        .mark_tool_waiting_user(api_call_id, tool_call_id, 11)
        .expect("durably mark Ask waiting");
    let result = build_tool_failure_invocation_result(
        &request.request,
        "The user rejected the structured question request.",
    );
    let output = persistent_tool_output_from_result(&result).expect("canonical Ask output");
    handle
        .cancel_waiting_tool(api_call_id, tool_call_id, Some(output), 12)
        .expect("durably cancel Ask");

    sink.apply_tool_boundary(&ToolExecutionRecoveryBoundary::Finished(result), 13)
        .await
        .expect("generic ToolFinished must confirm the durable cancellation");
    let (status, stats_count) = handle
        .read(|document| {
            let status = document
                .tool_status(api_call_id, tool_call_id)
                .expect("read durable Ask status");
            let stats_count = document
                .events()
                .iter()
                .filter(|event| matches!(&event.event, EventBody::ApiStats(_)))
                .count();
            (status, stats_count)
        })
        .expect("read finished Ask facts");
    assert_eq!(status, ToolCallStatus::Cancelled);
    assert_eq!(stats_count, 1);
}

/// A stale terminal identity remains a navigation-only durable reference after reload.
/// 一个已失效的终端身份在重载后仍保持为仅导航用途的可靠引用。
#[tokio::test]
async fn terminal_external_reference_round_trips_without_resolution() {
    let handle = published_open_turn();
    let mut sink = ProviderV1StreamSink::new(
        Arc::clone(&handle),
        Arc::new(ProjectResourceCatalogRegistry::default()),
    );
    prepare_sink_request(&handle, &mut sink);
    sink.accept_terminal(terminal_facts(1))
        .await
        .expect("defer terminal");
    let request = recovery_request(
        "tool-terminal-1",
        "terminal_session",
        json!({"action": "read", "terminal_session_id": "missing-terminal"}),
    );
    sink.apply_tool_boundary(
        &ToolExecutionRecoveryBoundary::BatchPrepared(vec![request.clone()]),
        9,
    )
    .await
    .expect("durable terminal Prepared event");
    sink.apply_tool_boundary(
        &ToolExecutionRecoveryBoundary::Started {
            request: request.request.clone(),
            binding_fingerprint: request.binding_fingerprint.clone(),
        },
        10,
    )
    .await
    .expect("durable terminal Running event");
    sink.apply_tool_boundary(
        &ToolExecutionRecoveryBoundary::Finished(ToolInvocationResult {
            tool_call_id: String::from("tool-terminal-1"),
            tool_name: String::from("terminal_session"),
            model_output: String::from("terminal output"),
            host_result: Some(json!({
                "kind": "terminal_session",
                "payload": {
                    "action": "read",
                    "terminal_session_id": "missing-terminal"
                }
            })),
            is_error: false,
        }),
        11,
    )
    .await
    .expect("finish terminal tool and API");

    let tool_event = handle
        .read(|document| {
            document.events().iter().find_map(|event| {
                matches!(&event.event, EventBody::ToolCall(_)).then(|| event.clone())
            })
        })
        .expect("read terminal event")
        .expect("terminal event exists");
    let serialized = serde_json::to_string(&tool_event).expect("serialize terminal event");
    let restored: ConversationEvent =
        serde_json::from_str(&serialized).expect("reload terminal event");
    let EventBody::ToolCall(restored_tool) = restored.event else {
        panic!("restored event must remain a tool call");
    };
    assert_eq!(restored_tool.external_refs.len(), 1);
    assert_eq!(
        restored_tool.external_refs[0].kind,
        ExternalResourceKind::Terminal
    );
    assert_eq!(restored_tool.external_refs[0].id, "missing-terminal");
}

/// Verifies Started cannot advance a tool that lacks a durable Prepared predecessor.
/// 验证 Started 不能推进缺少可靠 Prepared 前驱的工具。
#[tokio::test]
async fn tool_started_without_prepared_predecessor_is_rejected() {
    let handle = published_open_turn();
    let mut sink = ProviderV1StreamSink::new(
        Arc::clone(&handle),
        Arc::new(ProjectResourceCatalogRegistry::default()),
    );
    prepare_sink_request(&handle, &mut sink);
    sink.accept_terminal(terminal_facts(1))
        .await
        .expect("defer terminal");
    let request = recovery_request("tool-1", "shell_command", json!({"command": "echo"}));
    let error = sink
        .apply_tool_boundary(
            &ToolExecutionRecoveryBoundary::Started {
                request: request.request,
                binding_fingerprint: request.binding_fingerprint,
            },
            9,
        )
        .await
        .expect_err("Started without Prepared must fail");
    assert!(matches!(error, ProviderV1SinkError::ToolNotPrepared(_)));
}

/// Verifies a call whose Provider arguments failed to parse still prepares, runs, and fails durably.
/// 验证 Provider 参数解析失败的调用仍能可靠地完成准备、运行与失败终态。
#[tokio::test]
async fn unparseable_provider_arguments_settle_as_failed_tool() {
    let handle = published_open_turn();
    let mut sink = ProviderV1StreamSink::new(
        Arc::clone(&handle),
        Arc::new(ProjectResourceCatalogRegistry::default()),
    );
    prepare_sink_request(&handle, &mut sink);
    sink.accept_terminal(terminal_facts(1))
        .await
        .expect("defer terminal");
    let mut request = recovery_request("tool-1", "file", serde_json::Value::Null);
    request.request.argument_parse_error = Some(String::from("invalid JSON arguments"));
    sink.apply_tool_boundary(
        &ToolExecutionRecoveryBoundary::BatchPrepared(vec![request.clone()]),
        9,
    )
    .await
    .expect("a rejected call must still enter the durable batch");
    sink.apply_tool_boundary(
        &ToolExecutionRecoveryBoundary::Started {
            request: request.request.clone(),
            binding_fingerprint: request.binding_fingerprint.clone(),
        },
        10,
    )
    .await
    .expect("durable Running transition");
    sink.apply_tool_boundary(
        &ToolExecutionRecoveryBoundary::Finished(ToolInvocationResult {
            tool_call_id: String::from("tool-1"),
            tool_name: String::from("file"),
            model_output: String::from("not executed"),
            host_result: Some(json!({"kind": "tool_argument_parse_error", "status": "failed"})),
            is_error: true,
        }),
        11,
    )
    .await
    .expect("the rejection result closes the batch");
    let tools = handle
        .read(|document| {
            document
                .events()
                .iter()
                .filter_map(|event| match &event.event {
                    EventBody::ToolCall(tool) => Some(tool.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .expect("read settled tool");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].status, ToolCallStatus::Failed);
    assert_eq!(serde_json::to_value(&tools[0].input).unwrap(), json!({}));
}

/// Verifies malformed declared ChangeSet data is an explicit ToolFinished boundary error.
/// 验证畸形已声明 ChangeSet 数据会成为显式 ToolFinished 边界错误。
#[test]
fn malformed_tool_finished_changeset_fails_without_fallback() {
    let result = ToolInvocationResult {
        tool_call_id: String::from("tool-1"),
        tool_name: String::from("file_edit"),
        model_output: String::from("done"),
        host_result: Some(json!({
            "kind": "change_set",
            "payload": {
                "mode": "applied",
                "files": [{"change": "modify", "path": "bad.txt", "hunks": "invalid"}]
            }
        })),
        is_error: false,
    };
    let error = persistent_change_set_from_tool_result(&result)
        .expect_err("declared malformed change-set must fail");
    assert!(matches!(error, ProviderV1SinkError::ChangeSet(_)));
}

/// Verifies a lifecycle ChangeSet rejection preserves the running tool for terminal fallback.
/// 验证生命周期 ChangeSet 被拒绝时会保留运行中工具，交由终态兜底结算。
#[tokio::test]
async fn lifecycle_changeset_rejection_preserves_running_tool_and_exact_error() {
    // handle owns the durable V1 event graph inspected after the boundary rejects.
    // handle 持有边界拒绝后检查的可靠 V1 事件图。
    let handle = published_open_turn();
    // sink is the unique in-stream owner of the active API and tool lifecycle.
    // sink 是活动 API 与工具生命周期的唯一流内 owner。
    let mut sink = ProviderV1StreamSink::new(
        Arc::clone(&handle),
        Arc::new(ProjectResourceCatalogRegistry::default()),
    );
    prepare_sink_request(&handle, &mut sink);
    sink.accept_terminal(terminal_facts(2))
        .await
        .expect("terminal should wait for both tools");
    // request binds the exact Provider tool identity used at every durable boundary.
    // request 绑定每个可靠边界使用的精确 Provider 工具身份。
    let request = recovery_request("tool-1", "file_edit", json!({"path": "bad.txt"}));
    // sibling_request models a concurrently started tool whose real success must be retained.
    // sibling_request 模拟并发启动且必须保留真实成功结果的兄弟工具。
    let sibling_request = recovery_request(
        "tool-2",
        "shell_command",
        json!({"command": "echo settled"}),
    );
    sink.apply_tool_boundary(
        &ToolExecutionRecoveryBoundary::BatchPrepared(vec![
            request.clone(),
            sibling_request.clone(),
        ]),
        9,
    )
    .await
    .expect("tool batch should prepare");
    sink.apply_tool_boundary(
        &ToolExecutionRecoveryBoundary::Started {
            request: request.request.clone(),
            binding_fingerprint: request.binding_fingerprint.clone(),
        },
        10,
    )
    .await
    .expect("invalid tool should start");
    sink.apply_tool_boundary(
        &ToolExecutionRecoveryBoundary::Started {
            request: sibling_request.request.clone(),
            binding_fingerprint: sibling_request.binding_fingerprint.clone(),
        },
        10,
    )
    .await
    .expect("sibling tool should start");
    // invalid_result reproduces the original create-plus-hunks contract violation exactly.
    // invalid_result 精确复现原始 create 与 hunks 并存的契约违规。
    let invalid_result = ToolInvocationResult {
        tool_call_id: String::from("tool-1"),
        tool_name: String::from("file_edit"),
        model_output: String::from("written"),
        host_result: Some(json!({
            "kind": "change_set",
            "payload": {
                "mode": "applied",
                "files": [{
                    "resource_id": "resource-1",
                    "change": "create",
                    "path": "bad.txt",
                    "content_mode": "full",
                    "content": "created",
                    "total_line_count": 1,
                    "hunks": [{
                        "before": "",
                        "delete": [],
                        "insert": [{"line": 1, "content": "created"}],
                        "after": ""
                    }]
                }]
            }
        })),
        is_error: false,
    };
    // error proves the canonical validator, rather than a downstream generic failure, rejected it.
    // error 证明拒绝来自规范校验器，而不是下游通用失败。
    let error = sink
        .apply_tool_boundary(&ToolExecutionRecoveryBoundary::Finished(invalid_result), 11)
        .await
        .expect_err("create plus hunks must fail the ToolFinished boundary");
    assert!(matches!(&error, ProviderV1SinkError::ChangeSet(_)));
    assert_eq!(
        error.to_string(),
        "The V1 tool ChangeSet is invalid: Invalid change-set field at `payload.files[0].hunks`: lifecycle changes must not contain modify hunks"
    );
    // sibling_result is a valid terminal fact emitted after the first boundary already failed.
    // sibling_result 是首个边界已经失败后发出的合法兄弟终态事实。
    let sibling_result = ToolInvocationResult {
        tool_call_id: String::from("tool-2"),
        tool_name: String::from("shell_command"),
        model_output: String::from("settled"),
        host_result: None,
        is_error: false,
    };
    sink.apply_tool_boundary(&ToolExecutionRecoveryBoundary::Finished(sibling_result), 12)
        .await
        .expect("valid sibling terminal must remain persistable after the first failure");
    handle
        .read(|document| {
            assert!(document.active_api().is_some());
            assert!(document.events().iter().any(|event| {
                matches!(
                    &event.event,
                    EventBody::ToolCall(tool)
                        if tool.provider_tool_call_id.as_deref() == Some("tool-1")
                            && tool.status == ToolCallStatus::Running
                            && tool.finished_at_ms.is_none()
                )
            }));
            assert!(document.events().iter().any(|event| {
                matches!(
                    &event.event,
                    EventBody::ToolCall(tool)
                        if tool.provider_tool_call_id.as_deref() == Some("tool-2")
                            && tool.status == ToolCallStatus::Completed
                            && tool.finished_at_ms == Some(12)
                )
            }));
            assert!(
                document
                    .events()
                    .iter()
                    .all(|event| !matches!(&event.event, EventBody::ApiStats(_)))
            );
        })
        .expect("rejected boundary must leave the durable running state readable");
}

/// Verifies an unrelated host result is not interpreted as a ChangeSet.
/// 验证无关宿主结果不会被解释为 ChangeSet。
#[test]
fn non_changeset_tool_finished_returns_none() {
    let result = ToolInvocationResult {
        tool_call_id: String::from("tool-1"),
        tool_name: String::from("shell_command"),
        model_output: String::from("done"),
        host_result: Some(json!({"kind": "command_result", "exit_code": 0})),
        is_error: false,
    };
    assert_eq!(
        persistent_change_set_from_tool_result(&result).expect("non change-set should pass"),
        None
    );
}

/// Builds the frozen route identity used by the replay decision tests.
/// 构建重放判定测试使用的冻结路由身份。
fn deepseek_replay_route() -> V1ReplayRouteIdentity {
    V1ReplayRouteIdentity {
        provider_id: String::from("provider-1"),
        connection_id: String::from("connection-1"),
        route_fingerprint: String::from("https://provider.invalid/v1/responses"),
        protocol_profile_id: String::from("profile-1"),
        model_id: String::from("model-1"),
    }
}

/// Builds one non-empty durable envelope retaining its original ordered reasoning items.
/// 构建一份非空且保留其原始有序推理项的可持久化封套。
fn durable_reasoning_envelope() -> ProviderReplayEnvelope {
    ProviderReplayEnvelope::new(
        "provider-1",
        "connection-1",
        "https://provider.invalid/v1/responses",
        "profile-1",
        "model-1",
        durable_canonical_content(),
        vec![
            json!({
                "type": "reasoning",
                "content": [{"type": "reasoning_text", "text": "durable-reasoning"}]
            }),
            json!({
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "durable-answer"}]
            }),
        ],
    )
}

/// Builds the canonical assistant content that the durable envelope above fully covers.
/// 构建上面可持久化封套完全覆盖的规范助手内容。
fn durable_canonical_content() -> Vec<ContentItem> {
    vec![
        ContentItem::Reasoning {
            text: String::from("durable-reasoning"),
            signature: None,
        },
        ContentItem::Text {
            text: String::from("durable-answer"),
        },
    ]
}

/// Verifies a matching frozen route replaces canonical content with the durable provider items.
/// 验证匹配的冻结路由用可持久化供应商项替代规范内容。
#[test]
fn projected_assistant_replay_prefers_durable_items_on_matching_route() {
    let envelope = durable_reasoning_envelope();
    let route = deepseek_replay_route();
    let (content, items) = resolve_projected_assistant_replay(
        durable_canonical_content(),
        Some(&envelope),
        None,
        Some(&route),
        "v1-provider-message-1",
    )
    .expect("a fully covered matching route must replay");

    assert!(
        content.is_empty(),
        "provider items and canonical content are mutually exclusive"
    );
    assert_eq!(items, envelope.provider_context_items);
}

/// Verifies a matching canonical route retains its own recorded reasoning.
/// 验证匹配的规范路由保留自身记录的思考；无参数，无返回值。
#[test]
fn projected_assistant_replay_keeps_matching_canonical_reasoning() {
    // Empty native items select the proven canonical replay path.
    // 空的原生项选择已确认的规范回放路径。
    let mut envelope = durable_reasoning_envelope();
    envelope.provider_context_items.clear();
    // The matching route remains the owner of this reasoning.
    // 匹配路由仍是该思考的所有者。
    let route = deepseek_replay_route();
    // Compare complete content so no extra text can replace the reasoning item.
    // 比较完整内容，防止额外正文替代思考项。
    let (content, items) = resolve_projected_assistant_replay(
        durable_canonical_content(),
        Some(&envelope),
        None,
        Some(&route),
        "matching-canonical",
    )
    .expect("matching canonical history must remain replayable");
    assert_eq!(content, durable_canonical_content());
    assert!(
        content
            .iter()
            .any(|item| matches!(item, ContentItem::Reasoning { .. }))
    );
    assert!(items.is_empty());
}

/// Verifies route changes retain visible history and tool calls while omitting historical reasoning.
/// 验证路由变化保留可见历史及工具调用，同时省略历史思考。
#[test]
fn projected_assistant_replay_preserves_plaintext_on_foreign_route() {
    // Cover targets with and without a declared reasoning contract.
    // 覆盖声明与未声明思考回传契约的目标。
    {
        // Every frozen identity dimension independently marks the history as foreign.
        // 冻结身份的每个维度都可独立标记该历史来自外路由。
        for field in ["provider", "connection", "endpoint", "protocol", "model"] {
            // Preserve the original envelope for the immutability assertion.
            // 保留原始封套，用于验证其不可变性。
            let envelope = durable_reasoning_envelope();
            // Rebuild the target independently for each identity change.
            // 每次身份变化都独立构建目标。
            let mut route = deepseek_replay_route();
            match field {
                "provider" => route.provider_id.push_str("-changed"),
                "connection" => route.connection_id.push_str("-changed"),
                "endpoint" => route.route_fingerprint.push_str("-changed"),
                "protocol" => route.protocol_profile_id.push_str("-changed"),
                "model" => route.model_id.push_str("-changed"),
                _ => unreachable!(),
            }
            // Visible text and parallel calls retain their order, identities and arguments.
            // 可见正文与并行调用保留顺序、标识及参数。
            let expected = vec![
                ContentItem::Text {
                    text: String::from("durable-answer"),
                },
                ContentItem::ToolCall {
                    call: AiToolCall {
                        id: String::from("call-1"),
                        name: String::from("read_file"),
                        arguments: json!({"path":"a"}),
                        signature: None,
                    },
                },
                ContentItem::ToolCall {
                    call: AiToolCall {
                        id: String::from("call-2"),
                        name: String::from("read_file"),
                        arguments: json!({"path":"b"}),
                        signature: None,
                    },
                },
            ];
            // Both signed plaintext and opaque-only reasoning must stay on their original route.
            // 带签名明文思考与纯不透明思考都必须留在原路由。
            let mut source = vec![
                ContentItem::Reasoning {
                    text: String::from("durable-reasoning"),
                    signature: Some(String::from("openai#old-route-secret")),
                },
                ContentItem::Reasoning {
                    text: String::new(),
                    signature: Some(String::from("openai#opaque")),
                },
            ];
            source.extend(expected.clone());
            for item in &mut source {
                if let ContentItem::ToolCall { call } = item {
                    call.signature = Some(String::from("gemini#old-route-secret"));
                }
            }
            // Omit reasoning instead of injecting it as another visible text block.
            // 省略思考，不再将其作为额外可见正文注入。
            let (content, items) = resolve_projected_assistant_replay(
                source,
                Some(&envelope),
                None,
                Some(&route),
                "v1-provider-message-1",
            )
            .expect("a model switch must retain visible history without foreign reasoning");
            assert_eq!(
                content, expected,
                "visible history changed across the {field} boundary"
            );
            assert!(
                items.is_empty(),
                "foreign provider items crossed the {field} boundary"
            );
            assert_eq!(
                envelope,
                durable_reasoning_envelope(),
                "stored history must remain immutable"
            );
        }
    }
}

/// A cross-model target cannot consume encrypted-only reasoning but keeps the rest of the turn.
/// 跨模型目标无法消费纯加密推理，但保留回合其余内容。
#[test]
fn projected_assistant_replay_excludes_foreign_opaque_only_reasoning() {
    let envelope = durable_reasoning_envelope();
    let route = V1ReplayRouteIdentity {
        model_id: "another-model".into(),
        ..deepseek_replay_route()
    };
    let text = ContentItem::Text {
        text: "portable answer".into(),
    };
    let (content, items) = resolve_projected_assistant_replay(
        vec![
            ContentItem::Reasoning {
                text: String::new(),
                signature: Some("openai#opaque".into()),
            },
            text.clone(),
        ],
        Some(&envelope),
        None,
        Some(&route),
        "foreign-encrypted",
    )
    .expect("opaque state must not prevent a model switch with portable history");
    assert_eq!(content, vec![text]);
    assert!(items.is_empty());
}

/// Verifies a state-only fixture without a frozen execution never invents a matching route.
/// 验证缺少冻结执行的状态夹具绝不会凭空认定匹配路由。
#[test]
fn projected_assistant_replay_never_invents_a_route_for_state_only_fixtures() {
    let envelope = durable_reasoning_envelope();
    let (content, items) = resolve_projected_assistant_replay(
        durable_canonical_content(),
        Some(&envelope),
        None,
        None,
        "v1-provider-message-1",
    )
    .expect("a state-only fixture must stay conservative");

    assert_eq!(content, durable_canonical_content());
    assert!(items.is_empty());
}

/// Missing replay evidence is reported separately from an envelope owned by another route.
/// 缺失重放证据与“封套属于其他路由”分别报告。
#[test]
fn projected_assistant_replay_reports_missing_envelope_distinctly() {
    let route = deepseek_replay_route();
    let error = resolve_projected_assistant_replay(
        durable_canonical_content(),
        None,
        None,
        Some(&route),
        "v1-provider-message-27",
    )
    .expect_err("reasoning without durable replay evidence must fail before dispatch");

    let message = format!("{error}");
    assert!(
        message.contains("v1-provider-message-27 has reasoning but no durable replay envelope"),
        "the diagnostic must identify missing evidence precisely: {message}"
    );
    assert!(
        !message.contains("belongs to another route"),
        "missing evidence must not be misreported as a route mismatch: {message}"
    );
}

/// Verifies an envelope that no longer covers canonical output fails before dispatch.
/// 验证不再覆盖规范输出的封套会在分派前失败。
#[test]
fn projected_assistant_replay_rejects_unusable_envelope() {
    // damaged drops the assistant message while canonical content still declares its text.
    // damaged 丢弃助手消息，而规范内容仍声明其正文。
    let mut envelope = durable_reasoning_envelope();
    envelope
        .provider_context_items
        .retain(|item| item["type"] != "message");
    let route = deepseek_replay_route();

    let error = resolve_projected_assistant_replay(
        durable_canonical_content(),
        Some(&envelope),
        None,
        Some(&route),
        "v1-provider-message-1",
    )
    .expect_err("an envelope that no longer covers canonical output must fail before dispatch");

    assert!(
        format!("{error}").contains("v1-provider-message-1"),
        "the failure must name the projected item it refused: {error}"
    );
}

/// Verifies the real V1 rebuild keeps a durable envelope conservative without a frozen route.
/// 验证真实 V1 重建在缺少冻结路由时对可持久化封套保持保守。
#[test]
fn provider_transport_rebuild_keeps_durable_envelope_conservative()
-> Result<(), SessionRuntimeError> {
    let facts = request_facts();
    let envelope = durable_reasoning_envelope();
    let snapshot = SessionProviderProjectionSnapshot {
        unenveloped_assistant_requests: Default::default(),
        preparing_user_prompt_media_seqs: Vec::new(),
        compaction_frontier_seq: None,
        image_replay_from_seq: None,
        segment_generation: 1,
        current_compaction_id: None,
        document_revision: 1,
        open_turn_id: None,
        child_business_execution_id: None,
        retained_from_seq: Seq::try_new(1).expect("valid retained seq"),
        through_seq: Seq::try_new(1).expect("valid through seq"),
        items: vec![ProjectedItem::AssistantText {
            text: String::from("durable-answer"),
            reasoning: Some(String::from("durable-reasoning")),
            tool_calls: Vec::new(),
            provider_source: Some(Box::new(vulcan_session_core::v1::event::ProviderSource {
                fingerprint_version:
                    vulcan_session_core::v1::event::ProviderFingerprintVersion::ImmutableProjectionV1,
                provider: String::from("provider-1"),
                profile: Some(String::from("profile-1")),
                model: String::from("model-1"),
                connection_id: Some(String::from("connection-1")),
                message_id: Some(String::from("message-1")),
                response_id: None,
                replay: Some(envelope),
                covered_until_seq: Seq::try_new(1).expect("valid covered seq"),
                fingerprint_sha256: String::from("0").repeat(64),
            })),
        }],
        sourced_items: Vec::new(),
        project_directory: std::path::PathBuf::from("D:/project"),
        provider_usage_budget_anchor: None,
        initialized_locked_baseline: None,
    };
    let candidate = ProviderRequestPreparationCandidate {
        media_context: None,
        exact_request: AiExecutionRequest {
            operation: AiOperation::ConversationRespond,
            model_id: facts.request.model.clone(),
            cache_scope_id: None,
            continuation: None,
            messages: vec![text_ai_message(
                MessageRole::User,
                String::from("legacy-message"),
            )],
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
        controller_config: vulcan_core::types::ControllerConfig {
            provider_id: facts.request.provider.clone(),
            model_id: facts.request.model.clone(),
            temperature: facts.request.temperature,
            ..Default::default()
        },
        controller_snapshot: ConversationSnapshot::default(),
        request_snapshot: CoreProviderRequestSnapshot {
            provider_id: facts.request.provider.clone(),
            model_id: facts.request.model.clone(),
            system_prompt: String::from("legacy-system"),
            prompt_layers: ProviderPromptLayers::default(),
            prompt: String::from("legacy-message"),
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
        locked_baseline: LockedBaselineProjection::empty(),
        dynamic_prompt_layers: ProviderPromptLayers::default(),
        fixed_instruction_layers: ProviderPromptLayers::default(),
    };

    // Exercise summary controls on the same production reconstruction, with an actual durable business inventory.
    // 在相同生产重建路径验证摘要控制，并使用真实持久业务清单。
    let (_directory, handle) = published_stored_open_turn();
    let mut business_candidate = candidate.clone();
    business_candidate.fixed_instruction_layers = ProviderPromptLayers {
        system: "business-fixed-base".into(),
        developer: "business-fixed-guide".into(),
        ..Default::default()
    };
    business_candidate.dynamic_prompt_layers.runtime_context =
        String::from("original-business-memory");
    context_injection::synchronize_request_dynamic_context(&handle, &facts, &business_candidate)?;
    let before = handle.read(|document| {
        (
            document.memory_revision(),
            serde_json::to_value(&document.metadata).unwrap(),
            serde_json::to_value(document.events()).unwrap(),
        )
    })?;
    let mut summary_facts = facts.clone();
    summary_facts.purpose = ApiPurposeKind::Compaction;
    let mut summary_candidate = candidate.clone();
    summary_candidate.fixed_instruction_layers =
        business_candidate.fixed_instruction_layers.clone();
    summary_candidate.dynamic_prompt_layers = ProviderPromptLayers {
        system: String::from("local-summary-system"),
        developer: String::from("local-summary-developer"),
        tool_guidance: String::from("local-summary-tools"),
        runtime_context: String::from("local-summary-context"),
    };
    summary_candidate.request_snapshot.prompt =
        String::from("Summarize <history> & preserve pending questions.");
    context_injection::synchronize_request_dynamic_context(
        &handle,
        &summary_facts,
        &summary_candidate,
    )?;
    assert_eq!(
        before,
        handle.read(|document| (
            document.memory_revision(),
            serde_json::to_value(&document.metadata).unwrap(),
            serde_json::to_value(document.events()).unwrap(),
        ))?
    );
    let summary = rebuild_provider_transport_candidate(
        snapshot.clone(),
        &summary_facts,
        summary_candidate.clone(),
        media::PreparedV1UserMedia {
            prompts: BTreeMap::new(),
            bindings: Vec::new(),
        },
    )?;
    let serialized = serde_json::to_string(&summary.transport.exact_request.messages).unwrap();
    assert!(summary.transport.exact_request.messages.starts_with(&[
        text_ai_message(MessageRole::System, "business-fixed-base".into()),
        text_ai_message(MessageRole::System, HOST_MESSAGE_GUIDANCE.into()),
        text_ai_message(MessageRole::Developer, "business-fixed-guide".into()),
    ]));
    for body in [
        "local-summary-system",
        "local-summary-developer",
        "local-summary-tools",
        "local-summary-context",
    ] {
        assert!(!serialized.contains(body));
        assert!(
            !summary
                .transport
                .request_snapshot
                .system_prompt
                .contains(body)
        );
    }
    let last = summary.transport.exact_request.messages.last().unwrap();
    assert_eq!(last.role, MessageRole::User);
    assert!(matches!(&last.content[0], ContentItem::Text { text }
            if text.contains("scope=\"compaction_request\"")
            && text.contains("source=\"context_compaction:instruction\"")
            && text.contains("Summarize &lt;history&gt; &amp; preserve pending questions.")));
    assert_eq!(
        summary.transport.request_snapshot.history.len(),
        snapshot.items.len() + 1
    );
    assert_eq!(
        summary.transport.request_snapshot.history,
        summary.transport.controller_snapshot.messages
    );
    assert_eq!(
        summary
            .request_budget_evidence
            .request_text_measurement
            .history_message_count,
        (snapshot.items.len() + 1) as u64
    );
    assert!(summary.transport.dynamic_prompt_layers.is_empty());
    compaction_source::verify_compaction_transport_selection(
        snapshot.clone(),
        &summary_facts,
        summary_candidate.clone(),
    )?;
    // An exact business retry observes no revision or producer-state change from the isolated summary.
    // 精确业务重试不会观察到隔离摘要引起的修订或生产者状态变化。
    context_injection::synchronize_request_dynamic_context(&handle, &facts, &business_candidate)?;
    assert_eq!(
        before.0,
        handle.read(|document| document.memory_revision())?
    );
    summary_candidate.request_snapshot.prompt.clear();
    assert!(
        context_injection::compaction_request_messages(&summary_facts, &summary_candidate).is_err()
    );

    let rebuilt = rebuild_provider_transport_candidate(
        snapshot,
        &facts,
        candidate,
        media::PreparedV1UserMedia {
            prompts: BTreeMap::new(),
            bindings: Vec::new(),
        },
    )?;
    let assistant = rebuilt
        .transport
        .exact_request
        .messages
        .iter()
        .find(|message| message.role == MessageRole::Assistant)
        .expect("the provider-sourced assistant group must survive the rebuild");

    assert_eq!(assistant.content, durable_canonical_content());
    assert!(
        assistant.provider_context_items.is_empty(),
        "an unfrozen route must never replay another route's provider items"
    );
    Ok(())
}

/// Proves tool-request formation events gain the active API identity that projection requires, for every stream owner.
/// 证明工具请求构造事件获得投影所需的活动 API 身份，适用于每个事件流所有者。
#[test]
fn tool_request_formation_binds_the_active_api() {
    use vulcan_agent_runtime::agent_runtime::events::RuntimeEventKind;
    let handle = published_open_turn();
    let mut sink = ProviderV1StreamSink::new(
        Arc::clone(&handle),
        Arc::new(ProjectResourceCatalogRegistry::default()),
    );
    let formation = || {
        RuntimeEvent::new(RuntimeEventKind::ToolRequestProgress, "", "")
            .with_metadata(Some(json!({ "request_stream_id": "stream" })))
    };

    let mut early = formation();
    assert!(sink.bind_tool_request_preview(&mut early).is_err());
    assert!(
        early
            .metadata
            .as_ref()
            .unwrap()
            .get("api_call_id")
            .is_none()
    );

    prepare_sink_request(&handle, &mut sink);
    let api_call_id = sink.request_preview_api_id().unwrap();
    let mut bound = formation();
    sink.bind_tool_request_preview(&mut bound).unwrap();
    assert_eq!(bound.metadata.unwrap()["api_call_id"], json!(api_call_id));

    let mut other = RuntimeEvent::new(RuntimeEventKind::ToolRequestAborted, "", "")
        .with_metadata(Some(json!({ "request_stream_id": "stream" })));
    sink.bind_tool_request_preview(&mut other).unwrap();
    assert!(other.metadata.unwrap().get("api_call_id").is_none());
}
