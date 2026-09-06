use std::{env, net::TcpStream, path::PathBuf, time::Duration};

fn main() {
    let arguments = env::args_os().skip(1).collect::<Vec<_>>();
    let mut failure_exit_code = 1;
    let result = match arguments.as_slice() {
        [operation] if operation == "--ok" => Ok(()),
        [operation] if operation == "--assert-job" => assert_job(),
        [operation] if operation == "--assert-seatbelt" => assert_seatbelt(),
        [operation] if operation == "--read-fd3" => read_fd3(),
        [operation, path] if operation == "--touch" => {
            std::fs::write(PathBuf::from(path), b"sandbox canary").map_err(|error| {
                failure_exit_code = error
                    .raw_os_error()
                    .and_then(|code| u8::try_from(code).ok())
                    .map(i32::from)
                    .unwrap_or(1);
                error.to_string()
            })
        }
        [operation, address] if operation == "--read" => std::fs::read(PathBuf::from(address))
            .map(|_| ())
            .map_err(|error| error.to_string()),
        [operation, address] if operation == "--network" => address
            .to_string_lossy()
            .parse()
            .map_err(|_| "invalid canary network address".to_owned())
            .and_then(|address| {
                TcpStream::connect_timeout(&address, Duration::from_secs(2))
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            }),
        [operation, executable] if operation == "--spawn-child-assert-job" => {
            hachimi_process_policy::std_command(
                executable,
                hachimi_process_policy::ProcessPolicy::HiddenCaptured,
            )
            .arg("--assert-job")
            .status()
            .map_err(|error| error.to_string())
            .and_then(|status| {
                status
                    .success()
                    .then_some(())
                    .ok_or_else(|| format!("child job canary exited with {status}"))
            })
        }
        [operation, path] if operation == "--sleep-touch" => {
            std::thread::sleep(Duration::from_secs(3));
            std::fs::write(PathBuf::from(path), b"escaped process tree")
                .map_err(|error| error.to_string())
        }
        [operation, executable, path] if operation == "--spawn-child-sleep-touch" => {
            hachimi_process_policy::std_command(
                executable,
                hachimi_process_policy::ProcessPolicy::HiddenCaptured,
            )
            .arg("--sleep-touch")
            .arg(path)
            .status()
            .map_err(|error| error.to_string())
            .and_then(|status| {
                status
                    .success()
                    .then_some(())
                    .ok_or_else(|| format!("sleep child exited with {status}"))
            })
        }
        [operation, raw_handle] if operation == "--write-handle" => raw_handle
            .to_string_lossy()
            .parse::<usize>()
            .map_err(|_| "invalid inherited handle value".to_owned())
            .and_then(write_inherited_handle),
        _ => Err("invalid sandbox canary arguments".into()),
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(failure_exit_code);
    }
}

#[cfg(windows)]
fn assert_job() -> Result<(), String> {
    use windows_sys::Win32::System::{JobObjects::IsProcessInJob, Threading::GetCurrentProcess};

    let mut assigned = 0;
    if unsafe { IsProcessInJob(GetCurrentProcess(), std::ptr::null_mut(), &mut assigned) } == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if assigned == 0 {
        Err("canary process is not assigned to a Job Object".into())
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn write_inherited_handle(raw_handle: usize) -> Result<(), String> {
    use windows_sys::Win32::{Foundation::HANDLE, Storage::FileSystem::WriteFile};

    let bytes = b"sandbox handle escape";
    let mut written = 0_u32;
    let result = unsafe {
        WriteFile(
            raw_handle as HANDLE,
            bytes.as_ptr(),
            u32::try_from(bytes.len()).unwrap_or(u32::MAX),
            &mut written,
            std::ptr::null_mut(),
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error().to_string())
    } else if written != u32::try_from(bytes.len()).unwrap_or(u32::MAX) {
        Err("inherited handle write was incomplete".into())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn write_inherited_handle(_raw_handle: usize) -> Result<(), String> {
    Err("inherited handle canary is Windows-only".into())
}

#[cfg(not(windows))]
fn assert_job() -> Result<(), String> {
    Err("Job Object canary is Windows-only".into())
}

/// Proves the Seatbelt profile is active: writing to a scratch path outside
/// every grant must fail. If the write succeeds the process is unsandboxed.
#[cfg(target_os = "macos")]
fn assert_seatbelt() -> Result<(), String> {
    let probe = std::env::temp_dir().join(format!("hachimi-seatbelt-probe-{}", std::process::id()));
    match std::fs::write(&probe, b"must be denied") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            Err("canary write succeeded outside grants; Seatbelt is not active".into())
        }
        // Only a permission denial proves the sandbox; other I/O errors are
        // inconclusive and must fail the probe.
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => Ok(()),
        Err(error) => Err(format!("seatbelt probe was inconclusive: {error}")),
    }
}

/// Proves no unexpected file descriptors leak into the sandboxed child: only
/// stdio (0/1/2) may be open at entry.
#[cfg(target_os = "macos")]
fn read_fd3() -> Result<(), String> {
    match std::fs::read("/dev/fd/3") {
        Ok(_) => Err("fd 3 was readable; an unexpected descriptor was inherited".into()),
        Err(_) => Ok(()),
    }
}

#[cfg(not(target_os = "macos"))]
fn assert_seatbelt() -> Result<(), String> {
    Err("Seatbelt canary is macOS-only".into())
}

#[cfg(not(target_os = "macos"))]
fn read_fd3() -> Result<(), String> {
    Err("fd inheritance canary is macOS-only".into())
}
