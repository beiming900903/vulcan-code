//! Content-addressed vector spaces and resumable host-driven embedding workers.
//! 内容寻址向量空间与可恢复宿主嵌入工作器。

use super::*;
use rusqlite::{Connection, params};
use std::sync::atomic::{AtomicU64, Ordering};
use vulcan_lancedb_engine::{engine::LanceDbEngine, error::LanceDbEngineError, types::*};

/// One reserved batch; its exact vector space remains frozen during asynchronous inference.
/// 一份已保留批次；异步推理期间精确向量空间保持冻结。
struct EmbeddingBatch {
    /// Frozen request route and dimensions.
    /// 冻结请求路由和维度。
    embedding: KnowledgeEmbeddingSettings,
    /// Semantic space identity.
    /// 语义空间身份。
    space: String,
    /// Ordered distinct input hashes and text.
    /// 有序去重输入哈希与文本。
    inputs: Vec<(String, String)>,
}

/// Requires one exact metadata value; returns corruption if the declared key is absent.
/// 要求精确元数据值；声明键缺失时返回损坏。
fn metadata(connection: &Connection, key: &str) -> Result<String, String> {
    connection
        .query_row(
            "SELECT value FROM knowledge_metadata WHERE key=?;",
            [key],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())
}

/// Reads the explicitly nullable active embedding snapshot; no alternate route is inferred.
/// 读取显式可空的活动嵌入快照；不推断替代路由。
pub(super) fn active_embedding(
    connection: &Connection,
) -> Result<Option<KnowledgeEmbeddingSettings>, String> {
    serde_json::from_str(&metadata(connection, "active_embedding")?).map_err(|e| e.to_string())
}

/// Reads the exact persisted pause flag; rejects every undeclared representation.
/// 读取精确持久暂停标记；拒绝全部未声明表示。
fn paused(connection: &Connection) -> Result<bool, String> {
    match metadata(connection, "vector_worker_paused")?.as_str() {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err("ERR_KNOWLEDGE_WORKER_PAUSE_INVALID".into()),
    }
}

/// Queues current embedding hashes without resetting ready or exhausted states; returns pending count.
/// 排队当前嵌入哈希且不重置就绪或耗尽状态；返回待处理数。
pub(super) fn queue_current_embeddings(
    connection: &Connection,
    settings: &KnowledgeLibrarySettings,
) -> Result<u64, String> {
    // Only enabled vector work can queue hashes for the saved selection.
    // 只有启用向量能力时，才为已保存配置排队内容哈希。
    let Some(embedding) = settings.enabled_embedding() else {
        return Ok(0);
    };
    // Semantic identity excludes batch configuration.
    // 语义身份排除批量配置。
    let space = content::vector_space_id(embedding)?;
    connection.execute("INSERT OR IGNORE INTO kb_vector_state(content_sha256,vector_space_id,state,attempts,last_error,updated_at_ms) SELECT DISTINCT content_sha256,?,'pending',0,'',? FROM kb_chunks WHERE embed=1;",
        params![space,unix_epoch_millis()?]).map_err(|e|e.to_string())?;
    // Content admission must preserve human cancellation; only explicit lifecycle actions resume work.
    // 内容准入必须保留人工取消；仅明确生命周期动作恢复工作。
    current_counts(connection, &space).map(|(pending, _, _)| pending)
}

/// Returns distinct pending, ready and failed hashes still referenced by current embedded chunks.
/// 返回仍被当前嵌入切块引用的去重待处理、就绪和失败哈希数。
pub(super) fn current_counts(
    connection: &Connection,
    space: &str,
) -> Result<(u64, u64, u64), String> {
    connection.query_row("SELECT COALESCE(SUM(state='pending'),0),COALESCE(SUM(state='ready'),0),COALESCE(SUM(state='failed'),0) FROM kb_vector_state s WHERE vector_space_id=? AND EXISTS(SELECT 1 FROM kb_chunks c WHERE c.content_sha256=s.content_sha256 AND c.embed=1);",
        [space],|r|Ok((unsigned_column(r,0)?,unsigned_column(r,1)?,unsigned_column(r,2)?))).map_err(|e|e.to_string())
}

