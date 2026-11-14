import type { PackageFileListing, PackageFileSummary } from "./projects-files";

// Declared library modes are the sole search policy.
// 声明的库模式为唯一检索策略。
export type KnowledgeIndexMode = "browse" | "bm25" | "vector" | "hybrid";
// The selected model explicitly declares the vector distance.
// 选定模型明确声明向量距离。
export type KnowledgeVectorMetric = "euclidean" | "cosine" | "dot_product";
// Binding identity selects exactly one protocol or action family.
// 绑定身份精确选择协议或动作家族。
export type KnowledgeRouteBindingKind = "protocol" | "action";
// Per-document inclusion is independent of library mode.
// 每文档收录独立于库模式。
export type KnowledgeEmbedPolicy = "summary" | "full" | "none";
// History side refers to immutable stored row images.
// 历史侧别指向不可变存储行镜像。
export type KnowledgeChangeSide = "before" | "after";
// Session selection carries authorization inheritance only.
// 会话选择仅携带授权继承。
export type KnowledgeLibrarySelectionMode = "inherit" | "explicit";
// Request enums mirror their same-semantics response declarations.
// 请求枚举镜像相同语义响应声明。
export type KnowledgeIndexModeRequest = KnowledgeIndexMode;
// Browser requests use the same closed binding family.
// 浏览器请求使用相同封闭绑定家族。
export type KnowledgeRouteBindingKindRequest = KnowledgeRouteBindingKind;
// Historical read requests use the same exact side discriminator.
// 历史读取请求使用相同精确侧别判别标识。
export type KnowledgeChangeSideRequest = KnowledgeChangeSide;

// Exact frozen provider route, without alternate probing.
// 精确冻结的供应商路由，不探测替代路由。
export interface KnowledgeRouteReference {
  // Provider identity.
  // 供应商身份。
  providerId: string;
  // Declared protocol.
  // 声明协议。
  protocol: string;
  // Declared binding family.
  // 声明绑定家族。
  bindingKind: KnowledgeRouteBindingKind;
  // Exact service entry.
  // 精确服务入口。
  serviceEntryId: string;
  // Exact model identity.
  // 精确模型身份。
  modelId: string;
  // Authorized usage scope.
  // 已授权使用范围。
  usageScope: string;
  // Optional credential connection.
  // 可选凭据连接。
  connectionId: string | null;
}

// Frozen vector-space request settings.
// 冻结的向量空间请求设置。
export interface KnowledgeEmbeddingSettings {
  // Frozen distance declared by the selected model capability.
  // 选定模型能力声明的冻结距离。
  metric: KnowledgeVectorMetric;
  // Route shared by document and query embeddings.
  // 文档与查询嵌入共用路由。
  route: KnowledgeRouteReference;
  // Validated model and capability fingerprint.
  // 已验证模型与能力指纹。
  modelFingerprint: string;
  // Frozen positive output dimensions.
  // 冻结的正输出维度。
  dimensions: number;
  // Batch bounded by model capability.
  // 受模型能力限制的批量。
  batchSize: number;
}

// Durable policy; cannot mutate content.
// 持久策略，不能变更内容。
export interface KnowledgeLibrarySettings {
  // Explicit index mode.
  // 显式索引模式。
  indexMode: KnowledgeIndexMode;
  // Required in vector modes and retained as a saved selection while they are disabled.
  // 向量模式下必填，关闭向量模式时保留为已保存选择。
  embedding: KnowledgeEmbeddingSettings | null;
}

// Registry projection; invalid libraries expose their reason and deletion only.
// 注册投影；非法知识库只暴露原因与删除。
export interface KnowledgeLibrarySummary {
  // Immutable library UUID.
  // 不可变知识库 UUID。
  id: string;
  // Human-maintained name.
  // 人工维护名称。
  name: string;
  // Purpose guiding AI decisions.
  // 指导 AI 决策的用途。
  description: string;
  // Validated policy; absent on invalid formats.
  // 已验证策略；非法格式为空。
  settings: KnowledgeLibrarySettings | null;
  // Active, deleting or error.
  // 活动、删除中或错误。
  lifecycleState: string;
  // Explicit format or integrity reason.
  // 明确的格式或完整性原因。
  errorMessage: string | null;
  // Current document count.
  // 当前文档数。
  documentCount: number;
  // Current chunk count.
  // 当前切块数。
  chunkCount: number;
  // Documents requiring summary work.
  // 需要处理摘要的文档数。
  pendingSummaryCount: number;
  // Derived full-text status.
  // 派生全文状态。
  indexStatus: string;
  // Last content change.
  // 最后内容变更。
  headChangeSeq: number;
  // Last content commit time.
  // 最后内容提交时间。
  lastChangeAtMs: number | null;
  // Registry creation time.
  // 注册创建时间。
  createdAtMs: number;
  // Registry update time.
  // 注册更新时间。
  updatedAtMs: number;
}

