//! Runtime stream provider-loop driver for one prepared launch payload.
//! 面向单个已准备启动载荷的运行时流 Provider 循环驱动模块。

use crate::agent_runtime::events::ExecutionStreamEvent;
use crate::agent_runtime::provider_runtime_dispatch::execute_provider_prompt_stream;
use crate::agent_runtime::provider_transport_request::ProviderTransportTurnStartMode;
use crate::agent_runtime::runtime_stream_delta::process_provider_stream_delta;
use crate::agent_runtime::runtime_stream_finalize::{
    build_failure_terminal_events, build_success_terminal_events,
};
use crate::agent_runtime::runtime_stream_intervention::RuntimePromptIntervention;
use crate::agent_runtime::runtime_stream_launch_payload::{
    RuntimeStreamLaunch, RuntimeStreamTurnStartMode,
};
use crate::model::SessionId;
use crate::provider_config::ProviderConfigurationFile;
use async_stream::stream;
use futures_util::{Stream, StreamExt};
use std::time::Duration;

/// Workbench-facing stream delta flush cadence used to avoid repainting the full workbench for every provider token.
/// 面向工作台的流式增量刷新节奏，用于避免每个 Provider token 都触发整窗重绘。
const RUNTIME_STREAM_WORKBENCH_DELTA_FLUSH_INTERVAL: Duration = Duration::from_millis(50);

/// Accumulates adjacent visible stream deltas while preserving non-delta event ordering.
/// 在保留非增量事件顺序的同时，合并相邻的可见流式增量。
#[derive(Default)]
struct ExecutionStreamDeltaCoalescer {
    /// Pending visible thinking delta and its never-replay summary marker for the current flush window.
    /// 当前刷新窗口内等待发出的可见思考增量及其绝不重放摘要标记。
    pending_thinking_delta: Option<(SessionId, String, bool)>,
    /// Pending visible output delta for the current flush window.
    /// 当前刷新窗口内等待发出的可见输出增量。
    pending_output_delta: Option<(SessionId, String)>,
}

impl ExecutionStreamDeltaCoalescer {
    /// Await the next source item under the text buffer's flush policy.
    /// 按文本缓冲区的刷新策略等待下一条来源事件。
    /// `provider_stream` owns ordered events; `flush_at` is the current buffered-text deadline.
    /// `provider_stream` 持有有序事件；`flush_at` 是当前缓冲文本的刷新期限。
    /// Returns an unchanged item or stream end, or an elapsed deadline for pending text.
    /// 返回未经改写的事件或流结束；有待发文本时也可返回期限到达。
    async fn next_provider_item<S>(
        &self,
        provider_stream: &mut S,
        flush_at: tokio::time::Instant,
    ) -> Result<Option<S::Item>, tokio::time::error::Elapsed>
    where
        S: Stream + Unpin,
    {
        // An idle buffer owns no timer: reusing an elapsed flush deadline would spin until the model replies.
        // 空闲缓冲不持有计时器：复用已过刷新期限会在模型回复前持续空转。
        if self.is_empty() {
            return Ok(provider_stream.next().await);
        }
        tokio::time::timeout_at(flush_at, provider_stream.next()).await
    }

    /// Returns whether no visible delta is waiting for the next workbench flush.
    /// 返回当前是否没有等待下一次工作台刷新的可见增量。
    fn is_empty(&self) -> bool {
        self.pending_thinking_delta.is_none() && self.pending_output_delta.is_none()
    }

    /// Adds one execution event and returns any ordered events that must be emitted before it.
    /// 添加一个执行事件，并返回在它之前必须按顺序发出的事件集合。
    fn push_event(&mut self, event: ExecutionStreamEvent) -> Vec<ExecutionStreamEvent> {
        match event {
            ExecutionStreamEvent::ThinkingDelta {
                session_id,
                delta,
                display_only,
            } => {
                if self.pending_output_delta.is_none()
                    && let Some((pending_session_id, pending_delta, pending_display_only)) =
                        self.pending_thinking_delta.as_mut()
                    && *pending_session_id == session_id
                {
                    pending_delta.push_str(&delta);
                    // A merged window holding any summary text must stay out of replay.
                    // 合并窗口中只要含有摘要文本就必须排除在重放之外。
                    *pending_display_only |= display_only;
                    return Vec::new();
                }

                let flushed_events = self.flush();
                self.pending_thinking_delta = Some((session_id, delta, display_only));
                flushed_events
            }
            ExecutionStreamEvent::OutputDelta { session_id, delta } => {
                if self.pending_thinking_delta.is_none()
                    && let Some((pending_session_id, pending_delta)) =
                        self.pending_output_delta.as_mut()
                    && *pending_session_id == session_id
                {
                    pending_delta.push_str(&delta);
                    return Vec::new();
                }

                let flushed_events = self.flush();
                self.pending_output_delta = Some((session_id, delta));
                flushed_events
            }
            other_event => {
                let mut flushed_events = self.flush();
                flushed_events.push(other_event);
                flushed_events
            }
        }
    }

