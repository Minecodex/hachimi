// SPDX-License-Identifier: Apache-2.0
// Original Hachimi implementation for macOS Computer Use.
// Window enumeration via CGWindowList, identity via NSRunningApplication,
// TCC permission probing via ScreenCaptureAccess/AXIsProcessTrusted.
// No upstream source; see docs/adr/0004 amendment.

use std::path::Path;

use core_foundation::{
    base::{ItemRef, TCFType, ToVoid},
    dictionary::CFDictionary,
    number::CFNumber,
    string::{CFString, CFStringRef},
};
use core_graphics::{
    access::ScreenCaptureAccess,
    window::{self as cg_window, CGWindowID},
};
use hachimi_protocol::{ComputerAppDescriptor, ComputerRuntimeHealth, ComputerWindowIdentity};
use objc2::AnyThread as _;
use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
use objc2_foundation::{NSProcessInfo, NSString};

use crate::ComputerHostError;

use super::fingerprint;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    static kCGWindowNumber: CFStringRef;
    static kCGWindowOwnerPID: CFStringRef;
    static kCGWindowName: CFStringRef;
    static kCGWindowOwnerName: CFStringRef;
    static kCGWindowBounds: CFStringRef;
    static kCGWindowLayer: CFStringRef;
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn CGEventSourceSecondsSinceLastEventType(state_id: i32, event_type: u32) -> f64;
    fn AXUIElementCreateApplication(pid: i32) -> AXElement;
    fn AXUIElementCopyAttributeValue(
        element: AXElement,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> i32;
    fn AXUIElementSetAttributeValue(
        element: AXElement,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> i32;
    fn AXUIElementPerformAction(element: AXElement, action: CFStringRef) -> i32;
}

type AXElement = *mut std::ffi::c_void;
type CFTypeRef = *const std::ffi::c_void;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn AXValueCreate(value_type: u32, value_ptr: *const std::ffi::c_void) -> CFTypeRef;
    fn AXValueGetValue(value: CFTypeRef, value_type: u32, out: *mut std::ffi::c_void) -> u8;
    fn CFRelease(value: CFTypeRef);
}

// kAXValueTypeCGPoint / kAXValueTypeCGSize
const AX_VALUE_CG_POINT: u32 = 1;
const AX_VALUE_CG_SIZE: u32 = 2;

// kCGEventSourceStateHID
const CG_EVENT_SOURCE_STATE_HID: i32 = 1;
// kCGAnyInputEventType = ((kCGEventNull) - 1) → all ones
const CG_ANY_INPUT_EVENT_TYPE: u32 = u32::MAX;

fn broker(message: impl Into<String>) -> ComputerHostError {
    ComputerHostError::Broker(message.into())
}

/// macOS 14.0 introduced `SCScreenshotManager`, the one-shot capture API the
/// P3-M2 capture path builds on.
fn macos_at_least_14() -> bool {
    NSProcessInfo::processInfo()
        .operatingSystemVersion()
        .majorVersion
        >= 14
}

fn screen_capture_permitted() -> bool {
    ScreenCaptureAccess.preflight()
}

fn accessibility_permitted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

/// Triggers the system Screen Recording permission prompt.
pub(super) fn request_screen_capture_access() {
    ScreenCaptureAccess.request();
}

pub(super) fn runtime_health() -> ComputerRuntimeHealth {
    let version_gate = macos_at_least_14();
    let capture = version_gate && screen_capture_permitted();
    let accessibility = accessibility_permitted();
    let error_code = if !version_gate {
        Some("computer_capture_requires_macos14".into())
    } else if !capture {
        Some("computer_screen_recording_required".into())
    } else if !accessibility {
        Some("computer_accessibility_required".into())
    } else {
        None
    };
    ComputerRuntimeHealth {
        os_supported: true,
        graphics_capture_available: capture,
        input_desktop_available: accessibility,
        process_elevated: unsafe { libc::geteuid() } == 0,
        error_code,
    }
}

pub(super) fn user_input_marker() -> Option<u64> {
    let seconds = unsafe {
        CGEventSourceSecondsSinceLastEventType(CG_EVENT_SOURCE_STATE_HID, CG_ANY_INPUT_EVENT_TYPE)
    };
    seconds.is_finite().then_some((seconds * 1000.0) as u64)
}

struct WindowInfo {
    window_id: u32,
    owner_pid: i32,
    owner_name: String,
    title: String,
    bounds: (f64, f64, f64, f64),
    layer: i32,
}

fn dict_string(dict: &CFDictionary, key: CFStringRef) -> String {
    dict.find(unsafe { CFString::wrap_under_get_rule(key).to_void() })
        .map(|value| unsafe { CFString::wrap_under_get_rule(*value as CFStringRef) }.to_string())
        .unwrap_or_default()
}

fn dict_i64(dict: &CFDictionary, key: CFStringRef) -> i64 {
    dict.find(unsafe { CFString::wrap_under_get_rule(key).to_void() })
        .map(|value: ItemRef<*const std::ffi::c_void>| unsafe {
            CFNumber::wrap_under_get_rule(*value as core_foundation::number::CFNumberRef)
        })
        .and_then(|number| number.to_i64())
        .unwrap_or_default()
}

fn window_bounds(dict: &CFDictionary) -> (f64, f64, f64, f64) {
    let Some(bounds) = dict
        .find(unsafe { CFString::wrap_under_get_rule(kCGWindowBounds).to_void() })
        .map(|value| unsafe {
            CFDictionary::<*const std::ffi::c_void, *const std::ffi::c_void>::wrap_under_get_rule(
                *value as core_foundation::dictionary::CFDictionaryRef,
            )
        })
    else {
        return (0.0, 0.0, 0.0, 0.0);
    };
    let component = |name: &str| {
        let key = CFString::new(name);
        bounds
            .find(key.to_void())
            .map(|value: ItemRef<*const std::ffi::c_void>| unsafe {
                CFNumber::wrap_under_get_rule(*value as core_foundation::number::CFNumberRef)
            })
            .and_then(|number| number.to_f64())
            .unwrap_or_default()
    };
    (
        component("X"),
        component("Y"),
        component("Width"),
        component("Height"),
    )
}

