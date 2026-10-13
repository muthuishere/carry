//! End-to-end tool store test against a real S3 (MinIO). Runs only when
//! CARRY_TEST_ENDPOINT is set (plus CARRY_TEST_ACCESS_KEY / CARRY_TEST_SECRET_KEY,
//! optional CARRY_TEST_BUCKET, default "carry-test").
use std::path::Path;
use std::process::{Command, Output};

struct Env {
    vars: Vec<(String, String)>,
}

impl Env {
    fn cmd(&self, exe: &Path, args: &[&str]) -> Output {
        let mut c = Command::new(exe);
        c.args(args);
        for v in [
            "CARRY_PROFILE",
            "CARRY_KEY",
            "CARRY_KEY_FILE",
            "CARRY_TOOL_SIGNING_KEY",
        ] {
            c.env_remove(v);
        }
        for (k, v) in &self.vars {
            c.env(k, v);
        }
        c.output().unwrap()
    }
}

fn platform() -> String {
    let os = if std::env::consts::OS == "macos" {
        "darwin"
    } else {
        std::env::consts::OS
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        a => a,
    };
    format!("{os}/{arch}")
}

fn ok(o: &Output) -> serde_json::Value {
    assert!(
        o.status.success(),
        "failed: {} {}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    serde_json::from_slice(&o.stdout).unwrap_or(serde_json::Value::Null)
}

#[test]
fn publish_add_launch() {
    let Ok(endpoint) = std::env::var("CARRY_TEST_ENDPOINT") else {
        eprintln!("skipping: CARRY_TEST_ENDPOINT not set");
        return;
    };
    let bucket = std::env::var("CARRY_TEST_BUCKET").unwrap_or_else(|_| "carry-test".into());
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("root");
    let keyfile = d.path().join("sign.key");
    let exe = Path::new(env!("CARGO_BIN_EXE_carry"));
    let prefix = format!("tools-it-{}", std::process::id());
    let mut env = Env {
        vars: vec![
            (
                "CARRY_CONFIG".into(),
                d.path().join("none.yaml").display().to_string(),
            ),
            ("CARRY_REPO".into(), format!("s3://{bucket}/{prefix}")),
            ("CARRY_S3_ENDPOINT".into(), endpoint),
            ("CARRY_S3_REGION".into(), "us-east-1".into()),
            (
                "AWS_ACCESS_KEY_ID".into(),
                std::env::var("CARRY_TEST_ACCESS_KEY").unwrap(),
            ),
            (
                "AWS_SECRET_ACCESS_KEY".into(),
                std::env::var("CARRY_TEST_SECRET_KEY").unwrap(),
            ),
            ("CARRY_NO_ENCRYPTION".into(), "1".into()),
            ("CARRY_ROOT".into(), root.display().to_string()),
            (
                "CARRY_TOOL_SIGNING_KEY_FILE".into(),
                keyfile.display().to_string(),
            ),
        ],
    };

    // keygen writes ~/.config/carry/tool-signing.key and prints only the public key.
    let home = d.path().join("home");
    let kenv = Env {
        vars: vec![
            ("HOME".into(), home.display().to_string()),
            env.vars[0].clone(),
        ],
    };
    let k = ok(&kenv.cmd(exe, &["--json", "tool", "keygen"]));
    assert_eq!(
        Path::new(k["result"]["private_key_file"].as_str().unwrap()),
        home.join(".config/carry/tool-signing.key")
    );
    std::fs::copy(home.join(".config/carry/tool-signing.key"), &keyfile).unwrap();
    let pubkey = k["result"]["public_key"].as_str().unwrap().to_string();
    let secret = std::fs::read_to_string(&keyfile).unwrap();
    assert!(!k.to_string().contains(secret.trim()));

    let tool = d.path().join("hello.sh");
    std::fs::write(&tool, "#!/bin/sh\necho \"hello $1 from $0\"\nexit 3\n").unwrap();
    let plat = platform();
    let t = tool.display().to_string();
    ok(&env.cmd(
        exe,
        &[
            "--json",
            "tool",
            "publish",
            "hello@1.0",
            "--file",
            &t,
            "--platform",
            &plat,
        ],
    ));
    ok(&env.cmd(
        exe,
        &[
            "--json",
            "tool",
            "publish",
            "hello@2.0",
            "--file",
            &t,
            "--platform",
            &plat,
        ],
    ));
    ok(&env.cmd(
        exe,
        &[
            "--json",
            "tool",
            "promote",
            "hello@2.0",
            "--channel",
            "stable",
        ],
    ));
    let ls = ok(&env.cmd(exe, &["--json", "tool", "ls"]));
    let s = ls.to_string();
    assert!(
        s.contains("\"1.0\"") && s.contains("\"2.0\"") && s.contains("stable"),
        "{s}"
    );

    ok(&env.cmd(exe, &["--json", "tool", "add", "web-1", "hello@1.0"]));
    ok(&env.cmd(
        exe,
        &[
            "--json",
            "tool",
            "add",
            "web-1",
            "chan",
            "--channel",
            "stable",
        ],
    ));

    // Simulate restore.
    std::fs::create_dir_all(root.join("current")).unwrap();
    std::fs::write(
        root.join("current/manifest.json"),
        r#"{"apps":{"hello":{"version":"1.0"},"chan":{"channel":"stable"}}}"#,
    )
    .unwrap();
    let link = d.path().join("hello");
    std::os::unix::fs::symlink(exe, &link).unwrap();

    // No pubkey -> refused, nothing runs.
    let o = env.cmd(&link, &["x"]);
    assert_eq!(o.status.code(), Some(127));
    assert!(String::from_utf8_lossy(&o.stderr).contains("refused"));

    // Wrong pubkey -> refused.
    let (_, other) = {
        let other_home = d.path().join("other");
        let v = ok(&Env {
            vars: vec![
                ("HOME".into(), other_home.display().to_string()),
                env.vars[0].clone(),
            ],
        }
        .cmd(exe, &["--json", "tool", "keygen"]));
        ((), v["result"]["public_key"].as_str().unwrap().to_string())
    };
    env.vars.push(("CARRY_TOOL_PUBKEY".into(), other));
    let o = env.cmd(&link, &["x"]);
    assert_eq!(
        o.status.code(),
        Some(127),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    env.vars.pop();

    env.vars.push(("CARRY_TOOL_PUBKEY".into(), pubkey));
    let o = env.cmd(&link, &["world"]);
    assert_eq!(
        o.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(String::from_utf8_lossy(&o.stdout).starts_with("hello world"));

    // Cache hit: works with the store unreachable.
    let mut offline = Env {
        vars: env.vars.clone(),
    };
    for v in offline.vars.iter_mut() {
        if v.0 == "CARRY_S3_ENDPOINT" {
            v.1 = "http://127.0.0.1:1".into();
        }
    }
    let o = offline.cmd(&link, &["again"]);
    assert_eq!(
        o.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );

    // Channel resolution, then yank names the yank.
    let chan = d.path().join("chan");
    std::os::unix::fs::symlink(exe, &chan).unwrap();
    // The channel tool is "chan" in apps, but the store has only "hello": not found.
    let o = env.cmd(&chan, &[]);
    assert_eq!(o.status.code(), Some(127));
    ok(&env.cmd(
        exe,
        &[
            "--json",
            "tool",
            "publish",
            "chan@2.0",
            "--file",
            &t,
            "--platform",
            &plat,
        ],
    ));
    ok(&env.cmd(
        exe,
        &[
            "--json",
            "tool",
            "promote",
            "chan@2.0",
            "--channel",
            "stable",
        ],
    ));
    assert_eq!(env.cmd(&chan, &["c"]).status.code(), Some(3));
    ok(&env.cmd(exe, &["--json", "tool", "yank", "chan@2.0"]));
    let o = env.cmd(&chan, &["c"]);
    assert_eq!(o.status.code(), Some(127));
    assert!(String::from_utf8_lossy(&o.stderr).contains("yanked"));

    ok(&env.cmd(exe, &["--json", "tool", "rm", "web-1", "chan"]));
}
