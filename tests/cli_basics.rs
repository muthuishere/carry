use std::path::Path;
use std::process::{Command, Output};

fn carry(cfg: &Path, args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_carry"));
    c.args(args).env("CARRY_CONFIG", cfg);
    for v in [
        "CARRY_PROFILE",
        "CARRY_REPO",
        "CARRY_KEY",
        "CARRY_KEY_FILE",
        "AWS_ACCESS_KEY_ID",
        "AWS_SECRET_ACCESS_KEY",
    ] {
        c.env_remove(v);
    }
    for (k, v) in envs {
        c.env(k, v);
    }
    c.output().unwrap()
}

fn json(o: &Output) -> serde_json::Value {
    let s = String::from_utf8_lossy(&o.stdout);
    assert_eq!(s.trim().lines().count(), 1, "exactly one JSON line: {s}");
    serde_json::from_str(s.trim()).unwrap()
}

const CFG: &str = "current-profile: dev\nextra: 42\nprofiles:\n  dev:\n    repo: s3://dev-bucket/x\n  prod:\n    repo: s3://prod-bucket/x\n    readonly: true\n";

fn setup() -> (tempfile::TempDir, std::path::PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("config.yaml");
    std::fs::write(&p, CFG).unwrap();
    (d, p)
}

#[test]
fn profile_list_use_current() {
    let (_d, p) = setup();
    let o = carry(&p, &["--json", "profile", "list"], &[]);
    assert!(o.status.success());
    let v = json(&o);
    assert_eq!(v["ok"], true);
    assert_eq!(v["result"]["current"], "dev");
    assert_eq!(v["result"]["profiles"], serde_json::json!(["dev", "prod"]));

    let o = carry(&p, &["--json", "profile", "use", "prod"], &[]);
    assert_eq!(json(&o)["result"]["current"], "prod");
    let o = carry(&p, &["--json", "profile", "current"], &[]);
    assert_eq!(json(&o)["result"]["current"], "prod");
    let text = std::fs::read_to_string(&p).unwrap();
    assert!(text.contains("extra: 42") && text.contains("s3://dev-bucket/x"));

    let o = carry(&p, &["--json", "profile", "use", "nope"], &[]);
    assert_eq!(o.status.code(), Some(2));
    assert_eq!(json(&o)["error"]["code"], "not_found");

    let o = carry(&p, &["profile", "current"], &[]);
    assert!(String::from_utf8_lossy(&o.stdout).contains("current: prod"));
}

#[test]
fn readonly_profile_refuses_writes() {
    let (_d, p) = setup();
    let o = carry(&p, &["--json", "--profile", "prod", "rm", "web:/etc/a"], &[]);
    assert_eq!(o.status.code(), Some(2));
    let v = json(&o);
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"]["code"], "readonly");
    assert!(v["error"]["message"].as_str().unwrap().contains("prod"));
    // Human mode: error on stderr, nothing on stdout.
    let o = carry(&p, &["env", "set", "web", "A=1"], &[("CARRY_PROFILE", "prod")]);
    assert_eq!(o.status.code(), Some(2));
    assert!(o.stdout.is_empty());
    assert!(String::from_utf8_lossy(&o.stderr).contains("read-only"));
}

#[test]
fn usage_errors_exit_2() {
    let (_d, p) = setup();
    let o = carry(&p, &["--json", "frobnicate"], &[]);
    assert_eq!(o.status.code(), Some(2));
    assert_eq!(json(&o)["error"]["code"], "usage");
    let o = carry(&p, &["put"], &[]);
    assert_eq!(o.status.code(), Some(2));
    let o = carry(&p, &["--json", "put", "no-colon", "-"], &[]);
    assert_eq!(o.status.code(), Some(2));
    assert_eq!(json(&o)["error"]["code"], "usage");
    let o = carry(&p, &["--json", "--profile", "ghost", "states"], &[]);
    assert_eq!(o.status.code(), Some(2));
}

#[test]
fn help_works() {
    let (_d, p) = setup();
    let o = carry(&p, &["--help"], &[]);
    assert!(o.status.success());
    let s = String::from_utf8_lossy(&o.stdout);
    for c in ["init", "put", "run", "tool", "profile"] {
        assert!(s.contains(c), "help mentions {c}");
    }
}
