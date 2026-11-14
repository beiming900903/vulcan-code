// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { KnowledgeContent } from "./knowledge-content";
import { knowledgeDocumentFixture, knowledgeLibraryFixture } from "./knowledge-test-fixtures";

// Each synthetic read preserves the production directory/document ownership boundary.
// 每个合成读取保留生产目录及文档归属边界。
const service = vi.hoisted(() => ({ list: vi.fn(), read: vi.fn(), search: vi.fn() }));
vi.mock("@/service/knowledge-base-service", () => ({ listManagedKnowledgeDocuments: service.list, readManagedKnowledgeDocument: service.read, searchManagedKnowledgeLibrary: service.search }));
vi.mock("@/i18n", () => ({ useI18n: () => ({ t: (key: string) => key, locale: "en-US" }) }));
// This root owns the mounted content surface and portals.
// 此根拥有挂载内容区域及门户。
let root: Root;
// A detached host isolates DOM effects.
// 独立宿主隔离 DOM 副作用。
let host: HTMLDivElement;
beforeEach(() => {
  vi.resetAllMocks(); vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  service.list.mockImplementation(({ path }: { path: string }) => Promise.resolve({ entries: path === "" ? [{ kind: "folder", path: "notes", description: "Folder purpose", documentCount: 1, document: null }] : [{ kind: "document", path: knowledgeDocumentFixture.path, description: "", documentCount: 1, document: knowledgeDocumentFixture }], nextCursor: null, headChangeSeq: 1 }));
  service.read.mockImplementation(({ startLine }: { startLine: number }) => Promise.resolve({ document: { ...knowledgeDocumentFixture, lineCount: 9 }, content: "Body " + startLine, startLine, endLine: startLine === 1 ? 6 : 9, nextLine: startLine === 1 ? 7 : null }));
  host = document.createElement("div"); document.body.append(host); root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.unstubAllGlobals(); });

it("reads folders lazily, uses stable document IDs and follows the actual bounded line continuation", async () => {
  await act(async () => root.render(<KnowledgeContent library={knowledgeLibraryFixture} />));
  expect(service.list).toHaveBeenCalledExactlyOnceWith({ libraryId: knowledgeLibraryFixture.id, path: "", cursor: null, depth: 1 });
  expect(service.read).not.toHaveBeenCalled();
  await act(async () => host.querySelector<HTMLButtonElement>('[aria-expanded="false"]')!.click());
  expect(service.list).toHaveBeenLastCalledWith({ libraryId: knowledgeLibraryFixture.id, path: "notes", cursor: null, depth: 1 });
  // The stable title identifies the document row independently of tree position.
  // 稳定标题独立于树位置标识文档行。
  const documentButton = [...host.querySelectorAll("button")].find((entry) => entry.textContent?.includes(knowledgeDocumentFixture.title))!;
  await act(async () => documentButton.click());
  expect(service.read).toHaveBeenCalledExactlyOnceWith({ libraryId: knowledgeLibraryFixture.id, documentId: knowledgeDocumentFixture.id, startLine: 1 });
  // Only the body reader's next control is inside the labeled reader region.
  // 带标签的阅读区域内仅有正文阅读器的下一页控件。
  const reader = host.querySelector('section[aria-label="knowledge.view"]')!;
  const next = [...reader.querySelectorAll<HTMLButtonElement>("button")].find((entry) => entry.textContent === "knowledge.page.next")!;
  await act(async () => next.click());
  expect(service.read).toHaveBeenLastCalledWith({ libraryId: knowledgeLibraryFixture.id, documentId: knowledgeDocumentFixture.id, startLine: 7 });
  expect(reader.textContent).toContain("Body 7");
  expect(document.querySelector('textarea, [contenteditable="true"]')).toBeNull();
  expect(service.search).not.toHaveBeenCalled();
});

it("revokes a paid search result on mode changes without automatically issuing another query", async () => {
  service.search.mockResolvedValue({ queries: ["text"], hits: [], libraries: [], failures: [] });
  await act(async () => root.render(<KnowledgeContent library={knowledgeLibraryFixture} />));
  // Native React input dispatch follows the actual controlled-field contract.
  // 原生 React 输入派发遵循真实受控字段契约。
  const input = host.querySelector<HTMLInputElement>('input[aria-label="knowledge.content.search"]')!;
  await act(async () => { Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, "text"); input.dispatchEvent(new Event("input", { bubbles: true })); });
  await act(async () => host.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })));
  expect(service.search).toHaveBeenCalledExactlyOnceWith({ libraryId: knowledgeLibraryFixture.id, query: "text" });
  await act(async () => root.render(<KnowledgeContent library={{ ...knowledgeLibraryFixture, settings: { indexMode: "browse", embedding: null } }} />));
  expect(service.search).toHaveBeenCalledTimes(1);
  expect(host.querySelector("form")).toBeNull();
});

