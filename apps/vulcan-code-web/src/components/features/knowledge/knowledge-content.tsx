import * as React from "react";
import { Bot, ChevronDown, ChevronRight, FileText, Folder, FolderOpen, RefreshCw, Search } from "lucide-react";
import { PageEmptyState } from "@/components/page";
import { Button } from "@/ui-kit/primitives/button";
import { Input } from "@/ui-kit/primitives/input";
import { StreamPlaceholder } from "@/ui-kit/primitives/stream-placeholder";
import { useI18n, type TranslationKey } from "@/i18n";
import { formatKnowledgeIndexStatus } from "@/lib/knowledge-index-status";
import { listManagedKnowledgeDocuments, searchManagedKnowledgeLibrary } from "@/service/knowledge-base-service";
import type { KnowledgeLibrarySummary, KnowledgeListEntry, KnowledgeSearchHit } from "@/types/knowledge-base";
import { KnowledgeCurrentDocumentReader } from "./knowledge-document-reader";
import { KnowledgeReadFailure } from "./knowledge-feedback";
import { KNOWLEDGE_MODE_COPY } from "./knowledge-mode";
import { fillKnowledgeText } from "./knowledge-visuals";
import { useKnowledgeRead } from "./use-knowledge-read";

// Match-kind labels cover the retrieval vocabulary; unknown kinds keep their raw identifier.
// 匹配种类标签覆盖检索词汇；未知种类保留原始标识。
const MATCH_KINDS: Readonly<Record<string, { label: TranslationKey; tone: string }>> = {
  bm25: { label: "knowledge.content.match.bm25", tone: "info" },
  vector: { label: "knowledge.content.match.vector", tone: "primary" },
};

// The reader target is one exact current document, optionally focused on a search hit's line range.
// 阅读目标为一个精确当前文档，可选聚焦检索命中行范围。
interface KnowledgeReaderTarget {
  // Stable document UUID.
  // 稳定文档 UUID。
  documentId: string;
  // Inclusive body hit range, absent for directory and summary opens.
  // 包含边界的正文命中范围；从目录或摘要打开时省略。
  hit?: { start: number; end: number };
  // Selected search chunk, independent of any body range.
  // 选中的检索切块，与正文行范围独立。
  chunkId?: string;
}

// A directory subtree has one exact physical library, canonical path and document-open callback.
// 目录子树具有一个精确物理库、规范路径及文档打开回调。
interface KnowledgeDirectoryOwner {
  // Physical library UUID.
  // 物理库 UUID。
  libraryId: string;
  // Exact canonical logical directory, with empty string denoting root.
  // 精确规范逻辑目录，空字符串表示根。
  path: string;
  // Currently opened document ID, used only for selection styling.
  // 当前打开文档身份，仅用于选择样式。
  activeId: string | null;
  // Opens only the selected current document ID in the same library.
  // 仅在相同库中打开选定当前文档身份。
  onDocument: (documentId: string) => void;
}

