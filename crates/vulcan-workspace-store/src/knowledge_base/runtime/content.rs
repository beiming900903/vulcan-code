//! Atomic AI content mutations, immutable history, and read-only projections.
//! 原子 AI 内容变更、不可变历史与只读投影。

use super::*;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Serialize, de::DeserializeOwned};
use unicode_normalization::UnicodeNormalization;
use vulcan_sqlite_engine::{fts::ensure_fts_index, tokenizer::TokenizerMode};

mod history_read;
mod package_content;
pub(super) use package_content::{
    PackageExportContent, ValidatedPackageContent, prepare_package_content,
    serialize_package_content,
};

/// Fixed row projection shared by all document reads.
/// 全部文档读取共享的固定行投影。
const DOCUMENT_COLUMNS: &str = "id,path,path_key,folder_path,title,summary,summary_state,keywords_json,body_sha256,line_count,embed_policy,provenance_json,revision,created_change_seq,updated_change_seq";
/// Typed document snapshot indexed by stable identity.
/// 按稳定身份索引的类型化文档快照。
type Documents = BTreeMap<String, KnowledgeDocumentSummary>;
/// Typed explicit directory snapshot indexed by comparison key.
/// 按比较键索引的类型化显式目录快照。
type Folders = BTreeMap<String, KnowledgeFolder>;

/// Normalizes a content path; returns NFC spelling and case-insensitive identity or a stable error.
/// 规范内容路径；返回 NFC 拼写和忽略大小写身份，或稳定错误。
pub(super) fn normalize_path(path: &str, allow_root: bool) -> Result<(String, String), String> {
    // Validate after trimming boundary separators, while preserving meaningful spaces.
    // 去除边界分隔符后验证，同时保留有意义的空格。
    let normalized: String = path.trim_matches('/').nfc().collect();
    if normalized.is_empty() && allow_root {
        return Ok((String::new(), String::new()));
    }
    // Every path component has one interpretation across all host platforms.
    // 每个路径组件在所有宿主平台只有一种解释。
    let segments: Vec<_> = normalized.split('/').collect();
    if normalized.is_empty()
        || normalized.contains('\\')
        || normalized.chars().any(char::is_control)
        || segments.len() > MAX_KNOWLEDGE_PATH_DEPTH
        || segments.iter().any(|s| {
            s.is_empty()
                || *s == "."
                || *s == ".."
                || s.chars().count() > MAX_KNOWLEDGE_PATH_SEGMENT_CHARS
        })
    {
        return Err("ERR_KNOWLEDGE_PATH_INVALID".into());
    }
    // Lowercasing can introduce combining characters; normalize the comparison key again.
    // 小写化可能引入组合字符，因此比较键再次规范化。
    let key = normalized.to_lowercase().nfc().collect();
    Ok((normalized, key))
}

/// Serializes an exact typed value; returns an English serialization error on failure.
/// 序列化精确类型化值；失败时返回英文序列化错误。
fn json<T: Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value).map_err(|e| e.to_string())
}

/// Decodes a typed JSON SQLite column; returns a normal SQLite conversion error on corruption.
/// 解码类型化 JSON SQLite 列；损坏时返回标准 SQLite 转换错误。
fn column_json<T: DeserializeOwned>(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<T> {
    serde_json::from_str(&row.get::<_, String>(index)?).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(e))
    })
}

