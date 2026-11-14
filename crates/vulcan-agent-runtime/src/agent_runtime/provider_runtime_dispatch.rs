//! Provider runtime dispatch helpers that resolve runtime inputs and launch the host-controlled transport stream.
//! 负责解析运行时输入并启动宿主控制传输流的 Provider 运行时分发辅助模块。

use crate::agent_runtime::config::RuntimeAgentDefinition;
use crate::agent_runtime::execution_models::ExecutionPreparation;
use crate::agent_runtime::provider_reasoning::validate_exact_wire_reasoning_effort;
use crate::agent_runtime::provider_runtime_inputs::one_shot_provider_error_stream;
use crate::agent_runtime::provider_stream::ProviderDeltaStream;
use crate::agent_runtime::provider_transport::build_provider_transport_execution_request;
use crate::agent_runtime::provider_transport_request::{
    ProviderRouteIdentity, ProviderTransportTurnStartMode,
};
use crate::agent_runtime::provider_transport_request_budget::ProviderRequestBudgetGuard;
use crate::agent_runtime::provider_transport_retry::ProviderRetryPolicy;
use crate::agent_runtime::runtime_stream_intervention::RuntimePromptIntervention;
use crate::agent_runtime::task_runtime::TaskRuntime;
use crate::provider_config::ProviderConfigurationFile;
use async_stream::stream;
use futures_util::StreamExt;
use std::sync::Arc;
use vulcan_ai_contract::{
    ModelParameterValue, PromptCachePolicy, PromptCacheTtlSelection, ReasoningEffort,
};
use vulcan_core::types::{
    ConversationSnapshot as CoreConversationSnapshot,
    ProviderPromptLayers as CoreProviderPromptLayers,
};
use vulcan_provider_adapters::{
    CODEX_SERVICE_TIER_PARAMETER_ID, ResolvedPublicAiExecution, SAMPLING_TEMPERATURE_PARAMETER_ID,
};
use vulcan_session_core::v1::config::RequestOptionsConfig;
use vulcan_session_core::v1::context::LockedBaselineProjection;
use vulcan_tools_manager::{ResolvedToolDefinition, VulcanToolsManager};

/// Safe runtime default turn budget used when no explicit max-turn override is configured.
/// 未配置显式最大轮次覆盖时使用的安全运行时默认轮次预算。
pub(crate) const DEFAULT_RUNTIME_MAX_TURNS_LIMIT: usize = usize::MAX - 1;

/// Keeps the host-controlled provider dispatch loop unlimited for every agent configuration.
/// 为所有智能体配置保持宿主控制 Provider 分发循环无限。
pub(crate) fn resolve_runtime_default_max_turns_limit(
    _active_agent: Option<&RuntimeAgentDefinition>,
) -> usize {
    DEFAULT_RUNTIME_MAX_TURNS_LIMIT
}

/// Applies one runtime temperature override through the selected model's public parameter contract.
/// 通过选中模型的公共参数契约应用一次运行时温度覆盖。
///
/// # Parameters
/// # 参数
/// `execution` is the immutable public execution selection to update.
/// `execution` 是待更新的不可变公共执行选择。
/// `requested_temperature` is the agent or workbench override before provider normalization.
/// `requested_temperature` 是供应商归一化前来自智能体或工作台的覆盖值。
///
/// # Returns
/// # 返回值
/// Returns success after validation and insertion, or an explicit unsupported/value error.
/// 校验并插入后返回成功，否则返回显式的不支持或数值错误。
fn apply_runtime_temperature_override(
    execution: &mut ResolvedPublicAiExecution,
    requested_temperature: Option<f32>,
) -> Result<(), String> {
    let Some(requested_temperature) = requested_temperature else {
        return Ok(());
    };
    let definition = execution
        .model
        .capabilities
        .model_parameters
        .iter()
        .find(|definition| definition.id == SAMPLING_TEMPERATURE_PARAMETER_ID)
        .ok_or_else(|| {
            format!(
                "Model `{}` does not expose the registered sampling temperature parameter.",
                execution.model.id
            )
        })?;
    let protocol_id = execution.route.protocol_route.protocol_profile_id.as_str();
    if !definition.applies_to(protocol_id, &execution.model.id) {
        return Err(format!(
            "Model parameter `{SAMPLING_TEMPERATURE_PARAMETER_ID}` is not registered for protocol `{protocol_id}` and model `{}`.",
            execution.model.id
        ));
    }
    let value = ModelParameterValue::Decimal(requested_temperature.to_string());
    definition.validate_value(&value)?;
    execution
        .overrides
        .model_parameters
        .insert(String::from(SAMPLING_TEMPERATURE_PARAMETER_ID), value);
    Ok(())
}

/// Applies the session request options that the exact route supports and ignores the rest.
/// 应用精确路由支持的会话请求选项，忽略其余选项。
///
/// # Parameters
/// 参数
/// `execution` is the selected model whose overrides are refined, and `options` is the session
/// choice. A TTL applies only to a route that offers managed explicit caching with that option; a
/// service tier applies only to a model that registers it for the current protocol.
/// `execution` 是待细化覆盖的已选模型，`options` 是会话选择。TTL 只作用于提供该选项的显式托管
/// 缓存路由；服务档位只作用于在当前协议注册了它的模型。
fn apply_session_request_options(
    execution: &mut ResolvedPublicAiExecution,
    options: &RequestOptionsConfig,
) {
    apply_session_request_options_to_overrides(
        &execution.model.capabilities,
        execution.route.protocol_route.protocol_profile_id.as_str(),
        &mut execution.overrides,
        options,
    );
}

