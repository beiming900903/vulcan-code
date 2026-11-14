import React from "react";
import { ArrowUp, Check, Copy, Lock, Square } from "lucide-react";
import { toast } from "@/ui-kit/primitives/toast";
import { ComposerAttachmentStrip } from "@/components/composer/composer-attachment-controls";
import { composerAttachmentMedia } from "@/components/composer/composer-attachment-media";
import { useComposerAttachments } from "@/components/composer/composer-attachments";
import { useComposerDroppedFiles } from "@/components/composer/composer-panel/use-composer-dropped-files";
import { ComposerTextContextMenu } from "@/components/composer/composer-text-context-menu";
import { composerAttachmentForResource } from "@/components/conversation/conversation-branch-presentation-v1";
import { ConversationPromptFileReferences } from "@/components/conversation/conversation-prompt-file-references";
import { runConversationControlAction } from "@/components/conversation/conversation-control-feedback";
import { AnswerCodeSnapshotsProvider, MarkdownMessage } from "@/components/conversation/conversation-markdown";
import { GeneratedMediaPlayerWindow, type GeneratedMediaAttachment } from "@/components/media/generated-media";
import { Button } from "@/ui-kit/primitives/button";
import { InlineDisclosure } from "@/ui-kit/primitives/disclosure";
import { Spinner } from "@/ui-kit/primitives/spinner";
import { Textarea } from "@/ui-kit/primitives/textarea";
import { copyText } from "@/components/workspace/workbench-layout/types";
import { isElectronRuntime } from "@/host-shell";
import { formatMessage, useI18n } from "@/i18n";
import { formatDurationSeconds, formatTokenCount } from "@/lib/usage-format";
import { createClientOwnedActionId } from "@/notifications/client-identity";
import { localMediaSourceUrl } from "@/resources/localMedia";
import {
  reconcileComposerAttachmentDraftLease,
  stageComposerAttachmentBytes,
  stopConversationRun,
  submitConversationPrompt,
} from "@/service/conversation-actions-service";
import { dedicatedTurnForgotten, selectDedicatedMemory } from "@/service/dedicated-memory";
import { useConversationSessionEventWindowOwnerV1 } from "@/state/conversation-session-event-window-owner-v1";
import { useConversationSessionResourcesV1 } from "@/state/conversation-session-resources-v1";
import type { ComposerAttachmentCapabilitySummary, ComposerAttachmentSummary } from "@/types";
import type { AssistantEntry } from "./assistant-dock-context";
import { projectAssistantLastTurn, type AssistantReplyStep, type AssistantUsage } from "./assistant-last-turn";

/** Unsent input per assistant, kept while the user switches between assistants.
 * 每个助手尚未发送的输入，在用户切换助手时保留。 */
const assistantDrafts = new Map<string, string>();

// Retain the assistant's staged draft while switching between assistants.
// 在不同助手间切换时保留已暂存的助手草稿。
const assistantAttachmentDrafts = new Map<string, ComposerAttachmentSummary[]>();

// Core preflight is the admission authority for this input, which has no file picker.
// 此输入区没有文件选择器，Core 预检是准入权威。
const assistantPickerCapabilities: ComposerAttachmentCapabilitySummary[] = [];

/** Compact live view of one assistant: only its last turn plus an input; the session keeps everything.
 * 单个助手的精简实时视图：只显示最后一轮与输入框；完整内容保留在会话中。
 * @param props.assistant floating assistant identity and live run state.
 *                        浮动助手标识及实时运行状态。
 * @returns the memory status, latest turn, and composer.
 *          记忆状态、最后一轮及输入区。 */