/// Validates a SHA-256 space identity and derives the sole host-owned Lance table name.
/// 验证 SHA-256 空间身份并派生唯一宿主 Lance 表名。
pub(super) fn table_name(space: &str) -> Result<String, String> {
    validate_sha256(space)?;
    Ok(format!("kb_{space}"))
}

/// Validates the exact lowercase SHA-256 wire shape before SQL or table-name interpolation.
/// 在 SQL 或表名插值前验证精确小写 SHA-256 线格式。
pub(super) fn validate_sha256(value: &str) -> Result<(), String> {
    if value.len() != KNOWLEDGE_SHA256_HEX_CHARS
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("ERR_KNOWLEDGE_SHA256_INVALID".into());
    }
    Ok(())
}

impl KnowledgeBaseRuntime {
    /// Installs the host executor and resumes eligible work; fake executors use the identical pipeline.
    /// 安装宿主执行器并恢复具备资格工作；假执行器使用相同管线。
    pub async fn set_embedding_executor(
        &self,
        executor: KnowledgeEmbeddingExecutor,
    ) -> Result<(), String> {
        *self.shared.embedding_executor.write().await = Some(executor);
        self.recover_interrupted_jobs().await
    }

    /// Coalesces a wake into exactly one worker per library; committed writes never fail from scheduling.
    /// 将唤醒合并为每库恰好一个工作器；已提交写入不会因调度而失败。
    pub async fn wake_knowledge_worker(&self, library_id: &str) {
        // The map lock closes the idle-exit versus new-write wake race.
        // 映射锁封闭空闲退出与新写入唤醒竞态。
        let mut workers = self.shared.vector_workers.lock().await;
        if let Some(epoch) = workers.get(library_id) {
            epoch.fetch_add(1, Ordering::SeqCst);
            return;
        }
        // A worker owns one wake epoch until it removes itself under the same map lock.
        // 工作器在同一映射锁下移除自身前拥有一份唤醒代次。
        let epoch = Arc::new(AtomicU64::new(1));
        workers.insert(library_id.into(), epoch.clone());
        // The spawned worker retains the established host rather than constructing another data plane.
        // 派生工作器持有既有宿主，而非构建另一个数据面。
        let runtime = self.clone();
        // Own the canonical target across the asynchronous task lifetime.
        // 跨异步任务生命周期持有规范目标。
        let library_id = library_id.to_string();
        tokio::spawn(async move {
            loop {
                let observed = epoch.load(Ordering::SeqCst);
                let result = runtime.embedding_step(&library_id).await;
                if let Err(error) = &result {
                    let _guard = runtime.library_mutation_guard(&library_id).await;
                    if let Err(record_error) = runtime
                        .update_knowledge_operation_locked(&library_id, "failed", 0, 0, Some(error))
                        .await
                    {
                        tracing::error!(
                            "Failed to record knowledge worker failure: {record_error}"
                        );
                    }
                }
                if matches!(result, Ok(true)) {
                    continue;
                }
                let mut workers = runtime.shared.vector_workers.lock().await;
                if epoch.load(Ordering::SeqCst) != observed {
                    drop(workers);
                    continue;
                }
                workers.remove(&library_id);
                return;
            }
        });
    }

