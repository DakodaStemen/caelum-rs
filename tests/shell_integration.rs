//! The shell snippets must percent-encode the OSC 7 cwd so a hostile directory name
//! cannot terminate the sequence early and inject OSC 133 marks of its own.
use caelum_rs::osc::{OscEvent, OscScanner};
use std::{path::Path, process::Command};

/// BEL ends the OSC 7 early; `ESC ] 133;D;99 BEL` would then be a forged CommandEnd.
const HOSTILE: &str = "evil\x07\x1b]133;D;99\x07 tail%41\u{e9}";

fn hostile_dir() -> (tempfile::TempDir, std::path::PathBuf) {
    let base = tempfile::tempdir().unwrap();
    let dir = base.path().join(HOSTILE);
    std::fs::create_dir(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    (base, dir)
}

fn find(shell: &str) -> Option<std::path::PathBuf> {
    if let Some(p) = std::env::var_os(format!("CAELUM_TEST_{}", shell.to_uppercase())) {
        return Some(p.into());
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(shell))
        .find(|p| p.is_file())
}

fn snippet(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("shell")
        .join(name)
        .display()
        .to_string()
}

fn assert_single_cwd(output: &[u8], host: String, dir: &Path) {
    assert_eq!(
        output.iter().filter(|&&b| b == 0x07).count(),
        1,
        "raw BEL leaked into the OSC 7 payload: {output:?}"
    );
    let events: Vec<OscEvent> = OscScanner::new(host)
        .scan(output)
        .into_iter()
        .map(|(_, e)| e)
        .collect();
    assert_eq!(events, vec![OscEvent::Cwd(dir.to_path_buf())], "{output:?}");
}

fn run(cmd: &mut Command) -> Vec<u8> {
    let out = cmd
        .env("CAELUM_RS", "1")
        .env("LANG", "C.UTF-8")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

#[test]
fn bash_osc7_encodes_hostile_directory_names() {
    let Some(bash) = find("bash") else {
        eprintln!("bash not found; skipping");
        return;
    };
    let (_base, dir) = hostile_dir();
    let out = run(Command::new(bash)
        .arg("--norc")
        .arg("-c")
        .arg(format!(
            "source '{}'; HOSTNAME=box; cd -- \"$1\" && __caelum_osc7",
            snippet("caelum.bash")
        ))
        .arg("_")
        .arg(&dir));
    assert_single_cwd(&out, "box".into(), &dir);
}

#[test]
fn zsh_osc7_encodes_hostile_directory_names() {
    let Some(zsh) = find("zsh") else {
        eprintln!("zsh not found; skipping");
        return;
    };
    let (_base, dir) = hostile_dir();
    let out = run(Command::new(zsh)
        .arg("-f")
        .arg("-c")
        .arg(format!(
            "source '{}' 2>/dev/null; HOST=box; cd -- \"$1\" && __caelum_osc7",
            snippet("caelum.zsh")
        ))
        .arg("_")
        .arg(&dir));
    assert_single_cwd(&out, "box".into(), &dir);
}

#[test]
fn fish_osc7_encodes_hostile_directory_names() {
    let Some(fish) = find("fish") else {
        eprintln!("fish not found; skipping");
        return;
    };
    let (_base, dir) = hostile_dir();
    let out = run(Command::new(fish)
        .arg("--no-config")
        .arg("-c")
        .arg(format!(
            "source '{}'; cd $argv[1]; and __caelum_osc7",
            snippet("caelum.fish")
        ))
        .arg(&dir));
    // fish always reports its own $hostname; accept whatever host it printed.
    let text = String::from_utf8_lossy(&out);
    let host = text
        .strip_prefix("\x1b]7;file://")
        .and_then(|r| r.split('/').next())
        .unwrap_or_default()
        .to_string();
    assert_single_cwd(&out, host, &dir);
}