// Verify summary and body hits from the same document open different reading windows.
// 验证同一文档的摘要与正文命中打开不同的阅读窗口。
it("opens summary hits at the beginning and body hits at the cited line", async () => {
  // The shared fixture identifies one document returned by both retrieval modes.
  // 共用夹具标识由两种检索方式返回的同一文档。
  const document = knowledgeDocumentFixture;
  // Each hit keeps its own chunk identity even though they share a document.
  // 两个命中共用文档，但分别持有自己的切块身份。
  const hit = { documentId: document.id, revision: document.revision, path: document.path, title: document.title, score: 1, contentSha256: document.bodySha256, headingPath: "" };
  service.search.mockResolvedValue({
    queries: ["text"], libraries: [], failures: [],
    hits: [
      { ...hit, chunkId: "summary", kind: "summary", startLine: 0, endLine: 0, content: "Summary match", matchKinds: ["vector"] },
      { ...hit, chunkId: "body", kind: "body", startLine: 7, endLine: 9, content: "Body match", matchKinds: ["bm25"] },
    ],
  });
  await act(async () => root.render(<KnowledgeContent library={knowledgeLibraryFixture} />));
  // Submit through the controlled search form before opening a result.
  // 通过受控搜索表单提交，再打开检索结果。
  const input = host.querySelector<HTMLInputElement>('input[aria-label="knowledge.content.search"]')!;
  await act(async () => { Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, "text"); input.dispatchEvent(new Event("input", { bubbles: true })); });
  await act(async () => host.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })));
  // Match rows by their unique snippets so the test does not assume ranking order.
  // 按各自唯一摘要片段定位结果，避免测试依赖排名顺序。
  const buttons = [...host.querySelectorAll<HTMLButtonElement>("button.knowledge-hit")];
  // A summary displays its kind and selects only its own row.
  // 摘要显示命中类型且仅选中本行。
  const summary = buttons.find((button) => button.textContent?.includes("Summary match"))!;
  // A body hit still targets the exact inclusive range.
  // 正文命中仍定位到精确的包含边界范围。
  const body = buttons.find((button) => button.textContent?.includes("Body match"))!;
  expect(summary.querySelector("small")?.textContent).toBe("knowledge.reader.summary");
  await act(async () => summary.click());
  expect(service.read).toHaveBeenCalledExactlyOnceWith({ libraryId: knowledgeLibraryFixture.id, documentId: document.id, startLine: 1 });
  expect(summary.getAttribute("aria-current")).toBe("true");
  expect(host.querySelector('.knowledge-source-line[data-hit="true"]')).toBeNull();
  await act(async () => body.click());
  expect(service.read).toHaveBeenLastCalledWith({ libraryId: knowledgeLibraryFixture.id, documentId: document.id, startLine: 7 });
  expect(summary.getAttribute("aria-current")).toBe("false");
  expect(body.getAttribute("aria-current")).toBe("true");
  // Source mode exposes exact line highlights after Markdown opens the same window by default.
  // 默认以 Markdown 打开同一窗口，切到原文时显示精确命中行。
  const source = [...host.querySelectorAll<HTMLButtonElement>("button")].find((button) => button.textContent === "knowledge.reader.source")!;
  await act(async () => source.click());
  expect(host.querySelector('.knowledge-source-line[data-hit="true"]')?.textContent).toBe("Body 7");
});

// Render native interactive Markdown in the default preview and allow toggling to exact source.
// 在默认预览中渲染原生交互 Markdown，并允许切换到精确原文。
it("previews interactive tabs by default and lets the reader switch to source", async () => {
  // A complete native tabs fence shares the same syntax as a conversation reply.
  // 完整的原生标签围栏与会话回答共用同一语法。
  const content = "# Guide\n\n```ui-tabs\n### First\nFirst content\n### Second\nSecond content\n```";
  service.list.mockResolvedValue({ entries: [{ kind: "document", path: knowledgeDocumentFixture.path, document: knowledgeDocumentFixture }], nextCursor: null });
  service.read.mockResolvedValue({ document: { ...knowledgeDocumentFixture, lineCount: 8 }, content, startLine: 1, endLine: 8, nextLine: null });
  await act(async () => root.render(<KnowledgeContent library={knowledgeLibraryFixture} />));
  await act(async () => host.querySelector<HTMLButtonElement>(".knowledge-tree-row")!.click());
  expect(host.querySelector(".knowledge-reader-body h1")?.textContent).toBe("Guide");
  await act(async () => { await vi.dynamicImportSettled(); });
  // The second tab must change the rendered panel without re-reading the document.
  // 第二个标签必须切换渲染区域，且不重读文档。
  const tab = [...host.querySelectorAll<HTMLButtonElement>('button[role="tab"]')].find((button) => button.textContent === "Second")!;
  await act(async () => tab.click());
  expect(tab.getAttribute("aria-selected")).toBe("true");
  expect(host.querySelector('[role="tabpanel"]')?.textContent).toBe("Second content");
  // The original control returns the stored source with its literal fence.
  // 原文控件显示含原始围栏的存储正文。
  const source = [...host.querySelectorAll<HTMLButtonElement>("button")].find((button) => button.textContent === "knowledge.reader.source")!;
  await act(async () => source.click());
  expect(host.querySelector(".knowledge-source")?.textContent).toContain("```ui-tabs");
  expect(service.read).toHaveBeenCalledTimes(1);
});
