// P4-A composite probe (macOS): proves the OSR desktop-composite model
// end-to-end and self-terminates within ~15s (hard watchdog at 20s). Verifies:
//   Z-ORDER  — the overlay is the topmost content subview (above WKWebView)
//              and renders the live CEF frame pixels at Retina resolution
//   INPUT    — a real mouse click routes overlay → IPC → page DOM counter
// Run: cargo run -p hachimi-desktop --example cef_osr_probe

#[cfg(target_os = "macos")]
#[path = "cef_osr_probe/macos.rs"]
mod macos;

#[cfg(target_os = "macos")]
fn main() {
    macos::run();
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("cef_osr_probe requires macOS AppKit and the CEF OSR host");
    std::process::exit(1);
}
