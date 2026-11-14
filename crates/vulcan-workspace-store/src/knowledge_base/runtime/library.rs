//! Library registry lifecycle and exact current-format projection.
//! 知识库注册生命周期与精确当前格式投影。

use super::*;
use rusqlite::OptionalExtension;
use serde::Deserialize;

/// Exact application registry row; settings are parsed only after format validation.
/// 精确应用注册行；仅在格式验证后解析设置。
#[derive(Deserialize)]
struct LibraryRow {
    /// Immutable UUID.
    /// 不可变 UUID。
    id: String,
    /// Human name.
    /// 人工名称。
    name: String,
    /// Human purpose.
    /// 人工用途。
    description: String,
    /// Durable lifecycle.
    /// 持久生命周期。
    lifecycle_state: String,
    /// Exact persisted policy bytes.
    /// 精确持久策略字节。
    settings_json: String,
    /// Creation timestamp.
    /// 创建时间。
    created_at_ms: i64,
    /// Update timestamp.
    /// 更新时间。
    updated_at_ms: i64,
}

impl KnowledgeBaseRuntime {
    /// Creates an empty library with explicit name, purpose, project bindings and validated settings.
    /// 使用显式名称、用途、项目绑定和已验证设置创建空库。
    /// Returns the current registry projection or rolls back the new registry and storage on failure.
    /// 返回当前注册投影；失败则回滚新注册记录和存储。
    pub async fn create_library(
        &self,
        name: &str,
        description: &str,
        project_ids: &[String],
        settings: &KnowledgeLibrarySettings,
    ) -> Result<KnowledgeLibrarySummary, String> {
        // Allocate the immutable owner used by every embedding request and derived profile.
        // 分配全部嵌入请求及派生配置档使用的不可变所有者。
        let library_id = Uuid::new_v4().to_string();
        // The complete creation interval excludes writes, deletion and project revocation.
        // 完整创建区间排除写入、删除及项目撤销。
        let guard = self.library_mutation_guard(&library_id).await?;
        // Route snapshots contain no consumer identity; the library ID is the sole owner.
        // 路由快照不包含消费方身份；知识库身份为唯一所有者。
        let settings = settings.clone();
        settings.validate()?;
        // Only human metadata is trimmed; content paths use their separate strict authority.
        // 仅修剪人工元数据；内容路径使用各自严格权威。
        let name = validate_library_name(name)?;
        // One timestamp binds the registry and initial project authorizations.
        // 一个时间戳绑定注册记录及初始项目授权。
        let now = unix_epoch_millis()?;
        // No registry row or partial project authorization is visible until storage is initialized.
        // 存储初始化前不暴露注册行或部分项目授权。
        let initialized = async {
            self.initialize_library_storage(&library_id, &name).await?;
            // The new empty index can be built directly while this method owns the mutation lock.
            // 此方法拥有变更锁时，可直接构建新空索引。
            let (_, binding) = self.ensure_library_binding(&library_id).await?;
            local_sql::with_connection_async(&self.host_runtime.local, self.resolve_library_binding(binding)?, {
                let enabled = settings.index_mode.has_fts();
                move |connection| {
                    if enabled {
                        fts::rebuild_fts(connection)?;
                    }
                    Ok(())
                }
            }).await?;
            // Registry publication and every initial binding form one application transaction.
            // 注册发布及每项初始绑定构成一笔应用事务。
            let id = library_id.clone();
            // Own human fields before entering the blocking database boundary.
            // 进入阻塞数据库边界前持有人工字段。
            let name = name.clone();
            // Initial project identities are explicit caller input, never inferred from workspace paths.
            // 初始项目身份为显式调用方输入，绝不从工作区路径推断。
            let projects = project_ids.to_vec();
            // Persist the already validated policy without post-publication rewriting.
            // 持久化已验证策略，不在发布后改写。
            let policy = serde_json::to_string(&settings).map_err(|error| error.to_string())?;
            // Purpose is human metadata and has no AI content side effect.
            // 用途为人工元数据，不产生 AI 内容副作用。
            let description = description.trim().to_string();
            local_sql::with_connection_async(&self.host_runtime.local, LogicalDatabase::AppState, move |connection| {
                let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(|error| error.to_string())?;
                transaction.execute("INSERT INTO knowledge_libraries(id,name,name_key,description,lifecycle_state,settings_json,created_at_ms,updated_at_ms) VALUES(?,?,?,?,'active',?,?,?);", rusqlite::params![id,name,name.to_lowercase(),description,policy,now,now]).map_err(|error|error.to_string())?;
                for project in projects {
                    transaction.execute("INSERT INTO knowledge_library_project_bindings(library_id,project_id,default_enabled,created_at_ms,updated_at_ms) VALUES(?,?,1,?,?) ON CONFLICT(library_id,project_id) DO NOTHING;", rusqlite::params![id,project,now,now]).map_err(|error|error.to_string())?;
                }
                transaction.commit().map_err(|error|error.to_string())
            }).await?;
            self.get_library(&library_id).await
        }.await;
        if initialized.is_err() {
            self.release_library_binding(&library_id).await?;
            local_sql::execute_script_async(
                &self.host_runtime.local,
                LogicalDatabase::AppState,
                "DELETE FROM knowledge_libraries WHERE id=?;",
                vec![text(&library_id)],
            )
            .await?;
            self.remove_host_owned_child_directory_if_exists(
                &self.library_root(&library_id)?,
                &library_id,
            )?;
        }
        drop(guard);
        if initialized.is_ok() && settings.index_mode.has_vectors() {
            // Empty vector libraries become usable without sending any inference request.
            // 空向量库无需发送任何推理请求即可可用。
            self.wake_knowledge_worker(&library_id).await;
        }
        initialized
    }