export function AssistantConversation(props: { assistant: AssistantEntry }) {
  const { t } = useI18n();
  const owner = useConversationSessionEventWindowOwnerV1({
    sessionId: props.assistant.id,
    parentSessionId: null,
    live: true,
  });
  // The compact assistant surface consumes the full canonical reply in its own renderer.
  // 紧凑助手界面在自己的渲染器中消费完整的规范回复。
  const snapshot = React.useSyncExternalStore(owner.reader.subscribe, owner.reader.getSnapshot, owner.reader.getServerSnapshot);
  const turn = React.useMemo(() => projectAssistantLastTurn(snapshot.events), [snapshot.events]);
  // A converted assistant may still show a turn from before its memory was locked.
  // 刚转换的助手仍可能显示锁定记忆前的一轮。
  const memory = turn.closedStartSeq === null ? null : selectDedicatedMemory(snapshot);
  const turnForgotten = memory !== null && turn.closedStartSeq !== null
    && dedicatedTurnForgotten(memory.lockedSpans, turn.closedStartSeq, false);
  const resources = useConversationSessionResourcesV1(props.assistant.id, turn.resourceIds);
  const promptAttachments = React.useMemo(
    () => turn.resourceIds.map((resourceId) => composerAttachmentForResource(resourceId, resources)),
    [resources, turn.resourceIds],
  );
  const [mediaPlayer, setMediaPlayer] = React.useState<GeneratedMediaAttachment | null>(null);
  const running = props.assistant.running;
  const scrollRef = React.useRef<HTMLDivElement | null>(null);
  // Follow the newest output while the turn is still open.
  // 回合仍开放时跟随最新输出。
  React.useLayoutEffect(() => {
    const element = scrollRef.current;
    if (element && running) element.scrollTop = element.scrollHeight;
  }, [running, turn]);
  return (
    <div className="assistant-conversation">
      <div className="assistant-memory">
        <p className="assistant-memory-title">
          <Lock size={14} aria-hidden="true" />
          {t("conversation.dedicatedNotice.locked")}
        </p>
        <p>{t("assistantDock.memoryGuide")}</p>
      </div>
      <div ref={scrollRef} className="assistant-conversation-scroll">
        {owner.error ? <p className="assistant-conversation-error">{owner.error}</p> : null}
        {turn.prompt === null && !owner.loading ? (
          <p className="assistant-conversation-hint">
            {formatMessage(t("assistantDock.emptyTurn"), { name: props.assistant.name })}
          </p>
        ) : null}
        <AssistantAttachmentStrip attachments={promptAttachments} onOpenMedia={setMediaPlayer} />
        <ConversationPromptFileReferences references={turn.fileReferences} />
        {turn.prompt ? (
          <div className="assistant-prompt">{turn.prompt}</div>
        ) : null}
        {turn.steps.map((step) => <AssistantStep key={step.apiCallId} step={step} />)}
        {turn.error ? <p className="assistant-conversation-error">{turn.error}</p> : null}
        {(owner.loading && turn.prompt === null) || (running && turn.steps.length === 0) ? (
          <Spinner />
        ) : null}
      </div>
      {turn.total ? (
        <AssistantUsageLine
          className="assistant-turn-total"
          durationMs={turn.durationMs}
          label={t("assistantDock.turnTotal")}
          usage={turn.total}
        />
      ) : null}
      <p className="assistant-turn-memory" role="status">
        {turnForgotten ? t("assistantDock.turnNotRemembered") : null}
      </p>
      <AssistantComposer
        assistant={props.assistant}
        running={running}
        onOpenMedia={setMediaPlayer}
      />
      {mediaPlayer ? <GeneratedMediaPlayerWindow attachment={mediaPlayer} onClose={() => setMediaPlayer(null)} /> : null}
    </div>
  );
}

/** Attachment tiles reused from the main composer; read-only unless `onRemove` is given.
 * 复用主输入区的附件块；未提供 `onRemove` 时为只读。 */
function AssistantAttachmentStrip(props: {
  attachments: ComposerAttachmentSummary[];
  onOpenMedia?: (media: GeneratedMediaAttachment) => void;
  onRemove?: (index: number) => void;
  // Local previews share the same strip as uploaded attachments.
  // 本地预览与已上传附件共用附件栏。
  localTiles?: React.ReactNode[];
}) {
  return (
    <ComposerAttachmentStrip
      attachments={props.attachments}
      localTiles={props.localTiles}
      onOpenMedia={(attachment) => {
        const media = composerAttachmentMedia(attachment);
        if (media) props.onOpenMedia?.(media);
      }}
      onRemove={props.onRemove}
      resolvePreview={(attachment) => {
        const media = composerAttachmentMedia(attachment);
        if (!media) return null;
        return {
          kind: media.kind,
          sourceUrl: media.kind === "image" ? localMediaSourceUrl(media.previewLocalPath ?? media.localPath) : undefined,
        };
      }}
    />
  );
}

/** Characters of streaming thinking kept for the one-line ticker; far wider than any assistant window.
 * 单行滚动条保留的流式思考字符数；远宽于任何助手窗口。 */
const THINKING_TICKER_TAIL = 400;