// Explicit project authorization.
// 显式项目授权。
export interface KnowledgeProjectBinding {
  // Authorized library.
  // 已授权知识库。
  libraryId: string;
  // Owning project.
  // 所属项目。
  projectId: string;
  // Inherited enablement.
  // 继承启用状态。
  defaultEnabled: boolean;
}

// Observed source evidence.
// 已观测来源证据。
export interface KnowledgeProvenance {
  // File, URL or session discriminator.
  // 文件、网址或会话判别标识。
  kind: string;
  // Exact source identity.
  // 精确来源身份。
  ref: string;
  // Observed original-byte digest.
  // 已观测原字节摘要。
  sha256: string | null;
  // Observation time.
  // 观测时间。
  observedAtMs: number;
}

// Complete current document and history image; body is content addressed.
// 完整当前文档与历史镜像；正文采用内容寻址。
export interface KnowledgeDocumentSummary {
  // Stable document UUID across moves.
  // 移动后稳定的文档 UUID。
  id: string;
  // NFC relative path.
  // NFC 相对路径。
  path: string;
  // Case-insensitive comparison key.
  // 忽略大小写的比较键。
  pathKey: string;
  // Parent directory.
  // 父目录。
  folderPath: string;
  // AI-maintained title.
  // AI 维护标题。
  title: string;
  // AI-maintained summary.
  // AI 维护摘要。
  summary: string;
  // Provided, pending or stale.
  // 已提供、待补充或待复核。
  summaryState: string;
  // AI-maintained keywords.
  // AI 维护关键词。
  keywords: string[];
  // Immutable body digest shared by history.
  // 与历史共享的不可变正文摘要。
  bodySha256: string;
  // Logical body line count.
  // 正文逻辑行数。
  lineCount: number;
  // Vector inclusion policy.
  // 向量收录策略。
  embedPolicy: KnowledgeEmbedPolicy;
  // Source observations.
  // 来源观测。
  provenance: KnowledgeProvenance[];
  // Monotonic optimistic token, including restores.
  // 包括还原在内的单调乐观令牌。
  revision: number;
  // Creation change.
  // 创建变更。
  createdChangeSeq: number;
  // Latest update.
  // 最新更新。
  updatedChangeSeq: number;
}

// Read-only exact body window with authoritative metadata.
// 只读精确正文窗口与权威元数据。
export interface KnowledgeDocumentContent {
  // Current document.
  // 当前文档。
  document: KnowledgeDocumentSummary;
  // Exact requested text window.
  // 请求的精确文本窗口。
  content: string;
  // First inclusive line.
  // 包含边界首行。
  startLine: number;
  // Last inclusive line.
  // 包含边界末行。
  endLine: number;
  // Continuation if truncated.
  // 截断时续读行。
  nextLine: number | null;
}

// Explicit directory metadata; implicit folders derive from paths.
// 显式目录元数据；隐式目录从路径派生。
export interface KnowledgeFolder {
  // Comparison identity.
  // 比较身份。
  pathKey: string;
  // NFC display path.
  // NFC 显示路径。
  path: string;
  // AI-maintained purpose.
  // AI 维护用途。
  description: string;
  // Creation change.
  // 创建变更。
  createdChangeSeq: number;
  // Last update.
  // 最近更新。
  updatedChangeSeq: number;
}

// Shared directory fields emitted by current list_content, without loading bodies.
// 当前 list_content 产出的共享目录字段，不加载正文。
interface KnowledgeListEntryFields {
  // Canonical display path.
  // 规范显示路径。
  path: string;
  // AI-maintained folder purpose.
  // AI 维护的目录用途。
  description: string;
  // Descendant document count.
  // 后代文档数量。
  documentCount: number;
}
// list_content explicitly pairs folder/null and document/Some(metadata).
// list_content 明确配对目录及空值、文档及非空元数据。
export type KnowledgeListEntry = KnowledgeListEntryFields & (
  { kind: "folder"; document: null } | { kind: "document"; document: KnowledgeDocumentSummary }
);