/// Applies supported session request options to one model's execution overrides.
/// 将受支持的会话请求选项应用到一个模型的执行覆盖。
fn apply_session_request_options_to_overrides(
    capabilities: &vulcan_ai_contract::ModelCapabilities,
    protocol_id: &str,
    overrides: &mut vulcan_provider_adapters::ModelExecutionOverrides,
    options: &RequestOptionsConfig,
) {
    if let Some(option_id) = options.prompt_cache_ttl.as_deref() {
        let capability = &capabilities.prompt_cache;
        if capability.supports_managed_explicit
            && capability
                .supported_ttl_options
                .iter()
                .any(|option| option.id == option_id)
        {
            overrides.prompt_cache = Some(PromptCachePolicy::ManagedExplicit {
                ttl: PromptCacheTtlSelection::CapabilityOption {
                    option_id: option_id.to_string(),
                },
            });
        }
    }
    if let Some(tier) = options.service_tier.as_deref() {
        let value = ModelParameterValue::String(tier.to_string());
        let registered = capabilities
            .model_parameters
            .iter()
            .find(|definition| definition.id == CODEX_SERVICE_TIER_PARAMETER_ID)
            .is_some_and(|definition| {
                (definition.protocol_ids.is_empty()
                    || definition
                        .protocol_ids
                        .iter()
                        .any(|candidate| candidate == protocol_id))
                    && definition.validate_value(&value).is_ok()
            });
        if registered {
            overrides
                .model_parameters
                .insert(String::from(CODEX_SERVICE_TIER_PARAMETER_ID), value);
        }
    }
}

/// Resolves and validates the workbench reasoning preference against provider metadata.
/// 根据供应商元数据解析并校验工作台推理偏好。
///
/// # Parameters
/// 参数
/// `execution` is the immutable selected model and `requested` is the request-scoped workbench
/// preference.
/// `execution` 是不可变的已选模型，`requested` 是请求作用域工作台偏好。
///
/// # Returns
/// 返回值
/// No explicit override for `Off`, or one supported provider-neutral effort level.
/// `Off` 返回无显式覆盖，否则返回一个受支持的供应商无关推理档位。
fn resolve_runtime_reasoning_effort(
    execution: &ResolvedPublicAiExecution,
    requested: crate::provider_config::ReasoningEffortLevel,
) -> Result<Option<ReasoningEffort>, String> {
    resolve_runtime_reasoning_effort_for_capabilities(&execution.model.capabilities, requested)
}

/// Converts one resolved model capability contract into the final provider-neutral wire effort.
/// 将解析后的模型能力契约转换为最终供应商无关的 Wire 推理档位。
///
/// # Parameters
/// 参数
/// `capabilities` is the exact model contract after catalog overlays.
/// `capabilities` 是应用目录覆盖后的精确模型契约。
/// `requested` is the workbench preference, where `Off` means provider default.
/// `requested` 是工作台偏好，其中 `Off` 表示供应商默认行为。
///
/// # Returns
/// 返回值
/// Returns `None` only when the model has no explicit reasoning effort contract.
/// 仅当模型没有显式推理档位契约时返回 `None`。
fn resolve_runtime_reasoning_effort_for_capabilities(
    capabilities: &vulcan_ai_contract::ModelCapabilities,
    requested: crate::provider_config::ReasoningEffortLevel,
) -> Result<Option<ReasoningEffort>, String> {
    // Transport consumes the already selected session level and rejects drift instead of silently recalculating it.
    // 传输层消费会话已经选定的档位；发生漂移时明确拒绝，而不是静默重新计算。
    validate_exact_wire_reasoning_effort(capabilities, requested)
}

/// Builds the credential-bearing execution from the exact selection frozen during preparation.
/// 从准备阶段冻结的精确选择构建携带凭据的执行对象。
///
/// # Parameters
/// 参数
/// `runtime` owns the shared secret store; `selection` is the complete non-secret route,
/// model, overrides, and binding frozen at the activation boundary. Execution re-reads only
/// the current secret for the frozen `secret_reference`; it never re-resolves the active
/// catalog, never re-selects a protocol, and never rebuilds overlays (plan §6.3/§R4.3).
/// `runtime` 持有共享秘密仓库；`selection` 是在激活边界冻结的完整非秘密路由、模型、
/// 覆盖与绑定。执行阶段只为冻结的 `secret_reference` 重读当前秘密；绝不重新解析活动
/// 目录、绝不重新选择协议、绝不重建覆盖（方案 §6.3/§R4.3）。
///
/// # Returns
/// 返回值
/// The exact prepared account execution, or a controlled credential error that never
/// switches account or protocol.
/// 返回精确的已准备账号执行对象，或绝不切换账号与协议的受控凭据错误。
async fn resolve_frozen_provider_execution(
    runtime: &vulcan_provider_adapters::PublicAiRuntime,
    selection: vulcan_provider_adapters::ResolvedPublicAiSelection,
) -> Result<ResolvedPublicAiExecution, String> {
    runtime
        .resolve_execution_from_frozen_selection(selection)
        .await
}

/// Formats a bounded, secret-free diagnostic for one immutable provider selection.
/// 为一个不可变供应商选择格式化有界且不含秘密的诊断信息。
fn provider_selection_diagnostic_context(
    selection: &vulcan_provider_adapters::ResolvedPublicAiSelection,
) -> String {
    let request_profiles = selection
        .route
        .request_projection_profiles
        .iter()
        .map(|profile| format!("{}@{}", profile.profile_id, profile.revision))
        .collect::<Vec<_>>()
        .join(",");
    let response_profiles = selection
        .route
        .response_projection_profiles
        .iter()
        .map(|profile| format!("{}@{}", profile.profile_id, profile.revision))
        .collect::<Vec<_>>()
        .join(",");
    let manifest_revision = selection.protocol_binding.manifest_revision;
    format!(
        "provider={}, model={}, protocol={}, manifest_revision={}, manifest_generation={}, request_profiles=[{}], response_profiles=[{}]",
        selection.route.provider.id,
        selection.model.id,
        selection.route.protocol_route.protocol_profile_id,
        manifest_revision,
        selection.manifest_generation,
        request_profiles,
        response_profiles
    )
}

/// Appends immutable route context to one provider failure without exposing credential material.
/// 为一个供应商失败追加不可变路由上下文且不暴露凭据材料。
fn diagnose_provider_failure(error: String, context: &str) -> String {
    format!("{error} [provider_route: {context}]")
}

