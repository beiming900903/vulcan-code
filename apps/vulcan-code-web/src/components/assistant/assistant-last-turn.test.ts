import { describe, expect, it } from "vitest";
import { appendComposerFileReferences } from "@/lib/composer-file-references";
import { clientActionId, resourceId, type ConversationEventBody, type ConversationEventSummary } from "@/service/conversation-event-window-store";
import { projectAssistantLastTurn } from "./assistant-last-turn";

// event wraps one body into a rendered event at the given seq.
// event 将一个事件体包装为指定 seq 的已渲染事件。
const event = (seq: number, body: ConversationEventBody): ConversationEventSummary => ({
  seq, turn_id: 1, created_at_ms: seq, updated_at_ms: seq, event_revision: 1, visibility: "provider", event: body,
});

// input builds one user input of the given source.
// input 构建一条指定来源的用户输入。
const input = (seq: number, text: string, source: "root" | "pending_in_turn" = "root") => event(seq, {
  kind: "user_input", client_action_id: clientActionId(`action-${seq}`), input_id: seq, input_source: source, text,
});

const usage = { input_tokens: 100, cache_read_tokens: 40, output_tokens: 20, total_tokens: 120 };

describe("projectAssistantLastTurn", () => {
  it("keeps only the last turn with each request's thinking, answer, and own usage", () => {
    const turn = projectAssistantLastTurn([
      input(1, "old question"),
      event(2, { kind: "assistant_text", api_call_id: 1, text: "old answer", status: "completed" }),
      input(3, "new question"),
      event(4, { kind: "assistant_reasoning", api_call_id: 2, text: "thinking", status: "completed" }),
      event(5, { kind: "assistant_text", api_call_id: 2, text: "answer", status: "completed" }),
      event(6, {
        kind: "api_stats", api_call_id: 2, purpose: "main_generation", provider: "p", model: "m", usage,
        status: "completed", continuation_eligibility: "forbidden",
        timing: { dispatched_at_ms: 1000, completed_at_ms: 3500 },
      }),
      input(7, "steer", "pending_in_turn"),
    ]);
    expect(turn.prompt).toBe("new question");
    expect(turn.steps).toHaveLength(1);
    expect(turn.steps[0]).toMatchObject({
      reasoning: { text: "thinking", streaming: false },
      answer: { text: "answer", streaming: false },
      usage: { input: 100, cached: 40, output: 20, total: 120 },
      durationMs: 2500,
    });
    expect(turn.total).toBeNull();
    expect(turn.closedStartSeq).toBeNull();
  });

  it("reports the closed-turn total and marks a block still streaming", () => {
    const turn = projectAssistantLastTurn([
      input(1, "question"),
      event(2, { kind: "assistant_reasoning", api_call_id: 1, text: "partial", status: "streaming" }),
      event(3, {
        kind: "turn_stats", start_seq: 1, end_seq: 3, outcome: "completed", duration_ms: 4200,
        buckets: {
          direct: { api_call_count: 1, unknown_usage_api_call_count: 0, tool_call_count: 0, subagent_return_count: 0, usage },
          subagent: { api_call_count: 0, unknown_usage_api_call_count: 0, tool_call_count: 0, subagent_return_count: 0, usage: {} },
          total: { api_call_count: 1, unknown_usage_api_call_count: 0, tool_call_count: 0, subagent_return_count: 0, usage },
        },
      }),
    ]);
    expect(turn.steps[0].reasoning?.streaming).toBe(true);
    expect(turn.total).toEqual({ input: 100, cached: 40, output: 20, total: 120 });
    expect(turn.durationMs).toBe(4200);
  });

  it("returns an empty turn for a session without user input", () => {
    expect(projectAssistantLastTurn([])).toEqual({
      prompt: null, closedStartSeq: null, resourceIds: [], fileReferences: [], steps: [], total: null, durationMs: null, error: null,
    });
  });

  it("marks closure only for the displayed business turn, even when another turn closes", () => {
    // The assistant can receive closures for auxiliary work alongside its latest turn.
    // 助手的最后一轮进行期间也可能收到辅助工作的结束事件。
    const otherClosures = [
      { ...event(3, { kind: "turn_boundary", reason: "completed", outcome: "completed", purpose: "business" }), turn_id: 2 },
      event(4, { kind: "turn_boundary", reason: "completed", outcome: "completed", purpose: "question_execution" }),
    ];
    // Stop and failure also close a business turn even if it produced no reply.
    // 即使没有回复，停止和失败也会结束业务回合。
    const closed = event(5, { kind: "turn_boundary", reason: "stopped", outcome: "stopped", purpose: "business" });
    expect(projectAssistantLastTurn([input(2, "task"), ...otherClosures]).closedStartSeq).toBeNull();
    expect(projectAssistantLastTurn([input(2, "task"), ...otherClosures, closed]).closedStartSeq).toBe(2);
  });

  it("separates path references from the compact transcript prompt", () => {
    // Reference paths also appear on the compact transcript when sent without text.
    // 不带正文发送时，紧凑会话也保留引用路径。
    const references = ["D:/Example Folder/guide.chm"];
    expect(projectAssistantLastTurn([input(1, appendComposerFileReferences("", references))]))
      .toMatchObject({ prompt: "", fileReferences: references });
  });

  it("keeps a resource-only prompt as an empty prompt with its resources", () => {
    const turn = projectAssistantLastTurn([event(1, {
      kind: "user_input", client_action_id: clientActionId("action-1"), input_id: 1, input_source: "root", text: "",
      resource_ids: [resourceId(7)],
    })]);
    expect(turn.prompt).toBe("");
    expect(turn.resourceIds).toEqual([resourceId(7)]);
  });
});
