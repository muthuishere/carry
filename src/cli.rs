//! clap definitions, dispatch, JSON envelope and exit codes. See docs/cli-contract.md.
#![allow(dead_code)]

use crate::error::{CarryError, Result};
use crate::settings::{self, Settings};
use anyhow::anyhow;
use base64::Engine;
use clap::{Args, Parser, Subcommand};
use serde_json::{json, Value};
use std::io::{Read, Write};

#[derive(Debug, Parser)]
#[command(name = "carry", version, about = "Your container's tools and config, from any S3 bucket")]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,
    #[command(subcommand)]
    pub cmd: Command,
}

#[derive(Debug, Clone, Args, Default)]
pub struct GlobalArgs {
    /// Named profile from the config file
    #[arg(long, global = true)]
    pub profile: Option<String>,
    /// Repository URL, e.g. s3://bucket/prefix
    #[arg(long, global = true)]
    pub repo: Option<String>,
    /// S3 endpoint URL
    #[arg(long, global = true)]
    pub endpoint: Option<String>,
    /// S3 region
    #[arg(long, global = true)]
    pub region: Option<String>,
    /// Emit one JSON envelope on stdout
    #[arg(long, global = true)]
    pub json: bool,
    /// Store timeout in seconds (default 30)
    #[arg(long, global = true)]
    pub timeout: Option<u64>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create the repository (idempotent)
    Init,
    /// Check store reachability and capabilities
    Doctor,
    /// Upload a file (or `-` for stdin) to <state:path>
    Put { target: String, source: String },
    /// Download <state:path> to stdout or a file
    Get { target: String, dest: Option<String> },
    /// List <state>[:<dir>]
    Ls { target: String },
    /// Remove <state:path>
    Rm { target: String },
    /// Move <state:from> to <to>
    Mv { from: String, to: String },
    /// Show version history of a state
    Log {
        state: String,
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Compare two versions of a state
    Diff { state: String, v1: u64, v2: u64 },
    /// Make an old version current again
    Rollback { state: String, version: u64 },
    /// Manage a state's environment
    #[command(subcommand)]
    Env(EnvCmd),
    /// List states
    States,
    /// Local restore status
    Status,
    /// Restore, then run and supervise a command
    Run {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        cmd: Vec<String>,
    },
    /// Manage the shared tool store
    #[command(subcommand)]
    Tool(ToolCmd),
    /// Manage named profiles
    #[command(subcommand)]
    Profile(ProfileCmd),
}

#[derive(Debug, Subcommand)]
pub enum EnvCmd {
    Set {
        state: String,
        #[arg(required = true)]
        pairs: Vec<String>,
    },
    Unset {
        state: String,
        #[arg(required = true)]
        keys: Vec<String>,
    },
    Import { state: String, file: String },
    Get { state: String, key: Option<String> },
}

#[derive(Debug, Subcommand)]
pub enum ToolCmd {
    Publish {
        /// name@version
        spec: String,
        #[arg(long, conflicts_with = "url", required_unless_present = "url")]
        file: Option<String>,
        #[arg(long)]
        url: Option<String>,
        #[arg(long, requires = "url")]
        sha256: Option<String>,
        /// os/arch, e.g. linux/amd64
        #[arg(long)]
        platform: String,
    },
    Promote {
        spec: String,
        #[arg(long)]
        channel: String,
    },
    Yank { spec: String },
    Ls { name: Option<String> },
    Add {
        state: String,
        /// name[@version]
        spec: String,
        #[arg(long)]
        channel: Option<String>,
        #[arg(long)]
        prefetch: bool,
        #[arg(long)]
        required: bool,
    },
    Rm { state: String, name: String },
    Keygen,
}

#[derive(Debug, Subcommand)]
pub enum ProfileCmd {
    List,
    Use { name: String },
    Current,
}

impl Command {
    fn name(&self) -> &'static str {
        match self {
            Command::Init => "init",
            Command::Doctor => "doctor",
            Command::Put { .. } => "put",
            Command::Get { .. } => "get",
            Command::Ls { .. } => "ls",
            Command::Rm { .. } => "rm",
            Command::Mv { .. } => "mv",
            Command::Log { .. } => "log",
            Command::Diff { .. } => "diff",
            Command::Rollback { .. } => "rollback",
            Command::Env(_) => "env",
            Command::States => "states",
            Command::Status => "status",
            Command::Run { .. } => "run",
            Command::Tool(_) => "tool",
            Command::Profile(_) => "profile",
        }
    }