// Renders one bounded folder page and lazily mounts expanded subtrees.
// 渲染一个有界目录页，并惰性挂载已展开子树。
// Parameters: owner supplies exact library/path/reader callback. Returns read-only hierarchical entries and explicit pagination.
// 参数：owner 提供精确库、路径及阅读回调。返回只读层级条目及显式分页。
function KnowledgeDirectoryChildren({ libraryId, path, activeId, onDocument }: KnowledgeDirectoryOwner) {
  // Labels follow the active locale.
  // 标签遵循当前语言。
  const { t } = useI18n();
  // Opaque cursor history preserves exact server-issued continuations.
  // 不透明游标历史保留精确服务签发续读标识。
  const [cursors, setCursors] = React.useState<(string | null)[]>([null]);
  // The current page uses the final explicit cursor only.
  // 当前页仅使用最后一个明确游标。
  const cursor = cursors[cursors.length - 1];
  // The service owns default page size and maximum depth.
  // 默认页大小及最大深度由服务拥有。
  const load = React.useCallback(() => listManagedKnowledgeDocuments({ libraryId, path, cursor, depth: 1 }), [libraryId, path, cursor]);
  // Read generations are scoped by exact library/path/cursor.
  // 读取代次由精确库、路径及游标限定。
  const read = useKnowledgeRead(JSON.stringify([libraryId, path, cursor]), load);
  return <div className="knowledge-tree">
    {read.loading && read.value === null ? <StreamPlaceholder label={t("knowledge.librariesLoading")} /> : null}
    {read.error !== null ? <KnowledgeReadFailure error={read.error} onRetry={() => { if (cursor === null) void read.refresh(); else setCursors([null]); }} /> : null}
    {read.value !== null ? <>
      {read.value.entries.map((entry) => <div key={entry.kind + ":" + entry.path}>
        {entry.kind === "folder" ? <KnowledgeFolderRow libraryId={libraryId} entry={entry} activeId={activeId} onDocument={onDocument} />
          : <button type="button" className="knowledge-tree-row" aria-current={entry.document.id === activeId} onClick={() => onDocument(entry.document.id)}>
            <FileText size={15} aria-hidden="true" />
            <span><span className="knowledge-tree-label"><b>{entry.document.title}</b></span><small className="knowledge-tree-note">{entry.path.slice(entry.path.lastIndexOf("/") + 1)}</small></span>
          </button>}
      </div>)}
      {read.value.entries.length === 0 ? <p className="knowledge-pane-note">{t("knowledge.content.empty")}</p> : null}
      {cursors.length > 1 || read.value.nextCursor !== null ? <div className="knowledge-pager">
        <Button variant="ghost" size="sm" disabled={cursors.length === 1} onClick={() => setCursors((current) => current.slice(0, -1))}>{t("knowledge.page.previous")}</Button>
        <Button variant="ghost" size="sm" disabled={read.value.nextCursor === null} onClick={() => {
          // Capture this page's exact issued cursor before scheduling the state transition.
          // 在调度状态过渡前捕获此页精确签发游标。
          const next = read.value === null ? null : read.value.nextCursor;
          if (next !== null) setCursors((current) => [...current, next]);
        }}>{t("knowledge.page.next")}</Button>
      </div> : null}
    </> : null}
  </div>;
}

// Renders one exact folder; child reads begin only after conscious expansion.
// 渲染一个精确目录；仅在主动展开后开始子项读取。
// Parameters: entry is a proven folder row, libraryId owns it, onDocument retains the same ceiling. Returns a lazy subtree.
// 参数：entry 为已证明目录行，libraryId 拥有该行，onDocument 保持相同上限。返回惰性子树。
function KnowledgeFolderRow({ entry, libraryId, activeId, onDocument }: Omit<KnowledgeDirectoryOwner, "path"> & { entry: Extract<KnowledgeListEntry, { kind: "folder" }> }) {
  // Expansion belongs to this exact folder/head-mounted row.
  // 展开状态属于此精确目录及变更头挂载行。
  const [expanded, setExpanded] = React.useState(false);
  // Open and closed folders use distinct glyphs.
  // 展开及收起目录使用不同图标。
  const FolderIcon = expanded ? FolderOpen : Folder;
  return <>
    <button type="button" aria-expanded={expanded} className="knowledge-tree-row" onClick={() => setExpanded((current) => !current)}>
      {expanded ? <ChevronDown size={14} aria-hidden="true" /> : <ChevronRight size={14} aria-hidden="true" />}<FolderIcon size={15} aria-hidden="true" />
      <span><span className="knowledge-tree-label"><b>{entry.path.slice(entry.path.lastIndexOf("/") + 1)}</b><small>{entry.documentCount}</small></span>{entry.description !== "" ? <small className="knowledge-tree-note" title={entry.description}>{entry.description}</small> : null}</span>
    </button>
    {expanded ? <div className="knowledge-tree-children"><KnowledgeDirectoryChildren libraryId={libraryId} path={entry.path} activeId={activeId} onDocument={onDocument} /></div> : null}
  </>;
}

// Splits a snippet around the submitted query terms; returns text and highlighted fragments.
// 按已提交查询词拆分片段；返回文本及高亮片段。
// Parameters: content is the exact hit text; query is the submitted human query. Matching is case-insensitive and display-only.
// 参数：content 为精确命中文本；query 为已提交人工查询。匹配不区分大小写，仅用于展示。
function highlightSnippet(content: string, query: string): React.ReactNode[] {
  // Whitespace-separated terms cover multi-word queries; a CJK query stays one term.
  // 空白分隔词覆盖多词查询；CJK 查询保持为一个词。
  const terms = query.split(/\s+/).filter((term) => term !== "").map((term) => term.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));
  if (terms.length === 0) return [content];
  // Capturing split keeps every matched fragment at an odd index.
  // 捕获式拆分使每个匹配片段位于奇数下标。
  return content.split(new RegExp("(" + terms.join("|") + ")", "gi")).map((part, index) => index % 2 === 1 ? <mark key={index}>{part}</mark> : part);
}

