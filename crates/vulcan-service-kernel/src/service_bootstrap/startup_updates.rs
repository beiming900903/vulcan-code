//! Service bootstrap startup updates helpers.
//! 服务引导 startup updates 辅助逻辑。

use super::*;
use luaskills::{
    RuntimeSkillOperationProgressCallback, RuntimeSkillOperationProgressEvent, SkillOperationPlane,
    clear_skill_operation_progress_callback_if, set_skill_operation_progress_callback,
};

/// Owns LuaSkills download progress only while the startup installer is running.
/// 仅在启动安装器运行期间持有 LuaSkills 下载进度。
pub(crate) struct StartupSkillDownloads(RuntimeSkillOperationProgressCallback);

impl StartupSkillDownloads {
    /// Connects ROOT archive downloads to this Core's startup snapshot and returns a callback guard.
    /// 将 ROOT 归档下载接入当前 Core 的启动快照，并返回回调守卫。
    pub(crate) fn install(state: &SharedServiceBridgeState, emitter: &ServiceEventEmitter) -> Self {
        // Keep only host-owned startup inputs; upstream locators and diagnostic text never cross this boundary.
        // 仅保留宿主持有的启动输入，上游来源地址和诊断文本不跨过此边界。
        let state = Arc::clone(state);
        let emitter = emitter.clone();
        // Use the same process-wide slot as the workbench progress owner, at a disjoint earlier stage.
        // 使用与工作台进度所有者相同的进程级槽位，但仅在更早的不重叠阶段持有。
        let callback: RuntimeSkillOperationProgressCallback = Arc::new(move |event| {
            update_startup_skill_download(&state, &emitter, event);
        });
        set_skill_operation_progress_callback(Some(Arc::clone(&callback)));
        Self(callback)
    }
}

impl Drop for StartupSkillDownloads {
    /// Clears this callback by identity without removing a later workbench owner.
    /// 按身份清除此回调，避免移除后续工作台所有者。
    fn drop(&mut self) {
        clear_skill_operation_progress_callback_if(&self.0);
    }
}

/// Projects real byte progress for a ROOT skill archive onto the current startup step.
/// 将 ROOT 技能归档的真实字节进度投影到当前启动步骤。
fn update_startup_skill_download(
    state: &SharedServiceBridgeState,
    emitter: &ServiceEventEmitter,
    event: &RuntimeSkillOperationProgressEvent,
) {
    if event.plane != SkillOperationPlane::System
        || event.phase != "downloading_archive"
        || event.status != "progress"
    {
        return;
    }
    // The upstream download emitter provides the skill identity independently of the source locator.
    // 上游下载发射器提供的技能标识独立于来源地址。
    let Some(skill_id) = event.skill_id.as_deref() else {
        return;
    };
    // Replace only the live LuaSkills row, preserving earlier startup stages and diagnostics.
    // 只替换正在运行的 LuaSkills 行，保留先前的启动步骤和诊断。
    {
        let Ok(mut guard) = state.lock() else { return };
        if guard.startup.phase != "bootstrapping-luaskills" {
            return;
        }
        let Some(step) = guard
            .startup
            .steps
            .iter_mut()
            .find(|step| step.id == "luaskills" && step.status == StartupStepStatus::Running)
        else {
            return;
        };
        // An absent or zero length remains indeterminate even when bytes have arrived.
        // 总量缺失或为零时，即使收到字节也保持不定进度。
        let total = event.bytes_total.filter(|total| *total > 0);
        let percent = event
            .bytes_done
            .zip(total)
            .map(|(done, total)| ((u128::from(done.min(total)) * 100) / u128::from(total)) as u8);
        // Only the initial archive sample needs an event; every sample is available through RPC.
        // 只有首个归档进度需要事件，每个进度样本都可通过 RPC 读取。
        let download_started = step.skill_id.as_deref() != Some(skill_id);
        step.message = Some(format!("Downloading LuaSkills skill: {skill_id}"));
        step.progress_phase = Some(String::from("downloading"));
        step.skill_id = Some(skill_id.to_owned());
        step.completed_bytes = event.bytes_done;
        step.total_bytes = total;
        step.progress_percent = percent;
        step.updated_at = Some(OffsetDateTime::now_utc());
        // Startup clients query the retained snapshot; emit only when a new download begins.
        // 启动客户端读取保留快照，仅在新下载开始时推送事件。
        if !download_started {
            return;
        }
        let snapshot = guard.startup.clone();
        drop(guard);
        emit_service_event_with_trace(
            emitter,
            "startup_snapshot_emit",
            ServiceEvent::Startup(Box::new(snapshot)),
        );
    }
}

