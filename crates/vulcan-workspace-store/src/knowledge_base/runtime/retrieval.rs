//! Per-library declared-mode retrieval with explicit capability backlog and current-chunk projection.
//! 按库声明模式检索，显式报告能力积压并投影当前切块。

use super::*;
use rusqlite::params;
use serde::Deserialize;
use vulcan_lancedb_engine::types::{LanceDbOutputFormat, LanceDbSearchInput};
use vulcan_sqlite_engine::tokenizer::{TokenizerMode, tokenize_text};

/// Lance nearest-neighbor output, as declared by the pinned lancedb query schema.
/// 固定 lancedb 查询结构声明的 Lance 近邻输出。
#[derive(Deserialize)]
struct VectorRank {
    /// Immutable content hash primary key.
    /// 不可变内容哈希主键。
    id: String,
    /// The automatically projected L2 distance.
    /// 自动投影的 L2 距离。
    #[serde(rename = "_distance")]
    distance: f64,
}

impl KnowledgeBaseRuntime {
    /// Filters bounded `hits` against `library_id`'s current exact chunk/revision snapshot; returns retained candidates.
    /// 根据 `library_id` 当前精确切块及修订快照过滤有界 `hits`；返回保留候选。
    async fn current_search_hits(
        &self,
        library_id: &str,
        hits: Vec<KnowledgeSearchHit>,
    ) -> Result<Vec<KnowledgeSearchHit>, String> {
        // Current-row verification shares the deletion/mutation domain and never spans provider inference.
        // 当前行验证共享删除及变更域，且绝不跨越供应商推理。
        let _mutation = self.library_mutation_guard(library_id).await?;
        self.require_active_library(library_id).await?;
        // The known binding is retained for the complete final read snapshot.
        // 为完整最终读取快照保留既有绑定。
        let (_, binding) = self.ensure_library_binding(library_id).await?;
        local_sql::with_read_snapshot_async(&self.host_runtime.local,self.resolve_library_binding(binding)?,move |connection| {
            // Reuse one indexed existence query instead of scanning all current library metadata.
            // 复用一个有索引的存在性查询，不扫描全部当前库元数据。
            let mut statement = connection.prepare("SELECT EXISTS(SELECT 1 FROM kb_chunks c JOIN kb_documents d ON d.id=c.document_id WHERE c.id=? AND c.content_sha256=? AND d.id=? AND d.revision=?);").map_err(|error|error.to_string())?;
            // Every returned reference belongs to the same final logical snapshot.
            // 每份返回参考资料属于相同最终逻辑快照。
            let mut retained = Vec::new();
            for hit in hits {
                // SQLite stores the same checked signed revision used by document publication.
                // SQLite 保存与文档发布相同的已检查有符号修订。
                let revision = i64::try_from(hit.revision).map_err(|error|error.to_string())?;
                if statement.query_row(params![hit.chunk_id,hit.content_sha256,hit.document_id,revision],|row|row.get::<_,bool>(0)).map_err(|error|error.to_string())? {
                    retained.push(hit);
                }
            }
            Ok(retained)
        }).await
    }

