//! Provider runtime context helpers for system preamble and turn-budget resolution.
//! 面向系统前导语与轮次预算解析的 Provider 运行时上下文辅助模块。

use crate::agent_runtime::config::RuntimeAgentDefinition;
use crate::agent_runtime::execution_models::{
    ExecutionPreparation, ExecutionPromptPurpose, persisted_personality_to_prompt_selection,
};
use crate::agent_runtime::prompt_composer_runtime::{
    record_runtime_prompt_composer_report, runtime_prompt_composer_engine,
};
use crate::agent_runtime::provider_prompt_loader::{
    capability_guide_prompt, load_effective_system_prompt,
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::Path,
    sync::{Mutex, OnceLock},
};
use vulcan_ai_contract::{HOST_MESSAGE_GUIDANCE, HostMessageEnvelope, HostMessageKind};
use vulcan_context::runtime_host::{RUNTIME_HOST_METADATA_VERSION, RuntimeHostEventMetadata};
use vulcan_core::types::{
    ConversationMessage as CoreConversationMessage, ConversationPayload as CoreConversationPayload,
    ConversationRecord as CoreConversationRecord, ConversationSnapshot as CoreConversationSnapshot,
    ProviderPromptLayers as CoreProviderPromptLayers,
    TranscriptRecordKind as CoreTranscriptRecordKind, TranscriptRole as CoreTranscriptRole,
};
use vulcan_hooks::{SystemPromptHookBaselineKind, SystemPromptHookBlock};
use vulcan_prompt_composer::{
    AdditionalPromptSegment, AgentPromptSnapshot, DEFAULT_PROJECT_RULES_FILE_NAME,
    EnvironmentContextSnapshot, ModelDescriptor, ProjectRulesFileCache, ProjectRulesSnapshot,
    PromptAssemblyInput, PromptAssemblyMode, PromptAssemblyOutput, PromptBaselineKind,
    PromptBaselineProvenance, PromptSegment, PromptTarget, ProviderPromptSnapshot,
    SubagentSnapshot, ToolCatalogSnapshot, ToolDescriptor,
    build_context_compaction_developer_segment,
};
use vulcan_session_core::v1::DedicatedToolAccess;
use vulcan_session_core::v1::context::{
    LockedBaselineProjection, LockedBaselineSegmentInput, LockedPromptTarget, LockedSourceKind,
};

/// Process-wide project-rules source cache shared by provider runtime requests.
/// Provider 运行时请求共享的进程级项目规则来源缓存。
static PROJECT_RULES_FILE_CACHE: OnceLock<Mutex<ProjectRulesFileCache>> = OnceLock::new();

/// Builds the effective layered runtime prompt sent to the real provider agent.
/// 构建发送给真实 Provider 代理的生效分层运行时提示词。
///
/// # Parameters
/// # 参数
/// - `preparation`: Validated execution preparation owning every runtime prompt source.
/// - `preparation`：拥有全部运行时提示来源的已验证执行准备。
/// - `active_agent`: Optional resolved agent definition used when no request snapshot overrides it.
/// - `active_agent`：请求快照未覆盖时使用的可选已解析代理定义。
///
/// # Returns
/// # 返回值
/// Returns complete prompt layers, or an exact project-source or composer diagnostic before provider launch.
/// 返回完整提示层；若项目来源或组合器失败，则在 Provider 启动前返回精确诊断。
pub(crate) fn build_runtime_prompt_layers_for_agent(
    preparation: &ExecutionPreparation,
    active_agent: Option<&RuntimeAgentDefinition>,
) -> Result<RuntimePromptLayers, String> {
    // input contains every validated project-owned and runtime-owned prompt source.
    // input 包含所有已验证的项目级与运行时级提示来源。
    let input = build_runtime_prompt_assembly_input(preparation, active_agent)?;
    // output must represent a successful composition; composer failures cannot silently drop prompt layers.
    // output 必须代表成功装配；组合器失败不能静默丢弃提示层。
    let output = runtime_prompt_composer_engine()
        .compose(&input)
        .map_err(|error| format!("Runtime prompt composition failed: {error}"))?;
    if active_agent.is_none() {
        record_runtime_prompt_composer_report(
            preparation.session_id.to_string(),
            preparation.session_title.clone(),
            "primary",
            preparation.model_blueprint.model_id.clone(),
            &output,
        );
    }

    RuntimePromptLayers::from_prompt_output(&output, preparation)
}

/// Runtime prompt layers plus the legacy fallback text used by old transport code.
/// 运行时提示词层，以及旧传输代码使用的兼容回退文本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimePromptLayers {
    /// Legacy fallback system-preamble text.
    /// 旧版兼容系统前导语文本。
    pub(crate) fallback_text: String,
    /// Complete composer projection, including request-dynamic segments and every hook block.
    /// 完整组合器投影，包含请求动态片段与全部 Hook 块。
    ///
    /// This projection is not a fixed-top input: its `system` carries request-dynamic text, so
    /// handing it to a directly dispatched controller configuration would move dynamic content
    /// back into the top System/Developer message. The RuntimeOnly candidate instead uses the
    /// stable fallback top and synchronizes dynamic host events into tagged User history.
    /// The fixed-top authority is `fallback_text` plus `locked_baseline`.
    /// 此投影不是固定顶部输入：其 `system` 携带请求动态文本，交给控制器配置会把动态内容重新移回
    /// 顶部 System/Developer 消息。RuntimeOnly 候选则使用稳定回退顶部，并将动态宿主事件同步进
    /// 带标签 User 历史。
    /// 固定顶部权威是 `fallback_text` 与 `locked_baseline`。
    pub(crate) provider_layers: CoreProviderPromptLayers,
    /// Request-dynamic prompt layers that are not part of the locked context baseline.
    /// 不属于锁定上下文基线的请求动态提示词层。
    pub(crate) dynamic_prompt_layers: CoreProviderPromptLayers,
    /// Fixed system and developer instructions resolved for this execution.
    /// 本次执行解析出的固定系统和开发者指令。
    pub(crate) fixed_instruction_layers: CoreProviderPromptLayers,
    /// Canonical context-generation baseline projected from producer-owned typed provenance.
    /// 根据生产者持有的强类型来源投影出的规范上下文代际基线。
    pub(crate) locked_baseline: LockedBaselineProjection,
    /// Ordered non-V1 host events produced from this prompt assembly.
    /// 从本次提示词装配生产的有序非 V1 宿主事件。
    pub(crate) host_events: Vec<RuntimeHostEventCandidate>,
}

/// One typed host event candidate waiting for canonical User-history synchronization.
/// 等待同步到规范 User 历史的一条强类型宿主事件候选。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeHostEventCandidate {
    /// Stable producer-owned source identity, independent from body content.
    /// 与正文内容无关、由生产者持有的稳定来源身份。
    pub(crate) source_id: String,
    /// Host source label rendered into the envelope.
    /// 渲染到信封中的宿主来源标签。
    pub(crate) source: String,
    /// Provider-facing target identity.
    /// 面向 Provider 的目标身份。
    pub(crate) target: String,
    /// Optional logical task association.
    /// 可选的逻辑任务关联。
    pub(crate) task_id: Option<String>,
    /// Scope in which the event takes effect.
    /// 事件生效的作用域。
    pub(crate) scope: String,
    /// Untrusted host-provided body.
    /// 宿主提供的不可信事件正文。
    pub(crate) body: String,
    /// Stable occurrence identity supplied by a stateful producer when already allocated.
    /// 已由有状态生产者分配时携带的稳定发生点身份。
    pub(crate) occurrence_id: Option<String>,
    /// Producer-owned raw-body hash used for version comparison.
    /// 用于版本比较的生产者原始正文哈希。
    pub(crate) content_hash: Option<String>,
    /// Existing event identity for an expiration marker, if this is one.
    /// 若为失效标记，则携带已有的事件身份。
    pub(crate) event_id: Option<String>,
    /// Semantic host-message kind.
    /// 宿主消息的语义种类。
    pub(crate) kind: HostMessageKind,
    /// Whether this event invalidates an earlier occurrence of the same source.
    /// 此事件是否使同一来源的较早发生点失效。
    pub(crate) expires_source: bool,
}

impl RuntimePromptLayers {
    /// Build prompt layers from one prompt-composer output.
    /// 从一份提示词组合器输出构建提示词层。
    ///
    /// # Parameters
    /// # 参数
    /// - `output`: Successful structured prompt-composer output.
    /// - `output`：成功的结构化提示词组合器输出。
    /// - `preparation`: Validated execution preparation supplying hook blocks and provider resources.
    /// - `preparation`：提供 Hook 块与 Provider 资源的已验证执行准备。
    ///
    /// # Returns
    /// # 返回值
    /// Returns structured and rendered transport layers, or the exact resource failure needed by an empty-output fallback.
    /// 返回结构化与渲染后的传输层，或空输出回退所需资源的精确失败。
    fn from_prompt_output(
        output: &PromptAssemblyOutput,
        preparation: &ExecutionPreparation,
    ) -> Result<Self, String> {
        // provider_layers is the canonical structured projection produced by the prompt composer.
        // provider_layers 是提示词组合器生成的规范结构化投影。
        let mut provider_layers = render_runtime_prompt_layers(output);
        append_system_prompt_hook_region(
            &mut provider_layers,
            &preparation.system_prompt_hook_blocks,
        );
        // locked_baseline is derived from the same ordered typed segments that produced provider_layers.
        // locked_baseline 来自生成 provider_layers 的同一组有序强类型片段。
        let locked_baseline =
            project_locked_runtime_baseline(output, &preparation.system_prompt_hook_blocks)?;
        // dynamic_prompt_layers is filtered by producer-owned provenance, so the permit can
        // combine request-local instructions without duplicating the locked baseline.
        // dynamic_prompt_layers 按生产者持有的来源信息过滤，使许可可以合并请求级指令且不重复锁定基线。
        let dynamic_prompt_layers =
            render_request_dynamic_prompt_layers(output, &preparation.system_prompt_hook_blocks);
        // Carry product and agent foundations separately so existing V1 sessions can use them
        // without editing the persisted generation lock or freezing the editable base prompt.
        // 单独传递产品及代理基础指令，使现有 V1 会话能使用它们，同时保留已持久化的代际锁和基础提示词设置的生效时机。
        let fixed_instruction_layers = CoreProviderPromptLayers {
            system: render_fixed_instruction_segments(&output.system_segments),
            developer: render_fixed_instruction_segments(&output.developer_segments),
            ..Default::default()
        };
        // fallback_text contains only the producer-declared stable baseline; dynamic material is emitted as User events by the non-V1 transport.
        // fallback_text 只包含生产者声明的稳定基线；动态内容由非 V1 传输层作为 User 事件发送。
        let stable_provider_layers =
            render_locked_runtime_prompt_layers(output, &preparation.system_prompt_hook_blocks);
        let fallback_text = if stable_provider_layers.is_empty() {
            build_legacy_runtime_system_preamble(preparation)?
        } else {
            stable_provider_layers.render_fallback_system_prompt()
        };
        let host_events =
            build_runtime_prompt_host_events(output, &preparation.system_prompt_hook_blocks);
        Ok(Self {
            fallback_text,
            provider_layers,
            dynamic_prompt_layers,
            fixed_instruction_layers,
            locked_baseline,
            host_events,
        })
    }
}

/// Builds typed host-event candidates without collapsing them into a request-local prefix.
/// 构建强类型宿主事件候选，不将其折叠为请求局部前缀。
fn build_runtime_prompt_host_events(
    output: &PromptAssemblyOutput,
    hook_blocks: &[SystemPromptHookBlock],
) -> Vec<RuntimeHostEventCandidate> {
    let mut candidates = Vec::new();
    for segments in [
        output.system_segments.as_slice(),
        output.developer_segments.as_slice(),
        output.user_segments.as_slice(),
        output.tool_metadata_segments.as_slice(),
    ] {
        for segment in segments {
            let body = segment.body.trim();
            if body.is_empty() {
                continue;
            }
            if segment.baseline_provenance.is_some() && segment.target != PromptTarget::User {
                continue;
            }
            let source_id = if let Some(provenance) = segment.baseline_provenance.as_ref() {
                format!(
                    "prompt-baseline:{}:{}",
                    provenance.source_id, segment.node_id
                )
            } else {
                format!("prompt-dynamic:{}", segment.node_id)
            };
            let source = if segment.source_label.trim().is_empty() {
                String::from("prompt")
            } else {
                segment.source_label.clone()
            };
            candidates.push(RuntimeHostEventCandidate {
                source_id,
                source,
                target: String::from("model"),
                task_id: None,
                scope: if segment.baseline_provenance.is_some() {
                    String::from("session")
                } else {
                    String::from("request")
                },
                body: body.to_string(),
                occurrence_id: None,
                content_hash: None,
                event_id: None,
                kind: HostMessageKind::ContextUpdate,
                expires_source: false,
            });
        }
    }
    for block in hook_blocks {
        if block.baseline_provenance.is_some() {
            continue;
        }
        let Some(body) = render_system_prompt_hook_block(block) else {
            continue;
        };
        let source = block
            .source
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("hook");
        candidates.push(RuntimeHostEventCandidate {
            source_id: format!("hook:{}:{}:{}", block.phase.as_str(), source, block.tag),
            source: source.to_string(),
            target: String::from("model"),
            task_id: None,
            scope: String::from("request"),
            body,
            occurrence_id: None,
            content_hash: None,
            event_id: None,
            kind: HostMessageKind::ContextUpdate,
            expires_source: false,
        });
    }
    candidates
}