    /// Reports desired-space progress and a bounded current-document failure page; no history hashes leak in counts.
    /// 报告期望空间进度和有界当前文档失败页；计数不混入历史哈希。
    pub async fn embedding_status(
        &self,
        library_id: &str,
        limit: u32,
        offset: u64,
    ) -> Result<KnowledgeEmbeddingStatus, String> {
        if limit == 0 || limit > MAX_KNOWLEDGE_PAGE_SIZE {
            return Err("ERR_KNOWLEDGE_EMBEDDING_PAGE_RANGE".into());
        }
        // The desired route comes only from current library policy.
        // 期望路由仅来自当前知识库策略。
        let settings = self
            .require_active_library(library_id)
            .await?
            .settings
            .ok_or("ERR_KNOWLEDGE_SETTINGS_MISSING")?;
        // Status is read without scheduling or modifying vector work.
        // 状态读取不调度或修改向量工作。
        let (_, binding) = self.ensure_library_binding(library_id).await?;
        local_sql::with_read_snapshot_async(&self.host_runtime.local,self.resolve_library_binding(binding)?,move |connection|{
            let desired=settings.enabled_embedding().map(content::vector_space_id).transpose()?;
            let active=if desired.is_some(){active_embedding(connection)?.as_ref().map(content::vector_space_id).transpose()?}else{None};
            let paused=paused(connection)?;
            let (pending,ready,failed)=match &desired{Some(space)=>current_counts(connection,space)?,None=>(0,0,0)};
            let mut failures=Vec::new();
            if let Some(space)=&desired{
                let mut statement=connection.prepare("SELECT d.id,d.path,c.id,c.content_sha256,s.attempts,s.last_error FROM kb_chunks c JOIN kb_documents d ON d.id=c.document_id JOIN kb_vector_state s ON s.content_sha256=c.content_sha256 WHERE s.vector_space_id=? AND s.state='failed' AND c.embed=1 ORDER BY d.path_key,c.ordinal LIMIT ? OFFSET ?;").map_err(|e|e.to_string())?;
                failures=statement.query_map(params![space,limit+1,sql_integer(offset)?],|r|Ok(KnowledgeEmbeddingFailure{document_id:r.get(0)?,path:r.get(1)?,chunk_id:r.get(2)?,content_sha256:r.get(3)?,attempts:r.get(4)?,last_error:r.get(5)?})).map_err(|e|e.to_string())?.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())?;
            }
            let next_offset=if failures.len()>limit as usize{failures.truncate(limit as usize);Some(offset.checked_add(u64::from(limit)).ok_or("ERR_KNOWLEDGE_INTEGER_RANGE")?)}else{None};
            Ok(KnowledgeEmbeddingStatus{desired_space_id:desired,active_space_id:active,pending,ready,failed,paused,failures,next_offset})
        }).await
    }

    /// Opens the exact host vector engine; a search cannot create a missing artifact.
    /// 打开精确宿主向量引擎；检索不能创建缺失产物。
    pub(super) async fn knowledge_vector_engine(
        &self,
        library_id: &str,
        create: bool,
    ) -> Result<LanceDbEngine, String> {
        // Binding activation is host-shared across independently rebuilt wrappers.
        // 绑定激活在独立重建包装之间由宿主共享。
        let (_, binding) = self.activate_vector_binding(library_id, create).await?;
        self.resolve_vector_engine(binding).await
    }

    /// Creates only a missing derived table and queues invalidated ready hashes; never overwrites an existing space.
    /// 仅创建缺失派生表并排队失效就绪哈希；绝不覆盖已有空间。
    async fn ensure_vector_table_locked(
        &self,
        library_id: &str,
        embedding: &KnowledgeEmbeddingSettings,
    ) -> Result<(), String> {
        // Table identity is derived from the exact semantic space.
        // 表身份从精确语义空间派生。
        let space = content::vector_space_id(embedding)?;
        // User-controlled route strings never enter a Lance table name.
        // 用户控制的路由字符串绝不进入 Lance 表名。
        let table = table_name(&space)?;
        // Creation is restricted to the explicit background construction path.
        // 创建仅限显式后台构建路径。
        let engine = self.knowledge_vector_engine(library_id, true).await?;
        match engine
            .table_statistics(LanceDbTableStatisticsInput {
                table_name: table.clone(),
                compaction: Default::default(),
            })
            .await
        {
            Ok(_) => {}
            Err(LanceDbEngineError::TableNotFound { .. }) => {
                engine
                    .create_table(LanceDbCreateTableInput {
                        table_name: table,
                        columns: vec![
                            LanceDbColumnDef {
                                name: "id".into(),
                                column_type: LanceDbColumnType::String,
                                vector_dim: 0,
                                nullable: false,
                            },
                            LanceDbColumnDef {
                                name: "vector".into(),
                                column_type: LanceDbColumnType::VectorFloat32,
                                vector_dim: embedding.dimensions,
                                nullable: false,
                            },
                        ],
                        overwrite_if_exists: false,
                    })
                    .await
                    .map_err(|e| e.to_string())?;
                let (_, binding) = self.ensure_library_binding(library_id).await?;
                local_sql::execute_script_async(&self.host_runtime.local,self.resolve_library_binding(binding)?,
                    "UPDATE kb_vector_state SET state='pending',attempts=0,last_error='The vector table was missing; regeneration is queued',updated_at_ms=? WHERE vector_space_id=? AND state='ready';",
                    vec![int(unix_epoch_millis()?),text(&space)]).await?;
            }
            Err(error) => return Err(error.to_string()),
        }
        // Persist exact ownership for removal and crash-safe orphan cleanup.
        // 持久化精确归属，供删除与崩溃安全孤立清理。
        let (_, binding) = self.ensure_library_binding(library_id).await?;
        local_sql::execute_script_async(
            &self.host_runtime.local,
            self.resolve_library_binding(binding)?,
            "INSERT INTO knowledge_metadata(key,value) VALUES(?,?) ON CONFLICT(key) DO NOTHING;",
            vec![
                text(format!("vector_table:{space}")),
                text(serde_json::to_string(embedding).map_err(|e| e.to_string())?),
            ],
        )
        .await?;
        Ok(())
    }

    /// Removes obsolete vector spaces after publishing `keep`; caller owns the library lock.
    /// 发布 `keep` 后移除过期向量空间；调用方拥有知识库锁。
    async fn remove_vector_tables_locked(
        &self,
        library_id: &str,
        keep: &str,
    ) -> Result<(), String> {
        // Publication waits for queries holding the old space through inference and mapping.
        // 发布等待在推理与映射期间持有旧空间的查询。
        let _publication = self.vector_space_lock(library_id).await.write_owned().await;
        // Ownership keys are the exact set of tables this library constructed.
        // 归属键精确限定该库构建的表集合。
        let (_, binding) = self.ensure_library_binding(library_id).await?;
        validate_sha256(keep)?;
        // Read only explicit space registrations, not guessed directory candidates.
        // 仅读取显式空间注册，不猜测目录候选。
        let spaces=local_sql::with_read_snapshot_async(&self.host_runtime.local,self.resolve_library_binding(&binding)?,|connection|{
            let mut statement=connection.prepare("SELECT substr(key,length('vector_table:')+1) FROM knowledge_metadata WHERE key LIKE 'vector_table:%';").map_err(|e|e.to_string())?;
            statement.query_map([],|r|r.get::<_,String>(0)).map_err(|e|e.to_string())?.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())
        }).await?;
        for space in spaces {
            if keep == space {
                continue;
            }
            let table = table_name(&space)?;
            let engine = self.knowledge_vector_engine(library_id, false).await?;
            match engine
                .drop_table(LanceDbDropTableInput { table_name: table })
                .await
            {
                Ok(_) | Err(LanceDbEngineError::TableNotFound { .. }) => {}
                Err(error) => return Err(error.to_string()),
            }
            local_sql::with_connection_async(
                &self.host_runtime.local,
                self.resolve_library_binding(&binding)?,
                move |connection| {
                    let transaction = connection
                        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                        .map_err(|e| e.to_string())?;
                    transaction
                        .execute(
                            "DELETE FROM knowledge_metadata WHERE key=?;",
                            [format!("vector_table:{space}")],
                        )
                        .map_err(|e| e.to_string())?;
                    transaction
                        .execute(
                            "DELETE FROM kb_vector_state WHERE vector_space_id=?;",
                            [space],
                        )
                        .map_err(|e| e.to_string())?;
                    transaction.commit().map_err(|e| e.to_string())
                },
            )
            .await?;
        }
        Ok(())
    }
}