/// Decodes the single authoritative document projection; returns checked typed metadata.
/// 解码唯一权威文档投影；返回已检查的类型化元数据。
fn document_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<KnowledgeDocumentSummary> {
    Ok(KnowledgeDocumentSummary {
        id: row.get(0)?,
        path: row.get(1)?,
        path_key: row.get(2)?,
        folder_path: row.get(3)?,
        title: row.get(4)?,
        summary: row.get(5)?,
        summary_state: row.get(6)?,
        keywords: column_json(row, 7)?,
        body_sha256: row.get(8)?,
        line_count: unsigned_column(row, 9)?,
        embed_policy: serde_json::from_value(serde_json::Value::String(row.get(10)?)).map_err(
            |e| {
                rusqlite::Error::FromSqlConversionFailure(
                    10,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            },
        )?,
        provenance: column_json(row, 11)?,
        revision: unsigned_column(row, 12)?,
        created_change_seq: unsigned_column(row, 13)?,
        updated_change_seq: unsigned_column(row, 14)?,
    })
}

/// Reads all current document metadata; returns a stable identity map without reading bodies.
/// 读取全部当前文档元数据；返回稳定身份映射且不读取正文。
fn documents(connection: &Connection) -> Result<Documents, String> {
    // SQL column order is centralized in DOCUMENT_COLUMNS.
    // SQL 列顺序集中在 DOCUMENT_COLUMNS。
    let mut statement = connection
        .prepare(&format!(
            "SELECT {DOCUMENT_COLUMNS} FROM kb_documents ORDER BY path_key;"
        ))
        .map_err(|e| e.to_string())?;
    statement
        .query_map([], document_row)
        .map_err(|e| e.to_string())?
        .map(|r| r.map(|d| (d.id.clone(), d)).map_err(|e| e.to_string()))
        .collect()
}

/// Reads all explicit folders; returns a comparison-key map.
/// 读取全部显式目录；返回比较键映射。
fn folders(connection: &Connection) -> Result<Folders, String> {
    // Implicit folders are projected by list_content rather than persisted twice.
    // 隐式目录由 list_content 投影，不重复持久化。
    let mut statement=connection.prepare("SELECT path_key,path,description,created_change_seq,updated_change_seq FROM kb_folders ORDER BY path_key;").map_err(|e|e.to_string())?;
    statement
        .query_map([], |r| {
            Ok(KnowledgeFolder {
                path_key: r.get(0)?,
                path: r.get(1)?,
                description: r.get(2)?,
                created_change_seq: unsigned_column(r, 3)?,
                updated_change_seq: unsigned_column(r, 4)?,
            })
        })
        .map_err(|e| e.to_string())?
        .map(|r| {
            r.map(|f| (f.path_key.clone(), f))
                .map_err(|e| e.to_string())
        })
        .collect()
}

/// Reads one content-addressed body; returns the exact stored UTF-8 text.
/// 读取一个内容寻址正文；返回精确存储的 UTF-8 文本。
fn body(connection: &Connection, hash: &str) -> Result<String, String> {
    connection
        .query_row("SELECT content FROM kb_blobs WHERE sha256=?", [hash], |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())
}

/// Checks replacement expansion before allocating its resulting text.
/// 在分配结果文本前检查替换扩张。
/// Parameters: text/old/new byte lengths and the actual nonoverlapping replacement count describe one exact edit.
/// 参数：正文、旧文本及新文本的字节长度与实际非重叠替换次数描述一次精确编辑。
/// Returns the bounded result size, or the shared body-limit error on expansion, arithmetic overflow or invalid subtraction.
/// 返回有界结果大小；扩张、算术溢出或无效减法时返回共享正文限制错误。
fn checked_edit_output_bytes(
    text: usize,
    old: usize,
    new: usize,
    replacements: usize,
) -> Result<usize, String> {
    // Removing matched bytes first permits shrinking edits at the exact shared body boundary.
    // 先移除匹配字节，使精确共享正文边界上的缩减编辑仍可执行。
    let remaining = old
        .checked_mul(replacements)
        .and_then(|removed| text.checked_sub(removed))
        .ok_or("ERR_KNOWLEDGE_BODY_TOO_LARGE")?;
    // Repeated replacement must never allocate an amplified string before its budget is known.
    // 重复替换绝不能在预算已知前分配被放大的字符串。
    let inserted = new
        .checked_mul(replacements)
        .ok_or("ERR_KNOWLEDGE_BODY_TOO_LARGE")?;
    remaining
        .checked_add(inserted)
        .filter(|bytes| *bytes <= MAX_KNOWLEDGE_BODY_BYTES)
        .ok_or_else(|| "ERR_KNOWLEDGE_BODY_TOO_LARGE".into())
}

/// Validates a complete document and body; returns stable diagnostics before any commit.
/// 验证完整文档与正文；在提交前返回稳定诊断。
fn validate_document(document: &KnowledgeDocumentSummary, body: &str) -> Result<(), String> {
    normalize_path(&document.path, false)?;
    if body.len() > MAX_KNOWLEDGE_BODY_BYTES {
        return Err("ERR_KNOWLEDGE_BODY_TOO_LARGE".into());
    }
    if document.title.trim().is_empty() {
        return Err("ERR_KNOWLEDGE_TITLE_REQUIRED".into());
    }
    if document.title.chars().count() > MAX_KNOWLEDGE_TITLE_CHARS {
        return Err("ERR_KNOWLEDGE_TITLE_TOO_LONG".into());
    }
    if document.summary.chars().count() > MAX_KNOWLEDGE_SUMMARY_CHARS {
        return Err("ERR_KNOWLEDGE_SUMMARY_TOO_LONG".into());
    }
    match document.summary_state.as_str() {
        "provided" | "stale" if document.summary.trim().is_empty() => {
            return Err("ERR_KNOWLEDGE_SUMMARY_REQUIRED".into());
        }
        "pending" if !document.summary.is_empty() => {
            return Err("ERR_KNOWLEDGE_SUMMARY_STATE_INVALID".into());
        }
        "provided" | "pending" | "stale" => {}
        _ => return Err("ERR_KNOWLEDGE_SUMMARY_STATE_INVALID".into()),
    }
    if document.keywords.len() > MAX_KNOWLEDGE_KEYWORDS
        || document
            .keywords
            .iter()
            .any(|k| k.trim().is_empty() || k.chars().count() > MAX_KNOWLEDGE_PATH_SEGMENT_CHARS)
    {
        return Err("ERR_KNOWLEDGE_KEYWORDS_INVALID".into());
    }
    if document.provenance.len() > MAX_KNOWLEDGE_PROVENANCE_ITEMS {
        return Err("ERR_KNOWLEDGE_PROVENANCE_LIMIT".into());
    }
    if document.provenance.iter().any(|p| {
        !matches!(p.kind.as_str(), "file" | "url" | "session")
            || p.reference.is_empty()
            || p.reference.chars().count() > MAX_KNOWLEDGE_REFERENCE_CHARS
            || p.sha256.as_ref().is_some_and(|hash| {
                hash.len() != KNOWLEDGE_SHA256_HEX_CHARS
                    || !hash.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
    }) {
        return Err("ERR_KNOWLEDGE_PROVENANCE_INVALID".into());
    }
    Ok(())
}

/// Validates the complete namespace; returns conflicts between documents and explicit or implicit folders.
/// 验证完整命名空间；返回文档与显式或隐式目录冲突。
fn validate_namespace(documents: &Documents, folders: &Folders) -> Result<(), String> {
    if folders
        .values()
        .any(|folder| folder.description.chars().count() > MAX_KNOWLEDGE_METADATA_CHARS)
    {
        return Err("ERR_KNOWLEDGE_DESCRIPTION_LIMIT".into());
    }
    // A document may never occupy an ancestor folder, regardless of case or Unicode spelling.
    // 无论文档大小写和 Unicode 拼写如何，都不得占用祖先目录。
    let mut keys = BTreeSet::new();
    for document in documents.values() {
        if !keys.insert(document.path_key.clone()) || folders.contains_key(&document.path_key) {
            return Err("ERR_KNOWLEDGE_PATH_CONFLICT".into());
        }
    }
    for path in documents
        .values()
        .map(|d| &d.path_key)
        .chain(folders.keys())
    {
        // Walk exact component boundaries; prefix-like siblings remain independent.
        // 遍历精确组件边界；仅前缀相似的同级项保持独立。
        let mut ancestor = path.as_str();
        while let Some((parent, _)) = ancestor.rsplit_once('/') {
            if keys.contains(parent) {
                return Err("ERR_KNOWLEDGE_PATH_CONFLICT".into());
            }
            ancestor = parent;
        }
    }
    Ok(())
}

/// Selects exactly one document by ID or path; returns its immutable identity or an explicit selector error.
/// 按身份或路径精确选择文档；返回不可变身份或明确选择器错误。
fn select_document(
    documents: &Documents,
    id: Option<&str>,
    path: Option<&str>,
) -> Result<String, String> {
    match (id, path) {
        (Some(id), None) => documents
            .contains_key(id)
            .then(|| id.to_string())
            .ok_or("ERR_KNOWLEDGE_DOCUMENT_NOT_FOUND".into()),
        (None, Some(path)) => {
            // Path matching uses the same normalized identity as uniqueness validation.
            // 路径匹配使用与唯一性验证相同的规范身份。
            let (_, key) = normalize_path(path, false)?;
            documents
                .values()
                .find(|d| d.path_key == key)
                .map(|d| d.id.clone())
                .ok_or("ERR_KNOWLEDGE_DOCUMENT_NOT_FOUND".into())
        }
        _ => {
            Err("ERR_KNOWLEDGE_DOCUMENT_SELECTOR: specify exactly one of documentId or path".into())
        }
    }
}

/// Requires a field for a specific action; returns the borrowed value or a stable diagnostic.
/// 要求某动作的字段；返回借用值或稳定诊断。
fn required<'a, T>(value: &'a Option<T>, field: &str) -> Result<&'a T, String> {
    value
        .as_ref()
        .ok_or_else(|| format!("ERR_KNOWLEDGE_FIELD_REQUIRED: {field}"))
}

/// Rejects fields not belonging to an action; returns success only for the exact contract.
/// 拒绝不属于动作的字段；仅精确契约返回成功。
fn validate_action_fields(request: &KnowledgeWriteRequest) -> Result<(), String> {
    // JSON field presence, including explicit empty arrays, determines action-specific admission.
    // JSON 字段是否存在（包括显式空数组）决定动作准入。
    let fields = [
        ("documentId", request.document_id.is_some()),
        ("path", request.path.is_some()),
        ("expectedRevision", request.expected_revision.is_some()),
        ("title", request.title.is_some()),
        ("summary", request.summary.is_some()),
        ("body", request.body.is_some()),
        ("keywords", request.keywords.is_some()),
        ("embed", request.embed.is_some()),
        ("sources", request.sources.is_some()),
        ("edits", request.edits.is_some()),
        ("newPath", request.new_path.is_some()),
        ("description", request.description.is_some()),
    ];
    // Every action owns a closed set of fields, avoiding ignored content or accidental writes.
    // 每种动作拥有封闭字段集合，避免忽略内容或意外写入。
    let allowed: &[&str] = match request.action.as_str() {
        "create" => &[
            "path", "title", "summary", "body", "keywords", "embed", "sources",
        ],
        "update" => &[
            "documentId",
            "path",
            "expectedRevision",
            "title",
            "summary",
            "body",
            "keywords",
            "embed",
            "sources",
        ],
        "edit" => &["documentId", "path", "expectedRevision", "edits", "summary"],
        "move" => &["documentId", "path", "expectedRevision", "newPath"],
        "delete" => &["documentId", "path", "expectedRevision"],
        "folder" => &["path", "description"],
        "delete_folder" => &["path"],
        _ => return Err("ERR_KNOWLEDGE_ACTION_INVALID".into()),
    };
    if let Some((field, _)) = fields
        .iter()
        .find(|(f, present)| *present && !allowed.contains(f))
    {
        return Err(format!("ERR_KNOWLEDGE_FIELD_UNEXPECTED: {field}"));
    }
    if request.action == "update"
        && !fields.iter().any(|(f, present)| {
            *present && ["title", "summary", "body", "keywords", "embed", "sources"].contains(f)
        })
    {
        return Err("ERR_KNOWLEDGE_UPDATE_EMPTY".into());
    }
    Ok(())
}

/// Creates a validated current row from explicit content; returns metadata and exact body digest.
/// 从显式内容创建已验证当前行；返回元数据与精确正文摘要。
fn new_document(
    path: &str,
    title: &str,
    summary: Option<&str>,
    body: &str,
    embed: KnowledgeEmbedPolicy,
    provenance: Vec<KnowledgeProvenance>,
) -> Result<KnowledgeDocumentSummary, String> {
    // Normalize the namespace once before assigning the stable identity.
    // 分配稳定身份前只规范命名空间一次。
    let (path, path_key) = normalize_path(path, false)?;
    // Preserve the normalized parent spelling for directory presentation.
    // 保留规范父路径拼写供目录展示。
    let folder_path = path.rsplit_once('/').map_or("", |(p, _)| p).to_string();
    // An absent ingest summary is explicit pending state, never a guessed generated summary.
    // 缺失搬运摘要明确为待补充状态，绝不猜测生成摘要。
    let document = KnowledgeDocumentSummary {
        id: Uuid::new_v4().to_string(),
        path,
        path_key,
        folder_path,
        title: title.into(),
        summary: summary.unwrap_or("").into(),
        summary_state: if summary.is_some() {
            "provided"
        } else {
            "pending"
        }
        .into(),
        keywords: vec![],
        body_sha256: super::super::sha256_hex(body.as_bytes()),
        line_count: super::super::knowledge_body_lines(body).count(),
        embed_policy: embed,
        provenance,
        revision: 1,
        created_change_seq: 0,
        updated_change_seq: 0,
    };
    validate_document(&document, body)?;
    Ok(document)
}

/// Computes vector-space identity from semantic route, model and dimension, excluding batching.
/// 从语义路由、模型与维度计算向量空间身份，排除批量参数。
pub(super) fn vector_space_id(embedding: &KnowledgeEmbeddingSettings) -> Result<String, String> {
    Ok(super::super::sha256_hex(
        json(&(
            &embedding.route,
            &embedding.model_fingerprint,
            embedding.dimensions,
            embedding.metric,
        ))?
        .as_bytes(),
    ))
}

/// Stores one full row through the only SQL writer; returns storage errors without partial publication.
/// 经唯一 SQL 写入器保存完整行；返回存储错误且不部分发布。
fn save_document(
    transaction: &Transaction<'_>,
    document: &KnowledgeDocumentSummary,
) -> Result<(), String> {
    transaction.execute("INSERT INTO kb_documents(id,path,path_key,folder_path,title,summary,summary_state,keywords_json,body_sha256,line_count,embed_policy,provenance_json,revision,created_change_seq,updated_change_seq) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?);",
        params![document.id,document.path,document.path_key,document.folder_path,document.title,document.summary,document.summary_state,
            json(&document.keywords)?,document.body_sha256,sql_integer(document.line_count)?,serde_json::to_value(document.embed_policy).map_err(|e|e.to_string())?.as_str().ok_or("ERR_KNOWLEDGE_EMBED_POLICY_INVALID")?,json(&document.provenance)?,
            sql_integer(document.revision)?,sql_integer(document.created_change_seq)?,sql_integer(document.updated_change_seq)?]).map_err(|e|e.to_string())?;
    Ok(())
}

/// Rebuilds a changed document's summary/body chunks and FTS rows within the owning write transaction.
/// 在所属写事务内重建变更文档摘要、正文切块和全文行。
fn rebuild_document(
    transaction: &Transaction<'_>,
    document: &KnowledgeDocumentSummary,
    fts: bool,
) -> Result<(), String> {
    // Summary and body use the same content-hash contract for future restore reuse.
    // 摘要和正文采用相同内容哈希契约，供后续还原复用。
    let summary = format!(
        "{}\n{}\n{}",
        document.title,
        document.summary,
        document.keywords.join(" ")
    );
    // Ordinal zero is reserved for summary, independent of body chunk count.
    // 序号零保留给摘要，不依赖正文切块数量。
    let mut chunks = vec![(0, "summary", "".to_string(), 0, 0, summary)];
    for chunk in super::super::chunk_markdown(&body(transaction, &document.body_sha256)?) {
        chunks.push((
            chunk.ordinal + 1,
            "body",
            chunk.heading_path,
            chunk.start_line,
            chunk.end_line,
            chunk.content,
        ));
    }
    for (ordinal, kind, heading, start, end, text) in chunks {
        // Chunk identities survive unchanged content while every write refreshes their current ranges.
        // 切块身份在内容不变时保持稳定，每次写入刷新当前范围。
        let id = format!("{}:{ordinal}", document.id);
        // Only the explicitly selected document policy determines embedding membership.
        // 仅显式文档策略决定嵌入成员资格。
        let embed = document.embed_policy == KnowledgeEmbedPolicy::Full
            || (document.embed_policy == KnowledgeEmbedPolicy::Summary && kind == "summary");
        transaction.execute("INSERT INTO kb_chunks(id,document_id,ordinal,kind,heading_path,start_line,end_line,text,content_sha256,embed) VALUES(?,?,?,?,?,?,?,?,?,?);",
            params![id,document.id,sql_integer(ordinal)?,kind,heading,sql_integer(start)?,sql_integer(end)?,text,super::super::sha256_hex(text.as_bytes()),embed]).map_err(|e|e.to_string())?;
        if fts {
            transaction.execute(&format!("INSERT INTO {KNOWLEDGE_FTS_INDEX}(id,file_path,title,content) VALUES(?,?,?,?);"),params![id,document.path,document.title,text]).map_err(|e|e.to_string())?;
        }
    }
    Ok(())
}

/// Complete candidate transition and authenticated audit context for one atomic change.
/// 一次原子变更的完整候选过渡与已认证审计上下文。
struct SnapshotChange<'a> {
    /// Published documents before the change.
    /// 变更前已发布文档。
    before: &'a Documents,
    /// Published explicit folders before the change.
    /// 变更前已发布显式目录。
    before_folders: &'a Folders,
    /// Complete candidate documents.
    /// 完整候选文档。
    after: Documents,
    /// Complete candidate explicit folders.
    /// 完整候选显式目录。
    after_folders: Folders,
    /// New exact body bytes keyed by their verified hashes.
    /// 按已验证哈希索引的新精确正文字节。
    bodies: BTreeMap<String, String>,
    /// Runtime-authenticated invocation identity.
    /// 运行时已认证调用身份。
    actor: &'a KnowledgeChangeActor,
    /// Exact content operation discriminator.
    /// 精确内容操作判别标识。
    operation: &'a str,
    /// Auditable intent.
    /// 可审计意图。
    note: &'a str,
    /// Explicit restore target, absent for ordinary writes.
    /// 显式还原目标；普通写入为空。
    restore_target_seq: Option<u64>,
    /// Frozen library-owned indexing policy.
    /// 冻结的库自身索引策略。
    settings: &'a KnowledgeLibrarySettings,
}

