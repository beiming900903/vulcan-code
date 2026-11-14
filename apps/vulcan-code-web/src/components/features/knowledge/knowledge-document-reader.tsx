import * as React from "react";
import { Hash } from "lucide-react";
import { MarkdownMessage } from "@/components/conversation/conversation-markdown";
import { Button } from "@/ui-kit/primitives/button";
import { StreamPlaceholder } from "@/ui-kit/primitives/stream-placeholder";
import { useI18n, type TranslationKey } from "@/i18n";
import { formatDateTime } from "@/lib/display-format";
import { readKnowledgeChangeDocument, readManagedKnowledgeDocument } from "@/service/knowledge-base-service";
import type { KnowledgeChangeSide, KnowledgeDocumentContent, KnowledgeDocumentSummary } from "@/types/knowledge-base";
import { KnowledgeReadFailure } from "./knowledge-feedback";
import { fillKnowledgeText } from "./knowledge-visuals";
import { useKnowledgeRead } from "./use-knowledge-read";

// Summary freshness labels and tones are fixed per stored state; unknown states stay neutral.
// 摘要新鲜度标签及色调按存储状态固定；未知状态保持中性。
const SUMMARY_STATES: Readonly<Record<string, { label: TranslationKey; tone: string }>> = {
  provided: { label: "knowledge.reader.summaryState.provided", tone: "success" },
  pending: { label: "knowledge.reader.summaryState.pending", tone: "warning" },
  stale: { label: "knowledge.reader.summaryState.stale", tone: "warning" },
};

// Vector policy labels follow the closed embed-policy vocabulary.
// 向量策略标签遵循封闭收录策略词汇。
const EMBED_POLICIES: Readonly<Record<KnowledgeDocumentSummary["embedPolicy"], { label: TranslationKey; tone: string }>> = {
  summary: { label: "knowledge.reader.embed.summary", tone: "primary" },
  full: { label: "knowledge.reader.embed.full", tone: "primary" },
  none: { label: "knowledge.reader.embed.none", tone: "neutral" },
};

// Exact physical library/document owner for current read-only content.
// 当前只读内容的精确物理库及文档所有者。
interface KnowledgeDocumentOwner {
  // Owning library UUID.
  // 所属库 UUID。
  libraryId: string;
  // Stable document UUID, never a display title or guessed path.
  // 稳定文档 UUID，绝非展示标题或推测路径。
  documentId: string;
}

// Optional presentation for a current read opened from a search hit.
// 从检索命中打开当前读取时的可选展示。
interface KnowledgeCurrentReaderOptions extends KnowledgeDocumentOwner {
  // First requested line; the service still owns the returned window.
  // 首个请求行；返回窗口仍由服务拥有。
  initialLine?: number;
  // Inclusive hit range to emphasize when it is inside the returned window.
  // 位于返回窗口内时强调的包含边界命中范围。
  highlight?: { start: number; end: number };
}

// Reads the current document through its exact owner; returns a shared bounded reader.
// 通过精确所有者读取当前文档；返回共享有界阅读器。
// Parameters: libraryId/documentId identify one current document; initialLine/highlight focus a search hit. No content can be edited here.
// 参数：libraryId 及 documentId 标识一个当前文档；initialLine 及 highlight 聚焦检索命中。此处不能编辑内容。
export function KnowledgeCurrentDocumentReader({ libraryId, documentId, initialLine = 1, highlight }: KnowledgeCurrentReaderOptions) {
  // The loader uses only the declared document ID selector.
  // 加载器仅使用声明文档身份选择器。
  const load = React.useCallback((startLine: number) => readManagedKnowledgeDocument({ libraryId, documentId, startLine }), [libraryId, documentId]);
  // Owner changes remount line pagination before another document can be requested.
  // 所有者变化在请求另一文档之前重新挂载行分页。
  const scope = JSON.stringify([libraryId, documentId]);
  return <KnowledgeDocumentWindow key={JSON.stringify([scope, initialLine])} scope={scope} loadWindow={load} initialLine={initialLine} highlight={highlight} showHeader />;
}

// An immutable history side extends the exact physical owner without a current-content alternative.
// 不可变历史侧别扩展精确物理所有者，不提供当前内容替代。
interface KnowledgeHistoricalOwner extends KnowledgeDocumentOwner {
  // Exact immutable change sequence.
  // 精确不可变变更序号。
  seq: number;
  // Exact before or after image.
  // 精确前镜像或后镜像。
  side: KnowledgeChangeSide;
}