impl KnowledgeBaseRuntime {
    /// Returns the host-owned reader/publication lock for one exact library.
    /// 返回一个精确知识库的宿主读取与发布锁。
    pub(super) async fn vector_space_lock(&self, library_id: &str) -> Arc<tokio::sync::RwLock<()>> {
        // All callers acquire the library mutation guard before taking this lock.
        // 全部调用方在获取该锁前先获取知识库变更守卫。
        self.shared
            .vector_space_locks
            .lock()
            .await
            .entry(library_id.into())
            .or_insert_with(|| Arc::new(tokio::sync::RwLock::new(())))
            .clone()
    }

    /// Reads authoritative durable intent and reconciles explicit pause or resume while the caller holds the library mutation guard.
    /// 调用方持有库变更守卫时，读取权威持久意图并协调明确暂停或恢复。
    /// Parameters: library_id owns the operation; binding selects its verified current-format metadata. Returns the latest operation or an explicit storage error.
    /// 参数：library_id 拥有操作；binding 选择其已验证当前格式元数据。返回最新操作或明确存储错误。
    async fn knowledge_worker_operation_locked(
        &self,
        library_id: &str,
        binding: &str,
    ) -> Result<Option<KnowledgeOperationSummary>, String> {
        // Durable explicit intent is authoritative across the application and per-library commits.
        // 持久明确意图在应用及每库提交之间保持权威。
        let operation = self.get_operation(library_id, None).await?;
        // Only an accepted explicit active operation may clear an older human pause.
        // 仅已接受的明确活动操作可以清除较早人工暂停。
        let pause = match operation.as_ref().map(|operation| operation.state.as_str()) {
            Some("cancelled") => Some("1"),
            Some("queued" | "running") => Some("0"),
            Some("completed" | "failed") | None => None,
            Some(_) => return Err("ERR_KNOWLEDGE_OPERATION_STATE_INVALID".into()),
        };
        if let Some(pause) = pause {
            local_sql::execute_script_async(
                &self.host_runtime.local,
                self.resolve_library_binding(binding)?,
                "INSERT INTO knowledge_metadata(key,value) VALUES('vector_worker_paused',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value WHERE knowledge_metadata.value<>excluded.value;",
                vec![text(pause)],
            ).await?;
        }
        Ok(operation)
    }

