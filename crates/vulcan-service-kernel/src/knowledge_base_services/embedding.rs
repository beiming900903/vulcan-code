//! Frozen library-space embedding execution through the registered host port.
//! 通过已注册宿主端口执行冻结库空间嵌入。

#[cfg(test)]
mod tests;

use super::*;
use vulcan_ai_contract::{
    EmbeddingCapabilities, EmbeddingInput, EmbeddingInputContent, EmbeddingInvalidInputPolicy,
    EmbeddingPayload, EmbeddingRequest, HostAiCallContext, HostAiCapabilityRegistration,
    HostAiCapabilityRequirement, HostAiEmbeddingRequest, HostAiEmbeddingResponse,
    INFERENCE_CONTRACT_VERSION, InferenceCallerBudget, InferenceCallerContext, InferencePermission,
    InferenceSchedulingClass, InferenceUsageScope,
};

/// Maximum duration of one physical embedding attempt, in milliseconds.
/// 一次真实嵌入尝试的最大持续毫秒数。
const EMBEDDING_ATTEMPT_TIMEOUT_MS: u64 = 600_000;
/// UTF-8 input and SHA-256 identifier budget derived from the single content/batch authorities.
/// 从唯一内容及批次权威派生的 UTF-8 输入与 SHA-256 标识预算。
const EMBEDDING_INPUT_BUDGET: u64 = (MAX_KNOWLEDGE_BODY_BYTES + KNOWLEDGE_SHA256_HEX_CHARS) as u64
    * MAX_KNOWLEDGE_EMBEDDING_BATCH_SIZE as u64;

/// Derives the registration identity from exact immutable `profile`; returns its stable namespace.
/// 从精确不可变 `profile` 派生注册身份；返回稳定命名空间。
pub(super) fn knowledge_embedding_registration_id(profile: &str) -> String {
    format!("{profile}:embedding")
}

/// Resolves state's currently retained host port; returns an explicit lifecycle failure.
/// 解析 state 当前保留的宿主端口；返回明确生命周期失败。
pub(super) fn knowledge_host_ai_port(
    state: &SharedServiceBridgeState,
) -> Result<Arc<dyn HostAiPort>, String> {
    // Never dispatch inference through a draining or failed service generation.
    // 绝不通过排空或失败的服务代次分派推理。
    let guard = state.lock().map_err(|error| error.to_string())?;
    if guard.service_lifecycle_state.shutdown_drain_active
        || guard.service_lifecycle_state.shutdown_recovery_blocked
        || guard.service_lifecycle_state.http_bridge_failed
    {
        return Err("ERR_KNOWLEDGE_SERVICE_UNAVAILABLE".into());
    }
    guard
        .agent_runtime
        .as_ref()
        .map(|runtime| Arc::clone(&runtime.internal_ai_port))
        .ok_or("ERR_KNOWLEDGE_AI_PORT_UNAVAILABLE".into())
}

/// Registers exact `profile` on `port`; returns its least-privilege host handle or admission error.
/// 在 `port` 注册精确 `profile`；返回最小权限宿主句柄或准入错误。
fn register_embedding(
    port: &dyn HostAiPort,
    profile: &str,
) -> Result<vulcan_ai_contract::HostAiCapabilityHandle, String> {
    port.register_capability(HostAiCapabilityRegistration {
        registration_id: knowledge_embedding_registration_id(profile),
        owner_id: profile.into(),
        consumer_profile_id: profile.into(),
        caller: InferenceCallerContext {
            scheduling_class: InferenceSchedulingClass::ByUsageScope,
            caller_id: "vulcan-code-knowledge-embedding".into(),
            allowed_consumer_profile_ids: BTreeSet::from([profile.into()]),
            permissions: BTreeSet::from([
                InferencePermission::Status,
                InferencePermission::Embedding,
            ]),
            usage_scope: InferenceUsageScope::InternalBackground,
            interactive_session_id: None,
            owner_session_id: None,
            deadline_unix_ms: None,
            budget: InferenceCallerBudget {
                max_concurrent_executions: 1,
                max_input_units_per_request: Some(EMBEDDING_INPUT_BUDGET),
                max_output_tokens_per_request: None,
                max_requests_per_minute: None,
                max_input_units_per_minute: None,
                max_execution_duration_ms: Some(EMBEDDING_ATTEMPT_TIMEOUT_MS),
            },
        },
        requirements: vec![HostAiCapabilityRequirement {
            operation: AiOperation::EmbeddingCreate,
            purpose_id: None,
            required: true,
        }],
    })
    .map_err(|error| format!("Failed to register the knowledge embedding capability: {error}"))
}

