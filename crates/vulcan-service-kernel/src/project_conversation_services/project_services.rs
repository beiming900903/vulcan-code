//! Project list service handlers.
//! 项目列表服务处理器。

use super::conversation_services::{
    conversation_archived_delta, conversations_archived_delta_envelope,
};
use super::*;
use crate::project_terminals::{cancel_project_terminal_displays, project_terminal_runtime};

/// Returns the current Unix millisecond timestamp within the signed V1 lifecycle contract.
/// 返回符合有符号 V1 生命周期契约的当前 Unix 毫秒时间戳。
///
/// # Returns / 返回值
///
/// Returns the exact signed timestamp or an explicit range failure.
/// 返回精确的有符号时间戳，或显式的范围错误。
fn current_project_lifecycle_unix_millis() -> Result<i64, String> {
    i64::try_from(current_unix_millis_value())
        .map_err(|_| String::from("Project lifecycle timestamp exceeds the signed V1 range."))
}

/// Minimal retained project state needed to validate and persist one layout mutation.
/// 校验并持久化单次布局变更所需的最小保留项目状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RetainedProjectLayoutEntry {
    /// Exact active project identity retained by the workspace snapshot.
    /// 工作区快照保留的精确活动项目身份。
    pub(super) id: String,
    /// Current persisted pinned section retained by the workspace snapshot.
    /// 工作区快照保留的当前持久化置顶分区。
    pub(super) pinned: bool,
}

/// Complete ordered identities for both active project sections.
/// 两个活动项目分区的完整有序身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProjectLayoutIds {
    /// Complete ordered pinned-section project identities.
    /// 完整有序的置顶分区项目身份。
    pub(super) pinned: Vec<String>,
    /// Complete ordered regular-section project identities.
    /// 完整有序的普通分区项目身份。
    pub(super) regular: Vec<String>,
}

/// Builds the complete resulting layout for one exact pin transition.
/// 为一次精确置顶转换构建完整结果布局。
///
/// # Parameters
/// 参数
/// `projects` contains the retained active projects in current display order.
/// `projects` 包含按当前展示顺序排列的保留活动项目。
/// `project_id` is the exact project whose pinned state must change.
/// `project_id` 是必须改变置顶状态的精确项目。
///
/// # Returns
/// 返回值
/// Returns the resulting target pinned state and complete two-section layout.
/// 返回目标的结果置顶状态与完整双分区布局。
pub(super) fn project_layout_after_pin_toggle(
    projects: &[RetainedProjectLayoutEntry],
    project_id: &str,
) -> Result<(bool, ProjectLayoutIds), String> {
    // current_project proves the retained snapshot still exposes the requested active identity.
    // current_project 证明保留快照仍公开所请求的活动身份。
    let current_project = projects
        .iter()
        .find(|project| project.id == project_id)
        .ok_or_else(|| format!("Project to toggle the pin state not found: {project_id}"))?;
    // next_pinned is the exact result state persisted and applied to both retained projections.
    // next_pinned 是持久化并应用到两份保留投影的精确结果状态。
    let next_pinned = !current_project.pinned;
    // layout retains current display order while moving only the target between sections.
    // layout 保留当前展示顺序，同时只把目标移动到另一个分区。
    let mut layout = ProjectLayoutIds {
        pinned: Vec::new(),
        regular: Vec::new(),
    };
    for project in projects {
        // resulting_pinned differs from the snapshot only for the requested target.
        // resulting_pinned 仅对请求目标与快照状态不同。
        let resulting_pinned = if project.id == project_id {
            next_pinned
        } else {
            project.pinned
        };
        if resulting_pinned {
            layout.pinned.push(project.id.clone());
        } else {
            layout.regular.push(project.id.clone());
        }
    }
    Ok((next_pinned, layout))
}

/// Validates one complete same-section reorder and builds both resulting sections.
/// 校验一次完整同分区重排并构建两个结果分区。
///
/// # Parameters
/// 参数
/// `projects` contains the retained active projects in current display order.
/// `projects` 包含按当前展示顺序排列的保留活动项目。
/// `ordered_project_ids` is the caller-supplied complete order for one section.
/// `ordered_project_ids` 是调用方提供的单个分区完整顺序。
///
/// # Returns
/// 返回值
/// Returns both complete resulting sections or rejects partial, duplicate, missing, or cross-section identities.
/// 返回两个完整结果分区，或拒绝局部、重复、缺失及跨分区身份。
fn project_layout_after_section_reorder(
    projects: &[RetainedProjectLayoutEntry],
    ordered_project_ids: &[String],
) -> Result<ProjectLayoutIds, String> {
    // first_project establishes the only section accepted by this reorder request.
    // first_project 确立本次重排请求唯一允许的分区。
    let first_project_id = ordered_project_ids.first().ok_or_else(|| {
        String::from("Failed to reorder projects: the project order must not be empty.")
    })?;
    let first_project = projects
        .iter()
        .find(|project| project.id == *first_project_id)
        .ok_or_else(|| {
            String::from("Failed to reorder projects: the first project does not exist.")
        })?;
    // expected_section_ids is the complete authoritative retained membership for the selected section.
    // expected_section_ids 是所选分区完整的权威保留成员集合。
    let expected_section_ids = projects
        .iter()
        .filter(|project| project.pinned == first_project.pinned)
        .map(|project| project.id.as_str())
        .collect::<BTreeSet<_>>();
    // ordered_section_ids rejects duplicates before set equality is evaluated.
    // ordered_section_ids 在判断集合相等前拒绝重复身份。
    let ordered_section_ids = ordered_project_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if ordered_section_ids.len() != ordered_project_ids.len() {
        return Err(String::from(
            "Failed to reorder projects: the project order contains duplicate identifiers.",
        ));
    }
    if ordered_section_ids != expected_section_ids {
        return Err(String::from(
            "Failed to reorder projects: the complete project set of the same section must be submitted.",
        ));
    }
    // opposite_section_ids retain the untouched section in its current display order.
    // opposite_section_ids 按当前展示顺序保留未触碰的另一个分区。
    let opposite_section_ids = projects
        .iter()
        .filter(|project| project.pinned != first_project.pinned)
        .map(|project| project.id.clone())
        .collect::<Vec<_>>();
    if first_project.pinned {
        Ok(ProjectLayoutIds {
            pinned: ordered_project_ids.to_vec(),
            regular: opposite_section_ids,
        })
    } else {
        Ok(ProjectLayoutIds {
            pinned: opposite_section_ids,
            regular: ordered_project_ids.to_vec(),
        })
    }
}