// Search result props keep one explicit human query, its owner and the shared reader target.
// 检索结果属性保持一个明确人工查询、其所有者及共享阅读目标。
interface KnowledgeSearchResultsProps {
  // Exact library selector.
  // 精确库选择器。
  libraryId: string;
  // Submitted query text.
  // 已提交查询文本。
  query: string;
  // Currently opened target for selection styling.
  // 当前打开目标，用于选择样式。
  active: KnowledgeReaderTarget | null;
  // Opens the matched document, focusing the range for body hits.
  // 打开命中文档，正文命中时聚焦对应行范围。
  onOpen: (target: KnowledgeReaderTarget) => void;
  // Returns to the directory tree without issuing another query.
  // 返回目录树，不发起其他查询。
  onClose: () => void;
}

// Reads one explicit human query by the library's declared mode.
// 按库声明模式读取一个显式人工查询。
// Parameters: props fixes the request and reader callbacks. Returns partial-status-aware results.
// 参数：props 固定请求及阅读回调。返回感知部分状态的结果。
function KnowledgeSearchResults({ libraryId, query, active, onOpen, onClose }: KnowledgeSearchResultsProps) {
  // Shared labels never replace service diagnostics.
  // 共享标签绝不替换服务诊断。
  const { t } = useI18n();
  // The server executes only this exact query/library selector.
  // 服务仅执行此精确查询及库选择器。
  const load = React.useCallback(() => searchManagedKnowledgeLibrary({ libraryId, query }), [libraryId, query]);
  // A new submitted query revokes the previous result projection.
  // 新提交查询撤销先前结果投影。
  const read = useKnowledgeRead(JSON.stringify([libraryId, query]), load);
  // Selection is by chunk identity because summaries have no reading range.
  // 摘要没有正文读取范围，因此按切块身份标记选中状态。
  const isActive = (hit: KnowledgeSearchHit) => active?.documentId === hit.documentId && active.chunkId === hit.chunkId;
  return <>
    <div className="knowledge-pane-head"><span>{read.value === null ? t("knowledge.content.search") : fillKnowledgeText(t("knowledge.content.results"), { count: read.value.hits.length })}</span><Button variant="ghost" size="sm" onClick={onClose}>{t("knowledge.content.backToContents")}</Button></div>
    <div className="knowledge-pane-body">
      {read.loading ? <StreamPlaceholder label={t("knowledge.content.searching")} /> : null}
      {read.error !== null ? <KnowledgeReadFailure error={read.error} onRetry={read.refresh} /> : null}
      {read.value !== null ? <>
        {read.value.libraries.map((status) => <p className="knowledge-search-status" key={status.libraryId}>{t(KNOWLEDGE_MODE_COPY[status.indexMode].label)} · {fillKnowledgeText(t("knowledge.content.capabilities"), { fts: formatKnowledgeIndexStatus(status.ftsStatus, t), pending: status.embeddings.pending, failed: status.embeddings.failed })}</p>)}
        {read.value.failures.map((failure, index) => <pre role="alert" className="knowledge-error" key={index}>{failure.error}</pre>)}
        {read.value.hits.map((hit) => {
          // Summary chunks use zero as a sentinel; the reader defaults to page one for them.
          // 摘要切块用零作为标记；阅读器为它们打开正文首页。
          const range = hit.kind === "summary" ? undefined : { start: hit.startLine, end: hit.endLine };
          return <button type="button" className="knowledge-hit" aria-current={isActive(hit)} title={String(hit.score)} key={JSON.stringify([hit.chunkId, hit.contentSha256, hit.revision])} onClick={() => onOpen({ documentId: hit.documentId, chunkId: hit.chunkId, hit: range })}>
            <span className="knowledge-hit-title"><span>{hit.title}</span><small>{range === undefined ? t("knowledge.reader.summary") : fillKnowledgeText(t("knowledge.content.lines"), range)}</small></span>
            <span className="knowledge-hit-path">{hit.headingPath !== "" ? hit.path + " › " + hit.headingPath : hit.path}</span>
            <span className="knowledge-hit-snippet">{highlightSnippet(hit.content, query)}</span>
            <span className="knowledge-hit-kinds">{hit.matchKinds.map((kind) => <span className="knowledge-badge" data-tone={Object.hasOwn(MATCH_KINDS, kind) ? MATCH_KINDS[kind].tone : "neutral"} key={kind}>{Object.hasOwn(MATCH_KINDS, kind) ? t(MATCH_KINDS[kind].label) : kind}</span>)}</span>
          </button>;
        })}
        {read.value.hits.length === 0 ? <p className="knowledge-pane-note">{t("knowledge.content.noHits")}</p> : null}
      </> : null}
    </div>
  </>;
}