/// Synchronizes typed runtime host candidates into canonical User history once.
/// 将强类型运行时宿主候选只同步一次到规范 User 历史。
///
/// Existing canonical messages and metadata are never rewritten. New candidates are inserted
/// before the admitted user only while that user is still the current unsent boundary; callers
/// pass `None` after the boundary has been sent so later events append at the end.
/// 既有规范消息与元数据永不改写。只有在准入用户仍是未发送边界时才插入其前方；边界发送后调用方传入 `None`，后续事件追加到末尾。
pub(crate) fn synchronize_runtime_host_events(
    snapshot: &mut CoreConversationSnapshot,
    candidates: &[RuntimeHostEventCandidate],
    current_user_message_id: Option<&str>,
) -> Result<(), String> {
    // Only this producer's records carry its schema; malformed owned records must not be forgotten.
    // 仅本生产者的记录承载此 Schema；格式错误的所属记录不得被忽略。
    let mut metadata_rows = vulcan_context::runtime_host::read_runtime_host_ledger(
        &crate::agent_runtime::provider_request_bridge_conversation::core_snapshot_to_session_conversation_state(snapshot)?,
    )?;
    // Stage the entire batch so a later identity conflict cannot publish a partial projection.
    // 暂存整个批次，避免后续身份冲突发布部分投影。
    let mut staged_messages: Vec<CoreConversationMessage> = Vec::new();
    let mut staged_records = Vec::new();
    // The caller supplies the admitted unsent user boundary; absent boundaries append at the tail.
    // 调用方提供已准入且未发送的用户边界；未提供边界时追加到尾部。
    let insertion_index = match current_user_message_id {
        Some(message_id) => snapshot
            .messages
            .iter()
            .position(|message| message.id == message_id)
            .ok_or_else(|| format!("Runtime host insertion boundary `{message_id}` is missing."))?,
        None => snapshot.messages.len(),
    };

    for candidate in candidates {
        // Preserve the existing normalized envelope body while retaining the producer's original content hash.
        // 保留既有信封正文规范化规则，同时保留生产者原始内容哈希。
        let body = candidate.body.trim();
        if body.is_empty() {
            continue;
        }
        let body_hash = candidate
            .content_hash
            .clone()
            .unwrap_or_else(|| hex::encode(Sha256::digest(body.as_bytes())));
        // Render with an explicit event identity; expiry events reference an occurrence without reusing its event.
        // 使用明确事件身份渲染；失效事件引用发生点，但不复用其事件身份。
        let envelope_for_event = |event_id: &str| HostMessageEnvelope {
            event_id: event_id.to_string(),
            kind: candidate.kind,
            source: candidate.source.clone(),
            target: candidate.target.clone(),
            task_id: candidate.task_id.clone(),
            question_id: None,
            scope: candidate.scope.clone(),
            body: body.to_string(),
        };
        let render_event = |event_id: &str| envelope_for_event(event_id).render();
        // Exact rendered bytes also protect routing and scope, which a body hash alone cannot authenticate.
        // 精确渲染字节同时保护路由和作用域，单独正文哈希无法确认这些字段。
        let matches_message = |metadata: &RuntimeHostEventMetadata, rendered: &str| {
            if !metadata
                .envelope
                .render()
                .is_ok_and(|expected| expected == rendered)
            {
                return false;
            }
            let message_id = metadata.message_id();
            let mut matches = snapshot
                .messages
                .iter()
                .chain(staged_messages.iter())
                .filter(|message| message.id == message_id);
            let first = matches.next();
            if metadata.compacted_out {
                // Only a committed compaction fact permits an absent event; exact replay never restores it.
                // 仅已提交的压缩事实允许事件缺失；精确重放不会恢复该事件。
                return first.is_none();
            }
            matches.next().is_none()
                && first.is_some_and(|message| {
                    matches!(
                        &message.payload,
                        CoreConversationPayload::UserText { text, attachments }
                            if text == rendered && attachments.is_empty()
                    )
                })
        };
        // Only snapshot-style producers without occurrence identities may coalesce identical current values.
        // 只有不提供发生点身份的快照型生产者可以合并相同当前值。
        let latest_source = metadata_rows
            .iter()
            .filter(|metadata| metadata.source_id == candidate.source_id)
            .max_by_key(|metadata| metadata.occurrence);
        if candidate.occurrence_id.is_none()
            && candidate.event_id.is_none()
            && !candidate.expires_source
            && let Some(latest) = metadata_rows
                .iter()
                .filter(|metadata| {
                    metadata.source_id == candidate.source_id && !metadata.expires_source
                })
                .max_by_key(|metadata| metadata.occurrence)
            && latest.body_hash == body_hash
            && !metadata_rows
                .iter()
                .any(|event| event.expires_source && event.occurrence_id == latest.occurrence_id)
            && matches_message(latest, &render_event(&latest.event_id)?)
        {
            continue;
        }
        // Source sequence orders physical events, while occurrence identity binds update and expiry together.
        // 来源序号排序物理事件，发生点身份则关联更新及其失效事件。
        let occurrence = latest_source
            .map_or(0, |metadata| metadata.occurrence)
            .checked_add(1)
            .ok_or_else(|| String::from("Runtime host occurrence sequence exhausted."))?;
        let occurrence_id = candidate.occurrence_id.clone().unwrap_or_else(|| {
            format!(
                "runtime-host:{}:{occurrence}",
                source_identity_hash(&candidate.source_id)
            )
        });
        let event_id = candidate
            .event_id
            .clone()
            .unwrap_or_else(|| occurrence_id.clone());
        let rendered = render_event(&event_id)?;
        // Replay is keyed by the event, not by the occurrence shared with its expiration marker.
        // 重放按事件去重，不按与失效标记共享的发生点去重。
        if let Some(existing) = metadata_rows
            .iter()
            .find(|metadata| metadata.event_id == event_id)
        {
            if existing.source_id != candidate.source_id
                || existing.occurrence_id != occurrence_id
                || existing.body_hash != body_hash
                || existing.expires_source != candidate.expires_source
                || !matches_message(existing, &rendered)
            {
                return Err(format!(
                    "Runtime host event `{event_id}` was reused with different metadata or content."
                ));
            }
            continue;
        }
        // One occurrence has one update; distinct withdrawal and late TTL events may invalidate it independently.
        // 一个发生点只有一个更新；独立撤下和迟到 TTL 事件可以分别使其失效。
        if metadata_rows.iter().any(|metadata| {
            metadata.occurrence_id == occurrence_id
                && (metadata.source_id != candidate.source_id
                    || (!metadata.expires_source && !candidate.expires_source))
        }) {
            return Err(format!(
                "Runtime host occurrence `{occurrence_id}` was reused with a different event identity."
            ));
        }
        let message_id = format!("runtime-host:{event_id}");
        if snapshot
            .messages
            .iter()
            .chain(staged_messages.iter())
            .any(|message| message.id == message_id)
        {
            return Err(format!(
                "Runtime host message `{message_id}` has no matching metadata."
            ));
        }
        // Store the raw event identity consistently; the transport message prefix belongs only to message IDs.
        // 一致保存原始事件身份；传输消息前缀仅属于消息 ID。
        let envelope = envelope_for_event(&event_id);
        let metadata = RuntimeHostEventMetadata {
            version: RUNTIME_HOST_METADATA_VERSION,
            source_id: candidate.source_id.clone(),
            occurrence,
            occurrence_id,
            body_hash,
            event_id,
            expires_source: candidate.expires_source,
            envelope,
            compacted_out: false,
        };
        // Validate required ledger fields before publishing any part of the candidate batch.
        // 发布候选批次的任何部分前校验必需账本字段。
        let metadata_value = serde_json::to_value(&metadata)
            .map_err(|error| format!("Runtime host metadata serialization failed: {error}"))?;
        RuntimeHostEventMetadata::decode(&metadata.record_id(), Some(&metadata_value))?;
        staged_records.push(CoreConversationRecord {
            id: metadata.record_id(),
            role: CoreTranscriptRole::User,
            record_kind: CoreTranscriptRecordKind::ProjectionOnly,
            visible: false,
            title: String::from("Runtime host context"),
            body: body.to_string(),
            metadata: Some(metadata_value),
            created_at: String::from("0"),
        });
        staged_messages.push(CoreConversationMessage {
            id: message_id,
            payload: CoreConversationPayload::UserText {
                text: rendered,
                attachments: Vec::new(),
            },
        });
        metadata_rows.push(metadata);
    }
    // Publish only after every candidate has passed identity and envelope validation.
    // 仅在全部候选通过身份与信封校验后发布。
    snapshot
        .messages
        .splice(insertion_index..insertion_index, staged_messages);
    snapshot.records.extend(staged_records);
    Ok(())
}

/// Synchronizes one complete business request or one explicitly partial auxiliary request atomically.
/// 原子同步一个完整业务请求或一个明确局部的辅助请求。
/// `snapshot` is canonical history, `candidates` come from the completed prompt/injection assembly,
/// `purpose` determines source-list authority, and `current_user_message_id` is an unsent boundary only.
/// `snapshot` 是规范历史，`candidates` 来自完成的提示/注入装配，`purpose` 决定清单权威，
/// `current_user_message_id` 仅表示尚未发送的边界。
/// Returns success after one commit or an explicit conflict without partially publishing updates or withdrawals.
/// 一次提交后返回成功；冲突时不发布部分更新或撤下消息。
pub(crate) fn synchronize_runtime_host_request_context(
    snapshot: &mut CoreConversationSnapshot,
    candidates: &[RuntimeHostEventCandidate],
    current_user_message_id: Option<&str>,
    purpose: ExecutionPromptPurpose,
) -> Result<(), String> {
    match purpose {
        ExecutionPromptPurpose::QuestionExecution
        | ExecutionPromptPurpose::ContextCompaction
        | ExecutionPromptPurpose::ContextCompactionCachePreserving
        | ExecutionPromptPurpose::GitCommitSummary => {
            return synchronize_runtime_host_events(snapshot, candidates, current_user_message_id);
        }
        ExecutionPromptPurpose::UserTurn
        | ExecutionPromptPurpose::AgentTask
        | ExecutionPromptPurpose::ExecutionRetry => {}
    }
    // Complete current sources must not contain two competing updates for the same producer slot.
    // 完整当前清单不能为同一生产者槽位提供两个相互竞争的更新。
    let mut current_sources = std::collections::BTreeMap::new();
    for candidate in candidates
        .iter()
        .filter(|candidate| !candidate.expires_source)
    {
        if current_sources
            .insert(candidate.source_id.as_str(), candidate)
            .is_some()
        {
            return Err(format!(
                "Complete runtime host context has duplicate source `{}`.",
                candidate.source_id
            ));
        }
    }
    let mut staged = snapshot.clone();
    synchronize_runtime_host_events(&mut staged, candidates, current_user_message_id)?;
    let ledger = vulcan_context::runtime_host::read_runtime_host_ledger(
        &crate::agent_runtime::provider_request_bridge_conversation::core_snapshot_to_session_conversation_state(&staged)?,
    )?;
    let active_ids = vulcan_context::runtime_host::active_runtime_host_message_ids(&ledger);
    let active_sources = ledger
        .iter()
        .filter(|row| active_ids.contains(&row.message_id()))
        .map(|row| (row.source_id.as_str(), row))
        .collect::<std::collections::BTreeMap<_, _>>();
    for (source, candidate) in &current_sources {
        let active = active_sources.get(source).ok_or_else(|| format!("Current runtime host source `{source}` refers to an already invalidated occurrence."))?;
        if candidate
            .occurrence_id
            .as_ref()
            .is_some_and(|occurrence| occurrence != &active.occurrence_id)
        {
            return Err(format!(
                "Current runtime host source `{source}` does not match its active occurrence."
            ));
        }
    }
    // Withdraw only previously active sources absent from this complete inventory, preserving their original route.
    // 仅撤下完整清单中缺失的既有有效来源，并保留其原始路由。
    let withdrawn = active_sources.iter().filter(|(_, row)| !current_sources.contains_key(row.source_id.as_str()))
        .map(|(_, row)| RuntimeHostEventCandidate {
            source_id: row.source_id.clone(), source: row.envelope.source.clone(),
            target: row.envelope.target.clone(), task_id: row.envelope.task_id.clone(), scope: row.envelope.scope.clone(),
            body: format!("This context source has been withdrawn. Stop applying event `{}` (occurrence `{}`).", row.event_id, row.occurrence_id),
            occurrence_id: Some(row.occurrence_id.clone()), content_hash: Some(row.body_hash.clone()),
            event_id: Some(format!("context-withdrawn:{}", row.event_id)), kind: HostMessageKind::ContextExpired,
            expires_source: true,
        }).collect::<Vec<_>>();
    synchronize_runtime_host_events(&mut staged, &withdrawn, current_user_message_id)?;
    *snapshot = staged;
    Ok(())
}

/// Hashes only the typed producer identity used to namespace generated event IDs.
/// 仅对用于生成事件 ID 命名空间的强类型生产者身份做哈希。
fn source_identity_hash(source_id: &str) -> String {
    hex::encode(Sha256::digest(source_id.as_bytes()))
}

/// Converts producer-owned context blocks and expiration facts into typed runtime candidates.
/// 将生产者持有的上下文块与失效事实转换为强类型运行时候选。
pub(crate) fn runtime_context_host_event_candidates(
    injection: &vulcan_core::types::ProviderContextInjection,
) -> Vec<RuntimeHostEventCandidate> {
    let mut candidates = injection
        .host_blocks
        .iter()
        .map(|block| RuntimeHostEventCandidate {
            source_id: block.source_id.clone(),
            source: block.source.clone(),
            target: block.target.clone(),
            task_id: block.task_id.clone(),
            scope: block.scope.clone(),
            body: block.body.clone(),
            occurrence_id: Some(block.occurrence_id.clone()),
            content_hash: block.content_hash.clone(),
            event_id: None,
            kind: HostMessageKind::ContextUpdate,
            expires_source: false,
        })
        .collect::<Vec<_>>();
    candidates.extend(
        injection
            .expired_events
            .iter()
            .map(|event| RuntimeHostEventCandidate {
                source_id: event.source_id.clone(),
                source: event.source.clone(),
                target: event.target.clone(),
                task_id: None,
                scope: event.scope.clone(),
                body: event.body.clone(),
                occurrence_id: Some(event.occurrence_id.clone()),
                content_hash: Some(event.content_hash.clone()),
                event_id: Some(event.event_id.clone()),
                kind: HostMessageKind::ContextExpired,
                expires_source: true,
            }),
    );
    candidates
}