    /// Lists all libraries or a project's authorized libraries, validating each exact current format.
    /// 列出全部或项目已授权知识库，并验证每个精确当前格式。
    /// Returns errors as library lifecycle projections so an unsupported library remains deletable.
    /// 将库错误作为生命周期投影返回，确保不支持的库仍可删除。
    pub async fn list_libraries(
        &self,
        project_id: Option<&str>,
    ) -> Result<Vec<KnowledgeLibrarySummary>, String> {
        // Binding filters are explicit and parameterized.
        // 绑定过滤显式且参数化。
        let (sql, params) = match project_id {
            Some(project) => (
                "SELECT l.id,l.name,l.description,l.lifecycle_state,l.settings_json,l.created_at_ms,l.updated_at_ms FROM knowledge_libraries l JOIN knowledge_library_project_bindings b ON b.library_id=l.id WHERE b.project_id=? ORDER BY l.name_key;",
                vec![text(project)],
            ),
            None => (
                "SELECT id,name,description,lifecycle_state,settings_json,created_at_ms,updated_at_ms FROM knowledge_libraries ORDER BY name_key;",
                vec![],
            ),
        };
        // Registry identity discovery never reads legacy content.
        // 注册身份发现不读取旧内容。
        let rows: Vec<LibraryRow> = self.query_app_rows(sql, &params).await?;
        // Preserve library-local failures without hiding unrelated libraries.
        // 保留库内错误，不隐藏其他知识库。
        let mut summaries = Vec::with_capacity(rows.len());
        for row in rows {
            summaries.push(self.project_library(row).await?);
        }
        Ok(summaries)
    }

    /// Reads one exact registry identity and returns a validated projection or a missing-ID error.
    /// 读取一个精确注册身份，返回已验证投影或身份缺失错误。
    pub async fn get_library(&self, library_id: &str) -> Result<KnowledgeLibrarySummary, String> {
        validate_library_id(library_id)?;
        // Decode only current registry columns, independently of per-library format.
        // 仅解码当前注册列，与每库格式独立。
        let rows: Vec<LibraryRow> = self.query_app_rows(
            "SELECT id,name,description,lifecycle_state,settings_json,created_at_ms,updated_at_ms FROM knowledge_libraries WHERE id=?;",
            &[text(library_id)]).await?;
        self.project_library(rows.into_iter().next().ok_or("ERR_KNOWLEDGE_NOT_FOUND")?)
            .await
    }

