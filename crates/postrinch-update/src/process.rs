//! Waiting for another process to exit: the window a restart replaces,
//! which holds the store until it closes.

use std::time::{Duration, Instant};

/// Wait until process `pid` has exited or `timeout` has passed. Answers
/// whether it exited (a process that was never there has).
pub(crate) fn wait_for_exit(pid: u32, timeout: Duration) -> bool {
    wait(pid, Instant::now() + timeout)
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
mod tests {
    use super::*;
    use std::process::{Child, Command, Stdio};

    /// This test binary again, running only [`sleeper`]: a child process
    /// that stays up, on every system, with nothing else installed.
    fn sleeping_child() -> Child {
        Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "process::tests::sleeper"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    }

    #[test]
    #[ignore = "the child the other tests start"]
    fn sleeper() {
        std::thread::sleep(Duration::from_secs(60));
    }

    #[test]
    fn a_running_process_is_waited_for_until_the_timeout() {
        let mut child = sleeping_child();
        let started = Instant::now();
        assert!(!wait_for_exit(child.id(), Duration::from_millis(300)));
        assert!(started.elapsed() >= Duration::from_millis(300));
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
