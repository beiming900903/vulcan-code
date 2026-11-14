//! Derived full-text and dictionary lifecycle, with transactional reconstruction.
//! 派生全文与词典生命周期，采用事务重建。

use super::*;
use rusqlite::Connection;
use vulcan_sqlite_engine::{
    fts::ensure_fts_index,
    tokenizer::{self, TokenizerMode},
};

/// Rebuilds FTS for the current chunks and dictionary in the caller's transaction.
/// 在调用方事务内为当前切块及词典重建全文索引。
pub(super) fn rebuild_fts(connection: &Connection) -> Result<(), String> {
    ensure_fts_index(connection, KNOWLEDGE_FTS_INDEX, TokenizerMode::Jieba)
        .map_err(|e| e.to_string())?;
    connection.execute_batch(&format!("DELETE FROM {KNOWLEDGE_FTS_INDEX};
            INSERT INTO {KNOWLEDGE_FTS_INDEX}(id,file_path,title,content)
            SELECT c.id,d.path,d.title,c.text FROM kb_chunks c JOIN kb_documents d ON d.id=c.document_id;
            UPDATE knowledge_metadata SET value=(SELECT value FROM knowledge_metadata WHERE key='head_change_seq') WHERE key='indexed_change_seq';
            UPDATE knowledge_metadata SET value=(SELECT value FROM knowledge_metadata WHERE key='dictionary_revision') WHERE key='indexed_dictionary_revision';
            UPDATE knowledge_metadata SET value='ready' WHERE key='index_status';")).map_err(|e|e.to_string())?;
    Ok(())
}

/// Reports whether this library has a stored FTS table, even with search disabled.
/// 报告此库是否保存有全文索引表，包括检索停用时。
pub(super) fn is_present(connection: &Connection) -> Result<bool, String> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?);",
            [KNOWLEDGE_FTS_INDEX],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())
}