    /// Commands that mutate the store; refused under a readonly profile.
    fn is_write(&self) -> bool {
        match self {
            Command::Init
            | Command::Put { .. }
            | Command::Rm { .. }
            | Command::Mv { .. }
            | Command::Rollback { .. } => true,
            Command::Env(e) => !matches!(e, EnvCmd::Get { .. }),
            Command::Tool(t) => !matches!(t, ToolCmd::Ls { .. } | ToolCmd::Keygen),
            _ => false,
        }
    }
}

// ---------- path parsing ----------

/// Parse `<state>:<absolute path>`.
pub fn parse_state_path(s: &str) -> Result<(String, String)> {
    let (state, path) = s
        .split_once(':')
        .ok_or_else(|| CarryError::Usage(format!("expected <state>:<path>, got '{s}'")))?;
    if state.is_empty() {
        return Err(CarryError::Usage(format!("empty state name in '{s}'")));
    }
    if !path.starts_with('/') {
        return Err(CarryError::Usage(format!("path must be absolute in '{s}'")));
    }
    Ok((state.to_string(), path.to_string()))
}

/// Parse `<state>[:<dir>]`.
fn parse_state_dir(s: &str) -> Result<(String, Option<String>)> {
    match s.split_once(':') {
        Some(_) => parse_state_path(s).map(|(a, b)| (a, Some(b))),
        None if s.is_empty() => Err(CarryError::Usage("empty state name".into())),
        None => Ok((s.to_string(), None)),
    }
}

/// Parse `name@version` (version required) or `name[@version]` when `optional`.
fn parse_spec(s: &str, optional: bool) -> Result<(String, Option<String>)> {
    match s.split_once('@') {
        Some((n, v)) if !n.is_empty() && !v.is_empty() => Ok((n.into(), Some(v.into()))),
        None if optional && !s.is_empty() => Ok((s.into(), None)),
        _ => Err(CarryError::Usage(format!("expected name@version, got '{s}'"))),
    }
}

// ---------- output ----------

fn emit_ok(json_mode: bool, result: &Value) {
    let mut out = std::io::stdout().lock();
    if json_mode {
        let _ = writeln!(out, "{}", json!({"ok": true, "result": result}));
    } else {
        let _ = write!(out, "{}", render_human(result));
    }
}

fn emit_err(json_mode: bool, e: &CarryError) {
    if json_mode {
        let v = json!({"ok": false, "error": {"code": e.code(), "message": e.to_string()}});
        println!("{v}");
    } else {
        eprintln!("carry: {e}");
    }
}

fn scalar(v: &Value) -> String {
    match v {
        Value::Null => "-".into(),
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().map(scalar).collect::<Vec<_>>().join(","),
        other => other.to_string(),
    }
}