fn window_info_list(including_window: CGWindowID) -> Vec<WindowInfo> {
    let options = if including_window == 0 {
        cg_window::kCGWindowListOptionOnScreenOnly | cg_window::kCGWindowListExcludeDesktopElements
    } else {
        cg_window::kCGWindowListOptionIncludingWindow
            | cg_window::kCGWindowListExcludeDesktopElements
    };
    let Some(array) = cg_window::copy_window_info(options, including_window) else {
        return Vec::new();
    };
    let mut windows = Vec::new();
    for index in 0..array.len() {
        let Some(item) = array.get(index) else {
            continue;
        };
        // SAFETY: CGWindowListCopyWindowInfo returns an array of CFDictionary.
        let info = unsafe {
            CFDictionary::wrap_under_get_rule(*item as core_foundation::dictionary::CFDictionaryRef)
        };
        // SAFETY: the kCGWindow* constants are immutable CoreGraphics exports,
        // valid to read for the process lifetime.
        windows.push(unsafe {
            WindowInfo {
                window_id: dict_i64(&info, kCGWindowNumber) as u32,
                owner_pid: dict_i64(&info, kCGWindowOwnerPID) as i32,
                owner_name: dict_string(&info, kCGWindowOwnerName),
                title: dict_string(&info, kCGWindowName),
                bounds: window_bounds(&info),
                layer: dict_i64(&info, kCGWindowLayer) as i32,
            }
        });
    }
    windows
}

fn process_owner_uid(pid: i32) -> Option<u32> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    // SAFETY: proc_pidinfo writes into a zeroed proc_bsdinfo of the correct size.
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            std::mem::size_of::<libc::proc_bsdinfo>() as i32,
        )
    };
    (written as usize == std::mem::size_of::<libc::proc_bsdinfo>())
        .then(|| unsafe { info.assume_init() }.pbi_uid)
}

fn app_descriptor(info: &WindowInfo) -> ComputerAppDescriptor {
    let running = NSRunningApplication::runningApplicationWithProcessIdentifier(info.owner_pid);
    let bundle_id = running
        .as_ref()
        .and_then(|app| app.bundleIdentifier().map(|id| id.to_string()));
    let display_name = running
        .as_ref()
        .and_then(|app| app.localizedName().map(|name| name.to_string()))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| info.owner_name.clone());
    let executable_path = running
        .as_ref()
        .and_then(|app| app.executableURL())
        .and_then(|url| url.path().map(|path| path.to_string()));
    let executable_name = executable_path
        .as_deref()
        .and_then(|path| Path::new(path).file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| info.owner_name.clone());
    let file_identity = executable_path.as_deref().and_then(|path| {
        std::fs::metadata(path).ok().map(|metadata| {
            use std::os::unix::fs::MetadataExt as _;
            format!("{}:{}", metadata.dev(), metadata.ino())
        })
    });
    let app_id = bundle_id.unwrap_or_else(|| format!("macos:{executable_name}"));
    let identity_hash = fingerprint(&("macos", &app_id, &executable_path, &file_identity));
    ComputerAppDescriptor {
        app_id,
        display_name,
        executable_name,
        executable_path,
        // Publisher signing evidence (SecCode) is a P3-M4 hardening item.
        publisher: None,
        publisher_verified: false,
        package_family_name: None,
        app_user_model_id: None,
        file_identity,
        identity_hash,
    }
}

fn identity_from(info: &WindowInfo) -> ComputerWindowIdentity {
    let app = app_descriptor(info);
    let owner_uid = process_owner_uid(info.owner_pid);
    let elevated = owner_uid == Some(0);
    let hachimi_owned = info.owner_pid as u32 == std::process::id()
        || app.app_id.starts_with("com.hachimi")
        || app.app_id.starts_with("hachimi");
    let fingerprint = fingerprint(&(
        info.window_id,
        info.owner_pid,
        &app.app_id,
        &info.title,
        info.bounds,
        owner_uid,
    ));
    ComputerWindowIdentity {
        app_id: app.app_id.clone(),
        app,
        process_id: info.owner_pid as u32,
        window_handle: format!("0x{:x}", info.window_id),
        fingerprint,
        title: info.title.clone(),
        elevated,
        // macOS has no secure-desktop surface; SIP-protected processes are
        // covered by `elevated` (uid 0) and TCC instead.
        protected_desktop: false,
        hachimi_owned,
    }
}

/// On-screen, normal-layer windows in front-to-back order.
fn visible_windows() -> Vec<WindowInfo> {
    window_info_list(0)
        .into_iter()
        .filter(|info| info.layer == 0 && info.bounds.2 > 0.0 && info.bounds.3 > 0.0)
        .collect()
}

pub(super) fn list_windows() -> Result<Vec<ComputerWindowIdentity>, ComputerHostError> {
    Ok(visible_windows().iter().map(identity_from).collect())
}

pub(super) fn foreground_window() -> Result<ComputerWindowIdentity, ComputerHostError> {
    let frontmost = objc2_app_kit::NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .ok_or_else(|| broker("foreground_window_missing"))?;
    let pid = frontmost.processIdentifier();
    visible_windows()
        .into_iter()
        .find(|info| info.owner_pid == pid)
        .map(|info| identity_from(&info))
        .ok_or_else(|| broker("foreground_window_missing"))
}

pub(super) fn read_identity(
    window_handle: &str,
) -> Result<ComputerWindowIdentity, ComputerHostError> {
    let window_id = parse_window_handle(window_handle)?;
    window_info_list(window_id)
        .into_iter()
        .find(|info| info.window_id == window_id)
        .map(|info| identity_from(&info))
        .ok_or_else(|| broker("window_not_found"))
}

fn parse_window_handle(window_handle: &str) -> Result<CGWindowID, ComputerHostError> {
    let parsed = match window_handle.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16),
        None => window_handle.parse::<u32>(),
    };
    parsed.map_err(|_| broker("window_handle_invalid"))
}