/// Validates `response` against `request`, frozen `embedding` and declared `capabilities`; returns ordered dense vectors.
/// 根据 `request`、冻结 `embedding` 及声明 `capabilities` 验证 `response`；返回有序稠密向量。
fn validate_response(
    request: &HostAiEmbeddingRequest,
    response: HostAiEmbeddingResponse,
    embedding: &KnowledgeEmbeddingSettings,
    capabilities: &EmbeddingCapabilities,
) -> Result<Vec<Vec<f32>>, String> {
    if response.vector_space_id != request.vector_space_id {
        return Err("ERR_KNOWLEDGE_VECTOR_SPACE_MISMATCH".into());
    }
    response.response.validate_for_request(&request.request)?;
    // Physical response identity must agree with the selected route and semantic declaration.
    // 真实响应身份必须与选定路由及语义声明一致。
    let route = &response.response.route;
    // The provider fingerprint is independently checked rather than trusted through the route snapshot.
    // 独立检查供应商指纹，不通过路由快照间接信任。
    let fingerprint = &response.response.fingerprint;
    if settings::embedding_fingerprint(route, embedding.dimensions, capabilities)?
        != embedding.model_fingerprint
        || fingerprint.capability_revision != route.capability_revision
        || fingerprint.secondary_route_id != route.secondary_route_id
    {
        return Err("ERR_KNOWLEDGE_EMBEDDING_EXECUTION_CHANGED".into());
    }
    // Protocol/action optionality is declared by ExecutionRouteSnapshot, not a guessed fallback.
    // 协议及动作的可选性由 ExecutionRouteSnapshot 声明，并非推测兜底。
    let binding_matches = match embedding.route.binding_kind {
        KnowledgeRouteBindingKind::Protocol => {
            route.protocol_binding_id.as_deref() == Some(embedding.route.protocol.as_str())
                && route.action_binding_id.is_none()
        }
        KnowledgeRouteBindingKind::Action => {
            route.action_binding_id.as_deref() == Some(embedding.route.protocol.as_str())
                && route.protocol_binding_id.is_none()
        }
    };
    if route.provider_id != embedding.route.provider_id
        || route.service_entry_id != embedding.route.service_entry_id
        || Some(route.connection_id.as_str()) != embedding.route.connection_id.as_deref()
        || route.model_id != embedding.route.model_id
        || !binding_matches
        || route.usage_scope != InferenceUsageScope::InternalBackground
        || fingerprint.provider_id != embedding.route.provider_id
        || fingerprint.service_entry_id != embedding.route.service_entry_id
        || fingerprint.model_id != embedding.route.model_id
        || fingerprint.binding_id != embedding.route.protocol
        || fingerprint.task != request.request.task
        || fingerprint.dimensions != embedding.dimensions
        || fingerprint.output_kind != EmbeddingOutputKind::Dense
        || fingerprint.encoding != EmbeddingEncoding::Float
        || fingerprint.normalization != capabilities.normalization
        || fingerprint.distance_metric != capabilities.distance_metric
    {
        return Err("ERR_KNOWLEDGE_EMBEDDING_FINGERPRINT_MISMATCH".into());
    }
    // Coverage validation permits provider reordering, so sort by its verified original index.
    // 覆盖验证允许供应商重排，因此按已验证原始下标排序。
    let mut results = response.response.results;
    results.sort_by_key(|result| result.input_index);
    results
        .into_iter()
        .map(|result| match result.embedding {
            EmbeddingPayload::Dense { values }
                if values.len() == embedding.dimensions as usize
                    && values.iter().all(|value| value.is_finite()) =>
            {
                Ok(values)
            }
            _ => Err("ERR_KNOWLEDGE_EMBEDDING_VALUES_INVALID".into()),
        })
        .collect()
}