/// Concise human rendering: scalars as `key: value`, arrays of objects as aligned tables.
fn render_human(v: &Value) -> String {
    let mut s = String::new();
    let Value::Object(map) = v else {
        return format!("{}\n", scalar(v));
    };
    for (k, val) in map {
        match val {
            Value::Array(rows) if rows.iter().all(|r| r.is_object()) && !rows.is_empty() => {
                let cols: Vec<String> = rows[0].as_object().unwrap().keys().cloned().collect();
                let cells: Vec<Vec<String>> = rows
                    .iter()
                    .map(|r| cols.iter().map(|c| scalar(&r[c.as_str()])).collect())
                    .collect();
                let widths: Vec<usize> = cols
                    .iter()
                    .enumerate()
                    .map(|(i, c)| cells.iter().map(|r| r[i].len()).max().unwrap_or(0).max(c.len()))
                    .collect();
                s.push_str(&format!("{k}:\n"));
                let line = |r: &[String]| {
                    r.iter()
                        .zip(&widths)
                        .map(|(c, w)| format!("{c:<w$}"))
                        .collect::<Vec<_>>()
                        .join("  ")
                        .trim_end()
                        .to_string()
                };
                let hdr: Vec<String> = cols.iter().map(|c| c.to_uppercase()).collect();
                s.push_str(&format!("  {}\n", line(&hdr)));
                for r in &cells {
                    s.push_str(&format!("  {}\n", line(r)));
                }
            }
            Value::Object(inner) => {
                s.push_str(&format!("{k}:\n"));
                for (ik, iv) in inner {
                    s.push_str(&format!("  {ik}: {}\n", scalar(iv)));
                }
            }
            other => s.push_str(&format!("{k}: {}\n", scalar(other))),
        }
    }
    s
}

// ---------- entry ----------

pub fn main() -> i32 {
    let argv: Vec<String> = std::env::args().collect();
    let json_hint = argv.iter().take_while(|a| *a != "--").any(|a| a == "--json");
    let cli = match Cli::try_parse_from(&argv) {
        Ok(c) => c,
        Err(e) => {
            use clap::error::ErrorKind;
            if matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) {
                let _ = e.print();
                return 0;
            }
            if json_hint {
                let msg = e.kind().to_string();
                emit_err(true, &CarryError::Usage(format!("{msg}: {}", first_line(&e.to_string()))));
            } else {
                let _ = e.print();
            }
            return 2;
        }
    };
    let json_mode = cli.global.json;

    let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            emit_err(json_mode, &CarryError::Internal(anyhow!("tokio runtime: {e}")));
            return 1;
        }
    };
    rt.block_on(async move {
        match dispatch(cli).await {
            Ok(Outcome::Json(v)) => {
                emit_ok(json_mode, &v);
                0
            }
            Ok(Outcome::Done) => 0,
            Ok(Outcome::Exit(code)) => code,
            Err(e) => {
                emit_err(json_mode, &e);
                e.exit_code()
            }
        }
    })
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").trim_start_matches("error: ").to_string()
}

enum Outcome {
    Json(Value),
    /// Output already written (raw `get`).
    Done,
    /// Exit with a child's code (`run`).
    Exit(i32),
}