    /// Searches explicit libraries using their own modes and fuses independent queries; returns visible partial failures.
    /// 使用各自模式检索显式知识库并融合独立查询；返回可见部分失败。
    pub async fn search(
        &self,
        library_ids: &[String],
        queries: &[String],
        limit: u32,
    ) -> Result<KnowledgeSearchResult, String> {
        if queries.is_empty()
            || queries.len() > MAX_KNOWLEDGE_QUERIES
            || limit == 0
            || limit > MAX_KNOWLEDGE_PAGE_SIZE
            || queries.iter().any(|query| {
                query.trim().is_empty() || query.chars().count() > MAX_KNOWLEDGE_QUERY_CHARS
            })
        {
            return Err("ERR_KNOWLEDGE_SEARCH_RANGE".into());
        }
        // Stable identities deduplicate repeated library selectors without duplicating ranks.
        // 稳定身份去重重复知识库选择器，不重复排名。
        let libraries: BTreeSet<_> = library_ids.iter().collect();
        // Ranks can combine only the same physical library, chunk, content and document revision.
        // 排名仅可合并相同物理库、切块、内容及文档修订。
        let mut fused: BTreeMap<(String, String, String, u64), KnowledgeSearchHit> =
            BTreeMap::new();
        // Every library reports its explicit mode even when it contributes no hits.
        // 每个知识库即使没有命中也报告显式模式。
        let mut statuses = BTreeMap::new();
        // Errors remain attached to their exact library and query.
        // 错误保持归属精确知识库与查询。
        let mut failures = Vec::new();
        for library in libraries {
            for query in queries {
                match self.search_one(library, query, limit).await {
                    Ok((hits, status, errors)) => {
                        statuses.insert(library.clone(), status);
                        for error in errors {
                            failures.push(KnowledgeSearchFailure {
                                library_id: library.clone(),
                                query: Some(query.clone()),
                                error,
                            });
                        }
                        for hit in hits {
                            let key = (
                                hit.library_id.clone(),
                                hit.chunk_id.clone(),
                                hit.content_sha256.clone(),
                                hit.revision,
                            );
                            if let Some(existing) = fused.get_mut(&key) {
                                existing.score += hit.score;
                                for kind in hit.match_kinds {
                                    if !existing.match_kinds.contains(&kind) {
                                        existing.match_kinds.push(kind);
                                    }
                                }
                            } else {
                                fused.insert(key, hit);
                            }
                        }
                    }
                    Err(error) => failures.push(KnowledgeSearchFailure {
                        library_id: library.clone(),
                        query: Some(query.clone()),
                        error,
                    }),
                }
            }
        }
        // Scores compare ranks rather than incompatible raw BM25 and vector distances.
        // 分数比较排名，而非不可比较的原始全文分数与向量距离。
        let mut hits: Vec<_> = fused.into_values().collect();
        // Revalidate all merged query generations once against each library's current snapshot.
        // 针对各库当前快照一次复核全部合并查询代次。
        let mut current_hits = Vec::new();
        for library in library_ids.iter().collect::<BTreeSet<_>>() {
            // Take only this library's bounded candidate list into its exact read snapshot.
            // 仅将此库有界候选列表带入其精确读取快照。
            let candidates = hits
                .iter()
                .filter(|hit| &hit.library_id == library)
                .cloned()
                .collect::<Vec<_>>();
            match self.current_search_hits(library, candidates).await {
                Ok(rows) => current_hits.extend(rows),
                Err(error) => failures.push(KnowledgeSearchFailure {
                    library_id: library.clone(),
                    query: None,
                    error,
                }),
            }
        }
        hits = current_hits;
        hits.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.library_id.cmp(&b.library_id))
                .then_with(|| a.path.cmp(&b.path))
                .then_with(|| a.chunk_id.cmp(&b.chunk_id))
        });
        hits.truncate(limit as usize);
        Ok(KnowledgeSearchResult {
            queries: queries.to_vec(),
            hits,
            libraries: statuses.into_values().collect(),
            failures,
        })
    }

    /// Retrieves one query from a frozen mode and active vector lease; returns current chunk metadata only.
    /// 通过冻结模式与活动向量租约检索一个查询；仅返回当前切块元数据。
    async fn search_one(
        &self,
        library_id: &str,
        query: &str,
        limit: u32,
    ) -> Result<
        (
            Vec<KnowledgeSearchHit>,
            KnowledgeSearchLibraryStatus,
            Vec<String>,
        ),
        String,
    > {
        // Lock order is mutation then space lease, matching publication and deletion.
        // 锁顺序为变更后空间租约，与发布和删除一致。
        let mutation = self.library_mutation_guard(library_id).await?;
        // Library purpose and mode have one registry authority.
        // 知识库用途与模式只有一个注册权威。
        let library = self.require_active_library(library_id).await?;
        // Settings are absent only for a refused format, never interpreted by fallback.
        // 设置仅在格式拒绝时缺失，绝不通过兜底解释。
        let settings = library
            .settings
            .as_ref()
            .ok_or("ERR_KNOWLEDGE_SETTINGS_MISSING")?;
        // Existing readers retain their exact active table while a replacement is published.
        // 更换发布期间，已有读取者保留精确活动表。
        let _space_lease = self.vector_space_lock(library_id).await.read_owned().await;
        // The binding remains alive through final current-row mapping.
        // 绑定在最终当前行映射期间保持存活。
        let (_, binding) = self.ensure_library_binding(library_id).await?;
        // Desired-space backlog is reported separately from the still-active search space.
        // 期望空间积压与仍活动的检索空间分别报告。
        let embeddings = self.embedding_status(library_id, 1, 0).await?;
        // Capability failures are explicit even when the other hybrid capability succeeds.
        // 即使另一混合能力成功，能力失败也明确报告。
        let mut errors = Vec::new();
        // Admission never silently substitutes a different index mode.
        // 准入绝不静默替换索引模式。
        let fts_status = if settings.index_mode.has_fts() {
            match self.index_integrity(library_id).await {
                Ok(integrity) if integrity.healthy => "ready",
                Ok(_) => "building",
                Err(error) => {
                    errors.push(error);
                    "error"
                }
            }
        } else {
            "disabled"
        };
        // Read the nullable active snapshot while holding the publication lease.
        // 持发布租约时读取可空活动快照。
        let active = if settings.index_mode.has_vectors() {
            local_sql::with_read_snapshot_async(
                &self.host_runtime.local,
                self.resolve_library_binding(&binding)?,
                vector::active_embedding,
            )
            .await?
        } else {
            None
        };
        // Full-text ranks retain hashes so a concurrent body change cannot project unranked new text.
        // 全文排名保留哈希，避免并发正文变更投影未经排名的新文本。
        let mut fts_ranks = BTreeMap::new();
        if fts_status == "ready" {
            let result=local_sql::with_read_snapshot_async(&self.host_runtime.local,self.resolve_library_binding(&binding)?,{
                let query=query.to_string();
                move |connection|{
                    let tokenized=tokenize_text(Some(connection),TokenizerMode::Jieba,&query,true).map_err(|e|e.to_string())?;
                    let mut statement=connection.prepare(&format!("SELECT f.id,c.content_sha256,bm25({KNOWLEDGE_FTS_INDEX}) AS raw_score FROM {KNOWLEDGE_FTS_INDEX} f JOIN kb_chunks c ON c.id=f.id WHERE {KNOWLEDGE_FTS_INDEX} MATCH ? ORDER BY raw_score,f.id LIMIT ?;")).map_err(|e|e.to_string())?;
                    statement.query_map(params![tokenized.fts_query,limit],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).map_err(|e|e.to_string())?.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())
                }
            }).await;
            match result {
                Ok(rows) => {
                    for (rank, (id, hash)) in rows.into_iter().enumerate() {
                        fts_ranks.insert((id, hash), 1.0 / (KNOWLEDGE_RRF_K + rank as f64 + 1.0));
                    }
                }
                Err(error) => errors.push(error),
            }
        }
        // Paid query inference never holds the content mutation lock.
        // 付费查询推理绝不持有内容变更锁。
        drop(mutation);
        // Vector ranks are content-addressed and map only to currently embedded chunks.
        // 向量排名采用内容寻址，且仅映射到当前嵌入切块。
        let mut vector_ranks = BTreeMap::new();
        if settings.index_mode.has_vectors() {
            match active.as_ref() {
                Some(active) => {
                    let result = self
                        .rank_current_vectors(library_id, &binding, active, query, limit)
                        .await;
                    match result {
                        Ok(rows) => {
                            for (rank, hash) in rows.into_iter().enumerate() {
                                vector_ranks
                                    .insert(hash, 1.0 / (KNOWLEDGE_RRF_K + rank as f64 + 1.0));
                            }
                        }
                        Err(error) => errors.push(error),
                    }
                }
                None => errors.push(
                    "ERR_KNOWLEDGE_VECTOR_NOT_READY: no complete active vector space exists".into(),
                ),
            }
        }
        // Exact key sets bound authoritative projection to ranked rows.
        // 精确键集将权威投影限制为已排名行。
        let fts_ids =
            serde_json::to_string(&fts_ranks.keys().map(|(id, _)| id).collect::<Vec<_>>())
                .map_err(|e| e.to_string())?;
        // Query mappings use the active space selected above, not the desired replacement.
        // 查询映射使用上方选择的活动空间，而非期望更换空间。
        let active_space = active.as_ref().map(content::vector_space_id).transpose()?;
        // A distinct hash can correspond to several current documents; all are eligible for mapping.
        // 一个去重哈希可对应多个当前文档；全部均可映射。
        let vector_hashes = serde_json::to_string(&vector_ranks.keys().collect::<Vec<_>>())
            .map_err(|e| e.to_string())?;
        // Retain the exact human library label for untrusted reference attribution.
        // 保留精确人工库标签，供不可信参考资料归属。
        let library_name = library.name;
        // The library UUID scopes every returned chunk identity.
        // 知识库 UUID 限定每个返回切块身份。
        let library_id = library_id.to_string();
        // Keep a copy for status after the projection closure consumes its identity.
        // 保留副本，供投影闭包消费身份后的状态使用。
        let status_id = library_id.clone();
        // Final metadata is one current read snapshot, independent of inference latency.
        // 最终元数据为一个当前读取快照，与推理延迟独立。
        let mut hits=local_sql::with_read_snapshot_async(&self.host_runtime.local,self.resolve_library_binding(binding)?,move |connection|{
            let mut statement=connection.prepare("SELECT c.id,c.content_sha256,d.id,d.path,d.title,d.summary,c.kind,c.heading_path,c.start_line,c.end_line,c.text,(c.embed=1 AND EXISTS(SELECT 1 FROM kb_vector_state s WHERE s.vector_space_id=? AND s.content_sha256=c.content_sha256 AND s.state='ready')),d.revision FROM kb_chunks c JOIN kb_documents d ON d.id=c.document_id WHERE c.id IN (SELECT value FROM json_each(?)) OR (c.embed=1 AND c.content_sha256 IN (SELECT value FROM json_each(?)) AND EXISTS(SELECT 1 FROM kb_vector_state s WHERE s.vector_space_id=? AND s.content_sha256=c.content_sha256 AND s.state='ready')) ORDER BY d.path_key,c.ordinal;").map_err(|e|e.to_string())?;
            let rows=statement.query_map(params![active_space,fts_ids,vector_hashes,active_space],|r|{
                let id=r.get::<_,String>(0)?;
                let hash=r.get::<_,String>(1)?;
                let mut score=0.0;
                let mut kinds=Vec::new();
                if let Some(rank)=fts_ranks.get(&(id.clone(),hash.clone())){score+=rank;kinds.push("bm25".into());}
                if r.get::<_,bool>(11)?&& let Some(rank)=vector_ranks.get(&hash){score+=rank;kinds.push("vector".into());}
                if kinds.is_empty(){return Ok(None);}
                Ok(Some(KnowledgeSearchHit{library_id:library_id.clone(),library_name:library_name.clone(),document_id:r.get(2)?,revision:unsigned_column(r,12)?,content_sha256:hash,path:r.get(3)?,title:r.get(4)?,summary:r.get(5)?,chunk_id:id,kind:r.get(6)?,heading_path:r.get(7)?,start_line:unsigned_column(r,8)?,end_line:unsigned_column(r,9)?,content:r.get(10)?,score,match_kinds:kinds}))
            }).map_err(|e|e.to_string())?;
            rows.filter_map(|r|match r{Ok(Some(hit))=>Some(Ok(hit)),Ok(None)=>None,Err(error)=>Some(Err(error.to_string()))}).collect::<Result<Vec<_>,_>>()
        }).await?;
        hits.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.path.cmp(&b.path))
                .then_with(|| a.chunk_id.cmp(&b.chunk_id))
        });
        hits.truncate(limit as usize);
        Ok((
            hits,
            KnowledgeSearchLibraryStatus {
                library_id: status_id,
                index_mode: settings.index_mode,
                fts_status: fts_status.into(),
                embeddings,
            },
            errors,
        ))
    }

    /// Ranks only current ready hashes before mapping; retained historical vectors cannot crowd out live results.
    /// 映射前仅排名当前就绪哈希；保留的历史向量不能挤占当前结果。
    async fn rank_current_vectors(
        &self,
        library_id: &str,
        binding: &str,
        embedding: &KnowledgeEmbeddingSettings,
        query: &str,
        limit: u32,
    ) -> Result<Vec<String>, String> {
        // The active route is frozen for the query by the caller's space lease.
        // 查询的活动路由由调用方空间租约冻结。
        let space = content::vector_space_id(embedding)?;
        // Filter membership is read from current chunks and this exact ready vector space.
        // 过滤成员从当前切块与该精确就绪向量空间读取。
        let hashes=local_sql::with_read_snapshot_async(&self.host_runtime.local,self.resolve_library_binding(binding)?,{
            let space=space.clone();move |connection|{
                let mut statement=connection.prepare("SELECT DISTINCT c.content_sha256 FROM kb_chunks c JOIN kb_vector_state s ON s.content_sha256=c.content_sha256 WHERE c.embed=1 AND s.vector_space_id=? AND s.state='ready' ORDER BY c.content_sha256;").map_err(|e|e.to_string())?;
                statement.query_map([space],|r|r.get::<_,String>(0)).map_err(|e|e.to_string())?.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())
            }
        }).await?;
        if hashes.is_empty() {
            return Ok(vec![]);
        }
        // Inference registration is explicit and errors remain visible.
        // 推理注册显式确定，错误保持可见。
        let executor = self
            .shared
            .embedding_executor
            .read()
            .await
            .clone()
            .ok_or("ERR_KNOWLEDGE_EMBEDDING_EXECUTOR_UNAVAILABLE")?;
        // Query task semantics are distinct from document embeddings.
        // 查询任务语义与文档嵌入不同。
        let vectors = executor(KnowledgeEmbeddingRequest {
            library_id: library_id.to_string(),
            embedding: embedding.clone(),
            inputs: vec![query.into()],
            is_query: true,
        })
        .await?;
        if vectors.len() != 1 {
            return Err("ERR_KNOWLEDGE_EMBEDDING_RESPONSE_COUNT".into());
        }
        // The sole vector is selected only after exact cardinality validation.
        // 仅在精确基数验证后选择唯一向量。
        let vector = vectors
            .into_iter()
            .next()
            .ok_or("ERR_KNOWLEDGE_EMBEDDING_RESPONSE_COUNT")?;
        if vector.len() != embedding.dimensions as usize
            || vector.iter().any(|value| !value.is_finite())
        {
            return Err("ERR_KNOWLEDGE_EMBEDDING_VECTOR_INVALID".into());
        }
        // Local filtering is batched by the existing result bound, avoiding unbounded SQL predicates.
        // 本地过滤按既有结果上限分批，避免无界 SQL 谓词。
        let engine = self.knowledge_vector_engine(library_id, false).await?;
        // Distances permit exact global ordering across bounded predicate batches.
        // 距离允许在有界谓词批次间精确全局排序。
        let mut ranked = Vec::<VectorRank>::new();
        for group in hashes.chunks(MAX_KNOWLEDGE_PAGE_SIZE as usize) {
            for hash in group {
                vector::validate_sha256(hash)?;
            }
            let filter = format!(
                "id IN ({})",
                group
                    .iter()
                    .map(|hash| format!("'{hash}'"))
                    .collect::<Vec<_>>()
                    .join(",")
            );
            let result = engine
                .vector_search_with_metric(
                    LanceDbSearchInput {
                        table_name: vector::table_name(&space)?,
                        vector: vector.clone(),
                        limit,
                        filter,
                        vector_column: "vector".into(),
                        output_format: LanceDbOutputFormat::JsonRows,
                    },
                    embedding.metric,
                )
                .await
                .map_err(|e| e.to_string())?;
            let rows: Vec<VectorRank> =
                serde_json::from_slice(&result.data).map_err(|e| e.to_string())?;
            if rows.iter().any(|row| !row.distance.is_finite()) {
                return Err("ERR_KNOWLEDGE_VECTOR_DISTANCE_INVALID".into());
            }
            ranked.extend(rows);
        }
        ranked.sort_by(|a, b| {
            a.distance
                .total_cmp(&b.distance)
                .then_with(|| a.id.cmp(&b.id))
        });
        ranked.truncate(limit as usize);
        Ok(ranked.into_iter().map(|row| row.id).collect())
    }
}