/// Commits a candidate snapshot plus immutable images and derived chunks as one transaction.
/// 将候选快照、不可变镜像和派生切块作为单事务提交。
fn commit_snapshot(
    connection: &mut Connection,
    change: SnapshotChange<'_>,
) -> Result<KnowledgeWriteResult, String> {
    // Candidate state and audit context share one transaction boundary.
    // 候选状态与审计上下文共享一个事务边界。
    let SnapshotChange {
        before,
        before_folders,
        mut after,
        mut after_folders,
        bodies,
        actor,
        operation,
        note,
        restore_target_seq: target,
        settings,
    } = change;

    validate_namespace(&after, &after_folders)?;
    if note.chars().count() > MAX_KNOWLEDGE_METADATA_CHARS {
        return Err("ERR_KNOWLEDGE_NOTE_LIMIT".into());
    }
    if !matches!(actor.kind.as_str(), "ai" | "user" | "system") {
        return Err("ERR_KNOWLEDGE_ACTOR_INVALID".into());
    }
    // Acquire SQLite's write reservation before publishing any entity or history item.
    // 发布实体或历史项前获取 SQLite 写保留锁。
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    // An incremental write must not declare a partially built upgrade fully indexed.
    // 增量写入不得将部分构建的升级宣告为全部已索引。
    let index_was_ready = super::fts::is_current(&transaction)?;
    // Keep a previously built index current through writes even when FTS search is disabled.
    // 即使停用全文检索，也让已建索引随写入保持最新。
    let maintain_fts = settings.index_mode.has_fts() || super::fts::is_present(&transaction)?;
    // One invocation always produces exactly one change, including a no-op human restore.
    // 一次调用始终只产生一条变更，包括无差异人工还原。
    let now = unix_epoch_millis()?;
    transaction.execute("INSERT INTO kb_changes(at_ms,actor_kind,session_id,turn_id,tool_call_id,operation,note,restore_target_seq) VALUES(?,?,?,?,?,?,?,?);",
        params![now,actor.kind,actor.session_id,actor.turn_id,actor.tool_call_id,operation,note,target.map(sql_integer).transpose()?]).map_err(|e|e.to_string())?;
    // SQLite allocates the sole change identity inside this transaction.
    // SQLite 在该事务内分配唯一变更身份。
    let seq = u64::try_from(transaction.last_insert_rowid()).map_err(|e| e.to_string())?;
    for (hash, content) in bodies {
        transaction
            .execute(
                "INSERT OR IGNORE INTO kb_blobs(sha256,content,byte_size) VALUES(?,?,?);",
                params![hash, content, sql_integer(content.len())?],
            )
            .map_err(|e| e.to_string())?;
    }
    // Compare by immutable entity identity so moves produce one item, never delete-plus-create.
    // 按不可变实体身份比较，使移动只产生一项而非删除加创建。
    let changed_ids: BTreeSet<_> = before
        .keys()
        .chain(after.keys())
        .filter(|id| before.get(*id) != after.get(*id))
        .cloned()
        .collect();
    // Folder updates and deletions use the same image contract as documents.
    // 目录更新与删除使用与文档相同的镜像契约。
    let changed_folders: BTreeSet<_> = before_folders
        .keys()
        .chain(after_folders.keys())
        .filter(|key| before_folders.get(*key) != after_folders.get(*key))
        .cloned()
        .collect();
    if maintain_fts {
        ensure_fts_index(&transaction, KNOWLEDGE_FTS_INDEX, TokenizerMode::Jieba)
            .map_err(|e| e.to_string())?;
    }
    // Delete all changed current rows before insertion so restores can safely undo path swaps.
    // 插入前删除全部变更当前行，使还原可以安全撤销路径交换。
    for id in &changed_ids {
        if maintain_fts {
            transaction.execute(&format!("DELETE FROM {KNOWLEDGE_FTS_INDEX} WHERE id IN (SELECT id FROM kb_chunks WHERE document_id=?);"),[id]).map_err(|e|e.to_string())?;
        }
        transaction
            .execute("DELETE FROM kb_documents WHERE id=?;", [id])
            .map_err(|e| e.to_string())?;
    }
    // The ordered item counter defines the history's deterministic entity image order.
    // 有序条目计数器定义历史实体镜像的确定顺序。
    let mut ordinal = 0;
    // Surviving document receipts use the same row written to history and storage.
    // 仍存文档回执使用与历史和存储完全相同的行。
    let mut affected = Vec::new();
    for id in changed_ids {
        if let Some(document) = after.get_mut(&id) {
            if document.created_change_seq == 0 {
                document.created_change_seq = seq;
            }
            document.updated_change_seq = seq;
            save_document(&transaction, document)?;
            rebuild_document(&transaction, document, maintain_fts)?;
            affected.push(document.clone());
        }
        transaction.execute("INSERT INTO kb_change_items(change_seq,ordinal,entity,entity_key,before_json,after_json) VALUES(?,?,'document',?,?,?);",
            params![sql_integer(seq)?,ordinal,id,before.get(&id).map(json).transpose()?,after.get(&id).map(json).transpose()?]).map_err(|e|e.to_string())?;
        ordinal += 1;
    }
    for key in changed_folders {
        if let Some(folder) = after_folders.get_mut(&key) {
            if folder.created_change_seq == 0 {
                folder.created_change_seq = seq;
            }
            folder.updated_change_seq = seq;
            transaction.execute("INSERT INTO kb_folders(path_key,path,description,created_change_seq,updated_change_seq) VALUES(?,?,?,?,?) ON CONFLICT(path_key) DO UPDATE SET path=excluded.path,description=excluded.description,created_change_seq=excluded.created_change_seq,updated_change_seq=excluded.updated_change_seq;",
                params![folder.path_key,folder.path,folder.description,sql_integer(folder.created_change_seq)?,sql_integer(folder.updated_change_seq)?]).map_err(|e|e.to_string())?;
        } else {
            transaction
                .execute("DELETE FROM kb_folders WHERE path_key=?;", [&key])
                .map_err(|e| e.to_string())?;
        }
        transaction.execute("INSERT INTO kb_change_items(change_seq,ordinal,entity,entity_key,before_json,after_json) VALUES(?,?,'folder',?,?,?);",
            params![sql_integer(seq)?,ordinal,key,before_folders.get(&key).map(json).transpose()?,after_folders.get(&key).map(json).transpose()?]).map_err(|e|e.to_string())?;
        ordinal += 1;
    }
    // Missing hashes are queued once; ready and exhausted failed hashes remain untouched.
    // 缺失哈希只排队一次；就绪和已耗尽失败哈希保持不变。
    let pending = super::vector::queue_current_embeddings(&transaction, settings)?;
    transaction
        .execute(
            "UPDATE knowledge_metadata SET value=? WHERE key='head_change_seq';",
            [seq.to_string()],
        )
        .map_err(|e| e.to_string())?;
    if index_was_ready {
        transaction
            .execute(
                "UPDATE knowledge_metadata SET value=? WHERE key='indexed_change_seq';",
                [seq.to_string()],
            )
            .map_err(|e| e.to_string())?;
        transaction
            .execute(
                "UPDATE knowledge_metadata SET value='ready' WHERE key='index_status';",
                [],
            )
            .map_err(|e| e.to_string())?;
    }
    transaction.commit().map_err(|e| e.to_string())?;
    Ok(KnowledgeWriteResult {
        change_seq: Some(seq),
        documents: affected,
        unchanged: vec![],
        pending_embeddings: pending,
    })
}

