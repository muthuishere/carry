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
    if let Some(m) = &manifest {
        restore::materialise(&root, m);
        link(&root, &m.apps);
    }
    let env = child_env(&stored_env, std::env::vars(), &root.join("bin"));

    let mut command = tokio::process::Command::new(&cmd[0]);
    command.args(&cmd[1..]).env_clear().envs(&env);
    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("carry: cannot start {}: {e}", cmd[0]);
            return 127;
        }
    };
    let Some(pid) = child.id().map(|p| Pid::from_raw(p as i32)) else { return 1 };

    // Background follower: isolated in its own task, so a panic cannot reach the child.
    let follower = {
        let s = s.clone();
        tokio::spawn(async move {
            if AssertUnwindSafe(follow(s, state, pid, r.version)).catch_unwind().await.is_err() {
                eprintln!("carry: background follower panicked; app keeps running");
            }
        })
    };
    let forwarder = tokio::spawn(forward_signals(pid));

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
    code
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
    let path = env.get("PATH").cloned().unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".into());
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
    st.code().or_else(|| st.signal().map(|s| 128 + s)).unwrap_or(1)
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
    let mut sigchld = match signal(SignalKind::child()) {
        Ok(s) => s,
        Err(_) => return 1,
    };
    loop {
        loop {
            match waitpid(Pid::from_raw(-1), Some(WaitPidFlag::WNOHANG)) {
                Ok(WaitStatus::Exited(p, c)) if p == child => return c,
                Ok(WaitStatus::Signaled(p, s, _)) if p == child => return 128 + s as i32,
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
    let name = if up.starts_with("SIG") { up } else { format!("SIG{up}") };
    name.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_precedence() {
        let stored: BTreeMap<_, _> = [("A", "stored"), ("B", "stored")].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let env = child_env(&stored, vec![("B".into(), "proc".into()), ("PATH".into(), "/bin".into())], Path::new("/c/bin"));
        assert_eq!(env["A"], "stored");
        assert_eq!(env["B"], "proc");
        assert_eq!(env["PATH"], "/c/bin:/bin");
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
        assert_eq!(run(&s, vec!["sh".into(), "-c".into(), "exit 7".into()]).await, 7);
        assert!(t.elapsed() < Duration::from_millis(1300));
        assert_eq!(run(&s, vec!["sh".into(), "-c".into(), "kill -TERM $$".into()]).await, 128 + 15);
        assert_eq!(run(&s, vec!["/nonexistent/bin".into()]).await, 127);
    }
}
