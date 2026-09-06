// P3-M2 capture probe: verifies the ScreenCaptureKit path on a real desktop.
// First run may trigger the Screen Recording TCC prompt; approve, then re-run
// to capture the frontmost window into /tmp/hachimi-capture-probe.png.
use hachimi_computer::{ComputerBroker, PlatformComputerBroker, computer_runtime_health};

fn main() {
    let health = computer_runtime_health();
    println!(
        "health: os_supported={} capture={} input={} elevated={} error={:?}",
        health.os_supported,
        health.graphics_capture_available,
        health.input_desktop_available,
        health.process_elevated,
        health.error_code
    );
    // Trigger the system prompt so the approval lands on this binary.
    if !health.graphics_capture_available {
        hachimi_computer::request_screen_capture_access();
        eprintln!(
            "Screen Recording permission requested; approve Hachimi/the terminal in System Settings, then re-run this probe."
        );
        std::process::exit(2);
    }
    let broker = PlatformComputerBroker::new();
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let result: Result<(), String> = runtime.block_on(async {
        let windows = broker
            .list_windows()
            .await
            .map_err(|error| error.to_string())?;
        println!("windows: {}", windows.len());
        let foreground = broker
            .foreground_window()
            .await
            .map_err(|error| error.to_string())?;
        println!(
            "foreground: {} ({}) pid={}",
            foreground.app.display_name, foreground.app_id, foreground.process_id
        );
        let captured = broker
            .capture(&foreground.window_handle)
            .await
            .map_err(|error| error.to_string())?;
        println!(
            "captured: {}x{} token={}",
            captured.width, captured.height, captured.image_token
        );
        let bytes = broker
            .read_frame(&captured.image_token)
            .await
            .map_err(|error| error.to_string())?;
        if !bytes.starts_with(b"\x89PNG") {
            return Err("captured frame is not a PNG".into());
        }
        std::fs::write("/tmp/hachimi-capture-probe.png", &bytes).map_err(|e| e.to_string())?;
        println!(
            "wrote /tmp/hachimi-capture-probe.png ({} bytes)",
            bytes.len()
        );
        broker.release_frame(&captured.image_token);
        Ok(())
    });
    if let Err(error) = result {
        eprintln!("probe failed: {error}");
        std::process::exit(1);
    }
}