// Renders read-only directory discovery, current document reading and declared-mode search.
// 渲染只读目录发现、当前文档阅读及声明模式检索。
// Parameters: library is the current authoritative active summary. Returns no content editor or human ingest action.
// 参数：library 为当前权威活动摘要。返回值不含内容编辑器或人工搬运动作。
export function KnowledgeContent({ library }: { library: KnowledgeLibrarySummary }) {
  // Every label follows the application's locale.
  // 每个标签遵循应用语言。
  const { t } = useI18n();
  // The exact selected document owns the inline reader pane.
  // 精确选定文档拥有内嵌阅读窗格。
  const [target, setTarget] = React.useState<KnowledgeReaderTarget | null>(null);
  // Query editing alone never starts inference.
  // 仅编辑查询绝不开始推理。
  const [query, setQuery] = React.useState("");
  // A submitted query includes a user-triggered visit so repeated searches can explicitly refresh.
  // 已提交查询包含用户触发访问序号，使重复查询可显式刷新。
  const [search, setSearch] = React.useState<{ query: string; visit: number; scope: string } | null>(null);
  // An explicit tree refresh remounts the root page without touching search state.
  // 明确目录刷新重新挂载根页，不影响检索状态。
  const [treeVisit, setTreeVisit] = React.useState(0);
  // Policy/content changes revoke old query projections without automatically issuing paid inference.
  // 策略及内容变化撤销旧查询投影，不自动发起付费推理。
  const searchScope = JSON.stringify([library.id, library.headChangeSeq, library.settings]);
  // Browse-only libraries never expose a query field.
  // 仅浏览库绝不显示查询字段。
  const searchable = library.settings !== null && library.settings.indexMode !== "browse";
  // Results replace the tree only while their scope is still current.
  // 仅当结果范围仍为当前时，结果才替代目录树。
  const results = searchable && search !== null && search.scope === searchScope ? search : null;
  return <>
    <p className="knowledge-hint"><Bot size={14} aria-hidden="true" />{t("knowledge.content.readOnly")}</p>
    {searchable && library.settings !== null ? <>
      <form className="knowledge-search" onSubmit={(event) => { event.preventDefault(); if (query.trim()) setSearch((current) => ({ query: query.trim(), visit: current === null ? 1 : current.visit + 1, scope: searchScope })); }}>
        <div className="knowledge-search-field"><Search size={14} aria-hidden="true" /><Input aria-label={t("knowledge.content.search")} placeholder={t("knowledge.content.search")} value={query} onChange={(event) => setQuery(event.target.value)} /></div>
        <Button disabled={!query.trim()} type="submit">{t("knowledge.content.search")}</Button>
      </form>
      <p className="knowledge-search-status">{t(KNOWLEDGE_MODE_COPY[library.settings.indexMode].description)}</p>
    </> : null}
    <div className="knowledge-browser">
      <div className="knowledge-browser-pane">
        {results !== null ? <KnowledgeSearchResults key={results.visit} libraryId={library.id} query={results.query} active={target} onOpen={setTarget} onClose={() => setSearch(null)} /> : <>
          <div className="knowledge-pane-head"><span>{t("knowledge.content.contents")}</span><Button variant="ghost" size="icon-sm" aria-label={t("knowledge.refresh")} title={t("knowledge.refresh")} onClick={() => setTreeVisit((current) => current + 1)}><RefreshCw size={13} /></Button></div>
          <div className="knowledge-pane-body"><KnowledgeDirectoryChildren key={JSON.stringify([library.id, library.headChangeSeq, treeVisit])} libraryId={library.id} path="" activeId={target === null ? null : target.documentId} onDocument={(documentId) => setTarget({ documentId })} /></div>
        </>}
      </div>
      <section className="knowledge-reader" aria-label={t("knowledge.view")}>
        {target !== null ? <KnowledgeCurrentDocumentReader key={JSON.stringify([target.documentId, library.headChangeSeq])} libraryId={library.id} documentId={target.documentId} initialLine={target.hit === undefined ? 1 : target.hit.start} highlight={target.hit} />
          : <div className="knowledge-reader-placeholder"><PageEmptyState icon={<FileText />} title={t("knowledge.content.readerEmpty")} /></div>}
      </section>
    </div>
  </>;
}