/// Builds the prompt-composer input used by the runtime bridge.
/// 构建运行时桥接层使用的提示词组合输入。
///
/// # Parameters
/// # 参数
/// - `preparation`: Validated execution preparation containing workspace-owned prompt configuration.
/// - `preparation`：包含工作区自有提示配置的已验证执行准备。
/// - `active_agent`: Optional resolved agent definition used as a prompt fallback source.
/// - `active_agent`：作为提示词回退来源使用的可选已解析代理定义。
///
/// # Returns
/// # 返回值
/// Returns an assembly input only after every configured project source loads successfully.
/// 仅在所有已配置项目来源均成功加载后返回装配输入。
fn build_runtime_prompt_assembly_input(
    preparation: &ExecutionPreparation,
    active_agent: Option<&RuntimeAgentDefinition>,
) -> Result<PromptAssemblyInput, String> {
    // provider_prompt is the first non-blank layer of project override, user override, and built-in default.
    // provider_prompt 是项目覆盖、用户覆盖与内置默认中第一个非空层级。
    let provider_prompt =
        load_effective_system_prompt(resolve_workspace_directory(&preparation.workspace_path)?)?;
    // A dedicated session's base prompt replaces every agent prompt source.
    // 专职会话的基础提示词取代所有代理提示词来源。
    let agent_prompt = match preparation.dedicated_session.as_ref() {
        Some(profile) => Some(
            AgentPromptSnapshot::new(profile.base_prompt.clone())
                .with_source_label("Dedicated session base prompt")
                .with_fixed_instruction(),
        ),
        None => build_request_agent_prompt_snapshot(preparation)
            // A child has its own session and inherits the parent's captured base definition.
            // 子代理有独立会话，并继承已捕获的父代理基础定义。
            .map(|snapshot| {
                if preparation.subagent_prompt_context.is_some() {
                    snapshot.with_fixed_instruction()
                } else {
                    snapshot
                }
            })
            .or_else(|| active_agent.and_then(build_agent_prompt_snapshot)),
    };
    let environment_context = EnvironmentContextSnapshot::new(format!(
        "Current workspace: {}\nCurrent execution mode: {}",
        preparation.workspace_path, preparation.execution_mode
    ));
    // Dedicated sessions do not describe the general product; only their own tool contracts remain.
    // 专职会话不介绍通用产品能力，只保留其自身工具契约。
    let mut custom_developer_segments = if preparation.dedicated_session.is_some() {
        Vec::new()
    } else {
        build_prompt_purpose_developer_segments(preparation.prompt_purpose)
    };
    if let Some(segment) = preparation.session_plan_context.clone() {
        custom_developer_segments.push(AdditionalPromptSegment::dynamic(
            "Session Plan Context",
            segment,
            "Runtime session plan context",
        ));
    }
    custom_developer_segments.extend(preparation.additional_developer_segments.clone());

    let subagent_prompt_context = preparation.subagent_prompt_context.as_ref();
    Ok(PromptAssemblyInput {
        mode: prompt_assembly_mode_for_preparation(preparation),
        model: ModelDescriptor {
            family: preparation
                .model_blueprint
                .protocol_profile_key()
                .to_string(),
            model_id: preparation.model_blueprint.model_id.clone(),
            supports_native_personality: true,
        },
        provider_prompt: Some(
            ProviderPromptSnapshot::new(provider_prompt.text)
                .with_source_label(provider_prompt.source_label),
        ),
        agent_prompt,
        personality: persisted_personality_to_prompt_selection(
            &preparation.prompt_configuration.personality,
        ),
        output_style: preparation.output_style_snapshot.clone(),
        project_rules: load_workspace_project_rules_snapshot(&preparation.workspace_path)?,
        environment_context: Some(environment_context),
        // Session goals are delivered through append-only user turns so goal creation never rewrites the provider prefix.
        // 会话目标通过追加式用户回合投递，确保目标创建不会改写 Provider 前缀。
        task_goal: None,
        tool_catalog: ToolCatalogSnapshot::new(
            preparation
                .tool_descriptors
                .iter()
                .map(|tool| ToolDescriptor {
                    id: tool.id.clone(),
                    title: tool.name.clone(),
                    description: tool.description.clone(),
                    help_text: tool.help_text.clone(),
                    schema_fingerprint: None,
                })
                .collect(),
        ),
        subagent: subagent_prompt_context.map(|context| SubagentSnapshot {
            identity: context.identity.clone(),
            task: context.delegated_task.clone(),
            delegation: context.delegation.clone(),
            functional_system_prompt: context.functional_system_prompt.clone(),
        }),
        custom_developer_segments,
        custom_user_segments: Vec::new(),
    })
}

/// Selects the prompt-composer mode for one prepared execution.
/// 为一次已准备执行选择提示词组合模式。
fn prompt_assembly_mode_for_preparation(preparation: &ExecutionPreparation) -> PromptAssemblyMode {
    if let Some(context) = preparation.subagent_prompt_context.as_ref() {
        if context.uses_functional_prompt() {
            return PromptAssemblyMode::SubagentFunctional;
        }
        return PromptAssemblyMode::SubagentClone;
    }
    if let Some(profile) = preparation.dedicated_session.as_ref()
        && preparation.prompt_purpose.supports_user_turn_tooling()
    {
        return match profile.tool_access {
            DedicatedToolAccess::None => PromptAssemblyMode::DedicatedMinimal,
            DedicatedToolAccess::Standard => PromptAssemblyMode::DedicatedTooling,
        };
    }
    match preparation.prompt_purpose {
        ExecutionPromptPurpose::UserTurn
        | ExecutionPromptPurpose::AgentTask
        | ExecutionPromptPurpose::ExecutionRetry
        | ExecutionPromptPurpose::QuestionExecution
        | ExecutionPromptPurpose::ContextCompactionCachePreserving => PromptAssemblyMode::Primary,
        ExecutionPromptPurpose::ContextCompaction => PromptAssemblyMode::ContextCompactionMinimal,
        ExecutionPromptPurpose::GitCommitSummary => PromptAssemblyMode::ContextCompactionMinimal,
    }
}

/// Builds developer-side runtime instructions required by a special prompt purpose.
/// 构建特殊提示词用途所需的开发者侧运行时指令。
fn build_prompt_purpose_developer_segments(
    purpose: ExecutionPromptPurpose,
) -> Vec<AdditionalPromptSegment> {
    match purpose {
        ExecutionPromptPurpose::UserTurn
        | ExecutionPromptPurpose::AgentTask
        | ExecutionPromptPurpose::ExecutionRetry
        | ExecutionPromptPurpose::QuestionExecution
        | ExecutionPromptPurpose::ContextCompactionCachePreserving => {
            // Conversational purposes, including cache-preserving compaction, keep one shared prefix.
            // 会话类用途（含保留缓存的压缩）保持同一共享前缀。
            vec![AdditionalPromptSegment::locked(
                "Product Capability Guide",
                capability_guide_prompt(),
                "Built-in product capability guide",
                PromptBaselineProvenance::new(
                    PromptBaselineKind::FixedInstruction,
                    "product-capability-guide",
                ),
            )]
        }
        ExecutionPromptPurpose::ContextCompaction => vec![AdditionalPromptSegment::dynamic(
            "Context Compaction Contract",
            build_context_compaction_developer_segment(),
            "Runtime context-compaction purpose",
        )],
        ExecutionPromptPurpose::GitCommitSummary => vec![AdditionalPromptSegment::dynamic(
            "Git Commit Summary Contract",
            build_git_commit_summary_developer_segment(),
            "Runtime Git commit-summary purpose",
        )],
    }
}

/// Builds the fixed developer instruction used by internal Git commit-message summaries.
/// 构建内部 Git 提交信息总结使用的固定开发者指令。
fn build_git_commit_summary_developer_segment() -> String {
    String::from(
        "You generate Git commit messages for a local desktop client. Do not call tools. Use only the repository change data in the user message. Output only the final commit message text.",
    )
}

/// Builds an agent prompt snapshot with a useful runtime source label.
/// 构建带有可用运行时来源标签的 Agent 提示词快照。
fn build_agent_prompt_snapshot(agent: &RuntimeAgentDefinition) -> Option<AgentPromptSnapshot> {
    let text = agent.system_prompt.trim();
    if text.is_empty() {
        return None;
    }
    Some(
        AgentPromptSnapshot::new(text.to_string())
            .with_source_label(format!(
                "Runtime agent `{}` ({}) system prompt",
                agent.id, agent.name
            ))
            .with_fixed_instruction(),
    )
}

/// Builds a request-scoped fixed agent prompt snapshot when one is present.
/// 当请求携带固定代理提示词快照时构建请求级代理提示词快照。
fn build_request_agent_prompt_snapshot(
    preparation: &ExecutionPreparation,
) -> Option<AgentPromptSnapshot> {
    let snapshot = preparation.agent_prompt_snapshot.as_ref()?;
    let text = snapshot.text.trim();
    if text.is_empty() {
        return None;
    }
    Some(AgentPromptSnapshot::new(text.to_string()).with_source_label(snapshot.source_label()))
}

/// Renders one prompt-composer output into structured provider prompt layers.
/// 将一份提示词组合器输出渲染为结构化 Provider 提示词层。
fn render_runtime_prompt_layers(output: &PromptAssemblyOutput) -> CoreProviderPromptLayers {
    CoreProviderPromptLayers {
        system: output.render_system_text(),
        developer: output.render_developer_text(),
        tool_guidance: output.render_tool_metadata_text(),
        runtime_context: output.render_user_text(),
    }
}

/// Renders only producer-declared baseline segments for the stable provider prefix.
/// 仅渲染生产者声明的基线片段，作为稳定 Provider 前缀。
///
/// # Parameters
/// # 参数
/// - `output`: The ordered composer output whose baseline provenance is authoritative.
/// - `output`：基线来源信息具有权威性的有序组合器输出。
/// - `hook_blocks`: Hook blocks emitted for the same prompt assembly.
/// - `hook_blocks`：同一次提示词装配生成的 Hook 块。
///
/// # Returns
/// # 返回值
/// Returns stable layers without request-dynamic context or hook text.
/// 返回不含请求动态上下文或 Hook 文本的稳定层。
fn render_locked_runtime_prompt_layers(
    output: &PromptAssemblyOutput,
    hook_blocks: &[SystemPromptHookBlock],
) -> CoreProviderPromptLayers {
    let mut layers = CoreProviderPromptLayers {
        system: render_locked_prompt_segments(&output.system_segments),
        developer: render_locked_prompt_segments(&output.developer_segments),
        tool_guidance: render_locked_prompt_segments(&output.tool_metadata_segments),
        // User-targeted host baseline text is emitted as a User event by non-V1 transport.
        // 面向 User 的宿主基线文本由非 V1 传输层作为 User 事件发送。
        runtime_context: String::new(),
    };
    let baseline_hook_region = render_system_prompt_hook_region(
        &hook_blocks
            .iter()
            .filter(|block| block.baseline_provenance.is_some())
            .cloned()
            .collect::<Vec<_>>(),
    );
    append_prompt_text(&mut layers.system, &baseline_hook_region);
    append_prompt_text(&mut layers.system, HOST_MESSAGE_GUIDANCE);
    layers
}