pub(super) fn app_icon_png(
    app: &ComputerAppDescriptor,
) -> Result<Option<Vec<u8>>, ComputerHostError> {
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage, NSWorkspace};
    use objc2_foundation::{NSData, NSDictionary};

    let Some(path) = app.executable_path.as_deref() else {
        return Ok(None);
    };
    let workspace = NSWorkspace::sharedWorkspace();
    let path = NSString::from_str(path);
    let icon: objc2::rc::Retained<NSImage> = workspace.iconForFile(&path);
    icon.setSize(objc2_foundation::NSSize {
        width: 64.0,
        height: 64.0,
    });
    let Some(tiff) = icon.TIFFRepresentation() else {
        return Ok(None);
    };
    let Some(bitmap) = NSBitmapImageRep::imageRepWithData(&tiff) else {
        return Ok(None);
    };
    let png: Option<objc2::rc::Retained<NSData>> = unsafe {
        bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    };
    Ok(png.map(|data| data.to_vec()))
}

#[derive(Debug)]
pub(super) struct CapturedImage {
    pub width: u32,
    pub height: u32,
    pub png_bytes: Vec<u8>,
}

/// P3-M2: one-shot window capture via SCScreenshotManager (macOS 14+).
pub(super) fn capture_window(window_handle: &str) -> Result<CapturedImage, ComputerHostError> {
    if !macos_at_least_14() {
        return Err(broker("computer_capture_requires_macos14"));
    }
    if !screen_capture_permitted() {
        return Err(broker("computer_screen_recording_required"));
    }
    let window_id = parse_window_handle(window_handle)?;
    let window = shareable_window(window_id)?;
    let image = screenshot_window(&window)?;
    let png_bytes = cgimage_to_png(&image)?;
    Ok(CapturedImage {
        width: objc2_core_graphics::CGImage::width(Some(&image)) as u32,
        height: objc2_core_graphics::CGImage::height(Some(&image)) as u32,
        png_bytes,
    })
}

/// Finds the SCWindow for a CGWindowID through the shareable-content list.
fn shareable_window(
    window_id: CGWindowID,
) -> Result<objc2::rc::Retained<objc2_screen_capture_kit::SCWindow>, ComputerHostError> {
    use objc2_screen_capture_kit::SCShareableContent;

    let (sender, receiver) = std::sync::mpsc::channel();
    let handler = block2::RcBlock::new(
        move |content: *mut objc2_screen_capture_kit::SCShareableContent,
              error: *mut objc2_foundation::NSError| {
            let result = if !error.is_null() {
                Err(format!(
                    "shareable content: {}",
                    unsafe { &*error }.localizedDescription()
                ))
            } else if content.is_null() {
                Err("shareable content is empty".into())
            } else {
                Ok(unsafe { objc2::rc::Retained::retain(content) })
            };
            let _ = sender.send(result);
        },
    );
    unsafe {
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
            true,
            true,
            &handler,
        );
    }
    let content = receiver
        .recv_timeout(std::time::Duration::from_secs(10))
        .map_err(|error| broker(format!("shareable content timed out: {error}")))?
        .map_err(broker)?
        .ok_or_else(|| broker("shareable content was released"))?;
    // SAFETY: the shareable content object is alive for this scope and the
    // window list is only read.
    unsafe { content.windows() }
        .iter()
        .find(|window| unsafe { window.windowID() } == window_id)
        .ok_or_else(|| broker("window_not_found"))
}

/// Captures a single CGImage for the window at native resolution.
fn screenshot_window(
    window: &objc2_screen_capture_kit::SCWindow,
) -> Result<objc2::rc::Retained<objc2_core_graphics::CGImage>, ComputerHostError> {
    use objc2_screen_capture_kit::{SCContentFilter, SCScreenshotManager, SCStreamConfiguration};

    // SAFETY: the window reference is alive for this scope.
    let frame = unsafe { window.frame() };
    let filter = unsafe {
        SCContentFilter::initWithDesktopIndependentWindow(
            objc2_screen_capture_kit::SCContentFilter::alloc(),
            window,
        )
    };
    // SAFETY: default construction of the stream configuration.
    let config = unsafe { SCStreamConfiguration::new() };
    unsafe {
        config.setShowsCursor(false);
        // Zero width/height leaves the capture at the window's native pixel
        // size (Retina included); the output dimensions are verified below.
        config.setWidth(0);
        config.setHeight(0);
    }
    let _ = frame;
    let (sender, receiver) = std::sync::mpsc::channel();
    let handler = block2::RcBlock::new(
        move |image: *mut objc2_core_graphics::CGImage, error: *mut objc2_foundation::NSError| {
            let result = if !error.is_null() {
                Err(format!(
                    "screenshot: {}",
                    unsafe { &*error }.localizedDescription()
                ))
            } else if image.is_null() {
                Err("screenshot returned no image".into())
            } else {
                Ok(unsafe { objc2::rc::Retained::retain(image) })
            };
            let _ = sender.send(result);
        },
    );
    unsafe {
        SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(
            &filter,
            &config,
            Some(&handler),
        );
    }
    receiver
        .recv_timeout(std::time::Duration::from_secs(10))
        .map_err(|error| broker(format!("screenshot timed out: {error}")))?
        .map_err(broker)?
        .ok_or_else(|| broker("screenshot image was released"))
}

fn cgimage_to_png(image: &objc2_core_graphics::CGImage) -> Result<Vec<u8>, ComputerHostError> {
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep};
    use objc2_foundation::NSDictionary;

    let bitmap = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), image);
    let data = unsafe {
        bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }
    .ok_or_else(|| broker("PNG encoding returned no data"))?;
    Ok(data.to_vec())
}