// Directory page tied to a content head; stale cursors are rejected.
// 绑定内容变更头的目录页；拒绝过期游标。
export interface KnowledgeListPage {
  // Returned entries.
  // 返回条目。
  entries: KnowledgeListEntry[];
  // Opaque continuation.
  // 不透明续读标识。
  nextCursor: string | null;
  // Observed content head.
  // 观测到的内容头。
  headChangeSeq: number;
}

// Actual authenticated invocation identity.
// 实际已认证调用身份。
export interface KnowledgeChangeActor {
  // AI, user or system.
  // AI、用户或系统。
  kind: string;
  // Executing root or child session.
  // 执行的根会话或子会话。
  sessionId: string | null;
  // Actual turn identity.
  // 实际回合身份。
  turnId: string | null;
  // Actual tool call identity.
  // 实际工具调用身份。
  toolCallId: string | null;
}

// Immutable history summary.
// 不可变历史摘要。
export interface KnowledgeChange {
  // Monotonic sequence.
  // 单调序号。
  seq: number;
  // Commit time.
  // 提交时间。
  atMs: number;
  // Authenticated actor.
  // 已认证执行者。
  actor: KnowledgeChangeActor;
  // Explicit operation.
  // 显式操作。
  operation: string;
  // Auditable intent.
  // 可审计意图。
  note: string;
  // Target of a restore.
  // 还原目标。
  restoreTargetSeq: number | null;
  // Changed entity count.
  // 变更实体数。
  itemCount: number;
}

// Parameters: Kind is the stored entity discriminator; Image is its exact complete row type.
// 参数：Kind 为存储实体判别标识；Image 为其精确完整行类型。
interface KnowledgeChangeImages<Kind extends string, Image> {
  // Exact entity kind written by commit_snapshot.
  // commit_snapshot 写入的精确实体类别。
  entity: Kind;
  // Stable document UUID or normalized folder key.
  // 稳定文档 UUID 或规范目录键。
  entityKey: string;
  // Complete pre-image, absent for creation.
  // 完整前镜像，创建时为空。
  before: Image | null;
  // Complete post-image, absent for deletion.
  // 完整后镜像，删除时为空。
  after: Image | null;
}
// commit_snapshot serializes exactly Documents and Folders into their matching entity rows.
// commit_snapshot 将精确 Documents 与 Folders 序列化到对应实体行。
export type KnowledgeChangeItem = KnowledgeChangeImages<"document", KnowledgeDocumentSummary>
  | KnowledgeChangeImages<"folder", KnowledgeFolder>;

// History detail and ordered atomic images.
// 历史详情与有序原子镜像。
export interface KnowledgeChangeDetail {
  // Summary.
  // 摘要。
  change: KnowledgeChange;
  // Ordered images.
  // 有序镜像。
  items: KnowledgeChangeItem[];
  // Exclusive next item offset for this immutable change.
  // 此不可变变更的排他下一条目偏移。
  nextOffset: number | null;
}

// Human-maintained custom Jieba term; no document content is editable here.
// 人工维护的自定义结巴词；此处不能编辑文档内容。
export interface KnowledgeCustomWord {
  // Exact nonempty term.
  // 精确非空词语。
  word: string;
  // Positive dictionary weight.
  // 正词典权重。
  weight: number;
}

// Current derived-index consistency; computed from exact current rows rather than registry counters.
// 当前派生索引一致性；从精确当前行而非注册计数计算。
export interface KnowledgeIndexIntegritySummary {
  // Exact library index mode.
  // 精确知识库索引模式。
  indexMode: KnowledgeIndexMode;
  // Current authoritative documents.
  // 当前权威文档数。
  documentCount: number;
  // Current authoritative chunks.
  // 当前权威切块数。
  chunkCount: number;
  // Whether the FTS table exists.
  // 全文表是否存在。
  ftsPresent: boolean;
  // Actual FTS rows.
  // 实际全文行数。
  indexedChunkCount: number;
  // Missing, stale, duplicate or orphan FTS rows.
  // 缺失、过期、重复或孤立全文行数。
  mismatchedRows: number;
  // Current content head.
  // 当前内容头。
  headChangeSeq: number;
  // Indexed content head.
  // 已索引内容头。
  indexedChangeSeq: number;
  // Current custom dictionary revision.
  // 当前自定义词典修订。
  dictionaryRevision: number;
  // Indexed custom dictionary revision.
  // 已索引自定义词典修订。
  indexedDictionaryRevision: number;
  // Whether all required derived state agrees with current authority.
  // 全部必需派生状态是否与当前权威一致。
  healthy: boolean;
}

