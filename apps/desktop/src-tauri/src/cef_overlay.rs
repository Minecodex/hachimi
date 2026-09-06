//! macOS desktop composite for the windowless (OSR) CEF embedded browser
//! (docs/mac-plan/phase-4-cef-embedded-browser.md, P4-C).
//!
//! One `NSImageView`-derived overlay per browser tab is attached above the
//! workbench window's WKWebView — the AppKit equivalent of the Windows
//! sibling-HWND z-order. The CEF host writes BGRA frames into the profile's
//! `frames/` directory and emits `FrameReady` over IPC; this module reads the
//! frames and forwards AppKit input back as `SendInput` commands.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use hachimi_browser::{
    CefBounds, CefInputEvent, CefKeyEventKind, CefMouseButton, mac_event_modifiers_to_cef,
    mac_key_code_to_windows_vk,
};
use hachimi_protocol::BrowserTabId;
use objc2::rc::Retained;
use objc2::{
    AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
};
use objc2_app_kit::{NSBitmapImageRep, NSEvent, NSImage, NSImageView, NSView, NSWindow};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use parking_lot::Mutex;
use tauri::{AppHandle, Manager, Runtime, WebviewWindow};

/// Actions overlay views report back into the embedded browser service.
pub enum CefOverlayAction {
    Input(CefInputEvent),
    /// The overlay was clicked: focus the OSR browser.
    Focus,
}

type OverlayHandler = Arc<dyn Fn(BrowserTabId, CefOverlayAction) + Send + Sync>;

struct OverlayIvars {
    tab_id: BrowserTabId,
    handler: OverlayHandler,
}

define_class! {
    #[unsafe(super(NSImageView))]
    #[name = "HachimiCefOverlayView"]
    #[ivars = OverlayIvars]
    struct CefOverlayView;

    impl CefOverlayView {
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.grab_focus();
            self.send_mouse_button(event, CefMouseButton::Left, false);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            self.send_mouse_button(event, CefMouseButton::Left, true);
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, event: &NSEvent) {
            self.grab_focus();
            self.send_mouse_button(event, CefMouseButton::Right, false);
        }

        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up(&self, event: &NSEvent) {
            self.send_mouse_button(event, CefMouseButton::Right, true);
        }

        #[unsafe(method(otherMouseDown:))]
        fn other_mouse_down(&self, event: &NSEvent) {
            self.send_mouse_button(event, CefMouseButton::Middle, false);
        }

        #[unsafe(method(otherMouseUp:))]
        fn other_mouse_up(&self, event: &NSEvent) {
            self.send_mouse_button(event, CefMouseButton::Middle, true);
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            self.send_mouse_move(event, false);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            self.send_mouse_move(event, false);
        }

        #[unsafe(method(rightMouseDragged:))]
        fn right_mouse_dragged(&self, event: &NSEvent) {
            self.send_mouse_move(event, false);
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            // CEF expects pixel deltas; AppKit reports line deltas unless the
            // device has precise (trackpad) scrolling.
            let unit = if event.hasPreciseScrollingDeltas() {
                1.0
            } else {
                32.0
            };
            let delta_x = (event.scrollingDeltaX() * unit).round() as i32;
            let delta_y = (event.scrollingDeltaY() * unit).round() as i32;
            if delta_x == 0 && delta_y == 0 {
                return;
            }
            let (x, y) = self.local_point(event);
            self.report(CefOverlayAction::Input(CefInputEvent::MouseWheel {
                x,
                y,
                modifiers: event_modifiers(event),
                delta_x,
                delta_y,
            }));
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            let modifiers = event_modifiers(event);
            let native_key_code = i32::from(event.keyCode());
            let windows_key_code = mac_key_code_to_windows_vk(event.keyCode());
            self.send_key(
                CefKeyEventKind::RawKeyDown,
                windows_key_code,
                native_key_code,
                modifiers,
                0,
                0,
            );
            let characters = event
                .characters()
                .map(|value| value.to_string())
                .unwrap_or_default();
            let unmodified = event
                .charactersIgnoringModifiers()
                .map(|value| value.to_string())
                .unwrap_or_default();
            // CEF KeyEvent.character is a single UTF-16 code unit.
            let mut unmodified_units = unmodified.encode_utf16();
            for unit in characters.encode_utf16() {
                let unmodified_character = unmodified_units.next().unwrap_or(0);
                self.send_key(
                    CefKeyEventKind::Char,
                    windows_key_code,
                    native_key_code,
                    modifiers,
                    unit,
                    unmodified_character,
                );
            }
        }

        #[unsafe(method(keyUp:))]
        fn key_up(&self, event: &NSEvent) {
            self.send_key(
                CefKeyEventKind::KeyUp,
                mac_key_code_to_windows_vk(event.keyCode()),
                i32::from(event.keyCode()),
                event_modifiers(event),
                0,
                0,
            );
        }
    }
}