/// Joins only prompt segments with typed baseline provenance.
/// 仅拼接带有强类型基线来源的提示词片段。
///
/// # Parameters
/// # 参数
/// - `segments`: Ordered prompt segments for one target layer.
/// - `segments`：一个目标层的有序提示词片段。
///
/// # Returns
/// # 返回值
/// Returns the non-empty baseline segment bodies in source order.
/// 按来源顺序返回非空基线片段正文。
fn render_locked_prompt_segments(segments: &[PromptSegment]) -> String {
    segments
        .iter()
        .filter(|segment| segment.baseline_provenance.is_some())
        .map(|segment| segment.body.as_str())
        .filter(|body| !body.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Joins fixed instruction bodies in composer order for the V1 request head.
/// 按组合器顺序拼接固定指令正文，供 V1 请求头部使用。
/// `segments` contains the ordered segments for one role; returns empty text if none are fixed.
/// `segments` 包含同一角色的有序片段；没有固定指令时返回空文本。
fn render_fixed_instruction_segments(segments: &[PromptSegment]) -> String {
    segments
        .iter()
        .filter(|segment| {
            segment
                .baseline_provenance
                .as_ref()
                .is_some_and(|provenance| provenance.kind == PromptBaselineKind::FixedInstruction)
        })
        .map(|segment| segment.body.as_str())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Renders only request-dynamic prompt segments from one composer output.
/// 仅渲染一份组合器输出中的请求动态提示词片段。
///
/// # Parameters
/// # 参数
/// - `output`: Successful prompt-composer output with producer-owned baseline provenance.
/// - `output`：包含生产者持有基线来源信息的成功提示词组合器输出。
/// - `hook_blocks`: Ordered system hook blocks from the same request assembly.
/// - `hook_blocks`：来自同一次请求装配的有序系统 Hook 块。
///
/// # Returns
/// # 返回值
/// Returns layers containing only segments whose producers did not mark them as locked baseline sources.
/// 返回仅包含生产者未标记为锁定基线来源的提示词层。
fn render_request_dynamic_prompt_layers(
    output: &PromptAssemblyOutput,
    hook_blocks: &[SystemPromptHookBlock],
) -> CoreProviderPromptLayers {
    let mut layers = CoreProviderPromptLayers {
        system: render_dynamic_prompt_segments(&output.system_segments),
        developer: render_dynamic_prompt_segments(&output.developer_segments),
        tool_guidance: render_dynamic_prompt_segments(&output.tool_metadata_segments),
        runtime_context: render_dynamic_prompt_segments(&output.user_segments),
    };
    let hook_region = render_system_prompt_hook_region(
        &hook_blocks
            .iter()
            .filter(|block| block.baseline_provenance.is_none())
            .cloned()
            .collect::<Vec<_>>(),
    );
    append_prompt_text(&mut layers.system, &hook_region);
    layers
}

/// Joins request-dynamic composer segments without reclassifying them from names or titles.
/// 拼接请求动态组合器片段，不根据名称或标题重新分类。
///
/// # Parameters
/// # 参数
/// - `segments`: Ordered prompt segments for one provider target.
/// - `segments`：一个 Provider 目标的有序提示词片段。
///
/// # Returns
/// # 返回值
/// Returns non-empty bodies whose authoritative baseline provenance is absent.
/// 返回权威基线来源为空的非空正文。
fn render_dynamic_prompt_segments(segments: &[PromptSegment]) -> String {
    segments
        .iter()
        .filter(|segment| segment.baseline_provenance.is_none())
        .map(|segment| segment.body.as_str())
        .filter(|body| !body.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Appends one non-empty prompt block with the canonical two-newline separator.
/// 使用规范的双换行分隔符追加一个非空提示词块。
///
/// # Parameters
/// # 参数
/// - `target`: Existing target prompt text.
/// - `target`：现有目标提示词文本。
/// - `addition`: Candidate prompt block to append.
/// - `addition`：待追加的候选提示词块。
fn append_prompt_text(target: &mut String, addition: &str) {
    if addition.trim().is_empty() {
        return;
    }
    if target.trim().is_empty() {
        *target = addition.to_string();
    } else {
        *target = format!("{}\n\n{}", target.trim_end(), addition);
    }
}

/// Projects producer-declared mutable baselines from the exact assembled prompt boundary.
/// 从精确装配完成的提示词边界投影生产者声明的可变基线。
fn project_locked_runtime_baseline(
    output: &PromptAssemblyOutput,
    hook_blocks: &[SystemPromptHookBlock],
) -> Result<LockedBaselineProjection, String> {
    let mut segments = Vec::new();
    for segment in &output.system_segments {
        push_locked_prompt_segment(&mut segments, segment)?;
    }
    for block in hook_blocks {
        push_locked_hook_segment(&mut segments, block)?;
    }
    for segment in &output.developer_segments {
        push_locked_prompt_segment(&mut segments, segment)?;
    }
    for segment in &output.user_segments {
        push_locked_prompt_segment(&mut segments, segment)?;
    }
    for segment in &output.tool_metadata_segments {
        if segment.baseline_provenance.is_some() {
            return Err(format!(
                "Prompt segment `{}` declared locked provenance for excluded tool metadata.",
                segment.node_id
            ));
        }
    }
    LockedBaselineProjection::project(segments)
        .map_err(|error| format!("Runtime locked-baseline projection failed: {error}"))
}

/// Appends one composer segment only when its authoritative producer marked it as a baseline.
/// 仅当权威生产者将组合器片段标记为基线时才追加该片段。
fn push_locked_prompt_segment(
    output: &mut Vec<LockedBaselineSegmentInput>,
    segment: &PromptSegment,
) -> Result<(), String> {
    let Some(provenance) = segment.baseline_provenance.as_ref() else {
        return Ok(());
    };
    let target = match segment.target {
        PromptTarget::System => LockedPromptTarget::System,
        PromptTarget::Developer => LockedPromptTarget::Developer,
        PromptTarget::User => LockedPromptTarget::User,
        PromptTarget::ToolMetadata => {
            return Err(format!(
                "Prompt segment `{}` cannot lock tool metadata.",
                segment.node_id
            ));
        }
        PromptTarget::DiagnosticsOnly => {
            return Err(format!(
                "Prompt segment `{}` cannot lock diagnostics-only content.",
                segment.node_id
            ));
        }
    };
    let kind = match provenance.kind {
        PromptBaselineKind::FixedInstruction => return Ok(()),
        PromptBaselineKind::MemoryProfile => LockedSourceKind::MemoryProfile,
        PromptBaselineKind::AgentsMd => LockedSourceKind::AgentsMd,
        PromptBaselineKind::AgentPersona => LockedSourceKind::AgentPersona,
        PromptBaselineKind::OutputStyle => LockedSourceKind::OutputStyle,
        PromptBaselineKind::PromptSkillInjection => LockedSourceKind::PromptSkillInjection,
        PromptBaselineKind::PluginInstructions => LockedSourceKind::PluginInstructions,
        PromptBaselineKind::BaselineHooks => LockedSourceKind::BaselineHooks,
    };
    output.push(LockedBaselineSegmentInput {
        kind,
        node_id: segment.node_id.clone(),
        target,
        source_label: Some(segment.source_label.clone()),
        source_id: Some(provenance.source_id.clone()),
        content: segment.body.clone(),
    });
    Ok(())
}

/// Appends one rendered hook block only when the hook producer declared stable baseline provenance.
/// 仅当 Hook 生产者声明稳定基线来源时才追加渲染后的 Hook 块。
fn push_locked_hook_segment(
    output: &mut Vec<LockedBaselineSegmentInput>,
    block: &SystemPromptHookBlock,
) -> Result<(), String> {
    let Some(provenance) = block.baseline_provenance.as_ref() else {
        return Ok(());
    };
    let Some(content) = render_system_prompt_hook_block(block) else {
        return Ok(());
    };
    let kind = match provenance.kind {
        SystemPromptHookBaselineKind::MemoryProfile => LockedSourceKind::MemoryProfile,
        SystemPromptHookBaselineKind::PluginInstructions => LockedSourceKind::PluginInstructions,
        SystemPromptHookBaselineKind::BaselineHooks => LockedSourceKind::BaselineHooks,
    };
    output.push(LockedBaselineSegmentInput {
        kind,
        node_id: provenance.node_id.clone(),
        target: LockedPromptTarget::System,
        source_label: block.source.clone(),
        source_id: Some(provenance.source_id.clone()),
        content,
    });
    Ok(())
}

/// Appends request-scoped hook blocks into the system prompt layer.
/// 将请求级 hook 块追加到系统提示词层。
fn append_system_prompt_hook_region(
    provider_layers: &mut CoreProviderPromptLayers,
    blocks: &[SystemPromptHookBlock],
) {
    let region = render_system_prompt_hook_region(blocks);
    if region.is_empty() {
        return;
    }
    if provider_layers.system.trim().is_empty() {
        provider_layers.system = region;
    } else {
        provider_layers.system = format!("{}\n\n{}", provider_layers.system.trim_end(), region);
    }
}

/// Renders request-scoped hook blocks inside one stable system prompt region.
/// 在稳定系统提示词区域内渲染请求级 hook 块。
fn render_system_prompt_hook_region(blocks: &[SystemPromptHookBlock]) -> String {
    let rendered_blocks = blocks
        .iter()
        .filter_map(render_system_prompt_hook_block)
        .collect::<Vec<_>>();
    if rendered_blocks.is_empty() {
        String::new()
    } else {
        format!(
            "<system_prompt_hooks>\n{}\n</system_prompt_hooks>",
            rendered_blocks.join("\n\n")
        )
    }
}

/// Renders one sanitized hook block for provider-visible system prompt text.
/// 将单个清洗后的 hook 块渲染为 Provider 可见系统提示词文本。
fn render_system_prompt_hook_block(block: &SystemPromptHookBlock) -> Option<String> {
    let tag = normalize_system_prompt_hook_tag(&block.tag)?;
    let content = block.content.trim();
    if content.is_empty() {
        return None;
    }
    let mut lines = Vec::new();
    let description = block.description.trim();
    if !description.is_empty() {
        lines.push(description.to_string());
    }
    lines.push(format!("Phase: {}", block.phase.as_str()));
    if let Some(source) = block
        .source
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        lines.push(format!("Source: {source}"));
    }
    lines.push(String::new());
    lines.push(content.to_string());
    Some(format!("<{tag}>\n{}\n</{tag}>", lines.join("\n")))
}

/// Normalizes a hook tag so a malformed plugin tag cannot break the surrounding region.
/// 规范化 hook 标签，避免格式错误的插件标签破坏外围区域。
fn normalize_system_prompt_hook_tag(tag: &str) -> Option<String> {
    let normalized = tag
        .trim()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '-')
        .collect::<String>();
    (!normalized.is_empty()).then_some(normalized)
}

/// Loads one workspace-level project rules snapshot when `AGENTS.md` exists.
/// 当 `AGENTS.md` 存在时加载一份工作区级项目规则快照。
///
/// # Parameters
/// # 参数
/// - `workspace_path`: Persisted workspace path that may own the default `AGENTS.md` file.
/// - `workspace_path`：可能拥有默认 `AGENTS.md` 文件的持久化工作区路径。
///
/// # Returns
/// # 返回值
/// Returns `None` for a confirmed absent or blank rules file and preserves metadata or content failures.
/// 对确认缺失或空白的规则文件返回 `None`，并保留元数据或内容读取失败。
fn load_workspace_project_rules_snapshot(
    workspace_path: &str,
) -> Result<Option<ProjectRulesSnapshot>, String> {
    let Some(workspace) = resolve_workspace_directory(workspace_path)? else {
        return Ok(None);
    };
    // rules_path is the exact default source selected after workspace validation.
    // rules_path 是工作区验证后选择的精确默认来源。
    let rules_path = workspace.join(DEFAULT_PROJECT_RULES_FILE_NAME);
    // cache provides metadata-aware reuse without changing missing-file semantics.
    // cache 提供元数据感知复用，同时不改变文件缺失语义。
    let cache = PROJECT_RULES_FILE_CACHE.get_or_init(|| Mutex::new(ProjectRulesFileCache::new()));
    let mut cache = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache.load_file(&rules_path).map_err(|error| {
        format!(
            "Failed to load workspace project rules from `{}`: {error}",
            rules_path.display()
        )
    })
}

/// Invalidate the cached workspace-level project rules snapshot.
/// 使缓存中的工作区级项目规则快照失效。
///
/// # Parameters
/// # 参数
/// - `workspace_path`: Persisted workspace directory that owns the default rules file.
/// - `workspace_path`：拥有默认规则文件的持久化工作区目录。
///
/// # Returns
/// # 返回值
/// Returns success after invalidation or intentional absence, and preserves workspace metadata failures.
/// 完成失效处理或来源有意缺失时返回成功，并保留工作区元数据失败。
pub fn invalidate_workspace_project_rules_prompt_cache(workspace_path: &str) -> Result<(), String> {
    let Some(workspace) = resolve_workspace_directory(workspace_path)? else {
        return Ok(());
    };
    let cache = PROJECT_RULES_FILE_CACHE.get_or_init(|| Mutex::new(ProjectRulesFileCache::new()));
    let mut cache = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache.invalidate_workspace_default(workspace);
    Ok(())
}

/// Resolves and validates the workspace directory used by project-owned prompt sources.
/// 解析并验证项目自有提示来源使用的工作区目录。
///
/// # Parameters
/// # 参数
/// - `workspace_path`: Persisted workspace path supplied by execution preparation.
/// - `workspace_path`：执行准备提供的持久化工作区路径。
///
/// # Returns
/// # 返回值
/// Returns `None` only for an empty, confirmed missing, or non-directory path, and preserves metadata failures.
/// 仅对空路径、已确认缺失路径或非目录路径返回 `None`，并保留元数据失败。
fn resolve_workspace_directory(workspace_path: &str) -> Result<Option<&Path>, String> {
    if workspace_path.trim().is_empty() {
        return Ok(None);
    }
    // workspace is the exact persisted path whose metadata determines prompt-source eligibility.
    // workspace 是其元数据决定提示来源资格的精确持久化路径。
    let workspace = Path::new(workspace_path);
    // metadata distinguishes a confirmed missing workspace from an inaccessible one.
    // metadata 区分已确认缺失的工作区与不可访问工作区。
    let metadata = match fs::metadata(workspace) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "Failed to inspect workspace prompt source directory `{}`: {error}",
                workspace.display()
            ));
        }
    };
    if !metadata.is_dir() {
        return Ok(None);
    }
    Ok(Some(workspace))
}

/// Builds legacy transport text only when successful composition yields no structured layers.
/// 仅在成功组合未生成任何结构化提示层时构建旧传输文本。
///
/// # Parameters
/// # 参数
/// - `preparation`: Validated execution preparation whose workspace selects the base prompt layer.
/// - `preparation`：其工作区决定基础提示词层级的已验证执行准备。
///
/// # Returns
/// # 返回值
/// Returns rendered legacy transport text, or the exact provider prompt resource failure.
/// 返回渲染后的旧传输文本，或精确的 Provider 提示词资源失败。
fn build_legacy_runtime_system_preamble(
    preparation: &ExecutionPreparation,
) -> Result<String, String> {
    // provider_prompt comes from the same unique resource contract as structured composition.
    // provider_prompt 来自与结构化组合相同的唯一资源契约。
    let provider_prompt =
        load_effective_system_prompt(resolve_workspace_directory(&preparation.workspace_path)?)?
            .text;
    // The legacy fallback is kept stable; workspace, mode, and dynamic hooks are User events.
    // 旧版回退文本保持稳定；工作区、模式和动态 Hook 均作为 User 事件发送。
    Ok([provider_prompt, HOST_MESSAGE_GUIDANCE.to_string()].join("\n\n"))
}

/// Keeps the main provider tool loop unlimited regardless of legacy agent or prompt-purpose settings.
/// 无论旧版智能体或提示用途设置如何，均保持主 Provider 工具循环无限。
pub(crate) fn resolve_runtime_max_turns(
    _active_agent: Option<&RuntimeAgentDefinition>,
    _prompt_purpose: ExecutionPromptPurpose,
) -> Option<usize> {
    None
}

#[cfg(test)]
mod tests {
    //! Provider runtime context tests.
    //! Provider 运行时上下文测试。

    use super::*;
    use crate::agent_runtime::execution_models::{
        ExecutionAgentPromptSnapshot, ExecutionSubagentPromptContext,
    };
    use crate::agent_runtime::provider_prompt_loader::built_in_system_prompt;
    use crate::context_budget::ContextBudgetSnapshot;
    use crate::model::SessionId;
    use crate::provider_config::{
        PersistedPromptConfiguration, PersistedPromptPersonalitySelection, ReasoningEffortLevel,
        project_system_prompt_file_path, save_system_prompt_file,
    };
    use std::fs;
    use std::path::PathBuf;
    use vulcan_context::{ContextAssembly, ContextBaseline};

    /// Builds one typed runtime host candidate for append-only synchronization tests.
    /// 为追加式同步测试构建一条强类型运行时候选。
    fn host_candidate(
        source_id: &str,
        body: &str,
        occurrence_id: Option<&str>,
        event_id: Option<&str>,
        expires_source: bool,
    ) -> RuntimeHostEventCandidate {
        RuntimeHostEventCandidate {
            source_id: source_id.to_string(),
            source: String::from("test-host"),
            target: String::from("model"),
            task_id: None,
            scope: String::from("request"),
            body: body.to_string(),
            occurrence_id: occurrence_id.map(str::to_string),
            content_hash: None,
            event_id: event_id.map(str::to_string),
            kind: if expires_source {
                HostMessageKind::ContextExpired
            } else {
                HostMessageKind::ContextUpdate
            },
            expires_source,
        }
    }

