//! AI-maintained knowledge and explicit human lifecycle models.
//! AI 维护知识与显式人工生命周期模型。

use serde::{Deserialize, Serialize};
use vulcan_ai_contract::KNOWLEDGE_CONSUMER_PROFILE_PREFIX;
pub use vulcan_lancedb_engine::types::LanceDbDistanceMetric as KnowledgeVectorMetric;

/// Maximum UTF-8 bytes in one body.
/// 单篇正文的最大 UTF-8 字节数。
pub const MAX_KNOWLEDGE_BODY_BYTES: usize = 1024 * 1024;

/// Maximum files accepted by one ingest.
/// 单次搬运最大文件数。
pub const MAX_KNOWLEDGE_INGEST_FILES: usize = 200;

/// Maximum decoded bytes accepted by one ingest.
/// 单次搬运最大解码字节数。
pub const MAX_KNOWLEDGE_INGEST_BYTES: usize = 32 * MAX_KNOWLEDGE_BODY_BYTES;

/// Maximum summary Unicode scalar count.
/// 摘要 Unicode 标量数上限。
pub const MAX_KNOWLEDGE_SUMMARY_CHARS: usize = 2000;

/// Maximum keywords per document.
/// 每文档关键词数上限。
pub const MAX_KNOWLEDGE_KEYWORDS: usize = 32;

/// Maximum path component count.
/// 路径组件数量上限。
pub const MAX_KNOWLEDGE_PATH_DEPTH: usize = 8;

/// Maximum Unicode scalars per path segment.
/// 路径段 Unicode 标量数上限。
pub const MAX_KNOWLEDGE_PATH_SEGMENT_CHARS: usize = 128;

/// Maximum heading lines per ingest receipt.
/// 每篇搬运回执标题行数上限。
pub const MAX_KNOWLEDGE_OUTLINE_LINES: usize = 40;

/// Total UTF-8 heading bytes exposed by one ingest receipt.
/// 单次搬运回执暴露的标题 UTF-8 总字节数。
pub const MAX_KNOWLEDGE_OUTLINE_BYTES: usize = 64 * 1024;

/// Maximum Unicode scalars in an AI-maintained document title.
/// AI 维护文档标题的 Unicode 标量数量上限。
pub const MAX_KNOWLEDGE_TITLE_CHARS: usize = 512;

/// Maximum descendant levels in one logical listing.
/// 单次逻辑列表的最大后代层数。
pub const MAX_KNOWLEDGE_LIST_DEPTH: usize = 3;

/// Maximum Unicode scalars in a folder description or audit note.
/// 目录说明或审计说明中的最大 Unicode 标量数。
pub const MAX_KNOWLEDGE_METADATA_CHARS: usize = 2000;

/// Maximum source observations per document image.
/// 每个文档镜像的来源观测数量上限。
pub const MAX_KNOWLEDGE_PROVENANCE_ITEMS: usize = 32;

/// Maximum Unicode scalars in one source reference.
/// 单个来源引用的最大 Unicode 标量数。
pub const MAX_KNOWLEDGE_REFERENCE_CHARS: usize = 4096;

/// Maximum serialized history item bytes returned by one page.
/// 单页返回的历史条目序列化字节上限。
pub const MAX_KNOWLEDGE_HISTORY_BYTES: usize = 2 * 1024 * 1024;

/// Exact number of ASCII hex digits in a SHA-256 digest.
/// SHA-256 摘要中的精确 ASCII 十六进制位数。
pub const KNOWLEDGE_SHA256_HEX_CHARS: usize = 64;

/// Automatic attempts before human retry.
/// 人工重试前的自动尝试次数。
pub const MAX_KNOWLEDGE_EMBEDDING_ATTEMPTS: u32 = 3;

/// Maximum lines returned by one read.
/// 单次读取最大行数。
pub const MAX_KNOWLEDGE_READ_LINES: usize = 400;

/// Maximum federated query count.
/// 联合查询数量上限。
pub const MAX_KNOWLEDGE_QUERIES: usize = 8;

/// Maximum Unicode scalars per query.
/// 每查询 Unicode 标量数上限。
pub const MAX_KNOWLEDGE_QUERY_CHARS: usize = 512;

/// Maximum page rows or search hits.
/// 分页行数或检索命中上限。
pub const MAX_KNOWLEDGE_PAGE_SIZE: u32 = 200;

/// Reciprocal-rank-fusion denominator.
/// 倒数排名融合分母。
pub const KNOWLEDGE_RRF_K: f64 = 60.0;

/// Default embedding batch bounded by model capabilities.
/// 受模型能力限制的默认嵌入批量。
pub const KNOWLEDGE_DEFAULT_EMBEDDING_BATCH_SIZE: u32 = 64;