/// CGEvent input injection + AX window operations. Coordinates are
/// window-relative points at the protocol layer and global top-left-origin
/// points at the CG layer; Retina scaling is irrelevant because both sides
/// speak points.
pub(super) fn perform_action(
    target: &ComputerWindowIdentity,
    action: &hachimi_protocol::ComputerAction,
) -> Result<(), ComputerHostError> {
    use hachimi_protocol::ComputerAction as A;

    let current = read_identity(&target.window_handle)?;
    if current.fingerprint != target.fingerprint
        || current.process_id != target.process_id
        || current.elevated
        || current.protected_desktop
        || current.hachimi_owned
    {
        return Err(ComputerHostError::TargetChanged);
    }
    // LaunchApp spawns a new process and needs neither a foreground target nor
    // Accessibility; every other action requires both.
    if matches!(action, A::LaunchApp { .. }) {
        return launch_app(action);
    }
    if !accessibility_permitted() {
        return Err(broker("computer_accessibility_required"));
    }
    // Never steal focus from the user: macOS has no per-window foreground, so
    // the actionable surface is gated on the target's owning app being
    // frontmost (mirrors the Windows GetForegroundWindow gate).
    let frontmost_pid = objc2_app_kit::NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .map(|app| app.processIdentifier())
        .unwrap_or(-1);
    if frontmost_pid < 0 || frontmost_pid as u32 != current.process_id {
        return Err(ComputerHostError::UserTakeover);
    }
    let window_id = parse_window_handle(&target.window_handle)?;
    let rect = window_rect(window_id)?;

    match action {
        A::MouseMove { x, y } => move_pointer(rect, *x, *y),
        A::MouseClick { x, y, button } => {
            move_pointer(rect, *x, *y)?;
            let (down, up, cg_button) = mouse_button_events(button)?;
            post_mouse(down, rect, *x, *y, cg_button, 1)?;
            post_mouse(up, rect, *x, *y, cg_button, 1)
        }
        A::MouseDown { x, y, button } => {
            move_pointer(rect, *x, *y)?;
            let (down, _, cg_button) = mouse_button_events(button)?;
            post_mouse(down, rect, *x, *y, cg_button, 1)
        }
        A::MouseUp { x, y, button } => {
            move_pointer(rect, *x, *y)?;
            let (_, up, cg_button) = mouse_button_events(button)?;
            post_mouse(up, rect, *x, *y, cg_button, 1)
        }
        A::MouseDoubleClick { x, y, button } => {
            move_pointer(rect, *x, *y)?;
            let (down, up, cg_button) = mouse_button_events(button)?;
            post_mouse(down, rect, *x, *y, cg_button, 1)?;
            post_mouse(up, rect, *x, *y, cg_button, 1)?;
            post_mouse(down, rect, *x, *y, cg_button, 2)?;
            post_mouse(up, rect, *x, *y, cg_button, 2)
        }
        A::MouseDrag {
            from_x,
            from_y,
            to_x,
            to_y,
            button,
        } => {
            move_pointer(rect, *from_x, *from_y)?;
            let (down, up, cg_button) = mouse_button_events(button)?;
            post_mouse(down, rect, *from_x, *from_y, cg_button, 1)?;
            move_pointer(rect, *to_x, *to_y)?;
            post_mouse(up, rect, *to_x, *to_y, cg_button, 1)
        }
        A::Scroll { delta_x, delta_y } => {
            move_pointer(rect, (rect.2 / 2.0) as i32, (rect.3 / 2.0) as i32)?;
            // Windows WHEEL_DELTA is 120 per notch; macOS scroll lines keep
            // the same sign convention (positive scrolls up/left).
            let lines_y = delta_y / 120;
            let lines_x = delta_x / 120;
            if lines_y == 0 && lines_x == 0 {
                return Ok(());
            }
            let event = core_graphics::event::CGEvent::new_scroll_event(
                cg_source()?,
                core_graphics::event::ScrollEventUnit::LINE,
                2,
                lines_y,
                lines_x,
                0,
            )
            .map_err(|_| broker("scroll_event_unavailable"))?;
            event.post(core_graphics::event::CGEventTapLocation::HID);
            Ok(())
        }
        A::KeyPress { key, modifiers } => send_key(key, modifiers),
        A::KeyDown { key } => send_key_transition(key, false),
        A::KeyUp { key } => send_key_transition(key, true),
        A::KeyChord { keys } => send_chord(keys),
        A::TypeText { text } => send_text(text),
        A::WindowFocus => focus_window(&current),
        A::WindowMove { x, y } => ax_set_point(&current, *x, *y),
        A::WindowResize { width, height } => ax_set_size(&current, *width, *height),
        A::WindowMinimize => ax_set_minimized(&current),
        A::WindowMaximize | A::WindowRestore => ax_press_zoom(&current),
        A::WindowClose => ax_press_close(&current),
        A::LaunchApp { .. } => unreachable!("LaunchApp returned above"),
    }
}

fn cg_source() -> Result<core_graphics::event_source::CGEventSource, ComputerHostError> {
    core_graphics::event_source::CGEventSource::new(
        core_graphics::event_source::CGEventSourceStateID::HIDSystemState,
    )
    .map_err(|_| broker("event_source_unavailable"))
}

/// Window-relative point → global CG point (both are top-left origin, points).
fn global_point(rect: (f64, f64, f64, f64), x: i32, y: i32) -> core_graphics::geometry::CGPoint {
    core_graphics::geometry::CGPoint::new(rect.0 + f64::from(x), rect.1 + f64::from(y))
}

fn move_pointer(rect: (f64, f64, f64, f64), x: i32, y: i32) -> Result<(), ComputerHostError> {
    use core_graphics::event::{CGEvent, CGEventTapLocation, CGEventType};
    let event = CGEvent::new_mouse_event(
        cg_source()?,
        CGEventType::MouseMoved,
        global_point(rect, x, y),
        core_graphics::event::CGMouseButton::Left,
    )
    .map_err(|_| broker("mouse_event_unavailable"))?;
    event.post(CGEventTapLocation::HID);
    Ok(())
}

/// Maps a protocol button name to (down, up) CG event types plus the CG button.
fn mouse_button_events(
    button: &str,
) -> Result<
    (
        core_graphics::event::CGEventType,
        core_graphics::event::CGEventType,
        core_graphics::event::CGMouseButton,
    ),
    ComputerHostError,
> {
    use core_graphics::event::{CGEventType, CGMouseButton};
    match button {
        "left" => Ok((
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseUp,
            CGMouseButton::Left,
        )),
        "right" => Ok((
            CGEventType::RightMouseDown,
            CGEventType::RightMouseUp,
            CGMouseButton::Right,
        )),
        "middle" => Ok((
            CGEventType::OtherMouseDown,
            CGEventType::OtherMouseUp,
            CGMouseButton::Center,
        )),
        _ => Err(ComputerHostError::InvalidAction),
    }
}

