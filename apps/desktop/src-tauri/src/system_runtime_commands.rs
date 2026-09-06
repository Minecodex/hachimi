use hachimi_protocol::{
    RuntimeComponentId, RuntimeComponentState, SystemRuntimeSnapshot, SystemToolState,
};
use hachimi_system_runtime::SystemRuntimeManager;
use tauri::{Emitter, Manager, State, WebviewWindow};

use crate::{CommandError, ControlMethod, DesktopState, RuntimeSupervisor, require_window};

pub(super) const SYSTEM_RUNTIME_EVENT: &str = "system-runtime-changed";

pub(super) fn publish_system_runtime_health(
    supervisor: &RuntimeSupervisor,
    snapshot: &SystemRuntimeSnapshot,
) {
    let failure = snapshot
        .tools
        .iter()
        .find(|tool| tool.state != SystemToolState::Ready);
    if let Some(tool) = failure {
        supervisor.update(
            RuntimeComponentId::SystemTools,
            RuntimeComponentState::Degraded,
            tool.error_code.as_deref(),
            true,
            0,
            None,
        );
    } else {
        supervisor.ready(RuntimeComponentId::SystemTools);
    }
}

pub(super) fn start_system_runtime_refresh(
    manager: SystemRuntimeManager,
    supervisor: RuntimeSupervisor,
    app: tauri::AppHandle,
) {
    let retry = supervisor.retry_signal(RuntimeComponentId::SystemTools);
    let shutdown = supervisor.shutdown_token();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                () = retry.notified() => {
                    let snapshot = manager.refresh().await;
                    publish_system_runtime_health(&supervisor, &snapshot);
                    let _ = app.emit(SYSTEM_RUNTIME_EVENT, &snapshot);
                }
            }
        }
    });
}

#[tauri::command]
pub(super) fn get_system_runtime(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<SystemRuntimeSnapshot, CommandError> {
    state.authorize(&window, ControlMethod::WorkbenchWindow)?;
    require_window(&window, "workbench")?;
    Ok(state.system_runtime.snapshot())
}

#[tauri::command]
pub(super) async fn refresh_system_runtime(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<SystemRuntimeSnapshot, CommandError> {
    state.authorize(&window, ControlMethod::WorkbenchWindow)?;
    require_window(&window, "workbench")?;
    let snapshot = state.system_runtime.refresh().await;
    publish_system_runtime_health(&state.runtime_supervisor, &snapshot);
    window
        .app_handle()
        .emit(SYSTEM_RUNTIME_EVENT, &snapshot)
        .map_err(CommandError::from)?;
    Ok(snapshot)
}
