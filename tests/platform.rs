//! Runtime tests for the cross-platform process helpers. These run on every CI OS
//! (Linux, macOS, Windows); behavior that only exists on some platforms is gated
//! with `cfg`, never skipped at runtime.
use caelum_rs::platform;
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

/// Kills and reaps the child on drop so a failing assertion never leaks a process.
struct Reaper(Child);
impl Drop for Reaper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A long-running, side-effect-free command for this OS.
fn sleeper() -> Command {
    #[cfg(unix)]
    {
        let mut c = Command::new("sleep");
        c.arg("30");
        c
    }
    #[cfg(windows)]
    {
        let mut c = Command::new("ping");
        c.args(["-n", "30", "127.0.0.1"]);
        c
    }
}

/// Canonical form comparable across OSes (resolves /var -> /private/var on macOS and
/// drops the `\\?\` verbatim prefix that Windows canonicalization adds).
fn norm(p: &Path) -> PathBuf {
    let c = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    match c.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(stripped) => PathBuf::from(stripped),
        None => c,
    }
}

fn wait_until(timeout: Duration, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + timeout;
    loop {
        if f() {
            return true;
        }
        if Instant::now() >= end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn process_cwd_reports_child_working_directory() {
    let dir = tempfile::tempdir().unwrap();
    let want = norm(dir.path());
    let child = Reaper(
        sleeper()
            .current_dir(dir.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let pid = child.0.id();
    let mut got = None;
    let ok = wait_until(Duration::from_secs(10), || {
        got = platform::process_cwd(pid).map(|p| norm(&p));
        got.as_deref() == Some(want.as_path())
    });
    assert!(ok, "process_cwd({pid}) = {got:?}, want {want:?}");
}

#[test]
fn process_cwd_of_missing_process_is_none() {
    let mut child = sleeper()
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let pid = child.id();
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(platform::process_cwd(pid), None);
}

#[cfg(unix)]
fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks for existence.
    if unsafe { libc::kill(pid, 0) } != 0 {
        return false;
    }
    // A zombie that init has not reaped yet is already dead for our purposes.
    #[cfg(target_os = "linux")]
    if let Ok(s) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        return !s
            .rsplit(')')
            .next()
            .unwrap_or("")
            .trim_start()
            .starts_with('Z');
    }
    true
}

/// Opens a PTY and runs `script` under `sh -c`, as the app does for panes.
#[cfg(unix)]
fn spawn_in_pty(
    script: &str,
) -> (
    portable_pty::PtyPair,
    Box<dyn portable_pty::Child + Send + Sync>,
) {
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut cmd = CommandBuilder::new("sh");
    cmd.args(["-c", script]);
    let child = pair.slave.spawn_command(cmd).unwrap();
    (pair, child)
}

#[cfg(unix)]
#[test]
fn kill_process_tree_kills_child_and_grandchild() {
    // sh (child) backgrounds `sleep` (grandchild), prints its pid, and waits.
    let (pair, mut child) = spawn_in_pty("sleep 300 & echo $!; wait");
    let pid = child.process_id().expect("child pid") as i32;
    let reader = pair.master.try_clone_reader().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = std::io::BufRead::read_line(&mut std::io::BufReader::new(reader), &mut line);
        let _ = tx.send(line);
    });
    let line = rx
        .recv_timeout(Duration::from_secs(15))
        .expect("grandchild pid line");
    let grandchild: i32 = line.trim().parse().expect(&line);
    assert!(
        alive(pid) && alive(grandchild),
        "setup: both should be alive"
    );

    platform::kill_process_tree(pid as u32, &*pair.master);

    let _ = child.wait(); // reap the direct child
    assert!(
        wait_until(Duration::from_secs(10), || !alive(grandchild)),
        "grandchild {grandchild} survived kill_process_tree"
    );
    assert!(!alive(pid), "child {pid} survived kill_process_tree");
}

// Regression (macOS): kill_process_tree used to walk the parent-pid tree only *after*
// killing the shell's group, by which point the job had been reparented to launchd and
// survived. It now snapshots descendants before signalling.
#[cfg(unix)]
#[test]
fn kill_process_tree_kills_job_in_its_own_process_group() {
    // `set -m` gives the background job its own process group inside the pane's
    // session, like an interactive shell's `&` jobs.
    let (pair, mut child) = spawn_in_pty("set -m; sleep 300 & echo $!; wait");
    let pid = child.process_id().expect("child pid") as i32;
    let reader = pair.master.try_clone_reader().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = std::io::BufRead::read_line(&mut std::io::BufReader::new(reader), &mut line);
        let _ = tx.send(line);
    });
    let line = rx
        .recv_timeout(Duration::from_secs(15))
        .expect("job pid line");
    let job: i32 = line.trim().parse().expect(&line);
    assert!(alive(job));

    platform::kill_process_tree(pid as u32, &*pair.master);

    let _ = child.wait();
    assert!(
        wait_until(Duration::from_secs(10), || !alive(job)),
        "job {job} in its own process group survived kill_process_tree"
    );
}

/// On Windows the ConPTY job tears the tree down when the pseudoconsole closes, so
/// `kill_process_tree` is intentionally a no-op and `Child::kill` does the work.
/// This checks that contract: the call is harmless and the child can be killed + reaped.
#[cfg(windows)]
#[test]
fn kill_process_tree_is_harmless_and_child_kill_reaps() {
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut cmd = CommandBuilder::new("ping");
    cmd.args(["-n", "30", "127.0.0.1"]);
    let mut child = pair.slave.spawn_command(cmd).unwrap();
    let pid = child.process_id().expect("child pid");
    platform::kill_process_tree(pid, &*pair.master);
    child.kill().unwrap();
    assert!(
        wait_until(Duration::from_secs(10), || matches!(
            child.try_wait(),
            Ok(Some(_))
        )),
        "child {pid} not reaped after kill"
    );
}

#[test]
fn test_shell_command_runs_a_command() {
    let argv = platform::test_shell_command();
    assert!(!argv.is_empty());
    #[cfg(unix)]
    {
        // The command line must start an interactive shell that executes input and exits.
        use std::io::Write;
        let mut child = Command::new(&argv[0])
            .args(&argv[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"echo caelum-ok\nexit\n")
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(String::from_utf8_lossy(&out.stdout).contains("caelum-ok"));
    }
}
