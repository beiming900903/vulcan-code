use crate::cache::keys::node_cache_key;
use crate::cache::memory::MemoryPromptCache;
use crate::composer::diagnostics::PromptComposerDiagnostics;
use crate::composer::request::PromptAssemblyInput;
use crate::personality::catalog::PersonalityDefinition;
use crate::source::hashing::stable_text_hash;
use crate::strategy::PlannedNode;
use crate::types::{
    PromptBaselineKind, PromptBaselineProvenance, PromptNodeKind, PromptSegment, PromptTarget,
};

/// Render one planned non-bundle node.
/// 渲染一个非组合型计划节点。
pub fn render_node(
    planned_node: PlannedNode,
    input: &PromptAssemblyInput,
    personality: Option<&PersonalityDefinition>,
    cache: &MemoryPromptCache,
    diagnostics: &mut PromptComposerDiagnostics,
) -> Option<PromptSegment> {
    let mut segment_context = CachedSegmentContext { cache, diagnostics };
    let built = match planned_node {
        PlannedNode::ProviderBaseSystem => input.provider_prompt.as_ref().map(|snapshot| {
            build_cached_segment(
                &mut segment_context,
                CachedSegmentDescriptor::new(
                    "provider_base_system",
                    snapshot.fingerprint,
                    PromptNodeKind::System,
                    PromptTarget::System,
                    "Provider Base System Prompt",
                    &snapshot.text,
                    snapshot.source_label.clone(),
                )
                .with_baseline_provenance(PromptBaselineProvenance::new(
                    PromptBaselineKind::FixedInstruction,
                    "provider-base-system",
                )),
            )
        }),
        PlannedNode::AgentBaseSystem => input.agent_prompt.as_ref().map(|snapshot| {
            build_cached_segment(
                &mut segment_context,
                CachedSegmentDescriptor::new(
                    "agent_base_system",
                    snapshot.fingerprint,
                    PromptNodeKind::System,
                    PromptTarget::System,
                    "Agent Base System Prompt",
                    &snapshot.text,
                    snapshot.source_label.clone(),
                )
                .with_optional_baseline_provenance(
                    snapshot.fixed_instruction.then(|| {
                        PromptBaselineProvenance::new(
                            PromptBaselineKind::FixedInstruction,
                            "agent-base-system",
                        )
                    }),
                ),
            )
        }),
        PlannedNode::FunctionalAgentSystem => input
            .subagent
            .as_ref()
            .and_then(|snapshot| snapshot.functional_system_prompt.as_deref())
            .filter(|text| !text.trim().is_empty())
            .map(|text| {
                build_cached_segment(
                    &mut segment_context,
                    CachedSegmentDescriptor::new(
                        "functional_agent_system",
                        stable_text_hash(text),
                        PromptNodeKind::System,
                        PromptTarget::System,
                        "Functional Agent System Prompt",
                        text,
                        "Subagent functional system prompt",
                    ),
                )
            }),
        PlannedNode::Personality => personality.map(|definition| {
            let target = if input.model.supports_native_personality {
                PromptTarget::System
            } else {
                PromptTarget::Developer
            };
            let kind = if input.model.supports_native_personality {
                PromptNodeKind::System
            } else {
                PromptNodeKind::Developer
            };
            build_cached_segment(
                &mut segment_context,
                CachedSegmentDescriptor::new(
                    "personality",
                    stable_text_hash(&format!(
                        "{}|{}|{}",
                        definition.id,
                        definition.fingerprint,
                        input.model.supports_native_personality
                    )),
                    kind,
                    target,
                    format!("Personality {}", definition.label),
                    &definition.text,
                    definition.source_label.clone(),
                )
                .with_baseline_provenance(PromptBaselineProvenance::new(
                    PromptBaselineKind::AgentPersona,
                    definition.id.clone(),
                )),
            )
        }),
        PlannedNode::OutputStyle => input
            .output_style
            .as_ref()
            .filter(|snapshot| snapshot.is_injected())
            .map(|snapshot| {
                build_cached_segment(
                    &mut segment_context,
                    CachedSegmentDescriptor::new(
                        "output_style",
                        snapshot.fingerprint,
                        PromptNodeKind::System,
                        PromptTarget::System,
                        format!("Output Style {}", snapshot.label),
                        &snapshot.text,
                        snapshot.source_label.clone(),
                    )
                    .with_baseline_provenance(PromptBaselineProvenance::new(
                        PromptBaselineKind::OutputStyle,
                        snapshot.id.clone(),
                    )),
                )
            }),
        PlannedNode::ProjectRules => input.project_rules.as_ref().map(|snapshot| {
            build_cached_segment(
                &mut segment_context,
                CachedSegmentDescriptor::new(
                    "project_rules",
                    snapshot.content_hash,
                    PromptNodeKind::User,
                    PromptTarget::User,
                    format!("Project Rules {}", snapshot.source),
                    &snapshot.text,
                    format!("Project rules `{}`", snapshot.source),
                )
                .with_baseline_provenance(PromptBaselineProvenance::new(
                    PromptBaselineKind::AgentsMd,
                    snapshot.source.clone(),
                )),
            )
        }),
        PlannedNode::EnvironmentContext => input.environment_context.as_ref().map(|snapshot| {
            build_cached_segment(
                &mut segment_context,
                CachedSegmentDescriptor::new(
                    "environment_context",
                    snapshot.fingerprint,
                    PromptNodeKind::User,
                    PromptTarget::User,
                    "Environment Context",
                    &snapshot.text,
                    "Runtime environment context",
                ),
            )
        }),
        PlannedNode::TaskGoal => input.task_goal.as_ref().map(|snapshot| {
            build_cached_segment(
                &mut segment_context,
                CachedSegmentDescriptor::new(
                    "task_goal",
                    snapshot.fingerprint,
                    PromptNodeKind::User,
                    PromptTarget::User,
                    "Task Goal",
                    &snapshot.text,
                    "Runtime task goal",
                ),
            )
        }),
        PlannedNode::SubagentIdentity => input.subagent.as_ref().map(|snapshot| {
            let body = format!(
                "Subagent identity: {}\nDelegated task: {}\n{}",
                snapshot.identity,
                snapshot.task,
                snapshot
                    .delegation
                    .as_deref()
                    .map(|text| format!("Delegation guidance: {text}"))
                    .unwrap_or_default()
            )
            .trim()
            .to_string();
            build_cached_segment(
                &mut segment_context,
                CachedSegmentDescriptor::new(
                    "subagent_identity",
                    snapshot.fingerprint(),
                    PromptNodeKind::Developer,
                    PromptTarget::Developer,
                    "Subagent Identity",
                    body,
                    format!("Runtime subagent delegation `{}`", snapshot.identity),
                ),
            )
        }),
        PlannedNode::ToolHelpBundle => None,
    };
    built.flatten()
}

