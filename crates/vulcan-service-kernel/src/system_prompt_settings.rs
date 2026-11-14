//! Base system prompt settings: read, save, and reset the user and project override layers.
//! 基础系统提示词设置：读取、保存与重置用户及项目覆盖层级。

use std::path::{Path, PathBuf};

use vulcan_agent_runtime::agent_runtime::provider_prompt_loader::{
    built_in_system_prompt, load_effective_system_prompt,
};
use vulcan_agent_runtime::provider_config::{
    project_system_prompt_file_path, read_system_prompt_file, remove_system_prompt_file,
    save_system_prompt_file, user_system_prompt_file_path,
};
use vulcan_service_api::{
    SystemPromptResetRequest, SystemPromptSaveRequest, SystemPromptScope,
    SystemPromptSettingsSnapshot,
};

use crate::SharedServiceBridgeState;
use crate::runtime_workspace_helpers::workspace_task_project_for_project_id;

/// Resolves one exact project identifier to its workspace directory.
/// 将一个精确项目标识解析为其工作区目录。
///
/// # Returns
/// 返回值
/// The project directory, or an error when the workspace is not loaded or the project is unknown.
/// 返回项目目录；工作区未载入或项目未知时返回错误。
fn resolve_project_directory(
    state: &SharedServiceBridgeState,
    project_id: &str,
) -> Result<PathBuf, String> {
    let guard = state
        .lock()
        .map_err(|error| format!("failed to lock service state: {error}"))?;
    let workspace = guard
        .workspace_snapshot
        .as_ref()
        .ok_or_else(|| String::from("workspace is not loaded"))?;
    workspace_task_project_for_project_id(workspace, project_id)
        .map(|project| PathBuf::from(&project.path))
        .ok_or_else(|| format!("unknown project: {project_id}"))
}

/// Resolves the override file targeted by one scope.
/// 解析单个作用域指向的覆盖文件。
fn resolve_override_path(
    state: &SharedServiceBridgeState,
    scope: SystemPromptScope,
    project_id: Option<&str>,
) -> Result<PathBuf, String> {
    match scope {
        SystemPromptScope::User => Ok(user_system_prompt_file_path()),
        SystemPromptScope::Project => {
            let project_id = project_id
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| String::from("projectId is required for the project scope"))?;
            Ok(project_system_prompt_file_path(&resolve_project_directory(
                state, project_id,
            )?))
        }
    }
}

/// Builds the settings snapshot for the user layer and an optional project.
/// 为用户层级与可选项目构建设置快照。
fn build_snapshot(
    state: &SharedServiceBridgeState,
    project_id: Option<&str>,
) -> Result<SystemPromptSettingsSnapshot, String> {
    let project_id = project_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let project_directory = project_id
        .as_deref()
        .map(|project_id| resolve_project_directory(state, project_id))
        .transpose()?;
    let user_path = user_system_prompt_file_path();
    let project_path = project_directory
        .as_deref()
        .map(project_system_prompt_file_path);
    // effective reuses the runtime resolver so precedence has one definition.
    // effective 复用运行时解析器，使优先级只有一处定义。
    let effective = load_effective_system_prompt(project_directory.as_deref())?;
    Ok(SystemPromptSettingsSnapshot {
        built_in_text: built_in_system_prompt().to_string(),
        user_text: read_system_prompt_file(&user_path)?,
        user_path: user_path.display().to_string(),
        project_id,
        project_text: project_path
            .as_deref()
            .map(read_system_prompt_file)
            .transpose()?
            .flatten(),
        project_path: project_path
            .as_deref()
            .map(Path::display)
            .map(|path| path.to_string()),
        effective_source: effective.source.id().to_string(),
    })
}

/// Returns every base system prompt layer visible to the requested scope.
/// 返回请求作用域可见的全部基础系统提示词层级。
pub(crate) async fn service_system_prompt_settings(
    state: SharedServiceBridgeState,
    project_id: Option<String>,
) -> Result<SystemPromptSettingsSnapshot, String> {
    build_snapshot(&state, project_id.as_deref())
}

/// Saves one override for the next execution's fixed Provider request head.
/// 保存一个覆盖，供下一次执行解析为固定的 Provider 请求头部指令。
pub(crate) async fn service_save_system_prompt(
    state: SharedServiceBridgeState,
    request: SystemPromptSaveRequest,
) -> Result<SystemPromptSettingsSnapshot, String> {
    let path = resolve_override_path(&state, request.scope, request.project_id.as_deref())?;
    save_system_prompt_file(&path, &request.text)?;
    build_snapshot(&state, request.project_id.as_deref())
}

/// Removes one override so the next lower layer takes effect.
/// 移除一个覆盖，使下一较低层级生效。
pub(crate) async fn service_reset_system_prompt(
    state: SharedServiceBridgeState,
    request: SystemPromptResetRequest,
) -> Result<SystemPromptSettingsSnapshot, String> {
    let path = resolve_override_path(&state, request.scope, request.project_id.as_deref())?;
    remove_system_prompt_file(&path)?;
    build_snapshot(&state, request.project_id.as_deref())
}