async fn dispatch(cli: Cli) -> Result<Outcome> {
    let g = cli.global;
    // Profile management works without a resolvable store config.
    if let Command::Profile(p) = &cli.cmd {
        return Ok(Outcome::Json(match p {
            ProfileCmd::List => {
                let (current, profiles) = settings::profile_list()?;
                json!({"current": current, "profiles": profiles})
            }
            ProfileCmd::Use { name } => json!({"current": settings::profile_use(name)?}),
            ProfileCmd::Current => json!({"current": settings::profile_current(g.profile.as_deref())?}),
        }));
    }

    let s = Settings::resolve(&g)?;
    if cli.cmd.is_write() && s.readonly {
        return Err(CarryError::ReadOnly(s.profile.clone().unwrap_or_default()));
    }

    let v = match cli.cmd {
        Command::Init => adapter::init(&s).await?,
        Command::Doctor => adapter::doctor(&s).await?,
        Command::Put { target, source } => {
            let (state, path) = parse_state_path(&target)?;
            let (bytes, mode) = read_source(&source)?;
            adapter::put(&s, &state, &path, bytes, mode).await?
        }
        Command::Get { target, dest } => {
            let (state, path) = parse_state_path(&target)?;
            let out = adapter::get(&s, &state, &path).await?;
            if s.json {
                json!({
                    "path": path,
                    "size": out.bytes.len(),
                    "mode": out.mode,
                    "content_base64": base64::engine::general_purpose::STANDARD.encode(&out.bytes),
                })
            } else {
                write_dest(dest.as_deref(), &out.bytes, out.mode)?;
                return Ok(Outcome::Done);
            }
        }
        Command::Ls { target } => {
            let (state, dir) = parse_state_dir(&target)?;
            adapter::ls(&s, &state, dir.as_deref()).await?
        }
        Command::Rm { target } => {
            let (state, path) = parse_state_path(&target)?;
            adapter::rm(&s, &state, &path).await?
        }
        Command::Mv { from, to } => {
            let (state, from) = parse_state_path(&from)?;
            // `to` may be a bare absolute path or the same `state:path`.
            let to = if to.starts_with('/') {
                to
            } else {
                let (ts, tp) = parse_state_path(&to)?;
                if ts != state {
                    return Err(CarryError::Usage("mv cannot move between states".into()));
                }
                tp
            };
            adapter::mv(&s, &state, &from, &to).await?
        }
        Command::Log { state, limit } => adapter::log(&s, &state, limit).await?,
        Command::Diff { state, v1, v2 } => adapter::diff(&s, &state, v1, v2).await?,
        Command::Rollback { state, version } => adapter::rollback(&s, &state, version).await?,
        Command::Env(e) => match e {
            EnvCmd::Set { state, pairs } => {
                let kv = pairs
                    .iter()
                    .map(|p| {
                        p.split_once('=')
                            .filter(|(k, _)| !k.is_empty())
                            .map(|(k, v)| (k.to_string(), v.to_string()))
                            .ok_or_else(|| CarryError::Usage(format!("expected K=V, got '{p}'")))
                    })
                    .collect::<Result<Vec<_>>>()?;
                adapter::env_set(&s, &state, kv).await?
            }
            EnvCmd::Unset { state, keys } => adapter::env_unset(&s, &state, keys).await?,
            EnvCmd::Import { state, file } => {
                let (bytes, _) = read_source(&file)?;
                let text = String::from_utf8(bytes)
                    .map_err(|_| CarryError::Invalid(format!("{file} is not UTF-8")))?;
                adapter::env_import(&s, &state, &text).await?
            }
            EnvCmd::Get { state, key } => adapter::env_get(&s, &state, key.as_deref()).await?,
        },
        Command::States => adapter::states(&s).await?,
        Command::Status => adapter::status(&s).await?,
        Command::Run { cmd } => return Ok(Outcome::Exit(adapter::run(&s, cmd).await)),
        Command::Tool(t) => match t {
            ToolCmd::Publish { spec, file, url, sha256, platform } => {
                let (name, ver) = parse_spec(&spec, false)?;
                let src = match (file, url) {
                    (Some(f), _) => ToolSource::File(f.into()),
                    (None, Some(u)) => ToolSource::Url { url: u, sha256 },
                    _ => return Err(CarryError::Usage("--file or --url is required".into())),
                };
                adapter::tool_publish(&s, &name, &ver.unwrap(), src, &platform).await?
            }
            ToolCmd::Promote { spec, channel } => {
                let (name, ver) = parse_spec(&spec, false)?;
                adapter::tool_promote(&s, &name, &ver.unwrap(), &channel).await?
            }
            ToolCmd::Yank { spec } => {
                let (name, ver) = parse_spec(&spec, false)?;
                adapter::tool_yank(&s, &name, &ver.unwrap()).await?
            }
            ToolCmd::Ls { name } => adapter::tool_ls(&s, name.as_deref()).await?,
            ToolCmd::Add { state, spec, channel, prefetch, required } => {
                let (name, version) = parse_spec(&spec, true)?;
                let r = crate::model::ToolRef { version, channel, prefetch, required };
                adapter::tool_add(&s, &state, &name, r).await?
            }
            ToolCmd::Rm { state, name } => adapter::tool_rm(&s, &state, &name).await?,
            ToolCmd::Keygen => adapter::tool_keygen(&s).await?,
        },
        Command::Profile(_) => unreachable!("handled above"),
    };
    Ok(Outcome::Json(v))
}