/// Invalidates every pending persistence channel owned by one retained project.
/// 使一个保留项目拥有的全部待处理持久化通道失效。
///
/// # Parameters
/// 参数
/// `state` owns all WebUI session persistence version tables.
/// `state` 持有全部 WebUI 会话持久化版本表。
/// `project` contains the exact retained session membership being transitioned.
/// `project` 包含正在转换的精确保留会话成员关系。
///
/// # Returns
/// 返回值
/// Returns success after every project session receives a lifecycle persistence fence.
/// 每个项目会话都获得生命周期持久化隔离后返回成功。
/// Begins one VMM lifecycle fence for every session owned by a transitioning project.
/// 为正在转换的项目所拥有的每个会话启动 VMM 生命周期栅栏。
///
/// # Parameters
/// 参数
/// `state` owns the process-wide VMM controller registry.
/// `state` 持有进程级 VMM 控制器注册表。
/// `project` contains the exact session membership about to enter a storage transition.
/// `project` 包含即将进入存储转换的精确会话成员关系。
///
/// # Returns
/// 返回值
/// One RAII fence blocking every project session controller through the storage transition.
/// 返回一个在存储转换期间阻止全部项目会话控制器的 RAII 栅栏。
fn begin_project_session_vmm_lifecycle_transition(
    state: &SharedServiceBridgeState,
    project: &TaskProjectSummary,
) -> Result<crate::vmm_session_controller::SessionVmmLifecycleFence, String> {
    // registry is cloned so lifecycle fencing never nests the shared service lock.
    // registry 在克隆后执行生命周期栅栏化，避免嵌套共享服务锁。
    let registry = state
        .lock()
        .map_err(|error| format!("failed to lock service state: {error}"))?
        .vmm_controller_registry
        .clone();
    registry.begin_lifecycle_transition(project.session_ids.iter().copied())
}

/// Opens one project directory in the host system file manager.
/// 在宿主系统文件管理器中打开单个项目文件夹。
pub(crate) fn service_open_project_directory(
    state: SharedServiceBridgeState,
    project_id: String,
) -> Result<String, String> {
    let shell_host = shell_host_from_state(&state)?;
    let project = state
        .lock()
        .map_err(|error| format!("failed to lock service state: {error}"))?
        .workbench
        .projects
        .iter()
        .find(|project| project.id == project_id)
        .cloned()
        .ok_or_else(|| format!("project not found: {project_id}"))?;
    shell_host.open_path(&project.path)?;
    Ok(format!(
        "Requested opening the project directory: {}.",
        project.name
    ))
}

/// Opens one arbitrary project directory path in the host system file manager.
/// 在宿主系统文件管理器中打开任意项目目录路径。
pub(crate) fn service_open_project_directory_path(
    state: SharedServiceBridgeState,
    path: String,
) -> Result<String, String> {
    shell_host_from_state(&state)?.open_path(&path)?;
    Ok(format!("Requested opening the directory: {path}."))
}

/// Returns the best initial directory for the native project folder picker.
/// 返回原生项目文件夹选择器使用的最佳初始目录。
///
/// # Parameters
/// 参数
/// `state` is the shared service state that owns the current workbench snapshot.
/// `state` 是持有当前工作台快照的共享服务状态。
///
/// # Returns
/// 返回值
/// A normalized directory path string suitable for the host folder picker.
/// 返回适合宿主文件夹选择器使用的标准化目录路径字符串。
pub fn service_add_project_initial_directory(
    state: &SharedServiceBridgeState,
) -> Result<String, String> {
    Ok(state
        .lock()
        .map_err(|error| format!("failed to lock service state: {error}"))?
        .workbench
        .projects
        .first()
        .map(|project| project_directory_parent_or_self(&project.path))
        .unwrap_or_else(default_project_initial_directory))
}

/// Returns the picker start directory for one existing project path: the folder holding the
/// project so a sibling project is one click away, or the project directory itself when it has
/// no usable parent.
/// 为单个已有项目路径返回选择器起始目录：容纳该项目的上级目录，使同级新项目一键可达；
/// 没有可用上级目录时返回项目目录本身。
///
/// # Parameters
/// 参数
/// `project_path` is the normalized stored path of an existing project.
/// `project_path` 是已有项目存储的规范化路径。
///
/// # Returns
/// 返回值
/// A normalized directory path string suitable for the host folder picker.
/// 返回适合宿主文件夹选择器使用的标准化目录路径字符串。
fn project_directory_parent_or_self(project_path: &str) -> String {
    let path = std::path::Path::new(project_path);
    normalized_workspace_path_text_from_path(
        path.parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(path),
    )
}

/// Returns the picker start directory used before any project exists: the user home directory,
/// or the service workspace root when no home directory can be resolved.
/// 返回尚无任何项目时选择器使用的起始目录：用户主目录；无法解析主目录时回退为服务工作区根。
///
/// # Returns
/// 返回值
/// A normalized directory path string suitable for the host folder picker.
/// 返回适合宿主文件夹选择器使用的标准化目录路径字符串。
fn default_project_initial_directory() -> String {
    normalized_workspace_path_text_from_path(
        &vulcan_runtime_assets::user_home_dir().unwrap_or_else(workspace_root),
    )
}