// Persistent background operation replacing directory-import jobs.
// 替代目录导入任务的持久后台操作。
export interface KnowledgeOperationSummary {
  // Stable operation UUID.
  // 稳定操作 UUID。
  id: string;
  // Owning library UUID.
  // 所属知识库 UUID。
  libraryId: string;
  // Explicit index_mode, rebuild or retry kind.
  // 显式模式、重建或重试类别。
  kind: string;
  // Queued, running, completed, cancelled or failed.
  // 排队、运行、完成、取消或失败。
  state: string;
  // Current processing phase.
  // 当前处理阶段。
  phase: string;
  // Current total required content hashes or FTS chunks.
  // 当前必需内容哈希或全文切块总数。
  totalItems: number;
  // Successfully processed items.
  // 已成功处理条目。
  processedItems: number;
  // Current content identity when processing.
  // 处理中的当前内容身份。
  currentItem: string | null;
  // Explicit warnings.
  // 明确告警。
  warnings: string[];
  // English background failure diagnostic.
  // 英文后台失败诊断。
  errorMessage: string | null;
  // Queued time.
  // 排队时间。
  createdAtMs: number;
  // First execution time.
  // 首次执行时间。
  startedAtMs: number | null;
  // Terminal time.
  // 终止时间。
  finishedAtMs: number | null;
}

// Failure projection links current documents to an embedding hash.
// 失败投影将当前文档关联到嵌入哈希。
export interface KnowledgeEmbeddingFailure {
  // Current document UUID.
  // 当前文档 UUID。
  documentId: string;
  // Current document path.
  // 当前文档路径。
  path: string;
  // Current chunk UUID.
  // 当前切块 UUID。
  chunkId: string;
  // Content-addressed input hash.
  // 内容寻址输入哈希。
  contentSha256: string;
  // Attempts already performed.
  // 已执行尝试数。
  attempts: number;
  // English error retained for human retry.
  // 保留供人工重试的英文错误。
  lastError: string;
}

// Current desired space progress and independently active search space.
// 当前期望空间进度与独立活动检索空间。
export interface KnowledgeEmbeddingStatus {
  // Desired vector-space identity.
  // 期望向量空间身份。
  desiredSpaceId: string | null;
  // Currently active search space.
  // 当前活动检索空间。
  activeSpaceId: string | null;
  // Pending current content hashes.
  // 待生成当前内容哈希数。
  pending: number;
  // Ready current content hashes.
  // 就绪当前内容哈希数。
  ready: number;
  // Exhausted failed current content hashes.
  // 耗尽重试的失败当前内容哈希数。
  failed: number;
  // Whether automatic work was explicitly cancelled.
  // 自动工作是否被显式取消。
  paused: boolean;
  // Bounded failure detail page.
  // 有界失败详情页。
  failures: KnowledgeEmbeddingFailure[];
  // Exclusive next failure offset.
  // 排他下一失败偏移。
  nextOffset: number | null;
}

// One current chunk matched by the library's declared search capabilities.
// 被库声明检索能力匹配的一个当前切块。
export interface KnowledgeSearchHit {
  // Owning library UUID.
  // 所属知识库 UUID。
  libraryId: string;
  // Human library name.
  // 人工库名称。
  libraryName: string;
  // Stable current document UUID.
  // 稳定当前文档 UUID。
  documentId: string;
  // Exact document revision ranked and projected by this result.
  // 此结果排名及投影的精确文档修订。
  revision: number;
  // Exact chunk content hash used to isolate concurrent query generations.
  // 用于隔离并发查询代次的精确切块内容哈希。
  contentSha256: string;
  // Current document path.
  // 当前文档路径。
  path: string;
  // Current AI title.
  // 当前 AI 标题。
  title: string;
  // Current AI summary.
  // 当前 AI 摘要。
  summary: string;
  // Exact current chunk identity.
  // 精确当前切块身份。
  chunkId: string;
  // Summary or body discriminator.
  // 摘要或正文判别标识。
  kind: string;
  // Current heading ancestry.
  // 当前标题层级。
  headingPath: string;
  // Inclusive document first line; summary chunks use zero.
  // 包含边界的文档首行；摘要切块为零。
  startLine: number;
  // Inclusive document last line.
  // 包含边界的文档末行。
  endLine: number;
  // Untrusted reference content.
  // 不可信参考内容。
  content: string;
  // Reciprocal-rank fusion score.
  // 倒数排名融合分数。
  score: number;
  // Explicit BM25 or vector contributing capabilities.
  // 明确贡献的全文或向量能力。
  matchKinds: string[];
}

