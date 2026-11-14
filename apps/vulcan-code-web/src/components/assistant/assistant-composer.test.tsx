// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AssistantComposer, AssistantConversation } from "./assistant-conversation";
import { classifyDroppedComposerFiles, stageDroppedComposerFile } from "@/service/conversation-actions-service";
import type { ConversationEventWindowControllerSnapshot } from "@/service/conversation-event-window-controller";
import { clientActionId, type ConversationEventSummary } from "@/service/conversation-event-window-store";

// Only the text catalog and the service boundary are synthetic.
// 仅文案目录与服务边界为合成。
vi.mock("@/i18n", async (importOriginal) => ({
  ...await importOriginal<typeof import("@/i18n")>(),
  useI18n: () => ({ t: (key: string) => key }),
}));

const submitConversationPrompt = vi.fn<(...args: unknown[]) => Promise<unknown>>();
vi.mock("@/service/conversation-actions-service", () => ({
  classifyDroppedComposerFiles: vi.fn(),
  reconcileComposerAttachmentDraftLease: vi.fn(async () => undefined),
  stageComposerAttachmentBytes: vi.fn(),
  stageComposerAttachmentPreviewBytes: vi.fn(),
  stageDroppedComposerFile: vi.fn(),
  stopConversationRun: vi.fn(),
  submitConversationPrompt: (...args: unknown[]) => submitConversationPrompt(...args),
}));

// The assistant transcript test supplies successive snapshots at the live reader boundary.
// 助手转录测试在实时读取器边界提供连续的快照。
let assistantSnapshot: ConversationEventWindowControllerSnapshot;
vi.mock("@/state/conversation-session-event-window-owner-v1", () => ({
  useConversationSessionEventWindowOwnerV1: () => ({
    loading: false,
    error: null,
    reader: {
      subscribe: () => () => undefined,
      getSnapshot: () => assistantSnapshot,
      getServerSnapshot: () => assistantSnapshot,
    },
  }),
}));

let host: HTMLDivElement;
let root: Root;

beforeEach(() => {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
});

afterEach(async () => {
  await act(async () => root.unmount());
  host.remove();
  submitConversationPrompt.mockReset();
  vi.unstubAllGlobals();
});

const input = () => host.querySelector<HTMLTextAreaElement>("textarea")!;

// type replaces the input's value the way a keystroke does, so React sees the change.
// type 以按键的方式替换输入框的值，使 React 观察到变化。
async function type(value: string) {
  const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!;
  await act(async () => {
    setter.call(input(), value);
    input().dispatchEvent(new Event("input", { bubbles: true }));
  });
}

async function pressEnter() {
  await act(async () => {
    input().dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
  });
}

// sendPending renders the composer, sends `text`, and returns the receipt still pending.
// sendPending 渲染输入区并发送 `text`，返回仍在等待的回执。
async function sendPending(text: string) {
  let resolve!: (ack: unknown) => void;
  submitConversationPrompt.mockReturnValue(new Promise((done) => { resolve = done; }));
  const assistant = { id: "draft-assistant", name: "Helper", projectId: null, projectName: null, running: false };
  await act(async () => root.render(
    <AssistantComposer assistant={assistant} running={false} />,
  ));
  await type(text);
  await pressEnter();
  expect(submitConversationPrompt).toHaveBeenCalledOnce();
  return async () => { await act(async () => resolve({ accepted: true })); };
}

describe("floating assistant memory status", () => {
  it("shows the locked guide and marks only closed turns inside the lock span", async () => {
    // The retained turn happened before the lock, and the two later turns start inside it.
    // 保留的回合发生在锁定前，随后的两轮从锁定区间内开始。
    const assistant = { id: "locked-assistant", name: "Translator", projectId: null, projectName: null, running: false };
    assistantSnapshot = {
      owner: { session_id: assistant.id, window_generation: 1 },
      service_epoch: "fixture",
      memory_revision: 1,
      durable_revision: 1,
      events: [],
      older_before_seq: null,
      has_more: false,
      open_turn_id: null,
      open_turn_start_seq: null,
      session_config: {
        answer_mode: { mode: "standard", revision: 0, prepared: null },
        permission_mode: "workspace_write",
        current_request: { provider: null, connection_id: null, model: null, thinking_level: null, temperature: null, set_at_ms: null },
        next_request: null,
        context_window_selection: null,
        dedicated: { tool_access: "none", base_prompt: "Translate", locked_spans: [{ locked_at_seq: 10, unlocked_at_seq: null }] },
      },
    };
    // Render the supplied events and read the current memory announcement.
    // 渲染给定事件并读取当前的记忆提示。
    const render = async (events: readonly ConversationEventSummary[], running: boolean) => {
      assistantSnapshot = { ...assistantSnapshot, events };
      await act(async () => root.render(<AssistantConversation assistant={{ ...assistant, running }} />));
      return host.querySelector(".assistant-turn-memory[role='status']")?.textContent;
    };
    // Model a turn whose input starts at seq; its closure may be delivered in a later update.
    // 构造起点为 seq 的回合，结束事件可能在后续更新中送达。
    const turn = (seq: number): [ConversationEventSummary, ConversationEventSummary] => [
      {
        seq, turn_id: seq, created_at_ms: seq, updated_at_ms: seq, event_revision: 1, visibility: "provider",
        event: { kind: "user_input", client_action_id: clientActionId(`action-${seq}`), input_id: seq, input_source: "root", text: "Translate this" },
      },
      {
        seq: seq + 1, turn_id: seq, created_at_ms: seq + 1, updated_at_ms: seq + 1, event_revision: 1, visibility: "ui_only",
        event: { kind: "turn_boundary", purpose: "business", reason: "completed", outcome: "completed" },
      },
    ];

    expect(await render(turn(1), false)).toBe("");
    expect(host.querySelector(".assistant-memory")?.textContent).toContain("assistantDock.memoryGuide");
    for (const seq of [11, 20]) {
      const [opening, closing] = turn(seq);
      expect(await render([opening], true)).toBe("");
      expect(await render([opening, closing], false)).toBe("assistantDock.turnNotRemembered");
    }
  });
});