impl CefOverlayView {
    fn new(mtm: MainThreadMarker, tab_id: BrowserTabId, handler: OverlayHandler) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(OverlayIvars { tab_id, handler });
        let view: Retained<Self> = unsafe {
            msg_send![super(this), initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1.0, 1.0))]
        };
        view.setHidden(true);
        view
    }

    fn report(&self, action: CefOverlayAction) {
        let ivars = self.ivars();
        (ivars.handler)(ivars.tab_id.clone(), action);
    }

    fn grab_focus(&self) {
        if let Some(window) = self.window() {
            let responder: &objc2_app_kit::NSResponder = self;
            let _ = window.makeFirstResponder(Some(responder));
        }
        self.report(CefOverlayAction::Focus);
    }

    /// Converts an event location into tab-viewport coordinates: logical
    /// points, top-left origin (web convention).
    fn local_point(&self, event: &NSEvent) -> (i32, i32) {
        let local = self.convertPoint_fromView(event.locationInWindow(), None);
        let height = self.bounds().size.height;
        (local.x.round() as i32, (height - local.y).round() as i32)
    }

    fn send_mouse_move(&self, event: &NSEvent, leave: bool) {
        let (x, y) = self.local_point(event);
        self.report(CefOverlayAction::Input(CefInputEvent::MouseMove {
            x,
            y,
            modifiers: event_modifiers(event),
            leave,
        }));
    }

    fn send_mouse_button(&self, event: &NSEvent, button: CefMouseButton, up: bool) {
        let (x, y) = self.local_point(event);
        self.report(CefOverlayAction::Input(CefInputEvent::MouseButton {
            x,
            y,
            modifiers: event_modifiers(event),
            button,
            up,
            click_count: event.clickCount() as i32,
        }));
    }

    fn send_key(
        &self,
        kind: CefKeyEventKind,
        windows_key_code: i32,
        native_key_code: i32,
        modifiers: u32,
        character: u16,
        unmodified_character: u16,
    ) {
        self.report(CefOverlayAction::Input(CefInputEvent::Key {
            kind,
            windows_key_code,
            native_key_code,
            modifiers,
            character,
            unmodified_character,
        }));
    }
}

fn event_modifiers(event: &NSEvent) -> u32 {
    mac_event_modifiers_to_cef(event.modifierFlags().0 as u64)
}

/// Builds an NSImage from a CEF BGRA frame (swizzled into an owned RGBA
/// bitmap so the image outlives the frame bytes).
fn bgra_to_nsimage(bytes: &[u8], width: u32, height: u32) -> Option<Retained<NSImage>> {
    let mut rgba = bytes.to_vec();
    for px in rgba.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            width as isize,
            height as isize,
            8,
            4,
            true,
            false,
            objc2_app_kit::NSCalibratedRGBColorSpace,
            (width * 4) as isize,
            32,
        )
    }?;
    let dst = rep.bitmapData();
    if dst.is_null() {
        return None;
    }
    unsafe { std::ptr::copy_nonoverlapping(rgba.as_ptr(), dst, rgba.len()) };
    let cg_image = rep.CGImage()?;
    let size = NSSize::new(f64::from(width), f64::from(height));
    Some(NSImage::initWithCGImage_size(
        NSImage::alloc(),
        &cg_image,
        size,
    ))
}

/// Overlay views plus the content view they attach to. All access happens on
/// the AppKit main thread via `run_on_main_thread`.
#[derive(Default)]
struct MainThreadViews {
    content_view: Option<Retained<NSView>>,
    overlays: BTreeMap<BrowserTabId, Retained<CefOverlayView>>,
}

// SAFETY: `MainThreadViews` is only accessed inside `run_on_main_thread`
// closures, which execute on the AppKit main thread.
unsafe impl Send for MainThreadViews {}
unsafe impl Sync for MainThreadViews {}

#[derive(Default)]
struct CefOverlayInner {
    frames_dir: Mutex<Option<PathBuf>>,
    handler: Mutex<Option<OverlayHandler>>,
    views: Mutex<MainThreadViews>,
}