/// Builds the receipt for a call that changed nothing; `unchanged` names the observed paths.
/// 为未产生任何变更的调用构建回执；`unchanged` 列出已观测路径。
/// Returns no change sequence and the real desired-space backlog, without writing history.
/// 返回空变更序号及真实期望空间积压，不写入历史。
fn unchanged_receipt(
    connection: &Connection,
    settings: &KnowledgeLibrarySettings,
    unchanged: Vec<String>,
) -> Result<KnowledgeWriteResult, String> {
    let pending_embeddings = match settings.enabled_embedding() {
        Some(embedding) => {
            super::vector::current_counts(connection, &vector_space_id(embedding)?)?.0
        }
        _ => 0,
    };
    Ok(KnowledgeWriteResult {
        change_seq: None,
        documents: vec![],
        unchanged,
        pending_embeddings,
    })
}

impl KnowledgeBaseRuntime {
    /// Revalidates project authorization while holding the library mutation guard; returns exact settings.
    /// 持库变更锁时重新验证项目授权；返回精确设置。
    async fn content_settings(
        &self,
        library_id: &str,
        project_id: Option<&str>,
    ) -> Result<KnowledgeLibrarySettings, String> {
        // This check runs after admission and before SQLite mutation to close revocation races.
        // 该检查在准入后、SQLite 变更前运行，以封闭撤销竞态。
        let library = self.require_active_library(library_id).await?;
        if let Some(project_id) = project_id
            && !self
                .authorized_library_ids(project_id, false)
                .await?
                .iter()
                .any(|id| id == library_id)
        {
            return Err("ERR_KNOWLEDGE_NOT_AUTHORIZED".into());
        }
        library
            .settings
            .ok_or("ERR_KNOWLEDGE_SETTINGS_MISSING".into())
    }

