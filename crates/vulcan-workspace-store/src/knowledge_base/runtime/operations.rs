//! Durable knowledge operations and explicit index-mode lifecycle.
//! 持久知识库操作与显式索引模式生命周期。

use super::*;
use rusqlite::{OptionalExtension, params};

/// Exact row projection shared by latest and stable-ID operation queries.
/// 最新操作与稳定身份查询共享的精确行投影。
const OPERATION_COLUMNS: &str = "id,library_id,kind,state,phase,total_items,processed_items,current_item,warnings_json,error_message,created_at_ms,started_at_ms,finished_at_ms";

/// Decodes a persistent operation; returns checked counters and structured warnings.
/// 解码持久操作；返回已检查计数与结构化告警。
fn operation_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<KnowledgeOperationSummary> {
    Ok(KnowledgeOperationSummary {
        id: row.get(0)?,
        library_id: row.get(1)?,
        kind: row.get(2)?,
        state: row.get(3)?,
        phase: row.get(4)?,
        total_items: unsigned_column(row, 5)?,
        processed_items: unsigned_column(row, 6)?,
        current_item: row.get(7)?,
        warnings: serde_json::from_str(&row.get::<_, String>(8)?).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, Box::new(e))
        })?,
        error_message: row.get(9)?,
        created_at_ms: row.get(10)?,
        started_at_ms: row.get(11)?,
        finished_at_ms: row.get(12)?,
    })
}

impl KnowledgeBaseRuntime {
    /// Queues a durable derived-index rebuild for `library_id`; returns its stable operation identity.
    /// 为 `library_id` 排队持久派生索引重建；返回稳定操作身份。
    /// Existing content/history and ready vectors remain authoritative; exhausted vectors require explicit retry.
    /// 已有内容、历史及就绪向量保持权威；耗尽向量需要显式重试。
    pub async fn rebuild_index_operation(
        &self,
        library_id: &str,
    ) -> Result<KnowledgeOperationSummary, String> {
        // Rebuild admission shares the library's sole mutation boundary.
        // 重建准入共享知识库唯一变更边界。
        let guard = self.library_mutation_guard(library_id).await?;
        // The persisted mode determines which derived capabilities are required.
        // 持久模式决定需要哪些派生能力。
        let settings = self
            .require_active_library(library_id)
            .await?
            .settings
            .ok_or("ERR_KNOWLEDGE_SETTINGS_MISSING")?;
        // Admit one stable job before marking work pending.
        // 标记工作待处理前接纳一个稳定任务。
        let operation = self
            .begin_operation_locked(library_id, "rebuild", None)
            .await?;
        // The background worker handles FTS rebuild and missing vector-table recovery.
        // 后台工作器处理全文重建及缺失向量表恢复。
        let (_, binding) = self.ensure_library_binding(library_id).await?;
        local_sql::with_connection_async(
            &self.host_runtime.local,
            self.resolve_library_binding(binding)?,
            move |connection| {
                let transaction = connection
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .map_err(|error| error.to_string())?;
                if settings.index_mode.has_fts() {
                    transaction
                        .execute("UPDATE knowledge_metadata SET value='building' WHERE key='index_status';", [])
                        .map_err(|error| error.to_string())?;
                }
                transaction
                    .execute(
                        "UPDATE knowledge_metadata SET value='0' WHERE key='vector_worker_paused';",
                        [],
                    )
                    .map_err(|error| error.to_string())?;
                transaction.commit().map_err(|error| error.to_string())
            },
        )
        .await?;
        drop(guard);
        self.wake_knowledge_worker(library_id).await;
        Ok(operation)
    }