    /// Drains all pending visible deltas as ordered execution events.
    /// 将所有等待发出的可见增量按顺序提取为执行事件。
    fn flush(&mut self) -> Vec<ExecutionStreamEvent> {
        let mut events = Vec::new();

        if let Some((session_id, delta, display_only)) = self.pending_thinking_delta.take() {
            events.push(ExecutionStreamEvent::ThinkingDelta {
                session_id,
                delta,
                display_only,
            });
        }

        if let Some((session_id, delta)) = self.pending_output_delta.take() {
            events.push(ExecutionStreamEvent::OutputDelta { session_id, delta });
        }

        events
    }
}

/// Drives one prepared runtime-stream launch payload and yields provider delta and terminal events in order.
/// 驱动一份已准备完成的运行时流启动载荷，并按顺序产出 Provider 增量与终态事件。
pub(crate) fn drive_runtime_stream_provider_loop(
    config: ProviderConfigurationFile,
    launch: RuntimeStreamLaunch,
    prompt_intervention: Option<RuntimePromptIntervention>,
) -> impl Stream<Item = ExecutionStreamEvent> + Send + 'static {
    // Box both the state and yielded items; large event payloads otherwise inflate every yield's poll frame.
    // 同时装箱状态与产出事件；大型事件载荷否则会放大每个 yield 的轮询栈帧。
    Box::pin(stream! {
        let session_id = launch.session_id;
        let task_execution_context = launch.task_execution_context;
        let effective_prompt = launch.effective_prompt;
        let turn_start_mode = match launch.turn_start_mode {
            RuntimeStreamTurnStartMode::UserPrompt => ProviderTransportTurnStartMode::UserPrompt,
            RuntimeStreamTurnStartMode::RestoredCheckpoint => {
                ProviderTransportTurnStartMode::RestoredCheckpoint
            }
            RuntimeStreamTurnStartMode::ContinuedProviderTurn => {
                ProviderTransportTurnStartMode::ContinuedProviderTurn
            }
        };
        let preparation = launch.preparation;
        let active_agent = launch.active_agent;
        let task_runtime = launch.task_runtime;
        let tools_manager = launch.tools_manager;
        let resolved_tool_definitions = launch.resolved_tool_definitions;
        let discovery_rows = launch.discovery_rows;
        let discovery_errors = launch.discovery_errors;
        let warnings = launch.warnings;
        let system_preamble = launch.system_preamble;
        let dynamic_prompt_layers = launch.dynamic_prompt_layers;
        // The permit reuses these execution-resolved instructions ahead of the durable baseline.
        // 许可在持久基线前使用本次执行已解析的固定指令。
        let fixed_instruction_layers = launch.fixed_instruction_layers;
        let locked_baseline = launch.locked_baseline;
        let core_snapshot = launch.core_snapshot;
        let mut stream_state = launch.stream_state;
        let public_ai_runtime = launch.public_ai_runtime;
        // taskExecutionContextGuard owns the exact context generation used by this provider loop.
        // taskExecutionContextGuard 拥有本 Provider 循环使用的精确上下文注册世代。
        let task_execution_context_guard = if let Some(context) = task_execution_context {
            match task_runtime.install_execution_context(session_id, context) {
                Ok(guard) => Some(guard),
                Err(error) => {
                    for event in build_failure_terminal_events(
                        session_id,
                        preparation,
                        discovery_rows,
                        discovery_errors,
                        warnings,
                        &mut stream_state,
                        error,
                    ) {
                        yield Box::new(event);
                    }
                    return;
                }
            }
        } else {
            None
        };

        let mut provider_stream = execute_provider_prompt_stream(
            &config,
            &preparation,
            active_agent.as_ref(),
            &system_preamble,
            dynamic_prompt_layers,
            fixed_instruction_layers,
            locked_baseline,
            &effective_prompt,
            turn_start_mode,
            core_snapshot,
            task_runtime,
            tools_manager,
            resolved_tool_definitions,
            public_ai_runtime,
            prompt_intervention,
        );

        let mut delta_coalescer = ExecutionStreamDeltaCoalescer::default();
        let mut next_delta_flush_at =
            tokio::time::Instant::now() + RUNTIME_STREAM_WORKBENCH_DELTA_FLUSH_INTERVAL;

        loop {
            let item =
                match delta_coalescer.next_provider_item(&mut provider_stream, next_delta_flush_at).await {
                Ok(item) => item,
                Err(_) => {
                    if !delta_coalescer.is_empty()
                        && tokio::time::Instant::now() >= next_delta_flush_at
                    {
                        for event in delta_coalescer.flush() {
                            yield Box::new(event);
                        }
                        next_delta_flush_at = tokio::time::Instant::now()
                            + RUNTIME_STREAM_WORKBENCH_DELTA_FLUSH_INTERVAL;
                    }
                    continue;
                }
            };

            let Some(item) = item else {
                break;
            };

            for event in process_provider_stream_delta(session_id, &mut stream_state, item) {
                let was_empty = delta_coalescer.is_empty();
                let flushed_events = delta_coalescer.push_event(event);
                let starts_new_flush_window =
                    (was_empty || !flushed_events.is_empty()) && !delta_coalescer.is_empty();

                for flushed_event in flushed_events {
                    yield Box::new(flushed_event);
                }

                if starts_new_flush_window {
                    next_delta_flush_at = tokio::time::Instant::now()
                        + RUNTIME_STREAM_WORKBENCH_DELTA_FLUSH_INTERVAL;
                }
            }

            if !delta_coalescer.is_empty() && tokio::time::Instant::now() >= next_delta_flush_at {
                for event in delta_coalescer.flush() {
                    yield Box::new(event);
                }
                next_delta_flush_at =
                    tokio::time::Instant::now() + RUNTIME_STREAM_WORKBENCH_DELTA_FLUSH_INTERVAL;
            }

            if stream_state.has_terminal_error() {
                break;
            }
        }

        for event in delta_coalescer.flush() {
            yield Box::new(event);
        }

        // executionContextReleaseResult must succeed before any terminal event claims the loop is closed.
        // executionContextReleaseResult 必须成功，终态事件才能宣称本循环已经关闭。
        let execution_context_release_result = match task_execution_context_guard {
            Some(guard) => guard.release(),
            None => Ok(()),
        };
        if let Err(release_error) = execution_context_release_result {
            // terminalError preserves an existing provider failure while appending the cleanup failure.
            // terminalError 保留既有 Provider 失败，并追加清理失败。
            let terminal_error = match stream_state.take_terminal_error() {
                Some(provider_error) => format!(
                    "{provider_error} Additionally, the task execution context could not be released: {release_error}"
                ),
                None => format!(
                    "The task execution context could not be released after the provider stream ended: {release_error}"
                ),
            };
            for event in build_failure_terminal_events(
                session_id,
                preparation,
                discovery_rows,
                discovery_errors,
                warnings,
                &mut stream_state,
                terminal_error,
            ) {
                yield Box::new(event);
            }
            return;
        }

        if let Some((conversation_state, reason, provider_request_continuation)) =
            stream_state.take_context_compaction_required()
        {
            yield Box::new(ExecutionStreamEvent::ContextCompactionRequired {
                session_id,
                conversation_state,
                reason,
                provider_request_continuation,
            });
            return;
        }

        if let Some((conversation_state, reason, provider_request_continuation)) =
            stream_state.take_context_compaction_policy_refresh_required()
        {
            yield Box::new(ExecutionStreamEvent::ContextCompactionPolicyRefreshRequired {
                session_id,
                conversation_state,
                reason,
                provider_request_continuation,
            });
            return;
        }

        if let Some((conversation_state, reason, provider_request_continuation)) =
            stream_state.take_provider_route_switch_required()
        {
            yield Box::new(ExecutionStreamEvent::ProviderRouteSwitchRequired {
                session_id,
                conversation_state,
                reason,
                provider_request_continuation,
            });
            return;
        }

        if let Some(error) = stream_state.take_terminal_error() {
            for event in build_failure_terminal_events(
                session_id,
                preparation,
                discovery_rows,
                discovery_errors,
                warnings,
                &mut stream_state,
                error,
            ) {
                yield Box::new(event);
            }
            return;
        }

        for event in build_success_terminal_events(
            session_id,
            preparation,
            discovery_rows,
            discovery_errors,
            warnings,
            &mut stream_state,
        ) {
            yield Box::new(event);
        }
    })
    // Restore the existing event API only after the large generator's poll frame has unwound.
    // 大型生成器轮询栈帧退出后，才还原既有事件接口。
    .map(|event| *event)
}