    /// Reserves the next exact batch or publishes a complete space; caller receives no speculative route.
    /// 保留下个精确批次或发布完整空间；调用方不会收到猜测路由。
    async fn prepare_embedding_batch(
        &self,
        library_id: &str,
    ) -> Result<Option<EmbeddingBatch>, String> {
        // One mutation domain protects settings, current chunk membership and attempt reservations.
        // 一个变更域保护设置、当前切块成员和尝试保留。
        let _guard = self.library_mutation_guard(library_id).await?;
        // Invalid or deleted libraries stop before any derived artifact can be recreated.
        // 非法或已删除库在重建任何派生产物前停止。
        let settings = self
            .require_active_library(library_id)
            .await?
            .settings
            .ok_or("ERR_KNOWLEDGE_SETTINGS_MISSING")?;
        // Resolve the exact existing content binding.
        // 解析精确已有内容绑定。
        let (_, binding) = self.ensure_library_binding(library_id).await?;
        // Application job intent is authoritative across a crash between two database commits.
        // 应用任务意图在两个数据库提交间崩溃后仍为权威。
        let operation = self
            .knowledge_worker_operation_locked(library_id, &binding)
            .await?;
        if operation
            .as_ref()
            .is_some_and(|operation| operation.state == "cancelled")
        {
            return Ok(None);
        }
        // A queued rebuild must actually rebuild even if its metadata mark was interrupted.
        // 即使元数据标记被中断，排队重建也必须实际重建。
        let rebuild_requested = operation
            .as_ref()
            .is_some_and(|operation| operation.kind == "rebuild" && operation.state == "queued");
        // Reconcile interrupted FTS work before reserving paid vector requests.
        // 保留付费向量请求前协调中断的全文工作。
        let is_paused = local_sql::with_connection_async(
            &self.host_runtime.local,
            self.resolve_library_binding(&binding)?,
            {
                let settings = settings.clone();
                move |connection| {
                    if paused(connection)? {
                        return Ok(true);
                    }
                    if settings.index_mode.has_fts()
                        && (rebuild_requested || !fts::is_current(connection)?)
                    {
                        let transaction = connection
                            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                            .map_err(|e| e.to_string())?;
                        fts::rebuild_fts(&transaction)?;
                        transaction.commit().map_err(|e| e.to_string())?;
                    }
                    Ok(false)
                }
            },
        )
        .await?;
        if is_paused {
            return Ok(None);
        }
        // A nonvector target completes after FTS reconciliation and keeps its saved vectors dormant.
        // 非向量目标在全文协调后完成，保存的向量保持停用。
        let Some(embedding) = settings.enabled_embedding().cloned() else {
            let count = local_sql::with_read_snapshot_async(
                &self.host_runtime.local,
                self.resolve_library_binding(&binding)?,
                |connection| {
                    connection
                        .query_row("SELECT COUNT(*) FROM kb_chunks;", [], |r| {
                            unsigned_column(r, 0)
                        })
                        .map_err(|e| e.to_string())
                },
            )
            .await?;
            self.update_knowledge_operation_locked(library_id, "completed", count, count, None)
                .await?;
            return Ok(None);
        };
        self.ensure_vector_table_locked(library_id, &embedding)
            .await?;
        // The space identity binds attempt counters to this exact frozen model space.
        // 空间身份将尝试计数绑定到该精确冻结模型空间。
        let space = content::vector_space_id(&embedding)?;
        // Reserve attempts before inference; a crash consumes the accepted attempt rather than duplicating it silently.
        // 推理前保留尝试；崩溃消耗已接受尝试，而非静默重复。
        let inputs=local_sql::with_connection_async(&self.host_runtime.local,self.resolve_library_binding(&binding)?,{
            let embedding=embedding.clone();
            let settings=settings.clone();
            let space=space.clone();
            move |connection|{
                let transaction=connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(|e|e.to_string())?;
                queue_current_embeddings(&transaction,&settings)?;
                transaction.execute("UPDATE kb_vector_state SET state='failed',last_error='Embedding was interrupted at the automatic attempt limit',updated_at_ms=? WHERE vector_space_id=? AND state='pending' AND attempts>=?;",
                    params![unix_epoch_millis()?,space,MAX_KNOWLEDGE_EMBEDDING_ATTEMPTS]).map_err(|e|e.to_string())?;
                let inputs={
                    let mut statement=transaction.prepare("SELECT s.content_sha256,MIN(c.text) FROM kb_vector_state s JOIN kb_chunks c ON c.content_sha256=s.content_sha256 WHERE s.vector_space_id=? AND s.state='pending' AND s.attempts<? AND c.embed=1 GROUP BY s.content_sha256 ORDER BY s.content_sha256 LIMIT ?;").map_err(|e|e.to_string())?;
                    statement.query_map(params![space,MAX_KNOWLEDGE_EMBEDDING_ATTEMPTS,embedding.batch_size],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).map_err(|e|e.to_string())?.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())?
                };
                for (hash,_) in &inputs{
                    transaction.execute("UPDATE kb_vector_state SET attempts=attempts+1,updated_at_ms=? WHERE vector_space_id=? AND content_sha256=? AND state='pending';",params![unix_epoch_millis()?,space,hash]).map_err(|e|e.to_string())?;
                }
                transaction.commit().map_err(|e|e.to_string())?;
                Ok(inputs)
            }
        }).await?;
        // Progress is a live projection of current hashes, including new writes admitted during a build.
        // 进度是当前哈希的实时投影，包括构建期间准入的新写入。
        let counts = local_sql::with_read_snapshot_async(
            &self.host_runtime.local,
            self.resolve_library_binding(&binding)?,
            {
                let space = space.clone();
                move |connection| current_counts(connection, &space)
            },
        )
        .await?;
        if inputs.is_empty() {
            let (pending, ready, failed) = counts;
            if pending == 0 && failed == 0 {
                // Publish before cleanup, then wait for old readers before dropping the old space.
                // 清理前发布，随后等待旧读取者结束再删除旧空间。
                local_sql::execute_script_async(
                    &self.host_runtime.local,
                    self.resolve_library_binding(&binding)?,
                    "UPDATE knowledge_metadata SET value=? WHERE key='active_embedding';",
                    vec![text(
                        serde_json::to_string(&Some(&embedding)).map_err(|e| e.to_string())?,
                    )],
                )
                .await?;
                self.remove_vector_tables_locked(library_id, &space).await?;
                self.update_knowledge_operation_locked(library_id, "completed", ready, ready, None)
                    .await?;
            } else {
                self.update_knowledge_operation_locked(
                    library_id,
                    "failed",
                    pending + ready + failed,
                    ready,
                    Some("ERR_KNOWLEDGE_EMBEDDINGS_FAILED: exhausted items require human retry"),
                )
                .await?;
            }
            return Ok(None);
        }
        self.update_knowledge_operation_locked(
            library_id,
            "running",
            counts.0 + counts.1 + counts.2,
            counts.1,
            None,
        )
        .await?;
        Ok(Some(EmbeddingBatch {
            embedding,
            space,
            inputs,
        }))
    }

    /// Runs one frozen batch outside the mutation lock and commits only into its still-current desired space.
    /// 在变更锁外运行一批冻结请求，仅向仍为当前期望的空间提交。
    async fn embedding_step(&self, library_id: &str) -> Result<bool, String> {
        // Reservation returns no batch when paused, complete, exhausted or in a nonvector mode.
        // 暂停、完成、耗尽或非向量模式时，保留入口不返回批次。
        let Some(batch) = self.prepare_embedding_batch(library_id).await? else {
            return Ok(false);
        };
        // Clone the registered host executor without retaining its registration lock during inference.
        // 克隆已注册宿主执行器，推理期间不持有注册锁。
        let executor = self.shared.embedding_executor.read().await.clone();
        // The provider is called only with the declared route, dimension and ordered inputs.
        // 仅使用声明的路由、维度和有序输入调用供应商。
        let result = match executor {
            Some(executor) => {
                executor(KnowledgeEmbeddingRequest {
                    library_id: library_id.to_string(),
                    embedding: batch.embedding.clone(),
                    inputs: batch.inputs.iter().map(|(_, text)| text.clone()).collect(),
                    is_query: false,
                })
                .await
            }
            None => Err("ERR_KNOWLEDGE_EMBEDDING_EXECUTOR_UNAVAILABLE".into()),
        };
        // A route or mode change that completed during inference invalidates this publication.
        // 推理期间完成的路由或模式变更使该次发布失效。
        let _guard = self.library_mutation_guard(library_id).await?;
        // Deleted libraries cannot resurrect from delayed inference results.
        // 已删除库不能因延迟推理结果复活。
        let settings = self
            .require_active_library(library_id)
            .await?
            .settings
            .ok_or("ERR_KNOWLEDGE_SETTINGS_MISSING")?;
        if settings
            .enabled_embedding()
            .map(content::vector_space_id)
            .transpose()?
            .as_deref()
            != Some(&batch.space)
        {
            return Ok(true);
        }
        // Cancellation is checked after asynchronous inference and before every Lance/SQLite publication.
        // 异步推理后、每次 Lance 和 SQLite 发布前检查取消。
        let (_, binding) = self.ensure_library_binding(library_id).await?;
        // Application cancellation is authoritative even if its per-library pause commit was interrupted.
        // 即使每库暂停提交被中断，应用取消仍为权威。
        let operation = self
            .knowledge_worker_operation_locked(library_id, &binding)
            .await?;
        if operation
            .as_ref()
            .is_some_and(|operation| operation.state == "cancelled")
        {
            return Ok(false);
        }
        if local_sql::with_read_snapshot_async(
            &self.host_runtime.local,
            self.resolve_library_binding(&binding)?,
            paused,
        )
        .await?
        {
            return Ok(false);
        }
        // Item validation supports partial success without mislabeling an invalid vector as ready.
        // 条目验证支持部分成功，不将非法向量标成就绪。
        let results:Vec<Result<Vec<f32>,String>>=match result{
            Ok(vectors) if vectors.len()==batch.inputs.len()=>vectors.into_iter().map(|vector|{
                if vector.len()!=batch.embedding.dimensions as usize || vector.iter().any(|value|!value.is_finite()){
                    Err("ERR_KNOWLEDGE_EMBEDDING_VECTOR_INVALID: dimension or finite-value validation failed".into())
                }else{Ok(vector)}
            }).collect(),
            Ok(_)=>vec![Err("ERR_KNOWLEDGE_EMBEDDING_RESPONSE_COUNT".into());batch.inputs.len()],
            Err(error)=>vec![Err(error);batch.inputs.len()]
        };
        // Only valid vectors reach Lance, keyed by their immutable input digest.
        // 仅合法向量进入 Lance，以不可变输入摘要为键。
        let rows: Vec<_> = batch
            .inputs
            .iter()
            .zip(&results)
            .filter_map(|((hash, _), result)| {
                result
                    .as_ref()
                    .ok()
                    .map(|vector| serde_json::json!({"id":hash,"vector":vector}))
            })
            .collect();
        // Storage failure applies to all otherwise successful rows; retry stays content-addressed and idempotent.
        // 存储失败应用到全部原本成功行；重试保持内容寻址和幂等。
        let write_error = if rows.is_empty() {
            None
        } else {
            self.knowledge_vector_engine(library_id, false)
                .await?
                .vector_upsert(LanceDbUpsertInput {
                    table_name: table_name(&batch.space)?,
                    input_format: LanceDbInputFormat::JsonRows,
                    data: serde_json::to_vec(&rows).map_err(|e| e.to_string())?,
                    key_columns: vec!["id".into()],
                })
                .await
                .err()
                .map(|e| e.to_string())
        };
        local_sql::with_connection_async(&self.host_runtime.local,self.resolve_library_binding(binding)?,move |connection|{
            let transaction=connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(|e|e.to_string())?;
            for ((hash,_),result) in batch.inputs.into_iter().zip(results){
                let error=match result{Err(error)=>Some(error),Ok(_)=>write_error.clone()};
                if let Some(error)=error{
                    transaction.execute("UPDATE kb_vector_state SET state=CASE WHEN attempts>=? THEN 'failed' ELSE 'pending' END,last_error=?,updated_at_ms=? WHERE vector_space_id=? AND content_sha256=?;",
                        params![MAX_KNOWLEDGE_EMBEDDING_ATTEMPTS,error,unix_epoch_millis()?,batch.space,hash]).map_err(|e|e.to_string())?;
                }else{
                    transaction.execute("UPDATE kb_vector_state SET state='ready',last_error='',updated_at_ms=? WHERE vector_space_id=? AND content_sha256=?;",
                        params![unix_epoch_millis()?,batch.space,hash]).map_err(|e|e.to_string())?;
                }
            }
            transaction.commit().map_err(|e|e.to_string())
        }).await?;
        Ok(true)
    }
}
