use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeComponentId {
    Gateway,
    InternalResources,
    SystemTools,
    Mcp,
    Scheduler,
    BrowserExtension,
    Cef,
    ComputerUse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SystemToolId {
    Git,
    DefaultShell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SystemToolState {
    Ready,
    Degraded,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SystemToolSource {
    ShellSnapshot,
    ProcessEnvironment,
    OsRegistry,
    WellKnown,
    TestOverride,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SystemToolCapability {
    GitInspect,
    GitLocalMutation,
    GitWorktree,
    ShellCommand,
    ShellInteractive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SystemToolStatus {
    pub tool: SystemToolId,
    pub state: SystemToolState,
    pub executable_path: Option<String>,
    pub version: Option<String>,
    pub source: Option<SystemToolSource>,
    pub capabilities: Vec<SystemToolCapability>,
    pub error_code: Option<String>,
    #[specta(type = specta_typescript::Number)]
    pub observed_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SystemRuntimeSnapshot {
    #[specta(type = specta_typescript::Number)]
    pub revision: u64,
    pub tools: Vec<SystemToolStatus>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ShellKind {
    Posix,
    PowerShell,
    CommandPrompt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ShellLaunchSpec {
    pub executable_path: String,
    pub kind: ShellKind,
    pub interactive_args: Vec<String>,
    pub command_args: Vec<String>,
    #[specta(type = specta_typescript::Number)]
    pub runtime_revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeComponentState {
    Starting,
    Ready,
    Retrying,
    Degraded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeComponentHealth {
    pub component: RuntimeComponentId,
    pub state: RuntimeComponentState,
    pub error_code: Option<String>,
    pub retryable: bool,
    pub attempt: u32,
    #[specta(type = Option<specta_typescript::Number>)]
    pub next_retry_at_ms: Option<i64>,
    #[specta(type = specta_typescript::Number)]
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeHealthSnapshot {
    pub components: Vec<RuntimeComponentHealth>,
    #[specta(type = specta_typescript::Number)]
    pub revision: u64,
}