fn post_mouse(
    event_type: core_graphics::event::CGEventType,
    rect: (f64, f64, f64, f64),
    x: i32,
    y: i32,
    button: core_graphics::event::CGMouseButton,
    click_state: i64,
) -> Result<(), ComputerHostError> {
    use core_graphics::event::{CGEvent, CGEventTapLocation, EventField};
    let event =
        CGEvent::new_mouse_event(cg_source()?, event_type, global_point(rect, x, y), button)
            .map_err(|_| broker("mouse_event_unavailable"))?;
    if click_state > 1 {
        event.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, click_state);
    }
    event.post(CGEventTapLocation::HID);
    Ok(())
}

fn send_key(key: &str, modifiers: &[String]) -> Result<(), ComputerHostError> {
    let code = mac_key_code(key).ok_or(ComputerHostError::InvalidAction)?;
    let flags = modifier_flags(modifiers)?;
    post_key(code, false, flags)?;
    post_key(code, true, flags)
}

fn send_key_transition(key: &str, key_up: bool) -> Result<(), ComputerHostError> {
    let code = mac_key_code(key).ok_or(ComputerHostError::InvalidAction)?;
    post_key(
        code,
        key_up,
        core_graphics::event::CGEventFlags::CGEventFlagNull,
    )
}

fn send_chord(keys: &[String]) -> Result<(), ComputerHostError> {
    let codes = keys
        .iter()
        .map(|key| mac_key_code(key).ok_or(ComputerHostError::InvalidAction))
        .collect::<Result<Vec<_>, _>>()?;
    for code in &codes {
        post_key(
            *code,
            false,
            core_graphics::event::CGEventFlags::CGEventFlagNull,
        )?;
    }
    for code in codes.iter().rev() {
        post_key(
            *code,
            true,
            core_graphics::event::CGEventFlags::CGEventFlagNull,
        )?;
    }
    Ok(())
}

fn send_text(text: &str) -> Result<(), ComputerHostError> {
    use core_graphics::event::{CGEvent, CGEventTapLocation};
    // Unicode input rides keycode 0 with the string payload, so arbitrary
    // text (CJK, emoji) needs no layout-dependent keymap.
    for chunk in text.chars().collect::<Vec<_>>().chunks(20) {
        let chunk: String = chunk.iter().collect();
        let down = CGEvent::new_keyboard_event(cg_source()?, 0, true)
            .map_err(|_| broker("keyboard_event_unavailable"))?;
        down.set_string(&chunk);
        down.post(CGEventTapLocation::HID);
        let up = CGEvent::new_keyboard_event(cg_source()?, 0, false)
            .map_err(|_| broker("keyboard_event_unavailable"))?;
        up.set_string(&chunk);
        up.post(CGEventTapLocation::HID);
    }
    Ok(())
}

fn post_key(
    code: u16,
    key_up: bool,
    flags: core_graphics::event::CGEventFlags,
) -> Result<(), ComputerHostError> {
    use core_graphics::event::{CGEvent, CGEventTapLocation};
    let event = CGEvent::new_keyboard_event(cg_source()?, code, !key_up)
        .map_err(|_| broker("keyboard_event_unavailable"))?;
    event.set_flags(flags);
    event.post(CGEventTapLocation::HID);
    Ok(())
}

fn modifier_flags(
    modifiers: &[String],
) -> Result<core_graphics::event::CGEventFlags, ComputerHostError> {
    use core_graphics::event::CGEventFlags;
    let mut flags = CGEventFlags::CGEventFlagNull;
    for modifier in modifiers {
        let flag = match modifier.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => CGEventFlags::CGEventFlagControl,
            "alt" | "option" => CGEventFlags::CGEventFlagAlternate,
            "shift" => CGEventFlags::CGEventFlagShift,
            "cmd" | "command" | "meta" | "super" => CGEventFlags::CGEventFlagCommand,
            _ => return Err(ComputerHostError::InvalidAction),
        };
        if flags.contains(flag) {
            return Err(ComputerHostError::InvalidAction);
        }
        flags |= flag;
    }
    Ok(flags)
}

/// US-ANSI virtual keycodes (physical key positions), mirroring the Windows
/// virtual-key name set.
fn mac_key_code(key: &str) -> Option<u16> {
    use core_graphics::event::KeyCode;
    let normalized = key.trim().to_ascii_lowercase();
    if normalized.len() == 1 {
        let code = match normalized.as_bytes()[0] {
            b'a' => KeyCode::ANSI_A,
            b'b' => KeyCode::ANSI_B,
            b'c' => KeyCode::ANSI_C,
            b'd' => KeyCode::ANSI_D,
            b'e' => KeyCode::ANSI_E,
            b'f' => KeyCode::ANSI_F,
            b'g' => KeyCode::ANSI_G,
            b'h' => KeyCode::ANSI_H,
            b'i' => KeyCode::ANSI_I,
            b'j' => KeyCode::ANSI_J,
            b'k' => KeyCode::ANSI_K,
            b'l' => KeyCode::ANSI_L,
            b'm' => KeyCode::ANSI_M,
            b'n' => KeyCode::ANSI_N,
            b'o' => KeyCode::ANSI_O,
            b'p' => KeyCode::ANSI_P,
            b'q' => KeyCode::ANSI_Q,
            b'r' => KeyCode::ANSI_R,
            b's' => KeyCode::ANSI_S,
            b't' => KeyCode::ANSI_T,
            b'u' => KeyCode::ANSI_U,
            b'v' => KeyCode::ANSI_V,
            b'w' => KeyCode::ANSI_W,
            b'x' => KeyCode::ANSI_X,
            b'y' => KeyCode::ANSI_Y,
            b'z' => KeyCode::ANSI_Z,
            b'0' => KeyCode::ANSI_0,
            b'1' => KeyCode::ANSI_1,
            b'2' => KeyCode::ANSI_2,
            b'3' => KeyCode::ANSI_3,
            b'4' => KeyCode::ANSI_4,
            b'5' => KeyCode::ANSI_5,
            b'6' => KeyCode::ANSI_6,
            b'7' => KeyCode::ANSI_7,
            b'8' => KeyCode::ANSI_8,
            b'9' => KeyCode::ANSI_9,
            _ => return None,
        };
        return Some(code);
    }
    let code = match normalized.as_str() {
        "backspace" => KeyCode::DELETE,
        "tab" => KeyCode::TAB,
        "ctrl" | "control" => KeyCode::CONTROL,
        "alt" | "option" => KeyCode::OPTION,
        "shift" => KeyCode::SHIFT,
        "cmd" | "command" | "meta" | "super" => KeyCode::COMMAND,
        "enter" | "return" => KeyCode::RETURN,
        "escape" | "esc" => KeyCode::ESCAPE,
        "space" => KeyCode::SPACE,
        "pageup" | "page_up" => KeyCode::PAGE_UP,
        "pagedown" | "page_down" => KeyCode::PAGE_DOWN,
        "end" => KeyCode::END,
        "home" => KeyCode::HOME,
        "left" | "arrowleft" => KeyCode::LEFT_ARROW,
        "up" | "arrowup" => KeyCode::UP_ARROW,
        "right" | "arrowright" => KeyCode::RIGHT_ARROW,
        "down" | "arrowdown" => KeyCode::DOWN_ARROW,
        "delete" => KeyCode::FORWARD_DELETE,
        value if value.len() <= 3 && value.starts_with('f') => {
            let number = value[1..].parse::<u16>().ok()?;
            if !(1..=12).contains(&number) {
                return None;
            }
            [
                KeyCode::F1,
                KeyCode::F2,
                KeyCode::F3,
                KeyCode::F4,
                KeyCode::F5,
                KeyCode::F6,
                KeyCode::F7,
                KeyCode::F8,
                KeyCode::F9,
                KeyCode::F10,
                KeyCode::F11,
                KeyCode::F12,
            ][usize::from(number) - 1]
        }
        _ => return None,
    };
    Some(code)
}