/// Build one cached prompt segment.
/// 构建一个带缓存的提示词片段。
fn build_cached_segment(
    context: &mut CachedSegmentContext<'_>,
    descriptor: CachedSegmentDescriptor,
) -> Option<PromptSegment> {
    let cache_key = node_cache_key(descriptor.namespace, descriptor.fingerprint);
    if let Some(segment) = context.cache.get_node(&cache_key) {
        context.diagnostics.record_node_cache(&cache_key, true);
        return Some(
            segment
                .with_source_label(descriptor.source_label)
                .with_optional_baseline_provenance(descriptor.baseline_provenance),
        );
    }
    context.diagnostics.record_node_cache(&cache_key, false);
    if descriptor.body.trim().is_empty() {
        return None;
    }
    let segment = PromptSegment::new(
        descriptor.namespace,
        cache_key,
        descriptor.kind,
        descriptor.target,
        descriptor.title,
        descriptor.body,
    )
    .with_source_label(descriptor.source_label)
    .with_optional_baseline_provenance(descriptor.baseline_provenance);
    context.cache.put_node(segment.clone());
    Some(segment)
}

/// Runtime dependencies required to build cached prompt segments.
/// 构建缓存提示词片段所需的运行时依赖。
struct CachedSegmentContext<'a> {
    /// Shared prompt node cache.
    /// 共享提示词节点缓存。
    cache: &'a MemoryPromptCache,
    /// Prompt composer diagnostics accumulator.
    /// 提示词组合诊断累加器。
    diagnostics: &'a mut PromptComposerDiagnostics,
}

/// Immutable description of one prompt segment to cache or build.
/// 单个待缓存或构建提示词片段的不可变描述。
struct CachedSegmentDescriptor {
    /// Cache namespace and segment identifier prefix.
    /// 缓存命名空间与片段标识前缀。
    namespace: &'static str,
    /// Stable content fingerprint used by the cache key.
    /// 缓存键使用的稳定内容指纹。
    fingerprint: u64,
    /// Prompt node kind.
    /// 提示词节点类型。
    kind: PromptNodeKind,
    /// Prompt target bucket.
    /// 提示词目标桶。
    target: PromptTarget,
    /// Human-readable segment title.
    /// 人类可读的片段标题。
    title: String,
    /// Segment body text.
    /// 片段正文文本。
    body: String,
    /// Diagnostics source label.
    /// 诊断来源标签。
    source_label: String,
    /// Producer-declared locked-baseline provenance.
    /// 生产者声明的锁定基线来源。
    baseline_provenance: Option<PromptBaselineProvenance>,
}

impl CachedSegmentDescriptor {
    /// Build one cached segment descriptor from primitive segment fields.
    /// 根据基础片段字段构建缓存片段描述。
    fn new(
        namespace: &'static str,
        fingerprint: u64,
        kind: PromptNodeKind,
        target: PromptTarget,
        title: impl Into<String>,
        body: impl Into<String>,
        source_label: impl Into<String>,
    ) -> Self {
        Self {
            namespace,
            fingerprint,
            kind,
            target,
            title: title.into(),
            body: body.into(),
            source_label: source_label.into(),
            baseline_provenance: None,
        }
    }

    /// Attaches producer-declared locked-baseline provenance to this descriptor.
    /// 为当前描述附加由生产者声明的锁定基线来源。
    fn with_baseline_provenance(self, provenance: PromptBaselineProvenance) -> Self {
        self.with_optional_baseline_provenance(Some(provenance))
    }

    /// Sets the optional producer provenance and returns the updated cache descriptor.
    /// 设置生产者的可选来源信息，并返回更新后的缓存描述。
    fn with_optional_baseline_provenance(
        mut self,
        provenance: Option<PromptBaselineProvenance>,
    ) -> Self {
        self.baseline_provenance = provenance;
        self
    }
}