/** Collapsed live thinking as one line that starts at the beginning and then keeps its newest text in view;
 * it only moves forward and never rewinds.
 * 收起状态下的实时思考单行：从开头输出，之后始终显示最新文字；只向前推进，从不回卷。 */
function AssistantThinkingTicker(props: { text: string }) {
  const ref = React.useRef<HTMLDivElement | null>(null);
  const line = props.text.slice(-THINKING_TICKER_TAIL).replace(/\s+/gu, " ").trim();
  React.useLayoutEffect(() => {
    const element = ref.current;
    if (!element) return;
    element.scrollLeft = element.scrollWidth;
    element.toggleAttribute("data-overflow", element.scrollWidth > element.clientWidth);
  }, [line]);
  return <div aria-hidden="true" className="assistant-thinking-ticker" ref={ref}>{line}</div>;
}

/** Keeps a scrolling box at its bottom while its content grows, until the reader scrolls up; scrolling back to
 * the bottom pins it again. A box that mounts (the thinking is expanded) starts at the bottom.
 * 内容增长时让滚动框保持在底部，直到读者向上滚动；滚回底部后重新锁定。新挂载（思考展开）的滚动框从底部开始。 */
function useStickToBottom(content: string) {
  const element = React.useRef<HTMLDivElement | null>(null);
  const pinned = React.useRef(true);
  const ref = React.useCallback((node: HTMLDivElement | null) => {
    element.current = node;
    pinned.current = true;
    if (node) node.scrollTop = node.scrollHeight;
  }, []);
  React.useLayoutEffect(() => {
    const node = element.current;
    if (node && pinned.current) node.scrollTop = node.scrollHeight;
  }, [content]);
  const onScroll = React.useCallback(() => {
    const node = element.current;
    if (node) pinned.current = node.scrollHeight - node.scrollTop - node.clientHeight < 8;
  }, []);
  return { ref, onScroll };
}

/** One provider request: thinking folded by default (a one-line ticker while it streams), then the answer.
 * 一次供应商请求：思考默认收起（流式输出时显示单行滚动），随后是回答。 */
export function AssistantStep(props: { step: AssistantReplyStep }) {
  const { t } = useI18n();
  const { step } = props;
  const [thinkingOpen, setThinkingOpen] = React.useState(false);
  const thinkingLive = Boolean(step.reasoning?.streaming) && !step.answer;
  const thinkingScroll = useStickToBottom(step.reasoning?.text ?? "");
  return (
    <div className="assistant-step">
      {step.reasoning ? (
        <InlineDisclosure
          label={thinkingLive ? t("assistantDock.thinkingLive") : t("assistantDock.thinking")}
          onOpenChange={setThinkingOpen}
          open={thinkingOpen}
        >
          <div className="assistant-thinking-text" onScroll={thinkingScroll.onScroll} ref={thinkingScroll.ref}>{step.reasoning.text}</div>
        </InlineDisclosure>
      ) : null}
      {step.reasoning && thinkingLive && !thinkingOpen ? <AssistantThinkingTicker text={step.reasoning.text} /> : null}
      {step.tools.length > 0 ? (
        <div className="assistant-tool-list">
          {step.tools.map((tool) => <span key={tool.id} className="assistant-tool-chip">{tool.name}</span>)}
        </div>
      ) : null}
      {step.answer ? (
        <AnswerCodeSnapshotsProvider
          value={step.answerCodeSnapshots ? { pending: false, snapshots: step.answerCodeSnapshots } : step.answer.streaming ? { pending: true } : null}
        >
          <MarkdownMessage content={step.answer.text} eager streaming={step.answer.streaming} />
        </AnswerCodeSnapshotsProvider>
      ) : null}
      {step.usage || (step.answer && !step.answer.streaming) ? (
        <div className="assistant-step-footer">
          {step.usage ? <AssistantUsageLine durationMs={step.durationMs} usage={step.usage} /> : null}
          {step.answer && !step.answer.streaming ? <AssistantCopyButton text={step.answer.text} /> : null}
        </div>
      ) : null}
    </div>
  );
}

/** Copies one finished answer's Markdown source and briefly confirms it.
 * 复制一段已完成回答的 Markdown 原文，并短暂显示确认。 */