/// Toggles one project pinned state without rebuilding the full workbench snapshot.
/// 切换单个项目置顶状态，但不重建完整工作台快照。
pub(crate) async fn service_toggle_project_pinned(
    state: SharedServiceBridgeState,
    project_id: String,
) -> Result<(), String> {
    // retained_layout captures the exact active membership and current pin states before persistence.
    // retained_layout 在持久化前捕获精确活动成员关系与当前置顶状态。
    let (controller_runtime, retained_layout) =
        controller_runtime_and_project_layout_from_state(&state)?;
    // next_pinned and layout describe the single transition plus both complete resulting sections.
    // next_pinned 与 layout 描述单次转换以及两个完整结果分区。
    let (next_pinned, layout) = project_layout_after_pin_toggle(&retained_layout, &project_id)?;
    vulcan_workspace_store::workspace_store::persist_project_pinned_and_order_update(
        controller_runtime.clone(),
        &project_id,
        next_pinned,
        &layout.pinned,
        &layout.regular,
    )
    .await?;
    // The project tree orders the pinned section, so a new pin joins its end and an old one leaves it.
    // 项目树负责排列置顶分区，因此新置顶加入其末尾，取消的置顶离开该分区。
    let mut tree = vulcan_workspace_store::workspace_store::load_project_tree(&controller_runtime)?;
    tree.set_pinned(
        vulcan_workspace_store::workspace_store::ProjectTreePinKind::Project,
        &project_id,
        next_pinned,
    );
    vulcan_workspace_store::workspace_store::persist_project_tree(controller_runtime, &tree)
        .await?;
    // The refreshed snapshot applies both the new section and every recomputed sort order.
    // 刷新后的快照会同时应用新分区以及每个重新计算的顺序。
    let _refreshed_snapshot = refresh_workbench_snapshot(state).await?;
    Ok(())
}

/// Reorders visible projects inside one pinned or regular section.
/// 在同一个置顶或普通分区内重排可见项目。
pub(crate) async fn service_reorder_projects(
    state: SharedServiceBridgeState,
    project_ids: Vec<String>,
) -> Result<WorkbenchSnapshot, String> {
    if project_ids.is_empty() {
        return refresh_workbench_snapshot(state).await;
    }

    // retained_layout captures both complete sections before validating the caller order.
    // retained_layout 在校验调用方顺序前捕获两个完整分区。
    let (controller_runtime, retained_layout) =
        controller_runtime_and_project_layout_from_state(&state)?;
    // layout rejects partial or cross-section requests and preserves the untouched section.
    // layout 拒绝局部或跨分区请求，并保留未触碰分区。
    let layout = project_layout_after_section_reorder(&retained_layout, &project_ids)?;
    vulcan_workspace_store::workspace_store::persist_project_order_update(
        controller_runtime,
        &layout.pinned,
        &layout.regular,
    )
    .await?;
    refresh_workbench_snapshot(state).await
}

/// Applies a project display name and returns the refreshed workbench snapshot.
/// 应用项目显示名称并返回刷新后的工作台快照。
pub(crate) async fn service_rename_project(
    state: SharedServiceBridgeState,
    project_id: String,
    name: String,
) -> Result<WorkbenchSnapshot, String> {
    let controller_runtime = controller_runtime_from_state(&state)?;
    vulcan_workspace_store::workspace_store::rename_project(controller_runtime, &project_id, &name)
        .await?;
    refresh_workbench_snapshot(state).await
}

/// Stores how one project orders its unpinned sessions and returns the refreshed workbench snapshot.
/// 保存一个项目排列非置顶会话的方式，并返回刷新后的工作台快照。
pub(crate) async fn service_set_project_session_sort_mode(
    state: SharedServiceBridgeState,
    project_id: String,
    mode: vulcan_service_api::ProjectSessionSortMode,
) -> Result<WorkbenchSnapshot, String> {
    let (controller_runtime, _project) = controller_runtime_and_active_project_from_state(
        &state,
        &project_id,
        "set the session sort mode",
    )?;
    let mode = match mode {
        vulcan_service_api::ProjectSessionSortMode::LastRequest => {
            vulcan_workbench_model::ProjectSessionSortMode::LastRequest
        }
        vulcan_service_api::ProjectSessionSortMode::Manual => {
            vulcan_workbench_model::ProjectSessionSortMode::Manual
        }
    };
    vulcan_workspace_store::workspace_store::persist_project_session_sort_mode(
        controller_runtime,
        &project_id,
        mode,
    )
    .await?;
    refresh_workbench_snapshot(state).await
}

/// Stores the manual order of one project's sessions and returns the refreshed workbench snapshot.
/// 保存一个项目会话的手动顺序，并返回刷新后的工作台快照。
///
/// `conversation_ids` lists the project's unpinned sessions top to bottom; storage rejects
/// duplicates and sessions that are not active members of the project.
/// `conversation_ids` 自上而下列出该项目的非置顶会话；存储层拒绝重复项及非该项目活动成员的会话。
pub(crate) async fn service_reorder_project_sessions(
    state: SharedServiceBridgeState,
    project_id: String,
    conversation_ids: Vec<String>,
) -> Result<WorkbenchSnapshot, String> {
    let (controller_runtime, _project) = controller_runtime_and_active_project_from_state(
        &state,
        &project_id,
        "reorder the sessions",
    )?;
    vulcan_workspace_store::workspace_store::set_project_session_order_v1(
        &controller_runtime,
        &project_id,
        conversation_ids,
    )
    .await?;
    refresh_workbench_snapshot(state).await
}

/// Applies a project icon override without rebuilding the full workbench snapshot.
/// 应用项目图标覆盖，但不重建完整工作台快照。
pub(crate) async fn service_set_project_icon(
    state: SharedServiceBridgeState,
    project_id: String,
    icon_pack: String,
    icon_key: String,
) -> Result<(), String> {
    let normalized_icon_pack = normalize_non_empty_name(&icon_pack)?;
    let normalized_icon_key = normalize_non_empty_name(&icon_key)?;
    // controller_runtime is resolved together with the exact retained active project target.
    // controller_runtime 与精确保留活动项目目标一起解析。
    let (controller_runtime, _project) =
        controller_runtime_and_active_project_from_state(&state, &project_id, "set the icon")?;
    vulcan_workspace_store::workspace_store::persist_project_icon_update(
        controller_runtime,
        &project_id,
        Some(&normalized_icon_pack),
        Some(&normalized_icon_key),
    )
    .await?;
    update_project_icon_local_state(
        &state,
        &project_id,
        Some(normalized_icon_pack),
        Some(normalized_icon_key),
    )?;
    Ok(())
}

/// Clears a project icon override without rebuilding the full workbench snapshot.
/// 清除项目图标覆盖，但不重建完整工作台快照。
pub(crate) async fn service_reset_project_icon(
    state: SharedServiceBridgeState,
    project_id: String,
) -> Result<(), String> {
    // controller_runtime is resolved together with the exact retained active project target.
    // controller_runtime 与精确保留活动项目目标一起解析。
    let (controller_runtime, _project) =
        controller_runtime_and_active_project_from_state(&state, &project_id, "restore the icon")?;
    vulcan_workspace_store::workspace_store::persist_project_icon_update(
        controller_runtime,
        &project_id,
        None,
        None,
    )
    .await?;
    update_project_icon_local_state(&state, &project_id, None, None)?;
    Ok(())
}

