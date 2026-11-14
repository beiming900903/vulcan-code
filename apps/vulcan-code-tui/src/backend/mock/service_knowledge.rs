//! Deterministic current-contract knowledge service; no native files, providers or paid calls.
//! 确定性当前契约知识库服务；不访问原生文件、供应商或付费调用。

use crate::{
    model::{ConversationSummary, Project},
    service_api::ServiceCall,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use vulcan_service_api::*;

/// A synthetic immutable content snapshot and its actual history images.
/// 合成不可变内容快照及其真实历史镜像。
#[derive(Clone)]
struct Change {
    /// Current wire summary.
    /// 当前线协议摘要。
    summary: Value,
    /// Exact typed-shape before/after images.
    /// 精确强类型形状前后镜像。
    items: Vec<Value>,
    /// Full synthetic document records at this history point.
    /// 此历史点完整合成文档记录。
    documents: Vec<Value>,
    /// Full explicit folder records at this history point.
    /// 此历史点完整明确目录记录。
    folders: Vec<Value>,
}

/// One independent library's current and immutable synthetic state.
/// 一个独立知识库的当前及不可变合成状态。
struct Library {
    /// Sole registry authority.
    /// 唯一注册表权威。
    summary: Value,
    /// Records shaped as document plus exact body.
    /// 形状为文档及精确正文的记录。
    documents: Vec<Value>,
    /// Current explicit folders.
    /// 当前明确目录。
    folders: Vec<Value>,
    /// Permanent synthetic history.
    /// 永久合成历史。
    changes: Vec<Change>,
    /// Current atomic dictionary entries.
    /// 当前原子词典条目。
    words: Vec<Value>,
    /// Latest durable operation, with a stable identity.
    /// 具有稳定身份的最新持久操作。
    operation: Option<Value>,
    /// Current active/desired vector progress.
    /// 当前活动及期望向量进度。
    vectors: Value,
}

/// One synthetic portable archive and the exact current content it carries.
/// 一个合成可移植归档及其携带精确当前内容。
#[derive(Clone)]
struct Package {
    /// Frozen inspection receipt; its fingerprint is a deterministic test token, not cryptographic evidence.
    /// 冻结检查回执；其指纹为确定性测试标记，不是密码学证据。
    inspection: Value,
    /// Current document records; history and derived vectors are excluded.
    /// 当前文档记录；排除历史及派生向量。
    documents: Vec<Value>,
    /// Current explicit folders.
    /// 当前明确目录。
    folders: Vec<Value>,
}

/// Current synthetic knowledge contracts, isolated per library, project and session.
/// 当前合成知识库契约，按库、项目及会话隔离。
pub(super) struct KnowledgeService {
    /// Physical library identities own all current and historical data.
    /// 物理知识库身份拥有全部当前及历史数据。
    libraries: BTreeMap<String, Library>,
    /// Presence means project authorization.
    /// 存在表示项目授权。
    bindings: Vec<Value>,
    /// Host-declared current project identities.
    /// 宿主声明当前项目身份。
    projects: BTreeSet<String>,
    /// Actual saved session to project ownership, supplied by World.
    /// 由 World 提供的真实已保存会话至项目归属。
    sessions: BTreeMap<String, String>,
    /// Explicitly saved selections; reads never materialize defaults.
    /// 明确已保存选择；读取绝不物化默认值。
    selections: BTreeMap<String, SessionKnowledgeLibrarySelectionRequest>,
    /// Synthetic archive paths own immutable inspected content.
    /// 合成归档路径拥有不可变已检查内容。
    packages: BTreeMap<String, Package>,
    /// Explicit synthetic native directories.
    /// 明确合成原生目录。
    directories: BTreeSet<String>,
    /// Unique synthetic identity source.
    /// 唯一合成身份来源。
    serial: u64,
}

/// Strictly decodes a current shared request and never echoes malformed values.
/// 严格解码当前共享请求，绝不回显畸形值。
/// Parameters: call owns the exact request envelope. Returns the shared request or a static diagnostic.
/// 参数：call 拥有精确请求封装。返回共享请求或静态诊断。
fn request<T: DeserializeOwned>(call: &ServiceCall) -> Result<T, String> {
    serde_json::from_value(call.param("/request").clone())
        .map_err(|_| "Invalid current knowledge request".into())
}

/// Reads one required argument from its documented exact location.
/// 从文档定义精确位置读取一个必填参数。
/// Parameters: call owns arguments; pointer names the declared field. Returns its string or an explicit protocol error.
/// 参数：call 拥有参数；pointer 指定声明字段。返回字符串或明确协议错误。
fn argument<'a>(call: &'a ServiceCall, pointer: &str) -> Result<&'a str, String> {
    call.param(pointer)
        .as_str()
        .ok_or_else(|| "Missing current knowledge argument".into())
}

impl Library {
    /// Creates current synthetic state from one declared fixture or independent package.
    /// 从一个声明夹具或独立套件创建当前合成状态。
    /// Parameters: summary owns identity/configuration; documents/folders own current content. Returns an independent library.
    /// 参数：summary 拥有身份及配置；documents 及 folders 拥有当前内容。返回独立知识库。
    fn new(summary: Value, documents: Vec<Value>, folders: Vec<Value>) -> Self {
        Self {
            summary,
            documents,
            folders,
            changes: Vec::new(),
            words: Vec::new(),
            operation: None,
            vectors: json!({"desiredSpaceId":null,"activeSpaceId":null,"pending":0,"ready":0,"failed":0,"paused":false,"failures":[],"nextOffset":null}),
        }
    }

    /// Requires the exact active current-format library before any content/configuration action.
    /// 在任何内容及配置动作前要求精确活动当前格式知识库。
    /// Parameters: none. Returns success only for active readable fixture state.
    /// 参数：无。仅对活动可读夹具状态返回成功。
    fn active(&self) -> Result<(), String> {
        if self.summary["lifecycleState"] != "active" || !self.summary["settings"].is_object() {
            return Err("Unsupported demo knowledge format; delete the library".into());
        }
        Ok(())
    }

    /// Recounts the explicitly simplified synthetic two-chunk-per-document model.
    /// 重新统计明确简化的每文档两切块合成模型。
    /// Parameters: none. Returns after updating current registry counts.
    /// 参数：无。更新当前注册表数量后返回。
    fn recount(&mut self) {
        self.summary["documentCount"] = json!(self.documents.len());
        self.summary["chunkCount"] = json!(self.documents.len() * 2);
        self.summary["pendingSummaryCount"] = json!(
            self.documents
                .iter()
                .filter(|entry| entry["document"]["summaryState"] != "provided")
                .count()
        );
    }

    /// Appends a permanent synthetic change and freezes its resulting content snapshot.
    /// 追加永久合成变更，并冻结其结果内容快照。
    /// Parameters: operation and target describe the human operation; items contain exact row images. Returns the new sequence.
    /// 参数：operation 及 target 描述人工操作；items 包含精确行镜像。返回新序号。
    fn change(&mut self, operation: &str, target: Option<u64>, items: Vec<Value>) -> u64 {
        // Sequence grows from the permanent log, never from the restored target.
        // 序号从永久日志增长，绝非从还原目标增长。
        let seq = self.changes.last().map_or(1, |change| {
            change.summary["seq"].as_u64().expect("synthetic sequence") + 1
        });
        self.recount();
        self.summary["headChangeSeq"] = json!(seq);
        self.summary["lastChangeAtMs"] = json!(seq);
        self.changes.push(Change { summary: json!({"seq":seq,"atMs":seq,"actor":{"kind":"user","sessionId":null,"turnId":null,"toolCallId":null},"operation":operation,"note":"Synthetic content transaction","restoreTargetSeq":target,"itemCount":items.len()}), items, documents: self.documents.clone(), folders: self.folders.clone() });
        seq
    }