// Declared capability state accompanying every searched library.
// 伴随每个检索库返回的声明能力状态。
export interface KnowledgeSearchLibraryStatus {
  // Library UUID.
  // 知识库 UUID。
  libraryId: string;
  // Exact declared mode.
  // 精确声明模式。
  indexMode: KnowledgeIndexMode;
  // Ready, building or disabled FTS capability.
  // 就绪、构建中或禁用的全文能力。
  ftsStatus: string;
  // Current embedding progress and active/desired space distinction.
  // 当前嵌入进度以及活动和期望空间区分。
  embeddings: KnowledgeEmbeddingStatus;
}

// Explicit per-library query failure; partial hybrid results never hide this diagnostic.
// 明确每库查询失败；部分混合结果绝不隐藏该诊断。
export interface KnowledgeSearchFailure {
  // Exact failed library.
  // 精确失败知识库。
  libraryId: string;
  // Query that failed or no query for library admission.
  // 失败查询；库准入失败时为空。
  query: string | null;
  // English capability or inference error.
  // 英文能力或推理错误。
  error: string;
}

// Federated search result with explicit mode and backlog status for every participating library.
// 联合检索结果，包含每个参与库的明确模式和积压状态。
export interface KnowledgeSearchResult {
  // Exact queries evaluated.
  // 已评估精确查询。
  queries: string[];
  // Ranked current chunks.
  // 已排名当前切块。
  hits: KnowledgeSearchHit[];
  // Explicit per-library capability state.
  // 明确每库能力状态。
  libraries: KnowledgeSearchLibraryStatus[];
  // Visible partial failures.
  // 可见部分失败。
  failures: KnowledgeSearchFailure[];
}

// Exact current portable format manifest; previous versions are refused before content decoding.
// 精确当前可移植格式清单；内容解码前拒绝旧版本。
export interface KnowledgePackageManifest {
  // Stable package family.
  // 稳定套件家族。
  format: string;
  // Exact supported format version.
  // 精确支持的格式版本。
  version: number;
  // Human library name.
  // 人工知识库名称。
  name: string;
  // Human library purpose.
  // 人工知识库用途。
  description: string;
  // Frozen current indexing configuration.
  // 冻结当前索引配置。
  settings: KnowledgeLibrarySettings;
  // Current custom dictionary configuration.
  // 当前自定义词典配置。
  customWords: KnowledgeCustomWord[];
  // Exact document record count.
  // 精确文档记录数。
  documentCount: number;
  // Exact explicit folder record count.
  // 精确显式目录记录数。
  folderCount: number;
  // Digest of exact uncompressed content.jsonl bytes.
  // 精确未压缩 content.jsonl 字节的摘要。
  contentSha256: string;
  // Host export timestamp.
  // 宿主导出时间戳。
  exportedAtMs: number;
}

// Frozen inspection identity used to prevent import-after-preview races.
// 防止预检后导入竞态的冻结检查身份。
export interface KnowledgePackageInspection {
  // Exact canonical archive path.
  // 精确规范归档路径。
  archivePath: string;
  // Digest of the complete verified archive.
  // 完整已验证归档的摘要。
  sha256: string;
  // Actual compressed archive size.
  // 实际压缩归档大小。
  byteSize: number;
  // Strict validated current-format manifest.
  // 严格验证的当前格式清单。
  manifest: KnowledgePackageManifest;
}

// Successful new-library import receipt; history begins at its sole import change.
// 成功新库导入回执；历史从唯一导入变更开始。
export interface KnowledgePackageImportResult {
  // Newly created library and derived progress.
  // 新建知识库与派生进度。
  library: KnowledgeLibrarySummary;
  // Sole initial content change.
  // 唯一初始内容变更。
  changeSeq: number;
}

// Atomic mutation receipt.
// 原子变更回执。
export interface KnowledgeWriteResult {
  // New change; absent only for unchanged ingest.
  // 新变更；仅无变化搬运为空。
  changeSeq: number | null;
  // Affected surviving documents.
  // 受影响且仍存在的文档。
  documents: KnowledgeDocumentSummary[];
  // Unchanged ingest paths.
  // 无变化搬运路径。
  unchanged: string[];
  // Pending vector count.
  // 待生成向量数。
  pendingEmbeddings: number;
}

// Exact user-selected embedding route without host authorization fields.
// 不包含宿主授权字段的精确人工选择嵌入路由。
export interface KnowledgeRouteReferenceRequest {
  // Provider catalog identity.
  // 供应商目录身份。
  providerId: string;
  // Exact protocol profile or action binding.
  // 精确协议配置档或动作绑定。
  protocol: string;
  // Declared interpretation of protocol.
  // 协议的声明解释方式。
  bindingKind: KnowledgeRouteBindingKindRequest;
  // Exact service endpoint identity.
  // 精确服务端点身份。
  serviceEntryId: string;
  // Exact signed-catalog model identity.
  // 精确签名目录模型身份。
  modelId: string;
  // Exact credential connection identity; no credentials are transferred.
  // 精确凭据连接身份；不传递凭据。
  connectionId: string;
}

