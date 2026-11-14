//! Read-only hierarchical directories, exact body windows and explicit search.
//! 只读层级目录、精确正文窗口及明确检索。

use super::*;

/// Browses one bounded logical folder and follows only service-issued cursors.
/// 浏览一个有界逻辑目录，且仅遵循服务签发游标。
/// Parameters: app owns the physical library and logical directory. Returns folder/document navigation without editors.
/// 参数：app 拥有物理知识库及逻辑目录。返回不含编辑器的目录及文档导航。
pub(crate) fn documents(app: &App) -> Vec<Row> {
    // The shared header enforces the exact-format lifecycle boundary.
    // 共享标题执行精确格式生命周期边界。
    let mut rows = header(app, PageId::KnowledgeDocuments);
    if active_library(app).is_none() {
        return rows;
    }
    rows.push(note(tr!("knowledge.content.readOnly"), Tone::Muted));
    rows.push(info(
        tr!("toolDomain.knowledge.directory"),
        app.knowledge.folder.clone(),
        Tone::Normal,
    ));
    if !app.knowledge.folder.is_empty() {
        // A canonical path with no slash is a root child; its parent is the declared empty root.
        // 没有斜杠的规范路径为根子项；其父为声明空根。
        let parent = match app.knowledge.folder.rsplit_once('/') {
            Some((parent, _)) => parent,
            None => "",
        };
        rows.push(button(
            tr!("projectFolderPicker.parent"),
            UiAction::Knowledge(KnowledgeAction::Folder(parent.into())),
            Tone::Normal,
        ));
    }
    if !ready(app, Slot::KnowledgeDocuments, &mut rows) {
        return rows;
    }
    for entry in a(app.slots.value(Slot::KnowledgeDocuments), "/entries") {
        // The producer's explicit kind owns the folder or document member.
        // 产出者明确种类拥有目录或文档成员。
        match entry.get("kind").and_then(Value::as_str) {
            Some("folder") => {
                // Path and description come from this exact logical folder row.
                // 路径及说明来自此精确逻辑目录行。
                let path = s(entry, "/path");
                rows.push(
                    item(&path, path.clone(), s(entry, "/description"))
                        .badge(n(entry, "/documentCount").to_string(), Tone::Muted)
                        .actions(vec![action(
                            tr!("knowledge.view"),
                            UiAction::Knowledge(KnowledgeAction::Folder(path)),
                            Tone::Accent,
                        )]),
                );
            }
            Some("document") => {
                // The stable document ID survives moves and is the sole reader selector.
                // 稳定文档身份跨移动保留，为唯一阅读选择器。
                let Some(document) = entry.get("document").filter(|value| value.is_object()) else {
                    rows.push(note("ERR_KNOWLEDGE_DOCUMENT_SUMMARY".into(), Tone::Bad));
                    continue;
                };
                // All displayed metadata belongs to this exact document.
                // 全部展示元信息属于此精确文档。
                let id = s(document, "/id");
                rows.push(
                    item(&id, s(document, "/title"), s(document, "/path"))
                        .lines(vec![s(document, "/summary")])
                        .actions(vec![action(
                            tr!("knowledge.view"),
                            UiAction::Knowledge(KnowledgeAction::Document(
                                KnowledgeDocumentAddress::Current(id),
                            )),
                            Tone::Accent,
                        )]),
                );
            }
            _ => rows.push(note("ERR_KNOWLEDGE_LIST_ENTRY_KIND".into(), Tone::Bad)),
        }
    }
    if a(app.slots.value(Slot::KnowledgeDocuments), "/entries").is_empty() {
        rows.push(note(tr!("knowledge.content.empty"), Tone::Muted));
    }
    if app.knowledge.cursors.len() > 1 {
        rows.push(button(
            tr!("knowledge.page.previous"),
            UiAction::Knowledge(KnowledgeAction::DirectoryBack),
            Tone::Normal,
        ));
    }
    if let Some(cursor) = app
        .slots
        .at(Slot::KnowledgeDocuments, "/nextCursor")
        .as_str()
    {
        rows.push(button(
            tr!("knowledge.page.next"),
            UiAction::Knowledge(KnowledgeAction::DirectoryNext(cursor.into())),
            Tone::Normal,
        ));
    }
    rows
}

