//! Waiting for another process to exit: the window a restart replaces,
//! which holds the store until it closes.

use std::time::{Duration, Instant};

/// Wait until process `pid` has exited or `timeout` has passed. Answers
/// whether it exited (a process that was never there has).
pub(crate) fn wait_for_exit(pid: u32, timeout: Duration) -> bool {
    wait(pid, Instant::now() + timeout)
}

/// Windows: start `command` without a console window. The window is a GUI
/// program with no console of its own, so a console program it starts
/// (`tar.exe`) would otherwise open one of its own, a black window flashing
/// up over Postrinch's. Elsewhere this does nothing.
pub(crate) fn no_console(command: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = command;
}

/// Windows: the process's own handle, signalled when it exits. A pid that
/// cannot be opened names no running process this user may wait on.
#[cfg(windows)]
fn wait(pid: u32, deadline: Instant) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    // SAFETY: plain Win32 calls; the handle is closed before returning.
    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            return true;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        let millis = u32::try_from(left.as_millis()).unwrap_or(u32::MAX - 1);
        let waited = WaitForSingleObject(handle, millis);
        CloseHandle(handle);
        waited == WAIT_OBJECT_0
    }
}

/// Windows: the processes other than this one running a program from
/// `dir`, which an uninstall must not pull the files from under.
#[cfg(windows)]
pub(crate) fn running_from(dir: &std::path::Path) -> Vec<u32> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };
    let Some(dir) = dir.canonicalize().ok() else {
        return Vec::new();
    };
    let own = std::process::id();
    let mut found = Vec::new();
    // SAFETY: plain Win32 calls on structures sized as they ask; every
    // handle is closed before returning.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return found;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut more = Process32FirstW(snapshot, &mut entry) != 0;
        while more {
            let pid = entry.th32ProcessID;
            if pid != own {
                let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
                if !process.is_null() {
                    let mut name = [0u16; 1024];
                    let mut len = name.len() as u32;
                    let named = QueryFullProcessImageNameW(
                        process,
                        PROCESS_NAME_WIN32,
                        name.as_mut_ptr(),
                        &mut len,
                    ) != 0;
                    CloseHandle(process);
                    let image =
                        std::path::PathBuf::from(String::from_utf16_lossy(&name[..len as usize]));
                    let in_dir = image
                        .parent()
                        .and_then(|p| p.canonicalize().ok())
                        .is_some_and(|p| p == dir);
                    if named && in_dir {
                        found.push(pid);
                    }
                }
            }
            more = Process32NextW(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
    }
    found
}

/// Elsewhere, look every tenth of a second.
#[cfg(not(windows))]
fn wait(pid: u32, deadline: Instant) -> bool {
    loop {
        if !alive(pid) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(target_os = "linux")]
fn alive(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

#[cfg(all(unix, not(target_os = "linux")))]
fn alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::process::{Child, Command, Stdio};

    /// Set in the environment of the children [`sleeper`] sleeps in.
    pub(crate) const SLEEP_ENV: &str = "POSTRINCH_UPDATE_TEST_SLEEPER";

    /// `program` (a copy of this test binary, or itself) running only
    /// [`sleeper`]: a child process that stays up, on every system, with
    /// nothing else installed. Linux refuses to run a file another thread's
    /// child still holds open for writing for a moment after a copy
    /// ("text file busy", as tests start children in parallel), so that is
    /// tried again.
    pub(crate) fn sleeping(program: &std::path::Path) -> Child {
        let mut tries = 0;
        loop {
            let started = Command::new(program)
                .args(["--ignored", "--exact", "process::tests::sleeper"])
                .env(SLEEP_ENV, "1")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            match started {
                // ETXTBSY
                Err(e) if e.raw_os_error() == Some(26) && cfg!(unix) && tries < 50 => {
                    tries += 1;
                    std::thread::sleep(Duration::from_millis(20));
                }
                started => return started.unwrap(),
            }
        }
    }

    fn sleeping_child() -> Child {
        sleeping(&std::env::current_exe().unwrap())
    }

    /// Sleeps only as the child the other tests start, so `cargo test --
    /// --ignored` passes over it.
    #[test]
    #[ignore = "the child the other tests start"]
    fn sleeper() {
        if std::env::var_os(SLEEP_ENV).is_some() {
            std::thread::sleep(Duration::from_secs(60));
        }
    }

    /// What `--uninstall` looks for: a window (or bridge) still running
    /// from the install it is about to empty.
    #[cfg(windows)]
    #[test]
    fn a_program_running_from_a_folder_is_found() {
        let dir = tempfile::tempdir().unwrap();
        let copy = dir.path().join("running.exe");
        std::fs::copy(std::env::current_exe().unwrap(), &copy).unwrap();
        assert!(running_from(dir.path()).is_empty());
        let mut child = sleeping(&copy);
        let found = running_from(dir.path());
        child.kill().unwrap();
        child.wait().unwrap();
        let pid = child.id();
        drop(child);
        assert_eq!(found, [pid]);
        assert!(running_from(dir.path()).is_empty());
    }

    #[test]
    fn a_running_process_is_waited_for_until_the_timeout() {
        let mut child = sleeping_child();
        let started = Instant::now();
        assert!(!wait_for_exit(child.id(), Duration::from_millis(300)));
        // Windows' wait may end a hair before `Instant` says 300 ms passed.
        assert!(started.elapsed() >= Duration::from_millis(250));
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn an_exit_ends_the_wait() {
        let mut child = sleeping_child();
        let pid = child.id();
        let killer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            child.kill().unwrap();
            // Reaped, so on Unix the pid is gone rather than a zombie.
            child.wait().unwrap();
        });
        let started = Instant::now();
        assert!(wait_for_exit(pid, Duration::from_secs(30)));
        assert!(started.elapsed() < Duration::from_secs(20));
        killer.join().unwrap();
    }

    #[test]
    fn no_such_process_has_exited() {
        let mut child = sleeping_child();
        let pid = child.id();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(wait_for_exit(pid, Duration::from_secs(5)));
    }
}