#[cfg(test)]
mod tests {
    //! Runtime stream driver-loop tests for workbench-facing delta coalescing.
    //! 面向工作台增量合并的运行时流驱动循环测试。

    use super::*;
    use crate::agent_runtime::events::{RuntimeEvent, RuntimeEventKind};
    use futures_util::FutureExt;
    use uuid::Uuid;

    /// Keep an idle source pending even when an obsolete text deadline is already elapsed.
    /// 即使过时的文本期限已到达，也保持空闲来源等待。
    /// No parameters or return value; one direct poll detects immediate timeout without hanging a worker.
    /// 无参数及返回值；一次直接轮询检测立即超时，不挂起工作线程。
    #[tokio::test]
    async fn idle_provider_wait_ignores_expired_text_flush_deadline() {
        // Empty text has nothing to flush while a model or tool remains silent.
        // 模型或工具保持静默时，空文本没有任何内容需要刷新。
        let coalescer = ExecutionStreamDeltaCoalescer::default();
        // The source deliberately produces no event and uses no provider or network.
        // 来源明确不产生事件，不使用供应商或网络。
        let mut provider_stream = futures_util::stream::pending::<u8>();
        // The past deadline deterministically exposes the original immediate timeout.
        // 已过期限确定性地暴露原有立即超时。
        let flush_at = tokio::time::Instant::now() - Duration::from_secs(1);
        assert!(
            coalescer
                .next_provider_item(&mut provider_stream, flush_at)
                .now_or_never()
                .is_none(),
            "idle source must remain pending instead of repeatedly timing out"
        );
    }

