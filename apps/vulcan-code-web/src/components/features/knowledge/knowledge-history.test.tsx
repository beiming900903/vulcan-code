// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { KnowledgeChange } from "@/types/knowledge-base";
import { KnowledgeHistory, knowledgeHistoryItemPath } from "./knowledge-history";
import { knowledgeDocumentFixture, knowledgeLibraryFixture } from "./knowledge-test-fixtures";

// Synthetic immutable history endpoints never access a real library.
// 合成不可变历史端点绝不访问真实知识库。
const service = vi.hoisted(() => ({ list: vi.fn(), detail: vi.fn(), body: vi.fn() }));
vi.mock("@/service/knowledge-base-service", () => ({ listKnowledgeChanges: service.list, getKnowledgeChange: service.detail, readKnowledgeChangeDocument: service.body }));
// Parameters: key is a stable label. Returns keys and required interpolation tokens for identity assertions.
// 参数：key 为稳定标签。返回键及必要插值标记，以便身份断言。
function translate(key: string): string { return key === "knowledge.history.change" ? key + " {seq}" : key; }
vi.mock("@/i18n", () => ({ useI18n: () => ({ t: translate, locale: "en-US" }) }));

// Each root owns only this test's portal lifecycle.
// 每个根仅拥有此测试的门户生命周期。
let root: Root;
// The detached host keeps all reads synthetic.
// 独立宿主保持全部读取为合成。
let host: HTMLDivElement;
// A noncontiguous immutable sequence proves pagination cannot use arithmetic offsets.
// 不连续不可变序号证明分页不能使用算术偏移。
const changes: KnowledgeChange[] = Array.from({ length: 21 }, (_, index) => ({ seq: 100 - index * 3, atMs: 1, actor: { kind: "ai", sessionId: "s", turnId: "t", toolCallId: "c" }, operation: "create", note: "change-" + (100 - index * 3), restoreTargetSeq: null, itemCount: 1 }));
beforeEach(() => {
  vi.resetAllMocks(); vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  service.list.mockResolvedValue(changes);
  service.detail.mockResolvedValue({ change: changes[0], items: [{ entity: "document", entityKey: knowledgeDocumentFixture.id, before: null, after: knowledgeDocumentFixture }], nextOffset: null });
  service.body.mockResolvedValue({ document: knowledgeDocumentFixture, content: "# Historical body", startLine: 1, endLine: 1, nextLine: null });
  host = document.createElement("div"); document.body.append(host); root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.unstubAllGlobals(); });

// Finds a semantic control without relying on an evolving position.
// 查找语义控件，不依赖会演进的位置。
// Parameters: label and scope identify the actual button. Returns it or a diagnostic error.
// 参数：label 及 scope 标识实际按钮。返回该按钮或诊断错误。
function button(label: string, scope: ParentNode = document): HTMLButtonElement {
  // Exact catalog identity is stable across wording edits.
  // 精确目录身份不随措辞编辑变化。
  const found = [...scope.querySelectorAll("button")].find((entry) => entry.textContent === label);
  if (found === undefined) throw new Error("Missing control: " + label);
  return found;
}

describe("permanent history", () => {
  it("requires confirmation and serializes the exact restore target", async () => {
    // A delayed save exposes the double-click boundary.
    // 延迟保存暴露双击边界。
    let finish!: () => void;
    // Only the confirmed callback can mutate content.
    // 仅已确认回调可以变更内容。
    const restore = vi.fn(() => new Promise<void>((resolve) => { finish = resolve; }));
    await act(async () => root.render(<KnowledgeHistory library={knowledgeLibraryFixture} busy={false} onRestore={restore} />));
    // The note identifies this exact immutable history row.
    // 说明标识此精确不可变历史行。
    const row = [...host.querySelectorAll("ol li")].find((entry) => entry.textContent?.includes(changes[0].note))!;
    await act(async () => button("knowledge.history.restore", row).click());
    expect(restore).not.toHaveBeenCalled();
    // Confirmation belongs to the target captured above.
    // 确认属于上方捕获目标。
    const confirm = document.querySelector('[role="alertdialog"]')!;
    await act(async () => { button("knowledge.history.restore", confirm).click(); button("knowledge.history.restore", confirm).click(); });
    expect(restore).toHaveBeenCalledExactlyOnceWith(changes[0].seq);
    await act(async () => finish());
    expect(document.querySelector('[role="alertdialog"]')).toBeNull();
  });

  it("continues from the last displayed sequence rather than an assumed contiguous offset", async () => {
    await act(async () => root.render(<KnowledgeHistory library={knowledgeLibraryFixture} busy={false} onRestore={vi.fn()} />));
    // Observe the actual viewport so changing its page size does not invalidate the test.
    // 观测真实视口，使更改页大小不会使测试失效。
    const displayed = changes.filter((change) => [...host.querySelectorAll("ol li p")].some((entry) => entry.textContent === change.note));
    await act(async () => button("knowledge.page.next", host).click());
    expect(service.list).toHaveBeenLastCalledWith(expect.objectContaining({ libraryId: knowledgeLibraryFixture.id, beforeSeq: Math.min(...displayed.map((change) => change.seq)) }));
  });

  it("loads immutable bodies only after a document is selected and never substitutes a current body", async () => {
    await act(async () => root.render(<KnowledgeHistory library={knowledgeLibraryFixture} busy={false} onRestore={vi.fn()} />));
    // Select the row by its immutable note, not an index.
    // 按不可变说明选择行，而非下标。
    const row = [...host.querySelectorAll("ol li")].find((entry) => entry.textContent?.includes(changes[0].note))!;
    await act(async () => button("knowledge.history.compare", row).click());
    expect(service.body).not.toHaveBeenCalled();
    await act(async () => button(knowledgeDocumentFixture.path).click());
    expect(service.body).toHaveBeenCalledExactlyOnceWith({ libraryId: knowledgeLibraryFixture.id, seq: changes[0].seq, documentId: knowledgeDocumentFixture.id, side: "after", startLine: 1 });
    expect(document.querySelector('[role="dialog"] .knowledge-reader-body h1')?.textContent).toBe("Historical body");
    expect(knowledgeHistoryItemPath({ entity: "document", entityKey: knowledgeDocumentFixture.id, before: knowledgeDocumentFixture, after: null })).toBe(knowledgeDocumentFixture.path);
  });
});