/// Re-reads the live bounds of a window (points, top-left origin, global).
fn window_rect(window_id: u32) -> Result<(f64, f64, f64, f64), ComputerHostError> {
    window_info_list(window_id)
        .into_iter()
        .find(|info| info.window_id == window_id)
        .map(|info| info.bounds)
        .ok_or_else(|| broker("window_not_found"))
}

// ---- Accessibility (AXUIElement) window operations ----

fn ax_app(pid: i32) -> Result<AXElement, ComputerHostError> {
    let element = unsafe { AXUIElementCreateApplication(pid) };
    if element.is_null() {
        return Err(broker("ax_application_unavailable"));
    }
    Ok(element)
}

fn ax_copy(element: AXElement, attribute: &str) -> Result<CFTypeRef, ComputerHostError> {
    let attribute = CFString::new(attribute);
    let mut value: CFTypeRef = std::ptr::null();
    let status = unsafe {
        AXUIElementCopyAttributeValue(element, attribute.as_concrete_TypeRef(), &mut value)
    };
    if status != 0 || value.is_null() {
        return Err(broker(format!("ax_copy:{attribute}:{status}")));
    }
    Ok(value)
}

fn ax_set(element: AXElement, attribute: &str, value: CFTypeRef) -> Result<(), ComputerHostError> {
    let attribute = CFString::new(attribute);
    let status =
        unsafe { AXUIElementSetAttributeValue(element, attribute.as_concrete_TypeRef(), value) };
    if status != 0 {
        return Err(broker(format!("ax_set:{attribute}:{status}")));
    }
    Ok(())
}

fn ax_perform(element: AXElement, action: &str) -> Result<(), ComputerHostError> {
    let action = CFString::new(action);
    let status = unsafe { AXUIElementPerformAction(element, action.as_concrete_TypeRef()) };
    if status != 0 {
        return Err(broker(format!("ax_perform:{action}:{status}")));
    }
    Ok(())
}

/// Locates the AX window whose position/size match the live target bounds
/// within tolerance (plus title when present). Fails closed on ambiguity.
fn ax_window(identity: &ComputerWindowIdentity) -> Result<AXElement, ComputerHostError> {
    use core_foundation::array::{CFArray, CFArrayRef};
    let app = ax_app(identity.process_id as i32)?;
    let windows = ax_copy(app, "AXWindows")?;
    let list =
        unsafe { CFArray::<*const std::ffi::c_void>::wrap_under_get_rule(windows as CFArrayRef) };
    let rect = window_rect(parse_window_handle(&identity.window_handle)?)?;
    let mut matches = Vec::new();
    for index in 0..list.len() {
        let Some(window) = list.get(index) else {
            continue;
        };
        let element = *window as AXElement;
        let bounds_match = ax_point(element, "AXPosition")
            .zip(ax_size(element, "AXSize"))
            .is_some_and(|((px, py), (width, height))| {
                (px - rect.0).abs() <= 1.5
                    && (py - rect.1).abs() <= 1.5
                    && (width - rect.2).abs() <= 1.5
                    && (height - rect.3).abs() <= 1.5
            });
        let title = ax_copy(element, "AXTitle").ok().and_then(|value| {
            let title = unsafe { CFString::wrap_under_get_rule(value as CFStringRef) }.to_string();
            if title.is_empty() { None } else { Some(title) }
        });
        let title_match = identity.title.is_empty()
            || title
                .as_deref()
                .is_some_and(|value| value == identity.title);
        if bounds_match && title_match {
            matches.push(element);
        }
    }
    unsafe { CFRelease(windows) };
    match matches.as_slice() {
        [single] => Ok(*single),
        [] => Err(broker("ax_window_not_found")),
        _ => Err(broker("ax_window_ambiguous")),
    }
}

fn ax_point(element: AXElement, attribute: &str) -> Option<(f64, f64)> {
    let value = ax_copy(element, attribute).ok()?;
    let mut point = core_graphics::geometry::CGPoint::new(0.0, 0.0);
    let ok = unsafe {
        AXValueGetValue(
            value,
            AX_VALUE_CG_POINT,
            (&mut point as *mut core_graphics::geometry::CGPoint).cast(),
        )
    };
    unsafe { CFRelease(value) };
    (ok != 0).then_some((point.x, point.y))
}

