#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

/// macOS: CEF lives in the bundled `Chromium Embedded Framework.framework`,
/// which must be loaded before any CEF entry point (including command-line
/// creation). Helper apps under `Contents/Frameworks/` resolve it via a
/// different relative path. CEF's Objective-C base layer also needs an
/// autorelease pool on the main thread, which a bare CLI process lacks.
#[cfg(target_os = "macos")]
fn main() -> Result<(), String> {
    objc2::rc::autoreleasepool(|_| -> Result<(), String> {
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        let is_helper = executable
            .to_string_lossy()
            .contains(".app/Contents/Frameworks/");
        let args = cef::args::Args::new();
        // Helper processes must initialize the CEF sandbox before the CEF
        // framework is loaded. The browser process is not sandboxed by CEF;
        // macOS covers it via the app bundle's own entitlements.
        #[cfg(feature = "sandbox")]
        let _sandbox = if is_helper {
            let mut sandbox = cef::sandbox::Sandbox::new();
            sandbox.initialize(args.as_main_args());
            Some(sandbox)
        } else {
            None
        };
        let loader = cef::library_loader::LibraryLoader::new(&executable, is_helper);
        if !loader.load() {
            return Err("cef_framework_load_failed".into());
        }
        // Initialize the CEF API version before any other CEF call.
        let _ = cef::api_hash(cef::sys::CEF_API_VERSION_LAST, 0);
        let Some(command_line) = args.as_cmd_line() else {
            return Err("cef_command_line_invalid".into());
        };
        hachimi_cef_host::run_cef_host(args.as_main_args(), &command_line, std::ptr::null_mut())
    })
}

#[cfg(all(
    not(target_os = "macos"),
    not(all(feature = "sandbox", target_os = "windows"))
))]
fn main() -> Result<(), String> {
    let args = cef::args::Args::new();
    let Some(command_line) = args.as_cmd_line() else {
        return Err("cef_command_line_invalid".into());
    };
    hachimi_cef_host::run_cef_host(args.as_main_args(), &command_line, std::ptr::null_mut())
}

#[cfg(all(feature = "sandbox", target_os = "windows"))]
fn main() -> Result<(), String> {
    Err("hachimi-cef-host must be launched through the CEF sandbox bootstrap".into())
}
