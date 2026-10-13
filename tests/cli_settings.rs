//! Precedence as observed through the binary: flag > env > config file.
use std::process::{Command, Output};

fn run(cfg: &std::path::Path, args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_carry"));
    c.args(args).env("CARRY_CONFIG", cfg).env_remove("CARRY_PROFILE");
    for (k, v) in envs {
        c.env(k, v);
    }
    c.output().unwrap()
}

#[test]
fn precedence_flag_env_profile() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("c.yaml");
    std::fs::write(
        &p,
        "current-profile: dev\nprofiles:\n  dev: {repo: s3://a/b}\n  ro: {repo: s3://c/d, readonly: true}\n",
    )
    .unwrap();
    // env CARRY_PROFILE=ro beats file current-profile=dev -> readonly refusal.
    let o = run(&p, &["--json", "init"], &[("CARRY_PROFILE", "ro")]);
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stdout).contains("readonly"));
    // flag --profile dev beats env ro -> passes the guard.
    let o = run(&p, &["--json", "--profile", "dev", "init"], &[("CARRY_PROFILE", "ro")]);
    assert!(!String::from_utf8_lossy(&o.stdout).contains("readonly"));
    // profile current honours env over file.
    let o = run(&p, &["--json", "profile", "current"], &[("CARRY_PROFILE", "ro")]);
    assert!(String::from_utf8_lossy(&o.stdout).contains("\"ro\""));
}

#[test]
fn env_repo_beats_profile_repo() {
    // Unit-level check lives in src/settings.rs tests; here make sure CARRY_REPO
    // does not break resolution under a profile.
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("c.yaml");
    std::fs::write(&p, "current-profile: dev\nprofiles:\n  dev: {repo: s3://a/b}\n").unwrap();
    let o = run(&p, &["--json", "states"], &[("CARRY_REPO", "s3://env/x")]);
    assert_ne!(o.status.code(), Some(2), "{}", String::from_utf8_lossy(&o.stdout));
}