    /// Projects exact counts and policy for a registry row; invalid formats become explicit error state.
    /// 为注册行投影精确数量和策略；非法格式成为明确错误状态。
    async fn project_library(&self, row: LibraryRow) -> Result<KnowledgeLibrarySummary, String> {
        // Start with no policy so invalid data is never interpreted using a default mode.
        // 初始策略为空，确保非法数据绝不按默认模式解释。
        let mut summary = KnowledgeLibrarySummary {
            id: row.id.clone(),
            name: row.name,
            description: row.description,
            settings: None,
            lifecycle_state: row.lifecycle_state,
            error_message: None,
            document_count: 0,
            chunk_count: 0,
            pending_summary_count: 0,
            index_status: "error".into(),
            head_change_seq: 0,
            last_change_at_ms: None,
            created_at_ms: row.created_at_ms,
            updated_at_ms: row.updated_at_ms,
        };
        if summary.lifecycle_state == "deleting" {
            return Ok(summary);
        }
        // Version validation precedes settings deserialization and every content query.
        // 版本验证先于设置反序列化与全部内容查询。
        let inspected = async {
            let (_, binding_id) = self.ensure_library_binding(&row.id).await?;
            let settings: KnowledgeLibrarySettings = serde_json::from_str(&row.settings_json)
                .map_err(|e| format!("ERR_KNOWLEDGE_SETTINGS_FORMAT: {e}"))?;
            settings.validate()?;
            let stats = local_sql::with_read_snapshot_async(&self.host_runtime.local,
                self.resolve_library_binding(&binding_id)?, |connection| {
                    connection.query_row("SELECT (SELECT COUNT(*) FROM kb_documents),(SELECT COUNT(*) FROM kb_chunks),(SELECT COUNT(*) FROM kb_documents WHERE summary_state!='provided'),(SELECT COALESCE(MAX(seq),0) FROM kb_changes),(SELECT MAX(at_ms) FROM kb_changes),(SELECT value FROM knowledge_metadata WHERE key='index_status');",
                        [], |r| Ok((unsigned_column::<u64>(r,0)?, unsigned_column::<u64>(r,1)?, unsigned_column::<u64>(r,2)?, unsigned_column::<u64>(r,3)?, r.get::<_,Option<i64>>(4)?, r.get::<_,String>(5)?))).map_err(|e| e.to_string())
                }).await?;
            Ok::<_,String>((settings,stats))
        }.await;
        match inspected {
            Ok((settings, (documents, chunks, pending, head, last, status))) => {
                summary.index_status = if !settings.index_mode.has_fts() {
                    "disabled".into()
                } else if matches!(status.as_str(), "empty" | "disabled") {
                    "building".into()
                } else {
                    status
                };
                summary.settings = Some(settings);
                summary.document_count = documents;
                summary.chunk_count = chunks;
                summary.pending_summary_count = pending;
                summary.head_change_seq = head;
                summary.last_change_at_ms = last;
            }
            Err(error) => {
                summary.lifecycle_state = "error".into();
                summary.error_message = Some(error);
            }
        }
        Ok(summary)
    }

    /// Requires an active current-format library; returns its exact settings projection.
    /// 要求活动的当前格式知识库；返回其精确设置投影。
    pub(super) async fn require_active_library(
        &self,
        library_id: &str,
    ) -> Result<KnowledgeLibrarySummary, String> {
        // Every caller shares format validation, including writes after project authorization.
        // 所有调用共享格式验证，包括项目授权后的写入。
        let library = self.get_library(library_id).await?;
        if library.lifecycle_state != "active" {
            return Err(library
                .error_message
                .unwrap_or_else(|| "ERR_KNOWLEDGE_LIBRARY_INACTIVE".into()));
        }
        Ok(library)
    }

    /// Updates only human name and purpose; returns refreshed metadata without touching content.
    /// 仅更新人工名称与用途；返回刷新元数据且不修改内容。
    pub async fn rename_library(
        &self,
        library_id: &str,
        name: &str,
        description: &str,
    ) -> Result<KnowledgeLibrarySummary, String> {
        // Serialize metadata with content changes and deletion.
        // 将元数据与内容变更和删除串行化。
        let _guard = self.library_mutation_guard(library_id).await?;
        self.require_active_library(library_id).await?;
        // Human names retain registry-level uniqueness.
        // 人工名称保留注册级唯一约束。
        let name = validate_library_name(name)?;
        local_sql::execute_script_async(&self.host_runtime.local, LogicalDatabase::AppState,
            "UPDATE knowledge_libraries SET name=?,name_key=?,description=?,updated_at_ms=? WHERE id=?;",
            vec![text(&name),text(name.to_lowercase()),text(description.trim()),int(unix_epoch_millis()?),text(library_id)]).await?;
        self.write_manifest_atomic(&KnowledgeManifest {
            library_id: library_id.into(),
            name,
            schema_version: KNOWLEDGE_SCHEMA_VERSION,
        })?;
        self.get_library(library_id).await
    }