/// Maximum physical inputs retained in one knowledge embedding batch.
/// 单个知识嵌入批次保留的物理输入数量上限。
pub const MAX_KNOWLEDGE_EMBEDDING_BATCH_SIZE: u32 = KNOWLEDGE_DEFAULT_EMBEDDING_BATCH_SIZE;

/// Portable package family.
/// 可移植套件家族。
pub const KNOWLEDGE_PACKAGE_FORMAT: &str = "vulcan-code-knowledge-base";

/// Exact accepted package version.
/// 唯一接受的套件版本。
pub const KNOWLEDGE_PACKAGE_VERSION: u32 = 3;

/// Maximum compressed or expanded package bytes.
/// 压缩或解压后套件字节上限。
pub const MAX_KNOWLEDGE_PACKAGE_BYTES: u64 = 512 * 1024 * 1024;

/// Library-owned indexing contract; no silent mode changes.
/// 库自身拥有的索引契约；不得静默改变模式。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeIndexMode {
    /// Directory and read access only.
    /// 仅目录和读取访问。
    Browse,
    /// Jieba full-text search.
    /// 结巴全文检索。
    Bm25,
    /// Vector search only.
    /// 仅向量检索。
    Vector,
    /// Full-text and vector rank fusion.
    /// 全文与向量排名融合。
    Hybrid,
}

impl KnowledgeIndexMode {
    /// Returns whether this mode requires full-text indexing.
    /// 返回该模式是否需要全文索引。
    pub const fn has_fts(self) -> bool {
        matches!(self, Self::Bm25 | Self::Hybrid)
    }
    /// Returns whether this mode requires embeddings.
    /// 返回该模式是否需要嵌入。
    pub const fn has_vectors(self) -> bool {
        matches!(self, Self::Vector | Self::Hybrid)
    }
}

/// Exact route binding family.
/// 精确路由绑定家族。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeRouteBindingKind {
    /// Declared protocol profile route.
    /// 声明的协议配置路由。
    Protocol,
    /// Declared provider-native action route.
    /// 声明的供应商原生动作路由。
    Action,
}

impl KnowledgeRouteBindingKind {
    /// Returns the stable binding identifier.
    /// 返回稳定绑定标识。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Protocol => "protocol",
            Self::Action => "action",
        }
    }
}

/// Exact frozen provider route, without alternate probing.
/// 精确冻结的供应商路由，不探测替代路由。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeRouteReference {
    /// Provider identity.
    /// 供应商身份。
    pub provider_id: String,
    /// Declared protocol.
    /// 声明协议。
    pub protocol: String,
    /// Declared binding family.
    /// 声明绑定家族。
    pub binding_kind: KnowledgeRouteBindingKind,
    /// Exact service entry.
    /// 精确服务入口。
    pub service_entry_id: String,
    /// Exact model identity.
    /// 精确模型身份。
    pub model_id: String,
    /// Authorized usage scope.
    /// 已授权使用范围。
    pub usage_scope: String,
    /// Optional credential connection.
    /// 可选凭据连接。
    pub connection_id: Option<String>,
}

/// Frozen vector-space request settings.
/// 冻结的向量空间请求设置。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeEmbeddingSettings {
    /// Frozen distance declared by the selected model capability.
    /// 选定模型能力声明的冻结距离。
    pub metric: KnowledgeVectorMetric,
    /// Route shared by document and query embeddings.
    /// 文档与查询嵌入共用路由。
    pub route: KnowledgeRouteReference,
    /// Validated model and capability fingerprint.
    /// 已验证模型与能力指纹。
    pub model_fingerprint: String,
    /// Frozen positive output dimensions.
    /// 冻结的正输出维度。
    pub dimensions: u32,
    /// Batch bounded by model capability.
    /// 受模型能力限制的批量。
    pub batch_size: u32,
}

/// Durable policy; cannot mutate content.
/// 持久策略，不能变更内容。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeLibrarySettings {
    /// Explicit index mode.
    /// 显式索引模式。
    pub index_mode: KnowledgeIndexMode,
    /// Required in vector modes and retained as a saved selection while they are disabled.
    /// 向量模式下必填，关闭向量模式时保留为已保存选择。
    pub embedding: Option<KnowledgeEmbeddingSettings>,
}

impl KnowledgeLibrarySettings {
    /// Returns the saved embedding only when the current mode permits vector work.
    /// 仅当当前模式允许向量工作时返回已保存的嵌入配置。
    pub fn enabled_embedding(&self) -> Option<&KnowledgeEmbeddingSettings> {
        self.embedding
            .as_ref()
            .filter(|_| self.index_mode.has_vectors())
    }