/// Checks the index publication marker and revisions using the same SQLite connection.
/// 使用同一 SQLite 连接检查索引发布标记与修订。
pub(super) fn is_current(connection: &Connection) -> Result<bool, String> {
    let ready: bool = connection
        .query_row(
            "SELECT value='ready' FROM knowledge_metadata WHERE key='index_status';",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    Ok(ready
        && is_present(connection)?
        && metadata_integer(connection, "head_change_seq")?
            == metadata_integer(connection, "indexed_change_seq")?
        && metadata_integer(connection, "dictionary_revision")?
            == metadata_integer(connection, "indexed_dictionary_revision")?)
}

/// Reads one exact nonnegative metadata integer; returns corruption rather than a guessed default.
/// 读取一个精确非负元数据整数；返回损坏错误而非猜测默认值。
fn metadata_integer(connection: &Connection, key: &str) -> Result<u64, String> {
    connection
        .query_row(
            "SELECT value FROM knowledge_metadata WHERE key=?;",
            [key],
            |row| row.get::<_, String>(0),
        )
        .map_err(|e| e.to_string())?
        .parse()
        .map_err(|e| format!("ERR_KNOWLEDGE_METADATA_INTEGER: {e}"))
}

impl KnowledgeBaseRuntime {
    /// Upserts `word` with positive `weight` in `library_id`; returns the atomically refreshed dictionary.
    /// 在 `library_id` 中以正 `weight` 更新或插入 `word`；返回原子刷新词典。
    pub async fn upsert_custom_word(
        &self,
        library_id: &str,
        word: &str,
        weight: u32,
    ) -> Result<Vec<KnowledgeCustomWord>, String> {
        validate_custom_words(&[KnowledgeCustomWord {
            word: word.into(),
            weight,
        }])?;
        self.mutate_custom_word(library_id, word, Some(weight))
            .await
    }

    /// Removes exact `word` from `library_id`; returns the dictionary without a read/replace race.
    /// 从 `library_id` 移除精确 `word`；返回词典，避免读取后替换竞争。
    pub async fn remove_custom_word(
        &self,
        library_id: &str,
        word: &str,
    ) -> Result<Vec<KnowledgeCustomWord>, String> {
        validate_custom_words(&[KnowledgeCustomWord {
            word: word.into(),
            weight: 1,
        }])?;
        self.mutate_custom_word(library_id, word, None).await
    }

    /// Applies `weight` (Some = upsert, None = remove) to `word` under `library_id`'s lock; returns current terms.
    /// 在 `library_id` 锁内对 `word` 应用 `weight`（Some 为更新插入，None 为移除）；返回当前词语。
    async fn mutate_custom_word(
        &self,
        library_id: &str,
        word: &str,
        weight: Option<u32>,
    ) -> Result<Vec<KnowledgeCustomWord>, String> {
        // Hold the same mutation domain as content and full dictionary replacement.
        // 持有与内容及全量词典替换相同的变更域。
        let guard = self.library_mutation_guard(library_id).await?;
        // Read the sole current mode before rebuilding derived FTS.
        // 重建派生全文索引前读取唯一当前模式。
        let settings = self
            .require_active_library(library_id)
            .await?
            .settings
            .ok_or("ERR_KNOWLEDGE_SETTINGS_MISSING")?;
        // Resolve the exact library connection and move the term into its transaction.
        // 解析精确库连接，并将词语移入其事务。
        let (_, binding_id) = self.ensure_library_binding(library_id).await?;
        // The owned term cannot change while the asynchronous transaction executes.
        // 异步事务执行期间自有词语不会变化。
        let word = word.to_owned();
        local_sql::with_connection_async(&self.host_runtime.local, self.resolve_library_binding(&binding_id)?, move |connection| {
            // Dictionary content, revision and derived index publish together.
            // 词典内容、修订及派生索引共同发布。
            let outcome = (|| {
                let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(|error|error.to_string())?;
                match weight {
                    Some(weight) => { tokenizer::upsert_custom_word(&transaction, &word, weight as usize).map_err(|error|error.to_string())?; }
                    None => { tokenizer::remove_custom_word(&transaction, &word).map_err(|error|error.to_string())?; }
                }
                transaction.execute("UPDATE knowledge_metadata SET value=CAST(value AS INTEGER)+1 WHERE key='dictionary_revision';", []).map_err(|error|error.to_string())?;
                if settings.index_mode.has_fts() || is_present(&transaction)? {
                    rebuild_fts(&transaction)?;
                }
                transaction.commit().map_err(|error|error.to_string())
            })();
            resync_dictionary_after_rollback(connection, outcome)
        }).await?;
        // Read while still owning the mutation lock so the response describes this publication.
        // 仍持有变更锁时读取，使响应描述本次发布。
        let result = self.list_custom_words(library_id).await;
        drop(guard);
        result
    }

    /// Rebuilds only current derived FTS data; returns integrity after an atomic publication.
    /// 仅重建当前派生全文数据；原子发布后返回完整性。
    pub async fn rebuild_index(
        &self,
        library_id: &str,
    ) -> Result<KnowledgeIndexIntegritySummary, String> {
        // Rebuilds share the content lock, preventing lost writes between scan and publication.
        // 重建共享内容锁，防止扫描和发布之间丢失写入。
        let guard = self.library_mutation_guard(library_id).await?;
        // The target mode is library-owned, never inferred from old tables.
        // 目标模式由库拥有，绝不从旧表推断。
        let library = self.require_active_library(library_id).await?;
        // Invalid settings cannot be replaced with defaults during maintenance.
        // 维护期间非法设置不能以默认值替换。
        let settings = library.settings.ok_or("ERR_KNOWLEDGE_SETTINGS_MISSING")?;
        // Resolve the established host binding before entering the transaction.
        // 进入事务前解析既有宿主绑定。
        let (_, binding_id) = self.ensure_library_binding(library_id).await?;
        local_sql::with_connection_async(
            &self.host_runtime.local,
            self.resolve_library_binding(&binding_id)?,
            move |connection| {
                let transaction = connection
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .map_err(|e| e.to_string())?;
                if settings.index_mode.has_fts() {
                    rebuild_fts(&transaction)?;
                }
                transaction.commit().map_err(|e| e.to_string())
            },
        )
        .await?;
        drop(guard);
        self.index_integrity(library_id).await
    }

    /// Computes exact index consistency for current chunks and dictionaries; returns typed diagnostics.
    /// 为当前切块和词典计算精确索引一致性；返回类型化诊断。
    pub async fn index_integrity(
        &self,
        library_id: &str,
    ) -> Result<KnowledgeIndexIntegritySummary, String> {
        // Registry and metadata errors remain explicit before inspecting derived data.
        // 检查派生数据前保持注册和元数据错误明确。
        let settings = self
            .require_active_library(library_id)
            .await?
            .settings
            .ok_or("ERR_KNOWLEDGE_SETTINGS_MISSING")?;
        // The consistent snapshot covers all authoritative and derived comparisons.
        // 一致快照覆盖全部权威与派生比较。
        let (_, binding_id) = self.ensure_library_binding(library_id).await?;
        local_sql::with_read_snapshot_async(&self.host_runtime.local,self.resolve_library_binding(&binding_id)?,move |connection|{
            tokenizer::ensure_jieba_tokenizer_registered(connection).map_err(|e|e.to_string())?;
            let present=is_present(connection)?;
            let document_count=connection.query_row("SELECT COUNT(*) FROM kb_documents;",[],|row|unsigned_column(row,0)).map_err(|e|e.to_string())?;
            let chunk_count=connection.query_row("SELECT COUNT(*) FROM kb_chunks;",[],|row|unsigned_column(row,0)).map_err(|e|e.to_string())?;
            let (indexed_chunk_count,mismatched_rows)=if present {
                let count=connection.query_row(&format!("SELECT COUNT(*) FROM {KNOWLEDGE_FTS_INDEX};"),[],|row|unsigned_column(row,0)).map_err(|e|e.to_string())?;
                let mismatched=connection.query_row(&format!("SELECT
                    (SELECT COUNT(*) FROM kb_chunks c JOIN kb_documents d ON d.id=c.document_id WHERE
                     (SELECT COUNT(*) FROM {KNOWLEDGE_FTS_INDEX} f WHERE f.id=c.id AND f.file_path=d.path AND f.title=d.title AND f.content=c.text)!=1)
                    +(SELECT COUNT(*) FROM {KNOWLEDGE_FTS_INDEX} f WHERE NOT EXISTS(SELECT 1 FROM kb_chunks c WHERE c.id=f.id));"),[],|row|unsigned_column(row,0)).map_err(|e|e.to_string())?;
                (count,mismatched)
            }else{(0,chunk_count)};
            let head_change_seq=metadata_integer(connection,"head_change_seq")?;
            let indexed_change_seq=metadata_integer(connection,"indexed_change_seq")?;
            let dictionary_revision=metadata_integer(connection,"dictionary_revision")?;
            let indexed_dictionary_revision=metadata_integer(connection,"indexed_dictionary_revision")?;
            let healthy=!settings.index_mode.has_fts() || (present && chunk_count==indexed_chunk_count && mismatched_rows==0 && head_change_seq==indexed_change_seq && dictionary_revision==indexed_dictionary_revision);
            Ok(KnowledgeIndexIntegritySummary{index_mode:settings.index_mode,document_count,chunk_count,fts_present:present,indexed_chunk_count,mismatched_rows,head_change_seq,indexed_change_seq,dictionary_revision,indexed_dictionary_revision,healthy})
        }).await
    }

    /// Lists this library's exact custom dictionary; returns validated positive weights.
    /// 列出该库精确自定义词典；返回已验证正权重。
    pub async fn list_custom_words(
        &self,
        library_id: &str,
    ) -> Result<Vec<KnowledgeCustomWord>, String> {
        self.require_active_library(library_id).await?;
        // The dictionary table is created by the fresh schema, never as a legacy upgrade.
        // 词典表由全新结构创建，绝非旧格式升级。
        let (_, binding_id) = self.ensure_library_binding(library_id).await?;
        local_sql::with_read_snapshot_async(
            &self.host_runtime.local,
            self.resolve_library_binding(&binding_id)?,
            |connection| {
                tokenizer::list_custom_words(connection)
                    .map_err(|e| e.to_string())?
                    .words
                    .into_iter()
                    .map(|entry| {
                        Ok(KnowledgeCustomWord {
                            word: entry.word,
                            weight: u32::try_from(entry.weight).map_err(|e| e.to_string())?,
                        })
                    })
                    .collect()
            },
        )
        .await
    }

    /// Replaces only human dictionary settings and rebuilds FTS atomically; returns refreshed terms.
    /// 仅替换人工词典设置并原子重建全文索引；返回刷新词语。
    pub async fn set_custom_words(
        &self,
        library_id: &str,
        words: Vec<KnowledgeCustomWord>,
    ) -> Result<Vec<KnowledgeCustomWord>, String> {
        validate_custom_words(&words)?;
        // Dictionary replacement participates in the same library mutation domain.
        // 词典替换参与同一知识库变更域。
        let guard = self.library_mutation_guard(library_id).await?;
        // Current mode determines whether dictionary-dependent FTS exists.
        // 当前模式决定是否存在依赖词典的全文索引。
        let settings = self
            .require_active_library(library_id)
            .await?
            .settings
            .ok_or("ERR_KNOWLEDGE_SETTINGS_MISSING")?;
        // Keep dictionary rows, revision and rebuilt FTS in one SQLite transaction.
        // 将词典行、修订与重建全文索引保持在同一 SQLite 事务。
        let (_, binding_id) = self.ensure_library_binding(library_id).await?;
        local_sql::with_connection_async(&self.host_runtime.local,self.resolve_library_binding(&binding_id)?,move |connection|{
            let outcome = (|| {
                let transaction=connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(|e|e.to_string())?;
                for entry in tokenizer::list_custom_words(&transaction).map_err(|e|e.to_string())?.words {
                    tokenizer::remove_custom_word(&transaction,&entry.word).map_err(|e|e.to_string())?;
                }
                for entry in words {tokenizer::upsert_custom_word(&transaction,&entry.word,entry.weight as usize).map_err(|e|e.to_string())?;}
                transaction.execute("UPDATE knowledge_metadata SET value=CAST(value AS INTEGER)+1 WHERE key='dictionary_revision';",[]).map_err(|e|e.to_string())?;
                if settings.index_mode.has_fts() || is_present(&transaction)? {
                    rebuild_fts(&transaction)?;
                }
                transaction.commit().map_err(|e|e.to_string())
            })();
            resync_dictionary_after_rollback(connection, outcome)
        }).await?;
        drop(guard);
        self.list_custom_words(library_id).await
    }
}

/// Returns a dictionary transaction's outcome; after a rollback the shared tokenizer, which word
/// mutations refresh at once, is reloaded from the committed rows.
/// 返回词典事务的结果；回滚后，从已提交的行重新加载共享分词器（词语变更会立即刷新它）。
fn resync_dictionary_after_rollback(
    connection: &rusqlite::Connection,
    outcome: Result<(), String>,
) -> Result<(), String> {
    let Err(error) = outcome else {
        return Ok(());
    };
    match tokenizer::reload_custom_dictionary(connection) {
        Ok(()) => Err(error),
        Err(reload) => Err(format!(
            "{error}; reloading the committed dictionary also failed: {reload}"
        )),
    }
}

/// Validates complete custom dictionary settings; returns an explicit invalid-term or duplicate error.
/// 验证完整自定义词典设置；返回明确非法词语或重复错误。
pub(super) fn validate_custom_words(words: &[KnowledgeCustomWord]) -> Result<(), String> {
    // Settings and portable packages share exactly the same validation authority.
    // 设置与可移植套件共享完全相同的验证权威。
    let mut unique = BTreeSet::new();
    for entry in words {
        if entry.word.trim() != entry.word
            || entry.word.is_empty()
            || entry.word.chars().any(char::is_control)
            || entry.weight == 0
            || !unique.insert(&entry.word)
        {
            return Err("ERR_KNOWLEDGE_CUSTOM_WORD_INVALID".into());
        }
    }
    Ok(())
}