/// Persists the expanded project identifiers and returns the retained workbench snapshot.
/// 持久化展开项目标识集合并返回保留的工作台快照。
pub(crate) async fn service_set_expanded_task_projects(
    state: SharedServiceBridgeState,
    project_ids: Vec<String>,
) -> Result<WorkbenchSnapshot, String> {
    let (controller_runtime, collapsed_project_ids, expanded_project_ids) = {
        let guard = state
            .lock()
            .map_err(|error| format!("failed to lock service state: {error}"))?;
        let controller_runtime = guard.controller_runtime.clone().ok_or_else(|| {
            String::from("service is still starting; controller runtime is not ready")
        })?;
        let visible_project_ids = guard
            .workbench
            .projects
            .iter()
            .map(|project| project.id.clone())
            .collect::<BTreeSet<_>>();
        let expanded_project_ids = project_ids
            .into_iter()
            .filter(|project_id| visible_project_ids.contains(project_id))
            .collect::<BTreeSet<_>>();
        let collapsed_project_ids = visible_project_ids
            .iter()
            .filter(|project_id| !expanded_project_ids.contains(*project_id))
            .cloned()
            .collect::<Vec<_>>();
        (
            controller_runtime,
            collapsed_project_ids,
            expanded_project_ids,
        )
    };
    vulcan_workspace_store::workspace_store::persist_collapsed_task_project_ids(
        controller_runtime,
        &collapsed_project_ids,
    )
    .await?;

    let mut guard = state
        .lock()
        .map_err(|error| format!("failed to lock service state: {error}"))?;
    if let Some(workspace_snapshot) = guard.workspace_snapshot.as_mut() {
        workspace_snapshot.expanded_task_projects = expanded_project_ids.clone();
    }
    guard.workbench.expanded_project_ids = guard
        .workbench
        .projects
        .iter()
        .filter(|project| expanded_project_ids.contains(&project.id))
        .map(|project| project.id.clone())
        .collect::<Vec<_>>();
    guard.workbench_generation = guard.workbench_generation.wrapping_add(1);
    guard.workbench.revision = guard.workbench_generation;
    Ok(guard.workbench.clone())
}

/// Archives every persisted active conversation under one project, keeps the project active, and refreshes authoritative state.
/// 归档单个项目下的全部持久化活动对话、保留项目本身，并刷新权威状态。
///
/// # Parameters
/// 参数
/// `state` owns the controller runtime and retained WebUI projection.
/// `state` 持有控制器运行时与保留的 WebUI 投影。
/// `project_id` is the exact active project identity selected by the user.
/// `project_id` 是用户选择的精确活动项目身份。
///
/// # Returns
/// 返回值
/// Returns the workbench snapshot reloaded from the authoritative persistence index after archival.
/// 返回归档后从权威持久化索引重新加载的工作台快照。
pub(crate) async fn service_archive_project_conversations(
    app: ServiceEventEmitter,
    state: SharedServiceBridgeState,
    project_id: String,
) -> Result<WorkbenchSnapshot, String> {
    let (controller_runtime, project) =
        controller_runtime_and_active_project_from_state(&state, &project_id, "archive")?;
    // reserved_session_ids is the retained admission set used only while acquiring the project gate.
    // reserved_session_ids 仅是在获取项目准入闸门时使用的保留准入集合。
    let reserved_session_ids = project.session_ids.clone();
    // lifecycle_guard atomically rejects active children and blocks new turns through commit.
    // lifecycle_guard 原子拒绝活动子会话，并在提交期间阻止新回合。
    let _lifecycle_guard = begin_project_lifecycle_transition(
        &state,
        &project_id,
        reserved_session_ids.iter().copied(),
        "archive",
    )?;
    // session_registry is the unique mutable owner used by every child lifecycle transaction.
    // session_registry 是每个子生命周期事务使用的唯一可变 owner。
    let session_registry = state
        .lock()
        .map_err(|error| format!("failed to lock service state: {error}"))?
        .session_runtime_registry
        .clone();
    let vmm_lifecycle_fence = begin_project_session_vmm_lifecycle_transition(&state, &project)?;
    // archived_session_ids comes from the post-transition V1 SQLite projection, not retained UI state.
    // archived_session_ids 来自转换后的 V1 SQLite 投影，而不是保留的 UI 状态。
    let archived_session_ids = crate::session_v1_lifecycle::archive_project_sessions_lifecycle_v1(
        &controller_runtime,
        session_registry.as_ref(),
        &project_id,
        current_project_lifecycle_unix_millis()?,
    )
    .await
    .map_err(|error| error.to_string())?;
    // workbench is the authoritative retained projection committed after every metadata/index pair.
    // workbench 是每个 metadata/索引对提交后形成的权威保留投影。
    let workbench = refresh_workbench_snapshot(state.clone()).await?;
    vmm_lifecycle_fence.release()?;
    let plugin_cleanup_failures =
        drain_vulcan_plugin_sessions_for_owners(&state, &archived_session_ids).await;
    if !plugin_cleanup_failures.is_empty() {
        tracing::warn!(
            "[vulcan-plugin] archived project cleanup warnings: {}",
            plugin_cleanup_failures.join("; ")
        );
    }
    close_session_mcp_connections_for_owners(&state, &archived_session_ids).await;
    // lifecycle_event carries every child transition in one transport-safe generation.
    // lifecycle_event 在单个传输安全 generation 中携带全部子转换。
    let lifecycle_event = conversations_archived_delta_envelope(&workbench, &archived_session_ids);
    emit_service_event_with_trace(
        &app,
        "workbench_project_conversations_archived_delta_emit",
        ServiceEvent::WorkbenchDelta(Box::new(lifecycle_event)),
    );
    Ok(workbench)
}