/// Executes one provider prompt against the currently selected runtime family and returns a delta stream.
/// 针对当前选中的运行时族执行一次 Provider 提示词调用，并返回增量流。
pub(crate) fn execute_provider_prompt_stream(
    config: &ProviderConfigurationFile,
    preparation: &ExecutionPreparation,
    active_agent: Option<&RuntimeAgentDefinition>,
    system_preamble: &str,
    dynamic_prompt_layers: CoreProviderPromptLayers,
    fixed_instruction_layers: CoreProviderPromptLayers,
    locked_baseline: LockedBaselineProjection,
    prompt: &str,
    turn_start_mode: ProviderTransportTurnStartMode,
    core_snapshot: CoreConversationSnapshot,
    task_runtime: Arc<TaskRuntime>,
    tools_manager: VulcanToolsManager,
    resolved_tools: Vec<ResolvedToolDefinition>,
    public_ai_runtime: Option<Arc<vulcan_provider_adapters::PublicAiRuntime>>,
    prompt_intervention: Option<RuntimePromptIntervention>,
) -> ProviderDeltaStream {
    let Some(public_ai_runtime) = public_ai_runtime else {
        return one_shot_provider_error_stream(
            "The service has not installed the public AI runtime.",
        );
    };
    // Public execution owns the secret; the legacy transport request field is intentionally empty.
    // 公共执行拥有秘钥；旧传输请求字段刻意保持为空。
    // requested_temperature preserves the existing precedence where an active agent override wins over the workbench value.
    // requested_temperature 保留活动智能体覆盖优先于工作台值的既有优先级。
    let requested_temperature = active_agent
        .and_then(|agent| agent.temperature)
        .or(preparation.temperature);
    let temperature = requested_temperature.map(f64::from);
    let session_request_options = preparation.session_request_options.clone();
    // maxTurns remains unlimited so only the model's no-tool response ends the provider loop.
    // maxTurns 保持无限，因此只有模型返回无工具调用的响应才会结束 Provider 循环。
    let max_turns = resolve_runtime_default_max_turns_limit(active_agent);
    let model_id = preparation.model_blueprint.model_id.as_str();
    let request_budget_guard = Some(
        ProviderRequestBudgetGuard::new_v1(
            preparation.model_blueprint.provider_id.clone(),
            preparation.model_blueprint.model_id.clone(),
            preparation.model_blueprint.protocol_profile_id.clone(),
            preparation.model_blueprint.route_revision.clone(),
            preparation.model_blueprint.public_capabilities.clone(),
            preparation.model_blueprint.reasoning.clone(),
            preparation.context_compaction_policy,
            preparation.context_budget_override,
            preparation
                .context_budget
                .as_ref()
                .and_then(|budget| budget.auto_compact_prefill_input_tokens),
        )
        // Only user turns and their retries own a host compaction path that can resume after the line.
        // 只有用户回合及其重试拥有可在压缩线后继续执行的宿主压缩路径。
        .with_turn_compaction_line(
            preparation.prompt_purpose.supports_host_auto_compaction()
                && preparation.dedicated_session.is_none()
                && preparation
                    .context_budget
                    .as_ref()
                    .is_some_and(|budget| !budget.should_compact()),
        ),
    );
    let retry_policy = ProviderRetryPolicy::from_config(&config.retry);

    let mut transport_request = build_provider_transport_execution_request(
        config.provider_request_trace_enabled,
        config.provider_error_trace_enabled,
        preparation.model_blueprint.provider_id.clone(),
        model_id,
        preparation.workspace_path.as_str(),
        &preparation.execution_mode,
        system_preamble,
        dynamic_prompt_layers,
        fixed_instruction_layers,
        locked_baseline,
        prompt,
        turn_start_mode,
        core_snapshot,
        temperature,
        max_turns,
        task_runtime,
        tools_manager,
        preparation.session_id,
        resolved_tools,
        preparation.context_injection.clone(),
        request_budget_guard,
        retry_policy,
        prompt_intervention,
        preparation.tool_request_rejection_reason.clone(),
        preparation.tool_request_rejection_all_tools,
        preparation.tool_request_rejection_tool_ids.clone(),
        ProviderRouteIdentity {
            provider_id: preparation.model_blueprint.provider_id.clone(),
            service_entry_id: preparation.model_blueprint.service_entry_id.clone(),
            connection_id: preparation.model_blueprint.connection_id.clone(),
            credential_profile_id: preparation
                .model_blueprint
                .frozen_selection
                .as_ref()
                .map(|selection| selection.route.connection.credential_profile_id.clone())
                .unwrap_or_default(),
            model_id: preparation.model_blueprint.model_id.clone(),
            protocol_profile_id: preparation.model_blueprint.protocol_profile_id.clone(),
            catalog_generation: 0,
            manifest_revision: preparation.model_blueprint.manifest_revision,
            route_revision: preparation.model_blueprint.route_revision.clone(),
        },
    );
    // The complete frozen selection is cloned before the stream so the stream owns its
    // snapshot and never re-borrows the preparation (plan §R4.3/§R4.6).
    // 完整冻结选择在流创建前克隆，使流拥有其快照，绝不重新借用准备对象（方案 §R4.3/§R4.6）。
    let frozen_selection = preparation.model_blueprint.frozen_selection.clone();
    let reasoning_effort = preparation.model_blueprint.reasoning.requested_effort;
    Box::pin(stream! {
        // The catalog and route revisions are cloned before the selection moves into the resolver.
        // 目录与路由修订号在选择移入解析器前克隆。
        let Some(frozen_selection) = frozen_selection else {
            yield Err(String::from(
                "The prepared turn carries no frozen public selection and cannot execute.",
            ));
            return;
        };
        let diagnostic_context = provider_selection_diagnostic_context(&frozen_selection);
        let frozen_manifest_generation = frozen_selection.manifest_generation;
        let frozen_route_revision = frozen_selection.route_revision.clone();
        let mut public_execution = match resolve_frozen_provider_execution(
            public_ai_runtime.as_ref(),
            frozen_selection,
        ).await {
            Ok(execution) => execution,
            Err(error) => {
                yield Err(diagnose_provider_failure(error, diagnostic_context.as_str()));
                return;
            }
        };
        // Pin the exact frozen route identity so every retry attempt replays the same
        // resolved route without re-entering the protocol resolver (plan §R4.5/§R4.6).
        // 固定精确冻结路由身份，使每次重试尝试都重放同一已解析路由，绝不重新进入
        // 协议解析器（方案 §R4.5/§R4.6）。
        transport_request.route_identity = ProviderRouteIdentity {
            provider_id: public_execution.route.provider.id.clone(),
            service_entry_id: public_execution.route.entry.id.clone(),
            connection_id: public_execution.route.connection.id.clone(),
            credential_profile_id: public_execution
                .route
                .connection
                .credential_profile_id
                .clone(),
            model_id: public_execution.model.id.clone(),
            protocol_profile_id: public_execution
                .route
                .protocol_route
                .protocol_profile_id
                .clone(),
            catalog_generation: frozen_manifest_generation,
            manifest_revision: public_execution.protocol_binding.manifest_revision,
            route_revision: frozen_route_revision,
        };
        if let Err(error) =
            apply_runtime_temperature_override(&mut public_execution, requested_temperature)
        {
            yield Err(diagnose_provider_failure(error, diagnostic_context.as_str()));
            return;
        }
        apply_session_request_options(&mut public_execution, &session_request_options);
        let reasoning_effort =
            match resolve_runtime_reasoning_effort(&public_execution, reasoning_effort) {
                Ok(effort) => effort,
                Err(error) => {
                    yield Err(diagnose_provider_failure(error, diagnostic_context.as_str()));
                    return;
                }
            };
        let mut provider_stream =
            crate::agent_runtime::provider_transport_vulcan::execute_vulcan_public_stream(
                transport_request,
                public_execution,
                reasoning_effort,
            );
        while let Some(delta) = provider_stream.next().await {
            yield delta.map_err(|error| {
                diagnose_provider_failure(error, diagnostic_context.as_str())
            });
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{
        CODEX_SERVICE_TIER_PARAMETER_ID, ModelParameterValue, PromptCachePolicy,
        PromptCacheTtlSelection, RequestOptionsConfig, apply_session_request_options_to_overrides,
        provider_selection_diagnostic_context, resolve_frozen_provider_execution,
        resolve_runtime_reasoning_effort_for_capabilities,
    };
    use crate::provider_config::ReasoningEffortLevel;
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use vulcan_ai_contract::{
        CapabilitySupport, ModelCapabilities, ReasoningControlMode, ReasoningEffort,
    };
    use vulcan_llm_core::{
        ANTHROPIC_MESSAGES_PROFILE_ID, OPENAI_CHAT_PROFILE_ID, OPENAI_RESPONSES_PROFILE_ID,
    };
    use vulcan_provider_adapters::{
        ActiveAiModelSelection, CredentialConnection, CredentialMode, CredentialSecretEnvelope,
        CredentialSecretResolver, CredentialSecretStore, PersistedAiConfiguration, PublicAiRuntime,
        parse_openai_model_catalog,
    };

    /// Adds one protocol variant for each protocol id to every model of one catalog.
    /// 给一个目录的每个模型按协议 ID 各添加一个协议变体。
    ///
    /// # Parameters
    /// 参数
    /// `protocol_profile_ids` are the exact protocols the models bind to; every model
    /// receives a variant for each id so system-manifest bindings can prove their exact
    /// variant before execution (plan §R5.1).
    /// `protocol_profile_ids` 是模型绑定的精确协议；每个模型为每个 ID 获得一个变体，
    /// 使系统清单绑定能够在执行前证明其精确变体（方案 §R5.1）。
    fn with_protocol_variants(
        mut catalog: vulcan_provider_adapters::ConnectionModelCatalog,
        protocol_profile_ids: &[&str],
    ) -> vulcan_provider_adapters::ConnectionModelCatalog {
        for model in &mut catalog.models {
            for protocol_profile_id in protocol_profile_ids {
                model.protocol_variants.insert(
                    (*protocol_profile_id).to_string(),
                    vulcan_provider_adapters::ProtocolModelVariant {
                        capabilities: model.capabilities.clone(),
                        capability_evidence: Default::default(),
                        discovery_signature: String::from("test-signature"),
                        catalog_generation: catalog.generation,
                    },
                );
            }
        }
        catalog
    }

    /// Process-local secret store proving frozen execution reads the exact secret.
    /// 证明冻结执行读取精确秘密的进程内秘密存储。
    #[derive(Default)]
    struct TestMemorySecretStore(std::sync::Mutex<BTreeMap<String, CredentialSecretEnvelope>>);

    #[async_trait::async_trait]
    impl CredentialSecretResolver for TestMemorySecretStore {
        /// Returns the exact stored envelope when present.
        /// 存在时返回精确已存封装。
        async fn read_optional_secret(
            &self,
            secret_reference: &str,
        ) -> Result<Option<CredentialSecretEnvelope>, String> {
            Ok(self
                .0
                .lock()
                .map_err(|_| String::from("test secret store is poisoned."))?
                .get(secret_reference)
                .cloned())
        }
    }

    #[async_trait::async_trait]
    impl CredentialSecretStore for TestMemorySecretStore {
        /// Creates one secret only when the reference is absent.
        /// 仅在引用缺失时创建一个秘密。
        async fn create_secret(
            &self,
            secret_reference: &str,
            secret: &CredentialSecretEnvelope,
        ) -> Result<(), String> {
            let mut store = self
                .0
                .lock()
                .map_err(|_| String::from("test secret store is poisoned."))?;
            if store.contains_key(secret_reference) {
                return Err(format!("secret `{secret_reference}` already exists."));
            }
            store.insert(secret_reference.to_string(), secret.clone());
            Ok(())
        }

        /// Replaces one secret unconditionally for tests.
        /// 测试中无条件替换一个秘密。
        async fn replace_secret(
            &self,
            secret_reference: &str,
            _expected_revision: u64,
            secret: &CredentialSecretEnvelope,
        ) -> Result<(), String> {
            let mut store = self
                .0
                .lock()
                .map_err(|_| String::from("test secret store is poisoned."))?;
            store.insert(secret_reference.to_string(), secret.clone());
            Ok(())
        }

        /// Deletes one exact secret.
        /// 删除一个精确秘密。
        async fn delete_secret(&self, secret_reference: &str) -> Result<(), String> {
            let mut store = self
                .0
                .lock()
                .map_err(|_| String::from("test secret store is poisoned."))?;
            store.remove(secret_reference);
            Ok(())
        }

        /// Restores one exact secret for rollback.
        /// 为回滚恢复一个精确秘密。
        async fn restore_secret(
            &self,
            secret_reference: &str,
            secret: &CredentialSecretEnvelope,
        ) -> Result<(), String> {
            let mut store = self
                .0
                .lock()
                .map_err(|_| String::from("test secret store is poisoned."))?;
            store.insert(secret_reference.to_string(), secret.clone());
            Ok(())
        }
    }

    /// Verifies frozen execution fails on the exact frozen secret without account fallback.
    /// 验证冻结执行在精确冻结秘密上失败且不切换账号。
    ///
    /// Business invariant (plan §6.3/§R4.9): execution re-reads only the frozen
    /// `secret_reference`; a missing or invalid secret fails with a credential error and
    /// never re-selects another account or protocol.
    /// 业务不变量（方案 §6.3/§R4.9）：执行阶段只重读冻结的 `secret_reference`；
    /// 缺失或无效秘密以凭据错误失败，绝不改选其他账号或协议。
    #[test]
    fn frozen_provider_execution_fails_on_missing_secret_without_account_fallback() {
        // The selection is fully frozen: the route carries a secret reference the runtime
        // never resolves because the unavailable host store rejects every read.
        // 选择被完全冻结：路由携带运行时无法解析的秘密引用，因为不可用宿主仓库拒绝
        // 一切读取。
        let runtime = vulcan_provider_adapters::PublicAiRuntime::unavailable()
            .expect("test public AI runtime should initialize");
        let mut configuration = vulcan_provider_adapters::PersistedAiConfiguration::default();
        let connection = vulcan_provider_adapters::CredentialConnection {
            id: String::from("frozen-secret-account"),
            provider_id: String::from("deepseek"),
            service_entry_id: String::from("official-api"),
            credential_profile_id: String::from("api-key"),
            credential_mode: vulcan_provider_adapters::CredentialMode::Token,
            credential_id: String::from("credential-frozen-secret"),
            display_name: String::from("Frozen Secret Account"),
            secret_reference: String::from("secret-missing"),
            management_secret_reference: None,
            service_origin_override: None,
        };
        configuration
            .connection_graph
            .add_connection(
                &runtime
                    .providers()
                    .expect("provider registry should resolve for the frozen-secret fixture"),
                connection,
            )
            .expect("connection should register");
        let catalog = with_protocol_variants(
            parse_openai_model_catalog(
                "frozen-secret-account",
                1,
                r#"{"data":[{"id":"deepseek-v4-pro","context_window":131072,"max_output_tokens":8192,"input_modalities":["text"],"output_modalities":["text"]}]}"#,
            )
            .expect("catalog should parse"),
            &[ANTHROPIC_MESSAGES_PROFILE_ID, OPENAI_RESPONSES_PROFILE_ID],
        );
        runtime
            .stage_source_catalog_for_testing(catalog)
            .expect("source catalog should stage");
        configuration.set_model_enabled("deepseek", "deepseek-v4-pro", true);
        runtime
            .reapply_model_catalog_overlays(&configuration)
            .expect("frozen-secret test generation must publish");
        let selection = runtime
            .resolve_connection_model_selection(
                &configuration,
                "frozen-secret-account",
                "deepseek",
                "deepseek-v4-pro",
            )
            .expect("frozen selection must resolve");
        let diagnostic_context = provider_selection_diagnostic_context(&selection);
        for expected in [
            String::from("provider=deepseek"),
            String::from("model=deepseek-v4-pro"),
            format!("protocol={OPENAI_RESPONSES_PROFILE_ID}"),
            String::from("manifest_revision="),
            String::from("manifest_generation="),
            String::from("request_profiles=["),
            String::from("response_profiles=["),
        ] {
            assert!(
                diagnostic_context.contains(expected.as_str()),
                "diagnostic context must contain `{expected}`: {diagnostic_context}"
            );
        }
        let async_runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("test async runtime should initialize");
        let result = async_runtime.block_on(resolve_frozen_provider_execution(&runtime, selection));
        let Err(error) = result else {
            panic!("a missing frozen secret must fail");
        };
        assert!(
            error.contains("secret") || error.contains("credential"),
            "the failure must be a controlled credential error: {error}"
        );
    }

    /// Verifies session request options apply only where the model offers them.
    /// 验证会话请求选项只在模型提供时生效。
    #[test]
    fn session_request_options_apply_only_to_supporting_routes() {
        use vulcan_ai_contract::{
            ModelParameterDefault, ModelParameterDefinition, ModelParameterSchema,
            PromptCacheCapability, PromptCacheTtlOption,
        };
        let options = RequestOptionsConfig {
            prompt_cache_ttl: Some(String::from("1h")),
            service_tier: Some(String::from("priority")),
        };
        let supporting = ModelCapabilities {
            prompt_cache: PromptCacheCapability {
                supports_managed_explicit: true,
                supported_ttl_options: ["5m", "1h"]
                    .into_iter()
                    .map(|id| PromptCacheTtlOption {
                        id: String::from(id),
                        display_name: String::from(id),
                        description: None,
                    })
                    .collect(),
                provider_default_behavior: String::from("none"),
                profile_default_ttl_option_id: Some(String::from("5m")),
                minimum_cacheable_tokens: None,
                maximum_breakpoints: None,
            },
            model_parameters: vec![ModelParameterDefinition {
                id: String::from(CODEX_SERVICE_TIER_PARAMETER_ID),
                display_name: String::from("Service tier"),
                description: String::new(),
                schema: ModelParameterSchema::Enum {
                    values: vec![String::from("priority")],
                },
                default: ModelParameterDefault::ProviderDefault {},
                protocol_ids: vec![String::from("codex-protocol")],
                model_ids: Vec::new(),
                dependent_required: Vec::new(),
                conflicts: Vec::new(),
                evidence_revision: String::from("test"),
            }],
            ..ModelCapabilities::default()
        };

        let mut overrides = vulcan_provider_adapters::ModelExecutionOverrides::default();
        apply_session_request_options_to_overrides(
            &supporting,
            "codex-protocol",
            &mut overrides,
            &options,
        );
        assert_eq!(
            overrides.prompt_cache,
            Some(PromptCachePolicy::ManagedExplicit {
                ttl: PromptCacheTtlSelection::CapabilityOption {
                    option_id: String::from("1h"),
                },
            })
        );
        assert_eq!(
            overrides
                .model_parameters
                .get(CODEX_SERVICE_TIER_PARAMETER_ID),
            Some(&ModelParameterValue::String(String::from("priority")))
        );

        let mut untouched = vulcan_provider_adapters::ModelExecutionOverrides::default();
        apply_session_request_options_to_overrides(
            &supporting,
            "other-protocol",
            &mut untouched,
            &RequestOptionsConfig {
                prompt_cache_ttl: Some(String::from("24h")),
                service_tier: Some(String::from("priority")),
            },
        );
        apply_session_request_options_to_overrides(
            &ModelCapabilities::default(),
            "codex-protocol",
            &mut untouched,
            &options,
        );
        assert_eq!(
            untouched,
            vulcan_provider_adapters::ModelExecutionOverrides::default()
        );
    }

    /// Verifies the provider-default sentinel remains omitted at transport time.
    /// 验证供应商默认哨兵在传输阶段保持省略。
    #[test]
    fn off_preference_remains_provider_default() {
        let capabilities = ModelCapabilities {
            reasoning: CapabilitySupport::Native,
            reasoning_control: ReasoningControlMode::EffortSelection,
            reasoning_efforts: vec![ReasoningEffort::None, ReasoningEffort::Medium],
            ..ModelCapabilities::default()
        };

        assert_eq!(
            resolve_runtime_reasoning_effort_for_capabilities(
                &capabilities,
                ReasoningEffortLevel::Off,
            ),
            Ok(None)
        );
    }

    /// Verifies an explicit disabled contract cannot be re-enabled by stale effort metadata.
    /// 验证明确的关闭契约不会被旧推理档位元数据重新开启。
    #[test]
    fn explicit_disabled_contract_wins_over_stale_efforts() {
        let capabilities = ModelCapabilities {
            reasoning: CapabilitySupport::Unsupported,
            reasoning_control: ReasoningControlMode::FixedOff,
            reasoning_efforts: vec![ReasoningEffort::None, ReasoningEffort::High],
            ..ModelCapabilities::default()
        };

        assert_eq!(
            resolve_runtime_reasoning_effort_for_capabilities(
                &capabilities,
                ReasoningEffortLevel::Off,
            ),
            Ok(None)
        );
    }

    /// Verifies an explicit workbench disabled choice remains authoritative for an enabled model.
    /// 验证启用模型上的工作台明确关闭选择仍然保持权威。
    #[test]
    fn explicit_none_remains_disabled() {
        let capabilities = ModelCapabilities {
            reasoning: CapabilitySupport::Native,
            reasoning_control: ReasoningControlMode::EffortSelection,
            reasoning_efforts: vec![ReasoningEffort::None, ReasoningEffort::High],
            ..ModelCapabilities::default()
        };

        assert_eq!(
            resolve_runtime_reasoning_effort_for_capabilities(
                &capabilities,
                ReasoningEffortLevel::None,
            ),
            Ok(Some(ReasoningEffort::None))
        );
    }

    /// Verifies transport rejects a stale unsupported level instead of silently selecting another one.
    /// 验证传输层拒绝已经失效的不支持档位，而不是静默选择其他档位。
    #[test]
    fn unsupported_reasoning_level_is_rejected_without_recalculation() {
        let capabilities = ModelCapabilities {
            reasoning: CapabilitySupport::Native,
            reasoning_control: ReasoningControlMode::EffortSelection,
            reasoning_efforts: vec![ReasoningEffort::Low, ReasoningEffort::High],
            ..ModelCapabilities::default()
        };

        let error = resolve_runtime_reasoning_effort_for_capabilities(
            &capabilities,
            ReasoningEffortLevel::Adaptive,
        )
        .expect_err("transport must reject an unsupported active level");
        assert!(error.contains("not supported"));
    }

    /// R0/P0-03: a prepared turn keeps its frozen route after a manifest hot replace.
    /// R0/P0-03：已准备回合在清单热替换后保持其冻结路由。
    ///
    /// Business invariant (plan §6.3/§6.5): preparation freezes the complete non-secret route;
    /// execution re-reads only the secret and must never re-resolve the active catalog. A
    /// manifest hot replace may rebind new turns but must not invalidate an already-prepared
    /// turn.
    /// 业务不变量（方案 §6.3/§6.5）：准备阶段冻结完整非秘密路由；执行阶段只重读秘密，
    /// 绝不重新解析活动目录。清单热替换可以改变新回合，但绝不能使已准备回合失效。
    #[test]
    fn prepared_turn_keeps_frozen_route_after_manifest_hot_replace() {
        let secrets = Arc::new(TestMemorySecretStore::default());
        secrets.0.lock().expect("test store should lock").insert(
            "secret-deepseek".to_string(),
            CredentialSecretEnvelope::new_api_key(String::from("test-key"))
                .expect("api key envelope should build"),
        );
        let runtime = PublicAiRuntime::first_batch(secrets).expect("runtime should initialize");
        let mut configuration = PersistedAiConfiguration::default();
        let deepseek_connection = CredentialConnection {
            id: String::from("frozen-turn-account"),
            provider_id: String::from("deepseek"),
            service_entry_id: String::from("official-api"),
            credential_profile_id: String::from("api-key"),
            credential_mode: CredentialMode::Token,
            credential_id: String::from("credential-frozen-turn"),
            display_name: String::from("Frozen Turn Account"),
            secret_reference: String::from("secret-deepseek"),
            management_secret_reference: None,
            service_origin_override: None,
        };
        configuration
            .connection_graph
            .add_connection(
                &runtime
                    .providers()
                    .expect("provider registry should resolve for the frozen-turn fixture"),
                deepseek_connection,
            )
            .expect("connection should register");
        configuration.active = Some(ActiveAiModelSelection {
            provider_id: String::from("deepseek"),
            model_id: String::from("deepseek-v4-pro"),
            connection_id: String::from("frozen-turn-account"),
        });
        let catalog = with_protocol_variants(
            parse_openai_model_catalog(
                "frozen-turn-account",
                1,
                r#"{"data":[{"id":"deepseek-v4-pro","context_window":131072,"max_output_tokens":8192,"input_modalities":["text"],"output_modalities":["text"]},{"id":"deepseek-v4-flash","context_window":131072,"max_output_tokens":8192,"input_modalities":["text"],"output_modalities":["text"]}]}"#,
            )
            .expect("openai catalog should parse"),
            &[
                ANTHROPIC_MESSAGES_PROFILE_ID,
                OPENAI_CHAT_PROFILE_ID,
            ],
        );
        runtime
            .stage_source_catalog_for_testing(catalog)
            .expect("source catalog should stage");
        configuration.set_model_enabled("deepseek", "deepseek-v4-pro", true);
        configuration.set_model_enabled("deepseek", "deepseek-v4-flash", true);
        runtime
            .replace_configuration(configuration.clone())
            .expect("session resolver configuration should install");
        runtime
            .reapply_model_catalog_overlays(&configuration)
            .expect("bundled generation must publish before selection");

        // The bundled manifest freezes v4-pro to Responses at its declared revision.
        // 内置清单在其声明修订处把 v4-pro 冻结到 Responses。
        // sessionId owns the route lease across multiple turns of the same conversation.
        // sessionId 在同一会话的多个回合之间持有路由租约。
        let session_id = "session-frozen-route";
        // beforeBlueprint establishes the first turn's complete immutable session lease.
        // beforeBlueprint 建立首轮完整不可变会话租约。
        let before_blueprint =
            crate::agent_runtime::provider::resolve_public_ai_provider_blueprint(
                &runtime,
                session_id,
                None,
                crate::provider_config::ReasoningEffortLevel::Off,
                None,
                None,
                None,
            )
            .expect("bundled binding must resolve and lease");
        let before = before_blueprint
            .frozen_selection
            .clone()
            .expect("dispatchable blueprint must freeze a selection");
        let persisted_lease = before_blueprint
            .session_provider_route_lease_identity()
            .expect("leased blueprint must expose a persisted identity");
        let frozen_protocol = before.route.protocol_route.protocol_profile_id.clone();
        let frozen_revision = before.protocol_binding.manifest_revision;

        // conflictingTarget proves the session owner is the uniqueness key. A different model
        // cannot establish a second lease unless the explicit reselection path first clears the
        // existing session lease.
        // conflictingTarget 证明会话所有者就是唯一键。除非显式重选路径先清除现有会话租约，
        // 其他模型不能再建立第二条租约。
        let conflicting_target =
            crate::agent_runtime::provider::resolve_public_ai_provider_blueprint(
                &runtime,
                session_id,
                None,
                crate::provider_config::ReasoningEffortLevel::Off,
                Some("deepseek"),
                Some("deepseek-v4-flash"),
                Some("frozen-turn-account"),
            );
        assert!(
            conflicting_target
                .expect_err("one session must reject a second model target")
                .contains("does not match the requested model target"),
            "the existing session lease must remain the sole route authority"
        );

        // sameContentRefresh reproduces a restart after the provider returned identical model
        // content under a different process-local discovery generation.
        // sameContentRefresh 模拟供应商在不同进程内发现代次返回相同模型内容后的重启。
        let mut same_content_refresh = with_protocol_variants(
            parse_openai_model_catalog(
                "frozen-turn-account",
                17,
                r#"{"data":[{"id":"deepseek-v4-pro","context_window":131072,"max_output_tokens":8192,"input_modalities":["text"],"output_modalities":["text"]},{"id":"deepseek-v4-flash","context_window":131072,"max_output_tokens":8192,"input_modalities":["text"],"output_modalities":["text"]}]}"#,
            )
            .expect("identical refreshed catalog should parse"),
            &[
                ANTHROPIC_MESSAGES_PROFILE_ID,
                OPENAI_CHAT_PROFILE_ID,
            ],
        );
        for model in &mut same_content_refresh.models {
            for variant in model.protocol_variants.values_mut() {
                variant.catalog_generation = same_content_refresh.generation;
            }
        }
        runtime
            .stage_source_catalog_for_testing(same_content_refresh)
            .expect("identical refreshed source catalog should stage");
        runtime
            .reapply_model_catalog_overlays(&configuration)
            .expect("identical refreshed generation should publish");
        assert_eq!(
            runtime
                .clear_session_selection_leases(session_id)
                .expect("restart simulation should clear the process-local lease"),
            1
        );
        // executionIdentityDrift changes the content-addressed route and must still fail closed.
        // executionIdentityDrift 改变内容寻址路由，因此仍必须以 fail closed 拒绝。
        let mut execution_identity_drift = persisted_lease.clone();
        execution_identity_drift.route_revision = String::from("sha256:different-route");
        let rejected_restoration =
            crate::agent_runtime::provider::resolve_public_ai_provider_blueprint(
                &runtime,
                session_id,
                Some(&execution_identity_drift),
                crate::provider_config::ReasoningEffortLevel::Off,
                Some("deepseek"),
                Some("deepseek-v4-pro"),
                Some("frozen-turn-account"),
            );
        assert!(
            rejected_restoration
                .expect_err("execution route identity drift must fail closed")
                .contains("reselect the model"),
            "restart restoration must reject content-addressed route drift"
        );

        // crossProcessIdentity changes only diagnostic generations while retaining route identity.
        // crossProcessIdentity 仅改变诊断代际，同时保留执行路由身份。
        let mut cross_process_identity = persisted_lease.clone();
        cross_process_identity.manifest_revision =
            cross_process_identity.manifest_revision.saturating_add(100);
        cross_process_identity.manifest_generation = cross_process_identity
            .manifest_generation
            .saturating_add(100);
        cross_process_identity.catalog_revision = String::from("sha256:diagnostic-refresh");
        // restoredSelection proves restart restoration depends on stable target and route content,
        // not process-local counters or display-only catalog revisions (original plan §17.7).
        // restoredSelection 证明重启恢复依赖稳定目标与路由内容，而非进程局部计数或仅展示目录
        // 修订（原始方案 §17.7）。
        let restored_selection =
            crate::agent_runtime::provider::resolve_public_ai_provider_blueprint(
                &runtime,
                session_id,
                Some(&cross_process_identity),
                crate::provider_config::ReasoningEffortLevel::Off,
                Some("deepseek"),
                Some("deepseek-v4-pro"),
                Some("frozen-turn-account"),
            )
            .expect("diagnostic generation drift must restore an identical execution route");
        assert_eq!(
            restored_selection.route_revision, persisted_lease.route_revision,
            "restart restoration must preserve the content-addressed route identity"
        );

        // A hot replacement re-binds the same rule to chat with a bumped revision.
        // 热替换把同一条规则重新绑定到 chat 并提升修订号。
        let directory = vulcan_runtime_assets::resolve_bundled_provider_manifests_directory();
        let text = std::fs::read_to_string(directory.join("deepseek.json"))
            .expect("bundled deepseek manifest must exist");
        let mut value: serde_json::Value =
            serde_json::from_str(&text).expect("manifest should parse");
        value["revision"] = serde_json::json!(frozen_revision + 1);
        for rule in value["model_rules"]
            .as_array_mut()
            .expect("model_rules must be an array")
        {
            if rule["matcher"]["value"] == "deepseek-v4-pro" {
                rule["selected_protocol"] = serde_json::json!("openai-chat-completions.v1");
            }
        }
        let replacement_text = serde_json::to_string(&value).expect("manifest should serialize");
        let replaced = vulcan_provider_adapters::compile_manifest_text(
            &replacement_text,
            vulcan_provider_adapters::ManifestOwnership::System,
        )
        .expect("replacement manifest must compile");
        runtime
            .replace_system_manifest("deepseek", Arc::new(replaced))
            .expect("hot replace should succeed");
        let async_runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("test async runtime should initialize");
        async_runtime
            .block_on(runtime.activate_staged_manifest_catalogs(&configuration, &[]))
            .expect("replacement generation must publish atomically");
        let replacement_selection = runtime
            .resolve_connection_model_selection(
                &configuration,
                "frozen-turn-account",
                "deepseek",
                "deepseek-v4-pro",
            )
            .expect("new selection must observe the replacement generation");
        assert_eq!(
            replacement_selection
                .route
                .protocol_route
                .protocol_profile_id,
            OPENAI_CHAT_PROFILE_ID,
            "new turns must observe the newly published protocol"
        );

        // sameSessionBlueprint uses the exact target shape emitted by compaction and proves it
        // converges on the ordinary active turn's lease after hot replacement.
        // sameSessionBlueprint 使用压缩路径发出的精确目标形态，并证明它在热替换后仍汇聚到
        // 普通 active 回合的租约。
        let same_session_blueprint =
            crate::agent_runtime::provider::resolve_public_ai_provider_blueprint(
                &runtime,
                session_id,
                Some(&persisted_lease),
                crate::provider_config::ReasoningEffortLevel::Off,
                Some("deepseek"),
                Some("deepseek-v4-pro"),
                Some("frozen-turn-account"),
            )
            .expect("same session must retain the leased route");
        assert_eq!(
            same_session_blueprint.protocol_profile_id, frozen_protocol,
            "later turns in the same session must keep the original protocol"
        );

        // newSessionBlueprint proves a different session adopts the newly activated route.
        // newSessionBlueprint 证明不同会话采用新激活路由。
        let new_session_blueprint =
            crate::agent_runtime::provider::resolve_public_ai_provider_blueprint(
                &runtime,
                "session-new-route",
                None,
                crate::provider_config::ReasoningEffortLevel::Off,
                Some("deepseek"),
                Some("deepseek-v4-pro"),
                Some("frozen-turn-account"),
            )
            .expect("new session must adopt the replacement route");
        assert_eq!(
            new_session_blueprint.protocol_profile_id, OPENAI_CHAT_PROFILE_ID,
            "a new session must use the new manifest revision"
        );

        let cleared_lease_count = runtime
            .clear_session_selection_leases(session_id)
            .expect("explicit reselection must clear every session lease");
        assert_eq!(
            cleared_lease_count, 1,
            "one session can own exactly one process-local route lease"
        );
        // restartRestoration proves a persisted old identity cannot silently adopt the new route.
        // restartRestoration 证明持久化旧身份不能静默采用新路由。
        let restart_restoration =
            crate::agent_runtime::provider::resolve_public_ai_provider_blueprint(
                &runtime,
                session_id,
                Some(&persisted_lease),
                crate::provider_config::ReasoningEffortLevel::Off,
                Some("deepseek"),
                Some("deepseek-v4-pro"),
                Some("frozen-turn-account"),
            );
        assert!(
            restart_restoration
                .expect_err("stale persisted identity must fail closed")
                .contains("reselect the model"),
            "restart drift must provide one actionable reselection error"
        );
        // reselectedBlueprint proves selecting the same model explicitly still adopts the new route.
        // reselectedBlueprint 证明显式重选同一模型仍会采用新路由。
        let reselected_blueprint =
            crate::agent_runtime::provider::resolve_public_ai_provider_blueprint(
                &runtime,
                session_id,
                None,
                crate::provider_config::ReasoningEffortLevel::Off,
                Some("deepseek"),
                Some("deepseek-v4-pro"),
                Some("frozen-turn-account"),
            )
            .expect("explicit reselection must install the replacement route");
        assert_eq!(
            reselected_blueprint.protocol_profile_id, OPENAI_CHAT_PROFILE_ID,
            "explicit same-model reselection must use the latest revision"
        );

        // The prepared turn re-executes through its frozen selection and must keep the old
        // route; the hot replace only affects new turns.
        // 已准备回合按冻结选择重新执行并必须保持旧路由；热替换只影响新回合。
        let execution = async_runtime
            .block_on(resolve_frozen_provider_execution(&runtime, before))
            .expect("the prepared turn must keep its old snapshot and execute");
        assert_eq!(
            execution.route.protocol_route.protocol_profile_id, frozen_protocol,
            "hot replacement must not rebind an already-prepared turn"
        );
    }
}