function AssistantCopyButton(props: { text: string }) {
  const { t } = useI18n();
  const [copied, setCopied] = React.useState(false);
  const label = copied ? t("conversation.action.copiedFinal") : t("conversation.action.copyFinal");
  const copy = async () => {
    setCopied(false);
    setCopied(await runConversationControlAction(() => copyText(props.text), t("conversation.control.copyFailed")));
  };
  return (
    <Button
      aria-label={label}
      className="assistant-copy-button"
      onClick={() => void copy()}
      onMouseLeave={() => setCopied(false)}
      size="icon-xs"
      title={label}
      type="button"
      variant="ghost"
    >
      {copied ? <Check size={13} /> : <Copy size={13} />}
    </Button>
  );
}

/** Usage written out in one quiet line; nothing hides behind a hover.
 * 以一行低调文字直接写出用量；不藏在悬停提示后。 */
function AssistantUsageLine(props: {
  usage: AssistantUsage;
  durationMs: number | null;
  label?: string;
  className?: string;
}) {
  const { t } = useI18n();
  const metric = (key: Parameters<typeof t>[0], value: number | undefined) => (
    value === undefined ? null : formatMessage(t(key), { value: formatTokenCount(value) })
  );
  const parts = [
    props.label ?? null,
    metric("assistantDock.usage.input", props.usage.input),
    metric("assistantDock.usage.cached", props.usage.cached),
    metric("assistantDock.usage.output", props.usage.output),
    metric("assistantDock.usage.total", props.usage.total),
    props.durationMs === null ? null : formatDurationSeconds(props.durationMs),
  ].filter((part): part is string => part !== null);
  return <p className={props.className ? `assistant-usage-line ${props.className}` : "assistant-usage-line"}>{parts.join(" · ")}</p>;
}

/** One-line input that grows with its text; Enter sends, Shift+Enter breaks the line.
 * 随内容增高的单行输入；Enter 发送，Shift+Enter 换行。 */