/// Archives all project conversations, removes the active project row, and refreshes the snapshot.
/// 归档项目全部对话、移除活动项目记录，并刷新工作台快照。
///
/// # Parameters
/// 参数
/// `state` owns the controller runtime, removal policy state, and retained WebUI projection.
/// `state` 持有控制器运行时、移除策略状态与保留的 WebUI 投影。
/// `project_id` is the exact active project identity selected by the user.
/// `project_id` 是用户选择的精确活动项目身份。
///
/// # Returns
/// 返回值
/// Returns the workbench snapshot reloaded after all authoritative child locators are archived and the project row is removed.
/// 返回全部权威子定位行归档且项目行移除后重新加载的工作台快照。
pub(crate) async fn service_remove_project(
    app: ServiceEventEmitter,
    state: SharedServiceBridgeState,
    project_id: String,
) -> Result<WorkbenchSnapshot, String> {
    let (controller_runtime, project) =
        controller_runtime_and_active_project_from_state(&state, &project_id, "remove")?;
    let removal_effect = project_removal_default_project_effect(&state, &project_id, &project)?;
    // reserved_session_ids is the retained admission set used only while acquiring the project gate.
    // reserved_session_ids 仅是在获取项目准入闸门时使用的保留准入集合。
    let reserved_session_ids = project.session_ids.clone();
    // lifecycle_guard atomically rejects active children and blocks new turns through removal commit.
    // lifecycle_guard 原子拒绝活动子会话，并在移除提交期间阻止新回合。
    let _lifecycle_guard = begin_project_lifecycle_transition(
        &state,
        &project_id,
        reserved_session_ids.iter().copied(),
        "remove",
    )?;

    // project_terminal_ids is the exact project-owned runtime set confirmed by the removal dialog.
    // project_terminal_ids 是已由移除确认框确认的精确项目级运行时集合。
    let project_terminal_ids = project_terminal_runtime()
        .list_project_terminals(&project_id)
        .await?
        .into_iter()
        .map(|terminal| terminal.terminal_id)
        .collect::<Vec<_>>();
    for terminal_id in project_terminal_ids {
        // Close first so a failed terminal stop cannot silently detach a still-existing display.
        // 先关闭终端，避免终端停止失败时无提示地解除仍存在显示层的挂接。
        project_terminal_runtime()
            .close_terminal(&project_id, terminal_id)
            .await?;
        cancel_project_terminal_displays(&project_id, terminal_id)?;
    }

    // session_registry is the unique mutable owner used by every child lifecycle transaction.
    // session_registry 是每个子生命周期事务使用的唯一可变 owner。
    let session_registry = state
        .lock()
        .map_err(|error| format!("failed to lock service state: {error}"))?
        .session_runtime_registry
        .clone();
    let vmm_lifecycle_fence = begin_project_session_vmm_lifecycle_transition(&state, &project)?;
    // archived_session_ids comes from the post-transition V1 SQLite projection, not retained UI state.
    // archived_session_ids 来自转换后的 V1 SQLite 投影，而不是保留的 UI 状态。
    let archived_session_ids = crate::session_v1_lifecycle::archive_project_sessions_lifecycle_v1(
        &controller_runtime,
        session_registry.as_ref(),
        &project_id,
        current_project_lifecycle_unix_millis()?,
    )
    .await
    .map_err(|error| error.to_string())?;
    vulcan_workspace_store::workspace_store::delete_active_project_row_v1(
        &controller_runtime,
        &project_id,
    )
    .await?;
    if removal_effect == ProjectRemovalDefaultProjectEffect::RestoreDefault {
        vulcan_workspace_store::workspace_store::ensure_default_workspace_project(
            controller_runtime,
        )
        .await?;
    }
    // workbench is the authoritative retained projection after child archival and project removal.
    // workbench 是子会话归档与项目移除后的权威保留投影。
    let workbench = refresh_workbench_snapshot(state.clone()).await?;
    vmm_lifecycle_fence.release()?;
    let plugin_cleanup_failures =
        drain_vulcan_plugin_sessions_for_owners(&state, &archived_session_ids).await;
    if !plugin_cleanup_failures.is_empty() {
        tracing::warn!(
            "[vulcan-plugin] removed project cleanup warnings: {}",
            plugin_cleanup_failures.join("; ")
        );
    }
    close_session_mcp_connections_for_owners(&state, &archived_session_ids).await;
    // lifecycle_event combines project and child transitions because the transport owns one envelope per generation.
    // lifecycle_event 合并项目与子转换，因为传输层每个 generation 只持有一个信封。
    let lifecycle_event =
        project_removed_delta_envelope(&workbench, project_id, &archived_session_ids);
    emit_service_event_with_trace(
        &app,
        "workbench_project_removed_delta_emit",
        ServiceEvent::WorkbenchDelta(Box::new(lifecycle_event)),
    );
    Ok(workbench)
}

/// Builds one transport-safe project removal envelope with every committed child archive.
/// 构建一条包含全部已提交子会话归档的传输安全项目移除信封。
///
/// # Parameters
/// 参数
/// `workbench` is the authoritative retained projection after removal.
/// `workbench` 是移除后的权威保留投影。
/// `project_id` is the exact active project removed by the transaction.
/// `project_id` 是事务移除的精确活动项目。
/// `session_ids` are the exact child sessions archived with the project.
/// `session_ids` 是与项目一同归档的精确子会话集合。
///
/// # Returns
/// 返回值
/// Returns one envelope because the transport indexes pending deltas by generation.
/// 返回单个信封，因为传输层按 generation 索引待处理增量。
fn project_removed_delta_envelope(
    workbench: &WorkbenchSnapshot,
    project_id: String,
    session_ids: &[SessionId],
) -> WorkbenchDeltaEnvelope {
    // archived_conversations retains one target-owned lifecycle transition per removed child.
    // archived_conversations 为每个已移除子会话保留一条目标自有生命周期转换。
    let archived_conversations = session_ids
        .iter()
        .map(|session_id| conversation_archived_delta(workbench, *session_id))
        .collect();
    WorkbenchDeltaEnvelope {
        generation: workbench.revision,
        delta: WorkbenchDelta::ProjectRemoved(ProjectRemovedDelta {
            project_id,
            projects: workbench.projects.clone(),
            expanded_project_ids: workbench.expanded_project_ids.clone(),
            archived_conversations,
        }),
    }
}

/// Describes whether project removal must restore the default project afterward.
/// 描述项目移除后是否必须恢复默认项目。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectRemovalDefaultProjectEffect {
    /// No default-project repair is needed after removal.
    /// 移除后不需要修复默认项目。
    None,
    /// Removing this project leaves no active projects, so the default project must be restored.
    /// 移除此项目会导致没有活动项目，因此必须恢复默认项目。
    RestoreDefault,
}