describe("assistant composer", () => {
  it("shows a rejected file in the attachment strip and sends its path without staging", async () => {
    const file = new File(["archive data"], "chapter.zip", { type: "application/zip" });
    vi.stubGlobal("vulcanDesktop", { localFile: { inspect: vi.fn(), pathForFile: () => "D:/Docs/chapter.zip" } });
    vi.mocked(classifyDroppedComposerFiles).mockResolvedValueOnce({ eligible: [false], planId: null });
    const assistant = { id: "paste-assistant", name: "Helper", projectId: "fixture", projectName: null, running: false };
    await act(async () => root.render(<AssistantComposer assistant={assistant} running={false} />));
    const event = new Event("paste", { bubbles: true, cancelable: true });
    Object.defineProperty(event, "clipboardData", { value: { files: [file], items: [], getData: () => "" } });
    await act(async () => { input().dispatchEvent(event); });
    expect(classifyDroppedComposerFiles).toHaveBeenCalledWith(expect.objectContaining({
      conversationId: assistant.id, files: [file], forSubmission: true,
    }));
    expect(stageDroppedComposerFile).not.toHaveBeenCalled();
    expect(input().value).toBe("");
    expect(host.querySelector('.vk-attach .vk-attach-tile[title="D:/Docs/chapter.zip"]')).not.toBeNull();
    submitConversationPrompt.mockResolvedValueOnce({ accepted: true });
    await pressEnter();
    expect(submitConversationPrompt).toHaveBeenCalledOnce();
    expect(submitConversationPrompt.mock.calls[0][4]).toContain('"D:/Docs/chapter.zip"');
    expect(submitConversationPrompt.mock.calls[0][6]).toEqual([]);
    expect(host.querySelector(".vk-attach")).toBeNull();
  });

  it("stages a pasted image once and rechecks it before submission", async () => {
    // The staged asset supplies the real preview while a new route can downgrade it to the original path.
    // 暂存资产提供真实预览，新路线可将其降级为原始路径。
    const file = new File(["image"], "assistant.png", { type: "image/png" });
    vi.stubGlobal("vulcanDesktop", { localFile: { inspect: vi.fn(), pathForFile: () => "D:/Docs/assistant.png" } });
    vi.mocked(classifyDroppedComposerFiles)
      .mockResolvedValueOnce({ eligible: [true], planId: "assistant-stage" })
      .mockResolvedValueOnce({ eligible: [false], planId: null });
    vi.mocked(stageDroppedComposerFile).mockClear().mockResolvedValueOnce({
      assetId: "assistant-image", kind: "image", fileName: file.name, mimeType: file.type, localPath: "data:image/png;base64,AA==",
    });
    // Mount a separate draft so the result is independent of the reference-only test.
    // 挂载独立草稿，使结果不受仅引用测试影响。
    const assistant = { id: "image-assistant", name: "Helper", projectId: "fixture", projectName: null, running: false };
    await act(async () => root.render(<AssistantComposer assistant={assistant} running={false} />));
    // Dispatch the same clipboard shape as the browser event.
    // 派发与浏览器事件相同结构的剪贴板数据。
    const event = new Event("paste", { bubbles: true, cancelable: true });
    Object.defineProperty(event, "clipboardData", { value: { files: [file], items: [], getData: () => "" } });
    await act(async () => input().dispatchEvent(event));
    expect(host.querySelector('.vk-attach img[alt="assistant.png"]')).not.toBeNull();
    expect(stageDroppedComposerFile).toHaveBeenCalledExactlyOnceWith("assistant-stage", 0, file);
    submitConversationPrompt.mockResolvedValueOnce({ accepted: true });
    await pressEnter();
    expect(classifyDroppedComposerFiles).toHaveBeenLastCalledWith(expect.objectContaining({
      files: [file], forSubmission: false,
    }));
    expect(stageDroppedComposerFile).toHaveBeenCalledOnce();
    expect(submitConversationPrompt.mock.calls[0][4]).toContain('"D:/Docs/assistant.png"');
    expect(submitConversationPrompt.mock.calls[0][6]).toEqual([]);
  });

  it("clears only what was sent when the receipt arrives", async () => {
    const receive = await sendPending("first");
    await receive();
    expect(input().value).toBe("");
  });

  it("keeps a draft typed while the receipt was pending", async () => {
    const receive = await sendPending("first");
    await type("second");
    await receive();
    expect(input().value).toBe("second");
  });
});