    /// Starts one durable synthetic operation without a provider call.
    /// 开始一个持久合成操作，不调用供应商。
    /// Parameters: id is stable and kind is declared. Returns the queued current operation receipt.
    /// 参数：id 为稳定身份，kind 为声明种类。返回排队当前操作回执。
    fn start(&mut self, id: String, kind: &str) -> Result<Value, String> {
        if self
            .operation
            .as_ref()
            .is_some_and(|job| matches!(job["state"].as_str(), Some("queued" | "running")))
        {
            return Err("A synthetic knowledge operation is already active".into());
        }
        // Synthetic vector policy is deterministic and is not an embedding-quality test.
        // 合成向量策略为确定性，不是嵌入质量测试。
        let vector = matches!(
            self.summary["settings"]["indexMode"].as_str(),
            Some("vector" | "hybrid")
        );
        if vector && kind != "retry" {
            self.vectors["desiredSpaceId"] = json!(format!("demo-space-{id}"));
            self.vectors["pending"] = json!(
                self.documents
                    .iter()
                    .filter(|entry| entry["document"]["embedPolicy"] != "none")
                    .count()
            );
            self.vectors["failed"] = json!(0);
            self.vectors["ready"] = json!(0);
            self.vectors["paused"] = json!(false);
            self.vectors["failures"] = json!([]);
        } else if !vector {
            self.vectors = json!({"desiredSpaceId":null,"activeSpaceId":null,"pending":0,"ready":0,"failed":0,"paused":false,"failures":[],"nextOffset":null});
        }
        if kind == "retry" {
            self.vectors["paused"] = json!(false);
        }
        self.summary["indexStatus"] = json!(if matches!(
            self.summary["settings"]["indexMode"].as_str(),
            Some("bm25" | "hybrid")
        ) {
            "building"
        } else {
            "disabled"
        });
        // Zero-work operations still retain their stable queued receipt until observed.
        // 零工作操作仍保留稳定排队回执，直到被观察。
        let job = json!({"id":id,"libraryId":self.summary["id"],"kind":kind,"state":"queued","phase":"indexing","totalItems":self.summary["chunkCount"],"processedItems":0,"currentItem":null,"warnings":[],"errorMessage":null,"createdAtMs":1,"startedAtMs":null,"finishedAtMs":null});
        self.operation = Some(job.clone());
        Ok(job)
    }

    /// Advances only the mock job's deterministic queued/running/completed sequence.
    /// 仅推进模拟任务确定性排队、运行及完成序列。
    /// Parameters: none. Returns after updating the synthetic derived observations.
    /// 参数：无。更新合成派生观察后返回。
    fn observe(&mut self) {
        if let Some(job) = self.operation.as_mut() {
            match job["state"].as_str() {
                Some("queued") => {
                    job["state"] = json!("running");
                    job["startedAtMs"] = json!(2);
                }
                Some("running") => {
                    job["state"] = json!("completed");
                    job["finishedAtMs"] = json!(3);
                    job["processedItems"] = job["totalItems"].clone();
                    self.summary["indexStatus"] = json!(if matches!(
                        self.summary["settings"]["indexMode"].as_str(),
                        Some("bm25" | "hybrid")
                    ) {
                        "ready"
                    } else {
                        "disabled"
                    });
                    self.vectors["ready"] = json!(
                        self.vectors["ready"]
                            .as_u64()
                            .expect("synthetic ready count")
                            + self.vectors["pending"]
                                .as_u64()
                                .expect("synthetic pending count")
                    );
                    self.vectors["pending"] = json!(0);
                    self.vectors["activeSpaceId"] = self.vectors["desiredSpaceId"].clone();
                }
                _ => {}
            }
        }
    }

    /// Reads one exact current or historical body with its requested inclusive line window.
    /// 按请求包含边界行窗口读取一个精确当前或历史正文。
    /// Parameters: documents is the selected immutable/current snapshot; selector owns ID/path and range. Returns exact body and next line.
    /// 参数：documents 为选定不可变或当前快照；selector 拥有身份或路径及范围。返回精确正文及下一行。
    fn read(documents: &[Value], selector: &KnowledgeDocumentReadRequest) -> Result<Value, String> {
        if selector.document_id.is_some() == selector.path.is_some() {
            return Err("Exactly one document selector is required".into());
        }
        // These are the schema-declared mutually exclusive selectors.
        // 这些为结构声明互斥选择器。
        let entry = documents
            .iter()
            .find(|entry| match (&selector.document_id, &selector.path) {
                (Some(id), None) => entry["document"]["id"] == *id,
                (None, Some(path)) => entry["document"]["path"] == *path,
                _ => false,
            })
            .ok_or("Unknown demo document")?;
        // Normalized fixture bodies preserve literal blank lines and indentation.
        // 已规范化夹具正文保留字面空行及缩进。
        let body = entry["body"].as_str().ok_or("Invalid demo document body")?;
        // The mock reads the explicit fixture's lines; bounded behavior in production is tested by the store.
        // 模拟读取明确夹具行；生产有界行为由存储测试验证。
        let lines = body.split_inclusive('\n').collect::<Vec<_>>();
        // A missing endpoint means all remaining synthetic lines, not another document.
        // 省略末端表示全部剩余合成行，绝非另一文档。
        let start = selector.start_line.unwrap_or(1);
        // Empty documents have an explicit empty range.
        // 空文档具有明确空范围。
        let end = selector.end_line.unwrap_or(lines.len()).min(lines.len());
        if start == 0
            || (start > lines.len() && !lines.is_empty())
            || (end < start && !lines.is_empty())
        {
            return Err("Invalid demo body range".into());
        }
        Ok(
            json!({"document":entry["document"],"content":if lines.is_empty(){String::new()}else{lines[start-1..end].concat()},"startLine":if lines.is_empty(){0}else{start},"endLine":end,"nextLine":if end<lines.len(){Some(end+1)}else{None}}),
        )
    }
}