export function AssistantComposer(props: {
  assistant: AssistantEntry;
  running: boolean;
  // Opens a staged media attachment in the conversation's existing viewer.
  // 在会话的现有查看器打开已暂存媒体附件。
  onOpenMedia?: (media: GeneratedMediaAttachment) => void;
}) {
  const { t } = useI18n();
  const { assistant } = props;
  const [text, setText] = React.useState(() => assistantDrafts.get(assistant.id) ?? "");
  const [pending, setPending] = React.useState(false);
  // Serialize removal with sending and file admission for this compact draft.
  // 在此紧凑草稿中让移除、发送和文件准入互斥。
  const [removing, setRemoving] = React.useState(false);
  const inputRef = React.useRef<HTMLTextAreaElement | null>(null);
  // The compact and full composers submit to the same V1 draft owner.
  // 紧凑输入区与完整输入区向同一个 V1 草稿所有者提交。
  const slot = `conversation:${assistant.id}`;
  // Reuse managed staging and preview generation for originals admitted by Core.
  // 对 Core 准入的原件复用托管暂存和预览生成。
  const staging = useComposerAttachments({
    attachmentCapabilities: assistantPickerCapabilities,
    projectId: assistant.projectId,
    conversationId: assistant.id,
    initialAttachments: assistantAttachmentDrafts.get(assistant.id),
    onStageAttachmentBytes: stageComposerAttachmentBytes,
  });
  // Use the exact managed set when preflighting new files or freezing a send.
  // 新文件预检和冻结发送时使用准确的托管集合。
  const { attachments } = staging;
  const pastedFiles = useComposerDroppedFiles({
    draftSlotKey: slot,
    storageKey: `assistant:${assistant.id}`,
    conversationId: assistant.id,
    projectId: assistant.projectId,
    modelSelection: null,
    routeRevision: "",
    attachments,
    projectAttachments: attachments => attachments,
    stageApprovedFiles: staging.stageApprovedComposerFiles,
    pendingAttachmentCount: staging.pendingAttachmentCount,
    recheckManagedAtSend: true,
    setDragOver: () => undefined,
    flushRoute: async () => !pending && !removing,
  });
  React.useEffect(() => {
    if (attachments.length) assistantAttachmentDrafts.set(assistant.id, attachments);
    else assistantAttachmentDrafts.delete(assistant.id);
  }, [assistant.id, attachments]);
  React.useEffect(() => {
    if (text) assistantDrafts.set(assistant.id, text);
    else assistantDrafts.delete(assistant.id);
  }, [assistant.id, text]);
  const blocked = props.running || pending || removing || pastedFiles.checking || staging.pendingAttachmentCount > 0;
  const send = async () => {
    const sentText = text;
    const prompt = sentText.trim();
    if ((!prompt && !pastedFiles.count && !attachments.length) || blocked) return;
    setPending(true);
    // Keep the pending originals on rejection and release only this attempt's staged lease.
    // 提交被拒绝时保留待发原件，仅释放此尝试的暂存租约。
    let prepared: Awaited<ReturnType<typeof pastedFiles.prepare>> = null;
    let accepted = false;
    try {
      if (pastedFiles.count) {
        prepared = await pastedFiles.prepare(prompt, attachments);
        if (!prepared) return;
      }
      const ack = await submitConversationPrompt(
        createClientOwnedActionId(),
        assistant.projectId,
        assistant.id,
        slot,
        prepared?.prompt ?? prompt,
        undefined,
        prepared?.attachments ?? attachments,
        "build",
        null,
        null,
        null,
        prepared?.planId,
      );
      if (!ack) {
        toast.info(t("conversationActions.previewCannotSend"));
        return;
      }
      accepted = true;
      prepared?.complete();
      staging.setAttachments([]);
      if (ack.queuedTurnDispatch === "waiting") {
        toast.info(t("conversationActions.acceptedAwaitingDispatch"));
      } else if (ack.queuedTurnDispatch === "failed") {
        toast.error(t("conversationActions.acceptedDispatchFailed"));
      }
      // The input stays editable while the receipt is pending, so only what was sent is cleared.
      // 等待回执期间输入框仍可编辑，因此只清除已发送的内容。
      setText((current) => (current === sentText ? "" : current));
    } catch (reason) {
      toast.error(reason instanceof Error ? reason.message : String(reason));
    } finally {
      if (!accepted) await prepared?.rollback();
      setPending(false);
    }
  };
  const stop = async () => {
    try {
      await stopConversationRun(assistant.id, "main_and_subagents");
    } catch (reason) {
      toast.error(reason instanceof Error ? reason.message : String(reason));
    }
  };
  return (
    <div className="assistant-composer" onDragOver={pending || removing ? undefined : pastedFiles.dragOver}
      onDrop={pending || removing ? undefined : event => void pastedFiles.drop(event)}>
      <AssistantAttachmentStrip
        attachments={attachments}
        localTiles={pastedFiles.preview}
        onOpenMedia={props.onOpenMedia}
        onRemove={(index) => {
          if (pending || removing || pastedFiles.checking) return;
          // Keep the current tile until Core has released exactly this draft reference.
          // 在 Core 释放此草稿引用前，保持当前附件块。
          const attachment = attachments[index];
          setRemoving(true);
          void reconcileComposerAttachmentDraftLease(assistant.projectId, assistant.id, slot,
            attachments.filter(item => item !== attachment)).then(
            () => staging.removeComposerAttachment(attachment),
            (error: unknown) => toast.error(error instanceof Error ? error.message : String(error)),
          ).finally(() => setRemoving(false));
        }}
      />
      <div className="assistant-composer-row">
        {/* The desktop shell has no native editing menu, so the input uses the main composer's one. */}
        {/* 桌面壳没有原生编辑菜单，因此输入框使用与主输入区相同的菜单。 */}
        <ComposerTextContextMenu
          disabled={false}
          enabled={isElectronRuntime()}
          onChange={setText}
          ownerKey={`assistant:${assistant.id}`}
          textareaRef={inputRef}
          value={text}
        >
          <Textarea
            aria-label={formatMessage(t("assistantDock.placeholder"), { name: assistant.name })}
            className="assistant-composer-input"
            onChange={(event) => setText(event.target.value)}
            onPaste={pending || removing ? undefined : event => void pastedFiles.paste(event)}
            onKeyDown={(event) => {
              if (event.key !== "Enter" || event.shiftKey || event.nativeEvent.isComposing) return;
              event.preventDefault();
              void send();
            }}
            placeholder={formatMessage(t("assistantDock.placeholder"), { name: assistant.name })}
            ref={inputRef}
            rows={1}
            value={text}
          />
        </ComposerTextContextMenu>
        {props.running ? (
          <Button aria-label={t("assistantDock.stop")} onClick={() => void stop()} size="icon-sm" type="button" variant="secondary">
            <Square size={14} />
          </Button>
        ) : (
          <Button
            aria-label={t("assistantDock.send")}
            disabled={(!text.trim() && !pastedFiles.count && !attachments.length) || blocked}
            onClick={() => void send()}
            size="icon-sm"
            type="button"
          >
            <ArrowUp size={16} />
          </Button>
        )}
      </div>
    </div>
  );
}