/// Owns the macOS overlay views of every embedded browser tab.
#[derive(Clone, Default)]
pub struct CefOverlayManager {
    inner: Arc<CefOverlayInner>,
}

impl CefOverlayManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Directory the CEF host writes `<tab>.bgra` frames into.
    pub fn set_frames_dir(&self, dir: PathBuf) {
        *self.inner.frames_dir.lock() = Some(dir);
    }

    pub fn set_input_handler(&self, handler: OverlayHandler) {
        *self.inner.handler.lock() = Some(handler);
    }

    /// Captures the workbench window's content view (idempotent).
    pub fn attach<R: Runtime>(&self, window: &WebviewWindow<R>) {
        let inner = Arc::clone(&self.inner);
        let window = window.clone();
        let app = window.app_handle().clone();
        let _ = app.run_on_main_thread(move || {
            let mut views = inner.views.lock();
            if views.content_view.is_some() {
                return;
            }
            let Ok(ns_window) = window.ns_window() else {
                return;
            };
            let ns_window: &NSWindow = unsafe { &*ns_window.cast() };
            ns_window.setAcceptsMouseMovedEvents(true);
            views.content_view = ns_window.contentView();
        });
    }

    /// Creates/positions/shows (or hides) the overlay of a tab. `bounds` are
    /// physical pixels with a top-left origin; AppKit wants points with a
    /// bottom-left origin.
    pub fn sync_tab<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        tab_id: &BrowserTabId,
        bounds: CefBounds,
        visible: bool,
    ) {
        let inner = Arc::clone(&self.inner);
        let tab_id = tab_id.clone();
        let _ = app.run_on_main_thread(move || {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let Some(handler) = inner.handler.lock().clone() else {
                return;
            };
            let mut views = inner.views.lock();
            let Some(content) = views.content_view.clone() else {
                return;
            };
            let scale = f64::from(bounds.scale_factor).max(0.5);
            let width = bounds.width as f64 / scale;
            let height = bounds.height as f64 / scale;
            let frame = NSRect::new(
                NSPoint::new(
                    bounds.x as f64 / scale,
                    content.bounds().size.height - bounds.y as f64 / scale - height,
                ),
                NSSize::new(width, height),
            );
            let overlay = views.overlays.entry(tab_id.clone()).or_insert_with(|| {
                let view = CefOverlayView::new(mtm, tab_id.clone(), handler);
                content.addSubview_positioned_relativeTo(
                    &view,
                    objc2_app_kit::NSWindowOrderingMode::Above,
                    None,
                );
                view
            });
            overlay.setFrame(frame);
            overlay.setHidden(!visible);
        });
    }

    pub fn remove_tab<R: Runtime>(&self, app: &AppHandle<R>, tab_id: &BrowserTabId) {
        let inner = Arc::clone(&self.inner);
        let tab_id = tab_id.clone();
        let _ = app.run_on_main_thread(move || {
            if let Some(overlay) = inner.views.lock().overlays.remove(&tab_id) {
                overlay.removeFromSuperview();
            }
        });
    }

    /// Reads the latest frame of a tab and composites it into its overlay.
    pub fn frame_ready<R: Runtime>(&self, app: &AppHandle<R>, tab_id: &BrowserTabId) {
        let Some(dir) = self.inner.frames_dir.lock().clone() else {
            return;
        };
        let Ok(bytes) = std::fs::read(dir.join(tab_id.as_str()).with_extension("bgra")) else {
            return;
        };
        if bytes.len() < 4 || bytes.len() % 4 != 0 {
            return;
        }
        // The .json sidecar holds the authoritative dimensions; the file is
        // replaced atomically so a matching header always exists.
        let Ok(header) = std::fs::read(dir.join(tab_id.as_str()).with_extension("json"))
            .map_err(drop)
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).map_err(drop))
        else {
            return;
        };
        let width = header["width"].as_u64().unwrap_or(0) as u32;
        let height = header["height"].as_u64().unwrap_or(0) as u32;
        if width == 0 || height == 0 || bytes.len() != (width as usize) * (height as usize) * 4 {
            return;
        }
        let inner = Arc::clone(&self.inner);
        let tab_id = tab_id.clone();
        let _ = app.run_on_main_thread(move || {
            let Some(image) = bgra_to_nsimage(&bytes, width, height) else {
                return;
            };
            let views = inner.views.lock();
            if let Some(overlay) = views.overlays.get(&tab_id) {
                overlay.setImage(Some(&image));
            }
        });
    }
}
