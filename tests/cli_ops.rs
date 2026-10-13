//! put/get/log through the binary against a real S3 (MinIO). Skipped unless
//! CARRY_TEST_ENDPOINT is set (plus CARRY_TEST_ACCESS_KEY / CARRY_TEST_SECRET_KEY,
//! optional CARRY_TEST_BUCKET, default `carry-test`).
use std::process::{Command, Output};

// Fixed throwaway test key: 32 bytes of 'A'.
const TEST_KEY: &str = "QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE=";

fn carry(dir: &std::path::Path, repo: &str, endpoint: &str, args: &[&str]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_carry"));
    c.args(["--json", "--repo", repo, "--endpoint", endpoint, "--region", "us-east-1"])
        .args(args)
        .env("CARRY_CONFIG", dir.join("none.yaml"))
        .env("CARRY_KEY", TEST_KEY)
        .env_remove("CARRY_PROFILE")
        .env_remove("CARRY_KEY_FILE");
    if let Ok(v) = std::env::var("CARRY_TEST_ACCESS_KEY") {
        c.env("AWS_ACCESS_KEY_ID", v);
    }
    if let Ok(v) = std::env::var("CARRY_TEST_SECRET_KEY") {
        c.env("AWS_SECRET_ACCESS_KEY", v);
    }
    c.output().unwrap()
}

fn ok(o: &Output) -> serde_json::Value {
    let s = String::from_utf8_lossy(&o.stdout);
    let v: serde_json::Value = serde_json::from_str(s.trim()).unwrap_or_else(|e| panic!("{e}: {s} / {}", String::from_utf8_lossy(&o.stderr)));
    assert_eq!(v["ok"], true, "{v}");
    assert!(o.status.success());
    v["result"].clone()
}

#[test]
fn put_get_log_via_binary() {
    let Ok(endpoint) = std::env::var("CARRY_TEST_ENDPOINT") else { return };
    let bucket = std::env::var("CARRY_TEST_BUCKET").unwrap_or_else(|_| "carry-test".into());
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let repo = format!("s3://{bucket}/cli-ops-{nonce}");
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("app.yaml");
    std::fs::write(&src, b"port: 8080\n").unwrap();

    assert_eq!(ok(&carry(d.path(), &repo, &endpoint, &["init"]))["created"], true);
    let p = ok(&carry(d.path(), &repo, &endpoint, &["put", "web-1:/etc/app.yaml", src.to_str().unwrap()]));
    assert_eq!((p["version"].as_u64(), p["size"].as_u64()), (Some(1), Some(11)));
    let big = d.path().join("big");
    std::fs::write(&big, vec![9u8; 10_000]).unwrap();
    let p = ok(&carry(d.path(), &repo, &endpoint, &["put", "web-1:/big", big.to_str().unwrap()]));
    assert_eq!(p["uploaded"], true);

    let g = ok(&carry(d.path(), &repo, &endpoint, &["get", "web-1:/etc/app.yaml"]));
    assert_eq!(g["content_base64"], "cG9ydDogODA4MAo=");
    let l = ok(&carry(d.path(), &repo, &endpoint, &["log", "web-1"]));
    assert_eq!(l["versions"][0]["version"], 2);
    assert_eq!(l["versions"][1]["args"], serde_json::json!(["/etc/app.yaml"]));

    let o = carry(d.path(), &repo, &endpoint, &["get", "web-1:/missing"]);
    assert_eq!(o.status.code(), Some(2));
}
