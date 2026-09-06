use hachimi_protocol::{BrowserNavigationError, BrowserTabId};
use serde::{Deserialize, Serialize};

pub const CEF_IPC_PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CefHostCommandEnvelope {
    pub protocol_version: u32,
    pub request_id: u64,
    pub command: CefHostCommand,
}

impl CefHostCommandEnvelope {
    #[must_use]
    pub fn new(request_id: u64, command: CefHostCommand) -> Self {
        Self {
            protocol_version: CEF_IPC_PROTOCOL_VERSION,
            request_id,
            command,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CefBounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// Backing scale factor of the surface (physical px per CSS px). Hosts
    /// driving windowless (OSR) rendering use it to size the logical viewport
    /// and to report the frame scale; windowed hosts ignore it.
    #[serde(default = "default_scale_factor")]
    pub scale_factor: f32,
}

fn default_scale_factor() -> f32 {
    1.0
}

impl CefBounds {
    #[must_use]
    pub fn validated(self) -> Option<Self> {
        (self.width > 0
            && self.height > 0
            && self.width <= 16_384
            && self.height <= 16_384
            && self.x.unsigned_abs() <= 100_000
            && self.y.unsigned_abs() <= 100_000
            && self.scale_factor.is_finite()
            && (0.5..=4.0).contains(&self.scale_factor))
        .then_some(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CefHostCommand {
    SetParentWindow {
        parent_hwnd: u64,
    },
    CreateTab {
        tab_id: BrowserTabId,
        url: String,
        bounds: CefBounds,
        visible: bool,
    },
    CloseTab {
        tab_id: BrowserTabId,
    },
    ActivateTab {
        tab_id: BrowserTabId,
    },
    SetBounds {
        tab_id: BrowserTabId,
        bounds: CefBounds,
    },
    SetVisible {
        tab_id: BrowserTabId,
        visible: bool,
    },
    SetAgentNavigationPolicy {
        tab_id: BrowserTabId,
        allowed_origins: Vec<String>,
    },
    ClearAgentNavigationPolicy {
        tab_id: BrowserTabId,
    },
    Focus {
        tab_id: BrowserTabId,
    },
    Navigate {
        tab_id: BrowserTabId,
        url: String,
    },
    Back {
        tab_id: BrowserTabId,
    },
    Forward {
        tab_id: BrowserTabId,
    },
    Reload {
        tab_id: BrowserTabId,
        ignore_cache: bool,
    },
    Stop {
        tab_id: BrowserTabId,
    },
    ConfigureDownloads {
        directory: Option<String>,
        ask_where_to_save: bool,
    },
    ClearBrowsingData {
        cookies: bool,
        cache: bool,
    },
    CancelDownload {
        tab_id: BrowserTabId,
        download_id: u32,
    },
    Observe {
        tab_id: BrowserTabId,
    },
    DevTools {
        tab_id: BrowserTabId,
        method: String,
        params: serde_json::Value,
        full_access: bool,
    },
    /// Route a native input event into a windowless (OSR) tab. The desktop's
    /// overlay view translates OS events into this payload; the host forwards
    /// them to the browser via `BrowserHost::send_*_event`.
    SendInput {
        tab_id: BrowserTabId,
        event: CefInputEvent,
    },
    Shutdown,
}

impl CefHostCommand {
    #[must_use]
    pub fn tab_id(&self) -> Option<&BrowserTabId> {
        match self {
            Self::SetParentWindow { .. }
            | Self::ConfigureDownloads { .. }
            | Self::ClearBrowsingData { .. }
            | Self::Shutdown => None,
            Self::CreateTab { tab_id, .. }
            | Self::CloseTab { tab_id }
            | Self::ActivateTab { tab_id }
            | Self::SetBounds { tab_id, .. }
            | Self::SetVisible { tab_id, .. }
            | Self::SetAgentNavigationPolicy { tab_id, .. }
            | Self::ClearAgentNavigationPolicy { tab_id }
            | Self::Focus { tab_id }
            | Self::Navigate { tab_id, .. }
            | Self::Back { tab_id }
            | Self::Forward { tab_id }
            | Self::Reload { tab_id, .. }
            | Self::Stop { tab_id }
            | Self::CancelDownload { tab_id, .. }
            | Self::Observe { tab_id }
            | Self::DevTools { tab_id, .. }
            | Self::SendInput { tab_id, .. } => Some(tab_id),
        }
    }
}

/// `cef_event_flags_t` modifier bits carried by [`CefInputEvent`] payloads.
/// Values match the CEF constants so the host can forward them unchanged.
pub mod cef_event_flags {
    pub const CAPS_LOCK_ON: u32 = 1;
    pub const SHIFT_DOWN: u32 = 1 << 1;
    pub const CONTROL_DOWN: u32 = 1 << 2;
    pub const ALT_DOWN: u32 = 1 << 3;
    pub const LEFT_MOUSE_BUTTON: u32 = 1 << 4;
    pub const MIDDLE_MOUSE_BUTTON: u32 = 1 << 5;
    pub const RIGHT_MOUSE_BUTTON: u32 = 1 << 6;
    pub const COMMAND_DOWN: u32 = 1 << 7;
    pub const NUM_LOCK_ON: u32 = 1 << 8;
    pub const IS_KEY_PAD: u32 = 1 << 9;
    pub const IS_LEFT: u32 = 1 << 10;
    pub const IS_RIGHT: u32 = 1 << 11;
}

/// Mouse button identity for [`CefInputEvent::MouseButton`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CefMouseButton {
    Left,
    Middle,
    Right,
}

/// Key event phases mirroring `cef_key_event_type_t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CefKeyEventKind {
    RawKeyDown,
    KeyDown,
    KeyUp,
    Char,
}

/// A native input event routed from the desktop's overlay view into a
/// windowless (OSR) browser. Coordinates are logical (CSS) pixels relative to
/// the tab viewport with the Y axis pointing down (web convention); the
/// desktop performs the AppKit bottom-left → top-left flip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CefInputEvent {
    MouseMove {
        x: i32,
        y: i32,
        modifiers: u32,
        leave: bool,
    },
    MouseButton {
        x: i32,
        y: i32,
        modifiers: u32,
        button: CefMouseButton,
        up: bool,
        click_count: i32,
    },
    MouseWheel {
        x: i32,
        y: i32,
        modifiers: u32,
        delta_x: i32,
        delta_y: i32,
    },
    Key {
        kind: CefKeyEventKind,
        /// Chromium virtual-key code (shared across platforms).
        windows_key_code: i32,
        /// Platform-native key code (macOS `NSEvent.keyCode`).
        native_key_code: i32,
        modifiers: u32,
        /// UTF-16 code unit for Char events (0 when none).
        character: u16,
        unmodified_character: u16,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CefHostMessage {
    Ready {
        protocol_version: u32,
        chromium_version: String,
    },
    Response {
        request_id: u64,
        result: Result<CefHostResponse, CefHostFailure>,
    },
    Event {
        event: CefHostEvent,
    },
    Fatal {
        code: String,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CefHostResponse {
    Acknowledged,
    TabCreated { state: CefTabState },
    Observation { observation: CefObservation },
    DevTools { result: serde_json::Value },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CefHostFailure {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

impl CefHostFailure {
    #[must_use]
    pub fn new(code: impl Into<String>, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CefTabState {
    pub tab_id: BrowserTabId,
    pub url: String,
    pub title: String,
    pub loading: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub navigation_error: Option<BrowserNavigationError>,
    pub input_epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CefObservation {
    pub state: CefTabState,
    pub text: String,
    pub accessibility_tree: serde_json::Value,
    pub screenshot_base64: Option<String>,
    pub screenshot_mime_type: Option<String>,
    pub viewport_width: Option<u32>,
    pub viewport_height: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CefHostEvent {
    TabStateChanged {
        state: CefTabState,
    },
    UserInput {
        tab_id: BrowserTabId,
        input_epoch: u64,
    },
    ShortcutRequested {
        tab_id: BrowserTabId,
        shortcut: CefBrowserShortcut,
    },
    PopupRequested {
        opener_tab_id: BrowserTabId,
        target_url: String,
    },
    AgentNavigationBlocked {
        tab_id: BrowserTabId,
        target_url: String,
    },
    DownloadUpdated {
        tab_id: BrowserTabId,
        download_id: u32,
        url: String,
        suggested_name: String,
        destination: Option<String>,
        received_bytes: u64,
        total_bytes: Option<u64>,
        complete: bool,
        cancelled: bool,
        interrupted: bool,
    },
    RenderProcessTerminated {
        tab_id: BrowserTabId,
        status: String,
    },
    /// Windowless (OSR) hosts emit this after writing the latest frame to the
    /// frames directory; the desktop reads `<tab>.bgra` and composites it.
    FrameReady {
        tab_id: BrowserTabId,
        width: u32,
        height: u32,
    },
    RuntimeCrashed {
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CefBrowserShortcut {
    FocusAddress,
    NewTab,
    CloseTab,
    Reload,
    Back,
    Forward,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_envelope_round_trips_as_one_json_line() {
        let envelope = CefHostCommandEnvelope::new(
            42,
            CefHostCommand::CreateTab {
                tab_id: BrowserTabId::from("tab-1"),
                url: "https://example.com/".into(),
                bounds: CefBounds {
                    x: 10,
                    y: 20,
                    width: 800,
                    height: 600,
                    scale_factor: 2.0,
                },
                visible: true,
            },
        );
        let json = serde_json::to_string(&envelope).expect("serialize CEF command");
        assert!(!json.contains('\n'));
        assert_eq!(
            serde_json::from_str::<CefHostCommandEnvelope>(&json).expect("decode CEF command"),
            envelope
        );
    }

    #[test]
    fn bounds_scale_factor_defaults_to_one_for_legacy_payloads() {
        let bounds: CefBounds = serde_json::from_str(r#"{"x":0,"y":0,"width":4,"height":4}"#)
            .expect("decode legacy bounds without scaleFactor");
        assert_eq!(bounds.scale_factor, 1.0);
    }

    #[test]
    fn native_surface_bounds_are_strictly_bounded() {
        assert!(
            CefBounds {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
                scale_factor: 1.0,
            }
            .validated()
            .is_some()
        );
        assert!(
            CefBounds {
                x: 0,
                y: 0,
                width: 0,
                height: 600,
                scale_factor: 1.0,
            }
            .validated()
            .is_none()
        );
        assert!(
            CefBounds {
                x: 0,
                y: 0,
                width: 20_000,
                height: 600,
                scale_factor: 1.0,
            }
            .validated()
            .is_none()
        );
        assert!(
            CefBounds {
                x: 0,
                y: 0,
                width: 4,
                height: 4,
                scale_factor: 0.0,
            }
            .validated()
            .is_none()
        );
    }

    #[test]
    fn parent_window_command_round_trips_without_a_tab_scope() {
        let envelope = CefHostCommandEnvelope::new(
            7,
            CefHostCommand::SetParentWindow {
                parent_hwnd: 0x1234,
            },
        );
        let json = serde_json::to_string(&envelope).expect("serialize parent window command");
        assert!(json.contains("\"kind\":\"set_parent_window\""));
        assert_eq!(
            serde_json::from_str::<CefHostCommandEnvelope>(&json)
                .expect("decode parent window command"),
            envelope
        );
        assert!(envelope.command.tab_id().is_none());
    }

    #[test]
    fn send_input_command_round_trips() {
        let envelope = CefHostCommandEnvelope::new(
            9,
            CefHostCommand::SendInput {
                tab_id: BrowserTabId::from("tab-1"),
                event: CefInputEvent::Key {
                    kind: CefKeyEventKind::Char,
                    windows_key_code: 0x41,
                    native_key_code: 0x00,
                    modifiers: cef_event_flags::SHIFT_DOWN,
                    character: 'A' as u16,
                    unmodified_character: 'a' as u16,
                },
            },
        );
        let json = serde_json::to_string(&envelope).expect("serialize input command");
        assert!(json.contains("\"kind\":\"send_input\""));
        assert!(json.contains("\"type\":\"key\""));
        assert!(json.contains("\"kind\":\"char\""));
        assert_eq!(
            serde_json::from_str::<CefHostCommandEnvelope>(&json).expect("decode input command"),
            envelope
        );
        assert_eq!(
            envelope.command.tab_id().map(BrowserTabId::as_str),
            Some("tab-1")
        );
    }
}
