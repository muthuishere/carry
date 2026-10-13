use std::process::{Command, Output};
use std::time::{Duration, Instant};

fn carry(root: &std::path::Path, args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_carry"));
    c.args(args)
        .env("CARRY_ROOT", root)
        .env("CARRY_CONFIG", root.join("none.yaml"));
    for v in [
        "CARRY_PROFILE",
        "CARRY_REPO",
        "CARRY_KEY",
        "CARRY_KEY_FILE",
        "CARRY_STATE",
        "CARRY_S3_ENDPOINT",
    ] {
        c.env_remove(v);
    }
    for (k, v) in envs {
        c.env(k, v);
    }
    c.output().unwrap()
}

const UNREACHABLE: &[(&str, &str)] = &[
    ("CARRY_REPO", "s3://bucket/p"),
    ("CARRY_S3_ENDPOINT", "http://127.0.0.1:1"),
    ("CARRY_S3_REGION", "us-east-1"),
    ("AWS_ACCESS_KEY_ID", "x"),
    ("AWS_SECRET_ACCESS_KEY", "y"),
    ("CARRY_NO_ENCRYPTION", "1"),
    ("CARRY_RESTORE_BUDGET_MS", "500"),
    ("CARRY_STATE", "web-1"),
];

#[test]
fn no_repo_passthrough() {
    let t = tempfile::tempdir().unwrap();
    let o = carry(t.path(), &["run", "--", "sh", "-c", "echo hi; exit 7"], &[]);
    assert_eq!(o.status.code(), Some(7));
    assert_eq!(String::from_utf8_lossy(&o.stdout).trim(), "hi");
    let s = carry(t.path(), &["status", "--json"], &[]);
    let v: serde_json::Value = serde_json::from_slice(&s.stdout).unwrap();
    assert_eq!(v["result"]["mode"], "disabled");
}

#[test]
fn unreachable_store_degrades_and_runs() {
    let t = tempfile::tempdir().unwrap();
    let start = Instant::now();
    let o = carry(t.path(), &["run", "--", "sh", "-c", "exit 7"], UNREACHABLE);
    assert!(
        start.elapsed() < Duration::from_millis(1500),
        "{:?}",
        start.elapsed()
    );
    assert_eq!(
        o.status.code(),
        Some(7),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let s = carry(t.path(), &["status", "--json"], UNREACHABLE);
    let v: serde_json::Value = serde_json::from_slice(&s.stdout).unwrap();
    assert_eq!(v["result"]["mode"], "degraded");
    assert_eq!(v["result"]["state"], "web-1");
}