    /// Verifies stable source deduplication, append-only ordering, and reappearance identity.
    /// 验证稳定来源去重、追加式顺序以及重新出现时的新身份。
    #[test]
    fn runtime_host_events_preserve_prefix_and_new_occurrence_after_expiry() {
        let mut snapshot = CoreConversationSnapshot {
            messages: vec![CoreConversationMessage {
                id: String::from("user-1"),
                payload: CoreConversationPayload::UserText {
                    text: String::from("request"),
                    attachments: Vec::new(),
                },
            }],
            ..Default::default()
        };
        synchronize_runtime_host_events(
            &mut snapshot,
            &[host_candidate("env", "one", Some("occ-1"), None, false)],
            Some("user-1"),
        )
        .expect("first host event should synchronize");
        let first_message_id = snapshot.messages[0].id.clone();
        assert_eq!(first_message_id, "runtime-host:occ-1");

        synchronize_runtime_host_events(
            &mut snapshot,
            &[host_candidate("env", "one", Some("occ-1"), None, false)],
            Some("user-1"),
        )
        .expect("same host body should deduplicate");
        assert_eq!(snapshot.messages.len(), 2);

        synchronize_runtime_host_events(
            &mut snapshot,
            &[host_candidate(
                "env",
                "expired",
                Some("occ-1"),
                Some("expiry-1"),
                true,
            )],
            Some("user-1"),
        )
        .expect("expiry marker should append before the unsent user");
        synchronize_runtime_host_events(
            &mut snapshot,
            &[host_candidate("env", "one", Some("occ-2"), None, false)],
            Some("user-1"),
        )
        .expect("reappeared body should receive a new occurrence");
        assert_eq!(snapshot.messages.len(), 4);
        assert_eq!(snapshot.messages[0].id, first_message_id);
        assert_ne!(snapshot.messages[0].id, snapshot.messages[2].id);

        snapshot.messages.push(CoreConversationMessage {
            id: String::from("assistant-1"),
            payload: CoreConversationPayload::Assistant {
                text: String::from("answer"),
                reasoning: String::new(),
                provider_message_id: None,
                provider_message_state: None,
                usage: None,
                tool_calls: Vec::new(),
            },
        });
        synchronize_runtime_host_events(
            &mut snapshot,
            &[host_candidate("env", "later", Some("occ-3"), None, false)],
            None,
        )
        .expect("later host event should append after the sent turn");
        assert_eq!(
            snapshot.messages.last().map(|message| message.id.as_str()),
            Some("runtime-host:occ-3")
        );
        assert_eq!(snapshot.messages[0].id, first_message_id);
    }

    /// Verifies equal bodies retain distinct producer occurrences and replays never revive expired instructions.
    /// 验证相同正文保留不同生产者发生点，且重放不会重新激活失效指令。
    #[test]
    fn runtime_host_events_distinguish_occurrences_and_replay_expiry() {
        // The expiry references the original occurrence and content hash, but owns a separate event and body.
        // 失效标记引用原发生点与内容哈希，但具有独立事件及正文。
        let mut first = host_candidate("ttl", "remember", Some("occ-1"), None, false);
        first.content_hash = Some(String::from("original-hash"));
        let mut expiry = host_candidate("ttl", "expired", Some("occ-1"), Some("expiry-1"), true);
        expiry.content_hash = first.content_hash.clone();
        let second = host_candidate("ttl", "remember", Some("occ-2"), None, false);
        let mut snapshot = CoreConversationSnapshot::default();
        synchronize_runtime_host_events(
            &mut snapshot,
            &[first.clone(), second.clone(), expiry.clone()],
            None,
        )
        .expect("distinct updates and expiration should coexist");
        let before = serde_json::to_value(&snapshot).expect("snapshot serializes");
        synchronize_runtime_host_events(&mut snapshot, &[expiry, first, second], None)
            .expect("out-of-order retries should reuse exact events");
        assert_eq!(serde_json::to_value(&snapshot).unwrap(), before);
        assert_eq!(snapshot.messages.len(), 3);
        assert_eq!(snapshot.messages[2].id, "runtime-host:expiry-1");
    }

    /// Verifies source snapshots coalesce current values while later changes append without rewriting history.
    /// 验证来源快照合并相同当前值，后续变化仅追加而不改写历史。
    #[test]
    fn runtime_host_events_coalesce_only_unidentified_current_values() {
        // The producer lacks occurrence IDs, so synchronization allocates each changed value once.
        // 生产者不提供发生点 ID，因此同步为每次变化分配一次身份。
        let first = host_candidate("environment", "one", None, None, false);
        let second = host_candidate("environment", "two", None, None, false);
        let mut snapshot = CoreConversationSnapshot::default();
        synchronize_runtime_host_events(
            &mut snapshot,
            &[first.clone(), first.clone(), second.clone(), second],
            None,
        )
        .expect("snapshot values should synchronize");
        let original = snapshot.messages.clone();
        synchronize_runtime_host_events(&mut snapshot, &[first], None)
            .expect("returning value should append a new occurrence");
        assert_eq!(snapshot.messages.len(), 3);
        assert_eq!(&snapshot.messages[..2], original.as_slice());
        assert_ne!(snapshot.messages[0].id, snapshot.messages[2].id);
    }

    /// Verifies failed replay validation publishes no partial batch, even when the producer hash is unchanged.
    /// 验证重放校验失败不发布部分批次，即使生产者哈希保持不变。
    #[test]
    fn runtime_host_events_reject_conflicts_atomically() {
        // Exact identity owns both rendered body and routing, independently from the supplied content hash.
        // 精确身份同时拥有渲染正文与路由，不依赖传入内容哈希。
        let mut original = host_candidate("ttl", "one", Some("occ-1"), None, false);
        original.content_hash = Some(String::from("hash"));
        let mut snapshot = CoreConversationSnapshot::default();
        synchronize_runtime_host_events(&mut snapshot, &[original.clone()], None).unwrap();
        let before = serde_json::to_value(&snapshot).unwrap();
        let mut body_conflict = original.clone();
        body_conflict.body = String::from("changed with same hash");
        let mut routing_conflict = original.clone();
        routing_conflict.target = String::from("another-agent");
        let mut event_conflict = original;
        event_conflict.event_id = Some(String::from("second-update-id"));
        for conflict in [body_conflict, routing_conflict, event_conflict] {
            let staged = host_candidate("other", "valid", Some("other-occ"), None, false);
            assert!(
                synchronize_runtime_host_events(&mut snapshot, &[staged, conflict], None).is_err()
            );
            assert_eq!(serde_json::to_value(&snapshot).unwrap(), before);
        }
    }

    /// Verifies unknown or missing owned metadata and a missing insertion boundary fail explicitly.
    /// 验证未知或缺失的所属元数据及缺失插入边界明确失败。
    #[test]
    fn runtime_host_events_reject_unknown_projection_metadata() {
        // Unrelated projection records are outside this producer's schema ownership.
        // 无关投影记录不属于本生产者的 Schema 归属。
        let candidate = host_candidate("ttl", "one", Some("occ-1"), None, false);
        let mut snapshot = CoreConversationSnapshot::default();
        synchronize_runtime_host_events(&mut snapshot, std::slice::from_ref(&candidate), None)
            .unwrap();
        let mut unrelated = snapshot.records[0].clone();
        unrelated.id = String::from("another-producer");
        unrelated.metadata = Some(serde_json::json!({"version": 99}));
        snapshot.records.push(unrelated);
        synchronize_runtime_host_events(&mut snapshot, std::slice::from_ref(&candidate), None)
            .unwrap();
        assert!(synchronize_runtime_host_events(&mut snapshot, &[], Some("missing-user")).is_err());
        snapshot.records[0].metadata.as_mut().unwrap()["version"] = serde_json::json!(99);
        let before = serde_json::to_value(&snapshot).unwrap();
        assert!(
            synchronize_runtime_host_events(&mut snapshot, std::slice::from_ref(&candidate), None)
                .is_err()
        );
        assert_eq!(serde_json::to_value(&snapshot).unwrap(), before);
        snapshot.records[0].metadata = None;
        assert!(synchronize_runtime_host_events(&mut snapshot, &[candidate], None).is_err());
    }

    /// Verifies host identity metadata survives the real record compactor and both snapshot bridges.
    /// 验证宿主身份元数据经过真实记录压缩器和双向快照桥后仍完整保留。
    #[test]
    fn runtime_host_events_survive_record_compaction_and_replay() {
        use crate::agent_runtime::provider_request_bridge_conversation::{
            build_core_conversation_snapshot, core_snapshot_to_session_conversation_state,
        };

        // The ledger includes an expired occurrence and a newer active occurrence of the same source.
        // 账本同时包含同一来源的过期发生点和更新的有效发生点。
        let first = host_candidate("ttl", "remember", Some("occ-1"), None, false);
        let expiry = host_candidate("ttl", "expired", Some("occ-1"), Some("expiry-1"), true);
        let second = host_candidate("ttl", "new memory", Some("occ-2"), None, false);
        let candidates = vec![first, expiry, second];
        let mut snapshot = CoreConversationSnapshot::default();
        synchronize_runtime_host_events(&mut snapshot, &candidates, None).expect("initial events");
        // The existing bridge classifies ProjectionOnly records as Explore; establish that canonical baseline first.
        // 既有桥把 ProjectionOnly 记录归类为 Explore；先建立该规范基线。
        snapshot = build_core_conversation_snapshot(
            &core_snapshot_to_session_conversation_state(&snapshot)
                .expect("initial session bridge"),
        )
        .expect("initial core bridge");
        let before = serde_json::to_value(&snapshot).expect("snapshot serializes");

        for _ in 0..2 {
            // Child compaction converts snapshots, compacts records, and restores the core state.
            // 子代理压缩先转换快照，再压缩记录，最后还原核心状态。
            let mut state =
                core_snapshot_to_session_conversation_state(&snapshot).expect("to session");
            vulcan_context_compaction::compact_conversation_records_after_summary(
                &mut state,
                "summary-anchor",
                vulcan_context::ContextCompactionPolicy::default(),
                |value| Some(value.clone()),
            );
            snapshot = build_core_conversation_snapshot(&state).expect("to core");
            synchronize_runtime_host_events(&mut snapshot, &candidates, None)
                .expect("compacted host events must remain replayable");
            assert_eq!(serde_json::to_value(&snapshot).unwrap(), before);
        }

        // Preserving the ledger must not weaken route-conflict validation after compaction.
        // 保留账本不能削弱压缩后的路由冲突校验。
        let mut conflict = candidates[2].clone();
        conflict.target = String::from("different-agent");
        assert!(synchronize_runtime_host_events(&mut snapshot, &[conflict], None).is_err());
        assert_eq!(serde_json::to_value(&snapshot).unwrap(), before);
    }