    /// Preserve the flush deadline when visible text is buffered and the source has no next item.
    /// 存在可见缓冲文本且来源没有下一条事件时，保留刷新期限。
    /// No parameters or return value; requires timeout without consuming or dropping the buffered text.
    /// 无参数及返回值；要求返回超时，但不消费或丢弃缓冲文本。
    #[tokio::test]
    async fn buffered_provider_wait_retains_text_flush_deadline() {
        // The real coalescer owns one pending visible output delta.
        // 真实合并器持有一条待发送的可见输出增量。
        let mut coalescer = ExecutionStreamDeltaCoalescer::default();
        assert!(
            coalescer
                .push_event(ExecutionStreamEvent::OutputDelta {
                    session_id: Uuid::nil(),
                    delta: String::from("buffered text"),
                })
                .is_empty()
        );
        // The source stays silent while the exact buffered-text deadline has elapsed.
        // 精确缓冲文本期限到达时，来源仍保持静默。
        let mut provider_stream = futures_util::stream::pending::<u8>();
        assert!(
            coalescer
                .next_provider_item(
                    &mut provider_stream,
                    tokio::time::Instant::now() - Duration::from_secs(1),
                )
                .await
                .is_err()
        );
        assert!(!coalescer.is_empty());
        assert!(matches!(coalescer.flush().as_slice(),
            [ExecutionStreamEvent::OutputDelta { delta, .. }] if delta == "buffered text"));
    }