    /// Applies one AI action to an authorized enabled library; returns its sole atomic change receipt.
    /// 向已授权启用库应用一次 AI 动作；返回唯一原子变更回执。
    pub async fn write_content(
        &self,
        library_id: &str,
        project_id: &str,
        request: KnowledgeWriteRequest,
        actor: KnowledgeChangeActor,
    ) -> Result<KnowledgeWriteResult, String> {
        validate_action_fields(&request)?;
        if actor.kind != "ai"
            || actor.session_id.is_none()
            || actor.turn_id.is_none()
            || actor.tool_call_id.is_none()
        {
            return Err("ERR_KNOWLEDGE_AI_ACTOR_REQUIRED".into());
        }
        // Keep authorization, namespace resolution and commit in one library-exclusive interval.
        // 将授权、命名空间解析和提交保持在同一库独占区间。
        let _guard = self.library_mutation_guard(library_id).await?;
        // Library-owned settings define the derived indexes for this exact commit.
        // 库自身设置定义该次精确提交的派生索引。
        let settings = self.content_settings(library_id, Some(project_id)).await?;
        // Binding validation never upgrades a library.
        // 绑定验证绝不升级知识库。
        let (_, binding_id) = self.ensure_library_binding(library_id).await?;
        let receipt = local_sql::with_connection_async(
            &self.host_runtime.local,
            self.resolve_library_binding(&binding_id)?,
            move |connection| {
                // Candidate metadata is isolated until every validation succeeds.
                // 候选元数据在全部验证成功前保持隔离。
                let before = documents(connection)?;
                // Explicit folder metadata has the same atomic publication boundary.
                // 显式目录元数据具有相同原子发布边界。
                let before_folders = folders(connection)?;
                // Mutations target cloned candidates rather than published current rows.
                // 变更面向克隆候选，而非已发布当前行。
                let mut after = before.clone();
                // Folder candidate changes are committed with document changes.
                // 目录候选变更与文档变更一起提交。
                let mut after_folders = before_folders.clone();
                // New body bytes are keyed by their verified digest.
                // 新正文字节按已验证摘要索引。
                let mut bodies = BTreeMap::new();
                // The logical path named in an unchanged receipt.
                // 无变化回执中列出的逻辑路径。
                let subject: String;
                match request.action.as_str() {
                    "create" => {
                        let mut document = new_document(
                            required(&request.path, "path")?,
                            required(&request.title, "title")?,
                            Some(required(&request.summary, "summary")?),
                            required(&request.body, "body")?,
                            request.embed.unwrap_or_default(),
                            request.sources.clone().unwrap_or_default(),
                        )?;
                        if let Some(keywords) = &request.keywords {
                            document.keywords = keywords.clone();
                        }
                        validate_document(&document, required(&request.body, "body")?)?;
                        bodies.insert(
                            document.body_sha256.clone(),
                            required(&request.body, "body")?.clone(),
                        );
                        subject = document.path.clone();
                        after.insert(document.id.clone(), document);
                    }
                    "folder" | "delete_folder" => {
                        let (path, key) = normalize_path(required(&request.path, "path")?, false)?;
                        subject = path.clone();
                        if request.action == "delete_folder" {
                            if before
                                .values()
                                .any(|d| d.path_key.starts_with(&format!("{key}/")))
                                || before_folders
                                    .keys()
                                    .any(|k| k.starts_with(&format!("{key}/")))
                            {
                                return Err("ERR_KNOWLEDGE_FOLDER_NOT_EMPTY".into());
                            }
                            if after_folders.remove(&key).is_none() {
                                return Err("ERR_KNOWLEDGE_FOLDER_NOT_FOUND".into());
                            }
                        } else {
                            let description = required(&request.description, "description")?;
                            // An identical upsert leaves the published folder untouched.
                            // 完全相同的写入保持已发布目录不变。
                            if !before_folders.get(&key).is_some_and(|folder| {
                                folder.path == path && &folder.description == description
                            }) {
                                after_folders.insert(
                                    key.clone(),
                                    KnowledgeFolder {
                                        path_key: key.clone(),
                                        path,
                                        description: description.clone(),
                                        created_change_seq: before_folders
                                            .get(&key)
                                            .map_or(0, |f| f.created_change_seq),
                                        updated_change_seq: 0,
                                    },
                                );
                            }
                        }
                    }
                    _ => {
                        let id = select_document(
                            &before,
                            request.document_id.as_deref(),
                            request.path.as_deref(),
                        )?;
                        let document = after
                            .get_mut(&id)
                            .ok_or("ERR_KNOWLEDGE_DOCUMENT_NOT_FOUND")?;
                        if *required(&request.expected_revision, "expectedRevision")?
                            != document.revision
                        {
                            return Err("ERR_KNOWLEDGE_REVISION_CONFLICT".into());
                        }
                        subject = document.path.clone();
                        if request.action == "delete" {
                            after.remove(&id);
                        } else {
                            let mut text = body(connection, &document.body_sha256)?;
                            match request.action.as_str() {
                                "update" => {
                                    if let Some(value) = &request.title {
                                        document.title = value.clone();
                                    }
                                    if let Some(value) = &request.summary {
                                        document.summary = value.clone();
                                        document.summary_state = "provided".into();
                                    }
                                    if let Some(value) = &request.body {
                                        text = value.clone();
                                    }
                                    if let Some(value) = &request.keywords {
                                        document.keywords = value.clone();
                                    }
                                    if let Some(value) = &request.embed {
                                        document.embed_policy = *value;
                                    }
                                    if let Some(value) = &request.sources {
                                        document.provenance = value.clone();
                                    }
                                }
                                "edit" => {
                                    if required(&request.edits, "edits")?.is_empty() {
                                        return Err("ERR_KNOWLEDGE_EDITS_EMPTY".into());
                                    }
                                    for edit in required(&request.edits, "edits")? {
                                        if edit.old_text.is_empty() {
                                            return Err("ERR_KNOWLEDGE_EDIT_EMPTY_MATCH".into());
                                        }
                                        let count = text.matches(&edit.old_text).count();
                                        if count == 0 || (count != 1 && !edit.replace_all) {
                                            return Err("ERR_KNOWLEDGE_EDIT_MATCH_COUNT".into());
                                        }
                                        // Rust replacement allocates its complete result; enforce the output budget first.
                                        // Rust 替换会分配完整结果；先执行输出预算检查。
                                        checked_edit_output_bytes(
                                            text.len(),
                                            edit.old_text.len(),
                                            edit.new_text.len(),
                                            if edit.replace_all { count } else { 1 },
                                        )?;
                                        text = if edit.replace_all {
                                            text.replace(&edit.old_text, &edit.new_text)
                                        } else {
                                            text.replacen(&edit.old_text, &edit.new_text, 1)
                                        };
                                    }
                                    if let Some(value) = &request.summary {
                                        document.summary = value.clone();
                                        document.summary_state = "provided".into();
                                    }
                                }
                                "move" => {
                                    let (path, key) = normalize_path(
                                        required(&request.new_path, "newPath")?,
                                        false,
                                    )?;
                                    document.folder_path =
                                        path.rsplit_once('/').map_or("", |(p, _)| p).into();
                                    document.path = path;
                                    document.path_key = key;
                                }
                                _ => return Err("ERR_KNOWLEDGE_ACTION_INVALID".into()),
                            }
                            let body_sha256 = super::super::sha256_hex(text.as_bytes());
                            // Only a real body change invalidates a summary the AI did not resupply.
                            // 仅真实正文变化才使 AI 未重新提供的摘要待复核。
                            if body_sha256 != document.body_sha256
                                && request.summary.is_none()
                                && document.summary_state == "provided"
                            {
                                document.summary_state = "stale".into();
                            }
                            document.body_sha256 = body_sha256;
                            document.line_count = super::super::knowledge_body_lines(&text).count();
                            // A candidate identical to the published row keeps its revision and records nothing.
                            // 与已发布行完全相同的候选保留修订号且不记录任何变更。
                            if before.get(&id) != Some(&*document) {
                                document.revision = document
                                    .revision
                                    .checked_add(1)
                                    .ok_or("ERR_KNOWLEDGE_REVISION_OVERFLOW")?;
                                validate_document(document, &text)?;
                                bodies.insert(document.body_sha256.clone(), text);
                            }
                        }
                    }
                }
                // A call whose candidates equal the published state appends no history.
                // 候选与已发布状态相同的调用不追加历史。
                if after == before && after_folders == before_folders {
                    return unchanged_receipt(connection, &settings, vec![subject]);
                }
                commit_snapshot(
                    connection,
                    SnapshotChange {
                        before: &before,
                        before_folders: &before_folders,
                        after,
                        after_folders,
                        bodies,
                        actor: &actor,
                        operation: &request.action,
                        note: &request.note,
                        restore_target_seq: None,
                        settings: &settings,
                    },
                )
            },
        )
        .await?;
        if receipt.change_seq.is_some() {
            self.wake_knowledge_worker(library_id).await;
        }
        Ok(receipt)
    }