    /// Validates explicit settings; returns an English diagnostic for invalid mode or route data.
    /// 验证显式设置；模式或路由非法时返回英文诊断。
    pub fn validate(&self) -> Result<(), String> {
        if self.index_mode.has_vectors() && self.embedding.is_none() {
            return Err(
                "ERR_KNOWLEDGE_EMBEDDING_REQUIRED: vector modes require an embedding route".into(),
            );
        }
        if let Some(embedding) = &self.embedding {
            if embedding.dimensions == 0
                || embedding.batch_size == 0
                || embedding.batch_size > MAX_KNOWLEDGE_EMBEDDING_BATCH_SIZE
                || embedding.model_fingerprint.trim().is_empty()
            {
                return Err("ERR_KNOWLEDGE_EMBEDDING_INVALID: dimension, batch and fingerprint must be specified".into());
            }
            for value in [
                &embedding.route.provider_id,
                &embedding.route.protocol,
                &embedding.route.service_entry_id,
                &embedding.route.model_id,
                &embedding.route.usage_scope,
            ] {
                if value.trim().is_empty() {
                    return Err(
                        "ERR_KNOWLEDGE_ROUTE_INVALID: route identifiers must be nonempty".into(),
                    );
                }
            }
        }
        Ok(())
    }
}

/// Returns the host-owned inference namespace for the exact `library_id`.
/// 返回精确 `library_id` 的宿主所有推理命名空间。
/// The result scopes derived space profiles; it never grants a route by itself.
/// 结果为派生空间配置档划定范围；其自身绝不授予路由。
pub fn knowledge_consumer_profile_id(library_id: &str) -> String {
    format!("{KNOWLEDGE_CONSUMER_PROFILE_PREFIX}{library_id}")
}

/// Registry projection; invalid libraries expose their reason and deletion only.
/// 注册投影；非法知识库只暴露原因与删除。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeLibrarySummary {
    /// Immutable library UUID.
    /// 不可变知识库 UUID。
    pub id: String,
    /// Human-maintained name.
    /// 人工维护名称。
    pub name: String,
    /// Purpose guiding AI decisions.
    /// 指导 AI 决策的用途。
    pub description: String,
    /// Validated policy; absent on invalid formats.
    /// 已验证策略；非法格式为空。
    pub settings: Option<KnowledgeLibrarySettings>,
    /// Active, deleting or error.
    /// 活动、删除中或错误。
    pub lifecycle_state: String,
    /// Explicit format or integrity reason.
    /// 明确的格式或完整性原因。
    pub error_message: Option<String>,
    /// Current document count.
    /// 当前文档数。
    pub document_count: u64,
    /// Current chunk count.
    /// 当前切块数。
    pub chunk_count: u64,
    /// Documents requiring summary work.
    /// 需要处理摘要的文档数。
    pub pending_summary_count: u64,
    /// Derived full-text status.
    /// 派生全文状态。
    pub index_status: String,
    /// Last content change.
    /// 最后内容变更。
    pub head_change_seq: u64,
    /// Last content commit time.
    /// 最后内容提交时间。
    pub last_change_at_ms: Option<i64>,
    /// Registry creation time.
    /// 注册创建时间。
    pub created_at_ms: i64,
    /// Registry update time.
    /// 注册更新时间。
    pub updated_at_ms: i64,
}

/// Explicit project authorization.
/// 显式项目授权。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeProjectBinding {
    /// Authorized library.
    /// 已授权知识库。
    pub library_id: String,
    /// Owning project.
    /// 所属项目。
    pub project_id: String,
    /// Inherited enablement.
    /// 继承启用状态。
    pub default_enabled: bool,
}

/// Per-document vector inclusion policy.
/// 每文档向量收录策略。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeEmbedPolicy {
    /// Summary only.
    /// 仅摘要。
    #[default]
    Summary,
    /// Summary and body.
    /// 摘要和正文。
    Full,
    /// Exclude all chunks.
    /// 排除全部切块。
    None,
}

/// Observed source evidence.
/// 已观测来源证据。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeProvenance {
    /// File, URL or session discriminator.
    /// 文件、网址或会话判别标识。
    pub kind: String,
    /// Exact source identity.
    /// 精确来源身份。
    #[serde(rename = "ref")]
    pub reference: String,
    /// Observed original-byte digest.
    /// 已观测原字节摘要。
    pub sha256: Option<String>,
    /// Observation time.
    /// 观测时间。
    pub observed_at_ms: i64,
}