// Explicit model dimensions and optional batching selected by the human.
// 人工选择的显式模型维度及可选批量设置。
export interface KnowledgeEmbeddingSettingsRequest {
  // Exact selected embedding route.
  // 精确选定嵌入路由。
  route: KnowledgeRouteReferenceRequest;
  // Positive output dimension validated against current official capabilities.
  // 根据当前官方能力验证的正输出维度。
  dimensions: number;
  // Optional positive batch count bounded by official capabilities.
  // 受官方能力约束的可选正批量数量。
  batchSize?: number | null;
}

// Library-owned mode and optional exact embedding route.
// 库自身拥有的模式及可选精确嵌入路由。
export interface KnowledgeLibrarySettingsRequest {
  // Required declared library indexing mode.
  // 必填声明知识库索引模式。
  indexMode: KnowledgeIndexModeRequest;
  // Required in vector/hybrid modes and forbidden in other modes.
  // 向量及混合模式必填，其他模式禁止。
  embedding?: KnowledgeEmbeddingSettingsRequest | null;
}

// Creates an empty library with human lifecycle configuration.
// 使用人工生命周期配置创建空知识库。
export interface KnowledgeLibraryCreateRequest {
  // Unique visible library name.
  // 唯一可见知识库名称。
  name: string;
  // Human purpose guiding AI content organization.
  // 指导 AI 内容组织的人工用途。
  description: string;
  // Optional exact initial project bindings.
  // 可选精确初始项目绑定。
  projectIds?: string[];
  // Explicit mode and embedding selection.
  // 显式模式及嵌入选择。
  settings: KnowledgeLibrarySettingsRequest;
}

// Changes only human name and purpose; content is never accepted.
// 仅变更人工名称与用途；绝不接受内容。
export interface KnowledgeLibraryRenameRequest {
  // Immutable library UUID.
  // 不可变知识库 UUID。
  libraryId: string;
  // New unique visible name.
  // 新唯一可见名称。
  name: string;
  // New human purpose.
  // 新人工用途。
  description: string;
}

// Explicit index mode change returning a durable operation.
// 返回持久操作的显式索引模式变更。
export interface KnowledgeIndexModeRequestPayload {
  // Exact library UUID.
  // 精确知识库 UUID。
  libraryId: string;
  // New declared mode and exact embedding configuration.
  // 新声明模式及精确嵌入配置。
  settings: KnowledgeLibrarySettingsRequest;
}

// Authorizes or revokes one exact project binding.
// 授权或撤销单个精确项目绑定。
export interface KnowledgeProjectBindingRequest {
  // Exact library UUID.
  // 精确知识库 UUID。
  libraryId: string;
  // Exact project identity.
  // 精确项目身份。
  projectId: string;
  // Whether the binding must exist.
  // 绑定是否必须存在。
  authorized: boolean;
  // Whether inherited session selection includes this authorized library.
  // 继承会话选择是否包含此已授权库。
  defaultEnabled?: boolean;
}

// Creates or updates one library-local Jieba term.
// 创建或更新单个库内结巴词语。
export interface KnowledgeCustomWordRequest {
  // Exact library UUID.
  // 精确知识库 UUID。
  libraryId: string;
  // Exact term text.
  // 精确词语文本。
  word: string;
  // Positive dictionary weight.
  // 正词典权重。
  weight: number;
}

// Reads a head-bound logical directory page for management.
// 读取用于管理的绑定变更头逻辑目录页。
export interface KnowledgeLibraryDocumentListRequest {
  // Exact library UUID.
  // 精确知识库 UUID。
  libraryId: string;
  // Logical folder; empty means root.
  // 逻辑目录；空字符串表示根。
  path?: string;
  // Descendant levels, default one.
  // 后代层数，默认一层。
  depth?: number;
  // Opaque continuation for the same library/path/depth.
  // 相同库、路径及深度的不透明续读标识。
  cursor?: string | null;
  // Requested entries bounded by the storage authority.
  // 受存储权威约束的请求条目数。
  limit?: number;
}