// Reads the exact historical side; returns the same line-window interaction as current content.
// 读取精确历史侧别；返回与当前内容相同的行窗口交互。
// Parameters: owner supplies library/document/change/side IDs. Missing images remain explicit service errors.
// 参数：owner 提供库、文档、变更及侧别身份。缺失镜像保持明确服务错误。
export function KnowledgeHistoricalDocumentReader({ libraryId, documentId, seq, side }: KnowledgeHistoricalOwner) {
  // Every historical call goes only to the history-body endpoint.
  // 每次历史调用仅访问历史正文端点。
  const load = React.useCallback((startLine: number) => readKnowledgeChangeDocument({ libraryId, documentId, seq, side, startLine }), [libraryId, documentId, seq, side]);
  // Each immutable side owns an independent pagination lifetime.
  // 每个不可变侧别拥有独立分页寿命。
  const scope = JSON.stringify([libraryId, documentId, seq, side]);
  return <KnowledgeDocumentWindow key={scope} scope={scope} loadWindow={load} initialLine={1} showHeader={false} />;
}

// A body window consumes a single declared current/history loader.
// 正文窗口消费单个声明的当前或历史加载器。
interface KnowledgeDocumentWindowProps {
  // Immutable owner identity for this mounted read surface.
  // 此挂载读取区域的不可变所有者身份。
  scope: string;
  // Reads an inclusive first line; the service owns the maximum window size.
  // 读取包含边界首行；最大窗口大小由服务拥有。
  loadWindow: (startLine: number) => Promise<KnowledgeDocumentContent>;
  // First explicit start; a later "start of document" action resets to line one.
  // 首个明确首行；之后的“回到开头”动作重置为第一行。
  initialLine: number;
  // Optional inclusive line range to emphasize.
  // 可选的包含边界强调行范围。
  highlight?: { start: number; end: number };
  // History comparisons show metadata in their field table instead of this header.
  // 历史比较在字段表中显示元数据，而非此页眉。
  showHeader: boolean;
}

// Renders a document's identity, freshness, vector policy, keywords, summary and provenance.
// 渲染文档身份、新鲜度、向量策略、关键词、摘要及来源。
// Parameters: document is one exact returned image; t/locale localize labels and times. Returns a read-only header.
// 参数：document 为一个精确返回镜像；t 及 locale 本地化标签及时间。返回只读页眉。
function KnowledgeDocumentHeader({ document, t, locale }: { document: KnowledgeDocumentSummary; t: (key: TranslationKey) => string; locale: string }) {
  // Unknown freshness states stay visible as their raw identifier in a neutral badge.
  // 未知新鲜度状态以原始标识显示在中性徽章中。
  const summary = Object.hasOwn(SUMMARY_STATES, document.summaryState) ? SUMMARY_STATES[document.summaryState] : null;
  // The embed policy is a closed union and always has a label.
  // 收录策略为封闭联合，始终有标签。
  const embed = EMBED_POLICIES[document.embedPolicy];
  return <header className="knowledge-reader-head">
    <div className="knowledge-reader-title"><strong>{document.title}</strong><span className="knowledge-reader-path">{document.path}</span></div>
    <div className="knowledge-chips">
      <span className="knowledge-badge" data-tone="neutral"><Hash size={11} aria-hidden="true" />{fillKnowledgeText(t("knowledge.reader.revision"), { revision: document.revision })}</span>
      <span className="knowledge-badge" data-tone={summary === null ? "neutral" : summary.tone}>{summary === null ? document.summaryState : t(summary.label)}</span>
      <span className="knowledge-badge" data-tone={embed.tone}>{t(embed.label)}</span>
      {document.keywords.map((keyword) => <span className="knowledge-keyword" key={keyword}>{keyword}</span>)}
    </div>
    {document.summary !== "" ? <p className="knowledge-summary">{document.summary}</p> : null}
    <details className="knowledge-disclosure"><summary>{t("knowledge.history.metadata")}</summary>
      <dl className="knowledge-facts">
        <dt>{t("knowledge.field.body")}</dt><dd title={document.bodySha256}>{fillKnowledgeText(t("knowledge.reader.lineCount"), { count: document.lineCount })} · {document.bodySha256.slice(0, 12)}</dd>
        <dt>{t("knowledge.stats.changes")}</dt><dd>#{document.createdChangeSeq} → #{document.updatedChangeSeq}</dd>
        {document.provenance.map((source, index) => <React.Fragment key={index}><dt>{index === 0 ? t("knowledge.reader.sources") : ""}</dt><dd title={source.sha256 ?? undefined}>{source.ref} · {formatDateTime(source.observedAtMs, locale)}</dd></React.Fragment>)}
      </dl>
    </details>
  </header>;
}