impl KnowledgeService {
    /// Executes package calls against isolated synthetic host files without real filesystem or model effects.
    /// 对隔离合成宿主文件执行套件调用，不影响真实文件系统或模型。
    /// Parameters: call contains one current package RPC. Returns its exact current response or a validation error.
    /// 参数：call 包含一个当前套件 RPC。返回其精确当前响应或校验错误。
    fn package(&mut self, call: &ServiceCall) -> Result<Value, String> {
        match call.method {
            "service_export_knowledge_package" => {
                // Only declared synthetic directories exist in this host model.
                // 此宿主模型仅存在已声明合成目录。
                let query: KnowledgePackageExportRequest = request(call)?;
                if !self.directories.contains(&query.destination_directory) {
                    return Err("Unknown synthetic destination directory".into());
                }
                // Export freezes current content and configuration; no history or vector rows are portable.
                // 导出冻结当前内容及配置；历史及向量行均不可移植。
                let library = self
                    .libraries
                    .get(&query.library_id)
                    .ok_or("Unknown demo library")?;
                library.active()?;
                // Synthetic package identity is a deterministic test token, not a cryptographic archive implementation.
                // 合成套件身份为确定性测试令牌，绝非密码学归档实现。
                let sha256 = format!("{:064x}", self.serial);
                // Native package names use mock IDs, independently of arbitrary library display names.
                // 原生套件名使用模拟身份，独立于任意知识库显示名称。
                let archive_path = format!(
                    "{}/{}.vkb",
                    query.destination_directory.trim_end_matches('/'),
                    sha256
                );
                // JSON byte length is sufficient for synthetic listing behavior; store tests validate real ZIP byte sizes.
                // JSON 字节长度足以验证合成列表行为；存储测试验证真实 ZIP 字节数。
                let byte_size = serde_json::to_vec(&library.documents)
                    .map_err(|_| "Invalid synthetic content")?
                    .len();
                // This manifest mirrors the current portable protocol, without a duplicate decoder or legacy branch.
                // 此清单镜像当前可移植协议，不复制解码器或旧版分支。
                let inspection = json!({"archivePath":archive_path,"sha256":sha256,"byteSize":byte_size,"manifest":{
                    "format":"vulcan-code-knowledge-base","version":3,"name":library.summary["name"],"description":library.summary["description"],
                    "settings":library.summary["settings"],"customWords":library.words,"documentCount":library.documents.len(),"folderCount":library.folders.len(),
                    "contentSha256":sha256,"exportedAtMs":self.serial
                }});
                self.packages.insert(
                    archive_path,
                    Package {
                        inspection: inspection.clone(),
                        documents: library.documents.clone(),
                        folders: library.folders.clone(),
                    },
                );
                self.serial += 1;
                Ok(inspection)
            }
            "service_inspect_knowledge_package" => {
                // Inspection reads exactly one immutable synthetic file.
                // 预检读取精确一个不可变合成文件。
                let query: KnowledgePackageInspectRequest = request(call)?;
                Ok(self
                    .packages
                    .get(&query.archive_path)
                    .ok_or("Unknown synthetic package")?
                    .inspection
                    .clone())
            }
            "service_import_knowledge_package" => {
                // The name is editable; archive identity and bytes remain frozen by inspection.
                // 名称可编辑；归档身份及字节由预检冻结。
                let query: KnowledgePackageImportRequest = request(call)?;
                // Validate the complete immutable source before allocating the independent library.
                // 配置独立知识库前校验完整不可变来源。
                let package = self
                    .packages
                    .get(&query.archive_path)
                    .ok_or("Unknown synthetic package")?;
                if package.inspection["sha256"] != query.expected_sha256 {
                    return Err("Package changed after inspection".into());
                }
                // Frozen response-only route facts are deliberately excluded from the shared request DTO.
                // 冻结响应专用路由事实明确排除于共享请求 DTO。
                let frozen = &package.inspection["manifest"]["settings"];
                // Embedding is optional only for the schema-declared non-vector modes.
                // 嵌入仅在结构声明非向量模式下可省略。
                let embedding = if frozen["embedding"].is_null() {
                    Value::Null
                } else {
                    // The package owns this exact declared route.
                    // 套件拥有此精确声明路由。
                    let route = &frozen["embedding"]["route"];
                    json!({"route":{"providerId":route["providerId"],"protocol":route["protocol"],"bindingKind":route["bindingKind"],"serviceEntryId":route["serviceEntryId"],"modelId":route["modelId"],"connectionId":route["connectionId"]},"dimensions":frozen["embedding"]["dimensions"],"batchSize":frozen["embedding"]["batchSize"]})
                };
                // Decode through the actual request type before any registry mutation.
                // 任何注册表变更前通过实际请求类型解码。
                let settings: KnowledgeLibrarySettingsRequest = serde_json::from_value(
                    json!({"indexMode":frozen["indexMode"],"embedding":embedding}),
                )
                .map_err(|_| "Invalid synthetic package settings")?;
                // Portable content is cloned so importing never mutates or binds the source.
                // 克隆可移植内容，确保导入绝不修改或绑定来源。
                let mut documents = package.documents.clone();
                // Explicit directory metadata starts in the new import transaction.
                // 明确目录元信息从新导入事务开始。
                let mut folders = package.folders.clone();
                // Dictionary configuration is portable, unlike generated indexes.
                // 词典配置可移植，生成索引不可移植。
                let words = package.inspection["manifest"]["customWords"]
                    .as_array()
                    .ok_or("Invalid synthetic dictionary")?
                    .clone();
                // Purpose is current library configuration, not a historical actor record.
                // 用途为当前知识库配置，绝非历史执行者记录。
                let description = package.inspection["manifest"]["description"]
                    .as_str()
                    .ok_or("Invalid synthetic description")?
                    .to_string();
                // Fresh physical ownership has no project grants.
                // 新物理所有权不具有项目授权。
                let created = self.create(KnowledgeLibraryCreateRequest {
                    name: query.name,
                    description,
                    settings,
                    project_ids: Vec::new(),
                })?;
                // This newly allocated ID is the sole imported content owner.
                // 此新配置身份为导入内容唯一所有者。
                let id = created["id"]
                    .as_str()
                    .expect("created synthetic library")
                    .to_string();
                for entry in &mut documents {
                    entry["libraryId"] = json!(id);
                    entry["document"]["id"] = json!(self.id("document"));
                    entry["document"]["revision"] = json!(1);
                    entry["document"]["createdChangeSeq"] = json!(1);
                    entry["document"]["updatedChangeSeq"] = json!(1);
                }
                for folder in &mut folders {
                    folder["createdChangeSeq"] = json!(1);
                    folder["updatedChangeSeq"] = json!(1);
                }
                // All imported entities belong to exactly one fresh import change.
                // 全部导入实体属于精确一个全新导入变更。
                let items = documents.iter().map(|entry| json!({"entity":"document","entityKey":entry["document"]["id"],"before":null,"after":entry["document"]})).chain(folders.iter().map(|folder|json!({"entity":"folder","entityKey":folder["pathKey"],"before":null,"after":folder}))).collect();
                // The worker identity is independent from content history.
                // 工作者身份独立于内容历史。
                let operation_id = self.id("operation");
                // Newly created state cannot be absent after the successful atomic mock creation above.
                // 上述原子模拟创建成功后，新创建状态不可能缺失。
                let library = self
                    .libraries
                    .get_mut(&id)
                    .expect("created synthetic state");
                library.documents = documents;
                library.folders = folders;
                library.words = words;
                // A single import starts new history even when the package is empty.
                // 即使套件为空，单个导入也会开启新历史。
                let seq = library.change("import", None, items);
                if matches!(
                    library.summary["settings"]["indexMode"].as_str(),
                    Some("vector" | "hybrid")
                ) {
                    library.start(operation_id, "import")?;
                }
                Ok(json!({"library":library.summary,"changeSeq":seq}))
            }
            "service_browse_knowledge_packages" => {
                // None selects the synthetic host's declared initial directory.
                // None 选择合成宿主声明的初始目录。
                let path = match call.params.get("path") {
                    Some(Value::Null) => "X:/synthetic",
                    Some(Value::String(path)) => path,
                    _ => return Err("Invalid synthetic directory path".into()),
                };
                if !self.directories.contains(path) {
                    return Err("Unknown synthetic directory".into());
                }
                // Only direct children are exposed; paths come from the synthetic host authority.
                // 仅公开直接子项；路径来自合成宿主权威。
                let packages = self.packages.iter().filter(|(file,_)|file.rsplit_once('/').is_some_and(|(parent,_)|parent==path.trim_end_matches('/'))).map(|(file,package)|json!({"path":file,"name":file.rsplit('/').next().expect("synthetic file name"),"byteSize":package.inspection["byteSize"]})).collect::<Vec<_>>();
                // The root is intentionally finite; browsing never touches a real drive.
                // 根目录有意限定有限；浏览绝不访问真实磁盘。
                let entries = self.directories.iter().filter(|directory|directory.as_str()!=path && directory.rsplit_once('/').is_some_and(|(parent,_)|parent==path.trim_end_matches('/'))).map(|directory|json!({"path":directory,"name":directory.rsplit('/').next().expect("synthetic directory name"),"hidden":false})).collect::<Vec<_>>();
                Ok(
                    json!({"directory":{"path":path,"parentPath":if path=="X:/synthetic" { Some("X:/") }else{None},"roots":[{"path":"X:/","label":"X:"}],"canSelect":true,"entries":entries,"notice":null},"packages":packages}),
                )
            }
            _ => Err("Unknown current package method".into()),
        }
    }

    /// Loads only the new fixture shape; obsolete fields and methods have no adaptation path.
    /// 仅加载新夹具形状；过期字段及方法不存在适配路径。
    /// Parameters: fixture is repository-owned current data. Returns isolated synthetic state.
    /// 参数：fixture 为仓库拥有当前数据。返回隔离合成状态。
    pub(super) fn new(fixture: &Value) -> Self {
        // The fixture registry is the sole initial metadata authority.
        // 夹具注册表为唯一初始元信息权威。
        let mut libraries = BTreeMap::new();
        for summary in fixture["loads"]["service_list_knowledge_libraries"]
            .as_array()
            .expect("current registry fixture")
        {
            // Records explicitly declare their physical library owner.
            // 记录明确声明其物理知识库所有者。
            let id = summary["id"].as_str().expect("fixture library id");
            // Body records are selected by declared library ID, never source/corpus compatibility fields.
            // 正文记录按声明知识库身份选择，绝不使用来源及语料兼容字段。
            let documents = fixture["knowledgeDocuments"]
                .as_array()
                .expect("current documents fixture")
                .iter()
                .filter(|entry| entry["libraryId"] == id)
                .cloned()
                .collect::<Vec<_>>();
            // This fixture owns its explicit folders independently of its document records.
            // 此夹具独立于文档记录拥有明确目录。
            let folders = fixture["knowledgeFolders"]
                .as_array()
                .expect("current folders fixture")
                .iter()
                .filter(|entry| entry["libraryId"] == id)
                .map(|entry| entry["folder"].clone())
                .collect();
            // Initial fixture content begins with one immutable import-style change.
            // 初始夹具内容从一个不可变导入型变更开始。
            let mut library = Library::new(summary.clone(), documents, folders);
            if !library.documents.is_empty() {
                // Each imported current document has exactly one initial creation image.
                // 每个导入当前文档具有精确一个初始创建镜像。
                let items = library.documents.iter().map(|entry| json!({"entity":"document","entityKey":entry["document"]["id"],"before":null,"after":entry["document"]})).collect();
                library.change("import", None, items);
            }
            libraries.insert(id.into(), library);
        }
        // No library is inferred when the declared registry is empty.
        // 声明注册表为空时不推断知识库。
        let mut state = Self {
            libraries,
            bindings: fixture["loads"]["service_list_knowledge_project_bindings"]
                .as_array()
                .expect("current bindings fixture")
                .clone(),
            projects: BTreeSet::new(),
            sessions: BTreeMap::new(),
            selections: BTreeMap::new(),
            packages: BTreeMap::new(),
            directories: BTreeSet::from(["X:/".into(), "X:/synthetic".into()]),
            serial: 1,
        };
        state.context(
            &super::super::fixtures::projects(),
            &super::super::fixtures::conversations(),
        );
        state
    }