// Reads a project-authorized logical directory page.
// 读取经项目授权的逻辑目录页。
export interface KnowledgeDocumentListRequest {
  // Exact owning project identity.
  // 精确所属项目身份。
  projectId: string;
  // Exact authorized library UUID.
  // 精确已授权库 UUID。
  libraryId: string;
  // Logical directory, empty for root.
  // 逻辑目录；根目录为空。
  path?: string;
  // Descendant levels, default one.
  // 后代层数，默认一层。
  depth?: number;
  // Opaque continuation.
  // 不透明续读标识。
  cursor?: string | null;
  // Requested bounded page length.
  // 请求的有界分页长度。
  limit?: number;
}

// Reads exactly one document selector and a bounded inclusive line range.
// 读取精确单个文档选择器及有界包含边界行范围。
export interface KnowledgeDocumentReadRequest {
  // Exact library UUID.
  // 精确知识库 UUID。
  libraryId: string;
  // Stable document ID, mutually exclusive with path.
  // 稳定文档身份，与路径互斥。
  documentId?: string | null;
  // Exact logical document path, mutually exclusive with ID.
  // 精确逻辑文档路径，与身份互斥。
  path?: string | null;
  // First one-based inclusive line.
  // 从一开始且包含边界的首行。
  startLine?: number | null;
  // Last one-based inclusive line.
  // 从一开始且包含边界的末行。
  endLine?: number | null;
}

// Reads one document under an explicit project authorization.
// 在显式项目授权下读取单篇文档。
export interface KnowledgeProjectDocumentReadRequest {
  // Exact project identity.
  // 精确项目身份。
  projectId: string;
  // Exact library, selector and line window.
  // 精确知识库、选择器及行窗口。
  document: KnowledgeDocumentReadRequest;
}

// Searches one library according to its own declared mode.
// 按自身声明模式检索单个知识库。
export interface KnowledgeLibrarySearchRequest {
  // Exact library UUID.
  // 精确知识库 UUID。
  libraryId: string;
  // Nonempty natural-language query.
  // 非空自然语言查询。
  query: string;
  // Maximum current-chunk hits.
  // 当前切块最大命中数。
  limit?: number;
}

// Searches exact project-authorized libraries using their own modes.
// 使用各自模式检索精确项目授权知识库。
export interface KnowledgeSearchRequest {
  // Exact project identity.
  // 精确项目身份。
  projectId: string;
  // Exact authorized library subset.
  // 精确已授权库子集。
  libraryIds: string[];
  // Nonempty natural-language query.
  // 非空自然语言查询。
  query: string;
  // Maximum fused current-chunk hits.
  // 融合后当前切块最大命中数。
  limit?: number;
}

// Queries an exact operation, or explicitly discovers the latest library operation.
// 查询精确操作，或显式发现库最新操作。
export interface KnowledgeOperationRequest {
  // Exact owning library UUID.
  // 精确所属知识库 UUID。
  libraryId: string;
  // Stable operation ID; omission requests latest discovery.
  // 稳定操作身份；省略时请求发现最新操作。
  operationId?: string | null;
}

// Cancels only the exact currently active operation.
// 仅取消精确当前活动操作。
export interface KnowledgeOperationCancelRequest {
  // Exact owning library UUID.
  // 精确所属知识库 UUID。
  libraryId: string;
  // Exact stable operation UUID.
  // 精确稳定操作 UUID。
  operationId: string;
}

// Reads a bounded immutable history summary page.
// 读取有界不可变历史摘要页。
export interface KnowledgeChangeListRequest {
  // Exact library UUID.
  // 精确知识库 UUID。
  libraryId: string;
  // Exclusive upper sequence for older history.
  // 较早历史的排他上界序号。
  beforeSeq?: number | null;
  // Requested bounded summary count.
  // 请求的有界摘要数。
  limit?: number;
}

// Reads bounded items of one exact immutable change.
// 读取单个精确不可变变更的有界条目。
export interface KnowledgeChangeGetRequest {
  // Exact library UUID.
  // 精确知识库 UUID。
  libraryId: string;
  // Exact immutable change sequence.
  // 精确不可变变更序号。
  seq: number;
  // Zero-based image offset.
  // 从零开始的镜像偏移。
  offset?: number;
  // Maximum image count, also subject to the shared byte budget.
  // 最大镜像数，同时受共享字节预算约束。
  limit?: number;
}