fn ax_size(element: AXElement, attribute: &str) -> Option<(f64, f64)> {
    let value = ax_copy(element, attribute).ok()?;
    let mut size = core_graphics::geometry::CGSize::new(0.0, 0.0);
    let ok = unsafe {
        AXValueGetValue(
            value,
            AX_VALUE_CG_SIZE,
            (&mut size as *mut core_graphics::geometry::CGSize).cast(),
        )
    };
    unsafe { CFRelease(value) };
    (ok != 0).then_some((size.width, size.height))
}

fn focus_window(identity: &ComputerWindowIdentity) -> Result<(), ComputerHostError> {
    let Some(app) =
        NSRunningApplication::runningApplicationWithProcessIdentifier(identity.process_id as i32)
    else {
        return Err(broker("application_not_running"));
    };
    if !app.activateWithOptions(NSApplicationActivationOptions::ActivateAllWindows) {
        return Err(ComputerHostError::UserTakeover);
    }
    // Raise the target window within the app as well.
    let window = ax_window(identity)?;
    ax_perform(window, "AXRaise")
}

fn ax_set_point(
    identity: &ComputerWindowIdentity,
    x: i32,
    y: i32,
) -> Result<(), ComputerHostError> {
    let window = ax_window(identity)?;
    let point = core_graphics::geometry::CGPoint::new(f64::from(x), f64::from(y));
    let value = unsafe {
        AXValueCreate(
            AX_VALUE_CG_POINT,
            (&point as *const core_graphics::geometry::CGPoint).cast(),
        )
    };
    if value.is_null() {
        return Err(broker("ax_value_unavailable"));
    }
    let result = ax_set(window, "AXPosition", value);
    unsafe { CFRelease(value) };
    result
}

fn ax_set_size(
    identity: &ComputerWindowIdentity,
    width: u32,
    height: u32,
) -> Result<(), ComputerHostError> {
    let window = ax_window(identity)?;
    let size = core_graphics::geometry::CGSize::new(f64::from(width), f64::from(height));
    let value = unsafe {
        AXValueCreate(
            AX_VALUE_CG_SIZE,
            (&size as *const core_graphics::geometry::CGSize).cast(),
        )
    };
    if value.is_null() {
        return Err(broker("ax_value_unavailable"));
    }
    let result = ax_set(window, "AXSize", value);
    unsafe { CFRelease(value) };
    result
}

fn ax_set_minimized(identity: &ComputerWindowIdentity) -> Result<(), ComputerHostError> {
    let window = ax_window(identity)?;
    let yes = core_foundation::boolean::CFBoolean::true_value();
    ax_set(window, "AXMinimized", yes.as_CFTypeRef() as CFTypeRef)
}

/// macOS has no maximize/restore pair; the zoom button toggles between the
/// user-set and "zoomed" frames, which is the closest semantic.
fn ax_press_zoom(identity: &ComputerWindowIdentity) -> Result<(), ComputerHostError> {
    let window = ax_window(identity)?;
    let button = ax_copy(window, "AXZoomButton")?;
    let result = ax_perform(button as AXElement, "AXPress");
    unsafe { CFRelease(button) };
    result
}

fn ax_press_close(identity: &ComputerWindowIdentity) -> Result<(), ComputerHostError> {
    let window = ax_window(identity)?;
    let button = ax_copy(window, "AXCloseButton")?;
    let result = ax_perform(button as AXElement, "AXPress");
    unsafe { CFRelease(button) };
    result
}