    /// Reconciles exact project/session ownership from World's current typed records.
    /// 从 World 当前强类型记录校正精确项目及会话归属。
    /// Parameters: projects and conversations are authoritative mock host rows. Returns without changing selection defaults.
    /// 参数：projects 及 conversations 为权威模拟宿主行。返回且不改变选择默认值。
    pub(super) fn context(&mut self, projects: &[Project], conversations: &[ConversationSummary]) {
        self.projects = projects.iter().map(|project| project.id.clone()).collect();
        self.sessions = conversations
            .iter()
            .filter_map(|conversation| {
                conversation
                    .project_id
                    .as_ref()
                    .map(|project| (conversation.id.clone(), project.clone()))
            })
            .collect();
    }

    /// Allocates identities only inside the synthetic service.
    /// 仅在合成服务内部配置身份。
    /// Parameters: prefix identifies the resource kind. Returns a unique process-local ID.
    /// 参数：prefix 标识资源种类。返回唯一进程本地身份。
    fn id(&mut self, prefix: &str) -> String {
        let id = format!("demo-{prefix}-{}", self.serial);
        self.serial += 1;
        id
    }

    /// Requires a live explicit project grant for the exact library.
    /// 要求精确知识库具有实时明确项目授权。
    /// Parameters: project and library are exact identities. Returns success only for current active authorization.
    /// 参数：project 及 library 为精确身份。仅对当前活动授权返回成功。
    fn authorize(&self, project: &str, library: &str) -> Result<(), String> {
        self.libraries
            .get(library)
            .ok_or("Unknown demo library")?
            .active()?;
        if !self.projects.contains(project)
            || !self
                .bindings
                .iter()
                .any(|binding| binding["projectId"] == project && binding["libraryId"] == library)
        {
            return Err("Knowledge library is not authorized for this project".into());
        }
        Ok(())
    }

    /// Resolves effective selection from explicit saved IDs or live project defaults.
    /// 从明确已保存身份或实时项目默认值解析有效选择。
    /// Parameters: session is an actual saved mock session. Returns current authorized selection without persisting defaults.
    /// 参数：session 为实际已保存模拟会话。返回当前授权选择，不持久化默认值。
    fn selection(&self, session: &str) -> Result<Value, String> {
        // Session-to-project ownership is supplied by World, never guessed from the ID.
        // 会话至项目归属由 World 提供，绝不从身份推测。
        let project = self
            .sessions
            .get(session)
            .ok_or("Unknown demo session project")?;
        // Absent persisted selection is the shared contract's inherited runtime default.
        // 缺少持久选择为共享契约继承运行时默认值。
        let saved = self.selections.get(session);
        // Only explicit mode uses its persisted ID subset.
        // 仅明确模式使用其持久身份子集。
        let explicit = saved.is_some_and(|selection| selection.mode == "explicit");
        // Live grants always bound effective selection after revocation.
        // 实时授权始终在撤销后约束有效选择。
        let effective = if explicit {
            saved
                .expect("explicit saved selection")
                .library_ids
                .iter()
                .filter(|id| self.authorize(project, id).is_ok())
                .cloned()
                .collect::<Vec<_>>()
        } else {
            self.bindings
                .iter()
                .filter(|binding| {
                    binding["projectId"] == *project && binding["defaultEnabled"] == true
                })
                .filter_map(|binding| binding["libraryId"].as_str())
                .filter(|id| self.authorize(project, id).is_ok())
                .map(str::to_string)
                .collect()
        };
        Ok(
            json!({"mode":if explicit{"explicit"}else{"inherit"},"libraryIds":saved.map(|selection|selection.library_ids.clone()).unwrap_or_default(),"effectiveLibraryIds":effective}),
        )
    }

    /// Creates a complete current-format empty synthetic library with explicit settings.
    /// 使用明确设置创建完整当前格式空合成知识库。
    /// Parameters: request is the shared creation DTO. Returns the authoritative new summary.
    /// 参数：request 为共享创建 DTO。返回权威新摘要。
    fn create(&mut self, request: KnowledgeLibraryCreateRequest) -> Result<Value, String> {
        if request.name.trim().is_empty()
            || self.libraries.values().any(|library| {
                library.summary["name"]
                    .as_str()
                    .is_some_and(|name| name.to_lowercase() == request.name.trim().to_lowercase())
            })
        {
            return Err("Demo library name is empty or already exists".into());
        }
        if request
            .project_ids
            .iter()
            .any(|project| !self.projects.contains(project))
        {
            return Err("Unknown demo project".into());
        }
        // Request shape and required embedding membership are checked before any registry write.
        // 在任何注册表写入前检查请求形状及必要嵌入成员关系。
        let settings = Self::settings(request.settings)?;
        // A unique physical ID is independent from every imported document identity.
        // 唯一物理身份独立于每个导入文档身份。
        let id = self.id("library");
        // The complete registry row contains no old import timestamps or policy fields.
        // 完整注册表行不含旧导入时间戳或策略字段。
        let summary = json!({"id":id,"name":request.name.trim(),"description":request.description,"settings":settings,"lifecycleState":"active","errorMessage":null,"documentCount":0,"chunkCount":0,"pendingSummaryCount":0,"indexStatus":if matches!(settings["indexMode"].as_str(),Some("bm25"|"hybrid")) {"ready"}else{"disabled"},"headChangeSeq":0,"lastChangeAtMs":null,"createdAtMs":self.serial,"updatedAtMs":self.serial});
        self.libraries.insert(
            id.clone(),
            Library::new(summary.clone(), Vec::new(), Vec::new()),
        );
        for project in request.project_ids {
            self.bindings
                .push(json!({"libraryId":id,"projectId":project,"defaultEnabled":false}));
        }
        Ok(summary)
    }

    /// Converts explicit request settings into the declared frozen synthetic response shape.
    /// 将明确请求设置转换为声明冻结合成响应形状。
    /// Parameters: settings is the current shared DTO. Returns a current response or an explicit mode/embedding error.
    /// 参数：settings 为当前共享 DTO。返回当前响应或明确模式及嵌入错误。
    fn settings(settings: KnowledgeLibrarySettingsRequest) -> Result<Value, String> {
        // Vector membership is a declared mode branch, never an inferred route fallback.
        // 向量成员关系为声明模式分支，绝非推断路由兜底。
        let vector = matches!(
            settings.index_mode,
            KnowledgeIndexModeRequest::Vector | KnowledgeIndexModeRequest::Hybrid
        );
        if vector != settings.embedding.is_some() {
            return Err("Embedding configuration does not match the selected mode".into());
        }
        // These synthetic frozen facts satisfy the UI response contract without executing a model.
        // 这些合成冻结事实满足界面响应契约，不执行模型。
        let embedding = if let Some(embedding) = settings.embedding {
            if embedding.dimensions == 0 || embedding.batch_size == Some(0) {
                return Err("Invalid demo embedding dimensions or batch".into());
            }
            // The optional batch default is explicitly declared by this mock fixture, not a production limit.
            // 可选批量默认值由此模拟夹具明确声明，绝非生产上限。
            json!({"route":{"providerId":embedding.route.provider_id,"protocol":embedding.route.protocol,"bindingKind":embedding.route.binding_kind,"serviceEntryId":embedding.route.service_entry_id,"modelId":embedding.route.model_id,"connectionId":embedding.route.connection_id,"usageScope":"internal_background"},"modelFingerprint":"synthetic-model-fingerprint","dimensions":embedding.dimensions,"batchSize":embedding.batch_size.unwrap_or(1),"metric":"cosine"})
        } else {
            Value::Null
        };
        Ok(json!({"indexMode":settings.index_mode,"embedding":embedding}))
    }
}