/// Read a put/import source: a path or `-` for stdin. Returns (bytes, unix mode).
fn read_source(src: &str) -> Result<(Vec<u8>, u32)> {
    if src == "-" {
        let mut buf = Vec::new();
        std::io::stdin()
            .read_to_end(&mut buf)
            .map_err(|e| CarryError::Internal(anyhow!("reading stdin: {e}")))?;
        return Ok((buf, 0o644));
    }
    let bytes = std::fs::read(src).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => CarryError::NotFound(src.to_string()),
        _ => CarryError::Usage(format!("cannot read {src}: {e}")),
    })?;
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(src).map(|m| m.permissions().mode() & 0o7777).unwrap_or(0o644)
    };
    #[cfg(not(unix))]
    let mode = 0o644;
    Ok((bytes, mode))
}

fn write_dest(dest: Option<&str>, bytes: &[u8], mode: u32) -> Result<()> {
    match dest {
        None | Some("-") => std::io::stdout()
            .lock()
            .write_all(bytes)
            .map_err(|e| CarryError::Internal(anyhow!("writing stdout: {e}"))),
        Some(p) => {
            std::fs::write(p, bytes).map_err(|e| CarryError::Usage(format!("cannot write {p}: {e}")))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode));
            }
            Ok(())
        }
    }
}

pub enum ToolSource {
    File(std::path::PathBuf),
    Url { url: String, sha256: Option<String> },
}

pub struct GetOutput {
    pub bytes: Vec<u8>,
    pub mode: u32,
}

// =====================================================================
// ADAPTER: the only place cli.rs touches ops/restore/run/tools.
// Each stub below is to be replaced by a one-line call into the owning
// module once it lands. Expected signatures (all `async`, all return
// `crate::error::Result<serde_json::Value>` shaped per docs/cli-contract.md
// unless noted):
//
//   ops::init(&Settings) -> Result<Value>
//   ops::doctor(&Settings) -> Result<Value>
//   ops::put(&Settings, state: &str, path: &str, bytes: Vec<u8>, mode: u32) -> Result<Value>
//   ops::get(&Settings, state: &str, path: &str) -> Result<cli::GetOutput>   // {bytes, mode}
//   ops::ls(&Settings, state: &str, dir: Option<&str>) -> Result<Value>
//   ops::rm(&Settings, state: &str, path: &str) -> Result<Value>
//   ops::mv(&Settings, state: &str, from: &str, to: &str) -> Result<Value>
//   ops::log(&Settings, state: &str, limit: Option<usize>) -> Result<Value>
//   ops::diff(&Settings, state: &str, v1: u64, v2: u64) -> Result<Value>
//   ops::rollback(&Settings, state: &str, version: u64) -> Result<Value>
//   ops::env_set(&Settings, state: &str, kv: Vec<(String, String)>) -> Result<Value>
//   ops::env_unset(&Settings, state: &str, keys: Vec<String>) -> Result<Value>
//   ops::env_import(&Settings, state: &str, dotenv_text: &str) -> Result<Value>
//   ops::env_get(&Settings, state: &str, key: Option<&str>) -> Result<Value>
//   ops::states(&Settings) -> Result<Value>
//   restore::status(&Settings) -> Result<Value>          // reads <root>/status.json
//   run::run(&Settings, cmd: Vec<String>) -> i32         // child's exit code; never JSON
//   tools::publish(&Settings, name: &str, version: &str, src: cli::ToolSource, platform: &str) -> Result<Value>
//   tools::promote(&Settings, name: &str, version: &str, channel: &str) -> Result<Value>
//   tools::yank(&Settings, name: &str, version: &str) -> Result<Value>
//   tools::ls(&Settings, name: Option<&str>) -> Result<Value>
//   tools::add(&Settings, state: &str, name: &str, r: model::ToolRef) -> Result<Value>
//   tools::rm(&Settings, state: &str, name: &str) -> Result<Value>
//   tools::keygen(&Settings) -> Result<Value>            // writes key file, never prints private key
// =====================================================================
mod adapter {
    use super::{GetOutput, ToolSource};
    use crate::error::{CarryError, Result};
    use crate::model::ToolRef;
    use crate::settings::Settings;
    use anyhow::anyhow;
    use serde_json::Value;