    /// Forward ready items and end-of-stream unchanged with either an empty or populated text buffer.
    /// 无论文本缓冲为空或非空，都原样转发就绪事件和流结束。
    /// No parameters or return value; assertions exercise the same waiting method as the live driver.
    /// 无参数及返回值；断言执行与实际驱动相同的等待方法。
    #[tokio::test]
    async fn provider_wait_preserves_ready_items_and_stream_end() {
        for buffered in [false, true] {
            // Each case owns independent coalescing state and a single exact source item.
            // 每个用例独立持有合并状态及一条精确来源事件。
            let mut coalescer = ExecutionStreamDeltaCoalescer::default();
            if buffered {
                assert!(
                    coalescer
                        .push_event(ExecutionStreamEvent::ThinkingDelta {
                            session_id: Uuid::nil(),
                            delta: String::from("pending reasoning"),
                            display_only: false,
                        })
                        .is_empty()
                );
            }
            // Ready input must win without being replaced by an expired flush marker.
            // 就绪输入必须直接返回，不能被过期刷新标记替换。
            let mut provider_stream = futures_util::stream::iter([7_u8]);
            // Reuse the deadline to prove neither ready input nor EOF requires advancing time.
            // 复用期限以证明就绪输入和流结束均不要求推进时间。
            let flush_at = tokio::time::Instant::now() - Duration::from_secs(1);
            assert_eq!(
                coalescer
                    .next_provider_item(&mut provider_stream, flush_at)
                    .await
                    .expect("ready item"),
                Some(7)
            );
            assert_eq!(
                coalescer
                    .next_provider_item(&mut provider_stream, flush_at)
                    .await
                    .expect("ready stream end"),
                None
            );
            assert_eq!(coalescer.is_empty(), !buffered);
        }
    }

    /// Verifies adjacent output deltas for the same session are merged into one workbench event.
    /// 验证同一会话的相邻输出增量会合并为一个工作台事件。
    #[test]
    fn coalescer_merges_adjacent_output_deltas_for_same_session() {
        let session_id = Uuid::nil();
        let mut coalescer = ExecutionStreamDeltaCoalescer::default();

        assert!(
            coalescer
                .push_event(ExecutionStreamEvent::OutputDelta {
                    session_id,
                    delta: String::from("hello"),
                })
                .is_empty()
        );
        assert!(
            coalescer
                .push_event(ExecutionStreamEvent::OutputDelta {
                    session_id,
                    delta: String::from(" world"),
                })
                .is_empty()
        );

        let flushed_events = coalescer.flush();
        assert_eq!(flushed_events.len(), 1);
        assert!(matches!(
            &flushed_events[0],
            ExecutionStreamEvent::OutputDelta { delta, .. } if delta == "hello world"
        ));
    }

    /// Verifies a merged thinking window keeps the never-replay marker of any summary inside it.
    /// 验证合并后的思考窗口保留其中任一摘要的绝不重放标记。
    #[test]
    fn coalescer_keeps_display_only_marker_across_merged_thinking() {
        let session_id = Uuid::nil();
        let mut coalescer = ExecutionStreamDeltaCoalescer::default();
        for (delta, display_only) in [("model", false), (" summary", true)] {
            assert!(
                coalescer
                    .push_event(ExecutionStreamEvent::ThinkingDelta {
                        session_id,
                        delta: String::from(delta),
                        display_only,
                    })
                    .is_empty()
            );
        }

        assert!(matches!(
            coalescer.flush().as_slice(),
            [ExecutionStreamEvent::ThinkingDelta { delta, display_only: true, .. }]
                if delta == "model summary"
        ));
    }

    /// Verifies non-delta events force pending text to flush before the structural event is emitted.
    /// 验证非增量事件会先冲刷待发文本，再按顺序发出结构化事件。
    #[test]
    fn coalescer_flushes_pending_output_before_runtime_event() {
        let session_id = Uuid::nil();
        let mut coalescer = ExecutionStreamDeltaCoalescer::default();

        assert!(
            coalescer
                .push_event(ExecutionStreamEvent::OutputDelta {
                    session_id,
                    delta: String::from("partial"),
                })
                .is_empty()
        );

        let flushed_events = coalescer.push_event(ExecutionStreamEvent::RuntimeEvent {
            session_id,
            event: RuntimeEvent::new(RuntimeEventKind::ToolStarted, "tool", "started"),
        });

        assert_eq!(flushed_events.len(), 2);
        assert!(matches!(
            &flushed_events[0],
            ExecutionStreamEvent::OutputDelta { delta, .. } if delta == "partial"
        ));
        assert!(matches!(
            &flushed_events[1],
            ExecutionStreamEvent::RuntimeEvent { event, .. }
                if event.kind == RuntimeEventKind::ToolStarted
        ));
    }
}