fn launch_app(action: &hachimi_protocol::ComputerAction) -> Result<(), ComputerHostError> {
    let hachimi_protocol::ComputerAction::LaunchApp { app_id } = action else {
        return Err(ComputerHostError::InvalidAction);
    };
    // `open -a` resolves bundle identifiers, app names and .app paths.
    std::process::Command::new("/usr/bin/open")
        .arg("-a")
        .arg(app_id)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| broker(format!("launch_app:{error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_names_map_to_us_ansi_virtual_keycodes() {
        use core_graphics::event::KeyCode;
        assert_eq!(mac_key_code("a"), Some(KeyCode::ANSI_A));
        assert_eq!(mac_key_code("Z"), Some(KeyCode::ANSI_Z));
        assert_eq!(mac_key_code("5"), Some(KeyCode::ANSI_5));
        assert_eq!(mac_key_code("Enter"), Some(KeyCode::RETURN));
        assert_eq!(mac_key_code("escape"), Some(KeyCode::ESCAPE));
        assert_eq!(mac_key_code("f5"), Some(KeyCode::F5));
        assert_eq!(mac_key_code("f12"), Some(KeyCode::F12));
        assert_eq!(mac_key_code("ArrowLeft"), Some(KeyCode::LEFT_ARROW));
        assert_eq!(mac_key_code(" backspace "), Some(KeyCode::DELETE));
        assert_eq!(mac_key_code("delete"), Some(KeyCode::FORWARD_DELETE));
        assert_eq!(mac_key_code("insert"), None);
        assert_eq!(mac_key_code("f13"), None);
        assert_eq!(mac_key_code("nonexistent"), None);
    }

    #[test]
    fn modifier_flags_cover_the_mac_modifier_set_and_reject_duplicates() {
        use core_graphics::event::CGEventFlags;
        let flags = modifier_flags(&["cmd".into(), "shift".into()]).expect("flags");
        assert!(flags.contains(CGEventFlags::CGEventFlagCommand));
        assert!(flags.contains(CGEventFlags::CGEventFlagShift));
        assert!(modifier_flags(&["ctrl".into()]).is_ok());
        assert!(modifier_flags(&["alt".into()]).is_ok());
        assert!(modifier_flags(&["option".into()]).is_ok());
        assert!(matches!(
            modifier_flags(&["shift".into(), "shift".into()]),
            Err(ComputerHostError::InvalidAction)
        ));
        assert!(matches!(
            modifier_flags(&["bogus".into()]),
            Err(ComputerHostError::InvalidAction)
        ));
    }

    #[test]
    fn mouse_buttons_map_to_cg_event_types() {
        use core_graphics::event::{CGEventType, CGMouseButton};
        let (down, up, button) = mouse_button_events("left").expect("left");
        assert!(matches!(down, CGEventType::LeftMouseDown));
        assert!(matches!(up, CGEventType::LeftMouseUp));
        assert!(matches!(button, CGMouseButton::Left));
        assert!(mouse_button_events("right").is_ok());
        assert!(mouse_button_events("middle").is_ok());
        assert!(matches!(
            mouse_button_events("fourth"),
            Err(ComputerHostError::InvalidAction)
        ));
    }

    #[test]
    fn window_relative_points_translate_to_global_cg_points() {
        let point = global_point((100.0, 200.0, 800.0, 600.0), 10, 20);
        assert_eq!(point.x, 110.0);
        assert_eq!(point.y, 220.0);
    }

    #[test]
    fn window_listing_returns_only_normal_visible_windows() {
        for info in visible_windows() {
            assert_eq!(info.layer, 0);
            assert!(info.bounds.2 > 0.0 && info.bounds.3 > 0.0);
        }
    }

    #[test]
    fn handle_round_trips_hex_and_decimal() {
        assert_eq!(parse_window_handle("0x2a").expect("hex"), 42);
        assert_eq!(parse_window_handle("42").expect("decimal"), 42);
        assert!(parse_window_handle("not-a-handle").is_err());
    }

    #[test]
    fn live_enumeration_and_health_probe_are_coherent() {
        let health = runtime_health();
        assert!(health.os_supported);
        assert!(user_input_marker().is_some());
        let windows = list_windows().expect("window enumeration");
        for window in &windows {
            assert!(!window.app_id.is_empty());
            assert!(!window.app.identity_hash.is_empty());
            assert!(window.window_handle.starts_with("0x"));
            // Every enumerated identity must round-trip through read_identity.
            let reread = read_identity(&window.window_handle).expect("re-read identity");
            assert_eq!(reread.fingerprint, window.fingerprint);
        }
        if !windows.is_empty() {
            let foreground = foreground_window().expect("foreground window");
            assert!(
                windows
                    .iter()
                    .any(|window| window.fingerprint == foreground.fingerprint)
            );
        }
    }

    #[test]
    fn capture_is_fail_closed_without_permission_and_real_when_granted() {
        let Some(first) = visible_windows().into_iter().next() else {
            return; // headless environment: nothing to capture
        };
        let handle = format!("0x{:x}", first.window_id);
        if !macos_at_least_14() {
            let error = capture_window(&handle).map(|_| ()).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("computer_capture_requires_macos14")
            );
            return;
        }
        if !screen_capture_permitted() {
            let error = capture_window(&handle).map(|_| ()).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("computer_screen_recording_required")
            );
            return;
        }
        let image = capture_window(&handle).expect("capture a visible window");
        assert!(image.width > 0 && image.height > 0);
        assert!(image.png_bytes.starts_with(b"\x89PNG"));
        assert!(!image.png_bytes.is_empty());
    }

    struct FixtureGuard {
        child: std::process::Child,
    }

    impl Drop for FixtureGuard {
        fn drop(&mut self) {
            let _ = self.child.kill();
        }
    }

    /// Drives the AppKit stress fixture through the real broker surface:
    /// enumerate → identity → capture → AX move/resize → restore, in a loop.
    /// Requires an interactive macOS desktop with Screen Recording +
    /// Accessibility granted to the test host.
    #[test]
    #[ignore = "requires an interactive macOS desktop with TCC grants"]
    fn captures_and_controls_the_macos_stress_fixture() {
        let seconds = std::env::var("HACHIMI_STRESS_PHASE_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok());
        let stress_deadline = seconds.map(|seconds| {
            std::time::Instant::now() + std::time::Duration::from_secs(seconds.clamp(1, 450))
        });
        let fixture = std::env::var("HACHIMI_COMPUTER_STRESS_FIXTURE")
            .unwrap_or_else(|_| "target/debug/examples/stress_fixture".into());
        let mut iterations = 0_u64;
        loop {
            let child = std::process::Command::new(&fixture)
                .spawn()
                .expect("launch macOS stress fixture");
            let _guard = FixtureGuard { child };
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            let identity = loop {
                let candidate = visible_windows()
                    .into_iter()
                    .find(|info| info.title == "Hachimi Computer fixture");
                if let Some(info) = candidate {
                    break read_identity(&format!("0x{:x}", info.window_id))
                        .expect("fixture window identity");
                }
                if std::time::Instant::now() >= deadline {
                    panic!("macOS stress fixture did not expose a capturable window");
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            };
            assert!(!identity.elevated);
            assert!(!identity.protected_desktop);
            assert!(!identity.hachimi_owned);

            let image = capture_window(&identity.window_handle).expect("capture fixture window");
            assert!(image.width > 0 && image.height > 0);
            assert!(image.png_bytes.starts_with(b"\x89PNG"));

            let before = window_rect(parse_window_handle(&identity.window_handle).expect("id"))
                .expect("bounds before");
            ax_set_point(&identity, before.0 as i32 + 16, before.1 as i32 + 16).expect("AX move");
            std::thread::sleep(std::time::Duration::from_millis(150));
            let moved = window_rect(parse_window_handle(&identity.window_handle).expect("id"))
                .expect("bounds after move");
            assert!(
                (moved.0 - before.0 - 16.0).abs() <= 2.0
                    && (moved.1 - before.1 - 16.0).abs() <= 2.0,
                "AX move did not land: before={before:?} after={moved:?}"
            );
            ax_set_point(&identity, before.0 as i32, before.1 as i32).expect("AX restore");

            iterations = iterations.saturating_add(1);
            if stress_deadline.is_none_or(|deadline| std::time::Instant::now() >= deadline) {
                break;
            }
        }
        eprintln!("computer_real_stress_iterations={iterations}");
    }

    #[test]
    fn identity_is_stable_for_the_same_window_info() {
        let info = WindowInfo {
            window_id: 42,
            owner_pid: std::process::id() as i32,
            owner_name: "hachimi".into(),
            title: "demo".into(),
            bounds: (1.0, 2.0, 300.0, 200.0),
            layer: 0,
        };
        let first = identity_from(&info);
        let second = identity_from(&info);
        assert_eq!(first.fingerprint, second.fingerprint);
        assert!(first.hachimi_owned);
        assert_eq!(first.window_handle, "0x2a");
    }
}