/// Complete current document and history image; body is content addressed.
/// 完整当前文档与历史镜像；正文采用内容寻址。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeDocumentSummary {
    /// Stable document UUID across moves.
    /// 移动后稳定的文档 UUID。
    pub id: String,
    /// NFC relative path.
    /// NFC 相对路径。
    pub path: String,
    /// Case-insensitive comparison key.
    /// 忽略大小写的比较键。
    pub path_key: String,
    /// Parent directory.
    /// 父目录。
    pub folder_path: String,
    /// AI-maintained title.
    /// AI 维护标题。
    pub title: String,
    /// AI-maintained summary.
    /// AI 维护摘要。
    pub summary: String,
    /// Provided, pending or stale.
    /// 已提供、待补充或待复核。
    pub summary_state: String,
    /// AI-maintained keywords.
    /// AI 维护关键词。
    pub keywords: Vec<String>,
    /// Immutable body digest shared by history.
    /// 与历史共享的不可变正文摘要。
    pub body_sha256: String,
    /// Logical body line count.
    /// 正文逻辑行数。
    pub line_count: usize,
    /// Vector inclusion policy.
    /// 向量收录策略。
    pub embed_policy: KnowledgeEmbedPolicy,
    /// Source observations.
    /// 来源观测。
    pub provenance: Vec<KnowledgeProvenance>,
    /// Monotonic optimistic token, including restores.
    /// 包括还原在内的单调乐观令牌。
    pub revision: u64,
    /// Creation change.
    /// 创建变更。
    pub created_change_seq: u64,
    /// Latest update.
    /// 最新更新。
    pub updated_change_seq: u64,
}

/// Read-only exact body window with authoritative metadata.
/// 只读精确正文窗口与权威元数据。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeDocumentContent {
    /// Current document.
    /// 当前文档。
    pub document: KnowledgeDocumentSummary,
    /// Exact requested text window.
    /// 请求的精确文本窗口。
    pub content: String,
    /// First inclusive line.
    /// 包含边界首行。
    pub start_line: usize,
    /// Last inclusive line.
    /// 包含边界末行。
    pub end_line: usize,
    /// Continuation if truncated.
    /// 截断时续读行。
    pub next_line: Option<usize>,
}

/// Explicit directory metadata; implicit folders derive from paths.
/// 显式目录元数据；隐式目录从路径派生。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeFolder {
    /// Comparison identity.
    /// 比较身份。
    pub path_key: String,
    /// NFC display path.
    /// NFC 显示路径。
    pub path: String,
    /// AI-maintained purpose.
    /// AI 维护用途。
    pub description: String,
    /// Creation change.
    /// 创建变更。
    pub created_change_seq: u64,
    /// Last update.
    /// 最近更新。
    pub updated_change_seq: u64,
}

/// Directory or document entry with stable sorting identity.
/// 具有稳定排序身份的目录或文档条目。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeListEntry {
    /// Folder or document discriminator.
    /// 目录或文档判别标识。
    pub kind: String,
    /// Canonical display path.
    /// 规范显示路径。
    pub path: String,
    /// Directory purpose.
    /// 目录用途。
    pub description: String,
    /// Descendant documents.
    /// 后代文档数。
    pub document_count: usize,
    /// Document metadata for document entries.
    /// 文档条目的文档元数据。
    pub document: Option<KnowledgeDocumentSummary>,
}

/// Directory page tied to a content head; stale cursors are rejected.
/// 绑定内容变更头的目录页；拒绝过期游标。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeListPage {
    /// Returned entries.
    /// 返回条目。
    pub entries: Vec<KnowledgeListEntry>,
    /// Opaque continuation.
    /// 不透明续读标识。
    pub next_cursor: Option<String>,
    /// Observed content head.
    /// 观测到的内容头。
    pub head_change_seq: u64,
}

/// Actual authenticated invocation identity.
/// 实际已认证调用身份。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeChangeActor {
    /// AI, user or system.
    /// AI、用户或系统。
    pub kind: String,
    /// Executing root or child session.
    /// 执行的根会话或子会话。
    pub session_id: Option<String>,
    /// Actual turn identity.
    /// 实际回合身份。
    pub turn_id: Option<String>,
    /// Actual tool call identity.
    /// 实际工具调用身份。
    pub tool_call_id: Option<String>,
}

/// Immutable history summary.
/// 不可变历史摘要。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeChange {
    /// Monotonic sequence.
    /// 单调序号。
    pub seq: u64,
    /// Commit time.
    /// 提交时间。
    pub at_ms: i64,
    /// Authenticated actor.
    /// 已认证执行者。
    pub actor: KnowledgeChangeActor,
    /// Explicit operation.
    /// 显式操作。
    pub operation: String,
    /// Auditable intent.
    /// 可审计意图。
    pub note: String,
    /// Target of a restore.
    /// 还原目标。
    pub restore_target_seq: Option<u64>,
    /// Changed entity count.
    /// 变更实体数。
    pub item_count: usize,
}

