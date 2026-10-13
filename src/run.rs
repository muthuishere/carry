//! `carry run -- <cmd...>`: restore, materialise, link tools, then supervise the child.
//! The child's exit code is the only thing that matters; everything else fails open.
use crate::restore;
use crate::settings::Settings;
use futures::FutureExt;
use nix::sys::signal::{kill, Signal};
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::Pid;
use std::collections::BTreeMap;
use std::ffi::CString;
use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::time::Duration;
use tokio::signal::unix::{signal, SignalKind};

pub async fn run(s: &Settings, cmd: Vec<String>) -> i32 {
    if cmd.is_empty() {
        eprintln!("carry: run needs a command: carry run -- <cmd...>");
        return 2;
    }
    if s.repo.is_none() {
        return exec(&cmd);
    }
    let root = s.root.clone();
    let state = restore::state_name(s);
    let budget = Duration::from_millis(s.restore_budget_ms);
    let r = restore::restore(s, &state, &root, budget).await;
    if let Some(e) = &r.error {
        eprintln!("carry: restore degraded ({e}); starting with last good generation");
    }
    let manifest = restore::current_manifest(&root);
    let stored_env = manifest.as_ref().map(|m| m.env.clone()).unwrap_or_default();
    let apps = manifest
        .as_ref()
        .map(|m| m.apps.clone())
        .unwrap_or_default();
    if let Some(m) = &manifest {
        restore::materialise(&root, m);
        link(&root, &m.apps);
    }
    // Required tools: fetched before the app starts, within what is left of the restore
    // budget. A failure or timeout is reported and never blocks the start.
    let required: BTreeMap<_, _> = apps
        .iter()
        .filter(|(_, t)| t.required)
        .map(|(n, t)| (n.clone(), t.clone()))
        .collect();
    if !required.is_empty() {
        let left = budget.saturating_sub(Duration::from_millis(r.ms));
        match tokio::time::timeout(left, crate::tools::prefetch_tools(s, &root, &required, true)).await {
            Ok(res) => report_prefetch(&res),
            Err(_) => eprintln!("carry: warning: required tool prefetch exceeded the restore budget; starting anyway"),
        }
    }
    let mut env = child_env(&stored_env, std::env::vars(), &root.join("bin"));
    match write_launcher_env(s, &root) {
        Ok(p) => {
            env.insert(LAUNCHER_ENV.into(), p.display().to_string());
        }
        Err(e) => eprintln!(
            "carry: warning: cannot write launcher settings ({e}); tools may not reach the repo"
        ),
    }

    let mut command = tokio::process::Command::new(&cmd[0]);
    command.args(&cmd[1..]).env_clear().envs(&env);
    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("carry: cannot start {}: {e}", cmd[0]);
            return 127;
        }
    };
    let Some(pid) = child.id().map(|p| Pid::from_raw(p as i32)) else {
        return 1;
    };

    // Background follower: isolated in its own task, so a panic cannot reach the child.
    let follower = {
        let s = s.clone();
        tokio::spawn(async move {
            if AssertUnwindSafe(follow(s, state, pid, r.version))
                .catch_unwind()
                .await
                .is_err()
            {
                eprintln!("carry: background follower panicked; app keeps running");
            }
        })
    };
    let forwarder = tokio::spawn(forward_signals(pid));
    // Background prefetch of prefetch=true tools; never touches the app.
    let prefetch: BTreeMap<_, _> = apps.into_iter().filter(|(_, t)| t.prefetch).collect();
    let prefetcher = (!prefetch.is_empty()).then(|| {
        let (s, root) = (s.clone(), root.clone());
        tokio::spawn(async move {
            report_prefetch(&crate::tools::prefetch_tools(&s, &root, &prefetch, false).await)
        })
    });

    let code = if nix::unistd::getpid().as_raw() == 1 {
        reap_as_init(pid).await
    } else {
        match child.wait().await {
            Ok(st) => exit_code(st),
            Err(e) => {
                eprintln!("carry: wait failed: {e}");
                1
            }
        }
    };
    follower.abort();
    forwarder.abort();
    if let Some(p) = prefetcher {
        p.abort();
    }
    code
}

fn report_prefetch(res: &[(String, std::result::Result<(), String>)]) {
    for (name, r) in res {
        if let Err(e) = r {
            eprintln!("carry: warning: tool {name} not prefetched: {e}");
        }
    }
}

/// Secrets carry needs but the app must never see.
pub const STRIPPED_ENV: &[&str] = &[
    "CARRY_KEY",
    "CARRY_KEY_FILE",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_ACCESS_KEY_ID",
    "AWS_SESSION_TOKEN",
    "CARRY_TOOL_SIGNING_KEY",
    "CARRY_TOOL_SIGNING_KEY_FILE",
];
pub const LAUNCHER_ENV: &str = "CARRY_LAUNCHER_ENV";
pub const LAUNCHER_ENV_FILE: &str = ".launcher.env";

