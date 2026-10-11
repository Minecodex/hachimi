use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::{AllocAnyThread, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSApplication, NSBitmapImageRep, NSEvent, NSImage, NSImageView, NSView};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use tao::event_loop::EventLoop;
use tao::platform::run_return::EventLoopExtRunReturn;
use tao::window::WindowBuilder;

const WINDOW_W: f64 = 700.0;
const WINDOW_H: f64 = 500.0;
// Overlay area in top-left web coordinates (what the frontend would use).
const OVERLAY_X: f64 = 50.0;
const OVERLAY_Y: f64 = 60.0;
const FRAME_W: u32 = 400;
const FRAME_H: u32 = 300;

static CLICK_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

define_class! {
    #[unsafe(super(NSImageView))]
    struct FrameView;

    impl FrameView {
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &NSEvent) {
            CLICK_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

impl FrameView {
    fn new(mtm: objc2::MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

struct HostPipe {
    child: Child,
    next_request: u64,
}

impl HostPipe {
    fn send(&mut self, command: serde_json::Value) {
        let envelope = serde_json::json!({
            "protocolVersion": 1,
            "requestId": self.next_request,
            "command": command,
        });
        self.next_request += 1;
        let mut stdin = self.child.stdin.take().expect("host stdin");
        let _ = stdin.write_all(serde_json::to_string(&envelope).unwrap().as_bytes());
        let _ = stdin.write_all(b"\n");
        self.child.stdin = Some(stdin);
    }
}

/// Converts a CEF BGRA frame into a CGImage backed by an owned RGBA bitmap
/// (NSBitmapImageRep's 32-bit layout is RGBA, so B/R are swapped into a
/// fresh buffer that the rep owns) so the image stays valid after the frame
/// file bytes are dropped.
fn bgra_to_cgimage(
    bytes: &[u8],
    width: u32,
    height: u32,
) -> Option<Retained<objc2_core_graphics::CGImage>> {
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
    rep.CGImage()
}

/// Overlay frame in AppKit coordinates (bottom-left origin) from top-left web
/// coordinates.
fn overlay_rect() -> NSRect {
    NSRect::new(
        NSPoint::new(OVERLAY_X, WINDOW_H - OVERLAY_Y - FRAME_H as f64),
        NSSize::new(FRAME_W as f64, FRAME_H as f64),
    )
}

/// True when the center pixel of a BGRA frame is the marker page's blue —
/// i.e. the page has loaded and rendered (initial frames are blank).
fn frame_is_blue(bytes: &[u8], width: u32, height: u32) -> bool {
    let offset = ((height / 2) as usize) * (width as usize) * 4 + (width as usize / 2) * 4;
    bytes.len() > offset + 2 && bytes[offset] > 200 && bytes[offset + 2] < 60
}

pub(super) fn run() {
    // Hard watchdog: nothing in this probe may hang the caller, even if the
    // event loop or a stage gets stuck.
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(20));
        eprintln!("[probe] WATCHDOG force-exit");
        std::process::exit(2);
    });

    let profile = PathBuf::from("/tmp/hachimi-cef-composite-probe");
    let frames = profile.join("frames");
    std::fs::remove_dir_all(&profile).ok();
    std::fs::create_dir_all(&profile).expect("profile dir");

    let host_exe = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .join("target/cef-bundle/hachimi-cef-host.app/Contents/MacOS/hachimi-cef-host")
        .canonicalize()
        .expect("bundled CEF host (run pnpm cef:prepare first)");

    let mut child = Command::new(host_exe)
        .arg("--hachimi-osr")
        .arg("--hachimi-parent-hwnd=1")
        .arg(format!("--hachimi-profile-dir={}", profile.display()))
        // The dev machine's system proxy (127.0.0.1:7890) would otherwise
        // hijack the loopback marker page inside CEF.
        .arg("--no-proxy-server")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .env_remove("all_proxy")
        .env_remove("ALL_PROXY")
        .env_remove("http_proxy")
        .env_remove("HTTP_PROXY")
        .env_remove("https_proxy")
        .env_remove("HTTPS_PROXY")
        .spawn()
        .expect("spawn CEF host");
    let stdout = child.stdout.take().expect("host stdout");
    let (line_tx, line_rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if line_tx.send(line).is_err() {
                break;
            }
        }
    });

    // Local HTTP marker page for the tab (solid blue + click counter). Each
    // connection is served on its own thread: Chromium opens several sockets
    // speculatively and a stalled one must not starve the real request. The
    // request headers are drained before responding — closing a socket with
    // unread request data makes the kernel RST the connection and Chromium
    // reports ERR_CONNECTION_RESET.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        while let Ok((stream, _)) = listener.accept() {
            std::thread::spawn(move || {
                let mut stream = stream;
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
                // Drain the request headers.
                let mut received = Vec::new();
                let mut chunk = [0_u8; 4096];
                loop {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => {
                            received.extend_from_slice(&chunk[..read]);
                            if received.windows(4).any(|w| w == b"\r\n\r\n") {
                                break;
                            }
                        }
                    }
                }
                let body = b"<html><head><script>window.clicks=0;document.onclick=()=>{window.clicks+=1;document.title='clicks:'+window.clicks};</script></head><body style='margin:0;background:rgb(0,0,255)'></body></html>";
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.write_all(body);
            });
        }
    });

    let mut event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title("Hachimi CEF composite probe")
        .with_inner_size(tao::dpi::LogicalSize::new(WINDOW_W, WINDOW_H))
        .build(&event_loop)
        .expect("probe window");
    let _webview = wry::WebViewBuilder::new()
        .with_url("data:text/html,<body style='margin:0;background:rgb(255,0,0)'></body>")
        .build(&window)
        .expect("probe webview");

    // The overlay sits above the WKWebView, exactly like the Windows sibling
    // HWND sits above WebView2.
    let overlay = {
        let RawWindowHandle::AppKit(handle) = window.window_handle().expect("handle").as_raw()
        else {
            panic!("expected AppKit window handle");
        };
        let ns_view = handle.ns_view.as_ptr() as *const NSView;
        let content: &NSView = unsafe { &*ns_view };
        let overlay = FrameView::new(
            objc2::MainThreadMarker::new().expect("main thread"),
            overlay_rect(),
        );
        content.addSubview_positioned_relativeTo(
            &overlay,
            objc2_app_kit::NSWindowOrderingMode::Above,
            None,
        );
        overlay
    };

    let mut host = HostPipe {
        child,
        next_request: 1,
    };

    let started = Instant::now();
    let mut stage = 0_u8;
    let mut logged_stage = u8::MAX;
    let mut host_ready = false;
    let mut frames_seen = 0_u32;
    let mut z_order_ok = false;
    let mut input_ok = false;
    let deadline = started + Duration::from_secs(15);
    let mut click_posts = 0_u32;
    let mut last_click_post = started - Duration::from_secs(1);

    'probe: loop {
        if Instant::now() > deadline {
            break 'probe;
        }
        if stage != logged_stage {
            eprintln!(
                "[probe] stage {stage} t+{:.1}s",
                started.elapsed().as_secs_f64()
            );
            logged_stage = stage;
        }
        while let Ok(line) = line_rx.try_recv() {
            if line.contains("\"kind\":\"ready\"") {
                host_ready = true;
            }
            if line.contains("clicks:1") {
                input_ok = true;
            }
            // Surface navigation failures; they explain a page that never
            // renders its marker color.
            if line.contains("navigationError") || line.contains("\"fatal\"") {
                eprintln!("[probe] host: {line}");
            }
        }
        match stage {
            0 if host_ready => {
                host.send(serde_json::json!({
                    "kind": "create_tab",
                    "tab_id": "tab-probe",
                    "url": format!("http://127.0.0.1:{port}/"),
                    "bounds": { "x": 0, "y": 0, "width": FRAME_W, "height": FRAME_H },
                    "visible": true,
                }));
                stage = 1;
            }
            1 => {
                let frame = frames.join("tab-probe.bgra");
                let header = frames.join("tab-probe.json");
                if frame.exists()
                    && header.exists()
                    && let Ok(meta) = std::fs::read(&header).map_err(drop).and_then(|bytes| {
                        serde_json::from_slice::<serde_json::Value>(&bytes).map_err(drop)
                    })
                {
                    let width = meta["width"].as_u64().unwrap_or(0) as u32;
                    let height = meta["height"].as_u64().unwrap_or(0) as u32;
                    if width == FRAME_W
                        && height == FRAME_H
                        && let Ok(bytes) = std::fs::read(&frame)
                        && bytes.len() == (FRAME_W * FRAME_H * 4) as usize
                        && let Some(cg_image) = bgra_to_cgimage(&bytes, width, height)
                    {
                        let size = NSSize::new(width as f64, height as f64);
                        let image =
                            NSImage::initWithCGImage_size(NSImage::alloc(), &cg_image, size);
                        overlay.setImage(Some(&image));
                        frames_seen += 1;
                        // Advance only once the page has actually loaded and
                        // rendered its blue marker — counting bare frames is
                        // flaky because initial frames are blank.
                        if frame_is_blue(&bytes, width, height) {
                            stage = 2;
                        }
                    }
                }
            }
            2 => {
                // Z-order/coords proof:
                //  a) the overlay is the TOPMOST subview of the shared content
                //     view (the AppKit equivalent of the Windows sibling-HWND
                //     z-order above WebView2)
                //  b) a snapshot of the overlay view renders the CEF blue at
                //     Retina-scaled resolution (points → backing pixels proof)
                if let Some(content) = unsafe { overlay.superview() } {
                    let subviews = content.subviews();
                    let subview_count = subviews.len();
                    let topmost_is_overlay = subview_count > 0 && {
                        let last = subviews.objectAtIndex(subview_count - 1);
                        Retained::as_ptr(&last) == Retained::as_ptr(&overlay).cast()
                    };
                    let ordering_ok = subview_count >= 2 && topmost_is_overlay;

                    let scale = window.scale_factor();
                    let mut pixel_ok = false;
                    if let Some(rep) =
                        overlay.bitmapImageRepForCachingDisplayInRect(overlay.bounds())
                    {
                        overlay.cacheDisplayInRect_toBitmapImageRep(overlay.bounds(), &rep);
                        let pixels_wide = rep.pixelsWide() as usize;
                        let pixels_high = rep.pixelsHigh() as usize;
                        let bpp = rep.bitsPerPixel() as usize / 8;
                        let data = rep.bitmapData();
                        if !data.is_null() && pixels_wide * pixels_high * bpp > 0 {
                            let bytes = unsafe {
                                std::slice::from_raw_parts(data, pixels_wide * pixels_high * bpp)
                            };
                            let center = {
                                let off = ((pixels_high / 2) * pixels_wide + pixels_wide / 2) * bpp;
                                [bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]]
                            };
                            let retina_ok = pixels_wide == (FRAME_W as f64 * scale) as usize
                                && pixels_high == (FRAME_H as f64 * scale) as usize;
                            let blue = center[2] > 200 && center[0] < 60;
                            eprintln!(
                                "[probe] subviews={subview_count} topmost_overlay={topmost_is_overlay} \
                                 overlay={pixels_wide}x{pixels_high} scale={scale} center RGBA={center:?}"
                            );
                            pixel_ok = blue && retina_ok;
                        }
                    }
                    z_order_ok = ordering_ok && pixel_ok;
                }
                stage = 3;
            }
            3 => {
                // Input routing: post a real left-click NSEvent at the overlay
                // center; the FrameView mouseDown fires via OS dispatch. On a
                // cold host the window may not be ready to receive the event
                // yet, so repost on a slow timer until it lands (or give up
                // and let the deadline report the failure).
                if CLICK_COUNT.load(std::sync::atomic::Ordering::Relaxed) > 0 {
                    stage = 4;
                } else if click_posts < 10 && last_click_post.elapsed() > Duration::from_millis(500)
                {
                    let center = NSPoint::new(
                        OVERLAY_X + FRAME_W as f64 / 2.0,
                        WINDOW_H - (OVERLAY_Y + FRAME_H as f64 / 2.0),
                    );
                    let window_number = overlay
                        .window()
                        .map(|window| window.windowNumber())
                        .unwrap_or(0);
                    if let Some(event) = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
                        objc2_app_kit::NSEventType::LeftMouseDown,
                        center,
                        objc2_app_kit::NSEventModifierFlags::empty(),
                        0.0,
                        window_number,
                        None,
                        0,
                        1,
                        0.0,
                    ) {
                        NSApplication::sharedApplication(
                            objc2::MainThreadMarker::new().expect("main thread"),
                        )
                        .sendEvent(&event);
                    }
                    click_posts += 1;
                    last_click_post = Instant::now();
                }
            }
            4 if CLICK_COUNT.load(std::sync::atomic::Ordering::Relaxed) > 0 => {
                // The overlay received the OS click; forward the matching CDP
                // input to the host and read the page counter back.
                for r#type in ["mousePressed", "mouseReleased"] {
                    host.send(serde_json::json!({
                        "kind": "dev_tools",
                        "tab_id": "tab-probe",
                        "method": "Input.dispatchMouseEvent",
                        "params": {
                            "type": r#type, "x": 200, "y": 150,
                            "button": "left", "clickCount": 1,
                        },
                        "full_access": true,
                    }));
                }
                std::thread::sleep(Duration::from_millis(300));
                host.send(serde_json::json!({
                    "kind": "dev_tools",
                    "tab_id": "tab-probe",
                    "method": "Runtime.evaluate",
                    "params": { "expression": "document.title", "returnByValue": true },
                    "full_access": true,
                }));
                stage = 5;
            }
            _ => {}
        }
        // Pump pending AppKit events once, then return. With ControlFlow::Poll
        // alone, tao's run_return never yields (the loop always has more work
        // in poll mode) — that is what hung this probe before.
        event_loop.run_return(|event, _target, control| {
            *control = tao::event_loop::ControlFlow::Poll;
            if matches!(event, tao::event::Event::MainEventsCleared) {
                *control = tao::event_loop::ControlFlow::Exit;
            }
        });
        std::thread::sleep(Duration::from_millis(30));
    }

    host.child.kill().ok();
    println!("PROBE z_order={z_order_ok} input={input_ok} frames_seen={frames_seen} stage={stage}");
    println!(
        "COMPOSITE-PROBE-{}",
        if z_order_ok && input_ok { "OK" } else { "FAIL" }
    );
    std::process::exit(if z_order_ok && input_ok { 0 } else { 1 });
}