/// Executes `request` with immutable per-space configuration in `state`; `app` publishes only derived profile admission.
/// 在 `state` 中以不可变每空间配置执行 `request`；`app` 仅发布派生配置档准入。
/// Returns exact ordered vectors or an explicit error, without choosing another route.
/// 返回精确有序向量或明确错误，不选择其他路由。
async fn execute_embedding(
    app: ServiceEventEmitter,
    state: SharedServiceBridgeState,
    request: KnowledgeEmbeddingRequest,
) -> Result<Vec<Vec<f32>>, String> {
    // Check the service owner before doing configuration or inference work.
    // 执行配置或推理工作前检查服务所有者。
    let port = knowledge_host_ai_port(&state)?;
    // The physical library, never a portable consumer field, is the authorization owner.
    // 物理知识库为授权所有者，绝非可移植消费方字段。
    let runtime = retained_knowledge_runtime(&state)?;
    // Read current lifecycle while allowing a leased old query space during a model switch.
    // 读取当前生命周期，同时允许模型切换期间已租用的旧查询空间。
    let library = runtime.get_library(&request.library_id).await?;
    if library.lifecycle_state != "active" {
        return Err("ERR_KNOWLEDGE_LIBRARY_INACTIVE".into());
    }
    if !request.is_query {
        if library
            .settings
            .as_ref()
            .and_then(KnowledgeLibrarySettings::enabled_embedding)
            != Some(&request.embedding)
        {
            return Err("ERR_KNOWLEDGE_EMBEDDING_SUPERSEDED".into());
        }
        if runtime
            .get_operation(&request.library_id, None)
            .await?
            .is_some_and(|operation| operation.state == "cancelled")
        {
            return Err("ERR_KNOWLEDGE_OPERATION_CANCELLED".into());
        }
    }
    // Capabilities are revalidated exactly; a changed declaration requires an explicit new mode selection.
    // 精确复核能力；声明变更需要显式重新选择模式。
    let (execution, capabilities) = settings::embedding_contract(&state, &request.embedding.route)?;
    settings::validate_embedding_shape(
        &capabilities,
        request.embedding.dimensions,
        request.embedding.batch_size,
    )?;
    if settings::embedding_fingerprint(&execution, request.embedding.dimensions, &capabilities)?
        != request.embedding.model_fingerprint
        || settings::embedding_metric(&capabilities)? != request.embedding.metric
    {
        return Err("ERR_KNOWLEDGE_FROZEN_CAPABILITY_CHANGED".into());
    }
    if request.inputs.is_empty()
        || request.inputs.len() > request.embedding.batch_size as usize
        || request
            .inputs
            .iter()
            .any(|text| text.is_empty() || text.len() > MAX_KNOWLEDGE_BODY_BYTES)
    {
        return Err("ERR_KNOWLEDGE_EMBEDDING_INPUT_BOUNDS".into());
    }
    // Include the entire frozen semantic space in the profile identity, preserving old active readers.
    // 将完整冻结语义空间纳入配置档身份，保留旧活动读取者。
    let space = knowledge_vector_space_id(&request.embedding)?;
    // Profiles belong to one physical library and exactly one immutable space.
    // 配置档属于一个物理库及恰好一个不可变空间。
    let profile = format!(
        "{}:{space}",
        knowledge_consumer_profile_id(&request.library_id)
    );
    // A closed binding variant determines the exact inference configuration.
    // 封闭绑定变体决定精确推理配置。
    let binding = match request.embedding.route.binding_kind {
        KnowledgeRouteBindingKind::Protocol => InferenceRouteBinding::Protocol {
            protocol_profile_id: request.embedding.route.protocol.clone(),
        },
        KnowledgeRouteBindingKind::Action => InferenceRouteBinding::Action {
            action_binding_id: request.embedding.route.protocol.clone(),
        },
    };
    crate::public_ai_settings::sync_knowledge_inference_profile(
        app,
        state.clone(),
        &request.library_id,
        &profile,
        InferenceRouteConfiguration {
            provider_id: request.embedding.route.provider_id.clone(),
            service_entry_id: request.embedding.route.service_entry_id.clone(),
            connection_id: request
                .embedding
                .route
                .connection_id
                .clone()
                .ok_or("ERR_KNOWLEDGE_CONNECTION_REQUIRED")?,
            model_id: request.embedding.route.model_id.clone(),
            binding,
            operation: AiOperation::EmbeddingCreate,
            usage_scope: InferenceUsageScope::InternalBackground,
        },
        request.embedding.dimensions,
    )
    .await?;
    // Stable-sized identities distinguish repeated equal inputs without increasing the budget formula.
    // 固定大小身份区分重复相同输入，且不增加预算公式。
    let inputs = request
        .inputs
        .into_iter()
        .enumerate()
        .map(|(index, text)| {
            // Fixed-width ordinal bytes distinguish duplicate text inputs without an infallibility assertion.
            // 固定宽度序号字节区分重复文本输入，无需声明序列化必定成功。
            let mut identity = index.to_be_bytes().to_vec();
            identity.extend_from_slice(text.as_bytes());
            EmbeddingInput {
                input_id: sha256_hex(&identity),
                content: EmbeddingInputContent::Text { text },
            }
        })
        .collect();
    // One logical call corresponds to one storage-managed attempt; host retry is not requested.
    // 一次逻辑调用对应一次存储管理尝试；不请求宿主重试。
    let host_request = HostAiEmbeddingRequest {
        vector_space_id: space,
        request: EmbeddingRequest {
            contract_version: INFERENCE_CONTRACT_VERSION,
            request_id: Uuid::new_v4().to_string(),
            idempotency_key: None,
            consumer_profile_id: profile.clone(),
            inputs,
            task: settings::embedding_task(&capabilities, request.is_query)?,
            dimensions: Some(request.embedding.dimensions),
            output_kind: EmbeddingOutputKind::Dense,
            encoding: EmbeddingEncoding::Float,
            invalid_input_policy: EmbeddingInvalidInputPolicy::FailBatch,
        },
    };
    capabilities.validate_request(&host_request.request)?;
    // Registration uses the current port and exact derived profile only.
    // 注册仅使用当前端口及精确派生配置档。
    let handle = register_embedding(port.as_ref(), &profile)?;
    // The host enforces the per-attempt caller budget through this first-attempt context.
    // 宿主通过此首次尝试上下文实施每次尝试调用方预算。
    let response = port
        .create_embeddings(
            &handle,
            HostAiCallContext::new(&host_request.request.request_id),
            host_request.clone(),
        )
        .await
        .map_err(|error| format!("Knowledge embedding failed: {error}"))?;
    validate_response(&host_request, response, &request.embedding, &capabilities)
}