    /// Revokes and deletes an exact host-owned library, including unsupported formats; returns success on removal.
    /// 撤销并删除精确宿主知识库，包括不支持格式；移除成功后返回。
    pub async fn delete_library(&self, library_id: &str) -> Result<(), String> {
        // Unsupported formats can still be deleted without opening their content.
        // 不支持格式仍可删除，无需打开其内容。
        let _guard = self.library_mutation_guard(library_id).await?;
        self.delete_library_locked(library_id).await
    }
}

/// Validates a human library name; returns a trimmed nonempty bounded value.
/// 验证人工库名称；返回裁剪后的非空有界值。
pub(super) fn validate_library_name(name: &str) -> Result<String, String> {
    if name.trim().is_empty()
        || name.trim().chars().count() > MAX_KNOWLEDGE_PATH_SEGMENT_CHARS
        || name.chars().any(char::is_control)
    {
        return Err("ERR_KNOWLEDGE_NAME_INVALID".into());
    }
    Ok(name.trim().into())
}

impl KnowledgeBaseRuntime {
    /// Removes an already locked registry identity and exact host-owned storage; returns explicit I/O failures.
    /// 移除已持锁注册身份及精确宿主存储；返回明确输入输出错误。
    async fn delete_library_locked(&self, library_id: &str) -> Result<(), String> {
        // Ensure this identity really belongs to the registry before removing storage.
        // 移除存储前确保该身份确实属于注册表。
        let exists = local_sql::with_read_snapshot_async(
            &self.host_runtime.local,
            LogicalDatabase::AppState,
            {
                let id = library_id.to_string();
                move |c| {
                    c.query_row("SELECT id FROM knowledge_libraries WHERE id=?", [id], |r| {
                        r.get::<_, String>(0)
                    })
                    .optional()
                    .map_err(|e| e.to_string())
                }
            },
        )
        .await?;
        if exists.is_none() {
            return Err("ERR_KNOWLEDGE_NOT_FOUND".into());
        }
        local_sql::execute_script_async(
            &self.host_runtime.local,
            LogicalDatabase::AppState,
            "UPDATE knowledge_libraries SET lifecycle_state='deleting',updated_at_ms=? WHERE id=?;",
            vec![int(unix_epoch_millis()?), text(library_id)],
        )
        .await?;
        // Existing vector readers finish before their exact host bindings are closed.
        // 已有向量读取者在精确宿主绑定关闭前完成。
        let _publication = self.vector_space_lock(library_id).await.write_owned().await;
        self.release_library_binding(library_id).await?;
        self.remove_host_owned_child_directory_if_exists(
            &self.library_root(library_id)?,
            library_id,
        )?;
        local_sql::execute_script_async(
            &self.host_runtime.local,
            LogicalDatabase::AppState,
            "DELETE FROM knowledge_libraries WHERE id=?;",
            vec![text(library_id)],
        )
        .await
        .map(|_| ())
    }
    /// Cleans a failed new installation only while it still has no content history or current rows.
    /// 仅在仍无内容历史或当前行时清理失败新安装。
    /// The empty check and deletion share one lock so concurrent AI or human content cannot be discarded.
    /// 空检查和删除共享一把锁，确保并发 AI 或人工内容不会被丢弃。
    pub(super) async fn discard_empty_library(&self, library_id: &str) -> Result<(), String> {
        // Never infer emptiness from an unreadable or unsupported format.
        // 绝不从不可读或不支持的格式推断空状态。
        let _guard = self.library_mutation_guard(library_id).await?;
        // Counts and head come from exact current-format SQLite authority.
        // 数量和变更头来自精确当前格式 SQLite 权威。
        let library = self.require_active_library(library_id).await?;
        if library.head_change_seq != 0 || library.document_count != 0 || library.chunk_count != 0 {
            return Err(
                "ERR_KNOWLEDGE_IMPORT_CLEANUP_CONFLICT: changed content was preserved".into(),
            );
        }
        self.delete_library_locked(library_id).await
    }
}