    /// Reads one ID or path with a bounded inclusive line window; returns exact text and continuation.
    /// 按身份或路径读取有界包含边界行窗口；返回精确文本与续读标识。
    pub async fn read_document(
        &self,
        library_id: &str,
        document_id: Option<&str>,
        path: Option<&str>,
        start_line: Option<usize>,
        end_line: Option<usize>,
    ) -> Result<KnowledgeDocumentContent, String> {
        self.require_active_library(library_id).await?;
        // Current and historical reads share the same inclusive range authority.
        // 当前与历史读取共享相同包含边界范围权威。
        let (start, end) = history_read::read_range(start_line, end_line)?;
        // The declared selector chooses exactly one indexed SQL field before loading any metadata.
        // 声明选择器在加载任何元数据前精确选择一个已索引 SQL 字段。
        let (column, value) =
            match (document_id, path) {
                (Some(id), None) => ("id", id.to_string()),
                (None, Some(path)) => ("path_key", normalize_path(path, false)?.1),
                _ => return Err(
                    "ERR_KNOWLEDGE_DOCUMENT_SELECTOR: specify exactly one of documentId or path"
                        .into(),
                ),
            };
        // The snapshot binds the single selected row to its exact content-addressed body.
        // 快照将唯一选中行绑定至精确内容寻址正文。
        let (_, binding) = self.ensure_library_binding(library_id).await?;
        local_sql::with_read_snapshot_async(
            &self.host_runtime.local,
            self.resolve_library_binding(binding)?,
            move |connection| {
                let document = connection
                    .query_row(
                        &format!("SELECT {DOCUMENT_COLUMNS} FROM kb_documents WHERE {column}=?;"),
                        [value],
                        document_row,
                    )
                    .optional()
                    .map_err(|error| error.to_string())?
                    .ok_or("ERR_KNOWLEDGE_DOCUMENT_NOT_FOUND")?;
                let text = body(connection, &document.body_sha256)?;
                history_read::document_window(document, &text, start, end)
            },
        )
        .await
    }

