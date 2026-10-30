//! OS-specific behavior lives here so the rest of the crate stays portable.
use std::path::{Path, PathBuf};

/// The user's home directory (`HOME`/`USERPROFILE`, then the OS account database, then `/`).
pub fn home() -> PathBuf {
    resolve_home(
        std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")),
        || dirs::home_dir().or_else(account_home),
    )
}

/// Pure helper: use `env` unless unset/empty, then `fallback`, then the filesystem root.
pub fn resolve_home(
    env: Option<std::ffi::OsString>,
    fallback: impl FnOnce() -> Option<PathBuf>,
) -> PathBuf {
    env.filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .or_else(|| fallback().filter(|p| !p.as_os_str().is_empty()))
        .unwrap_or_else(|| PathBuf::from(std::path::MAIN_SEPARATOR_STR))
}

#[cfg(unix)]
fn account_home() -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    // SAFETY: getpwuid_r writes only into the buffers we provide; pw_dir is read
    // while buf is alive and copied before returning.
    unsafe {
        let mut pwd: libc::passwd = std::mem::zeroed();
        let mut buf = vec![0u8; 4096];
        loop {
            let mut result: *mut libc::passwd = std::ptr::null_mut();
            let rc = libc::getpwuid_r(
                libc::getuid(),
                &mut pwd,
                buf.as_mut_ptr().cast(),
                buf.len(),
                &mut result,
            );
            if rc == libc::ERANGE && buf.len() < 1 << 20 {
                let n = buf.len() * 2;
                buf.resize(n, 0);
                continue;
            }
            if rc != 0 || result.is_null() || pwd.pw_dir.is_null() {
                return None;
            }
            let dir = std::ffi::CStr::from_ptr(pwd.pw_dir).to_bytes().to_vec();
            return Some(PathBuf::from(std::ffi::OsString::from_vec(dir)));
        }
    }
}
#[cfg(not(unix))]
fn account_home() -> Option<PathBuf> {
    None
}

/// Default per-user profile directory.
/// Linux keeps `$XDG_DATA_HOME/caelum-rs` (or `~/.local/share`); macOS uses
/// `~/Library/Application Support`; Windows uses `%APPDATA%`.
pub fn default_profile_dir() -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        std::env::var_os("XDG_DATA_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".local/share"))
            .join("caelum-rs")
    }
    #[cfg(not(target_os = "linux"))]
    {
        dirs::data_dir()
            .unwrap_or_else(|| home().join(".caelum-data"))
            .join("caelum-rs")
    }
}

/// Fallback shell when `$SHELL` is unusable.
#[cfg(target_os = "macos")]
pub const FALLBACK_SHELL: &str = "/bin/zsh";
#[cfg(all(unix, not(target_os = "macos")))]
pub const FALLBACK_SHELL: &str = "/bin/bash";
#[cfg(windows)]
pub const FALLBACK_SHELL: &str = "powershell.exe";
#[cfg(not(any(unix, windows)))]
compile_error!("caelum-rs supports only unix-like and Windows targets");

/// Pick the shell to launch. On unix `candidate` (normally `$SHELL`) must be an
/// absolute path to an executable file. On Windows prefer PowerShell 7, then
/// Windows PowerShell, then `%COMSPEC%`.
pub fn resolve_shell(candidate: Option<&str>) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match candidate {
            Some(c)
                if !c.is_empty()
                    && Path::new(c).is_absolute()
                    && std::fs::metadata(c)
                        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                        .unwrap_or(false) =>
            {
                c.to_string()
            }
            _ => FALLBACK_SHELL.to_string(),
        }
    }
    #[cfg(windows)]
    {
        // Honour an explicit candidate (absolute path, or a bare name found on PATH).
        if let Some(c) = candidate.filter(|c| !c.is_empty()) {
            let p = Path::new(c);
            let exists = if p.is_absolute() || p.components().count() > 1 {
                p.is_file()
            } else {
                std::env::var_os("PATH")
                    .map(|v| std::env::split_paths(&v).any(|d| d.join(c).is_file()))
                    .unwrap_or(false)
            };
            if exists {
                return c.to_string();
            }
        }
        let on_path = |exe: &str| {
            std::env::var_os("PATH")
                .map(|p| std::env::split_paths(&p).any(|d| d.join(exe).is_file()))
                .unwrap_or(false)
        };
        if on_path("pwsh.exe") {
            "pwsh.exe".to_string()
        } else if on_path("powershell.exe") {
            FALLBACK_SHELL.to_string()
        } else {
            std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())
        }
    }
}

