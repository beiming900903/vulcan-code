//! Service bootstrap startup snapshot helpers.
//! 服务引导 startup snapshot 辅助逻辑。

use super::*;

pub(crate) fn startup_snapshot_with_phase(
    phase: &str,
    steps: Vec<StartupStepSnapshot>,
    logs: &[String],
) -> StartupSnapshot {
    StartupSnapshot {
        ready: phase == "ready",
        ui_locale: None,
        phase: phase.to_string(),
        steps: if steps.is_empty() {
            startup_steps_with_status(
                OffsetDateTime::now_utc(),
                &[
                    (
                        "service-process",
                        StartupStepStatus::Pending,
                        "Waiting for Core Service to start",
                    ),
                    (
                        "data-store",
                        StartupStepStatus::Pending,
                        "Waiting for initialization",
                    ),
                    (
                        "luaskills",
                        StartupStepStatus::Pending,
                        "Waiting for initialization",
                    ),
                    (
                        "mcp",
                        StartupStepStatus::Pending,
                        "Waiting for initialization",
                    ),
                    (
                        "plugins",
                        StartupStepStatus::Pending,
                        "Waiting for initialization",
                    ),
                    (
                        "workspace",
                        StartupStepStatus::Pending,
                        "Waiting for initialization",
                    ),
                ],
            )
        } else {
            steps
        },
        recent_logs: logs.to_vec(),
        model_reconciliation_notices: Vec::new(),
    }
}

/// Builds ordered startup steps from compact tuples.
/// 从紧凑元组构建有序启动步骤。
pub(crate) fn startup_steps_with_status(
    updated_at: OffsetDateTime,
    compact_steps: &[(&str, StartupStepStatus, &str)],
) -> Vec<StartupStepSnapshot> {
    compact_steps
        .iter()
        .map(|(id, status, message)| StartupStepSnapshot {
            id: (*id).to_string(),
            label: startup_step_label(id),
            status: *status,
            message: Some((*message).to_string()),
            progress_percent: None,
            completed_bytes: None,
            total_bytes: None,
            progress_phase: None,
            skill_id: None,
            updated_at: Some(updated_at),
        })
        .collect()
}

/// Resolves the user-facing label for one startup step id.
/// 解析单个启动步骤标识对应的用户可见名称。
pub(crate) fn startup_step_label(id: &str) -> String {
    match id {
        "service-process" => "Core Service",
        "data-store" => "Data store",
        "luaskills" => "LuaSkills",
        "mcp" => "MCP Runtime",
        "plugins" => "Plugin Runtime",
        "providers" => "Model Providers",
        "memory" => "Memory",
        "agent-runtime" => "Agent Runtime",
        "workspace" => "Workspace",
        _ => id,
    }
    .to_string()
}
