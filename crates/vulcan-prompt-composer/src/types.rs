/// Final destination used by one prompt segment.
/// 单个提示词片段的最终目标层。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PromptTarget {
    /// System instruction stream.
    /// 系统指令流。
    System,
    /// Developer instruction stream.
    /// 开发者指令流。
    Developer,
    /// User-visible contextual stream.
    /// 用户侧上下文流。
    User,
    /// Tool-help metadata stream.
    /// 工具帮助元数据流。
    ToolMetadata,
    /// Diagnostics-only stream.
    /// 仅诊断流。
    DiagnosticsOnly,
}

/// Logical node kind used by diagnostics and ordering.
/// 诊断与排序使用的逻辑节点类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PromptNodeKind {
    /// System node.
    /// 系统节点。
    System,
    /// Developer node.
    /// 开发者节点。
    Developer,
    /// User node.
    /// 用户节点。
    User,
    /// Tool-help node.
    /// 工具帮助节点。
    ToolMetadata,
}

/// Producer-declared kind of a stable prompt segment.
/// 生产者声明的稳定提示词片段类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PromptBaselineKind {
    /// Fixed instruction resolved at execution launch and rendered at the request head.
    /// 在执行启动时解析、在请求头部渲染的固定指令。
    FixedInstruction,
    /// Memory profile baseline.
    /// 记忆画像基线。
    MemoryProfile,
    /// One participating `AGENTS.md` layer.
    /// 一层实际参与组合的 `AGENTS.md`。
    AgentsMd,
    /// Active agent persona.
    /// 当前代理人格。
    AgentPersona,
    /// Active output style.
    /// 当前输出风格。
    OutputStyle,
    /// Fixed injection produced by the enabled prompt-skill set.
    /// 已启用提示词技能集合产生的固定注入。
    PromptSkillInjection,
    /// Long-lived plugin instructions.
    /// 长期插件指令。
    PluginInstructions,
    /// Long-lived baseline hook body.
    /// 长期基线 Hook 正文。
    BaselineHooks,
}

impl PromptBaselineKind {
    /// Returns the stable cache-identity label for this producer-declared kind.
    /// 返回当前生产者声明类型的稳定缓存身份标签。
    pub const fn cache_identity(self) -> &'static str {
        match self {
            Self::FixedInstruction => "fixed-instruction",
            Self::MemoryProfile => "memory-profile",
            Self::AgentsMd => "agents-md",
            Self::AgentPersona => "agent-persona",
            Self::OutputStyle => "output-style",
            Self::PromptSkillInjection => "prompt-skill-injection",
            Self::PluginInstructions => "plugin-instructions",
            Self::BaselineHooks => "baseline-hooks",
        }
    }
}

/// Typed provenance attached by the authoritative prompt source producer.
/// 由权威提示词来源生产者附加的强类型来源信息。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PromptBaselineProvenance {
    /// Stable baseline kind declared by the producer.
    /// 由生产者声明的稳定基线类型。
    pub kind: PromptBaselineKind,
    /// Stable producer-owned source identity.
    /// 由生产者持有的稳定来源身份。
    pub source_id: String,
}

impl PromptBaselineProvenance {
    /// Builds one explicit producer-owned baseline provenance value.
    /// 构建一条显式且由生产者持有的基线来源信息。
    pub fn new(kind: PromptBaselineKind, source_id: impl Into<String>) -> Self {
        Self {
            kind,
            source_id: source_id.into(),
        }
    }
}

/// Prompt assembly mode used to select one strategy.
/// 选择策略的提示词装配模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PromptAssemblyMode {
    /// Primary/root agent composition.
    /// 主代理组合模式。
    Primary,
    /// Minimal pruned context-compaction composition.
    /// 极简裁剪上下文压缩组合模式。
    ContextCompactionMinimal,
    /// Subagent clone composition.
    /// 子代理分身组合模式。
    SubagentClone,
    /// Subagent functional composition.
    /// 子代理功能组合模式。
    SubagentFunctional,
    /// Dedicated-session composition without general tools.
    /// 不开放通用工具的专职会话组合模式。
    DedicatedMinimal,
    /// Dedicated-session composition with general tools.
    /// 开放通用工具的专职会话组合模式。
    DedicatedTooling,
}