impl KnowledgeService {
    /// Lists an exact logical page with a mock-owned cursor bound to library, path, depth and content head.
    /// 列出精确逻辑页，模拟拥有游标绑定知识库、路径、深度及内容头。
    /// Parameters: request owns selectors; folders_only matches the separate folder endpoint. Returns current entry DTOs.
    /// 参数：request 拥有选择器；folders_only 匹配独立目录端点。返回当前条目 DTO。
    fn list(
        &self,
        request: KnowledgeLibraryDocumentListRequest,
        folders_only: bool,
    ) -> Result<Value, String> {
        // Only the requested library may provide content.
        // 仅请求知识库可提供内容。
        let library = self
            .libraries
            .get(&request.library_id)
            .ok_or("Unknown demo library")?;
        library.active()?;
        if request.limit == 0 || request.depth == 0 {
            return Err("Invalid demo directory page".into());
        }
        // Canonical root and descendants are explicit in this synthetic logical format.
        // 规范根及后代在此合成逻辑格式中明确声明。
        let prefix = if request.path.is_empty() {
            String::new()
        } else {
            format!("{}/", request.path)
        };
        // Explicit and implicit folders are both defined by the current content contract.
        // 明确及隐式目录均由当前内容契约定义。
        let mut folders = BTreeMap::<String, String>::new();
        for folder in &library.folders {
            folders.insert(
                folder["path"].as_str().ok_or("Invalid demo folder")?.into(),
                folder["description"]
                    .as_str()
                    .ok_or("Invalid demo folder description")?
                    .into(),
            );
        }
        for entry in &library.documents {
            // Logical slash segments, not native path guessing, define implicit ancestors.
            // 逻辑斜杠路径段定义隐式祖先，而非原生路径推测。
            let path = entry["document"]["path"]
                .as_str()
                .ok_or("Invalid demo document path")?;
            // Every non-final segment is a logical folder.
            // 每个非末尾段均为逻辑目录。
            let parts = path.split('/').collect::<Vec<_>>();
            for end in 1..parts.len() {
                folders.entry(parts[..end].join("/")).or_default();
            }
        }
        // Folder ordering precedes document ordering, with stable lexical path keys.
        // 目录排序先于文档排序，使用稳定词法路径键。
        let mut entries = BTreeMap::<(u8, String), Value>::new();
        for (path, description) in folders {
            if let Some(relative) = path.strip_prefix(&prefix).filter(|relative| {
                !relative.is_empty() && relative.split('/').count() <= request.depth
            }) {
                // The relative segment is used only to prove bounded descendant membership.
                // 相对段仅用于证明有界后代成员关系。
                let _ = relative;
                // Count current documents under this exact folder prefix.
                // 统计此精确目录前缀下当前文档。
                let count = library
                    .documents
                    .iter()
                    .filter(|entry| {
                        entry["document"]["path"]
                            .as_str()
                            .is_some_and(|document| document.starts_with(&(path.clone() + "/")))
                    })
                    .count();
                entries.insert((0, path.clone()), json!({"kind":"folder","path":path,"description":description,"documentCount":count,"document":null}));
            }
        }
        for entry in &library.documents {
            // One current summary owns this exact path, metadata and stable ID.
            // 一个当前摘要拥有此精确路径、元信息及稳定身份。
            let document = &entry["document"];
            // The service-defined logical path is the sole descendant selector.
            // 服务定义逻辑路径为唯一后代选择器。
            let path = document["path"]
                .as_str()
                .ok_or("Invalid demo document path")?;
            if path.strip_prefix(&prefix).is_some_and(|relative| {
                !relative.is_empty() && relative.split('/').count() <= request.depth
            }) {
                entries.insert((1, path.into()), json!({"kind":"document","path":path,"description":document["summary"],"documentCount":1,"document":document}));
            }
        }
        // Mock cursors are opaque to the client and reject changed snapshot ownership.
        // 模拟游标对客户端不透明，并拒绝变化快照归属。
        let offset = if let Some(cursor) = request.cursor {
            // Decode only this mock producer's exact cursor document.
            // 仅解码此模拟产出者精确游标文档。
            let cursor: Value = serde_json::from_str(&cursor).map_err(|_| "Invalid demo cursor")?;
            if cursor["libraryId"] != request.library_id
                || cursor["path"] != request.path
                || cursor["depth"] != request.depth
                || cursor["head"] != library.summary["headChangeSeq"]
            {
                return Err("Stale demo knowledge cursor".into());
            }
            usize::try_from(
                cursor["offset"]
                    .as_u64()
                    .ok_or("Invalid demo cursor offset")?,
            )
            .map_err(|_| "Invalid demo cursor offset")?
        } else {
            0
        };
        // The selected viewport is bounded by the caller's declared request.
        // 选定视口受调用方声明请求约束。
        let entries = entries.into_values().collect::<Vec<_>>();
        if offset > entries.len() {
            return Err("Invalid demo cursor offset".into());
        }
        // Continuation remains bound to the full combined-entry page, matching the folder endpoint.
        // 续读仍绑定完整合并条目页，与目录端点相同。
        let end = offset
            .saturating_add(request.limit as usize)
            .min(entries.len());
        // No client receives or reconstructs this cursor's internal fields.
        // 客户端不接收或重建此游标内部字段。
        let next = (end < entries.len()).then(|| json!({"libraryId":request.library_id,"path":request.path,"depth":request.depth,"head":library.summary["headChangeSeq"],"offset":end}).to_string());
        Ok(
            json!({"entries":entries[offset..end].iter().filter(|entry| !folders_only || entry["kind"] == "folder").collect::<Vec<_>>(),"nextCursor":next,"headChangeSeq":library.summary["headChangeSeq"]}),
        )
    }

    /// Simulates query rendering only; real BM25/vector behavior is verified in the store tests.
    /// 仅模拟查询展示；真实 BM25 及向量行为由存储测试验证。
    /// Parameters: ids are exact authorized libraries; query/limit define this synthetic match. Returns current response fields.
    /// 参数：ids 为精确授权知识库；query 及 limit 定义此合成匹配。返回当前响应字段。
    fn search(&self, ids: &[String], query: &str, limit: u32) -> Result<Value, String> {
        if query.trim().is_empty() {
            return Err("Empty demo knowledge query".into());
        }
        // Every queried library retains its declared mode and independent lag/failure status.
        // 每个查询知识库保留其声明模式及独立滞后或失败状态。
        let mut statuses = Vec::new();
        // Synthetic hits never claim a real embedding call or production ranking quality.
        // 合成命中绝不声称真实嵌入调用或生产排序质量。
        let mut hits = Vec::new();
        // Browsing-only libraries remain an explicit per-library search failure.
        // 仅浏览知识库保持为明确逐库检索失败。
        let mut failures = Vec::new();
        for id in ids {
            // Exact physical ownership is checked before reading any current body.
            // 在读取任何当前正文前检查精确物理归属。
            let library = self.libraries.get(id).ok_or("Unknown demo library")?;
            library.active()?;
            statuses.push(json!({"libraryId":id,"indexMode":library.summary["settings"]["indexMode"],"ftsStatus":library.summary["indexStatus"],"embeddings":library.vectors}));
            if library.summary["settings"]["indexMode"] == "browse" {
                failures.push(
                    json!({"libraryId":id,"query":query,"error":"ERR_KNOWLEDGE_BROWSE_ONLY"}),
                );
                continue;
            }
            for entry in &library.documents {
                // Matching is deterministic fixture text matching, not a second search engine.
                // 匹配为确定性夹具文本匹配，不是第二个检索引擎。
                let body = entry["body"].as_str().ok_or("Invalid demo body")?;
                if !body.to_lowercase().contains(&query.to_lowercase()) {
                    continue;
                }
                // Current metadata remains attached to its exact synthetic body.
                // 当前元信息保持属于其精确合成正文。
                let document = &entry["document"];
                hits.push(json!({"libraryId":id,"libraryName":library.summary["name"],"documentId":document["id"],"revision":document["revision"],"contentSha256":document["bodySha256"],"path":document["path"],"title":document["title"],"summary":document["summary"],"chunkId":format!("{}:body",document["id"].as_str().ok_or("Invalid demo document id")?),"kind":"body","headingPath":"","startLine":1,"endLine":document["lineCount"],"content":body,"score":1.0,"matchKinds":["fts"]}));
            }
        }
        hits.truncate(limit as usize);
        Ok(json!({"queries":[query],"hits":hits,"libraries":statuses,"failures":failures}))
    }