/// Exact row images without repeated body bytes.
/// 不重复正文字节的精确行镜像。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeChangeItem {
    /// Document or folder.
    /// 文档或目录。
    pub entity: String,
    /// Stable document ID or folder key.
    /// 稳定文档身份或目录键。
    pub entity_key: String,
    /// Complete prior row.
    /// 完整前一记录。
    pub before: Option<serde_json::Value>,
    /// Complete next row.
    /// 完整后一记录。
    pub after: Option<serde_json::Value>,
}

/// History detail and ordered atomic images.
/// 历史详情与有序原子镜像。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeChangeDetail {
    /// Summary.
    /// 摘要。
    pub change: KnowledgeChange,
    /// Ordered images.
    /// 有序镜像。
    pub items: Vec<KnowledgeChangeItem>,
    /// Exclusive next item offset for this immutable change.
    /// 此不可变变更的排他下一条目偏移。
    pub next_offset: Option<u64>,
}

/// Exact side of one immutable history image.
/// 单个不可变历史镜像的精确侧别。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeChangeSide {
    /// State immediately before the change.
    /// 变更紧前状态。
    Before,
    /// State immediately after the change.
    /// 变更紧后状态。
    After,
}

/// Exact literal edit checked before committing.
/// 提交前校验的精确字面编辑。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeTextEdit {
    /// Nonempty literal match.
    /// 非空字面匹配。
    pub old_text: String,
    /// Replacement text.
    /// 替换文本。
    pub new_text: String,
    /// Explicitly permit multiple matches.
    /// 显式允许多个匹配。
    #[serde(default)]
    pub replace_all: bool,
}

/// Flat AI request; action validation rejects irrelevant fields.
/// 平铺 AI 请求；动作验证拒绝无关字段。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeWriteRequest {
    /// Create, update, edit, move, delete, folder or delete_folder.
    /// 创建、更新、编辑、移动、删除、目录或删除目录。
    pub action: String,
    /// Exact document selector.
    /// 精确文档选择器。
    pub document_id: Option<String>,
    /// Exact path selector or destination for creation.
    /// 精确路径选择器或创建目标。
    pub path: Option<String>,
    /// Required token for existing documents.
    /// 已有文档必填令牌。
    pub expected_revision: Option<u64>,
    /// Replacement title.
    /// 替换标题。
    pub title: Option<String>,
    /// Replacement summary.
    /// 替换摘要。
    pub summary: Option<String>,
    /// Replacement body.
    /// 替换正文。
    pub body: Option<String>,
    /// Replacement keywords.
    /// 替换关键词。
    pub keywords: Option<Vec<String>>,
    /// Replacement vector policy.
    /// 替换向量策略。
    pub embed: Option<KnowledgeEmbedPolicy>,
    /// Source observations.
    /// 来源观测。
    pub sources: Option<Vec<KnowledgeProvenance>>,
    /// Exact literal edits.
    /// 精确字面编辑。
    pub edits: Option<Vec<KnowledgeTextEdit>>,
    /// Move destination.
    /// 移动目标。
    pub new_path: Option<String>,
    /// Directory description.
    /// 目录说明。
    pub description: Option<String>,
    /// Auditable change intent.
    /// 可审计变更意图。
    #[serde(default)]
    pub note: String,
}

/// One decoded file accepted for host ingest without model round trips.
/// 宿主搬运接受的一份解码文件，不往返模型。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeIngestDocument {
    /// Validated target path.
    /// 已验证目标路径。
    pub path: String,
    /// Host-derived title.
    /// 宿主派生标题。
    pub title: String,
    /// Exact decoded body.
    /// 精确解码正文。
    pub body: String,
    /// Optional AI summary.
    /// 可选 AI 摘要。
    pub summary: Option<String>,
    /// Original file observation and digest.
    /// 原文件观测与摘要。
    pub provenance: KnowledgeProvenance,
    /// Vector inclusion.
    /// 向量收录。
    pub embed: KnowledgeEmbedPolicy,
}

/// Atomic mutation receipt.
/// 原子变更回执。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeWriteResult {
    /// New change; absent only for unchanged ingest.
    /// 新变更；仅无变化搬运为空。
    pub change_seq: Option<u64>,
    /// Affected surviving documents.
    /// 受影响且仍存在的文档。
    pub documents: Vec<KnowledgeDocumentSummary>,
    /// Unchanged ingest paths.
    /// 无变化搬运路径。
    pub unchanged: Vec<String>,
    /// Pending vector count.
    /// 待生成向量数。
    pub pending_embeddings: u64,
}

/// Human-maintained custom Jieba term; no document content is editable here.
/// 人工维护的自定义结巴词；此处不能编辑文档内容。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeCustomWord {
    /// Exact nonempty term.
    /// 精确非空词语。
    pub word: String,
    /// Positive dictionary weight.
    /// 正词典权重。
    pub weight: u32,
}