/// Validates default-project removal policy and returns the needed post-removal repair effect.
/// 校验默认项目移除策略，并返回移除后需要执行的修复效果。
///
/// # Parameters
/// 参数
/// `state` owns the current retained workspace snapshot.
/// `state` 持有当前保留的工作区快照。
/// `project_id` is the active project being removed.
/// `project_id` 是正在被移除的活动项目。
/// `project` is the resolved active project summary.
/// `project` 是已解析的活动项目摘要。
///
/// # Returns
/// 返回值
/// Returns the default-project repair effect or a user-facing rejection message.
/// 返回默认项目修复效果，或面向用户的拒绝信息。
fn project_removal_default_project_effect(
    state: &SharedServiceBridgeState,
    project_id: &str,
    project: &TaskProjectSummary,
) -> Result<ProjectRemovalDefaultProjectEffect, String> {
    let guard = state
        .lock()
        .map_err(|error| format!("failed to lock service state: {error}"))?;
    let workspace_snapshot = guard.workspace_snapshot.as_ref().ok_or_else(|| {
        String::from("The workspace snapshot is not loaded yet; the project cannot be processed.")
    })?;
    project_removal_default_project_effect_from_snapshot(workspace_snapshot, project_id, project)
}

/// Computes the default-project repair effect from a concrete workspace snapshot.
/// 根据具体工作区快照计算默认项目修复效果。
///
/// # Parameters
/// 参数
/// `workspace_snapshot` is the retained snapshot containing active projects.
/// `workspace_snapshot` 是包含活动项目的保留快照。
/// `project_id` is the active project being removed.
/// `project_id` 是正在被移除的活动项目。
/// `project` is the resolved active project summary.
/// `project` 是已解析的活动项目摘要。
///
/// # Returns
/// 返回值
/// Returns the repair effect or rejects removing the only default project.
/// 返回修复效果，或拒绝移除唯一默认项目。
fn project_removal_default_project_effect_from_snapshot(
    workspace_snapshot: &WorkspaceSnapshot,
    project_id: &str,
    project: &TaskProjectSummary,
) -> Result<ProjectRemovalDefaultProjectEffect, String> {
    let remaining_active_project_count = workspace_snapshot
        .task_projects
        .iter()
        .filter(|candidate| candidate.id != project_id)
        .count();
    if remaining_active_project_count > 0 {
        return Ok(ProjectRemovalDefaultProjectEffect::None);
    }
    if vulcan_workspace_store::workspace_store::is_default_workspace_project(project)? {
        return Err(String::from(
            "The default project is the only project and cannot be removed.",
        ));
    }
    Ok(ProjectRemovalDefaultProjectEffect::RestoreDefault)
}

/// Returns the controller runtime and minimal layout state for every retained active project.
/// 返回控制器运行时以及每个保留活动项目的最小布局状态。
///
/// # Parameters
/// 参数
/// `state` owns the controller runtime and retained authoritative workspace snapshot.
/// `state` 持有控制器运行时与保留的权威工作区快照。
///
/// # Returns
/// 返回值
/// Returns the controller runtime and active projects in current persisted display order.
/// 返回控制器运行时以及按当前持久化展示顺序排列的活动项目。
pub(super) fn controller_runtime_and_project_layout_from_state(
    state: &SharedServiceBridgeState,
) -> Result<(HostRuntimeHandle, Vec<RetainedProjectLayoutEntry>), String> {
    // guard provides one coherent read of the runtime and retained workspace projection.
    // guard 提供对运行时与保留工作区投影的一致读取。
    let guard = state
        .lock()
        .map_err(|error| format!("failed to lock service state: {error}"))?;
    // controller_runtime is required before any authoritative layout mutation can run.
    // controller_runtime 是执行任何权威布局变更前的必要条件。
    let controller_runtime = guard.controller_runtime.clone().ok_or_else(|| {
        String::from("service is still starting; controller runtime is not ready")
    })?;
    // workspace_snapshot is the retained source of active project identity and ordering.
    // workspace_snapshot 是活动项目身份与顺序的保留来源。
    let workspace_snapshot = guard.workspace_snapshot.as_ref().ok_or_else(|| {
        String::from("The workspace snapshot is not loaded yet; the project cannot be processed.")
    })?;
    // project_layout deliberately copies only fields needed by the guarded mutation.
    // project_layout 有意只复制受保护变更所需字段。
    let project_layout = workspace_snapshot
        .task_projects
        .iter()
        .map(|project| RetainedProjectLayoutEntry {
            id: project.id.clone(),
            pinned: project.pinned,
        })
        .collect::<Vec<_>>();
    Ok((controller_runtime, project_layout))
}

/// Returns the controller runtime and one retained active project without reloading the workspace.
/// 返回控制器运行时与一个保留的活动项目，不重新加载工作区。
///
/// # Parameters
/// 参数
/// `state` owns the controller runtime and retained workspace snapshot.
/// `state` 持有控制器运行时与保留的工作区快照。
/// `project_id` is the exact active project identity to resolve.
/// `project_id` 是需要解析的精确活动项目身份。
/// `action_label` is the localized operation label used in lookup failures.
/// `action_label` 是查找失败时使用的本地化操作标签。
///
/// # Returns
/// 返回值
/// Returns the controller runtime and validated active project without deriving child-session scope from the snapshot.
/// 返回控制器运行时与已验证活动项目，且不从快照推导子会话范围。
pub(super) fn controller_runtime_and_active_project_from_state(
    state: &SharedServiceBridgeState,
    project_id: &str,
    action_label: &str,
) -> Result<(HostRuntimeHandle, TaskProjectSummary), String> {
    let guard = state
        .lock()
        .map_err(|error| format!("failed to lock service state: {error}"))?;
    let controller_runtime = guard.controller_runtime.clone().ok_or_else(|| {
        String::from("service is still starting; controller runtime is not ready")
    })?;
    let workspace_snapshot = guard.workspace_snapshot.as_ref().ok_or_else(|| {
        String::from("The workspace snapshot is not loaded yet; the project cannot be processed.")
    })?;
    let project = active_project_from_workspace(workspace_snapshot, project_id, action_label)?;
    Ok((controller_runtime, project))
}