/// Reads current or immutable historical content through its single declared endpoint.
/// 通过其单个声明端点读取当前或不可变历史内容。
/// Parameters: app owns document address and service line starts. Returns exact whitespace-preserving text and bounded pagination.
/// 参数：app 拥有文档地址及服务行首。返回精确保留空白文本及有界分页。
pub(crate) fn document(app: &App) -> Vec<Row> {
    // Historical reading retains its exact side without a current-body fallback.
    // 历史阅读保留精确侧别，不提供当前正文兜底。
    let mut rows = header(app, PageId::KnowledgeDocument);
    if active_library(app).is_none() {
        return rows;
    }
    rows.push(button(
        tr!("knowledge.tabs.content"),
        UiAction::GoPage(PageId::KnowledgeDocuments),
        Tone::Normal,
    ));
    if let Some(KnowledgeDocumentAddress::Historical { seq, side, .. }) = &app.knowledge.document {
        rows.push(info(
            tr!("knowledge.history.change", seq = seq),
            match side {
                KnowledgeChangeSideRequest::Before => tr!("knowledge.history.before"),
                KnowledgeChangeSideRequest::After => tr!("knowledge.history.after"),
            },
            Tone::Accent,
        ));
        rows.push(button(
            tr!("knowledge.history.compare"),
            UiAction::Knowledge(KnowledgeAction::Change(*seq)),
            Tone::Normal,
        ));
    }
    if !ready(app, Slot::KnowledgeDocument, &mut rows) {
        return rows;
    }
    // The exact content response owns both its summary and body range.
    // 精确内容响应同时拥有其摘要及正文范围。
    let read = app.slots.value(Slot::KnowledgeDocument);
    rows.push(section(s(read, "/document/title")));
    rows.push(note(s(read, "/document/path"), Tone::Muted));
    rows.push(note(s(read, "/document/summary"), Tone::Normal));
    rows.push(info(
        tr!(
            "knowledge.reader.revision",
            revision = n(read, "/document/revision")
        ),
        tr!(
            "knowledge.reader.range",
            start = n(read, "/startLine"),
            end = n(read, "/endLine"),
            total = n(read, "/document/lineCount")
        ),
        Tone::Muted,
    ));
    rows.push(
        item("document-metadata", tr!("knowledge.history.metadata"), "")
            .lines(details(v(read, "/document"))),
    );
    // Missing content is an explicit protocol error, never another corpus or document.
    // 正文缺失为明确协议错误，绝非另一语料或文档。
    match read.get("content").and_then(Value::as_str) {
        Some(content) => rows.push(Row::new(
            RowKind::Document {
                content: content.into(),
            },
            tr!("toolDomain.knowledge.content"),
        )),
        None => rows.push(note("ERR_KNOWLEDGE_DOCUMENT_CONTENT".into(), Tone::Bad)),
    }
    if app.knowledge.starts.len() > 1 {
        rows.push(button(
            tr!("knowledge.page.previous"),
            UiAction::Knowledge(KnowledgeAction::BodyBack),
            Tone::Normal,
        ));
    }
    if let Some(start) = read
        .get("nextLine")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
    {
        rows.push(button(
            tr!("knowledge.page.next"),
            UiAction::Knowledge(KnowledgeAction::BodyNext(start)),
            Tone::Normal,
        ));
    }
    rows
}

/// Searches only after an explicit human query, using the selected library's own mode.
/// 仅在明确人工查询后检索，使用所选知识库自身模式。
/// Parameters: app owns the submitted query and current results. Returns citations, backlog and explicit partial failures.
/// 参数：app 拥有提交查询及当前结果。返回引用、积压及明确部分失败。
pub(crate) fn search(app: &App) -> Vec<Row> {
    // A browsing-only library never exposes a query action.
    // 仅浏览知识库绝不暴露查询动作。
    let mut rows = header(app, PageId::KnowledgeSearch);
    let Some(library) = active_library(app) else {
        return rows;
    };
    if library
        .pointer("/settings/indexMode")
        .and_then(Value::as_str)
        == Some("browse")
    {
        rows.push(note(tr!("knowledge.mode.browse.description"), Tone::Muted));
        return rows;
    }
    rows.push(button(
        tr!("knowledge.content.search"),
        prompt(
            tr!("knowledge.content.search"),
            "".into(),
            app.knowledge.query.clone(),
            PromptThen::KnowledgeSearch,
        ),
        Tone::Accent,
    ));
    rows.push(note(
        crate::app::knowledge_state::mode_description(&s(library, "/settings/indexMode")),
        Tone::Muted,
    ));
    if app.knowledge.query.is_empty() {
        return rows;
    }
    rows.push(info(
        tr!("knowledge.content.search"),
        app.knowledge.query.clone(),
        Tone::Normal,
    ));
    if !ready(app, Slot::KnowledgeSearch, &mut rows) {
        return rows;
    }
    for status in a(app.slots.value(Slot::KnowledgeSearch), "/libraries") {
        rows.push(info(
            mode_label(&s(status, "/indexMode")),
            tr!(
                "knowledge.content.capabilities",
                fts = crate::app::knowledge_state::fts_label(&s(status, "/ftsStatus")),
                pending = n(status, "/embeddings/pending"),
                failed = n(status, "/embeddings/failed")
            ),
            Tone::Muted,
        ));
    }
    for failure in a(app.slots.value(Slot::KnowledgeSearch), "/failures") {
        rows.push(note(s(failure, "/error"), Tone::Bad));
    }
    for hit in a(app.slots.value(Slot::KnowledgeSearch), "/hits") {
        // Hit identity and exact citation range belong to the current server result.
        // 命中身份及精确引用范围属于当前服务结果。
        let id = s(hit, "/documentId");
        // Summary chunks have no body citation and open the existing first-page reader.
        // 摘要切块没有正文引用范围，沿用首页阅读入口。
        let location = if s(hit, "/kind") == "summary" {
            format!("{} · {}", s(hit, "/path"), tr!("knowledge.reader.summary"))
        } else {
            format!(
                "{}:{}–{}",
                s(hit, "/path"),
                n(hit, "/startLine"),
                n(hit, "/endLine")
            )
        };
        rows.push(
            item(&s(hit, "/chunkId"), s(hit, "/title"), location)
                .lines(vec![s(hit, "/content")])
                .actions(vec![action(
                    tr!("knowledge.view"),
                    UiAction::Knowledge(KnowledgeAction::Document(
                        KnowledgeDocumentAddress::Current(id),
                    )),
                    Tone::Accent,
                )]),
        );
    }
    if a(app.slots.value(Slot::KnowledgeSearch), "/hits").is_empty() {
        rows.push(note(tr!("knowledge.content.noHits"), Tone::Muted));
    }
    rows
}