/// Arguments that make `shell` a login shell (unix shells only).
pub fn shell_args(shell: &str) -> &'static [&'static str] {
    let _ = shell;
    #[cfg(unix)]
    {
        &["-l"]
    }
    #[cfg(windows)]
    {
        &[]
    }
}

/// A deterministic interactive shell command for automated validation.
pub fn test_shell_command() -> Vec<String> {
    #[cfg(unix)]
    {
        // Prefer bash (absent from /bin on some systems); fall back to the platform shell.
        let shell = ["/bin/bash", "/usr/bin/bash", "/usr/local/bin/bash"]
            .into_iter()
            .find(|p| Path::new(p).is_file())
            .unwrap_or(FALLBACK_SHELL);
        let mut argv: Vec<String> = ["/usr/bin/env", "HISTFILE=/dev/null", "PS1=$ ", shell]
            .iter()
            .map(|s| s.to_string())
            .collect();
        if shell.ends_with("bash") {
            argv.extend(["--noprofile", "--norc"].map(String::from));
        } else {
            argv.push("-f".into()); // zsh: skip startup files
        }
        argv.push("-i".into());
        argv
    }
    #[cfg(windows)]
    {
        vec!["cmd.exe".to_string(), "/Q".to_string()]
    }
}

/// Current working directory of process `pid`, where the OS exposes it cheaply.
pub fn process_cwd(pid: u32) -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
    }
    #[cfg(not(target_os = "linux"))]
    {
        use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
        let pid = Pid::from_u32(pid);
        let mut sys = System::new();
        sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing().with_cwd(UpdateKind::Always),
        );
        sys.process(pid)?.cwd().map(Path::to_path_buf)
    }
}

/// An exclusive, process-held lock; released when dropped (or the process exits).
pub struct InstanceLock {
    _file: std::fs::File,
}

/// Takes the profile lock; `Ok(None)` means another instance already holds it.
pub fn lock_instance(path: &Path) -> std::io::Result<Option<InstanceLock>> {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(path)?;
        let _ = f.set_permissions(std::fs::Permissions::from_mode(0o600));
        // SAFETY: the descriptor stays owned by `f` for the lifetime of the lock.
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Ok(None);
        }
        Ok(Some(InstanceLock { _file: f }))
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // share_mode(0) denies all sharing, so a second open fails while we hold it.
        match std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .share_mode(0)
            .open(path)
        {
            Ok(f) => Ok(Some(InstanceLock { _file: f })),
            // ERROR_SHARING_VIOLATION
            Err(e) if e.raw_os_error() == Some(32) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// Snapshot of host load/memory/uptime/processes for the system panel.
#[derive(Default)]
pub struct SystemSnapshot {
    pub load: String,
    pub memory: String,
    pub uptime: String,
    /// (pid, name, resident memory)
    pub processes: Vec<(String, String, String)>,
}

#[cfg(target_os = "linux")]
pub fn system_snapshot() -> SystemSnapshot {
    use std::fs;
    let load = fs::read_to_string("/proc/loadavg")
        .unwrap_or_default()
        .split_whitespace()
        .take(3)
        .collect::<Vec<_>>()
        .join("  ");
    let mem = fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let value = |key: &str| {
        mem.lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0)
    };
    let total = value("MemTotal:");
    let used = total.saturating_sub(value("MemAvailable:"));
    let memory = format!(
        "{:.1} / {:.1} GiB",
        used as f64 / 1048576.,
        total as f64 / 1048576.
    );
    let seconds = fs::read_to_string("/proc/uptime")
        .unwrap_or_default()
        .split_whitespace()
        .next()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.) as u64;
    let mut processes = Vec::new();
    if let Ok(entries) = fs::read_dir("/proc") {
        for e in entries.flatten() {
            let pid = e.file_name().to_string_lossy().to_string();
            if pid.parse::<u32>().is_err() {
                continue;
            }
            let status = fs::read_to_string(e.path().join("status")).unwrap_or_default();
            let name = status
                .lines()
                .find_map(|l| l.strip_prefix("Name:\t"))
                .unwrap_or("")
                .to_string();
            let rss = status
                .lines()
                .find_map(|l| l.strip_prefix("VmRSS:\t"))
                .unwrap_or("0 kB")
                .trim()
                .to_string();
            if !name.is_empty() {
                processes.push((pid, name, rss));
            }
        }
    }
    processes.sort_by_key(|p| p.0.parse::<u32>().unwrap_or(0));
    SystemSnapshot {
        load,
        memory,
        uptime: format!("{}h {}m", seconds / 3600, seconds / 60 % 60),
        processes,
    }
}