/// Resolves one active workspace project for a project-level action.
/// 为项目级操作解析单个活动工作区项目。
///
/// # Parameters
/// 参数
/// `workspace_snapshot` contains active project summaries.
/// `workspace_snapshot` 包含活动项目摘要。
/// `project_id` is the requested project identifier.
/// `project_id` 是请求的项目标识。
/// `action_label` labels the user-facing error.
/// `action_label` 标记面向用户的错误信息。
///
/// # Returns
/// 返回值
/// The matching active project summary.
/// 返回匹配的活动项目摘要。
fn active_project_from_workspace(
    workspace_snapshot: &WorkspaceSnapshot,
    project_id: &str,
    action_label: &str,
) -> Result<TaskProjectSummary, String> {
    workspace_task_project_for_project_id(workspace_snapshot, project_id)
        .cloned()
        .ok_or_else(|| format!("Project to {action_label} not found: {project_id}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use vulcan_workbench_model::SessionId;

    /// Builds one workspace snapshot from active project rows.
    /// 使用活动项目行构建一个工作区快照。
    ///
    /// # Parameters
    /// 参数
    /// `task_projects` contains the active project rows visible to project actions.
    /// `task_projects` 包含项目操作可见的活动项目行。
    ///
    /// # Returns
    /// 返回值
    /// A workspace snapshot with only the supplied active projects populated.
    /// 返回仅填充给定活动项目的工作区快照。
    fn workspace_snapshot_with_projects(
        task_projects: Vec<TaskProjectSummary>,
    ) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            task_projects,
            archived_task_projects: Vec::new(),
            sessions: Vec::new(),
            archived_sessions: Vec::new(),
            session_runtime_state: BTreeMap::new(),
            archived_session_runtime_state: BTreeMap::new(),
            session_run_phases: BTreeMap::new(),
            expanded_task_projects: BTreeSet::new(),
            selected_session: SessionId::nil(),
            hydrated_session_ids: BTreeSet::new(),
        }
    }

    /// Builds one project summary for active-project resolver tests.
    /// 为活动项目解析器测试构建单个项目摘要。
    ///
    /// # Parameters
    /// 参数
    /// `project_id` is the persisted project identifier.
    /// `project_id` 是持久化项目标识。
    /// `title` is the displayed project title.
    /// `title` 是项目展示标题。
    ///
    /// # Returns
    /// 返回值
    /// A task project summary without nested sessions.
    /// 返回不包含嵌套会话的任务项目摘要。
    fn project_summary(project_id: &str, title: &str) -> TaskProjectSummary {
        TaskProjectSummary::new(
            project_id,
            title,
            format!("D:/workspace/{project_id}"),
            false,
            0,
            None,
            None,
            Vec::new(),
        )
    }

    /// Builds the default workspace project summary used by removal policy tests.
    /// 构建移除策略测试使用的默认工作区项目摘要。
    ///
    /// # Returns
    /// 返回值
    /// A project summary whose path matches the canonical default workspace path.
    /// 返回路径匹配标准默认工作区路径的项目摘要。
    fn default_project_summary() -> TaskProjectSummary {
        TaskProjectSummary::new(
            "default-project",
            "VulcanProject",
            vulcan_workspace_store::workspace_store::default_workspace_project_path_text()
                .expect("test host should expose a home directory"),
            false,
            0,
            None,
            None,
            Vec::new(),
        )
    }

    /// Resolves active projects through the shared workspace project lookup.
    /// 通过共享工作区项目查找解析活动项目。
    #[test]
    fn active_project_from_workspace_resolves_active_project() {
        // Workspace snapshot containing one active project row.
        // 包含一个活动项目行的工作区快照。
        let workspace_snapshot =
            workspace_snapshot_with_projects(vec![project_summary("project-a", "Project A")]);

        // Resolved project returned by the project action helper.
        // 项目操作辅助函数返回的已解析项目。
        let project = active_project_from_workspace(&workspace_snapshot, "project-a", "remove")
            .expect("active project should resolve through shared lookup");

        assert_eq!(project.id, "project-a");
        assert_eq!(project.title, "Project A");
    }

    /// Rejects blank project identifiers instead of matching blank project rows.
    /// 拒绝空白项目标识，而不是匹配空白项目行。
    #[test]
    fn active_project_from_workspace_rejects_blank_project_id() {
        // Workspace snapshot containing a malformed blank active project row.
        // 包含一个异常空白活动项目行的工作区快照。
        let workspace_snapshot =
            workspace_snapshot_with_projects(vec![project_summary("   ", "Blank Project")]);

        // Error emitted by the shared blank-project-id semantics.
        // 共享空白项目标识语义返回的错误。
        let error = active_project_from_workspace(&workspace_snapshot, "   ", "remove")
            .expect_err("blank project id should be rejected by shared lookup");

        assert!(error.contains("Project to remove not found"));
    }

    /// Rejects removing the default project when it is the only active project.
    /// 当默认项目是唯一活动项目时拒绝移除。
    #[test]
    fn project_removal_rejects_only_default_project() {
        // Workspace snapshot containing only the built-in default project.
        // 仅包含内置默认项目的工作区快照。
        let default_project = default_project_summary();
        let workspace_snapshot = workspace_snapshot_with_projects(vec![default_project.clone()]);

        // Removal policy refuses to leave the workspace without any active project.
        // 移除策略拒绝让工作区进入没有活动项目的状态。
        let error = project_removal_default_project_effect_from_snapshot(
            &workspace_snapshot,
            &default_project.id,
            &default_project,
        )
        .expect_err("the only default project should not be removable");

        assert!(error.contains("only project"));
    }

    /// Requests default-project restoration after removing the only non-default project.
    /// 移除唯一非默认项目后要求恢复默认项目。
    #[test]
    fn project_removal_restores_default_after_last_non_default_project() {
        // Workspace snapshot containing one ordinary active project.
        // 包含一个普通活动项目的工作区快照。
        let project = project_summary("project-a", "Project A");
        let workspace_snapshot = workspace_snapshot_with_projects(vec![project.clone()]);

        // Removal is allowed, but the default project must be restored afterward.
        // 允许移除，但之后必须恢复默认项目。
        let effect = project_removal_default_project_effect_from_snapshot(
            &workspace_snapshot,
            &project.id,
            &project,
        )
        .expect("ordinary last project removal should be repairable");

        assert_eq!(effect, ProjectRemovalDefaultProjectEffect::RestoreDefault);
    }

    /// Allows removing the default project when another active project remains.
    /// 当仍有其他活动项目时允许移除默认项目。
    #[test]
    fn project_removal_allows_default_project_when_another_project_remains() {
        // Workspace snapshot containing the default project and one ordinary project.
        // 包含默认项目和一个普通项目的工作区快照。
        let default_project = default_project_summary();
        let workspace_snapshot = workspace_snapshot_with_projects(vec![
            default_project.clone(),
            project_summary("project-a", "Project A"),
        ]);

        // Another project remains visible, so no default restoration is required.
        // 仍有另一个项目可见，因此无需恢复默认项目。
        let effect = project_removal_default_project_effect_from_snapshot(
            &workspace_snapshot,
            &default_project.id,
            &default_project,
        )
        .expect("default project removal should be allowed when another project remains");

        assert_eq!(effect, ProjectRemovalDefaultProjectEffect::None);
    }

    /// Builds one minimal retained project layout entry for service-layout tests.
    /// 为服务布局测试构建一个最小保留项目布局条目。
    ///
    /// # Parameters
    /// 参数
    /// `project_id` is the exact active project identity.
    /// `project_id` 是精确活动项目身份。
    /// `pinned` is the current retained pinned state.
    /// `pinned` 是当前保留置顶状态。
    ///
    /// # Returns
    /// 返回值
    /// Returns one retained project layout entry.
    /// 返回一个保留项目布局条目。
    fn retained_project_layout(project_id: &str, pinned: bool) -> RetainedProjectLayoutEntry {
        RetainedProjectLayoutEntry {
            id: project_id.to_string(),
            pinned,
        }
    }

    /// Verifies pin transitions preserve display order while moving only the target section membership.
    /// 验证置顶转换会保留展示顺序，同时只移动目标的分区归属。
    #[test]
    fn project_layout_after_pin_toggle_preserves_resulting_section_order() {
        // projects model two pinned projects followed by two regular projects.
        // projects 模拟两个置顶项目后接两个普通项目。
        let projects = vec![
            retained_project_layout("project-a", true),
            retained_project_layout("project-b", true),
            retained_project_layout("project-c", false),
            retained_project_layout("project-d", false),
        ];
        // pinned_result appends project-c to the existing pinned section.
        // pinned_result 将 project-c 追加到现有置顶分区。
        let (next_pinned, pinned_result) = project_layout_after_pin_toggle(&projects, "project-c")
            .expect("regular project should move into pinned section");
        // regular_result prepends project-b to the regular section according to retained display order.
        // regular_result 按保留展示顺序将 project-b 放到普通分区开头。
        let (next_regular, regular_result) =
            project_layout_after_pin_toggle(&projects, "project-b")
                .expect("pinned project should move into regular section");

        assert!(next_pinned);
        assert_eq!(
            vec!["project-a", "project-b", "project-c"],
            pinned_result.pinned
        );
        assert_eq!(vec!["project-d"], pinned_result.regular);
        assert!(!next_regular);
        assert_eq!(vec!["project-a"], regular_result.pinned);
        assert_eq!(
            vec!["project-b", "project-c", "project-d"],
            regular_result.regular
        );
    }

    /// Verifies section reorder requests must contain the exact complete section membership.
    /// 验证分区重排请求必须包含精确完整的分区成员集合。
    #[test]
    fn project_layout_after_section_reorder_requires_complete_exact_section() {
        // projects model the authoritative retained membership of both sections.
        // projects 模拟两个分区的权威保留成员关系。
        let projects = vec![
            retained_project_layout("project-a", true),
            retained_project_layout("project-b", true),
            retained_project_layout("project-c", false),
            retained_project_layout("project-d", false),
        ];
        // reordered_ids contain every pinned project exactly once in the desired order.
        // reordered_ids 按目标顺序恰好包含每个置顶项目一次。
        let reordered_ids = vec![String::from("project-b"), String::from("project-a")];
        // layout preserves the untouched regular section while applying the complete pinned order.
        // layout 在应用完整置顶顺序的同时保留未触碰的普通分区。
        let layout = project_layout_after_section_reorder(&projects, &reordered_ids)
            .expect("complete same-section reorder should validate");

        assert_eq!(reordered_ids, layout.pinned);
        assert_eq!(vec!["project-c", "project-d"], layout.regular);
        assert!(
            project_layout_after_section_reorder(&projects, &[String::from("project-a")])
                .expect_err("partial section should fail")
                .contains("complete project set")
        );
        assert!(
            project_layout_after_section_reorder(
                &projects,
                &[String::from("project-a"), String::from("project-c")],
            )
            .expect_err("cross-section reorder should fail")
            .contains("complete project set")
        );
        assert!(
            project_layout_after_section_reorder(
                &projects,
                &[String::from("project-a"), String::from("project-a")],
            )
            .expect_err("duplicate reorder identity should fail")
            .contains("duplicate identifiers")
        );
    }

    /// Verifies project removal keeps child archives and project state in one transport envelope.
    /// 验证项目移除会把子会话归档与项目状态保存在单个传输信封中。
    #[test]
    fn project_removed_delta_uses_one_committed_generation() {
        // session_id identifies the child archived with the removed project.
        // session_id 标识与移除项目一同归档的子会话。
        let session_id = SessionId::from_u128(0x71);
        // workbench supplies the authoritative post-removal project projection.
        // workbench 提供移除后的权威项目投影。
        let mut workbench = crate::fallback_workbench_projection::fallback_workbench_snapshot(&[]);
        workbench.revision = 71;
        workbench.expanded_project_ids = vec![String::from("remaining-project")];

        let envelope = project_removed_delta_envelope(
            &workbench,
            String::from("removed-project"),
            &[session_id],
        );

        assert_eq!(envelope.generation, 71);
        let WorkbenchDelta::ProjectRemoved(delta) = envelope.delta else {
            panic!("expected project removed delta");
        };
        assert_eq!(delta.project_id, "removed-project");
        assert_eq!(delta.expanded_project_ids, vec!["remaining-project"]);
        assert_eq!(delta.archived_conversations.len(), 1);
        assert_eq!(
            delta.archived_conversations[0].conversation_id,
            session_id.to_string()
        );
    }
}