/// Personality selection requested by the caller.
/// 调用方请求的人格选择。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PersonalitySelection {
    /// Use the catalog default.
    /// 使用目录默认人格。
    Default,
    /// Use one named personality.
    /// 使用具名人格。
    Named(String),
    /// Use one inline personality payload.
    /// 使用内联人格内容。
    Inline {
        /// Stable inline personality identifier.
        /// 稳定的内联人格标识。
        id: String,
        /// Inline personality text body.
        /// 内联人格正文文本。
        text: String,
    },
    /// Disable personality injection.
    /// 禁用人格注入。
    Disabled,
}

/// Model capabilities used by prompt assembly.
/// 提示词装配使用的模型能力描述。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelDescriptor {
    /// Provider/model family slug.
    /// Provider 或模型家族标识。
    pub family: String,
    /// Exact model identifier.
    /// 精确模型标识。
    pub model_id: String,
    /// Whether the model can absorb personality into system instructions.
    /// 模型是否可将人格直接吸收到系统提示词。
    pub supports_native_personality: bool,
}

/// Visible tool descriptor used by tool-help composition.
/// 工具帮助组合使用的可见工具描述。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDescriptor {
    /// Stable tool identifier.
    /// 稳定工具标识。
    pub id: String,
    /// Human-facing display title.
    /// 面向人的显示标题。
    pub title: String,
    /// Short capability description.
    /// 简短能力描述。
    pub description: String,
    /// Optional explicit help text override.
    /// 可选显式帮助文本覆盖。
    pub help_text: Option<String>,
    /// Optional schema fingerprint supplied by the caller.
    /// 调用方提供的可选 schema 指纹。
    pub schema_fingerprint: Option<String>,
}

/// Final rendered prompt segment.
/// 最终渲染后的提示词片段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptSegment {
    /// Stable node identifier.
    /// 稳定节点标识。
    pub node_id: String,
    /// Stable cache key.
    /// 稳定缓存键。
    pub cache_key: String,
    /// Logical node kind.
    /// 逻辑节点类型。
    pub kind: PromptNodeKind,
    /// Final target layer.
    /// 最终目标层。
    pub target: PromptTarget,
    /// Human-readable title.
    /// 可读标题。
    pub title: String,
    /// Final body text.
    /// 最终正文文本。
    pub body: String,
    /// Human-readable source label for diagnostics.
    /// 供诊断使用的人类可读来源标签。
    pub source_label: String,
    /// Typed stable provenance, or `None` for request-dynamic content.
    /// 强类型稳定来源；请求动态内容为 `None`。
    pub baseline_provenance: Option<PromptBaselineProvenance>,
    /// Whether the segment came from cache.
    /// 片段是否来自缓存。
    pub from_cache: bool,
}

impl PromptSegment {
    /// Build one fresh segment instance.
    /// 构建一个新的片段实例。
    pub fn new(
        node_id: impl Into<String>,
        cache_key: impl Into<String>,
        kind: PromptNodeKind,
        target: PromptTarget,
        title: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            node_id: node_id.into(),
            cache_key: cache_key.into(),
            kind,
            target,
            title: title.into(),
            body: body.into(),
            source_label: String::from("unspecified"),
            baseline_provenance: None,
            from_cache: false,
        }
    }

    /// Attach one human-readable source label to this segment.
    /// 为该片段附加一个人类可读来源标签。
    pub fn with_source_label(mut self, source_label: impl Into<String>) -> Self {
        self.source_label = source_label.into();
        self
    }

    /// Attaches producer-declared locked-baseline provenance to this segment.
    /// 为当前片段附加由生产者声明的锁定基线来源。
    pub fn with_baseline_provenance(mut self, provenance: PromptBaselineProvenance) -> Self {
        self.baseline_provenance = Some(provenance);
        self
    }

    /// Replaces the optional producer-declared locked-baseline provenance.
    /// 替换可选的生产者声明锁定基线来源。
    pub fn with_optional_baseline_provenance(
        mut self,
        provenance: Option<PromptBaselineProvenance>,
    ) -> Self {
        self.baseline_provenance = provenance;
        self
    }

    /// Clone the segment and mark it as cache-backed.
    /// 克隆片段并将其标记为来自缓存。
    pub fn cached_copy(&self) -> Self {
        let mut copy = self.clone();
        copy.from_cache = true;
        copy
    }
}