    /// Lists a directory with depth 1–3 and a head-bound cursor; returns deterministic read-only entries.
    /// 按一到三级深度和绑定变更头游标列目录；返回确定的只读条目。
    pub async fn list_content(
        &self,
        library_id: &str,
        path: &str,
        depth: usize,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<KnowledgeListPage, String> {
        self.require_active_library(library_id).await?;
        if !(1..=MAX_KNOWLEDGE_LIST_DEPTH).contains(&depth)
            || limit == 0
            || limit > MAX_KNOWLEDGE_PAGE_SIZE
        {
            return Err("ERR_KNOWLEDGE_LIST_RANGE".into());
        }
        // Cursor scope includes the exact library, normalized directory and requested depth.
        // 游标范围包含精确知识库、规范目录和请求深度。
        let library_id = library_id.to_string();
        // Comparison paths share the write-time normalization authority.
        // 比较路径共享写入时的规范化权威。
        let (_, key) = normalize_path(path, true)?;
        // Own the untrusted cursor before entering the snapshot boundary.
        // 进入快照边界前持有不可信游标。
        let cursor = cursor.map(str::to_string);
        // Metadata and derived ancestors are observed in one consistent snapshot.
        // 元数据和派生祖先在同一一致快照内观测。
        let (_, binding_id) = self.ensure_library_binding(&library_id).await?;
        local_sql::with_read_snapshot_async(
            &self.host_runtime.local,
            self.resolve_library_binding(&binding_id)?,
            move |connection| {
                // Directory listing never loads body blobs.
                // 目录列表绝不加载正文块。
                let docs = documents(connection)?;
                // Explicit descriptions override only the exact matching directory identity.
                // 显式说明只覆盖精确匹配目录身份。
                let explicit = folders(connection)?;
                // The head binds pagination to one immutable namespace generation.
                // 变更头将分页绑定到同一不可变命名空间代次。
                let head = connection
                    .query_row(
                        "SELECT value FROM knowledge_metadata WHERE key='head_change_seq';",
                        [],
                        |r| r.get::<_, String>(0),
                    )
                    .map_err(|e| e.to_string())?
                    .parse::<u64>()
                    .map_err(|e| e.to_string())?;
                // A cursor from another library or scope is always rejected.
                // 来自其他库或范围的游标始终拒绝。
                let scope = super::super::sha256_hex(
                    format!("{library_id}:{head}:{key}:{depth}").as_bytes(),
                );
                // Cursor offsets are checked against the exact sorted entry set below.
                // 游标偏移会对照下方精确排序条目集检查。
                let offset = match cursor {
                    Some(cursor) => {
                        let (cursor_scope, offset) = cursor
                            .split_once(':')
                            .ok_or("ERR_KNOWLEDGE_CURSOR_INVALID")?;
                        if cursor_scope != scope {
                            return Err("ERR_KNOWLEDGE_CURSOR_STALE".into());
                        }
                        offset
                            .parse::<usize>()
                            .map_err(|_| "ERR_KNOWLEDGE_CURSOR_INVALID")?
                    }
                    None => 0,
                };
                // Component-delimited prefixes keep similarly named siblings distinct.
                // 组件分隔前缀使名称相似的同级项保持独立。
                let prefix = if key.is_empty() {
                    String::new()
                } else {
                    format!("{key}/")
                };
                // Relative depth is measured from the requested directory.
                // 相对深度从请求目录计算。
                let base_depth = if key.is_empty() {
                    0
                } else {
                    key.split('/').count()
                };
                // A single map merges explicit folders and ancestors from documents or empty folders.
                // 单一映射合并显式目录以及文档或空目录的祖先。
                let mut folder_paths = BTreeMap::new();
                for path in docs
                    .values()
                    .map(|d| d.folder_path.as_str())
                    .chain(explicit.values().map(|f| f.path.as_str()))
                {
                    // All namespace entries use slash components verified at write time.
                    // 全部命名空间条目使用写入时验证的斜杠组件。
                    let parts: Vec<_> = path.split('/').filter(|part| !part.is_empty()).collect();
                    for count in 1..=parts.len() {
                        // Preserve spelling while deduplicating by normalized comparison identity.
                        // 保留拼写，同时按规范比较身份去重。
                        let ancestor = parts[..count].join("/");
                        // Every derived ancestor goes through the same canonical normalizer.
                        // 每个派生祖先都经过同一规范化入口。
                        let (_, ancestor_key) = normalize_path(&ancestor, false)?;
                        folder_paths.entry(ancestor_key).or_insert(ancestor);
                    }
                }
                // Sort folder and document entries together using stable normalized identities.
                // 使用稳定规范身份将目录和文档条目一起排序。
                let mut entries = BTreeMap::new();
                for (folder_key, path) in folder_paths {
                    if !folder_key.starts_with(&prefix)
                        || path.split('/').count() > base_depth + depth
                    {
                        continue;
                    }
                    // Explicit metadata belongs to exactly this folder, never a neighboring candidate.
                    // 显式元数据精确归属该目录，绝非邻近候选。
                    let folder = explicit.get(&folder_key);
                    entries.insert(
                        (folder_key.clone(), "folder"),
                        KnowledgeListEntry {
                            kind: "folder".into(),
                            path: folder.map_or(path, |folder| folder.path.clone()),
                            description: folder
                                .map_or(String::new(), |folder| folder.description.clone()),
                            document_count: docs
                                .values()
                                .filter(|d| d.path_key.starts_with(&format!("{folder_key}/")))
                                .count(),
                            document: None,
                        },
                    );
                }
                for document in docs.values() {
                    if document.path_key.starts_with(&prefix)
                        && document.path.split('/').count() <= base_depth + depth
                    {
                        entries.insert(
                            (document.path_key.clone(), "document"),
                            KnowledgeListEntry {
                                kind: "document".into(),
                                path: document.path.clone(),
                                description: String::new(),
                                document_count: 1,
                                document: Some(document.clone()),
                            },
                        );
                    }
                }
                // Reject externally fabricated offsets rather than silently returning a misleading empty page.
                // 拒绝外部伪造偏移，避免静默返回误导空页。
                let total = entries.len();
                if offset > total {
                    return Err("ERR_KNOWLEDGE_CURSOR_INVALID".into());
                }
                Ok(KnowledgeListPage {
                    entries: entries
                        .into_values()
                        .skip(offset)
                        .take(limit as usize)
                        .collect(),
                    next_cursor: (offset.saturating_add(limit as usize) < total)
                        .then(|| format!("{scope}:{}", offset + limit as usize)),
                    head_change_seq: head,
                })
            },
        )
        .await
    }
}

/// Decodes a history summary row; returns exact actor identity and affected item count.
/// 解码历史摘要行；返回精确执行者身份和影响条目数。
fn change_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<KnowledgeChange> {
    Ok(KnowledgeChange {
        seq: unsigned_column(row, 0)?,
        at_ms: row.get(1)?,
        actor: KnowledgeChangeActor {
            kind: row.get(2)?,
            session_id: row.get(3)?,
            turn_id: row.get(4)?,
            tool_call_id: row.get(5)?,
        },
        operation: row.get(6)?,
        note: row.get(7)?,
        restore_target_seq: row
            .get::<_, Option<i64>>(8)?
            .map(|value| u64::try_from(value).map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        item_count: unsigned_column(row, 9)?,
    })
}

/// Fixed history projection shared by list and detail APIs.
/// 历史列表与详情接口共享的固定投影。
const CHANGE_COLUMNS: &str = "c.seq,c.at_ms,c.actor_kind,c.session_id,c.turn_id,c.tool_call_id,c.operation,c.note,c.restore_target_seq,(SELECT COUNT(*) FROM kb_change_items i WHERE i.change_seq=c.seq)";

/// Compares user-visible document state excluding revision and audit sequence metadata.
/// 比较用户可见文档状态，排除修订与审计序号元数据。
fn same_document_content(
    first: &KnowledgeDocumentSummary,
    second: &KnowledgeDocumentSummary,
) -> bool {
    // Revision tokens are deliberately regenerated by restores rather than copied backward.
    // 还原时修订令牌刻意重新生成，绝不向后复制。
    let mut first = first.clone();
    // Normalize only audit metadata for the semantic comparison.
    // 仅为语义比较规范审计元数据。
    let mut second = second.clone();
    first.revision = 0;
    first.created_change_seq = 0;
    first.updated_change_seq = 0;
    second.revision = 0;
    second.created_change_seq = 0;
    second.updated_change_seq = 0;
    first == second
}

impl KnowledgeBaseRuntime {
    /// Lists history before an optional exclusive sequence with a bounded page; returns newest first.
    /// 在可选排他序号前有界分页列历史；按最新优先返回。
    pub async fn list_changes(
        &self,
        library_id: &str,
        before_seq: Option<u64>,
        limit: u32,
    ) -> Result<Vec<KnowledgeChange>, String> {
        self.require_active_library(library_id).await?;
        if limit == 0 || limit > MAX_KNOWLEDGE_PAGE_SIZE {
            return Err("ERR_KNOWLEDGE_HISTORY_RANGE".into());
        }
        // Validate bindings before dispatch so unsupported formats are never queried.
        // 派发前验证绑定，确保不查询不支持格式。
        let (_, binding_id) = self.ensure_library_binding(library_id).await?;
        local_sql::with_read_snapshot_async(&self.host_runtime.local,self.resolve_library_binding(&binding_id)?,move |connection| {
            let mut statement=connection.prepare(&format!("SELECT {CHANGE_COLUMNS} FROM kb_changes c WHERE (? IS NULL OR c.seq<?) ORDER BY c.seq DESC LIMIT ?;")).map_err(|e|e.to_string())?;
            statement.query_map(params![before_seq.map(sql_integer).transpose()?,before_seq.map(sql_integer).transpose()?,limit],change_row).map_err(|e|e.to_string())?.map(|r|r.map_err(|e|e.to_string())).collect()
        }).await
    }

    /// Reads the first bounded page of exact `seq` images in `library_id`; returns an explicit continuation.
    /// 读取 `library_id` 中精确 `seq` 镜像的首个有界页；返回显式续读标识。
    pub async fn get_change(
        &self,
        library_id: &str,
        seq: u64,
    ) -> Result<KnowledgeChangeDetail, String> {
        self.get_change_page(library_id, seq, 0, MAX_KNOWLEDGE_PAGE_SIZE)
            .await
    }

    /// Restores exact content at target_seq as one new human change; retains all prior history.
    /// 将目标序号精确内容还原为一条新人工变更；保留全部历史。
    /// Sequence zero means the original empty library; restored revisions never move backward.
    /// 序号零表示最初空库；还原后的修订绝不倒退。
    pub async fn restore_content(
        &self,
        library_id: &str,
        target_seq: u64,
        actor: KnowledgeChangeActor,
        note: &str,
    ) -> Result<KnowledgeWriteResult, String> {
        if actor.kind != "user" {
            return Err("ERR_KNOWLEDGE_RESTORE_USER_REQUIRED".into());
        }
        // Human restore shares the AI mutation domain, closing restore-versus-write races.
        // 人工还原共享 AI 变更域，封闭还原与写入竞态。
        let _guard = self.library_mutation_guard(library_id).await?;
        // Restoring content preserves the current human-owned indexing policy.
        // 还原内容保留当前人工拥有的索引策略。
        let settings = self.content_settings(library_id, None).await?;
        // Own the audit note before crossing the blocking connection boundary.
        // 跨越阻塞连接边界前持有审计说明。
        let note = note.to_string();
        // All reverse image reads and the final commit occur on one guarded connection.
        // 全部逆向镜像读取与最终提交在同一受守卫连接完成。
        let (_, binding_id) = self.ensure_library_binding(library_id).await?;
        let receipt = local_sql::with_connection_async(&self.host_runtime.local,self.resolve_library_binding(&binding_id)?,move |connection| {
            if target_seq!=0 && !connection.query_row("SELECT EXISTS(SELECT 1 FROM kb_changes WHERE seq=?);",[sql_integer(target_seq)?],|r|r.get::<_,bool>(0)).map_err(|e|e.to_string())? {return Err("ERR_KNOWLEDGE_CHANGE_NOT_FOUND".into());}
            let before=documents(connection)?;
            let before_folders=folders(connection)?;
            let mut after=before.clone();
            let mut after_folders=before_folders.clone();
            {
                let mut statement=connection.prepare("SELECT i.entity,i.entity_key,i.before_json FROM kb_change_items i WHERE i.change_seq>? ORDER BY i.change_seq DESC,i.ordinal DESC;").map_err(|e|e.to_string())?;
                let rows=statement.query_map([sql_integer(target_seq)?],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?))).map_err(|e|e.to_string())?;
                for row in rows {
                    let (entity,key,image)=row.map_err(|e|e.to_string())?;
                    match entity.as_str() {
                        "document"=>if let Some(image)=image {after.insert(key,serde_json::from_str(&image).map_err(|e|e.to_string())?);}else{after.remove(&key);},
                        "folder"=>if let Some(image)=image {after_folders.insert(key,serde_json::from_str(&image).map_err(|e|e.to_string())?);}else{after_folders.remove(&key);},
                        _=>return Err("ERR_KNOWLEDGE_HISTORY_ENTITY_INVALID".into())
                    }
                }
            }
            for (id,document) in &mut after {
                if let Some(current)=before.get(id)
                    && same_document_content(current,document) {*document=current.clone();continue;}
                // A deleted then restored document must also reject tokens issued before deletion.
                // 已删除后还原的文档也必须拒绝删除前签发的令牌。
                let previous_revision:u64=connection.query_row("SELECT COALESCE(MAX(revision),0) FROM (SELECT CAST(json_extract(before_json,'$.revision') AS INTEGER) AS revision FROM kb_change_items WHERE entity='document' AND entity_key=? UNION ALL SELECT CAST(json_extract(after_json,'$.revision') AS INTEGER) FROM kb_change_items WHERE entity='document' AND entity_key=?);",params![id,id],|r|unsigned_column(r,0)).map_err(|e|e.to_string())?;
                document.revision=previous_revision.checked_add(1).ok_or("ERR_KNOWLEDGE_REVISION_OVERFLOW")?;
            }
            for (key,folder) in &mut after_folders {
                if let Some(current)=before_folders.get(key)
                    && folder.path==current.path && folder.description==current.description {*folder=current.clone();}
            }
            commit_snapshot(connection, SnapshotChange { before: &before, before_folders: &before_folders, after, after_folders, bodies: BTreeMap::new(), actor: &actor, operation: "restore", note: &note, restore_target_seq: Some(target_seq), settings: &settings })
        }).await?;
        self.wake_knowledge_worker(library_id).await;
        Ok(receipt)
    }