    /// Restores an immutable synthetic snapshot as a new permanent change.
    /// 将不可变合成快照还原为新永久变更。
    /// Parameters: request freezes library and target. Returns the new content receipt without deleting history.
    /// 参数：request 冻结知识库及目标。返回新内容回执，不删除历史。
    fn restore(&mut self, request: KnowledgeRestoreRequest) -> Result<Value, String> {
        // The current owner is checked before any restoration work.
        // 在任何还原工作前检查当前所有者。
        let library = self
            .libraries
            .get_mut(&request.library_id)
            .ok_or("Unknown demo library")?;
        library.active()?;
        // Zero explicitly denotes the initially empty snapshot.
        // 零明确表示初始空快照。
        let (mut documents, folders) = if request.target_seq == 0 {
            (Vec::new(), Vec::new())
        } else {
            // Immutable sequence identity, not a current array position, selects the target.
            // 不可变序号身份选择目标，而非当前数组位置。
            let target = library
                .changes
                .iter()
                .find(|change| change.summary["seq"] == request.target_seq)
                .ok_or("Unknown demo history target")?;
            (target.documents.clone(), target.folders.clone())
        };
        // Recreated and modified rows receive revisions above all historical revisions.
        // 重建及修改行获得高于全部历史修订的修订号。
        for entry in &mut documents {
            // A restored body never revives an obsolete optimistic revision.
            // 还原正文绝不恢复过期乐观修订。
            let maximum = library
                .changes
                .iter()
                .flat_map(|change| &change.documents)
                .filter(|old| old["document"]["id"] == entry["document"]["id"])
                .filter_map(|old| old["document"]["revision"].as_u64())
                .max()
                .unwrap_or(0);
            entry["document"]["revision"] = json!(maximum + 1);
        }
        // Exact IDs form the before/after image union, including deletions.
        // 精确身份组成前后镜像并集，包含删除。
        let ids = library
            .documents
            .iter()
            .chain(&documents)
            .map(|entry| {
                entry["document"]["id"]
                    .as_str()
                    .expect("synthetic id")
                    .to_string()
            })
            .collect::<BTreeSet<_>>();
        // Images retain the complete current metadata shape for both sides.
        // 镜像为两侧保留完整当前元信息形状。
        let mut items = ids.into_iter().map(|id| json!({"entity":"document","entityKey":id,"before":library.documents.iter().find(|entry|entry["document"]["id"]==id).map(|entry|&entry["document"]),"after":documents.iter().find(|entry|entry["document"]["id"]==id).map(|entry|&entry["document"])})).collect::<Vec<_>>();
        // Explicit folder metadata is restorable independently from document-derived ancestors.
        // 明确目录元信息可独立于文档派生祖先还原。
        let paths = library
            .folders
            .iter()
            .chain(&folders)
            .map(|folder| {
                folder["pathKey"]
                    .as_str()
                    .expect("synthetic folder key")
                    .to_string()
            })
            .collect::<BTreeSet<_>>();
        for key in paths {
            items.push(json!({"entity":"folder","entityKey":key,"before":library.folders.iter().find(|folder|folder["pathKey"]==key),"after":folders.iter().find(|folder|folder["pathKey"]==key)}));
        }
        library.documents = documents;
        library.folders = folders;
        // The new sequence appends to the same permanent log.
        // 新序号追加至相同永久日志。
        let seq = library.change("restore", Some(request.target_seq), items);
        Ok(
            json!({"changeSeq":seq,"documents":library.documents.iter().map(|entry|&entry["document"]).collect::<Vec<_>>(),"unchanged":[],"pendingEmbeddings":library.vectors["pending"]}),
        )
    }

