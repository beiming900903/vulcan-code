use crate::output_style::PromptOutputStyleSnapshot;
use crate::source::snapshot::{
    AgentPromptSnapshot, EnvironmentContextSnapshot, ProjectRulesSnapshot, ProviderPromptSnapshot,
    SubagentSnapshot, TaskGoalSnapshot, ToolCatalogSnapshot,
};
use crate::types::{
    ModelDescriptor, PersonalitySelection, PromptAssemblyMode, PromptBaselineProvenance,
};

/// One explicitly classified runtime-provided prompt segment.
/// 一条已显式分类的运行时提示词片段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdditionalPromptSegment {
    /// Human-readable segment title.
    /// 人类可读的片段标题。
    pub title: String,
    /// Exact provider-visible body.
    /// 精确的 Provider 可见正文。
    pub body: String,
    /// Human-readable source label used only for diagnostics.
    /// 仅用于诊断的人类可读来源标签。
    pub source_label: String,
    /// Producer-declared stable provenance, or `None` for request-dynamic content.
    /// 生产者声明的稳定来源；请求动态内容为 `None`。
    pub baseline_provenance: Option<PromptBaselineProvenance>,
}

impl AdditionalPromptSegment {
    /// Builds one request-dynamic segment that must not enter the runtime lock.
    /// 构建一条不得进入运行时锁定的请求动态片段。
    pub fn dynamic(
        title: impl Into<String>,
        body: impl Into<String>,
        source_label: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            source_label: source_label.into(),
            baseline_provenance: None,
        }
    }

    /// Builds one producer-declared stable segment with the supplied provenance.
    /// 使用给定来源构建一条由生产者声明的稳定片段。
    pub fn locked(
        title: impl Into<String>,
        body: impl Into<String>,
        source_label: impl Into<String>,
        baseline_provenance: PromptBaselineProvenance,
    ) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            source_label: source_label.into(),
            baseline_provenance: Some(baseline_provenance),
        }
    }
}

/// Complete prompt assembly input payload.
/// 完整提示词装配输入载荷。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptAssemblyInput {
    /// Composition mode.
    /// 组合模式。
    pub mode: PromptAssemblyMode,
    /// Target model descriptor.
    /// 目标模型描述。
    pub model: ModelDescriptor,
    /// Optional provider base prompt.
    /// 可选 Provider 基础提示词。
    pub provider_prompt: Option<ProviderPromptSnapshot>,
    /// Optional agent base prompt.
    /// 可选 Agent 基础提示词。
    pub agent_prompt: Option<AgentPromptSnapshot>,
    /// Requested personality selection.
    /// 请求的人格选择。
    pub personality: PersonalitySelection,
    /// Immutable output-style snapshot captured at a safe session boundary.
    /// 在安全会话边界捕获的不可变输出风格快照。
    pub output_style: Option<PromptOutputStyleSnapshot>,
    /// Optional project rules snapshot.
    /// 可选项目规则快照。
    pub project_rules: Option<ProjectRulesSnapshot>,
    /// Optional environment context snapshot.
    /// 可选环境上下文快照。
    pub environment_context: Option<EnvironmentContextSnapshot>,
    /// Optional task-goal snapshot.
    /// 可选任务目标快照。
    pub task_goal: Option<TaskGoalSnapshot>,
    /// Visible tool catalog snapshot.
    /// 可见工具目录快照。
    pub tool_catalog: ToolCatalogSnapshot,
    /// Optional subagent snapshot.
    /// 可选子代理快照。
    pub subagent: Option<SubagentSnapshot>,
    /// Additional explicitly classified developer-side segments.
    /// 额外的已显式分类开发者侧片段。
    pub custom_developer_segments: Vec<AdditionalPromptSegment>,
    /// Additional explicitly classified user-side segments.
    /// 额外的已显式分类用户侧片段。
    pub custom_user_segments: Vec<AdditionalPromptSegment>,
}