    /// Ingests prevalidated host-decoded files atomically; unchanged source hashes produce no change.
    /// 原子搬运预验证的宿主解码文件；来源哈希未变时不产生变更。
    /// Rechecks authorization after locking and returns one receipt for every changed file in the call.
    /// 加锁后重新检查授权，为调用内全部变化文件返回一份回执。
    pub async fn ingest_documents(
        &self,
        library_id: &str,
        project_id: &str,
        input: Vec<KnowledgeIngestDocument>,
        actor: KnowledgeChangeActor,
        note: &str,
    ) -> Result<KnowledgeWriteResult, String> {
        if input.len() > MAX_KNOWLEDGE_INGEST_FILES
            || input
                .iter()
                .try_fold(0usize, |sum, d| sum.checked_add(d.body.len()))
                .ok_or("ERR_KNOWLEDGE_INGEST_LIMIT")?
                > MAX_KNOWLEDGE_INGEST_BYTES
        {
            return Err("ERR_KNOWLEDGE_INGEST_LIMIT".into());
        }
        if actor.kind != "ai"
            || actor.session_id.is_none()
            || actor.turn_id.is_none()
            || actor.tool_call_id.is_none()
        {
            return Err("ERR_KNOWLEDGE_AI_ACTOR_REQUIRED".into());
        }
        // Authorization revocation cannot interleave with a partially ingested batch.
        // 授权撤销不能与部分搬运批次交错。
        let _guard = self.library_mutation_guard(library_id).await?;
        // Index policy is captured for the same library-exclusive interval.
        // 索引策略在同一库独占区间捕获。
        let settings = self.content_settings(library_id, Some(project_id)).await?;
        // Preserve exact audit text across the connection boundary.
        // 跨连接边界保留精确审计文本。
        let note = note.to_string();
        // Source bodies never cross the AI result path.
        // 来源正文绝不跨越 AI 结果路径。
        let (_, binding_id) = self.ensure_library_binding(library_id).await?;
        let receipt = local_sql::with_connection_async(
            &self.host_runtime.local,
            self.resolve_library_binding(&binding_id)?,
            move |connection| {
                let before = documents(connection)?;
                let before_folders = folders(connection)?;
                let mut after = before.clone();
                let mut bodies = BTreeMap::new();
                let mut unchanged = Vec::new();
                let mut seen = BTreeSet::new();
                for source in input {
                    if source.provenance.kind != "file" || source.provenance.sha256.is_none() {
                        return Err("ERR_KNOWLEDGE_INGEST_PROVENANCE".into());
                    }
                    let (path, key) = normalize_path(&source.path, false)?;
                    if !seen.insert(key.clone()) {
                        return Err("ERR_KNOWLEDGE_INGEST_DUPLICATE_TARGET".into());
                    }
                    let existing = before.values().find(|d| d.path_key == key);
                    // Ingest refreshes only its own mirror; other documents need a revision-checked update.
                    // 搬运只刷新自身镜像；其他文档必须经过修订校验的更新。
                    if let Some(current) = existing
                        && !current.provenance.iter().any(|p| {
                            p.kind == "file" && p.reference == source.provenance.reference
                        })
                    {
                        return Err(format!(
                            "ERR_KNOWLEDGE_INGEST_TARGET_EXISTS: {} already holds a document not ingested from this source; choose another targetFolder or use update with expectedRevision",
                            current.path
                        ));
                    }
                    if let Some(current) = existing
                        && current.provenance.iter().any(|p| {
                            p.kind == "file"
                                && p.reference == source.provenance.reference
                                && p.sha256 == source.provenance.sha256
                        })
                    {
                        unchanged.push(path);
                        continue;
                    }
                    let mut document = if let Some(current) = existing {
                        let mut document = current.clone();
                        document.title = source.title;
                        document.body_sha256 = super::super::sha256_hex(source.body.as_bytes());
                        document.line_count =
                            super::super::knowledge_body_lines(&source.body).count();
                        document.revision = document
                            .revision
                            .checked_add(1)
                            .ok_or("ERR_KNOWLEDGE_REVISION_OVERFLOW")?;
                        document.provenance = vec![source.provenance];
                        document.embed_policy = source.embed;
                        if let Some(summary) = source.summary {
                            document.summary = summary;
                            document.summary_state = "provided".into();
                        } else {
                            document.summary_state = if document.summary.is_empty() {
                                "pending"
                            } else {
                                "stale"
                            }
                            .into();
                        }
                        document
                    } else {
                        new_document(
                            &path,
                            &source.title,
                            source.summary.as_deref(),
                            &source.body,
                            source.embed,
                            vec![source.provenance],
                        )?
                    };
                    validate_document(&document, &source.body)?;
                    document.updated_change_seq = 0;
                    bodies.insert(document.body_sha256.clone(), source.body);
                    after.insert(document.id.clone(), document);
                }
                if bodies.is_empty() {
                    return unchanged_receipt(connection, &settings, unchanged);
                }
                let mut receipt = commit_snapshot(
                    connection,
                    SnapshotChange {
                        before: &before,
                        before_folders: &before_folders,
                        after,
                        after_folders: before_folders.clone(),
                        bodies,
                        actor: &actor,
                        operation: "ingest",
                        note: &note,
                        restore_target_seq: None,
                        settings: &settings,
                    },
                )?;
                receipt.unchanged = unchanged;
                Ok(receipt)
            },
        )
        .await?;
        self.wake_knowledge_worker(library_id).await;
        Ok(receipt)
    }
}

#[cfg(test)]
mod edit_allocation_tests {
    use super::*;

    /// Output admission uses UTF-8 byte lengths and permits exact-boundary shrink or unchanged-size edits.
    /// 输出准入使用 UTF-8 字节长度，并允许精确边界缩减或大小不变的编辑。
    #[test]
    fn replacement_size_preserves_utf8_and_shared_boundaries() {
        // These source and expected values prove byte accounting without locking user-facing wording.
        // 这些来源及预期值证明字节记账，不锁定面向用户措辞。
        let (text, old, new, expected) = ("éé", "é", "汉", "汉汉");
        assert_eq!(
            checked_edit_output_bytes(text.len(), old.len(), new.len(), 2).unwrap(),
            expected.len()
        );
        assert_eq!(
            checked_edit_output_bytes(MAX_KNOWLEDGE_BODY_BYTES, 1, 1, MAX_KNOWLEDGE_BODY_BYTES)
                .unwrap(),
            MAX_KNOWLEDGE_BODY_BYTES
        );
        assert_eq!(
            checked_edit_output_bytes(MAX_KNOWLEDGE_BODY_BYTES, 1, 0, MAX_KNOWLEDGE_BODY_BYTES)
                .unwrap(),
            0
        );
    }

    /// Amplification and usize overflow reject using lengths alone, without allocating dangerous text.
    /// 放大及 usize 溢出仅凭长度即被拒绝，不分配危险文本。
    #[test]
    fn repeated_expansion_and_arithmetic_overflow_are_rejected_before_allocation() {
        for (text, old, new, count) in [
            (
                MAX_KNOWLEDGE_BODY_BYTES,
                1,
                MAX_KNOWLEDGE_BODY_BYTES,
                MAX_KNOWLEDGE_BODY_BYTES,
            ),
            (1, 1, usize::MAX, 2),
            (usize::MAX, 1, usize::MAX, 2),
            (usize::MAX, 1, usize::MAX, 1),
            (usize::MAX, usize::MAX, 0, 2),
            (MAX_KNOWLEDGE_BODY_BYTES, 0, 1, 1),
        ] {
            assert_eq!(
                checked_edit_output_bytes(text, old, new, count).unwrap_err(),
                "ERR_KNOWLEDGE_BODY_TOO_LARGE"
            );
        }
    }
}
