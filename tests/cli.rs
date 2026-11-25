//! End-to-end tests of the headless subcommands, run against the built binary.

use std::io::Write;
use std::process::{Command, Output, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_logradar");

const SAMPLE: &str = "\
[INFO] user alice logged in
[INFO] user bob logged in
[INFO] user carol logged in
[ERROR] db timeout after 30ms
[ERROR] db timeout after 45ms
[WARN] disk 91% full
[DEBUG] cache probe
";

fn run(args: &[&str], stdin: Option<&str>) -> Output {
    let mut child = Command::new(BIN)
        .args(args)
        .env_remove("RUST_BACKTRACE")
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run logradar");
    if let Some(input) = stdin {
        child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    }
    child.wait_with_output().unwrap()
}

fn sample_file(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("logradar-cli-{}-{}.log", std::process::id(), name));
    std::fs::write(&path, SAMPLE).unwrap();
    path
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[test]
fn summary_table_from_file() {
    let f = sample_file("table");
    let out = run(&["summary", "--file", f.to_str().unwrap()], None);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let stdout = text(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert!(lines[0].contains("COUNT") && lines[0].contains("PATTERN"));
    assert!(lines[1].contains("3") && lines[1].contains("[INFO] user <*> logged in"), "{}", stdout);
    assert!(stdout.contains("[ERROR] db timeout after <DUR>"));
    assert!(!stdout.contains("cache probe"), "DEBUG is below the default level");
    assert!(stdout.contains("3 patterns from 7 lines (6 at INFO or above)"), "{}", stdout);
}

#[test]
fn summary_json_from_stdin() {
    let out = run(&["summary", "--file", "-", "--json", "--level", "debug"], Some(SAMPLE));
    assert!(out.status.success(), "{}", text(&out.stderr));
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["lines"], 7);
    assert_eq!(doc["matched"], 7);
    let patterns = doc["patterns"].as_array().unwrap();
    assert_eq!(patterns.len(), 4);
    assert_eq!(patterns[0]["pattern"], "[INFO] user <*> logged in");
    assert_eq!(patterns[0]["count"], 3);
    assert_eq!(patterns[0]["sources"][0], "stdin");
    assert_eq!(patterns[0]["first_sample"], "[INFO] user alice logged in");
}

#[test]
fn summary_exact_mode_keeps_lines_apart() {
    let out = run(&["summary", "--file", "-", "--exact", "--json"], Some(SAMPLE));
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["total_patterns"], 5, "{}", doc);
}

#[test]
fn summary_fail_on_sets_exit_code() {
    let out = run(&["summary", "--file", "-", "--fail-on", "error"], Some(SAMPLE));
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("1 pattern(s) at ERROR or above"));

    let clean = "[INFO] all good\n[WARN] meh\n";
    let out = run(&["summary", "--file", "-", "--fail-on", "error"], Some(clean));
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn errors_are_one_line_with_exit_code_2() {
    let out = run(&["summary"], None);
    assert_eq!(out.status.code(), Some(2));
    let err = text(&out.stderr);
    assert_eq!(err.lines().count(), 1, "{}", err);
    assert!(err.starts_with("logradar: no sources"));

    let out = run(&["summary", "--file", "/nonexistent/logradar.log"], None);
    assert_eq!(out.status.code(), Some(2), "a missing file must not pass CI");

    let out = run(&["summary", "--file", "-", "--level", "loud"], Some(""));
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn watch_prints_new_patterns_until_input_ends() {
    let out = run(&["watch", "--file", "-"], Some(SAMPLE));
    assert!(out.status.success(), "{}", text(&out.stderr));
    let stdout = text(&out.stdout);
    let new: Vec<&str> = stdout.lines().filter(|l| l.contains(" NEW ")).collect();
    assert_eq!(new.len(), 3, "{}", stdout);
    assert!(new[0].contains("[INFO] user alice logged in"));
}

#[test]
fn watch_json_lines() {
    let out = run(&["watch", "--file", "-", "--json", "--level", "error"], Some(SAMPLE));
    let lines: Vec<serde_json::Value> = text(&out.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["event"], "new_pattern");
    assert_eq!(lines[0]["level"], "ERROR");
}

/// Output goes to a terminal or a log pipeline: control characters in log
/// lines must not reach it.
#[test]
fn headless_output_is_sanitized() {
    let evil = "[ERROR] boom \x1b]0;owned\x07 \x1b[2J done\n";
    for args in [
        &["summary", "--file", "-"][..],
        &["watch", "--file", "-"][..],
    ] {
        let out = run(args, Some(evil));
        let stdout = text(&out.stdout);
        assert!(!stdout.contains('\x1b'), "{:?}: {:?}", args, stdout);
        assert!(!stdout.contains('\x07'), "{:?}: {:?}", args, stdout);
    }
}
