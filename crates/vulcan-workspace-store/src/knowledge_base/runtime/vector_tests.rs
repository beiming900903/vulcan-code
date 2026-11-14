//! Fake inference over real SQLite and LanceDB lifecycle behavior.
//! 在真实 SQLite 与 LanceDB 生命周期行为上使用假推理。

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// Finite local worker deadline; no paid inference is used.
/// 有限本地工作器截止时间；不使用付费推理。
const WORKER_TIMEOUT: Duration = Duration::from_secs(30);

/// Verifies multi-query fusion never merges reused chunk IDs across document revisions.
/// 验证多查询融合绝不跨文档修订合并复用切块身份。
/// Uses gated fake inference over real stores; takes no inputs and returns no value.
/// 在真实存储上使用闸门假推理；无输入及返回值。
#[tokio::test]
async fn multi_query_fusion_revalidates_changed_document_generations() {
    // Start with a fully indexed document whose summary and body will both change.
    // 从完整索引文档开始，其摘要及正文均将变化。
    let (_root, runtime, library) = fixture(KnowledgeIndexMode::Browse).await;
    // Full embedding makes both chunk kinds observable to both query generations.
    // 完整嵌入使两类切块均对两个查询代次可见。
    let mut input = create("document.md", "old body");
    input.embed = Some(KnowledgeEmbedPolicy::Full);
    // Preserve the exact original revision for the concurrent update.
    // 为并发更新保留精确原始修订。
    let original = runtime
        .write_content(&library, "project", input, ai())
        .await
        .unwrap()
        .documents
        .into_iter()
        .next()
        .unwrap();
    runtime
        .set_embedding_executor(fake_executor(Arc::new(AtomicUsize::new(0))))
        .await
        .unwrap();
    runtime
        .set_index_mode(
            &library,
            vector_settings(KnowledgeIndexMode::Vector, "fusion"),
        )
        .await
        .unwrap();
    settled(&runtime, &library).await;
    // Signal only when the first query has already been merged and the second query begins.
    // 仅在首个查询已融合且第二查询开始时发信号。
    let arrived = Arc::new(tokio::sync::Notify::new());
    // Keep the second query in inference while content commits and vectors refresh.
    // 内容提交及向量刷新期间将第二查询保持在推理中。
    let release = Arc::new(tokio::sync::Notify::new());
    // The closure captures independent gate references for each finite call.
    // 闭包为每次有限调用捕获独立闸门引用。
    let arrived_call = arrived.clone();
    // Query and document workers share the executor but only the named query is blocked.
    // 查询及文档工作器共享执行器，但仅指定查询被阻塞。
    let release_call = release.clone();
    runtime
        .set_embedding_executor(Arc::new(move |request| {
            // Every future owns its gate clones.
            // 每个 future 拥有其闸门克隆。
            let arrived = arrived_call.clone();
            // Document batches must remain free to finish during the query pause.
            // 查询暂停期间文档批次必须保持可完成。
            let release = release_call.clone();
            Box::pin(async move {
                if request.is_query && request.inputs.iter().any(|input| input == "second") {
                    arrived.notify_one();
                    release.notified().await;
                }
                Ok(fake_vectors(&request))
            })
        }))
        .await
        .unwrap();
    // Spawn the full public multi-query path, not an internal fusion reproduction.
    // 启动完整公开多查询链路，而非内部融合复现。
    let search_runtime = runtime.clone();
    // The owned selector fixes the physical library throughout the request.
    // 自有选择器在请求期间固定物理库。
    let search_library = library.clone();
    // The task returns after final current-generation validation.
    // 任务在最终当前代次验证后返回。
    let search = tokio::spawn(async move {
        search_runtime
            .search(
                &[search_library],
                &["first".into(), "second".into(), "third".into()],
                10,
            )
            .await
    });
    tokio::time::timeout(WORKER_TIMEOUT, arrived.notified())
        .await
        .unwrap();
    // One update replaces both ranked texts while retaining document/chunk identities.
    // 一次更新替换两类已排名文本，同时保留文档及切块身份。
    let changed = runtime
        .write_content(
            &library,
            "project",
            KnowledgeWriteRequest {
                action: "update".into(),
                document_id: Some(original.id),
                expected_revision: Some(original.revision),
                body: Some("new body".into()),
                summary: Some("new summary".into()),
                ..Default::default()
            },
            ai(),
        )
        .await
        .unwrap()
        .documents
        .into_iter()
        .next()
        .unwrap();
    // Wait for ready hashes, not worker exit: publication intentionally waits for this active query lease.
    // 等待哈希就绪，而非工作器退出：发布按设计等待此活动查询租约。
    tokio::time::timeout(WORKER_TIMEOUT, async {
        loop {
            if runtime
                .embedding_status(&library, 1, 0)
                .await
                .unwrap()
                .pending
                == 0
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    release.notify_one();
    // The old generation cannot consume the new generation's valid rank or retain stale text.
    // 旧代次不能消费新代次合法排名，也不能保留过时文本。
    let result = tokio::time::timeout(WORKER_TIMEOUT, search)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(result.failures.is_empty(), "{:?}", result.failures);
    assert!(!result.hits.is_empty());
    assert!(
        result
            .hits
            .iter()
            .all(|hit| hit.revision == changed.revision && !hit.content.contains("old body"))
    );
    assert!(
        result
            .hits
            .iter()
            .all(|hit| hit.score <= 1.0 / (KNOWLEDGE_RRF_K + 1.0))
    );
}

/// Unchanged ingest reports an in-flight desired-space backlog without appending history.
/// 无变化搬运报告在途期望空间积压，同时不追加历史。
#[tokio::test]
async fn unchanged_ingest_preserves_real_pending_embedding_count() {
    // This fixture starts without vectors so its initial content state is deterministic.
    // 此夹具以无向量状态开始，使初始内容状态确定。
    let (_root, runtime, library) = fixture(KnowledgeIndexMode::Browse).await;
    // The gate prevents the pending state from racing to ready before the receipt is read.
    // 闸门防止读取回执前待处理状态竞态变为就绪。
    let release = Arc::new(tokio::sync::Notify::new());
    // One permit wakes the sole document batch; no paid endpoint is installed.
    // 单个许可唤醒唯一文档批次；不安装付费端点。
    let worker_release = release.clone();
    runtime
        .set_embedding_executor(Arc::new(move |request| {
            // Each asynchronous request owns its own gate reference.
            // 每个异步请求拥有自身闸门引用。
            let release = worker_release.clone();
            Box::pin(async move {
                release.notified().await;
                Ok(fake_vectors(&request))
            })
        }))
        .await
        .unwrap();
    runtime
        .set_index_mode(
            &library,
            vector_settings(KnowledgeIndexMode::Vector, "unchanged"),
        )
        .await
        .unwrap();
    settled(&runtime, &library).await;
    // The summary is embedded even while the transferred body remains outside the AI result.
    // 即使搬运正文留在 AI 结果之外，摘要仍被嵌入。
    let mut input = source("original exact text");
    input.embed = KnowledgeEmbedPolicy::Summary;
    runtime
        .ingest_documents(&library, "project", vec![input.clone()], ai(), "first")
        .await
        .unwrap();
    // A second observation of the same bytes is content-idempotent and still reports the queue.
    // 第二次观测相同字节在内容上幂等，仍报告队列。
    let receipt = runtime
        .ingest_documents(&library, "project", vec![input], ai(), "same")
        .await
        .unwrap();
    assert!(receipt.change_seq.is_none());
    assert!(receipt.pending_embeddings > 0);
    assert_eq!(
        runtime
            .list_changes(&library, None, 20)
            .await
            .unwrap()
            .len(),
        1
    );
    release.notify_one();
    assert_eq!(settled(&runtime, &library).await.pending, 0);
}

/// Creates a synthetic route for a declared mode; returns no network endpoint.
/// 为声明模式创建合成路由；不返回网络端点。
pub(super) fn vector_settings(mode: KnowledgeIndexMode, name: &str) -> KnowledgeLibrarySettings {
    KnowledgeLibrarySettings {
        index_mode: mode,
        embedding: mode.has_vectors().then(|| KnowledgeEmbeddingSettings {
            metric: KnowledgeVectorMetric::Euclidean,
            route: KnowledgeRouteReference {
                provider_id: "fixture".into(),
                protocol: "fixture".into(),
                binding_kind: KnowledgeRouteBindingKind::Protocol,
                service_entry_id: "fixture".into(),
                model_id: name.into(),
                usage_scope: "fixture".into(),
                connection_id: None,
            },
            model_fingerprint: name.into(),
            dimensions: 2,
            batch_size: KNOWLEDGE_DEFAULT_EMBEDDING_BATCH_SIZE,
        }),
    }
}

/// Returns deterministic finite fake vectors in exact input order.
/// 按精确输入顺序返回确定的有限假向量。
fn fake_vectors(request: &KnowledgeEmbeddingRequest) -> Vec<Vec<f32>> {
    request
        .inputs
        .iter()
        .map(|input| vec![input.bytes().map(u32::from).sum::<u32>() as f32, 1.0])
        .collect()
}

/// Returns a fake executor counting only document requests.
/// 返回仅计数文档请求的假执行器。
pub(super) fn fake_executor(calls: Arc<AtomicUsize>) -> KnowledgeEmbeddingExecutor {
    Arc::new(move |request| {
        if !request.is_query {
            calls.fetch_add(1, Ordering::SeqCst);
        }
        Box::pin(async move { Ok(fake_vectors(&request)) })
    })
}

/// Waits for this exact library worker to exit and returns authoritative embedding status.
/// 等待精确知识库工作器退出并返回权威嵌入状态。
pub(super) async fn settled(
    runtime: &KnowledgeBaseRuntime,
    library: &str,
) -> KnowledgeEmbeddingStatus {
    tokio::time::timeout(WORKER_TIMEOUT, async {
        loop {
            if !runtime
                .shared
                .vector_workers
                .lock()
                .await
                .contains_key(library)
            {
                return runtime
                    .embedding_status(library, MAX_KNOWLEDGE_PAGE_SIZE, 0)
                    .await
                    .expect("status");
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("worker must settle before the local deadline")
}

/// Verifies all four search modes and incremental catch-up after a disabled interval.
/// 验证四种检索模式，以及停用期间发生变化后的增量补索引。
#[tokio::test]
async fn four_modes_and_downgrade_obey_the_declared_capabilities() {
    // Every file and inference request belongs to an isolated local fixture.
    // 全部文件和推理请求属于隔离本地夹具。
    let (root, runtime, library) = fixture(KnowledgeIndexMode::Browse).await;
    // The counter excludes query embeddings so background behavior can be assessed independently.
    // 计数器排除查询嵌入，使后台行为可以独立评估。
    let calls = Arc::new(AtomicUsize::new(0));
    runtime
        .set_embedding_executor(fake_executor(calls.clone()))
        .await
        .expect("fake");
    // A browse-mode write must not issue inference.
    // 浏览模式写入不得发起推理。
    let document = runtime
        .write_content(
            &library,
            "project",
            KnowledgeWriteRequest {
                embed: Some(KnowledgeEmbedPolicy::Full),
                ..create("search.md", "specialneedle searchable content")
            },
            ai(),
        )
        .await
        .expect("create full-vector document")
        .documents
        .into_iter()
        .find(|document| document.path == "search.md")
        .expect("created document path may have changed");
    settled(&runtime, &library).await;
    assert!(
        runtime
            .search(
                std::slice::from_ref(&library),
                &["specialneedle".into()],
                20
            )
            .await
            .expect("browse search")
            .hits
            .is_empty()
    );
    for mode in [
        KnowledgeIndexMode::Bm25,
        KnowledgeIndexMode::Vector,
        KnowledgeIndexMode::Hybrid,
    ] {
        // Each transition has an independently queryable durable identity.
        // 每次过渡都有可独立查询的持久身份。
        let operation = runtime
            .set_index_mode(&library, vector_settings(mode, "space-a"))
            .await
            .expect("transition");
        // Completion requires every currently referenced hash, not historical orphan rows.
        // 完成要求全部当前引用哈希，而非历史孤立行。
        let state = settled(&runtime, &library).await;
        assert_eq!((state.pending, state.failed), (0, 0));
        assert_eq!(
            runtime
                .get_operation(&library, Some(&operation.id))
                .await
                .expect("job")
                .expect("stored job")
                .state,
            "completed"
        );
        // Retrieval must report the actual mode and preserve visible capability errors.
        // 检索必须报告实际模式并保留可见能力错误。
        let found = runtime
            .search(
                std::slice::from_ref(&library),
                &["specialneedle".into()],
                20,
            )
            .await
            .expect("search");
        assert!(found.failures.is_empty(), "{:?}", found.failures);
        assert!(found.hits.iter().any(|hit| hit.document_id == document.id));
        assert!(
            found
                .libraries
                .iter()
                .any(|status| status.index_mode == mode)
        );
        if mode == KnowledgeIndexMode::Bm25 {
            assert_eq!(calls.load(Ordering::SeqCst), 0);
        }
        if mode == KnowledgeIndexMode::Vector {
            assert!(found.hits.iter().all(|hit| hit.match_kinds == ["vector"]));
        }
        if mode == KnowledgeIndexMode::Hybrid {
            assert!(
                found
                    .hits
                    .iter()
                    .any(|hit| hit.match_kinds.contains(&"bm25".into())
                        && hit.match_kinds.contains(&"vector".into()))
            );
        }
    }
    // Capture the published space and its deduplicated ready count before disabling it.
    // 停用前记录已发布向量空间及其去重后的就绪数量。
    let before_disable = settled(&runtime, &library).await;
    runtime
        .set_index_mode(
            &library,
            vector_settings(KnowledgeIndexMode::Browse, "unused"),
        )
        .await
        .expect("downgrade");
    // Disabled modes expose no active vector work while the persisted selection remains available.
    // 停用模式不公开活动向量工作，并保留已保存配置。
    let state = settled(&runtime, &library).await;
    assert!(state.active_space_id.is_none());
    assert_eq!((state.pending, state.ready, state.failed), (0, 0, 0));
    assert!(
        runtime
            .index_integrity(&library)
            .await
            .expect("integrity")
            .fts_present
    );
    // Edits during the disabled interval may update local FTS but must issue no embedding request.
    // 停用期间的编辑可以维护本地全文索引，但不得发起嵌入请求。
    let previously_built = calls.load(Ordering::SeqCst);
    let new_doc = runtime
        .write_content(
            &library,
            "project",
            KnowledgeWriteRequest {
                embed: Some(KnowledgeEmbedPolicy::Summary),
                summary: Some("New document summary".into()),
                ..create("new.md", "new content")
            },
            ai(),
        )
        .await
        .expect("write with vector mode disabled");
    assert_eq!(new_doc.pending_embeddings, 0);
    // The dormant FTS table must advance with the write before the mode is restored.
    // 模式恢复前，已停用的全文索引表必须随写入推进。
    let dormant_fts = runtime.index_integrity(&library).await.unwrap();
    assert_eq!(dormant_fts.indexed_change_seq, new_doc.change_seq.unwrap());
    assert_eq!(dormant_fts.indexed_chunk_count, dormant_fts.chunk_count);
    settled(&runtime, &library).await;
    assert_eq!(calls.load(Ordering::SeqCst), previously_built);
    assert!(runtime.retry_embeddings(&library, &[]).await.is_err());
    assert!(
        runtime
            .read_document(&library, Some(&document.id), None, None, None)
            .await
            .is_ok()
    );
    // Reopen the real stores to prove both selection and indexes survive outside process memory.
    // 重新打开真实存储，证明配置与索引均能跨进程保留。
    let runtime = reopen(&root, runtime, &library, calls.clone()).await;
    settled(&runtime, &library).await;
    let dormant = runtime
        .get_library(&library)
        .await
        .unwrap()
        .settings
        .unwrap();
    assert_eq!(dormant.index_mode, KnowledgeIndexMode::Browse);
    assert_eq!(
        dormant.embedding,
        vector_settings(KnowledgeIndexMode::Hybrid, "space-a").embedding
    );
    // Capture exact provider inputs so a full rebuild cannot pass by keeping the same batch count.
    // 捕获精确供应商输入，避免全量重建因批次数相同而通过。
    let inputs = Arc::new(std::sync::Mutex::new(Vec::new()));
    runtime
        .set_embedding_executor(Arc::new({
            let inputs = Arc::clone(&inputs);
            move |request: KnowledgeEmbeddingRequest| {
                if !request.is_query {
                    inputs
                        .lock()
                        .unwrap()
                        .extend(request.inputs.iter().cloned());
                }
                Box::pin(async move { Ok(fake_vectors(&request)) })
            }
        }))
        .await
        .unwrap();
    settled(&runtime, &library).await;
    runtime
        .set_index_mode(
            &library,
            KnowledgeLibrarySettings {
                index_mode: KnowledgeIndexMode::Hybrid,
                embedding: dormant.embedding,
            },
        )
        .await
        .unwrap();
    let restored = settled(&runtime, &library).await;
    assert_eq!((restored.pending, restored.failed), (0, 0));
    assert_eq!(restored.active_space_id, restored.desired_space_id);
    assert_eq!(restored.active_space_id, before_disable.active_space_id);
    assert_eq!(restored.ready, before_disable.ready + 1);
    assert!(
        matches!(inputs.lock().unwrap().as_slice(), [only] if only.contains("New document summary"))
    );
    // The retained full-text publication must already cover the new document.
    // 保留的全文索引发布必须已覆盖新增文档。
    let fts = runtime.index_integrity(&library).await.unwrap();
    assert!(fts.healthy);
    assert_eq!(fts.indexed_change_seq, new_doc.change_seq.unwrap());
    assert!(
        runtime
            .search(&[library], &["new content".into()], 20)
            .await
            .unwrap()
            .hits
            .iter()
            .any(|hit| hit.document_id == new_doc.documents[0].id
                && hit.match_kinds.iter().any(|kind| kind == "bm25"))
    );
}

/// Verifies automatic attempt exhaustion and explicit selected-document retry.
/// 验证自动尝试耗尽与显式所选文档重试。
#[tokio::test]
async fn exhausted_failures_stop_and_human_retry_recovers() {
    let (_root, runtime, library) = fixture(KnowledgeIndexMode::Browse).await;
    // Every automatic failing batch increments one observable fake request.
    // 每个自动失败批次增加一次可观测假请求。
    let failures = Arc::new(AtomicUsize::new(0));
    runtime
        .set_embedding_executor({
            let failures = failures.clone();
            Arc::new(move |_| {
                failures.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Err("fixture route unavailable".into()) })
            })
        })
        .await
        .expect("failing fake");
    let document = add(&runtime, &library, "failed.md", "content needing vectors").await;
    runtime
        .set_index_mode(
            &library,
            vector_settings(KnowledgeIndexMode::Vector, "failure-space"),
        )
        .await
        .expect("mode");
    let failed = settled(&runtime, &library).await;
    assert_eq!(
        failures.load(Ordering::SeqCst),
        MAX_KNOWLEDGE_EMBEDDING_ATTEMPTS as usize
    );
    assert!(failed.failed > 0);
    assert!(
        failed
            .failures
            .iter()
            .all(|item| item.attempts == MAX_KNOWLEDGE_EMBEDDING_ATTEMPTS
                && item.document_id == document.id)
    );
    runtime.wake_knowledge_worker(&library).await;
    settled(&runtime, &library).await;
    assert_eq!(
        failures.load(Ordering::SeqCst),
        MAX_KNOWLEDGE_EMBEDDING_ATTEMPTS as usize
    );
    // Replacing the executor alone never resets exhausted item state.
    // 仅替换执行器绝不重置已耗尽条目状态。
    let successes = Arc::new(AtomicUsize::new(0));
    runtime
        .set_embedding_executor(fake_executor(successes.clone()))
        .await
        .expect("healthy fake");
    runtime
        .retry_embeddings(&library, std::slice::from_ref(&document.id))
        .await
        .expect("explicit retry");
    let ready = settled(&runtime, &library).await;
    assert_eq!((ready.pending, ready.failed), (0, 0));
    assert!(ready.ready > 0);
    assert!(successes.load(Ordering::SeqCst) > 0);
    // The default policy indexes only summary chunks even when body text exists.
    // 默认策略即使存在正文也只索引摘要切块。
    let result = runtime
        .search(std::slice::from_ref(&library), &["content".into()], 20)
        .await
        .expect("summary-only search");
    assert!(result.hits.iter().all(|hit| hit.kind == "summary"));
}

/// Verifies restore reuses historical ready hashes without another embedding request.
/// 验证还原复用历史就绪哈希，不再发起嵌入请求。
#[tokio::test]
async fn restore_reuses_ready_hash_vectors() {
    let (_root, runtime, library) = fixture(KnowledgeIndexMode::Browse).await;
    let calls = Arc::new(AtomicUsize::new(0));
    runtime
        .set_embedding_executor(fake_executor(calls.clone()))
        .await
        .expect("fake");
    runtime
        .set_index_mode(
            &library,
            vector_settings(KnowledgeIndexMode::Vector, "restore-space"),
        )
        .await
        .expect("mode");
    settled(&runtime, &library).await;
    let document = runtime
        .write_content(
            &library,
            "project",
            KnowledgeWriteRequest {
                embed: Some(KnowledgeEmbedPolicy::Full),
                ..create("history.md", "original body")
            },
            ai(),
        )
        .await
        .expect("create full-vector document")
        .documents
        .into_iter()
        .find(|document| document.path == "history.md")
        .expect("created document path may have changed");
    settled(&runtime, &library).await;
    // Select the creation point by operation, independently of row position.
    // 按操作选择创建点，不依赖行位置。
    let point = runtime
        .list_changes(&library, None, 20)
        .await
        .expect("history")
        .into_iter()
        .find(|change| change.operation == "create")
        .expect("creation")
        .seq;
    runtime
        .write_content(
            &library,
            "project",
            KnowledgeWriteRequest {
                action: "update".into(),
                document_id: Some(document.id),
                expected_revision: Some(document.revision),
                body: Some("different body".into()),
                ..Default::default()
            },
            ai(),
        )
        .await
        .expect("update");
    settled(&runtime, &library).await;
    let before = calls.load(Ordering::SeqCst);
    runtime
        .restore_content(&library, point, human(), "recover original")
        .await
        .expect("restore");
    assert_eq!(settled(&runtime, &library).await.pending, 0);
    assert_eq!(calls.load(Ordering::SeqCst), before);
}

/// Verifies the old route remains searchable while replacement generation is deliberately blocked.
/// 验证更换生成被刻意阻塞时，旧路由仍可检索。
#[tokio::test]
async fn replacement_keeps_old_space_until_every_current_hash_is_ready() {
    let (_root, runtime, library) = fixture(KnowledgeIndexMode::Browse).await;
    runtime
        .set_embedding_executor(fake_executor(Arc::new(AtomicUsize::new(0))))
        .await
        .expect("fake");
    let document = add(&runtime, &library, "active.md", "active content").await;
    runtime
        .set_index_mode(&library, vector_settings(KnowledgeIndexMode::Vector, "old"))
        .await
        .expect("old mode");
    let old = settled(&runtime, &library)
        .await
        .active_space_id
        .expect("old space");
    // These notifications are deterministic inference boundaries rather than timing assumptions.
    // 这些通知是确定推理边界，而非时间假设。
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    runtime
        .set_embedding_executor({
            let started = started.clone();
            let release = release.clone();
            Arc::new(move |request| {
                let started = started.clone();
                let release = release.clone();
                Box::pin(async move {
                    if !request.is_query && request.embedding.model_fingerprint == "new" {
                        started.notify_one();
                        release.notified().await;
                    }
                    Ok(fake_vectors(&request))
                })
            })
        })
        .await
        .expect("gated fake");
    settled(&runtime, &library).await;
    runtime
        .set_index_mode(&library, vector_settings(KnowledgeIndexMode::Vector, "new"))
        .await
        .expect("replace");
    tokio::time::timeout(WORKER_TIMEOUT, started.notified())
        .await
        .expect("started");
    let status = runtime
        .embedding_status(&library, 20, 0)
        .await
        .expect("status");
    assert_eq!(status.active_space_id.as_deref(), Some(old.as_str()));
    assert_ne!(status.active_space_id, status.desired_space_id);
    let result = runtime
        .search(std::slice::from_ref(&library), &["active".into()], 20)
        .await
        .expect("old-space search");
    assert!(result.hits.iter().any(|hit| hit.document_id == document.id));
    assert!(
        result
            .libraries
            .iter()
            .any(|state| state.embeddings.pending > 0)
    );
    release.notify_one();
    let current = settled(&runtime, &library).await;
    assert_eq!(current.active_space_id, current.desired_space_id);
    assert_ne!(current.active_space_id.as_deref(), Some(old.as_str()));
}

/// Verifies cancellation after request admission blocks delayed vector publication.
/// 验证请求准入后取消阻止延迟向量发布。
#[tokio::test]
async fn cancellation_prevents_delayed_vector_publication() {
    // All storage and inference belong to one isolated library.
    // 全部存储及推理属于一个隔离知识库。
    let (_root, runtime, library) = fixture(KnowledgeIndexMode::Browse).await;
    // The fake exposes the exact document-inference admission boundary.
    // 假执行器公开精确文档推理准入边界。
    let started = Arc::new(tokio::sync::Notify::new());
    // Publication waits until cancellation and subsequent content actions have finished.
    // 发布等待取消及后续内容动作完成。
    let release = Arc::new(tokio::sync::Notify::new());
    runtime
        .set_embedding_executor({
            // Each request owns its gate references without retaining a mutation lock.
            // 每个请求持有自己的闸门引用，不保留变更锁。
            let started = started.clone();
            // The release gate is reused only after an explicit human resume.
            // 释放闸门仅在明确人工恢复后复用。
            let release = release.clone();
            Arc::new(move |request| {
                // Asynchronous ownership keeps these notifications valid until completion.
                // 异步所有权使这些通知在完成前保持有效。
                let started = started.clone();
                // This request can complete only after the test releases its own permit.
                // 此请求仅在测试释放其自身许可后完成。
                let release = release.clone();
                Box::pin(async move {
                    if !request.is_query {
                        started.notify_one();
                        release.notified().await;
                    }
                    Ok(fake_vectors(&request))
                })
            })
        })
        .await
        .expect("gated fake");
    // The stored change sequence is the exact later restore target, independent of fixture order.
    // 已存变更序号为稍后的精确还原目标，独立于夹具顺序。
    let initial = add(&runtime, &library, "cancel.md", "content").await;
    // One durable operation owns the cancelled background construction.
    // 一个持久操作拥有被取消的后台构建。
    let operation = runtime
        .set_index_mode(
            &library,
            vector_settings(KnowledgeIndexMode::Vector, "cancel"),
        )
        .await
        .expect("mode");
    tokio::time::timeout(WORKER_TIMEOUT, started.notified())
        .await
        .expect("started");
    assert!(
        runtime
            .cancel_operation(&library, &operation.id)
            .await
            .expect("cancel")
    );
    add(&runtime, &library, "later.md", "later content").await;
    // Observe the pause immediately after an ordinary write.
    // 普通写入后立即观测暂停。
    let after_write = runtime.embedding_status(&library, 1, 0).await.unwrap();
    // Ingestion and its unchanged observation use the same queue admission path.
    // 搬运及其无变化观测使用同一排队准入路径。
    let mut input = source("ingested after cancellation");
    input.embed = KnowledgeEmbedPolicy::Summary;
    runtime
        .ingest_documents(&library, "project", vec![input.clone()], ai(), "new")
        .await
        .unwrap();
    // New file content must not silently resume cancelled inference.
    // 新文件内容不得静默恢复已取消推理。
    let after_ingest = runtime.embedding_status(&library, 1, 0).await.unwrap();
    // No-op observations preserve both audit idempotence and the human pause.
    // 无变化观测同时保留审计幂等性及人工暂停。
    let unchanged = runtime
        .ingest_documents(&library, "project", vec![input], ai(), "unchanged")
        .await
        .unwrap();
    // The unchanged branch also queues status and must remain read-only with respect to pause.
    // 无变化分支也查询排队状态，但对暂停必须保持只读。
    let after_unchanged = runtime.embedding_status(&library, 1, 0).await.unwrap();
    runtime
        .restore_content(
            &library,
            initial.updated_change_seq,
            human(),
            "restore while paused",
        )
        .await
        .unwrap();
    // Restore changes content while preserving the current explicit indexing lifecycle.
    // 还原改变内容，同时保留当前明确索引生命周期。
    let after_restore = runtime.embedding_status(&library, 1, 0).await.unwrap();
    // Simulate interruption after durable cancellation but before its per-library pause commit.
    // 模拟持久取消之后、每库暂停提交之前中断。
    let (_, binding) = runtime.ensure_library_binding(&library).await.unwrap();
    local_sql::execute_script_async(
        &runtime.host_runtime.local,
        runtime.resolve_library_binding(binding).unwrap(),
        "UPDATE knowledge_metadata SET value='0' WHERE key='vector_worker_paused';",
        vec![],
    )
    .await
    .unwrap();
    release.notify_one();
    // Wait for the delayed request to exit before evaluating any failed admission assertion.
    // 判断任何失败准入断言前等待延迟请求退出。
    let state = settled(&runtime, &library).await;
    for (action, observed) in [
        ("write", after_write),
        ("ingest", after_ingest),
        ("unchanged ingest", after_unchanged),
        ("restore", after_restore),
    ] {
        assert!(
            observed.paused,
            "{action} must preserve the explicit human pause"
        );
    }
    assert!(unchanged.change_seq.is_none());
    assert!(state.paused);
    assert_eq!(state.ready, 0);
    assert!(state.pending > 0);
    assert!(state.active_space_id.is_none());
    assert_eq!(
        runtime
            .get_operation(&library, Some(&operation.id))
            .await
            .unwrap()
            .unwrap()
            .state,
        "cancelled"
    );
    // Explicit human retry is the declared authority to resume pending work.
    // 明确人工重试是恢复待处理工作的声明权威。
    let resumed = runtime.retry_embeddings(&library, &[]).await.unwrap();
    tokio::time::timeout(WORKER_TIMEOUT, started.notified())
        .await
        .expect("explicit resume starts inference");
    release.notify_one();
    // The replacement operation completes without changing the old stable cancellation.
    // 新操作完成，不改变旧稳定取消状态。
    let completed = settled(&runtime, &library).await;
    assert!(!completed.paused);
    assert_eq!(completed.pending, 0);
    assert!(completed.ready > 0);
    assert_eq!(
        runtime
            .get_operation(&library, Some(&resumed.id))
            .await
            .unwrap()
            .unwrap()
            .state,
        "completed"
    );
    assert_eq!(
        runtime
            .get_operation(&library, Some(&operation.id))
            .await
            .unwrap()
            .unwrap()
            .state,
        "cancelled"
    );
    // A queued explicit retry is durable before its metadata resume commit; reconstruct that crash snapshot.
    // 明确重试在元数据恢复提交前已持久；重建该崩溃快照。
    local_sql::execute_script_async(&runtime.host_runtime.local, LogicalDatabase::AppState,
        "UPDATE knowledge_operations SET state='queued',phase='queued',finished_at_ms=NULL WHERE id=?;",
        vec![text(&resumed.id)]).await.unwrap();
    // The old pause is valid until startup reconciles the newer explicit queued intent.
    // 启动协调更新的明确排队意图前，旧暂停仍为有效值。
    let (_, binding) = runtime.ensure_library_binding(&library).await.unwrap();
    local_sql::execute_script_async(
        &runtime.host_runtime.local,
        runtime.resolve_library_binding(binding).unwrap(),
        "UPDATE knowledge_metadata SET value='1' WHERE key='vector_worker_paused';",
        vec![],
    )
    .await
    .unwrap();
    // A fresh host must finish this current-format operation without another user click or provider request.
    // 全新宿主必须完成此当前格式操作，无需用户再次点击或供应商请求。
    let recovered = reopen(&_root, runtime, &library, Arc::new(AtomicUsize::new(0))).await;
    // Recovery uses the same stable operation and existing ready content hashes.
    // 恢复使用同一稳定操作及已有就绪内容哈希。
    let recovered_state = settled(&recovered, &library).await;
    assert!(
        !recovered_state.paused,
        "A durable explicit resume must reconcile the earlier pause"
    );
    assert_eq!(
        recovered
            .get_operation(&library, Some(&resumed.id))
            .await
            .unwrap()
            .unwrap()
            .state,
        "completed"
    );
    assert_eq!(
        recovered
            .get_operation(&library, Some(&operation.id))
            .await
            .unwrap()
            .unwrap()
            .state,
        "cancelled"
    );
}

/// Reopens the same durable fixture using a fresh host and empty binding caches.
/// 使用全新宿主与空绑定缓存重新打开同一持久夹具。
async fn reopen(
    root: &tempfile::TempDir,
    runtime: KnowledgeBaseRuntime,
    library: &str,
    calls: Arc<AtomicUsize>,
) -> KnowledgeBaseRuntime {
    // The old worker must have released every inference and publication lease before shutdown.
    // 关闭前旧工作器必须已释放全部推理和发布租约。
    assert!(
        !runtime
            .shared
            .vector_workers
            .lock()
            .await
            .contains_key(library)
    );
    runtime
        .release_library_binding(library)
        .await
        .expect("close old bindings");
    drop(runtime);
    // These paths are the exact fixture contract, not candidate locations.
    // 这些路径是精确夹具契约，而非候选位置。
    let database = root.path().join("sqlite/app.sqlite3");
    // A new host owns new runtime caches while preserving all existing durable bytes.
    // 新宿主拥有新运行时缓存，同时保留全部已有持久字节。
    let runtime = KnowledgeBaseRuntime::new(HostRuntimeHandle::new_for_test(HostRuntimeMetadata {
        data_root: root.path().to_path_buf(),
        sqlite_path: database.clone(),
        sqlite_binding_id: format!("restart-{}", Uuid::new_v4()),
        system_statistics_sqlite_path: database,
        system_statistics_sqlite_binding_id: format!("restart-stat-{}", Uuid::new_v4()),
    }));
    runtime
        .set_embedding_executor(fake_executor(calls))
        .await
        .expect("restore host executor");
    runtime
}

/// Verifies a fresh host resumes durable pending work without recreating or duplicating its vector space.
/// 验证全新宿主恢复持久待处理工作，不重建或重复向量空间。
#[tokio::test]
async fn fresh_host_recovers_pending_vectors_and_running_jobs() {
    let (root, runtime, library) = fixture(KnowledgeIndexMode::Browse).await;
    runtime
        .set_embedding_executor(fake_executor(Arc::new(AtomicUsize::new(0))))
        .await
        .expect("fake");
    add(&runtime, &library, "restart.md", "durable content").await;
    let operation = runtime
        .set_index_mode(
            &library,
            vector_settings(KnowledgeIndexMode::Vector, "restart"),
        )
        .await
        .expect("mode");
    let before = settled(&runtime, &library).await;
    // Simulate a crash after durable reservation and before provider completion.
    // 模拟持久保留后、供应商完成前的崩溃。
    let (_, binding) = runtime
        .ensure_library_binding(&library)
        .await
        .expect("binding");
    local_sql::execute_script_async(
        &runtime.host_runtime.local,
        runtime.resolve_library_binding(binding).expect("resolve"),
        "UPDATE kb_vector_state SET state='pending',attempts=1;",
        vec![],
    )
    .await
    .expect("interrupted items");
    local_sql::execute_script_async(&runtime.host_runtime.local,LogicalDatabase::AppState,
        "UPDATE knowledge_operations SET state='running',phase='running',finished_at_ms=NULL WHERE id=?;",vec![text(&operation.id)]).await.expect("interrupted job");
    // A fresh registration recovers jobs and explicit pending state.
    // 全新注册恢复任务与显式待处理状态。
    let calls = Arc::new(AtomicUsize::new(0));
    let runtime = reopen(&root, runtime, &library, calls.clone()).await;
    let after = settled(&runtime, &library).await;
    assert_eq!(after.active_space_id, before.active_space_id);
    assert_eq!(after.ready, before.ready);
    assert_eq!((after.pending, after.failed), (0, 0));
    assert!(calls.load(Ordering::SeqCst) > 0);
    assert_eq!(
        runtime
            .get_operation(&library, Some(&operation.id))
            .await
            .expect("job")
            .expect("durable operation")
            .state,
        "completed"
    );
    assert!(
        runtime
            .search(std::slice::from_ref(&library), &["durable".into()], 20)
            .await
            .expect("reopened search")
            .failures
            .is_empty()
    );
}

/// Verifies retained historical vectors cannot occupy the only result slot after their document is deleted.
/// 验证对应文档删除后，保留历史向量不能占据唯一结果位置。
#[tokio::test]
async fn historical_nearest_vectors_never_crowd_out_live_documents() {
    let (_root, runtime, library) = fixture(KnowledgeIndexMode::Browse).await;
    runtime
        .set_embedding_executor(fake_executor(Arc::new(AtomicUsize::new(0))))
        .await
        .expect("fake");
    runtime
        .set_index_mode(
            &library,
            vector_settings(KnowledgeIndexMode::Vector, "current-only"),
        )
        .await
        .expect("mode");
    settled(&runtime, &library).await;
    let deleted = runtime
        .write_content(
            &library,
            "project",
            KnowledgeWriteRequest {
                embed: Some(KnowledgeEmbedPolicy::Full),
                ..create("deleted.md", "closest")
            },
            ai(),
        )
        .await
        .expect("first")
        .documents
        .into_iter()
        .find(|d| d.path == "deleted.md")
        .expect("document");
    settled(&runtime, &library).await;
    let live = runtime
        .write_content(
            &library,
            "project",
            KnowledgeWriteRequest {
                embed: Some(KnowledgeEmbedPolicy::Full),
                ..create("live.md", "farther text with a very different vector")
            },
            ai(),
        )
        .await
        .expect("second")
        .documents
        .into_iter()
        .find(|d| d.path == "live.md")
        .expect("document");
    settled(&runtime, &library).await;
    runtime
        .write_content(
            &library,
            "project",
            KnowledgeWriteRequest {
                action: "delete".into(),
                document_id: Some(deleted.id.clone()),
                expected_revision: Some(deleted.revision),
                ..Default::default()
            },
            ai(),
        )
        .await
        .expect("delete");
    settled(&runtime, &library).await;
    let result = runtime
        .search(std::slice::from_ref(&library), &["closest".into()], 1)
        .await
        .expect("current-only query");
    assert_eq!(result.hits.len(), 1);
    assert!(
        result
            .hits
            .iter()
            .all(|hit| hit.document_id == live.id && hit.document_id != deleted.id)
    );
}

/// Verifies space publication waits for an in-flight old-space query and does not deadlock its final mapping.
/// 验证空间发布等待进行中的旧空间查询，且不会使其最终映射死锁。
#[tokio::test]
async fn active_query_lease_survives_publication_and_old_table_cleanup() {
    let (_root, runtime, library) = fixture(KnowledgeIndexMode::Browse).await;
    runtime
        .set_embedding_executor(fake_executor(Arc::new(AtomicUsize::new(0))))
        .await
        .expect("fake");
    add(&runtime, &library, "lease.md", "leased content").await;
    runtime
        .set_index_mode(
            &library,
            vector_settings(KnowledgeIndexMode::Vector, "old-lease"),
        )
        .await
        .expect("mode");
    settled(&runtime, &library).await;
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    runtime
        .set_embedding_executor({
            let started = started.clone();
            let release = release.clone();
            Arc::new(move |request| {
                let started = started.clone();
                let release = release.clone();
                Box::pin(async move {
                    if request.is_query && request.embedding.model_fingerprint == "old-lease" {
                        started.notify_one();
                        release.notified().await;
                    }
                    Ok(fake_vectors(&request))
                })
            })
        })
        .await
        .expect("query gate");
    settled(&runtime, &library).await;
    let query = tokio::spawn({
        let runtime = runtime.clone();
        let library = library.clone();
        async move { runtime.search(&[library], &["leased".into()], 20).await }
    });
    tokio::time::timeout(WORKER_TIMEOUT, started.notified())
        .await
        .expect("query entered old space");
    let operation = runtime
        .set_index_mode(
            &library,
            vector_settings(KnowledgeIndexMode::Vector, "new-lease"),
        )
        .await
        .expect("replace");
    tokio::time::timeout(WORKER_TIMEOUT, async {
        loop {
            let state = runtime
                .embedding_status(&library, 20, 0)
                .await
                .expect("new space progress");
            if state.pending == 0 && state.desired_space_id == state.active_space_id {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("new space reached publication");
    assert_ne!(
        runtime
            .get_operation(&library, Some(&operation.id))
            .await
            .expect("operation")
            .expect("job")
            .state,
        "completed"
    );
    release.notify_one();
    let result = tokio::time::timeout(WORKER_TIMEOUT, query)
        .await
        .expect("old query must finish")
        .expect("query task")
        .expect("old query");
    assert!(result.failures.is_empty(), "{:?}", result.failures);
    assert!(!result.hits.is_empty());
    settled(&runtime, &library).await;
    assert_eq!(
        runtime
            .get_operation(&library, Some(&operation.id))
            .await
            .expect("operation")
            .expect("job")
            .state,
        "completed"
    );
}

/// Verifies empty spaces and explicitly excluded documents never spend query or document inference.
/// 验证空空间与显式排除文档绝不消耗查询或文档推理。
#[tokio::test]
async fn empty_vector_library_and_none_policy_need_no_inference() {
    // The executor counts all requests here, including retrieval queries.
    // 此处执行器计数全部请求，包括检索查询。
    let (_root, runtime, _browse) = fixture(KnowledgeIndexMode::Browse).await;
    // An empty vector library must become ready using only local derived storage.
    // 空向量库必须仅使用本地派生存储成为就绪。
    let calls = Arc::new(AtomicUsize::new(0));
    runtime
        .set_embedding_executor({
            let calls = calls.clone();
            Arc::new(move |request| {
                calls.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move { Ok(fake_vectors(&request)) })
            })
        })
        .await
        .expect("counting fake");
    // The explicit vector route is synthetic and performs no network calls.
    // 显式向量路由为合成路由，不执行网络调用。
    let library = runtime
        .create_library(
            "Empty vector",
            "No paid work",
            &["project".into()],
            &vector_settings(KnowledgeIndexMode::Vector, "empty-space"),
        )
        .await
        .expect("create vector library");
    // The active empty space is still a complete space.
    // 活动空空间仍是完整空间。
    let status = settled(&runtime, &library.id).await;
    assert_eq!(status.active_space_id, status.desired_space_id);
    assert!(status.active_space_id.is_some());
    runtime
        .write_content(
            &library.id,
            "project",
            KnowledgeWriteRequest {
                embed: Some(KnowledgeEmbedPolicy::None),
                ..create("excluded.md", "visible only through read")
            },
            ai(),
        )
        .await
        .expect("excluded document");
    settled(&runtime, &library.id).await;
    // Empty vector membership is checked before query inference is requested.
    // 请求查询推理前检查空向量成员集合。
    let found = runtime
        .search(std::slice::from_ref(&library.id), &["read".into()], 20)
        .await
        .expect("empty-vector search");
    assert!(found.hits.is_empty());
    assert!(found.failures.is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