pub(crate) fn update_startup<F>(
    state: &SharedServiceBridgeState,
    event_emitter: &ServiceEventEmitter,
    phase: &str,
    logs: &[String],
    build_steps: F,
) where
    F: FnOnce(OffsetDateTime) -> Vec<StartupStepSnapshot>,
{
    let mut startup =
        startup_snapshot_with_phase(phase, build_steps(OffsetDateTime::now_utc()), logs);
    if let Ok(mut guard) = state.lock() {
        startup.ui_locale = guard.startup.ui_locale.clone();
        startup.model_reconciliation_notices = guard.startup.model_reconciliation_notices.clone();
        guard.startup = startup.clone();
    }
    emit_service_event_with_trace(
        event_emitter,
        "startup_snapshot_emit",
        ServiceEvent::Startup(Box::new(startup)),
    );
}

/// Publishes the active runtime stage after LuaSkills and plugin hooks have finished.
/// LuaSkills 与插件 Hook 完成后发布当前活动的运行时阶段。
/// The earlier LuaSkills outcome is preserved and later steps stay pending until reached.
/// 保留前序 LuaSkills 的结果，后续步骤在到达前保持等待。
pub(crate) fn advance_runtime_startup(
    state: &SharedServiceBridgeState,
    emitter: &ServiceEventEmitter,
    logs: &[String],
    luaskills: (StartupStepStatus, &str),
    active: &str,
    message: &str,
) {
    // Keep successful stages and the active label in the same canonical order.
    // 以同一规范顺序排列成功阶段与活动标签。
    let phase = format!("bootstrapping-{active}");
    update_startup(state, emitter, &phase, logs, |now| {
        // Each stage before the active one has passed its synchronous initialization boundary.
        // 活动阶段之前的每个阶段均已通过同步初始化边界。
        let stages = ["mcp", "providers", "memory", "agent-runtime", "workspace"];
        let mut steps = startup_steps_with_status(
            now,
            &[
                (
                    "service-process",
                    StartupStepStatus::Ready,
                    "Core Service started",
                ),
                (
                    "data-store",
                    StartupStepStatus::Ready,
                    "Application data store ready",
                ),
                ("luaskills", luaskills.0, luaskills.1),
                (
                    "plugins",
                    StartupStepStatus::Ready,
                    "Plugin catalog and enabled runtimes ready",
                ),
            ],
        );
        // Retain the exact position at which the remaining stages start waiting.
        // 保留其余阶段开始等待的确切位置。
        let mut waiting = false;
        for id in stages {
            let (status, detail) = if id == active {
                waiting = true;
                (StartupStepStatus::Running, message.to_owned())
            } else if waiting {
                (
                    StartupStepStatus::Pending,
                    String::from("Waiting for startup dependencies"),
                )
            } else {
                (StartupStepStatus::Ready, startup_ready_message(id))
            };
            steps.extend(startup_steps_with_status(now, &[(id, status, &detail)]));
        }
        steps
    });
}

/// Emits a LuaSkills-specific startup progress snapshot.
/// 发出 LuaSkills 专用启动进度快照。
pub(crate) fn update_luaskills_bootstrap_progress(
    state: &SharedServiceBridgeState,
    event_emitter: &ServiceEventEmitter,
    logs: &[String],
    message: &str,
) {
    update_startup(
        state,
        event_emitter,
        "bootstrapping-luaskills",
        logs,
        |now| {
            startup_steps_with_status(
                now,
                &[
                    (
                        "service-process",
                        StartupStepStatus::Ready,
                        "Core Service started",
                    ),
                    (
                        "data-store",
                        StartupStepStatus::Ready,
                        "Application data store ready",
                    ),
                    ("luaskills", StartupStepStatus::Running, message),
                    (
                        "mcp",
                        StartupStepStatus::Pending,
                        "Waiting for LuaSkills to be ready",
                    ),
                    (
                        "plugins",
                        StartupStepStatus::Pending,
                        "Waiting for the plugin catalog initialization",
                    ),
                    (
                        "workspace",
                        StartupStepStatus::Pending,
                        "Waiting for the workspace database to load",
                    ),
                ],
            )
        },
    );
}

/// Emits exact native managed-runtime download progress during LuaSkills bootstrap.
/// 在 LuaSkills 初始化期间发出精确的原生受管运行时下载进度。
pub(crate) fn update_managed_runtime_bootstrap_progress(
    state: &SharedServiceBridgeState,
    event_emitter: &ServiceEventEmitter,
    logs: &[String],
    progress: &vulcan_service_runtime::managed_runtime_bootstrap::ManagedRuntimeBootstrapProgress,
) {
    update_startup(
        state,
        event_emitter,
        "bootstrapping-luaskills",
        logs,
        |now| {
            let mut steps = startup_steps_with_status(
                now,
                &[
                    (
                        "service-process",
                        StartupStepStatus::Ready,
                        "Core Service started",
                    ),
                    (
                        "data-store",
                        StartupStepStatus::Ready,
                        "Application data store ready",
                    ),
                    (
                        "luaskills",
                        StartupStepStatus::Running,
                        progress.message.as_str(),
                    ),
                    (
                        "mcp",
                        StartupStepStatus::Pending,
                        "Waiting for LuaSkills to be ready",
                    ),
                    (
                        "plugins",
                        StartupStepStatus::Pending,
                        "Waiting for the plugin catalog initialization",
                    ),
                    (
                        "workspace",
                        StartupStepStatus::Pending,
                        "Waiting for the workspace database to load",
                    ),
                ],
            );
            if let Some(step) = steps.iter_mut().find(|step| step.id == "luaskills") {
                step.progress_percent = progress.overall_percent;
                step.completed_bytes = progress.completed_bytes;
                step.total_bytes = progress.total_bytes;
                step.progress_phase = Some(progress.phase.clone());
            }
            steps
        },
    );
}