/// Current derived-index consistency; computed from exact current rows rather than registry counters.
/// 当前派生索引一致性；从精确当前行而非注册计数计算。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeIndexIntegritySummary {
    /// Exact library index mode.
    /// 精确知识库索引模式。
    pub index_mode: KnowledgeIndexMode,
    /// Current authoritative documents.
    /// 当前权威文档数。
    pub document_count: u64,
    /// Current authoritative chunks.
    /// 当前权威切块数。
    pub chunk_count: u64,
    /// Whether the FTS table exists.
    /// 全文表是否存在。
    pub fts_present: bool,
    /// Actual FTS rows.
    /// 实际全文行数。
    pub indexed_chunk_count: u64,
    /// Missing, stale, duplicate or orphan FTS rows.
    /// 缺失、过期、重复或孤立全文行数。
    pub mismatched_rows: u64,
    /// Current content head.
    /// 当前内容头。
    pub head_change_seq: u64,
    /// Indexed content head.
    /// 已索引内容头。
    pub indexed_change_seq: u64,
    /// Current custom dictionary revision.
    /// 当前自定义词典修订。
    pub dictionary_revision: u64,
    /// Indexed custom dictionary revision.
    /// 已索引自定义词典修订。
    pub indexed_dictionary_revision: u64,
    /// Whether all required derived state agrees with current authority.
    /// 全部必需派生状态是否与当前权威一致。
    pub healthy: bool,
}

/// Exact host embedding request for document batches or query vectors.
/// 文档批次或查询向量的精确宿主嵌入请求。
#[derive(Debug, Clone)]
pub struct KnowledgeEmbeddingRequest {
    /// Actual physical library owner, independent of portable route fields.
    /// 实际物理知识库所有者，独立于可移植路由字段。
    pub library_id: String,
    /// Frozen semantic route and vector space.
    /// 冻结语义路由与向量空间。
    pub embedding: KnowledgeEmbeddingSettings,
    /// Ordered decoded inputs.
    /// 有序解码输入。
    pub inputs: Vec<String>,
    /// Whether inputs are retrieval queries rather than stored documents.
    /// 输入是否为检索查询而非存储文档。
    pub is_query: bool,
}

/// Asynchronous host inference result; vectors correspond exactly to request order.
/// 异步宿主推理结果；向量与请求顺序精确对应。
pub type KnowledgeEmbeddingFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<Vec<f32>>, String>> + Send>>;
/// Registered host executor; tests install a fake and never call a paid service.
/// 已注册宿主执行器；测试安装假实现且不调用付费服务。
pub type KnowledgeEmbeddingExecutor =
    std::sync::Arc<dyn Fn(KnowledgeEmbeddingRequest) -> KnowledgeEmbeddingFuture + Send + Sync>;

/// Persistent background operation replacing directory-import jobs.
/// 替代目录导入任务的持久后台操作。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeOperationSummary {
    /// Stable operation UUID.
    /// 稳定操作 UUID。
    pub id: String,
    /// Owning library UUID.
    /// 所属知识库 UUID。
    pub library_id: String,
    /// Explicit index_mode, rebuild or retry kind.
    /// 显式模式、重建或重试类别。
    pub kind: String,
    /// Queued, running, completed, cancelled or failed.
    /// 排队、运行、完成、取消或失败。
    pub state: String,
    /// Current processing phase.
    /// 当前处理阶段。
    pub phase: String,
    /// Current total required content hashes or FTS chunks.
    /// 当前必需内容哈希或全文切块总数。
    pub total_items: u64,
    /// Successfully processed items.
    /// 已成功处理条目。
    pub processed_items: u64,
    /// Current content identity when processing.
    /// 处理中的当前内容身份。
    pub current_item: Option<String>,
    /// Explicit warnings.
    /// 明确告警。
    pub warnings: Vec<String>,
    /// English background failure diagnostic.
    /// 英文后台失败诊断。
    pub error_message: Option<String>,
    /// Queued time.
    /// 排队时间。
    pub created_at_ms: i64,
    /// First execution time.
    /// 首次执行时间。
    pub started_at_ms: Option<i64>,
    /// Terminal time.
    /// 终止时间。
    pub finished_at_ms: Option<i64>,
}

/// Failure projection links current documents to an embedding hash.
/// 失败投影将当前文档关联到嵌入哈希。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeEmbeddingFailure {
    /// Current document UUID.
    /// 当前文档 UUID。
    pub document_id: String,
    /// Current document path.
    /// 当前文档路径。
    pub path: String,
    /// Current chunk UUID.
    /// 当前切块 UUID。
    pub chunk_id: String,
    /// Content-addressed input hash.
    /// 内容寻址输入哈希。
    pub content_sha256: String,
    /// Attempts already performed.
    /// 已执行尝试数。
    pub attempts: u32,
    /// English error retained for human retry.
    /// 保留供人工重试的英文错误。
    pub last_error: String,
}