    /// Reads a stable operation ID or latest library operation; returns absence without inventing progress.
    /// 读取稳定操作身份或库最新操作；缺失时不虚构进度。
    pub async fn get_operation(
        &self,
        library_id: &str,
        operation_id: Option<&str>,
    ) -> Result<Option<KnowledgeOperationSummary>, String> {
        self.require_active_library(library_id).await?;
        // Both selectors retain the owning library boundary.
        // 两种选择器均保留所属知识库边界。
        let library_id = library_id.to_string();
        // A stable operation ID never falls back to the latest operation.
        // 稳定操作身份绝不兜底为最新操作。
        let operation_id = operation_id.map(str::to_string);
        local_sql::with_read_snapshot_async(&self.host_runtime.local,LogicalDatabase::AppState,move |connection|{
            match operation_id{
                Some(id)=>connection.query_row(&format!("SELECT {OPERATION_COLUMNS} FROM knowledge_operations WHERE library_id=? AND id=?;"),params![library_id,id],operation_row).optional().map_err(|e|e.to_string()),
                None=>connection.query_row(&format!("SELECT {OPERATION_COLUMNS} FROM knowledge_operations WHERE library_id=? ORDER BY created_at_ms DESC,rowid DESC LIMIT 1;"),[library_id],operation_row).optional().map_err(|e|e.to_string())
            }
        }).await
    }