    /// Verifies real compaction excludes expired instructions, backfills active ones, and replays archived identities exactly.
    /// 验证真实压缩排除失效指令、回填有效指令，并精确重放归档身份。
    #[test]
    fn runtime_host_compaction_filters_backfills_and_replays_without_resurrection() {
        use crate::agent_runtime::provider_request_bridge_conversation::{
            build_core_conversation_snapshot, core_snapshot_to_session_conversation_state,
        };
        use vulcan_context::runtime_host::prepare_runtime_host_compaction;
        use vulcan_context::{SessionConversationMessage, SessionConversationPayload};
        use vulcan_context_compaction::{
            CompactionPlanInput, CompactionResult, plan_context_compaction,
        };

        // Updates precede the summarized rounds; old expiry arrives after a newer occurrence at the retained tail.
        // 更新位于摘要轮次之前；旧失效事件晚于新发生点到达保留尾段。
        let old = host_candidate("ttl", "old directive", Some("old"), None, false);
        let current = host_candidate("ttl", "current directive", Some("current"), None, false);
        let permanent = host_candidate(
            "persistent",
            "exact active instruction",
            Some("persistent"),
            None,
            false,
        );
        let short = host_candidate("short", "expired directive", Some("short"), None, false);
        let late_old_expiry =
            host_candidate("ttl", "old expired", Some("old"), Some("expire-old"), true);
        let short_expiry = host_candidate(
            "short",
            "short expired",
            Some("short"),
            Some("expire-short"),
            true,
        );
        let mut snapshot = CoreConversationSnapshot::default();
        synchronize_runtime_host_events(
            &mut snapshot,
            &[
                old.clone(),
                current.clone(),
                permanent.clone(),
                short.clone(),
            ],
            None,
        )
        .unwrap();
        let mut history = core_snapshot_to_session_conversation_state(&snapshot).unwrap();
        for round in 0..8 {
            vulcan_context::append_assistant_text_message(
                &mut history,
                format!("assistant-{round}"),
                "completed work",
            )
            .unwrap();
        }
        // A real tool pair and user-authored lookalike are unrelated to host-ledger filtering.
        // 真实工具配对和用户书写的相似标签不属于宿主账本筛选范围。
        if let SessionConversationPayload::Assistant { tool_calls, .. } =
            &mut history.messages.last_mut().unwrap().payload
        {
            tool_calls.push(vulcan_context::SessionConversationToolCall {
                id: String::from("local-tool"),
                tool_call_id: Some(String::from("call-1")),
                tool_name: String::from("inspect"),
                arguments: serde_json::json!({}),
                argument_parse_error: None,
            });
        }
        history.messages.push(SessionConversationMessage {
            id: String::from("native-result"),
            payload: SessionConversationPayload::ToolResult {
                tool_call_id: Some(String::from("call-1")),
                is_error: false,
                text: String::from("exact native result"),
            },
        });
        vulcan_context::append_user_text_message(
            &mut history,
            "human-lookalike",
            "<vulcan_context_update>human</vulcan_context_update>",
            Vec::new(),
        )
        .unwrap();
        snapshot = build_core_conversation_snapshot(&history).unwrap();
        synchronize_runtime_host_events(
            &mut snapshot,
            &[late_old_expiry.clone(), short_expiry.clone()],
            None,
        )
        .unwrap();
        let original = serde_json::to_value(&snapshot).unwrap();
        let mut source = prepare_runtime_host_compaction(
            &core_snapshot_to_session_conversation_state(&snapshot).unwrap(),
        )
        .unwrap();
        for removed in ["old", "short", "expire-old", "expire-short"] {
            assert!(
                !source
                    .messages
                    .iter()
                    .any(|message| message.id == format!("runtime-host:{removed}"))
            );
        }
        assert_eq!(
            serde_json::to_value(&snapshot).unwrap(),
            original,
            "planning must not edit ordinary history"
        );
        let messages =
            vulcan_context::conversation_state_to_compaction_source_messages(&source).unwrap();
        let plan = plan_context_compaction(CompactionPlanInput {
            session_label: "host-compaction",
            messages: &messages,
            all_messages: &messages,
            context_start_message_id: None,
            source_until_message_id: None,
            policy: vulcan_context::ContextCompactionPolicy {
                tail_turns: 2,
                preserve_recent_tokens: 5_000,
                ..Default::default()
            },
        })
        .expect("real tail plan");
        assert!(
            !plan
                .tail_selection
                .retained_message_ids
                .iter()
                .any(|id| id.starts_with("runtime-host:")),
            "active hosts start in the summarized prefix"
        );
        let result = CompactionResult::from_model_summary(&plan, "work summary");
        vulcan_context_compaction::compact_conversation_with_compaction_result(
            &mut source,
            "summary",
            &result,
        )
        .unwrap();
        vulcan_context_compaction::compact_conversation_records_after_summary(
            &mut source,
            "summary",
            result.policy,
            |value| Some(value.clone()),
        );
        for retained in [
            "runtime-host:current",
            "runtime-host:persistent",
            "native-result",
            "human-lookalike",
        ] {
            let expected = snapshot
                .messages
                .iter()
                .find(|message| message.id == retained)
                .unwrap();
            let actual = build_core_conversation_snapshot(&source)
                .unwrap()
                .messages
                .into_iter()
                .find(|message| message.id == retained)
                .unwrap();
            assert_eq!(
                serde_json::to_value(actual).unwrap(),
                serde_json::to_value(expected).unwrap()
            );
        }
        snapshot = build_core_conversation_snapshot(&source).unwrap();
        let after = serde_json::to_value(&snapshot).unwrap();
        synchronize_runtime_host_events(
            &mut snapshot,
            &[
                short_expiry,
                permanent,
                late_old_expiry,
                old.clone(),
                short,
                current.clone(),
            ],
            None,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&snapshot).unwrap(),
            after,
            "archived replay must not resurrect expired bodies"
        );
        // Archived events still bind the original recipient and exact envelope bytes.
        // 归档事件仍绑定原收件人和精确信封字节。
        let mut conflict = old;
        conflict.target = String::from("wrong-recipient");
        assert!(synchronize_runtime_host_events(&mut snapshot, &[conflict], None).is_err());
        assert_eq!(serde_json::to_value(&snapshot).unwrap(), after);
        let prefix = snapshot.messages.clone();
        let expiry = host_candidate(
            "ttl",
            "current expired",
            Some("current"),
            Some("expire-current"),
            true,
        );
        synchronize_runtime_host_events(&mut snapshot, &[expiry], None).unwrap();
        assert_eq!(
            serde_json::to_value(&snapshot.messages[..prefix.len()]).unwrap(),
            serde_json::to_value(&prefix).unwrap()
        );
        let next = prepare_runtime_host_compaction(
            &core_snapshot_to_session_conversation_state(&snapshot).unwrap(),
        )
        .unwrap();
        assert!(!next.messages.iter().any(
            |message| message.id == "runtime-host:current" || message.id == "runtime-host:old"
        ));
        snapshot = build_core_conversation_snapshot(&next).unwrap();
        let fresh = host_candidate("ttl", &current.body, Some("fresh"), None, false);
        synchronize_runtime_host_events(&mut snapshot, &[fresh], None).unwrap();
        assert_eq!(snapshot.messages.last().unwrap().id, "runtime-host:fresh");
    }

    /// Verifies a real injection-slot replacement reaches host history with a new identity and expires exactly once.
    /// 验证真实注入槽位更新以新身份进入宿主历史，并精确失效一次。
    #[test]
    fn runtime_host_real_injection_replacement_survives_compaction() {
        use crate::agent_runtime::context_injection_runtime::{
            core_provider_context_injection_from_state_for_turn,
            core_provider_context_injection_from_state_for_turn_draining_expiry_events,
        };
        use crate::agent_runtime::provider_request_bridge_conversation::{
            build_core_conversation_snapshot, core_snapshot_to_session_conversation_state,
        };
        use vulcan_context_injection::{ContextInjectionState, InjectionInsert, InjectionKind};

        // Both writes address one producer slot; only a changed body allocates the replacement occurrence.
        // 两次写入指向同一生产者槽位；正文变化才分配替换发生点。
        let mut injection = ContextInjectionState::default();
        let memory = vulcan_vmm::VmmSessionMemoryState::default();
        let mut snapshot = CoreConversationSnapshot::default();
        for (body, source) in [
            ("original memory", "source-a"),
            ("replacement memory", "source-a"),
            ("replacement memory", "source-b"),
        ] {
            let prefix = serde_json::to_value(&snapshot.messages).unwrap();
            let prefix_len = snapshot.messages.len();
            let mut insert = InjectionInsert::single(
                InjectionKind::Transient,
                "memory",
                "description",
                body,
                Some(1),
            );
            insert.items[0].dedupe_key = Some(String::from("slot-1"));
            insert.source_display = Some(source.to_string());
            injection.apply_insert_for_turn(insert, "1", Some("business-1"));
            let overlay = core_provider_context_injection_from_state_for_turn(
                &injection,
                &memory,
                Some("business-1"),
                false,
            )
            .unwrap();
            synchronize_runtime_host_events(
                &mut snapshot,
                &runtime_context_host_event_candidates(&overlay),
                None,
            )
            .unwrap();
            assert_eq!(snapshot.messages.len(), prefix_len + 1);
            assert_eq!(
                serde_json::to_value(&snapshot.messages[..prefix_len]).unwrap(),
                prefix
            );
        }
        injection.consume_turn("business-1", "2");
        let overlay = core_provider_context_injection_from_state_for_turn_draining_expiry_events(
            &mut injection,
            &memory,
            Some("business-2"),
            false,
        )
        .unwrap();
        let expiration = runtime_context_host_event_candidates(&overlay);
        synchronize_runtime_host_events(&mut snapshot, &expiration, None).unwrap();
        let compacted = vulcan_context::runtime_host::prepare_runtime_host_compaction(
            &core_snapshot_to_session_conversation_state(&snapshot).unwrap(),
        )
        .unwrap();
        assert!(
            compacted.messages.is_empty(),
            "neither the replaced nor expired version remains active"
        );
        snapshot = build_core_conversation_snapshot(&compacted).unwrap();
        let before = serde_json::to_value(&snapshot).unwrap();
        synchronize_runtime_host_events(&mut snapshot, &expiration, None).unwrap();
        assert_eq!(serde_json::to_value(&snapshot).unwrap(), before);
        assert!(
            core_provider_context_injection_from_state_for_turn_draining_expiry_events(
                &mut injection,
                &memory,
                Some("business-2"),
                false
            )
            .is_none()
        );
    }

    /// Verifies an actual persistent injection deletion becomes a routed withdrawal without deleting the sent prefix.
    /// 验证真实长期注入删除转为按原路由发送的撤下消息，不删除已经发送的前缀。
    #[test]
    fn runtime_host_real_injection_removal_appends_once_and_compacts() {
        use crate::agent_runtime::context_injection_runtime::core_provider_context_injection_from_state_for_turn;
        use crate::agent_runtime::provider_request_bridge_conversation::{
            build_core_conversation_snapshot, core_snapshot_to_session_conversation_state,
        };
        use vulcan_context_injection::{ContextInjectionState, InjectionInsert, InjectionKind};
        let mut injection = ContextInjectionState::default();
        injection.apply_insert(
            InjectionInsert::single(
                InjectionKind::Persistent,
                "memory",
                "description",
                "persistent instruction",
                None,
            ),
            "1",
        );
        let overlay = core_provider_context_injection_from_state_for_turn(
            &injection,
            &Default::default(),
            None,
            false,
        )
        .unwrap();
        let mut snapshot = CoreConversationSnapshot::default();
        synchronize_runtime_host_request_context(
            &mut snapshot,
            &runtime_context_host_event_candidates(&overlay),
            None,
            ExecutionPromptPurpose::UserTurn,
        )
        .unwrap();
        let prefix = serde_json::to_value(&snapshot.messages).unwrap();
        assert!(injection.remove_group(Some(InjectionKind::Persistent), "memory"));
        assert!(
            core_provider_context_injection_from_state_for_turn(
                &injection,
                &Default::default(),
                None,
                false
            )
            .is_none()
        );
        synchronize_runtime_host_request_context(
            &mut snapshot,
            &[],
            None,
            ExecutionPromptPurpose::UserTurn,
        )
        .unwrap();
        assert_eq!(snapshot.messages.len(), 2);
        assert_eq!(
            serde_json::to_value(&snapshot.messages[..1]).unwrap(),
            prefix
        );
        let next = vulcan_context::runtime_host::prepare_runtime_host_compaction(
            &core_snapshot_to_session_conversation_state(&snapshot).unwrap(),
        )
        .unwrap();
        assert!(next.messages.is_empty());
        snapshot = build_core_conversation_snapshot(&next).unwrap();
        let compacted = serde_json::to_value(&snapshot).unwrap();
        synchronize_runtime_host_request_context(
            &mut snapshot,
            &[],
            None,
            ExecutionPromptPurpose::UserTurn,
        )
        .unwrap();
        assert_eq!(serde_json::to_value(&snapshot).unwrap(), compacted);
    }

    /// Verifies complete inventories withdraw absent sources once while partial auxiliary requests preserve them.
    /// 验证完整清单只撤下缺失来源一次，而局部辅助请求保留这些来源。
    #[test]
    fn runtime_host_inventory_withdrawal_is_routed_atomic_and_replay_safe() {
        use crate::agent_runtime::provider_request_bridge_conversation::core_snapshot_to_session_conversation_state;
        use vulcan_context::runtime_host::read_runtime_host_ledger;
        let mut original = host_candidate("memory-slot", "instruction", Some("occ"), None, false);
        original.target = String::from("specific-model-target");
        original.task_id = Some(String::from("business-owner"));
        original.scope = String::from("session");
        let mut snapshot = CoreConversationSnapshot::default();
        synchronize_runtime_host_request_context(
            &mut snapshot,
            &[original.clone()],
            None,
            ExecutionPromptPurpose::UserTurn,
        )
        .unwrap();
        let initial = serde_json::to_value(&snapshot).unwrap();
        for purpose in [
            ExecutionPromptPurpose::QuestionExecution,
            ExecutionPromptPurpose::ContextCompaction,
            ExecutionPromptPurpose::ContextCompactionCachePreserving,
            ExecutionPromptPurpose::GitCommitSummary,
        ] {
            synchronize_runtime_host_request_context(&mut snapshot, &[], None, purpose).unwrap();
            assert_eq!(serde_json::to_value(&snapshot).unwrap(), initial);
        }
        synchronize_runtime_host_request_context(
            &mut snapshot,
            &[],
            None,
            ExecutionPromptPurpose::ExecutionRetry,
        )
        .unwrap();
        assert_eq!(snapshot.messages.len(), 2);
        let ledger = read_runtime_host_ledger(
            &core_snapshot_to_session_conversation_state(&snapshot).unwrap(),
        )
        .unwrap();
        let withdrawn = &ledger[1];
        assert!(withdrawn.expires_source);
        assert_eq!(withdrawn.occurrence_id, "occ");
        assert_eq!(withdrawn.envelope.target, original.target);
        assert_eq!(withdrawn.envelope.source, original.source);
        assert_eq!(withdrawn.envelope.task_id, original.task_id);
        assert_eq!(withdrawn.envelope.scope, original.scope);
        let after = serde_json::to_value(&snapshot).unwrap();
        synchronize_runtime_host_request_context(
            &mut snapshot,
            &[],
            None,
            ExecutionPromptPurpose::AgentTask,
        )
        .unwrap();
        assert_eq!(serde_json::to_value(&snapshot).unwrap(), after);
        // A stale complete snapshot cannot reactivate an already withdrawn occurrence.
        // 过时完整快照不能重新激活已撤下的发生点。
        assert!(
            synchronize_runtime_host_request_context(
                &mut snapshot,
                &[original],
                None,
                ExecutionPromptPurpose::UserTurn
            )
            .is_err()
        );
        assert_eq!(serde_json::to_value(&snapshot).unwrap(), after);
        let late_ttl = host_candidate(
            "memory-slot",
            "TTL also expired",
            Some("occ"),
            Some("late-ttl"),
            true,
        );
        synchronize_runtime_host_events(&mut snapshot, std::slice::from_ref(&late_ttl), None)
            .unwrap();
        let late = serde_json::to_value(&snapshot).unwrap();
        synchronize_runtime_host_events(&mut snapshot, &[late_ttl], None).unwrap();
        assert_eq!(serde_json::to_value(&snapshot).unwrap(), late);
        let fresh = host_candidate("memory-slot", "instruction", Some("new-occ"), None, false);
        synchronize_runtime_host_request_context(
            &mut snapshot,
            std::slice::from_ref(&fresh),
            None,
            ExecutionPromptPurpose::UserTurn,
        )
        .unwrap();
        let before_conflict = serde_json::to_value(&snapshot).unwrap();
        assert!(
            synchronize_runtime_host_request_context(
                &mut snapshot,
                &[fresh.clone(), fresh],
                None,
                ExecutionPromptPurpose::UserTurn
            )
            .is_err()
        );
        assert_eq!(serde_json::to_value(&snapshot).unwrap(), before_conflict);
    }

    /// Verifies the project-rules loader reads a non-empty workspace AGENTS.md file.
    /// 验证项目规则加载器会读取非空的工作区 AGENTS.md 文件。
    #[test]
    fn project_rules_loader_reads_workspace_agents_file() {
        let root = unique_temp_dir("vulcan-project-rules-read");
        fs::create_dir_all(&root).expect("create temp workspace");
        let rules_path = root.join(DEFAULT_PROJECT_RULES_FILE_NAME);
        fs::write(&rules_path, "Rule A\n").expect("write project rules");

        let snapshot = load_workspace_project_rules_snapshot(&root.display().to_string())
            .expect("project rules load should succeed")
            .expect("project rules should exist");

        assert_eq!(snapshot.text, "Rule A");
        assert_eq!(snapshot.source, rules_path.display().to_string());

        let _ = fs::remove_dir_all(root);
    }

    /// Verifies missing or empty project rules are not injected.
    /// 验证缺失或空白项目规则不会被注入。
    #[test]
    fn project_rules_loader_skips_missing_or_empty_agents_file() {
        let missing_root = unique_temp_dir("vulcan-project-rules-missing");
        fs::create_dir_all(&missing_root).expect("create missing workspace");
        assert!(
            load_workspace_project_rules_snapshot(&missing_root.display().to_string())
                .expect("missing project rules load should succeed")
                .is_none()
        );

        let empty_root = unique_temp_dir("vulcan-project-rules-empty");
        fs::create_dir_all(&empty_root).expect("create empty workspace");
        fs::write(empty_root.join(DEFAULT_PROJECT_RULES_FILE_NAME), " \n\t")
            .expect("write empty project rules");
        assert!(
            load_workspace_project_rules_snapshot(&empty_root.display().to_string())
                .expect("empty project rules load should succeed")
                .is_none()
        );

        let _ = fs::remove_dir_all(missing_root);
        let _ = fs::remove_dir_all(empty_root);
    }

    /// Verifies external watcher events can invalidate the shared project-rules cache.
    /// 验证外部监听事件可以使共享项目规则缓存失效。
    #[test]
    fn project_rules_loader_supports_explicit_cache_invalidation() {
        let root = unique_temp_dir("vulcan-project-rules-invalidate");
        fs::create_dir_all(&root).expect("create temp workspace");
        let rules_path = root.join(DEFAULT_PROJECT_RULES_FILE_NAME);
        fs::write(&rules_path, "Rule before\n").expect("write first rules");

        let first = load_workspace_project_rules_snapshot(&root.display().to_string())
            .expect("first project rules load should succeed")
            .expect("first project rules should exist");
        assert_eq!(first.text, "Rule before");

        fs::write(&rules_path, "Rule after\n").expect("write second rules");
        invalidate_workspace_project_rules_prompt_cache(&root.display().to_string())
            .expect("project rules cache invalidation should succeed");

        let second = load_workspace_project_rules_snapshot(&root.display().to_string())
            .expect("second project rules load should succeed")
            .expect("second project rules should exist");
        assert_eq!(second.text, "Rule after");

        let _ = fs::remove_dir_all(root);
    }

    /// Verifies unreadable project-rules content aborts prompt assembly instead of disappearing.
    /// 验证无法读取的项目规则内容会中止提示装配，而不是静默消失。
    ///
    /// # Parameters
    /// # 参数
    /// This test has no parameters.
    /// 此测试没有参数。
    ///
    /// # Returns
    /// # 返回值
    /// This test returns no value and fails if invalid UTF-8 rules are treated as absent.
    /// 此测试无返回值；若无效 UTF-8 规则被视为不存在则失败。
    #[test]
    fn runtime_prompt_layers_propagate_project_rules_read_failure() {
        // root isolates the invalid project-rules source from every other cache entry.
        // root 将无效项目规则来源与其他所有缓存条目隔离。
        let root = unique_temp_dir("vulcan-project-rules-invalid-utf8");
        fs::create_dir_all(&root).expect("create invalid-rules workspace");
        // rules_path is the exact default project-rules source selected by the runtime.
        // rules_path 是运行时选择的精确默认项目规则来源。
        let rules_path = root.join(DEFAULT_PROJECT_RULES_FILE_NAME);
        fs::write(&rules_path, [0xff, 0xfe, 0xfd]).expect("write invalid project rules");
        // preparation binds prompt assembly to the invalid workspace source.
        // preparation 将提示装配绑定到无效工作区来源。
        let preparation = build_test_preparation(
            root.display().to_string(),
            PersistedPromptConfiguration::default(),
        );

        // error must retain both the source category and exact file path.
        // error 必须同时保留来源类别与精确文件路径。
        let error = build_runtime_prompt_layers_for_agent(&preparation, None)
            .expect_err("invalid project rules should abort prompt assembly");

        assert!(error.contains("workspace project rules"));
        assert!(error.contains(&rules_path.display().to_string()));
        fs::remove_dir_all(root).expect("remove invalid-rules workspace");
    }

    /// Verifies a project system prompt override replaces the built-in base prompt in the system layer.
    /// 验证项目系统提示词覆盖会在系统层中替换内置基础提示词。
    #[test]
    fn runtime_prompt_layers_use_project_system_prompt_override() {
        let root = unique_temp_dir("vulcan-project-system-prompt");
        save_system_prompt_file(
            &project_system_prompt_file_path(&root),
            "Project base system prompt",
        )
        .expect("write project system prompt");

        let preparation = build_test_preparation(
            root.display().to_string(),
            PersistedPromptConfiguration::default(),
        );
        let layers = build_runtime_prompt_layers_for_agent(&preparation, None)
            .expect("runtime prompt layers should build");

        assert!(
            layers
                .provider_layers
                .system
                .contains("Project base system prompt")
        );
        assert!(
            !layers
                .provider_layers
                .system
                .contains(built_in_system_prompt())
        );
        assert_eq!(
            layers.fixed_instruction_layers.system,
            "Project base system prompt"
        );
        assert!(
            layers
                .fallback_text
                .starts_with("Project base system prompt")
        );
        assert!(
            !layers
                .dynamic_prompt_layers
                .system
                .contains("Project base system prompt")
        );
        assert!(
            layers
                .host_events
                .iter()
                .all(|event| event.body != "Project base system prompt")
        );

        let _ = fs::remove_dir_all(root);
    }

    /// Verifies the product capability guide stays attached when the base prompt is overridden.
    /// 验证基础提示词被覆盖时产品功能说明仍会附加。
    #[test]
    fn runtime_prompt_layers_attach_capability_guide_outside_editable_prompt() {
        let root = unique_temp_dir("vulcan-capability-guide");
        save_system_prompt_file(&project_system_prompt_file_path(&root), "Minimal base")
            .expect("write project system prompt");

        let preparation = build_test_preparation(
            root.display().to_string(),
            PersistedPromptConfiguration::default(),
        );
        let layers = build_runtime_prompt_layers_for_agent(&preparation, None)
            .expect("runtime prompt layers should build");

        assert!(
            layers
                .provider_layers
                .developer
                .contains(capability_guide_prompt())
        );
        assert!(
            !layers
                .provider_layers
                .system
                .contains(capability_guide_prompt())
        );
        assert_eq!(
            layers.fixed_instruction_layers.developer,
            capability_guide_prompt()
        );
        assert!(
            !layers
                .dynamic_prompt_layers
                .developer
                .contains(capability_guide_prompt())
        );
        assert!(
            layers
                .host_events
                .iter()
                .all(|event| event.body != capability_guide_prompt())
        );

        let _ = fs::remove_dir_all(root);
    }

    /// Verifies an unreadable project system prompt aborts prompt assembly with its exact path.
    /// 验证无法读取的项目系统提示词会以精确路径中止提示装配。
    #[test]
    fn runtime_prompt_layers_propagate_project_system_prompt_read_failure() {
        let root = unique_temp_dir("vulcan-project-system-prompt-invalid-utf8");
        let prompt_path = project_system_prompt_file_path(&root);
        fs::create_dir_all(
            prompt_path
                .parent()
                .expect("project system prompt should have a parent"),
        )
        .expect("create project system prompt directory");
        fs::write(&prompt_path, [0xff, 0xfe, 0xfd]).expect("write invalid project system prompt");
        let preparation = build_test_preparation(
            root.display().to_string(),
            PersistedPromptConfiguration::default(),
        );

        let error = build_runtime_prompt_layers_for_agent(&preparation, None)
            .expect_err("invalid project system prompt should abort prompt assembly");

        assert!(error.contains(&prompt_path.display().to_string()));
        fs::remove_dir_all(root).expect("remove invalid-prompt workspace");
    }

    /// Verifies persisted personality settings can disable personality injection.
    /// 验证持久化人格设置可以禁用人格注入。
    #[test]
    fn runtime_prompt_layers_respect_disabled_personality_setting() {
        let prompt_configuration = PersistedPromptConfiguration {
            personality: PersistedPromptPersonalitySelection::Disabled,
            ..PersistedPromptConfiguration::default()
        };
        let preparation = build_test_preparation(String::from("D:/missing"), prompt_configuration);
        let layers = build_runtime_prompt_layers_for_agent(&preparation, None)
            .expect("runtime prompt layers should build");

        assert_eq!(
            layers
                .provider_layers
                .system
                .matches("You optimize for team morale")
                .count(),
            0
        );
    }

    /// Verifies request-scoped agent prompt snapshots enter the shared runtime prompt composer.
    /// 验证请求级代理提示词快照会进入共享运行时提示词组合器。
    #[test]
    fn runtime_prompt_layers_use_request_scoped_agent_prompt_snapshot() {
        // A request-specific agent override is dynamic; changing it must preserve the stable top bytes.
        // 请求专属代理覆盖属于动态内容；改变它必须保留稳定顶部字节。
        let mut preparation = build_test_preparation(
            String::from("D:/missing"),
            PersistedPromptConfiguration::default(),
        );
        preparation.agent_prompt_snapshot = ExecutionAgentPromptSnapshot::fixed_parent_runtime(
            "Task-specific reviewer instructions.",
        );
        let first = build_runtime_prompt_layers_for_agent(&preparation, None)
            .expect("runtime prompt layers should build");
        assert!(
            !first
                .fallback_text
                .contains("Task-specific reviewer instructions.")
        );
        assert!(
            first
                .dynamic_prompt_layers
                .system
                .contains("Task-specific reviewer instructions.")
        );
        let mut snapshot = CoreConversationSnapshot::default();
        synchronize_runtime_host_events(&mut snapshot, &first.host_events, None)
            .expect("first dynamic request should synchronize");
        let prefix = snapshot.messages.clone();

        preparation.agent_prompt_snapshot =
            ExecutionAgentPromptSnapshot::fixed_parent_runtime("Updated reviewer instructions.");
        let second = build_runtime_prompt_layers_for_agent(&preparation, None)
            .expect("updated runtime prompt layers should build");
        assert_eq!(first.fallback_text, second.fallback_text);
        synchronize_runtime_host_events(&mut snapshot, &second.host_events, None)
            .expect("changed override should append");
        assert_eq!(&snapshot.messages[..prefix.len()], prefix.as_slice());
        assert_eq!(snapshot.messages.len(), prefix.len() + 1);
        assert!(matches!(&snapshot.messages.last().unwrap().payload,
                CoreConversationPayload::UserText { text, .. }
                    if text.contains("Updated reviewer instructions.")
                        && text.contains("<vulcan_context_update ")));
    }

    /// Verifies active session goals never enter the global provider prompt layers.
    /// 验证活动会话目标绝不会进入 Provider 全局提示词层。
    #[test]
    fn runtime_prompt_layers_exclude_session_goal_context() {
        let mut preparation = build_test_preparation(
            String::from("D:/missing"),
            PersistedPromptConfiguration::default(),
        );
        preparation.session_goal_context = Some(
            crate::agent_runtime::execution_models::ExecutionSessionGoalContext {
                goal_id: String::from("goal-cache-stable"),
                prompt_context: String::from("GOAL_CONTEXT_MUST_STAY_OUT_OF_PREFIX"),
            },
        );

        let layers = build_runtime_prompt_layers_for_agent(&preparation, None)
            .expect("runtime prompt layers should build");

        assert!(
            !layers
                .fallback_text
                .contains("GOAL_CONTEXT_MUST_STAY_OUT_OF_PREFIX")
        );
        assert!(
            !layers
                .provider_layers
                .runtime_context
                .contains("GOAL_CONTEXT_MUST_STAY_OUT_OF_PREFIX")
        );
    }

    /// Verifies child-agent prompt context enters the shared prompt composer directly.
    /// 验证子智能体提示词上下文会直接进入共享提示词组合器。
    #[test]
    fn runtime_prompt_layers_use_subagent_prompt_context() {
        let mut preparation = build_test_preparation(
            String::from("D:/missing"),
            PersistedPromptConfiguration::default(),
        );
        preparation.subagent_prompt_context = Some(ExecutionSubagentPromptContext {
            identity: String::from("reviewer"),
            delegated_task: String::from("Review code"),
            delegation: Some(String::from("Return findings to the parent.")),
            functional_system_prompt: None,
        });
        preparation.agent_prompt_snapshot =
            ExecutionAgentPromptSnapshot::fixed_parent_runtime("Inherited agent foundation");

        let layers = build_runtime_prompt_layers_for_agent(&preparation, None)
            .expect("runtime prompt layers should build");

        assert!(
            layers
                .provider_layers
                .developer
                .contains("Subagent identity: reviewer")
        );
        assert!(
            layers
                .provider_layers
                .developer
                .contains("Delegated task: Review code")
        );
        assert!(!layers.provider_layers.system.contains("Subagent identity"));
        assert!(
            layers
                .fixed_instruction_layers
                .system
                .ends_with("Inherited agent foundation")
        );
        assert!(
            !layers
                .dynamic_prompt_layers
                .system
                .contains("Inherited agent foundation")
        );
    }

    /// Verifies functional child-agent prompts use the functional subagent strategy.
    /// 验证功能型子智能体提示词会使用功能型子智能体策略。
    #[test]
    fn runtime_prompt_layers_use_functional_subagent_prompt() {
        let mut preparation = build_test_preparation(
            String::from("D:/missing"),
            PersistedPromptConfiguration::default(),
        );
        preparation.subagent_prompt_context = Some(ExecutionSubagentPromptContext {
            identity: String::from("custom-reviewer"),
            delegated_task: String::from("Review code"),
            delegation: Some(String::from("Return findings to the parent.")),
            functional_system_prompt: Some(String::from("You are a strict reviewer.")),
        });

        let layers = build_runtime_prompt_layers_for_agent(&preparation, None)
            .expect("runtime prompt layers should build");

        assert!(
            layers
                .provider_layers
                .system
                .contains("You are a strict reviewer.")
        );
        assert!(
            layers
                .provider_layers
                .developer
                .contains("Subagent identity: custom-reviewer")
        );
    }

    /// Verifies tool registration help reaches the runtime tool-guidance layer.
    /// 验证工具注册说明会进入运行时工具指导层。
    #[test]
    fn runtime_prompt_layers_include_registered_tool_help_text() {
        let mut preparation = build_test_preparation(
            String::from("D:/missing"),
            PersistedPromptConfiguration::default(),
        );
        preparation.tool_descriptors = vec![vulcan_tools_manager::VulcanToolDescriptor::new(
            String::from("custom_tool"),
            String::from("custom_tool"),
            String::from("Custom test tool"),
            Some(String::from("Use category 5 for project context.")),
            vulcan_tools_manager::VulcanToolOrigin::new(
                vulcan_tools_manager::VulcanToolSourceKind::Native,
                "test",
                String::from("custom_tool"),
            ),
        )];

        let layers = build_runtime_prompt_layers_for_agent(&preparation, None)
            .expect("runtime prompt layers should build");

        assert!(
            layers
                .provider_layers
                .tool_guidance
                .contains("Use category 5 for project context.")
        );
    }

    /// Verifies request-scoped system hook blocks are appended to the system layer.
    /// 验证请求级系统 hook 块会追加到系统层。
    #[test]
    fn runtime_prompt_layers_append_system_prompt_hook_region() {
        let mut preparation = build_test_preparation(
            String::from("D:/missing"),
            PersistedPromptConfiguration::default(),
        );
        preparation.system_prompt_hook_blocks = vec![SystemPromptHookBlock {
            tag: String::from("vmm_profile"),
            description: String::from("Profile details"),
            content: String::from("User prefers precise answers."),
            source: Some(String::from("test")),
            phase: vulcan_hooks::SystemPromptHookPhase::FirstSession,
            baseline_provenance: None,
        }];

        let layers = build_runtime_prompt_layers_for_agent(&preparation, None)
            .expect("runtime prompt layers should build");

        assert!(
            layers
                .provider_layers
                .system
                .contains("<system_prompt_hooks>")
        );
        assert!(layers.provider_layers.system.contains("<vmm_profile>"));
        assert!(
            layers
                .provider_layers
                .system
                .contains("User prefers precise answers.")
        );
        assert!(!layers.fallback_text.contains("<system_prompt_hooks>"));
    }

    /// Verifies every request-dynamic producer stays out of the fixed top and inside dynamic layers.
    /// 验证所有请求动态生产者都不进入固定顶部，而是位于动态层。
    ///
    /// Covers the Plan, environment, temporary-guidance (Hook) and request-scoped agent producers in
    /// one production composition, and pins that `provider_layers` is the only projection that
    /// carries them. A fixed-top regression therefore fails here instead of reaching a wire request.
    /// 在一次生产组合中覆盖 Plan、环境、临时指引（Hook）与请求级代理生产者，并固定
    /// `provider_layers` 是唯一携带它们的投影。固定顶部回归会在此失败，而不会进入真实请求。
    #[test]
    fn request_dynamic_producers_stay_out_of_fixed_top_layers() {
        // A workspace-free preparation isolates this test from project-owned prompt sources.
        // 无工作区的执行准备把本测试与项目自有提示来源隔离。
        let mut preparation =
            build_test_preparation(String::new(), PersistedPromptConfiguration::default());
        preparation.prompt_purpose = ExecutionPromptPurpose::UserTurn;
        preparation.agent_prompt_snapshot =
            crate::agent_runtime::execution_models::ExecutionAgentPromptSnapshot::fixed_parent_runtime(
                "dynamic agent instructions body",
            );
        preparation.session_plan_context = Some(String::from("dynamic plan context body"));
        preparation.system_prompt_hook_blocks = vec![SystemPromptHookBlock {
            tag: String::from("vmm_profile"),
            description: String::from("Dynamic profile details"),
            content: String::from("dynamic hook body"),
            source: Some(String::from("dynamic-producer-test")),
            phase: vulcan_hooks::SystemPromptHookPhase::FirstSession,
            baseline_provenance: None,
        }];

        let layers = build_runtime_prompt_layers_for_agent(&preparation, None)
            .expect("runtime prompt layers should build");

        // Every dynamic producer is confined to the non-top projection.
        // 每个动态生产者都被限制在非顶部投影内。
        for dynamic_body in [
            "dynamic agent instructions body",
            "dynamic plan context body",
            "dynamic hook body",
            "Current execution mode: Build",
        ] {
            assert!(
                layers.provider_layers.system.contains(dynamic_body)
                    || layers.provider_layers.developer.contains(dynamic_body)
                    || layers
                        .provider_layers
                        .runtime_context
                        .contains(dynamic_body),
                "`{dynamic_body}` must remain reachable for the dynamic transport"
            );
            assert!(
                !layers.fallback_text.contains(dynamic_body),
                "the fixed top must not carry dynamic producer text `{dynamic_body}`"
            );
            assert!(
                !layers
                    .locked_baseline
                    .locked_sources()
                    .iter()
                    .any(|source| source.content.contains(dynamic_body)),
                "the locked baseline must not carry dynamic producer text `{dynamic_body}`"
            );
            // The same body must remain available to the dynamic transport that publishes it durably.
            // 同一正文必须仍对可靠发布它的动态传输可用。
            assert!(
                layers.dynamic_prompt_layers.system.contains(dynamic_body)
                    || layers
                        .dynamic_prompt_layers
                        .developer
                        .contains(dynamic_body)
                    || layers
                        .dynamic_prompt_layers
                        .runtime_context
                        .contains(dynamic_body),
                "`{dynamic_body}` must remain in the request-dynamic layers"
            );
        }
        // The fixed top always ends with the fixed host guidance and never with a dynamic hook region.
        // 固定顶部始终以固定宿主指引结尾，绝不以动态 Hook 区域结尾。
        assert!(layers.fallback_text.contains(HOST_MESSAGE_GUIDANCE));
        assert!(!layers.fallback_text.contains("<system_prompt_hooks>"));
        // Every locked source that belongs to a top layer is rendered into that fixed top.
        // 每个属于顶部层的锁定来源都会渲染进该固定顶部。
        for source in layers.locked_baseline.locked_sources() {
            let rendered_into_top = matches!(
                source.target,
                vulcan_session_core::v1::context::LockedPromptTarget::System
                    | vulcan_session_core::v1::context::LockedPromptTarget::Developer
            );
            if rendered_into_top {
                assert!(
                    layers.fallback_text.contains(&source.content),
                    "locked source `{}` must be byte-identical inside the fixed top",
                    source.node_id
                );
            }
        }
    }

    /// Verifies the Plan producer is request-dynamic and reaches the request through a tagged User event.
    /// 验证 Plan 生产者属于请求动态内容，并通过带标签 User 事件到达请求。
    ///
    /// Uses the production composer and the production host-event synchronizer; a Text-only
    /// projection would silently drop plan text for every non-V1 owner.
    /// 使用生产组合器与生产宿主事件同步器；若只做文本投影，所有非 V1 所有者都会静默丢失计划正文。
    #[test]
    fn session_plan_context_reaches_request_as_tagged_user_event() {
        let mut preparation =
            build_test_preparation(String::new(), PersistedPromptConfiguration::default());
        preparation.session_plan_context =
            Some(String::from("PLAN_CONTEXT_MUST_BE_REQUEST_DYNAMIC"));

        let layers = build_runtime_prompt_layers_for_agent(&preparation, None)
            .expect("runtime prompt layers should build");

        assert!(
            !layers
                .fallback_text
                .contains("PLAN_CONTEXT_MUST_BE_REQUEST_DYNAMIC")
        );
        assert!(
            layers
                .dynamic_prompt_layers
                .developer
                .contains("PLAN_CONTEXT_MUST_BE_REQUEST_DYNAMIC")
        );
        // The matching host-event candidate carries the identical body without inventing a user.
        // 匹配的宿主事件候选携带相同正文，且不虚构用户。
        let plan_events = layers
            .host_events
            .iter()
            .filter(|event| event.body.contains("PLAN_CONTEXT_MUST_BE_REQUEST_DYNAMIC"))
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            plan_events.len(),
            1,
            "exactly one host-event candidate must carry the plan context"
        );
        assert_eq!(plan_events[0].scope, "request");
        assert!(!plan_events[0].expires_source);

        let mut snapshot = CoreConversationSnapshot::default();
        synchronize_runtime_host_events(&mut snapshot, &plan_events, None)
            .expect("plan context should synchronize as one User event");
        let plan_messages = snapshot
            .messages
            .iter()
            .filter(|message| {
                matches!(
                    &message.payload,
                    CoreConversationPayload::UserText { text, .. }
                        if text.contains("PLAN_CONTEXT_MUST_BE_REQUEST_DYNAMIC")
                            && text.contains("<vulcan_context_update ")
                )
            })
            .count();
        assert_eq!(plan_messages, 1);
    }

    /// Verifies locked-baseline projection trusts only producer-owned typed provenance.
    /// 验证锁定基线投影只信任生产者持有的强类型来源信息。
    #[test]
    fn runtime_locked_baseline_does_not_infer_from_titles_or_hook_tags() {
        let output = PromptAssemblyOutput {
            system_segments: vec![
                PromptSegment::new(
                    "dynamic-lookalike",
                    "dynamic-cache",
                    vulcan_prompt_composer::PromptNodeKind::System,
                    PromptTarget::System,
                    "AGENTS.md",
                    "same-looking body",
                )
                .with_source_label("dynamic AGENTS.md lookalike"),
                PromptSegment::new(
                    "authoritative-agents",
                    "locked-cache",
                    vulcan_prompt_composer::PromptNodeKind::System,
                    PromptTarget::System,
                    "AGENTS.md",
                    "same-looking body",
                )
                .with_source_label("D:/repo/AGENTS.md")
                .with_baseline_provenance(
                    vulcan_prompt_composer::PromptBaselineProvenance::new(
                        PromptBaselineKind::AgentsMd,
                        "D:/repo/AGENTS.md",
                    ),
                ),
            ],
            ..PromptAssemblyOutput::default()
        };
        let hooks = vec![
            SystemPromptHookBlock::new("vmm_profile", "dynamic lookalike"),
            SystemPromptHookBlock {
                tag: String::from("vmm_profile"),
                description: String::from("Profile details"),
                content: String::from("authoritative profile body"),
                source: Some(String::from("VMM profile full")),
                phase: vulcan_hooks::SystemPromptHookPhase::FirstSession,
                baseline_provenance: Some(vulcan_hooks::SystemPromptHookBaselineProvenance::new(
                    SystemPromptHookBaselineKind::MemoryProfile,
                    "hook.vmm-memory-profile",
                    "builtin.vmm.profile_prepare",
                )),
            },
        ];

        let projection = project_locked_runtime_baseline(&output, &hooks)
            .expect("typed provenance should project");
        let sources = projection.locked_sources();
        let dynamic_layers = render_request_dynamic_prompt_layers(&output, &hooks);

        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].node_id, "authoritative-agents");
        assert_eq!(sources[0].kind, LockedSourceKind::AgentsMd);
        assert_eq!(sources[1].node_id, "hook.vmm-memory-profile");
        assert_eq!(sources[1].kind, LockedSourceKind::MemoryProfile);
        assert!(sources[1].content.contains("authoritative profile body"));
        assert_eq!(
            dynamic_layers.system.matches("same-looking body").count(),
            1,
            "the locked lookalike must not be duplicated into dynamic layers"
        );
        assert!(dynamic_layers.system.contains("dynamic lookalike"));
        assert!(!dynamic_layers.system.contains("authoritative profile body"));
    }

    /// Verifies context-compaction prompts use the prompt composer instead of a post-launch system suffix.
    /// 验证上下文压缩提示词会使用提示词组合器，而不是启动后追加系统提示词后缀。
    #[test]
    fn runtime_prompt_layers_for_context_compaction_use_developer_layer() {
        let mut preparation = build_test_preparation(
            String::from("D:/missing"),
            PersistedPromptConfiguration::default(),
        );
        preparation.prompt_purpose = ExecutionPromptPurpose::ContextCompaction;
        preparation.tool_descriptors = vec![vulcan_tools_manager::VulcanToolDescriptor::new(
            String::from("shell_command"),
            String::from("shell_command"),
            String::from("Run one shell command"),
            None,
            vulcan_tools_manager::VulcanToolOrigin::new(
                vulcan_tools_manager::VulcanToolSourceKind::Native,
                "test",
                String::from("shell_command"),
            ),
        )];

        let layers = build_runtime_prompt_layers_for_agent(&preparation, None)
            .expect("runtime prompt layers should build");

        assert!(
            layers
                .provider_layers
                .developer
                .contains("Internal host task: summarize earlier conversation context")
        );
        assert!(
            !layers
                .provider_layers
                .tool_guidance
                .contains("shell_command")
        );
        assert!(layers.provider_layers.system.trim().is_empty());
    }

    /// Builds one minimal execution preparation for prompt-layer tests.
    /// 为提示词层测试构建一份最小执行准备。
    fn build_test_preparation(
        workspace_path: String,
        prompt_configuration: PersistedPromptConfiguration,
    ) -> ExecutionPreparation {
        ExecutionPreparation {
            session_id: SessionId::nil(),
            session_storage_directory: None,
            conversation_hook_turn_id: None,
            continuation_action_id: None,
            session_title: String::from("test"),
            agent_id: String::from("default"),
            agent_name: String::from("Default"),
            agent_kind: String::from("Primary"),
            agent_prompt_snapshot: None,
            dedicated_session: None,
            subagent_prompt_context: None,
            model_blueprint: sample_model_blueprint(),
            prompt_preview: String::new(),
            workspace_path,
            conversation_attachments: Vec::new(),
            context_history_count: 0,
            execution_mode: String::from("Build"),
            temperature: None,
            session_request_options: Default::default(),
            prompt_configuration,
            output_style_snapshot: None,
            prompt_purpose: ExecutionPromptPurpose::UserTurn,
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
            context_budget: Option::<ContextBudgetSnapshot>::None,
            context_compaction_policy: None,
            context_budget_override: None,
            context_assembly: Option::<ContextAssembly>::None,
            context_baseline: Option::<ContextBaseline>::None,
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

    /// Builds one real provider blueprint through the catalog resolver.
    /// 通过目录解析器构建一份真实供应商蓝图。
    fn sample_model_blueprint() -> crate::agent_runtime::provider::ProviderRuntimeBlueprint {
        crate::agent_runtime::provider::ProviderRuntimeBlueprint {
            provider_id: String::from("openrouter"),
            service_entry_id: String::from("public-api"),
            connection_id: String::from("test-connection"),
            provider_name: String::from("OpenRouter"),
            runtime_label: String::from("Vulcan Public AI Contract"),
            model_id: String::from("vendor/model"),
            model_name: String::from("Model"),
            protocol_profile_id: String::from(vulcan_llm_core::OPENAI_CHAT_PROFILE_ID),
            manifest_revision: 0,
            frozen_selection: None,
            route_revision: String::from("sha256:test-route"),
            secondary_route: Default::default(),
            public_capabilities: vulcan_ai_contract::ModelCapabilities::default(),
            base_url: Some(String::from("https://openrouter.ai/api/v1")),
            supports_remote_model_refresh: true,
            credential_source: String::from("Account A"),
            reasoning:
                crate::agent_runtime::provider_reasoning::ProviderRuntimeReasoningResolution {
                    requested_effort: ReasoningEffortLevel::Off,
                    source_label: String::from("public contract"),
                    note: None,
                },
        }
    }

    /// Builds one unique temporary directory for tests.
    /// 为测试构建一个唯一临时目录。
    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{suffix}"))
    }
}