// Renders exact text or the existing shared Markdown pipeline with explicit line pagination.
// 使用精确文本或既有共享 Markdown 链路渲染，并提供显式行分页。
// Parameters: props supplies one stable loader and owner. Returns metadata plus one selectable body window.
// 参数：props 提供一个稳定加载器及所有者。返回元数据及一个可选择正文窗口。
function KnowledgeDocumentWindow({ scope, loadWindow, initialLine, highlight, showHeader }: KnowledgeDocumentWindowProps) {
  // All visible labels use shared catalogs.
  // 全部可见标签使用共享目录。
  const { t, locale } = useI18n();
  // Prior starts are explicit service continuations; page size is never duplicated in this client.
  // 先前首行均为明确服务续读值；此客户端不复制页大小。
  const [starts, setStarts] = React.useState([initialLine]);
  // Open in the shared Markdown renderer; source mode keeps the original whitespace and hit markers.
  // 默认使用共享 Markdown 渲染器；原文模式保留空白及命中标记。
  const [rendered, setRendered] = React.useState(true);
  // The last explicit start owns the current body read.
  // 最后一个明确首行拥有当前正文读取。
  const start = starts[starts.length - 1];
  // Stable loader identity changes only with the exact owner or requested window.
  // 稳定加载器身份仅随精确所有者或请求窗口变化。
  const load = React.useCallback(() => loadWindow(start), [loadWindow, start]);
  // Superseded windows cannot repaint the current document or side.
  // 已替代窗口不能重绘当前文档或侧别。
  const read = useKnowledgeRead(JSON.stringify([scope, start]), load);
  // Source lines are bounded by the returned inclusive range so a trailing newline adds no phantom row.
  // 源码行受返回的包含边界范围限制，尾随换行不会产生虚行。
  const lines = read.value === null ? [] : read.value.content.split("\n").slice(0, Math.max(0, read.value.endLine - read.value.startLine + 1));
  return <>
    {read.loading && read.value === null ? <div className="knowledge-reader-load"><StreamPlaceholder label={t("knowledge.viewer.loadingDocument")} /></div> : null}
    {read.error !== null ? <div className="knowledge-reader-load"><KnowledgeReadFailure error={read.error} onRetry={() => { if (start === starts[0] && starts.length === 1) void read.refresh(); else setStarts([starts[0]]); }} /></div> : null}
    {read.value !== null ? <>
      {showHeader ? <KnowledgeDocumentHeader document={read.value.document} t={t} locale={locale} /> : null}
      <div className="knowledge-reader-toolbar">
        <span>{read.value.document.lineCount === 0 ? t("knowledge.reader.empty") : fillKnowledgeText(t("knowledge.reader.range"), { start: read.value.startLine, end: read.value.endLine, total: read.value.document.lineCount })}</span>
        <div className="knowledge-segmented">
          <button type="button" aria-pressed={!rendered} onClick={() => setRendered(false)}>{t("knowledge.reader.source")}</button>
          <button type="button" aria-pressed={rendered} onClick={() => setRendered(true)}>{t("knowledge.reader.markdown")}</button>
        </div>
      </div>
      <div className="knowledge-reader-body" aria-busy={read.loading}>
        {rendered ? <MarkdownMessage content={read.value.content} eager />
          : <pre className="knowledge-source" style={{ "--knowledge-first-line": read.value.startLine - 1 } as React.CSSProperties}><code>{lines.map((line, index) => {
            // Line numbers come from the service window, never from client-side page arithmetic.
            // 行号来自服务窗口，绝非客户端分页算术。
            const number = read.value === null ? 0 : read.value.startLine + index;
            return <span className="knowledge-source-line" data-hit={highlight !== undefined && number >= highlight.start && number <= highlight.end} key={number}>{line}</span>;
          })}</code></pre>}
      </div>
      {starts.length > 1 || starts[0] > 1 || read.value.nextLine !== null ? <footer className="knowledge-reader-foot">
        {starts[0] > 1 ? <Button size="sm" variant="ghost" onClick={() => setStarts([1])}>{t("knowledge.reader.fromStart")}</Button> : <span />}
        <div className="knowledge-pager">
          <Button size="sm" variant="outline" disabled={starts.length === 1} onClick={() => setStarts((current) => current.slice(0, -1))}>{t("knowledge.page.previous")}</Button>
          <Button size="sm" variant="outline" disabled={read.value.nextLine === null} onClick={() => {
            // Capture the validated continuation before the state updater runs.
            // 在状态更新器运行前捕获已验证续读值。
            const next = read.value === null ? null : read.value.nextLine;
            if (next !== null) setStarts((current) => [...current, next]);
          }}>{t("knowledge.page.next")}</Button>
        </div>
      </footer> : null}
    </> : null}
  </>;
}