// Reads exact historical document text for a before/after comparison.
// 读取用于前后比较的精确历史文档文本。
export interface KnowledgeChangeDocumentReadRequest {
  // Exact library UUID.
  // 精确知识库 UUID。
  libraryId: string;
  // Exact immutable change sequence.
  // 精确不可变变更序号。
  seq: number;
  // Exact changed document UUID.
  // 精确变更文档 UUID。
  documentId: string;
  // Explicit before or after image.
  // 显式变更前或变更后镜像。
  side: KnowledgeChangeSideRequest;
  // First one-based inclusive line.
  // 从一开始且包含边界的首行。
  startLine?: number | null;
  // Last one-based inclusive line.
  // 从一开始且包含边界的末行。
  endLine?: number | null;
}

// Human-confirmed restoration that appends a new reversible change.
// 追加新可还原变更的人工确认还原。
export interface KnowledgeRestoreRequest {
  // Exact library UUID.
  // 精确知识库 UUID。
  libraryId: string;
  // Target history point; zero means initially empty.
  // 目标历史点；零表示初始空库。
  targetSeq: number;
  // Optional human audit intent.
  // 可选人工审计意图。
  note?: string;
}

// Reads current vector backlog and a bounded failure page.
// 读取当前向量积压及有界失败页。
export interface KnowledgeEmbeddingStatusRequest {
  // Exact library UUID.
  // 精确知识库 UUID。
  libraryId: string;
  // Zero-based failure detail offset.
  // 从零开始的失败详情偏移。
  offset?: number;
  // Maximum failure rows.
  // 最大失败行数。
  limit?: number;
}

// Retries all exhausted hashes or an exact selected document subset.
// 重试全部耗尽哈希或精确选中文档子集。
export interface KnowledgeEmbeddingRetryRequest {
  // Exact library UUID.
  // 精确知识库 UUID。
  libraryId: string;
  // Empty means all failed current hashes; no pending or ready work is reset.
  // 空集合表示全部当前失败哈希；不重置待处理或就绪工作。
  documentIds?: string[];
}

// Exports current content and configuration only.
// 仅导出当前内容及配置。
export interface KnowledgePackageExportRequest {
  // Exact library UUID.
  // 精确知识库 UUID。
  libraryId: string;
  // Existing native destination directory.
  // 已有原生目标目录。
  destinationDirectory: string;
}

// Validates a package without side effects.
// 无副作用校验套件。
export interface KnowledgePackageInspectRequest {
  // Exact native package path.
  // 精确原生套件路径。
  archivePath: string;
}

// Imports the inspected bytes as a new independent library and fresh history.
// 将已预检字节导入为新的独立库及全新历史。
export interface KnowledgePackageImportRequest {
  // Exact native package path.
  // 精确原生套件路径。
  archivePath: string;
  // Unique final library name.
  // 唯一最终知识库名称。
  name: string;
  // Whole-file SHA-256 returned by inspection.
  // 预检返回的完整文件 SHA-256。
  expectedSha256: string;
}

// One selectable .vkb knowledge package file.
// 单个可选 .vkb 知识库套件文件。
export type KnowledgePackageFileSummary = PackageFileSummary;

// Directory navigation with selectable .vkb knowledge packages.
// 包含可选 .vkb 知识库套件的目录导航。
export type KnowledgePackageFileListing = PackageFileListing;

// Persists only a session's library selection, never a retrieval policy.
// 仅持久化会话库选择，绝不持久化检索策略。
export interface SessionKnowledgeLibrarySelectionRequest {
  // Exact owning project identity.
  // 精确所属项目身份。
  projectId: string;
  // Exact existing conversation UUID.
  // 精确已有会话 UUID。
  conversationId: string;
  // Explicit inherit or explicit selection mode.
  // 显式 inherit 或 explicit 选择模式。
  mode: KnowledgeLibrarySelectionMode;
  // Exact selected authorized libraries.
  // 精确选中已授权知识库。
  libraryIds?: string[];
}

// Persisted selection and currently effective authorization.
// 持久选择及当前有效授权。
export interface SessionKnowledgeLibrarySelectionSummary {
  // Persisted inherit or explicit selection mode.
  // 持久化 inherit 或 explicit 选择模式。
  mode: KnowledgeLibrarySelectionMode;
  // Exact persisted selected libraries.
  // 精确持久化选中库。
  libraryIds: string[];
  // Current project-authorized enabled libraries.
  // 当前项目授权启用库。
  effectiveLibraryIds: string[];
}

// Metadata-only updates share the exact rename request.
// 仅元数据更新共享精确重命名请求。
export type KnowledgeLibrarySettingsUpdateRequest = KnowledgeLibraryRenameRequest;
// Export returns the same complete inspected archive identity.
// 导出返回相同完整已检查归档身份。
export type KnowledgePackageExportResult = KnowledgePackageInspection;