/// Updates the shared service state after a startup failure.
/// 在启动失败后更新共享服务状态。
pub(crate) fn update_service_failure(
    state: &SharedServiceBridgeState,
    event_emitter: &ServiceEventEmitter,
    error: ServiceBootstrapError,
) {
    let logs = vec![
        format!(
            "[webui-service] startup failed at {}: {}",
            error.step_label, error.message
        ),
        format!(
            "[webui-service] workspace root: {}",
            normalized_workspace_path_text_from_path(&workspace_root())
        ),
    ];
    tracing::error!("{}", logs[0]);
    let failure = error.service_status_failure();
    let mut startup = startup_snapshot_with_phase(
        "failed",
        startup_failure_steps(OffsetDateTime::now_utc(), &error),
        &logs,
    );
    let mut workbench = fallback_workbench_snapshot(&logs);
    if let Ok(mut guard) = state.lock() {
        startup.ui_locale = guard.startup.ui_locale.clone();
        startup.model_reconciliation_notices = guard.startup.model_reconciliation_notices.clone();
        workbench.ui_locale = guard.startup.ui_locale.clone();
        guard.startup = startup.clone();
        crate::preserve_project_settings_revisions(&mut workbench, &guard.workbench);
        guard.workbench_generation = guard.workbench_generation.wrapping_add(1);
        workbench.revision = guard.workbench_generation;
        guard.workbench = workbench.clone();
        guard.service_lifecycle_state.last_bootstrap_failure = Some(failure);
    }
    mark_webui_service_discovery_failed(state);
    emit_service_snapshots(event_emitter, &startup, &workbench);
}

/// Builds ordered startup steps for one scoped bootstrap failure.
/// 为单个带阶段的引导失败构建有序启动步骤。
pub(crate) fn startup_failure_steps(
    updated_at: OffsetDateTime,
    error: &ServiceBootstrapError,
) -> Vec<StartupStepSnapshot> {
    let step_order = [
        "service-process",
        "data-store",
        "luaskills",
        "plugins",
        "mcp",
        "providers",
        "memory",
        "agent-runtime",
        "workspace",
    ];
    let failed_index = step_order
        .iter()
        .position(|step_id| *step_id == error.step_id)
        .unwrap_or(0);
    step_order
        .iter()
        .enumerate()
        .map(|(index, step_id)| {
            let (status, message) = if *step_id == error.step_id {
                (StartupStepStatus::Error, error.message.clone())
            } else if index < failed_index {
                (StartupStepStatus::Ready, startup_ready_message(step_id))
            } else {
                (
                    StartupStepStatus::Pending,
                    String::from("An upstream startup step failed; not continued"),
                )
            };
            StartupStepSnapshot {
                id: (*step_id).to_string(),
                label: startup_step_label(step_id),
                status,
                message: Some(message),
                progress_percent: None,
                completed_bytes: None,
                total_bytes: None,
                progress_phase: None,
                skill_id: None,
                updated_at: Some(updated_at),
            }
        })
        .collect()
}

/// Returns the stable ready message for one startup step.
/// 返回单个启动步骤的稳定就绪文案。
pub(crate) fn startup_ready_message(step_id: &str) -> String {
    match step_id {
        "service-process" => "Core Service started",
        "data-store" => "Application data store ready",
        "luaskills" => "LuaSkills engine and skill catalog ready",
        "mcp" => "MCP runtime initialized",
        "plugins" => "Plugin catalog and enabled runtimes ready",
        "providers" => "Provider model catalogs ready",
        "memory" => "Memory initialization finished",
        "agent-runtime" => "Agent runtime ready",
        "workspace" => "Project and session index loaded",
        _ => "Ready",
    }
    .to_string()
}

/// Emits startup and workbench snapshots to subscribed WebUI clients.
/// 向已订阅的 WebUI 客户端发送启动与工作台快照。
pub(crate) fn emit_service_snapshots(
    event_emitter: &ServiceEventEmitter,
    startup: &StartupSnapshot,
    workbench: &WorkbenchSnapshot,
) {
    emit_service_event_with_trace(
        event_emitter,
        "startup_snapshot_emit",
        ServiceEvent::Startup(Box::new(startup.clone())),
    );
    emit_service_event_with_trace(
        event_emitter,
        "service_workbench_snapshot_emit",
        ServiceEvent::Workbench(Box::new(workbench.clone())),
    );
}