#[cfg(not(target_os = "linux"))]
pub fn system_snapshot() -> SystemSnapshot {
    use sysinfo::System;
    let mut sys = System::new();
    sys.refresh_memory();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    let la = System::load_average();
    let seconds = System::uptime();
    let gib = |b: u64| b as f64 / 1073741824.;
    let mut processes: Vec<_> = sys
        .processes()
        .iter()
        .map(|(pid, p)| {
            (
                pid.as_u32().to_string(),
                p.name().to_string_lossy().into_owned(),
                format!("{} kB", p.memory() / 1024),
            )
        })
        .collect();
    processes.sort_by_key(|p| p.0.parse::<u32>().unwrap_or(0));
    SystemSnapshot {
        // Windows has no load average; sysinfo reports zeros there.
        load: format!("{:.2}  {:.2}  {:.2}", la.one, la.five, la.fifteen),
        memory: format!(
            "{:.1} / {:.1} GiB",
            gib(sys.used_memory()),
            gib(sys.total_memory())
        ),
        uptime: format!("{}h {}m", seconds / 3600, seconds / 60 % 60),
        processes,
    }
}

/// Force-kills a still-running child's whole process group (unix). On Windows the
/// ConPTY job closes with the pseudoconsole, so `Child::kill` is sufficient.
pub fn kill_process_tree(pid: u32, master: &dyn portable_pty::MasterPty) {
    // No /proc session scan on macOS/BSD. Snapshot the targets (and each one's process
    // group) BEFORE sending any signal: once the shell dies, its jobs are reparented to
    // launchd/init and no longer show up as descendants, so a parent-pid walk done after
    // the first kill misses a job in its own process group (the macOS CI failure "job N
    // in its own process group survived"). The shell may already be dead when we get
    // here (Session::drop signals its group first), so also take every process that is
    // still in the pane's session: the session id survives reparenting, and the unreaped
    // leader keeps `pid` from being reused.
    #[cfg(all(unix, not(target_os = "linux")))]
    let snapshot: Vec<(i32, i32)> = {
        let pairs = process_parents();
        let mut targets = descendants(&pairs, pid);
        for &(p, _) in &pairs {
            // SAFETY: getsid has no memory-safety preconditions.
            if p != pid && !targets.contains(&p) && unsafe { libc::getsid(p as i32) } == pid as i32
            {
                targets.push(p);
            }
        }
        targets
    }
    .into_iter()
    .map(|member| {
        let member = member as i32;
        // SAFETY: getsid/getpgid have no memory-safety preconditions.
        let group = unsafe {
            if libc::getsid(member) == pid as i32 {
                libc::getpgid(member)
            } else {
                -1
            }
        };
        (member, group)
    })
    .collect();
    #[cfg(unix)]
    // SAFETY: portable-pty creates a new session led by this still-owned, unreaped child.
    unsafe {
        if let Some(fg) = master.process_group_leader()
            && fg > 0
            && libc::getsid(fg) == pid as i32
        {
            libc::kill(-fg, libc::SIGKILL);
        }
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
    // Background jobs run in their own process groups inside the same session; find
    // them by session id so they don't outlive the closed pane.
    #[cfg(target_os = "linux")]
    for member in session_members(pid as i32) {
        // SAFETY: kill has no memory-safety preconditions; we only target our session.
        unsafe {
            libc::kill(member, libc::SIGKILL);
        }
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    for &(member, group) in &snapshot {
        // SAFETY: kill has no memory-safety preconditions; we only target descendants
        // snapshotted above and process groups inside the pane's own session.
        unsafe {
            if group > 1 {
                libc::kill(-group, libc::SIGKILL);
            }
            libc::kill(member, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (pid, master);
    }
}

/// (pid, parent pid) for every visible process.
#[cfg(all(unix, not(target_os = "linux")))]
fn process_parents() -> Vec<(u32, u32)> {
    use sysinfo::{ProcessesToUpdate, System};
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    sys.processes()
        .iter()
        .filter_map(|(pid, p)| Some((pid.as_u32(), p.parent()?.as_u32())))
        .collect()
}

/// All transitive children of `root` (excluding `root`) given (pid, parent) pairs.
#[cfg(any(test, all(unix, not(target_os = "linux"))))]
fn descendants(pairs: &[(u32, u32)], root: u32) -> Vec<u32> {
    let mut found = vec![root];
    let mut i = 0;
    while i < found.len() {
        let parent = found[i];
        for &(pid, ppid) in pairs {
            if ppid == parent && pid != parent && !found.contains(&pid) {
                found.push(pid);
            }
        }
        i += 1;
    }
    found.remove(0);
    found
}

/// Parses the session id (field 6) from a `/proc/<pid>/stat` line. The command
/// name is parenthesized and may itself contain spaces or `)`.
#[cfg(target_os = "linux")]
fn stat_session(stat: &str) -> Option<i32> {
    let rest = &stat[stat.rfind(')')? + 1..];
    // After ')': state ppid pgrp session ...
    rest.split_whitespace().nth(3)?.parse().ok()
}

/// Lists live pids whose session id is `sid` (excluding `sid` itself).
#[cfg(target_os = "linux")]
fn session_members(sid: i32) -> Vec<i32> {
    if sid <= 1 {
        return vec![];
    }
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return vec![];
    };
    dir.flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok())
        .filter(|&p| p != sid)
        .filter(|p| {
            std::fs::read_to_string(format!("/proc/{p}/stat"))
                .ok()
                .and_then(|s| stat_session(&s))
                == Some(sid)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn home_prefers_env_then_fallback_then_root() {
        let fb = || Some(PathBuf::from("/home/pw"));
        assert_eq!(resolve_home(Some("/x".into()), fb), PathBuf::from("/x"));
        assert_eq!(resolve_home(Some("".into()), fb), PathBuf::from("/home/pw"));
        assert_eq!(
            resolve_home(None, || None),
            PathBuf::from(std::path::MAIN_SEPARATOR_STR)
        );
    }
    #[test]
    fn shell_resolution_always_yields_something() {
        assert!(!resolve_shell(None).is_empty());
        assert!(!resolve_shell(Some("relative/sh")).is_empty());
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn parses_session_from_proc_stat() {
        assert_eq!(
            stat_session("1234 (bash) S 1 1234 1234 34816 1234 4194560"),
            Some(1234)
        );
        assert_eq!(stat_session("77 (we ird) name)) R 5 60 99 0 -1"), Some(99));
        assert_eq!(stat_session("garbage"), None);
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn background_job_in_session_is_killed() {
        use std::os::unix::process::CommandExt;
        use std::process::Command;
        // A new session whose leader backgrounds a job (found via its session id).
        let mut leader = Command::new("sh");
        leader
            .args(["-c", "sleep 300 & echo $!; wait"])
            .stdout(std::process::Stdio::piped());
        // SAFETY: setsid is async-signal-safe and runs in the forked child only.
        unsafe {
            leader.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        let mut child = leader.spawn().unwrap();
        let mut line = String::new();
        std::io::BufRead::read_line(
            &mut std::io::BufReader::new(child.stdout.take().unwrap()),
            &mut line,
        )
        .unwrap();
        let job: i32 = line.trim().parse().unwrap();
        let sid = child.id() as i32;
        assert!(session_members(sid).contains(&job));
        assert!(!session_members(sid).contains(&sid));
        // Killing session members alone (not the leader's group) reaches the job.
        for p in session_members(sid) {
            // SAFETY: test-owned processes.
            unsafe { libc::kill(p, libc::SIGKILL) };
        }
        let gone = (0..100).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(20));
            std::fs::read_to_string(format!("/proc/{job}/stat"))
                .map(|s| s.contains(") Z "))
                .unwrap_or(true)
        });
        // SAFETY: test-owned process group.
        unsafe { libc::kill(-sid, libc::SIGKILL) };
        let _ = child.wait();
        assert!(gone, "background job {job} survived");
    }
    #[test]
    fn descendants_walks_tree_and_survives_cycles() {
        let pairs = [(2, 1), (3, 2), (4, 2), (5, 9), (1, 3)];
        let mut d = descendants(&pairs, 2);
        d.sort();
        assert_eq!(d, vec![1, 3, 4]);
        assert!(descendants(&pairs, 5).is_empty());
    }
    #[cfg(unix)]
    #[test]
    fn test_shell_command_uses_existing_shell() {
        let argv = test_shell_command();
        assert!(Path::new(&argv[3]).is_file(), "{argv:?}");
    }
    #[test]
    fn process_cwd_of_self_matches_current_dir() {
        let got = process_cwd(std::process::id()).map(|p| p.canonicalize().unwrap());
        assert_eq!(
            got,
            Some(std::env::current_dir().unwrap().canonicalize().unwrap())
        );
    }
    #[test]
    fn instance_lock_is_exclusive() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("app.lock");
        let first = lock_instance(&p).unwrap();
        assert!(first.is_some());
        assert!(lock_instance(&p).unwrap().is_none());
        drop(first);
        // flock belongs to the open file description. A sibling test thread that is
        // between fork() and exec() (std::process / portable-pty) briefly holds a copy
        // of our descriptor, so the release can lag by that window. Retry for a bound.
        let relocked = (0..200).any(|_| {
            lock_instance(&p).unwrap().is_some() || {
                std::thread::sleep(std::time::Duration::from_millis(10));
                false
            }
        });
        assert!(relocked, "lock not released within 2s of drop");
    }
    /// The real guarantee behind the flake above: exec'd children (shells, panes) must
    /// never inherit the lock, or a surviving shell would block relaunching the app.
    #[cfg(unix)]
    #[test]
    fn instance_lock_fd_is_close_on_exec() {
        use std::os::fd::AsRawFd;
        let d = tempfile::tempdir().unwrap();
        let lock = lock_instance(&d.path().join("app.lock")).unwrap().unwrap();
        // SAFETY: F_GETFD only reads flags of a descriptor we own.
        let flags = unsafe { libc::fcntl(lock._file.as_raw_fd(), libc::F_GETFD) };
        assert!(flags >= 0 && flags & libc::FD_CLOEXEC != 0, "flags={flags}");
    }
}