/// Current desired space progress and independently active search space.
/// 当前期望空间进度与独立活动检索空间。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeEmbeddingStatus {
    /// Desired vector-space identity.
    /// 期望向量空间身份。
    pub desired_space_id: Option<String>,
    /// Currently active search space.
    /// 当前活动检索空间。
    pub active_space_id: Option<String>,
    /// Pending current content hashes.
    /// 待生成当前内容哈希数。
    pub pending: u64,
    /// Ready current content hashes.
    /// 就绪当前内容哈希数。
    pub ready: u64,
    /// Exhausted failed current content hashes.
    /// 耗尽重试的失败当前内容哈希数。
    pub failed: u64,
    /// Whether automatic work was explicitly cancelled.
    /// 自动工作是否被显式取消。
    pub paused: bool,
    /// Bounded failure detail page.
    /// 有界失败详情页。
    pub failures: Vec<KnowledgeEmbeddingFailure>,
    /// Exclusive next failure offset.
    /// 排他下一失败偏移。
    pub next_offset: Option<u64>,
}

/// One current chunk matched by the library's declared search capabilities.
/// 被库声明检索能力匹配的一个当前切块。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeSearchHit {
    /// Owning library UUID.
    /// 所属知识库 UUID。
    pub library_id: String,
    /// Human library name.
    /// 人工库名称。
    pub library_name: String,
    /// Stable current document UUID.
    /// 稳定当前文档 UUID。
    pub document_id: String,
    /// Exact document revision ranked and projected by this result.
    /// 此结果排名及投影的精确文档修订。
    pub revision: u64,
    /// Exact chunk content hash used to isolate concurrent query generations.
    /// 用于隔离并发查询代次的精确切块内容哈希。
    pub content_sha256: String,
    /// Current document path.
    /// 当前文档路径。
    pub path: String,
    /// Current AI title.
    /// 当前 AI 标题。
    pub title: String,
    /// Current AI summary.
    /// 当前 AI 摘要。
    pub summary: String,
    /// Exact current chunk identity.
    /// 精确当前切块身份。
    pub chunk_id: String,
    /// Summary or body discriminator.
    /// 摘要或正文判别标识。
    pub kind: String,
    /// Current heading ancestry.
    /// 当前标题层级。
    pub heading_path: String,
    /// Inclusive document first line; summary chunks use zero.
    /// 包含边界的文档首行；摘要切块为零。
    pub start_line: usize,
    /// Inclusive document last line.
    /// 包含边界的文档末行。
    pub end_line: usize,
    /// Untrusted reference content.
    /// 不可信参考内容。
    pub content: String,
    /// Reciprocal-rank fusion score.
    /// 倒数排名融合分数。
    pub score: f64,
    /// Explicit BM25 or vector contributing capabilities.
    /// 明确贡献的全文或向量能力。
    pub match_kinds: Vec<String>,
}

/// Declared capability state accompanying every searched library.
/// 伴随每个检索库返回的声明能力状态。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeSearchLibraryStatus {
    /// Library UUID.
    /// 知识库 UUID。
    pub library_id: String,
    /// Exact declared mode.
    /// 精确声明模式。
    pub index_mode: KnowledgeIndexMode,
    /// Ready, building or disabled FTS capability.
    /// 就绪、构建中或禁用的全文能力。
    pub fts_status: String,
    /// Current embedding progress and active/desired space distinction.
    /// 当前嵌入进度以及活动和期望空间区分。
    pub embeddings: KnowledgeEmbeddingStatus,
}

/// Explicit per-library query failure; partial hybrid results never hide this diagnostic.
/// 明确每库查询失败；部分混合结果绝不隐藏该诊断。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeSearchFailure {
    /// Exact failed library.
    /// 精确失败知识库。
    pub library_id: String,
    /// Query that failed or no query for library admission.
    /// 失败查询；库准入失败时为空。
    pub query: Option<String>,
    /// English capability or inference error.
    /// 英文能力或推理错误。
    pub error: String,
}

/// Federated search result with explicit mode and backlog status for every participating library.
/// 联合检索结果，包含每个参与库的明确模式和积压状态。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeSearchResult {
    /// Exact queries evaluated.
    /// 已评估精确查询。
    pub queries: Vec<String>,
    /// Ranked current chunks.
    /// 已排名当前切块。
    pub hits: Vec<KnowledgeSearchHit>,
    /// Explicit per-library capability state.
    /// 明确每库能力状态。
    pub libraries: Vec<KnowledgeSearchLibraryStatus>,
    /// Visible partial failures.
    /// 可见部分失败。
    pub failures: Vec<KnowledgeSearchFailure>,
}

