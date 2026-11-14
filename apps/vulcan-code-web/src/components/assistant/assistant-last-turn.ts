import type {
  AnswerCodeSnapshot,
  ConversationEventBody,
  ConversationEventSummary,
  UsageSnapshot,
  AggregatedUsage,
  ResourceId,
} from "@/service/conversation-event-window-store";
import { splitComposerFileReferences } from "@/lib/composer-file-references";

/** Token usage shown in an assistant footer; absent dimensions stay unknown.
 * 助手底栏显示的 Token 用量；缺失维度保持未知。 */
export interface AssistantUsage {
  input?: number;
  cached?: number;
  output?: number;
  total?: number;
}

/** Streamed text of one reasoning or answer block.
 * 一段思考或回答的流式文本。 */
export interface AssistantStreamText {
  text: string;
  streaming: boolean;
}

/** One provider request inside the last turn: its thinking, answer, tools, and own usage.
 * 最后一轮中的一次供应商请求：思考、回答、工具及其自身用量。 */
export interface AssistantReplyStep {
  apiCallId: number;
  reasoning: AssistantStreamText | null;
  answer: AssistantStreamText | null;
  /** Answer-time `ui-code` captures, present once the answer is sealed.
   * 回答当时的 `ui-code` 抓取结果，回答封口后才存在。 */
  answerCodeSnapshots?: readonly AnswerCodeSnapshot[];
  tools: { id: number; name: string; status: Extract<ConversationEventBody, { kind: "tool_call" }>["status"] }[];
  usage: AssistantUsage | null;
  durationMs: number | null;
}

/** The single turn a floating assistant shows; older turns stay in the full session.
 * 浮动助手显示的唯一一轮；更早的回合保留在完整会话中。 */
export interface AssistantLastTurn {
  /** Prompt text of the turn; empty when only resources were sent, null when the turn has no user input.
   * 本轮提示词；只发送资源时为空字符串，回合没有用户输入时为 null。 */
  prompt: string | null;
  /** The opening input's seq once its business turn has closed, or null while open or absent.
   * 业务回合结束后记录首条输入的 seq；回合仍开放或不存在时为 null。 */
  closedStartSeq: number | null;
  /** Project resources attached to the prompt, in send order.
   * 随提示词发送的项目资源，按发送顺序。 */
  resourceIds: ResourceId[];
  /** Original file paths in the compact transcript's collapsed attachment list.
   * 紧凑会话折叠附件列表中的原始文件路径。 */
  fileReferences: string[];
  steps: AssistantReplyStep[];
  /** Closed-turn total, present once the turn statistics are written.
   * 回合闭合后的总用量，写入回合统计后才存在。 */
  total: AssistantUsage | null;
  durationMs: number | null;
  /** Latest error notice detail of the turn, if any.
   * 本轮最近一条错误通知的细节。 */
  error: string | null;
}

/** Projects the last user turn of a session event window into the compact assistant view.
 * 将会话事件窗口的最后一个用户回合投影为精简助手视图。
 *
 * @param events rendered events of the session, in seq order.
 *               会话已渲染事件，按 seq 排序。
 * @returns the last turn; an empty session yields no prompt and no steps. */
export function projectAssistantLastTurn(events: readonly ConversationEventSummary[]): AssistantLastTurn {
  const start = lastTurnStartIndex(events);
  const turn: AssistantLastTurn = { prompt: null, closedStartSeq: null, resourceIds: [], fileReferences: [], steps: [], total: null, durationMs: null, error: null };
  if (start < 0) return turn;
  // The opening input owns the turn identity used to match its durable closure.
  // 首条输入提供匹配该回合持久化结束事件所需的回合标识。
  const openingInput = events[start];
  const startEvent = openingInput.event;
  if (startEvent.kind === "user_input") {
    // Use the same projection as the full transcript for compact assistant prompts.
    // 紧凑助手提示词与完整会话使用相同的投影。
    const { text, references } = splitComposerFileReferences(startEvent.text);
    turn.prompt = text;
    turn.fileReferences = references;
    turn.resourceIds = startEvent.resource_ids ?? [];
  }
  const steps = new Map<number, AssistantReplyStep>();
  const step = (apiCallId: number): AssistantReplyStep => {
    let current = steps.get(apiCallId);
    if (!current) {
      current = { apiCallId, reasoning: null, answer: null, tools: [], usage: null, durationMs: null };
      steps.set(apiCallId, current);
    }
    return current;
  };
  for (const entry of events.slice(start + 1)) {
    if (entry.visibility === "host_only") continue;
    const event = entry.event;
    switch (event.kind) {
      case "assistant_reasoning":
        step(event.api_call_id).reasoning = streamText(step(event.api_call_id).reasoning, event.text, event.status);
        break;
      case "assistant_text":
        step(event.api_call_id).answer = streamText(step(event.api_call_id).answer, event.text, event.status);
        if (event.status !== "streaming") step(event.api_call_id).answerCodeSnapshots = event.code_snapshots ?? [];
        break;
      case "tool_call":
        step(event.api_call_id).tools.push({ id: event.tool_call_id, name: event.tool_name, status: event.status });
        break;
      case "api_stats":
        if (event.purpose === "compaction") break;
        step(event.api_call_id).usage = assistantUsage(event.usage);
        step(event.api_call_id).durationMs = requestDuration(event.timing);
        break;
      case "turn_boundary":
        if (event.purpose === "business" && entry.turn_id !== null && entry.turn_id === openingInput.turn_id) {
          turn.closedStartSeq = openingInput.seq;
        }
        break;
      case "turn_stats":
        turn.total = assistantUsage(event.buckets.total.usage);
        turn.durationMs = event.duration_ms ?? null;
        break;
      case "display_note":
        if (event.severity === "error") turn.error = event.redacted_detail ?? event.reason_code;
        break;
      default:
        break;
    }
  }
  turn.steps = [...steps.values()].filter((entry) => entry.reasoning || entry.answer || entry.tools.length > 0);
  return turn;
}

/** Index of the last turn-opening user input; mid-turn and agent inputs never open a turn.
 * 最后一个开启回合的用户输入下标；回合内补充与代理输入不会开启回合。 */
function lastTurnStartIndex(events: readonly ConversationEventSummary[]): number {
  for (let index = events.length - 1; index >= 0; index -= 1) {
    const event = events[index].event;
    if (event.kind === "user_input" && (event.input_source === "root" || event.input_source === "queued_scheduled")) {
      return index;
    }
  }
  return -1;
}

/** Appends one block of the same request; the latest block decides whether it still streams.
 * 追加同一请求的一个块；最新的块决定是否仍在流式输出。 */
function streamText(
  current: AssistantStreamText | null,
  text: string,
  status: "streaming" | "completed" | "interrupted",
): AssistantStreamText {
  return {
    text: current ? `${current.text}\n\n${text}` : text,
    streaming: status === "streaming",
  };
}

/** Projects provider usage onto the footer dimensions.
 * 将供应商用量投影为底栏维度。 */
function assistantUsage(usage: UsageSnapshot | AggregatedUsage): AssistantUsage {
  return {
    input: usage.input_tokens,
    cached: usage.cache_read_tokens,
    output: usage.output_tokens,
    total: usage.total_tokens,
  };
}

/** Measured request duration, or null when either endpoint is missing.
 * 实测请求时长；任一端点缺失时为 null。 */
function requestDuration(timing: { dispatched_at_ms?: number; completed_at_ms?: number }): number | null {
  if (timing.dispatched_at_ms === undefined || timing.completed_at_ms === undefined) return null;
  return Math.max(0, timing.completed_at_ms - timing.dispatched_at_ms);
}