/// Installs the weak-state executor before recovering jobs; `state` owns the runtime and `app` owns event publication.
/// 恢复任务前安装弱状态执行器；`state` 拥有运行时，`app` 拥有事件发布。
/// Returns startup errors; background attempts retain their own explicit operation status.
/// 返回启动错误；后台尝试保留自身明确操作状态。
pub(crate) async fn start_knowledge_workers(
    state: &SharedServiceBridgeState,
    app: &ServiceEventEmitter,
) -> Result<(), String> {
    // A weak state reference avoids a service -> host executor -> service ownership cycle.
    // 弱状态引用避免服务至宿主执行器再至服务的所有权循环。
    let weak_state = Arc::downgrade(state);
    // Each call upgrades only for its finite attempt.
    // 每次调用仅为其有限尝试升级引用。
    let app = app.clone();
    retained_knowledge_runtime(state)?
        .set_embedding_executor(Arc::new(move |request| {
            // Clone only event ownership and the weak state into the future.
            // 仅将事件所有权及弱状态克隆至 future。
            let state = weak_state.upgrade();
            // Each attempt owns its event emitter clone.
            // 每次尝试拥有自己的事件发布器克隆。
            let app = app.clone();
            Box::pin(async move {
                execute_embedding(app, state.ok_or("ERR_KNOWLEDGE_SERVICE_DROPPED")?, request).await
            })
        }))
        .await
}