/// Write `<root>/.launcher.env` (0600, owned by us) with what a tool launcher needs to
/// reach the repo, since the app's environment no longer carries the secrets.
pub fn write_launcher_env(s: &Settings, root: &Path) -> std::io::Result<std::path::PathBuf> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut lines = Vec::new();
    let mut add = |k: &str, v: Option<&str>| {
        if let Some(v) = v.filter(|v| !v.contains('\n')) {
            lines.push(format!("{k}={v}"));
        }
    };
    add("CARRY_REPO", s.repo.as_deref());
    add("CARRY_S3_ENDPOINT", s.endpoint.as_deref());
    add("CARRY_S3_REGION", s.region.as_deref());
    add("AWS_ACCESS_KEY_ID", s.access_key_id.as_deref());
    add("AWS_SECRET_ACCESS_KEY", s.secret_access_key.as_deref());
    add(
        "AWS_SESSION_TOKEN",
        std::env::var("AWS_SESSION_TOKEN").ok().as_deref(),
    );
    add("CARRY_KEY", s.master_key.as_deref());
    add("CARRY_TOOL_PUBKEY", s.tool_pubkey.as_deref());
    if s.no_encryption {
        add("CARRY_NO_ENCRYPTION", Some("1"));
    }
    std::fs::create_dir_all(root)?;
    let path = root.join(LAUNCHER_ENV_FILE);
    let tmp = root.join(format!(".launcher.env.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits())
        .open(&tmp)?;
    f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    f.write_all(lines.join("\n").as_bytes())?;
    f.write_all(b"\n")?;
    f.sync_all()?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

fn link(root: &Path, apps: &BTreeMap<String, crate::model::ToolRef>) {
    if apps.is_empty() {
        return;
    }
    if let Err(e) = crate::tools::link_tools(root, apps) {
        eprintln!("carry: warning: cannot link tools: {e}");
    }
}

/// Stored env, overridden by the process env, with `<bin>` prepended to PATH.
pub fn child_env(
    stored: &BTreeMap<String, String>,
    process: impl IntoIterator<Item = (String, String)>,
    bin: &Path,
) -> BTreeMap<String, String> {
    let mut env = stored.clone();
    env.extend(process);
    for k in STRIPPED_ENV {
        env.remove(*k);
    }
    let path = env
        .get("PATH")
        .cloned()
        .unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".into());
    env.insert("PATH".into(), format!("{}:{path}", bin.display()));
    env
}

fn exec(cmd: &[String]) -> i32 {
    let args: Vec<CString> = match cmd.iter().map(|a| CString::new(a.as_bytes())).collect() {
        Ok(a) => a,
        Err(_) => {
            eprintln!("carry: argument contains a NUL byte");
            return 2;
        }
    };
    let e = nix::unistd::execvp(&args[0], &args).unwrap_err();
    eprintln!("carry: cannot exec {}: {e}", cmd[0]);
    127
}

fn exit_code(st: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    st.code()
        .or_else(|| st.signal().map(|s| 128 + s))
        .unwrap_or(1)
}

async fn forward_signals(pid: Pid) {
    let kinds = [
        (SignalKind::terminate(), Signal::SIGTERM),
        (SignalKind::interrupt(), Signal::SIGINT),
        (SignalKind::hangup(), Signal::SIGHUP),
        (SignalKind::quit(), Signal::SIGQUIT),
    ];
    let mut streams = Vec::new();
    for (k, sig) in kinds {
        match signal(k) {
            Ok(s) => streams.push((s, sig)),
            Err(e) => eprintln!("carry: cannot watch {sig}: {e}"),
        }
    }
    loop {
        let futs = streams.iter_mut().map(|(s, sig)| {
            let sig = *sig;
            Box::pin(async move {
                s.recv().await;
                sig
            })
        });
        let (sig, _, _) = futures::future::select_all(futs).await;
        let _ = kill(pid, sig);
    }
}

/// PID 1: reap every zombie; return when our child exits.
async fn reap_as_init(child: Pid) -> i32 {
    reap_with(child, || {
        waitpid(Pid::from_raw(-1), Some(WaitPidFlag::WNOHANG))
    })
    .await
}

async fn reap_with(child: Pid, mut wait: impl FnMut() -> nix::Result<WaitStatus>) -> i32 {
    let mut sigchld = match signal(SignalKind::child()) {
        Ok(s) => s,
        Err(_) => return 1,
    };
    loop {
        loop {
            match wait() {
                Ok(WaitStatus::Exited(p, c)) if p == child => return c,
                Ok(WaitStatus::Signaled(p, s, _)) if p == child => return 128 + s as i32,
                // ECHILD: nothing left to wait for, so our child is gone (reaped elsewhere).
                Err(nix::errno::Errno::ECHILD) => return 1,
                Ok(WaitStatus::StillAlive) | Err(_) => break,
                Ok(_) => continue,
            }
        }
        // Poll as a fallback in case a SIGCHLD was coalesced.
        let _ = tokio::time::timeout(Duration::from_secs(1), sigchld.recv()).await;
    }
}

/// Poll HEAD; on a new version restore, materialise and signal the child.
async fn follow(s: Settings, state: String, child: Pid, mut version: Option<u64>) {
    let every = Duration::from_secs(s.poll_secs.max(1));
    let reload = s.reload_signal.as_deref().and_then(parse_signal);
    if s.reload_signal.is_some() && reload.is_none() {
        eprintln!("carry: warning: unknown CARRY_RELOAD_SIGNAL, reload disabled");
    }
    loop {
        tokio::time::sleep(every).await;
        let budget = Duration::from_millis(s.restore_budget_ms.max(every.as_millis() as u64));
        let r = restore::restore(&s, &state, &s.root, budget).await;
        if let Some(e) = &r.error {
            eprintln!("carry: poll: {e}");
            continue;
        }
        if r.version == version {
            continue;
        }
        version = r.version;
        if let Some(m) = restore::current_manifest(&s.root) {
            restore::materialise(&s.root, &m);
            link(&s.root, &m.apps);
        }
        if let Some(sig) = reload {
            let _ = kill(child, sig);
        }
    }
}

pub fn parse_signal(v: &str) -> Option<Signal> {
    if let Ok(n) = v.parse::<i32>() {
        return Signal::try_from(n).ok();
    }
    let up = v.trim().to_ascii_uppercase();
    let name = if up.starts_with("SIG") {
        up
    } else {
        format!("SIG{up}")
    };
    name.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_precedence() {
        let stored: BTreeMap<_, _> = [("A", "stored"), ("B", "stored")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let env = child_env(
            &stored,
            vec![("B".into(), "proc".into()), ("PATH".into(), "/bin".into())],
            Path::new("/c/bin"),
        );
        assert_eq!(env["A"], "stored");
        assert_eq!(env["B"], "proc");
        assert_eq!(env["PATH"], "/c/bin:/bin");
    }

    #[test]
    fn secrets_are_stripped_from_child_env() {
        let stored: BTreeMap<String, String> = [("CARRY_KEY".to_string(), "s".to_string())]
            .into_iter()
            .collect();
        let proc: Vec<(String, String)> = STRIPPED_ENV
            .iter()
            .map(|k| (k.to_string(), "secret".to_string()))
            .chain([("KEEP".into(), "1".into())])
            .collect();
        let env = child_env(&stored, proc, Path::new("/c/bin"));
        for k in STRIPPED_ENV {
            assert!(!env.contains_key(*k), "{k} leaked");
        }
        assert_eq!(env["KEEP"], "1");
    }

    #[test]
    fn launcher_env_file_is_private() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let tmp = tempfile::tempdir().unwrap();
        let s = Settings {
            repo: Some("s3://b/p".into()),
            master_key: Some("k".into()),
            secret_access_key: Some("sk".into()),
            ..settings(tmp.path())
        };
        let p = write_launcher_env(&s, tmp.path()).unwrap();
        let md = std::fs::metadata(&p).unwrap();
        assert_eq!(md.permissions().mode() & 0o777, 0o600);
        assert_eq!(md.uid(), nix::unistd::geteuid().as_raw());
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(
            text.contains("CARRY_REPO=s3://b/p\n")
                && text.contains("CARRY_KEY=k\n")
                && text.contains("AWS_SECRET_ACCESS_KEY=sk\n")
        );
    }

    #[tokio::test]
    async fn reaper_treats_echild_as_child_gone() {
        let me = Pid::from_raw(4242);
        let r = tokio::time::timeout(
            Duration::from_secs(3),
            reap_with(me, || Err(nix::errno::Errno::ECHILD)),
        )
        .await;
        assert_eq!(r.unwrap(), 1);
        let mut n = 0;
        let r = reap_with(me, || {
            n += 1;
            if n == 1 {
                Ok(WaitStatus::Exited(Pid::from_raw(7), 0))
            } else {
                Ok(WaitStatus::Exited(me, 3))
            }
        });
        assert_eq!(r.await, 3);
    }

    #[test]
    fn signals() {
        assert_eq!(parse_signal("HUP"), Some(Signal::SIGHUP));
        assert_eq!(parse_signal("sigusr1"), Some(Signal::SIGUSR1));
        assert_eq!(parse_signal("15"), Some(Signal::SIGTERM));
        assert_eq!(parse_signal("NOPE"), None);
    }

    fn settings(root: &Path) -> Settings {
        Settings {
            repo: Some("s3://bucket/p".into()),
            endpoint: Some("http://127.0.0.1:1".into()),
            region: Some("us-east-1".into()),
            access_key_id: Some("x".into()),
            secret_access_key: Some("y".into()),
            root: root.into(),
            no_encryption: true,
            restore_budget_ms: 300,
            poll_secs: 30,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn exit_code_propagates_when_degraded() {
        let tmp = tempfile::tempdir().unwrap();
        let s = settings(tmp.path());
        let t = std::time::Instant::now();
        assert_eq!(
            run(&s, vec!["sh".into(), "-c".into(), "exit 7".into()]).await,
            7
        );
        assert!(t.elapsed() < Duration::from_millis(1300));
        assert_eq!(
            run(&s, vec!["sh".into(), "-c".into(), "kill -TERM $$".into()]).await,
            128 + 15
        );
        assert_eq!(run(&s, vec!["/nonexistent/bin".into()]).await, 127);
    }
}