    fn todo_dispatch<T>(cmd: &str) -> Result<T> {
        Err(CarryError::Internal(anyhow!("not implemented: {cmd}")))
    }

    pub async fn init(_s: &Settings) -> Result<Value> { todo_dispatch("init") }
    pub async fn doctor(_s: &Settings) -> Result<Value> { todo_dispatch("doctor") }
    pub async fn put(_s: &Settings, _state: &str, _path: &str, _bytes: Vec<u8>, _mode: u32) -> Result<Value> { todo_dispatch("put") }
    pub async fn get(_s: &Settings, _state: &str, _path: &str) -> Result<GetOutput> { todo_dispatch("get") }
    pub async fn ls(_s: &Settings, _state: &str, _dir: Option<&str>) -> Result<Value> { todo_dispatch("ls") }
    pub async fn rm(_s: &Settings, _state: &str, _path: &str) -> Result<Value> { todo_dispatch("rm") }
    pub async fn mv(_s: &Settings, _state: &str, _from: &str, _to: &str) -> Result<Value> { todo_dispatch("mv") }
    pub async fn log(_s: &Settings, _state: &str, _limit: Option<usize>) -> Result<Value> { todo_dispatch("log") }
    pub async fn diff(_s: &Settings, _state: &str, _v1: u64, _v2: u64) -> Result<Value> { todo_dispatch("diff") }
    pub async fn rollback(_s: &Settings, _state: &str, _version: u64) -> Result<Value> { todo_dispatch("rollback") }
    pub async fn env_set(_s: &Settings, _state: &str, _kv: Vec<(String, String)>) -> Result<Value> { todo_dispatch("env set") }
    pub async fn env_unset(_s: &Settings, _state: &str, _keys: Vec<String>) -> Result<Value> { todo_dispatch("env unset") }
    pub async fn env_import(_s: &Settings, _state: &str, _text: &str) -> Result<Value> { todo_dispatch("env import") }
    pub async fn env_get(_s: &Settings, _state: &str, _key: Option<&str>) -> Result<Value> { todo_dispatch("env get") }
    pub async fn states(_s: &Settings) -> Result<Value> { todo_dispatch("states") }
    pub async fn status(_s: &Settings) -> Result<Value> { todo_dispatch("status") }
    pub async fn run(_s: &Settings, _cmd: Vec<String>) -> i32 {
        eprintln!("carry: not implemented: run");
        1
    }
    pub async fn tool_publish(_s: &Settings, _name: &str, _ver: &str, _src: ToolSource, _platform: &str) -> Result<Value> { todo_dispatch("tool publish") }
    pub async fn tool_promote(_s: &Settings, _name: &str, _ver: &str, _channel: &str) -> Result<Value> { todo_dispatch("tool promote") }
    pub async fn tool_yank(_s: &Settings, _name: &str, _ver: &str) -> Result<Value> { todo_dispatch("tool yank") }
    pub async fn tool_ls(_s: &Settings, _name: Option<&str>) -> Result<Value> { todo_dispatch("tool ls") }
    pub async fn tool_add(_s: &Settings, _state: &str, _name: &str, _r: ToolRef) -> Result<Value> { todo_dispatch("tool add") }
    pub async fn tool_rm(_s: &Settings, _state: &str, _name: &str) -> Result<Value> { todo_dispatch("tool rm") }
    pub async fn tool_keygen(_s: &Settings) -> Result<Value> { todo_dispatch("tool keygen") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_path() {
        assert_eq!(parse_state_path("web-1:/etc/a").unwrap(), ("web-1".into(), "/etc/a".into()));
        assert!(parse_state_path("web-1").is_err());
        assert!(parse_state_path("web-1:rel").is_err());
        assert!(parse_state_path(":/x").is_err());
        assert_eq!(parse_state_dir("w").unwrap(), ("w".into(), None));
        assert_eq!(parse_spec("jq@1.7", false).unwrap(), ("jq".into(), Some("1.7".into())));
        assert!(parse_spec("jq", false).is_err());
        assert_eq!(parse_spec("jq", true).unwrap(), ("jq".into(), None));
    }
}