    /// Dispatches only the current method set; old knowledge methods are explicit errors.
    /// 仅派发当前方法集合；旧知识库方法为明确错误。
    /// Parameters: call is the same request sent by the real backend. Returns current synthetic DTOs or an explicit failure.
    /// 参数：call 为真实后端发送的相同请求。返回当前合成 DTO 或明确失败。
    pub(super) fn answer(&mut self, call: &ServiceCall) -> Result<Value, String> {
        match call.method {
            "service_list_knowledge_libraries" => {
                // Optional projectId is a declared API subset, not an alternate registry owner.
                // 可选 projectId 为声明 API 子集，绝非替代注册表所有者。
                let project = match call.param("/projectId") {
                    Value::Null => None,
                    Value::String(project) => Some(project.as_str()),
                    _ => return Err("Invalid demo project selector".into()),
                };
                Ok(json!(
                    self.libraries
                        .iter()
                        .filter(|(id, _)| project
                            .is_none_or(|project| self.authorize(project, id).is_ok()))
                        .map(|(_, library)| &library.summary)
                        .collect::<Vec<_>>()
                ))
            }
            "service_create_knowledge_library" => self.create(request(call)?),
            "service_rename_knowledge_library" | "service_set_knowledge_library_settings" => {
                // Both current endpoints accept only human name/purpose, never mode or old policy fields.
                // 两个当前端点仅接受人工名称及用途，绝不接受模式或旧策略字段。
                let rename: KnowledgeLibraryRenameRequest = request(call)?;
                if rename.name.trim().is_empty() {
                    return Err("Empty demo library name".into());
                }
                // Duplicate-name validation precedes any metadata mutation.
                // 名称重复验证先于任何元信息变更。
                if self.libraries.iter().any(|(id, library)| {
                    *id != rename.library_id
                        && library.summary["name"].as_str().is_some_and(|name| {
                            name.to_lowercase() == rename.name.trim().to_lowercase()
                        })
                }) {
                    return Err("Duplicate demo library name".into());
                }
                // Only the exact active owner may be renamed.
                // 仅精确活动所有者可重命名。
                let library = self
                    .libraries
                    .get_mut(&rename.library_id)
                    .ok_or("Unknown demo library")?;
                library.active()?;
                library.summary["name"] = json!(rename.name.trim());
                library.summary["description"] = json!(rename.description);
                Ok(library.summary.clone())
            }
            "service_delete_knowledge_library" => {
                // Deletion is deliberately available even for an unreadable old-format row.
                // 即使旧格式行不可读，也刻意允许删除。
                let id = argument(call, "/libraryId")?;
                if self.libraries.remove(id).is_none() {
                    return Err("Unknown demo library".into());
                }
                self.bindings.retain(|binding| binding["libraryId"] != id);
                Ok(Value::Null)
            }
            "service_list_knowledge_project_bindings" => {
                // The exact physical library owns its grant list.
                // 精确物理知识库拥有其授权列表。
                let id = argument(call, "/libraryId")?;
                self.libraries
                    .get(id)
                    .ok_or("Unknown demo library")?
                    .active()?;
                Ok(json!(
                    self.bindings
                        .iter()
                        .filter(|binding| binding["libraryId"] == id)
                        .collect::<Vec<_>>()
                ))
            }
            "service_list_project_knowledge_bindings" => {
                // The exact host project owns this authorized subset.
                // 精确宿主项目拥有此授权子集。
                let id = argument(call, "/projectId")?;
                if !self.projects.contains(id) {
                    return Err("Unknown demo project".into());
                }
                Ok(json!(
                    self.bindings
                        .iter()
                        .filter(|binding| binding["projectId"] == id
                            && binding["libraryId"]
                                .as_str()
                                .is_some_and(|library| self.authorize(id, library).is_ok()))
                        .collect::<Vec<_>>()
                ))
            }
            "service_set_knowledge_project_binding" => {
                // Revocation removes the exact row and never guesses a replacement project.
                // 撤销移除精确行，绝不猜测替代项目。
                let binding: KnowledgeProjectBindingRequest = request(call)?;
                self.libraries
                    .get(&binding.library_id)
                    .ok_or("Unknown demo library")?
                    .active()?;
                if !self.projects.contains(&binding.project_id) {
                    return Err("Unknown demo project".into());
                }
                self.bindings.retain(|row| {
                    row["libraryId"] != binding.library_id || row["projectId"] != binding.project_id
                });
                if binding.authorized {
                    self.bindings.push(json!({"libraryId":binding.library_id,"projectId":binding.project_id,"defaultEnabled":binding.default_enabled}));
                }
                Ok(Value::Null)
            }
            "service_set_knowledge_index_mode" => {
                // Required route membership is validated before starting the durable operation.
                // 必要路由成员关系在开始持久操作前验证。
                let mode: KnowledgeIndexModeRequestPayload = request(call)?;
                // Frozen response shape belongs solely to this new explicit request.
                // 冻结响应形状仅属于此新明确请求。
                let mut settings = Self::settings(mode.settings)?;
                // Stable operation identity survives every subsequent observation.
                // 稳定操作身份跨每个后续观察保留。
                let operation = self.id("operation");
                // The exact library owns both settings and operation.
                // 精确知识库同时拥有设置及操作。
                let library = self
                    .libraries
                    .get_mut(&mode.library_id)
                    .ok_or("Unknown demo library")?;
                library.active()?;
                if library
                    .operation
                    .as_ref()
                    .is_some_and(|job| matches!(job["state"].as_str(), Some("queued" | "running")))
                {
                    return Err("A synthetic knowledge operation is already active".into());
                }
                if settings["embedding"].is_null() {
                    settings["embedding"] = library.summary["settings"]["embedding"].clone();
                }
                library.summary["settings"] = settings;
                library.start(operation, "index_mode")
            }
            "service_rebuild_knowledge_index" | "service_repair_knowledge_index" => {
                // Both current maintenance methods rebuild derived state through one durable job.
                // 两个当前维护方法均经一个持久任务重建派生状态。
                let id = argument(call, "/libraryId")?;
                // Identity allocation is independent of mutable library access.
                // 身份配置独立于可变知识库访问。
                let operation = self.id("operation");
                // Unknown or unreadable owners reject before work begins.
                // 未知或不可读所有者在工作开始前拒绝。
                let library = self.libraries.get_mut(id).ok_or("Unknown demo library")?;
                library.active()?;
                library.start(operation, "rebuild")
            }
            "service_get_knowledge_operation" => {
                // Latest discovery and exact stable-ID lookup are explicitly distinct request cases.
                // 最新发现及精确稳定身份查询为明确不同请求分支。
                let query: KnowledgeOperationRequest = request(call)?;
                // Only this requested owner may advance synthetic work.
                // 仅此请求所有者可推进合成工作。
                let library = self
                    .libraries
                    .get_mut(&query.library_id)
                    .ok_or("Unknown demo library")?;
                library.active()?;
                if query.operation_id.as_ref().is_some_and(|id| {
                    library
                        .operation
                        .as_ref()
                        .is_none_or(|job| job["id"] != *id)
                }) {
                    return Ok(Value::Null);
                }
                library.observe();
                Ok(library.operation.clone().unwrap_or(Value::Null))
            }
            "service_cancel_knowledge_operation" => {
                // A stale operation ID cannot cancel the current operation.
                // 过期操作身份不能取消当前操作。
                let cancel: KnowledgeOperationCancelRequest = request(call)?;
                // Cancellation preserves the same durable receipt and content history.
                // 取消保留相同持久回执及内容历史。
                let library = self
                    .libraries
                    .get_mut(&cancel.library_id)
                    .ok_or("Unknown demo library")?;
                library.active()?;
                if let Some(job) = library.operation.as_mut()
                    && job["id"] == cancel.operation_id
                    && matches!(job["state"].as_str(), Some("queued" | "running"))
                {
                    job["state"] = json!("cancelled");
                    job["finishedAtMs"] = json!(3);
                    library.vectors["paused"] = json!(true);
                    return Ok(json!(true));
                }
                Ok(json!(false))
            }
            "service_get_knowledge_index_integrity" => {
                // Physical integrity is simplified but remains independent of historical content.
                // 物理完整性经过简化，但仍独立于历史内容。
                let library = self
                    .libraries
                    .get(argument(call, "/libraryId")?)
                    .ok_or("Unknown demo library")?;
                library.active()?;
                // Disabled FTS is a valid state, not an empty fabricated search index.
                // 禁用全文为有效状态，绝非伪造空检索索引。
                let enabled = matches!(
                    library.summary["settings"]["indexMode"].as_str(),
                    Some("bm25" | "hybrid")
                );
                Ok(
                    json!({"indexMode":library.summary["settings"]["indexMode"],"documentCount":library.summary["documentCount"],"chunkCount":library.summary["chunkCount"],"ftsPresent":enabled,"indexedChunkCount":if enabled{library.summary["chunkCount"].clone()}else{json!(0)},"mismatchedRows":0,"headChangeSeq":library.summary["headChangeSeq"],"indexedChangeSeq":library.summary["headChangeSeq"],"dictionaryRevision":0,"indexedDictionaryRevision":0,"healthy":true}),
                )
            }
            "service_get_knowledge_embedding_status" => {
                // Failure pages use their live offset and retain global counts.
                // 失败页使用其实时偏移并保留全局数量。
                let query: KnowledgeEmbeddingStatusRequest = request(call)?;
                // Current desired/active spaces belong only to this exact owner.
                // 当前期望及活动空间仅属于此精确所有者。
                let library = self
                    .libraries
                    .get(&query.library_id)
                    .ok_or("Unknown demo library")?;
                library.active()?;
                // Pagination changes only the failure collection and continuation.
                // 分页仅改变失败集合及续读值。
                let mut status = library.vectors.clone();
                // The full synthetic failure collection is already independent from history items.
                // 完整合成失败集合已独立于历史条目。
                let failures = library.vectors["failures"]
                    .as_array()
                    .ok_or("Invalid demo vector failures")?;
                // End offsets are derived from actual returned rows.
                // 末端偏移从真实返回行派生。
                let end = query
                    .offset
                    .saturating_add(u64::from(query.limit))
                    .min(failures.len() as u64);
                if query.offset > failures.len() as u64 {
                    return Err("Invalid demo failure offset".into());
                }
                status["failures"] = json!(&failures[query.offset as usize..end as usize]);
                status["nextOffset"] = json!((end < failures.len() as u64).then_some(end));
                Ok(status)
            }
            "service_retry_knowledge_embeddings" => {
                // Retry IDs must still be current documents in the requested library.
                // 重试身份必须仍为请求知识库当前文档。
                let retry: KnowledgeEmbeddingRetryRequest = request(call)?;
                // Allocate one stable retry operation.
                // 配置一个稳定重试操作。
                let operation = self.id("operation");
                // Unrelated or removed document IDs reject explicitly.
                // 无关或已移除文档身份明确拒绝。
                let library = self
                    .libraries
                    .get_mut(&retry.library_id)
                    .ok_or("Unknown demo library")?;
                library.active()?;
                if retry.document_ids.iter().any(|id| {
                    !library
                        .documents
                        .iter()
                        .any(|entry| entry["document"]["id"] == *id)
                }) {
                    return Err("Unknown current demo retry document".into());
                }
                if !matches!(
                    library.summary["settings"]["indexMode"].as_str(),
                    Some("vector" | "hybrid")
                ) {
                    return Err("Synthetic library has no embedding configuration".into());
                }
                // Only selected failed work changes state; pending/ready work remains untouched.
                // 仅选定失败工作改变状态；待处理及就绪工作保持不变。
                let failures = library.vectors["failures"]
                    .as_array_mut()
                    .expect("synthetic failures");
                // Failure identities are current document IDs from the status response.
                // 失败身份为状态响应中的当前文档身份。
                let count = failures
                    .iter()
                    .filter(|failure| {
                        retry.document_ids.is_empty()
                            || retry
                                .document_ids
                                .iter()
                                .any(|id| failure["documentId"] == *id)
                    })
                    .count() as u64;
                failures.retain(|failure| {
                    !retry.document_ids.is_empty()
                        && !retry
                            .document_ids
                            .iter()
                            .any(|id| failure["documentId"] == *id)
                });
                library.vectors["pending"] = json!(
                    library.vectors["pending"]
                        .as_u64()
                        .expect("synthetic pending")
                        + count
                );
                library.vectors["failed"] = json!(
                    library.vectors["failed"]
                        .as_u64()
                        .expect("synthetic failed")
                        - count
                );
                library.start(operation, "retry")
            }
            "service_list_knowledge_custom_words" => {
                let library = self
                    .libraries
                    .get(argument(call, "/libraryId")?)
                    .ok_or("Unknown demo library")?;
                library.active()?;
                Ok(json!(library.words))
            }
            "service_upsert_knowledge_custom_word" => {
                // Only this single term changes; concurrent sibling terms remain intact.
                // 仅此单个词语变化；并发同级词语保持完整。
                let word: KnowledgeCustomWordRequest = request(call)?;
                if word.word.trim().is_empty() || word.weight == 0 {
                    return Err("Invalid demo dictionary term".into());
                }
                // The dictionary belongs to the exact active physical library.
                // 词典属于精确活动物理知识库。
                let library = self
                    .libraries
                    .get_mut(&word.library_id)
                    .ok_or("Unknown demo library")?;
                library.active()?;
                library.words.retain(|row| row["word"] != word.word);
                library
                    .words
                    .push(json!({"word":word.word,"weight":word.weight}));
                Ok(json!(library.words))
            }
            "service_remove_knowledge_custom_word" => {
                let library = self
                    .libraries
                    .get_mut(argument(call, "/libraryId")?)
                    .ok_or("Unknown demo library")?;
                library.active()?;
                let word = argument(call, "/word")?;
                library.words.retain(|row| row["word"] != word);
                Ok(json!(library.words))
            }
            "service_list_knowledge_library_documents" => self.list(request(call)?, false),
            "service_list_knowledge_library_folders" => self.list(request(call)?, true),
            "service_list_knowledge_documents" => {
                // Project authorization is live before using the same current content reader.
                // 使用相同当前内容阅读器前，项目授权为实时。
                let query: KnowledgeDocumentListRequest = request(call)?;
                self.authorize(&query.project_id, &query.library_id)?;
                self.list(
                    KnowledgeLibraryDocumentListRequest {
                        library_id: query.library_id,
                        path: query.path,
                        depth: query.depth,
                        cursor: query.cursor,
                        limit: query.limit,
                    },
                    false,
                )
            }
            "service_read_knowledge_library_document" => {
                let query: KnowledgeDocumentReadRequest = request(call)?;
                let library = self
                    .libraries
                    .get(&query.library_id)
                    .ok_or("Unknown demo library")?;
                library.active()?;
                Library::read(&library.documents, &query)
            }
            "service_read_knowledge_document" => {
                let query: KnowledgeProjectDocumentReadRequest = request(call)?;
                self.authorize(&query.project_id, &query.document.library_id)?;
                Library::read(
                    &self.libraries[&query.document.library_id].documents,
                    &query.document,
                )
            }
            "service_search_knowledge_library" => {
                let query: KnowledgeLibrarySearchRequest = request(call)?;
                self.search(&[query.library_id], &query.query, query.limit)
            }
            "service_search_knowledge" => {
                let query: KnowledgeSearchRequest = request(call)?;
                for id in &query.library_ids {
                    self.authorize(&query.project_id, id)?;
                }
                self.search(&query.library_ids, &query.query, query.limit)
            }
            "service_list_knowledge_changes" => {
                // Exclusive sequence boundaries never assume contiguous history numbering.
                // 排他序号边界绝不假定历史编号连续。
                let query: KnowledgeChangeListRequest = request(call)?;
                // Permanent log order remains independent from content restoration.
                // 永久日志顺序保持独立于内容还原。
                let library = self
                    .libraries
                    .get(&query.library_id)
                    .ok_or("Unknown demo library")?;
                library.active()?;
                Ok(json!(
                    library
                        .changes
                        .iter()
                        .rev()
                        .filter(|change| query.before_seq.is_none_or(
                            |before| change.summary["seq"].as_u64().expect("synthetic sequence")
                                < before
                        ))
                        .take(query.limit as usize)
                        .map(|change| &change.summary)
                        .collect::<Vec<_>>()
                ))
            }
            "service_get_knowledge_change" => {
                // Immutable item pages retain exact pre/post row images.
                // 不可变条目页保留精确前后行镜像。
                let query: KnowledgeChangeGetRequest = request(call)?;
                // The immutable sequence is resolved by identity rather than a vector index.
                // 不可变序号按身份解析，而非向量下标。
                let library = self
                    .libraries
                    .get(&query.library_id)
                    .ok_or("Unknown demo library")?;
                library.active()?;
                // Every requested detail must exist in the same permanent log.
                // 每个请求详情必须存在于相同永久日志。
                let change = library
                    .changes
                    .iter()
                    .find(|change| change.summary["seq"] == query.seq)
                    .ok_or("Unknown demo history change")?;
                if query.offset > change.items.len() as u64 {
                    return Err("Invalid demo history offset".into());
                }
                // Next offset is derived solely from the exact immutable collection.
                // 下一偏移仅从精确不可变集合派生。
                let end = query
                    .offset
                    .saturating_add(u64::from(query.limit))
                    .min(change.items.len() as u64);
                Ok(
                    json!({"change":change.summary,"items":&change.items[query.offset as usize..end as usize],"nextOffset":(end<change.items.len()as u64).then_some(end)}),
                )
            }
            "service_read_knowledge_change_document" => {
                // A history body side never falls back to current content.
                // 历史正文侧别绝不兜底为当前内容。
                let query: KnowledgeChangeDocumentReadRequest = request(call)?;
                // The selected library and immutable change are both mandatory.
                // 所选知识库及不可变变更均为必需。
                let library = self
                    .libraries
                    .get(&query.library_id)
                    .ok_or("Unknown demo library")?;
                library.active()?;
                // The item explicitly declares whether this side exists.
                // 条目明确声明此侧是否存在。
                let change = library
                    .changes
                    .iter()
                    .find(|change| change.summary["seq"] == query.seq)
                    .ok_or("Unknown demo history change")?;
                // Only the selected change item's explicitly present side is readable.
                // 仅所选变更条目明确存在侧别可读。
                let side = match query.side {
                    KnowledgeChangeSideRequest::Before => "before",
                    KnowledgeChangeSideRequest::After => "after",
                };
                if !change.items.iter().any(|item| {
                    item["entity"] == "document"
                        && item["entityKey"] == query.document_id
                        && !item[side].is_null()
                }) {
                    return Err("Historical demo document side is absent".into());
                }
                // Before bodies belong to the preceding immutable snapshot; after bodies belong to this snapshot.
                // 前正文属于先前不可变快照；后正文属于此快照。
                let documents = match query.side {
                    KnowledgeChangeSideRequest::After => &change.documents,
                    KnowledgeChangeSideRequest::Before => library
                        .changes
                        .iter()
                        .rev()
                        .find(|change| {
                            change.summary["seq"].as_u64().expect("synthetic sequence") < query.seq
                        })
                        .map(|change| &change.documents)
                        .ok_or("Historical demo document side is absent")?,
                };
                Library::read(
                    documents,
                    &KnowledgeDocumentReadRequest {
                        library_id: query.library_id,
                        document_id: Some(query.document_id),
                        path: None,
                        start_line: query.start_line,
                        end_line: query.end_line,
                    },
                )
            }
            "service_restore_knowledge_library" => self.restore(request(call)?),
            "service_get_session_knowledge_libraries" => {
                self.selection(argument(call, "/conversationId")?)
            }
            "service_set_session_knowledge_libraries" => {
                // Session ownership and every explicit ID are checked before persisting selection.
                // 持久化选择前检查会话归属及每个明确身份。
                let selection: SessionKnowledgeLibrarySelectionRequest = request(call)?;
                if self.sessions.get(&selection.conversation_id) != Some(&selection.project_id)
                    || !matches!(selection.mode.as_str(), "inherit" | "explicit")
                {
                    return Err("Invalid demo session knowledge selection".into());
                }
                for id in &selection.library_ids {
                    self.authorize(&selection.project_id, id)?;
                }
                // Persist only a direct human request, never a read-derived default.
                // 仅持久化直接人工请求，绝非读取派生默认值。
                let id = selection.conversation_id.clone();
                self.selections.insert(id.clone(), selection);
                self.selection(&id)
            }
            "service_export_knowledge_package"
            | "service_inspect_knowledge_package"
            | "service_import_knowledge_package"
            | "service_browse_knowledge_packages" => self.package(call),
            _ => Err("Unknown current knowledge method; legacy methods are unsupported".into()),
        }
    }
}