/// Maximum logical folder/document records in one portable package.
/// 单个可移植套件的逻辑目录与文档记录数上限。
pub const MAX_KNOWLEDGE_PACKAGE_ENTRIES: usize = 10_000;
/// Independent bound on the portable JSON manifest.
/// 可移植 JSON 清单的独立大小上限。
pub const MAX_KNOWLEDGE_PACKAGE_MANIFEST_BYTES: u64 = 1024 * 1024;

/// Portable current folder content, excluding audit sequence metadata.
/// 可移植当前目录内容，排除审计序号元数据。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgePackageFolder {
    /// Exact canonical relative path.
    /// 精确规范相对路径。
    pub path: String,
    /// AI-maintained directory purpose.
    /// AI 维护的目录用途。
    pub description: String,
}

/// Portable document content; stable local IDs, revisions and derived indexes are excluded.
/// 可移植文档内容；排除本地稳定身份、修订与派生索引。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgePackageDocument {
    /// Exact canonical relative path.
    /// 精确规范相对路径。
    pub path: String,
    /// AI-maintained title.
    /// AI 维护的标题。
    pub title: String,
    /// Exact current summary.
    /// 精确当前摘要。
    pub summary: String,
    /// Provided, pending or stale summary state.
    /// 已提供、待补充或待复核摘要状态。
    pub summary_state: String,
    /// AI-maintained keywords.
    /// AI 维护的关键词。
    pub keywords: Vec<String>,
    /// Exact UTF-8 body, preserving source separators.
    /// 精确 UTF-8 正文，保留来源分隔符。
    pub body: String,
    /// Declared vector inclusion policy.
    /// 声明的向量收录策略。
    pub embed_policy: KnowledgeEmbedPolicy,
    /// Exact source observations.
    /// 精确来源观测。
    pub provenance: Vec<KnowledgeProvenance>,
}

/// One strict JSONL entity record; content is never executed or treated as instructions.
/// 一条严格 JSONL 实体记录；内容绝不执行或作为指令。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "entity", rename_all = "snake_case")]
pub enum KnowledgePackageRecord {
    /// Explicit directory metadata.
    /// 显式目录元数据。
    Folder(KnowledgePackageFolder),
    /// Current document content.
    /// 当前文档内容。
    Document(KnowledgePackageDocument),
}

/// Exact current portable format manifest; previous versions are refused before content decoding.
/// 精确当前可移植格式清单；内容解码前拒绝旧版本。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgePackageManifest {
    /// Stable package family.
    /// 稳定套件家族。
    pub format: String,
    /// Exact supported format version.
    /// 精确支持的格式版本。
    pub version: u32,
    /// Human library name.
    /// 人工知识库名称。
    pub name: String,
    /// Human library purpose.
    /// 人工知识库用途。
    pub description: String,
    /// Frozen current indexing configuration.
    /// 冻结当前索引配置。
    pub settings: KnowledgeLibrarySettings,
    /// Current custom dictionary configuration.
    /// 当前自定义词典配置。
    pub custom_words: Vec<KnowledgeCustomWord>,
    /// Exact document record count.
    /// 精确文档记录数。
    pub document_count: usize,
    /// Exact explicit folder record count.
    /// 精确显式目录记录数。
    pub folder_count: usize,
    /// Digest of exact uncompressed content.jsonl bytes.
    /// 精确未压缩 content.jsonl 字节的摘要。
    pub content_sha256: String,
    /// Host export timestamp.
    /// 宿主导出时间戳。
    pub exported_at_ms: i64,
}

/// Frozen inspection identity used to prevent import-after-preview races.
/// 防止预检后导入竞态的冻结检查身份。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgePackageInspection {
    /// Exact canonical archive path.
    /// 精确规范归档路径。
    pub archive_path: String,
    /// Digest of the complete verified archive.
    /// 完整已验证归档的摘要。
    pub sha256: String,
    /// Actual compressed archive size.
    /// 实际压缩归档大小。
    pub byte_size: u64,
    /// Strict validated current-format manifest.
    /// 严格验证的当前格式清单。
    pub manifest: KnowledgePackageManifest,
}

/// Atomic export receipt, sharing the exact inspected package identity.
/// 原子导出回执，共享精确已检查套件身份。
pub type KnowledgePackageExportResult = KnowledgePackageInspection;

/// Successful new-library import receipt; history begins at its sole import change.
/// 成功新库导入回执；历史从唯一导入变更开始。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgePackageImportResult {
    /// Newly created library and derived progress.
    /// 新建知识库与派生进度。
    pub library: KnowledgeLibrarySummary,
    /// Sole initial content change.
    /// 唯一初始内容变更。
    pub change_seq: u64,
}