    /// Creates one durable job while the caller holds the library lock; optionally freezes a new target mode.
    /// 调用方持库锁时创建一个持久任务；可同时冻结新目标模式。
    async fn begin_operation_locked(
        &self,
        library_id: &str,
        kind: &str,
        settings: Option<&KnowledgeLibrarySettings>,
    ) -> Result<KnowledgeOperationSummary, String> {
        // Allocate the operation identity before writing the application transaction.
        // 写入应用事务前分配操作身份。
        let id = Uuid::new_v4().to_string();
        // Creation time is also the job's first progress reference.
        // 创建时间也是任务首次进度基准。
        let now = unix_epoch_millis()?;
        // Mode updates and job admission share the application transaction.
        // 模式更新和任务准入共享应用事务。
        let settings = settings
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| e.to_string())?;
        // Own immutable arguments across the blocking connection boundary.
        // 跨阻塞连接边界持有不可变参数。
        let library = library_id.to_string();
        // Job kind is validated by its public lifecycle entry point.
        // 任务类别由公共生命周期入口验证。
        let operation_kind = kind.to_string();
        // Preserve the public identity for the stable-ID read after commit.
        // 保留公共身份供提交后稳定身份读取。
        let operation_id = id.clone();
        local_sql::with_connection_async(&self.host_runtime.local,LogicalDatabase::AppState,move |connection|{
            let transaction=connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(|e|e.to_string())?;
            let busy=transaction.query_row("SELECT EXISTS(SELECT 1 FROM knowledge_operations WHERE library_id=? AND state IN ('queued','running'));",[&library],|r|r.get::<_,bool>(0)).map_err(|e|e.to_string())?;
            if busy{return Err("ERR_KNOWLEDGE_OPERATION_BUSY".into());}
            if let Some(settings)=settings{
                transaction.execute("UPDATE knowledge_libraries SET settings_json=?,updated_at_ms=? WHERE id=? AND lifecycle_state='active';",params![settings,now,library]).map_err(|e|e.to_string())?;
            }
            transaction.execute("INSERT INTO knowledge_operations(id,library_id,kind,state,phase,created_at_ms) VALUES(?,?,?,'queued','queued',?);",params![operation_id,library,operation_kind,now]).map_err(|e|e.to_string())?;
            transaction.commit().map_err(|e|e.to_string())
        }).await?;
        self.get_operation(library_id, Some(&id))
            .await?
            .ok_or("ERR_KNOWLEDGE_OPERATION_NOT_FOUND".into())
    }

    /// Saves the requested mode and optional embedding for `library_id`; returns a durable catch-up job.
    /// 为 `library_id` 保存所请求模式及可选嵌入配置；返回持久补索引任务。
    /// Disabled capabilities retain their last selection and on-disk indexes.
    /// 暂停的检索能力保留原配置与磁盘索引。
    pub async fn set_index_mode(
        &self,
        library_id: &str,
        mut settings: KnowledgeLibrarySettings,
    ) -> Result<KnowledgeOperationSummary, String> {
        // All mode transitions serialize with content, restore and deletion.
        // 全部模式过渡与内容、还原和删除串行化。
        let guard = self.library_mutation_guard(library_id).await?;
        let current = self
            .require_active_library(library_id)
            .await?
            .settings
            .ok_or("ERR_KNOWLEDGE_SETTINGS_MISSING")?;
        if !settings.index_mode.has_vectors() {
            settings.embedding = current.embedding;
        }
        settings.validate()?;
        // A durable application job makes partial cross-database work resumable after restart.
        // 持久应用任务使跨数据库部分工作可在重启后恢复。
        let operation = self
            .begin_operation_locked(library_id, "index_mode", Some(&settings))
            .await?;
        // Expose the exact new backlog before returning; the worker also queues it on recovery.
        // 返回前公开精确的新积压；后台工作器在恢复时也会排队。
        if settings.enabled_embedding().is_some() {
            let (_, binding) = self.ensure_library_binding(library_id).await?;
            local_sql::with_connection_async(
                &self.host_runtime.local,
                self.resolve_library_binding(binding)?,
                move |connection| {
                    vector::queue_current_embeddings(connection, &settings).map(|_| ())
                },
            )
            .await?;
        }
        drop(guard);
        self.wake_knowledge_worker(library_id).await;
        Ok(operation)
    }

    /// Retries exhausted embeddings for all current failed hashes or exact selected documents; returns a job.
    /// 对全部当前失败哈希或精确所选文档重试耗尽嵌入；返回任务。
    pub async fn retry_embeddings(
        &self,
        library_id: &str,
        document_ids: &[String],
    ) -> Result<KnowledgeOperationSummary, String> {
        // Retry admission and failed-state reset use the same mutation guard.
        // 重试准入和失败状态重置使用同一变更守卫。
        let guard = self.library_mutation_guard(library_id).await?;
        // Browse and BM25 libraries cannot schedule paid vector inference.
        // 浏览和全文库不能调度付费向量推理。
        let settings = self
            .require_active_library(library_id)
            .await?
            .settings
            .ok_or("ERR_KNOWLEDGE_SETTINGS_MISSING")?;
        // The desired space is explicit; retry never changes routes or dimensions.
        // 期望空间显式确定；重试绝不改变路由或维度。
        let embedding = settings
            .enabled_embedding()
            .ok_or("ERR_KNOWLEDGE_VECTOR_MODE_REQUIRED")?;
        // Use semantic identity rather than a mutable table generation.
        // 使用语义身份而非可变表代次。
        let space = content::vector_space_id(embedding)?;
        // Document membership is validated before allocating a durable operation.
        // 分配持久操作前验证文档成员资格。
        let (_, binding) = self.ensure_library_binding(library_id).await?;
        // Own selected stable document IDs for the connection closure.
        // 为连接闭包持有所选稳定文档身份。
        let ids = document_ids.to_vec();
        local_sql::with_read_snapshot_async(
            &self.host_runtime.local,
            self.resolve_library_binding(&binding)?,
            {
                let ids = ids.clone();
                move |connection| {
                    for id in ids {
                        if !connection
                            .query_row(
                                "SELECT EXISTS(SELECT 1 FROM kb_documents WHERE id=?);",
                                [id],
                                |r| r.get::<_, bool>(0),
                            )
                            .map_err(|e| e.to_string())?
                        {
                            return Err("ERR_KNOWLEDGE_DOCUMENT_NOT_FOUND".into());
                        }
                    }
                    Ok(())
                }
            },
        )
        .await?;
        // The job is durable before failed work becomes eligible again.
        // 失败工作重新具备资格前任务已持久化。
        let operation = self
            .begin_operation_locked(library_id, "retry_embeddings", None)
            .await?;
        local_sql::with_connection_async(&self.host_runtime.local,self.resolve_library_binding(binding)?,move |connection|{
            let transaction=connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(|e|e.to_string())?;
            transaction.execute("UPDATE kb_vector_state SET state='pending',attempts=0,last_error='',updated_at_ms=? WHERE vector_space_id=? AND state='failed' AND EXISTS(SELECT 1 FROM kb_chunks c WHERE c.content_sha256=kb_vector_state.content_sha256 AND c.embed=1 AND (?='[]' OR c.document_id IN (SELECT value FROM json_each(?))));",
                params![unix_epoch_millis()?,space,serde_json::to_string(&ids).map_err(|e|e.to_string())?,serde_json::to_string(&ids).map_err(|e|e.to_string())?]).map_err(|e|e.to_string())?;
            transaction.execute("INSERT INTO knowledge_metadata(key,value) VALUES('vector_worker_paused','0') ON CONFLICT(key) DO UPDATE SET value='0';",[]).map_err(|e|e.to_string())?;
            transaction.commit().map_err(|e|e.to_string())
        }).await?;
        drop(guard);
        self.wake_knowledge_worker(library_id).await;
        Ok(operation)
    }

    /// Cancels only the exact currently active job and pauses its worker at the next safe boundary.
    /// 仅取消精确当前活动任务，并在下一安全边界暂停其工作器。
    pub async fn cancel_operation(
        &self,
        library_id: &str,
        operation_id: &str,
    ) -> Result<bool, String> {
        // Cancellation cannot modify an unrelated library or a newer operation.
        // 取消不能修改无关知识库或更新的操作。
        let _guard = self.library_mutation_guard(library_id).await?;
        self.require_active_library(library_id).await?;
        // Freeze exact stable selectors across the application transaction.
        // 跨应用事务冻结精确稳定选择器。
        let id = operation_id.to_string();
        // Library ownership is part of the cancellation predicate.
        // 知识库归属是取消谓词的一部分。
        let library = library_id.to_string();
        // Return whether an active operation actually transitioned.
        // 返回活动操作是否实际发生过渡。
        let cancelled=local_sql::with_connection_async(&self.host_runtime.local,LogicalDatabase::AppState,move |connection|{
            connection.execute("UPDATE knowledge_operations SET state='cancelled',phase='cancelled',finished_at_ms=? WHERE id=? AND library_id=? AND state IN ('queued','running');",params![unix_epoch_millis()?,id,library]).map(|count|count==1).map_err(|e|e.to_string())
        }).await?;
        if cancelled {
            let (_, binding) = self.ensure_library_binding(library_id).await?;
            local_sql::execute_script_async(&self.host_runtime.local,self.resolve_library_binding(binding)?,
                "INSERT INTO knowledge_metadata(key,value) VALUES('vector_worker_paused','1') ON CONFLICT(key) DO UPDATE SET value='1';",vec![]).await?;
        }
        Ok(cancelled)
    }

    /// Updates only queued/running jobs for the locked library; terminal jobs never resurrect.
    /// 仅更新持锁知识库的排队或运行任务；终止任务绝不复活。
    pub(super) async fn update_knowledge_operation_locked(
        &self,
        library_id: &str,
        state: &str,
        total: u64,
        processed: u64,
        error: Option<&str>,
    ) -> Result<(), String> {
        // Terminal timestamps derive from the one host clock.
        // 终止时间戳从唯一宿主时钟派生。
        let now = unix_epoch_millis()?;
        local_sql::execute_script_async(&self.host_runtime.local,LogicalDatabase::AppState,
            "UPDATE knowledge_operations SET state=?,phase=?,total_items=?,processed_items=?,error_message=?,started_at_ms=COALESCE(started_at_ms,?),finished_at_ms=CASE WHEN ? IN ('completed','failed','cancelled') THEN ? ELSE NULL END WHERE library_id=? AND state IN ('queued','running');",
            vec![text(state),text(state),int(sql_integer(total)?),int(sql_integer(processed)?),error.map_or(SqliteValue::Null,text),int(now),text(state),int(now),text(library_id)]).await?;
        Ok(())
    }

    /// Restarts only unfinished jobs and unpaused current-library work; does not revive cancelled operations.
    /// 仅重启未完成任务与未暂停的当前库工作；不复活已取消操作。
    pub async fn recover_interrupted_jobs(&self) -> Result<(), String> {
        local_sql::execute_script_async(&self.host_runtime.local,LogicalDatabase::AppState,
            "UPDATE knowledge_operations SET state='queued',phase='queued',error_message=NULL,finished_at_ms=NULL WHERE state='running';",vec![]).await?;
        for library in self.list_libraries(None).await? {
            if library.lifecycle_state == "active" {
                self.wake_knowledge_worker(&library.id).await;
            }
        }
        Ok(())
    }
}
