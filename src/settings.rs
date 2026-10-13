//! Settings resolution: flag > CARRY_* env > profile > default.
//! Profile management (list/use/current) over the YAML config file.
#![allow(dead_code)]

use crate::cli::GlobalArgs;
use crate::error::{CarryError, Result};
use anyhow::anyhow;
use serde::Deserialize;
use serde_yaml::{Mapping, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const DEFAULT_ROOT: &str = "/carry";
pub const DEFAULT_TIMEOUT_SECS: u64 = 30;
pub const DEFAULT_POLL_SECS: u64 = 30;
pub const DEFAULT_RESTORE_BUDGET_MS: u64 = 2000;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct ConfigFile {
    #[serde(default)]
    pub current_profile: Option<String>,
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Profile {
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub credentials: Option<SecretRef>,
    #[serde(default)]
    pub key: Option<SecretRef>,
    #[serde(default)]
    pub readonly: bool,
}

/// `{env: NAME}` or `{file: path}` in YAML.
#[derive(Debug, Clone, Deserialize)]
#[serde(from = "SecretRefRaw")]
pub enum SecretRef {
    Env(String),
    File(String),
    None,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretRefRaw {
    #[serde(default)]
    env: Option<String>,
    #[serde(default)]
    file: Option<String>,
}

impl From<SecretRefRaw> for SecretRef {
    fn from(r: SecretRefRaw) -> Self {
        match (r.env, r.file) {
            (Some(e), _) => SecretRef::Env(e),
            (None, Some(f)) => SecretRef::File(f),
            _ => SecretRef::None,
        }
    }
}

/// Resolved settings. `Debug` redacts secrets.
#[derive(Clone, Default)]
pub struct Settings {
    pub repo: Option<String>,
    pub endpoint: Option<String>,
    pub region: Option<String>,
    pub access_key_id: Option<String>,
    pub secret_access_key: Option<String>,
    /// Base64 master key (32 bytes), from CARRY_KEY / CARRY_KEY_FILE / profile.
    pub master_key: Option<String>,
    pub root: PathBuf,
    pub state: Option<String>,
    pub timeout_secs: u64,
    pub json: bool,
    pub readonly: bool,
    pub profile: Option<String>,
    pub tool_pubkey: Option<String>,
    /// Base64/PEM signing key contents (never printed).
    pub signing_key: Option<String>,
    pub poll_secs: u64,
    pub restore_budget_ms: u64,
    pub reload_signal: Option<String>,
    pub no_encryption: bool,
}

impl std::fmt::Debug for Settings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let red = |o: &Option<String>| o.as_ref().map(|_| "<redacted>");
        f.debug_struct("Settings")
            .field("repo", &self.repo)
            .field("endpoint", &self.endpoint)
            .field("region", &self.region)
            .field("access_key_id", &red(&self.access_key_id))
            .field("secret_access_key", &red(&self.secret_access_key))
            .field("master_key", &red(&self.master_key))
            .field("root", &self.root)
            .field("state", &self.state)
            .field("timeout_secs", &self.timeout_secs)
            .field("json", &self.json)
            .field("readonly", &self.readonly)
            .field("profile", &self.profile)
            .field("tool_pubkey", &self.tool_pubkey)
            .field("signing_key", &red(&self.signing_key))
            .field("poll_secs", &self.poll_secs)
            .field("restore_budget_ms", &self.restore_budget_ms)
            .field("reload_signal", &self.reload_signal)
            .field("no_encryption", &self.no_encryption)
            .finish()
    }
}

/// Real env first, then the launcher settings file (`CARRY_LAUNCHER_ENV`, written by
/// `carry run` for tool launchers whose env has the secrets stripped).
fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty()).or_else(|| launcher_env(name))
}

/// Value of `name` in the file named by CARRY_LAUNCHER_ENV. Ignored unless it is a
/// regular file owned by the current user and not readable by group/other.
fn launcher_env(name: &str) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let path = std::env::var("CARRY_LAUNCHER_ENV").ok().filter(|v| !v.is_empty())?;
    let md = std::fs::symlink_metadata(&path).ok()?;
    if !md.is_file() || md.uid() != nix::unistd::geteuid().as_raw() || md.mode() & 0o077 != 0 {
        return None;
    }
    std::fs::read_to_string(&path)
        .ok()?
        .lines()
        .filter_map(|l| l.split_once('='))
        .find(|(k, _)| k.trim() == name)
        .map(|(_, v)| v.to_string())
        .filter(|v| !v.is_empty())
}

fn read_trimmed(path: &str, what: &str) -> Result<String> {
    std::fs::read_to_string(expand(path))
        .map(|s| s.trim().to_string())
        .map_err(|e| CarryError::Usage(format!("cannot read {what} file {path}: {e}")))
}

fn expand(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(h) = dirs::home_dir() {
            return h.join(rest);
        }
    }
    PathBuf::from(path)
}

fn parse_u64(v: &str, what: &str) -> Result<u64> {
    v.trim()
        .parse()
        .map_err(|_| CarryError::Usage(format!("{what} must be a non-negative integer, got '{v}'")))
}

/// Parse `access_key_id` / `secret_access_key` from an ini (`k = v`) or yaml (`k: v`) file.
fn read_credentials_file(path: &str) -> Result<(Option<String>, Option<String>)> {
    let text = std::fs::read_to_string(expand(path))
        .map_err(|e| CarryError::Usage(format!("cannot read credentials file {path}: {e}")))?;
    let (mut a, mut s) = (None, None);
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.starts_with(';') || line.starts_with('[') {
            continue;
        }
        let Some(idx) = line.find(['=', ':']) else { continue };
        let k = line[..idx].trim().to_ascii_lowercase();
        let v = line[idx + 1..].trim().trim_matches(|c| c == '"' || c == '\'').to_string();
        match k.as_str() {
            "access_key_id" | "aws_access_key_id" => a = Some(v),
            "secret_access_key" | "aws_secret_access_key" => s = Some(v),
            _ => {}
        }
    }
    Ok((a, s))
}

pub fn config_path() -> PathBuf {
    if let Some(p) = env("CARRY_CONFIG") {
        return expand(&p);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config/carry/config.yaml")
}

pub fn load_config(path: &Path) -> Result<ConfigFile> {
    match std::fs::read_to_string(path) {
        Ok(t) if t.trim().is_empty() => Ok(ConfigFile::default()),
        Ok(t) => serde_yaml::from_str(&t)
            .map_err(|e| CarryError::Usage(format!("invalid config {}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ConfigFile::default()),
        Err(e) => Err(CarryError::Internal(anyhow!("reading {}: {e}", path.display()))),
    }
}

impl Settings {
    pub fn resolve(g: &GlobalArgs) -> Result<Settings> {
        let cfg = load_config(&config_path())?;
        let profile_name = g
            .profile
            .clone()
            .or_else(|| env("CARRY_PROFILE"))
            .or_else(|| cfg.current_profile.clone());
        let profile = match &profile_name {
            Some(n) => Some(
                cfg.profiles
                    .get(n)
                    .cloned()
                    .ok_or_else(|| CarryError::Usage(format!("unknown profile '{n}'")))?,
            ),
            None => None,
        };
        let p = profile.clone().unwrap_or_default();

        // Credentials: env AWS_* beats profile.
        let (mut ak, mut sk) = (env("AWS_ACCESS_KEY_ID"), env("AWS_SECRET_ACCESS_KEY"));
        if ak.is_none() || sk.is_none() {
            let (pa, ps) = match &p.credentials {
                Some(SecretRef::Env(prefix)) => (
                    env(&format!("{prefix}_ACCESS_KEY_ID")),
                    env(&format!("{prefix}_SECRET_ACCESS_KEY")),
                ),
                Some(SecretRef::File(f)) => read_credentials_file(f)?,
                _ => (None, None),
            };
            ak = ak.or(pa);
            sk = sk.or(ps);
        }

        let master_key = if let Some(k) = env("CARRY_KEY") {
            Some(k)
        } else if let Some(f) = env("CARRY_KEY_FILE") {
            Some(read_trimmed(&f, "key")?)
        } else {
            match &p.key {
                Some(SecretRef::Env(v)) => env(v),
                Some(SecretRef::File(f)) => Some(read_trimmed(f, "key")?),
                _ => None,
            }
        };

        let signing_key = if let Some(k) = env("CARRY_TOOL_SIGNING_KEY") {
            Some(k)
        } else if let Some(f) = env("CARRY_TOOL_SIGNING_KEY_FILE") {
            // A missing file is tolerated (e.g. `tool keygen` is about to create it);
            // commands that need the key report "no signing key" later.
            if expand(&f).exists() {
                Some(read_trimmed(&f, "signing key")?)
            } else {
                None
            }
        } else {
            None
        };

        let timeout_secs = match (&g.timeout, env("CARRY_TIMEOUT")) {
            (Some(t), _) => *t,
            (None, Some(v)) => parse_u64(&v, "CARRY_TIMEOUT")?,
            _ => DEFAULT_TIMEOUT_SECS,
        };

        Ok(Settings {
            repo: g.repo.clone().or_else(|| env("CARRY_REPO")).or(p.repo),
            endpoint: g.endpoint.clone().or_else(|| env("CARRY_S3_ENDPOINT")).or(p.endpoint),
            region: g.region.clone().or_else(|| env("CARRY_S3_REGION")).or(p.region),
            access_key_id: ak,
            secret_access_key: sk,
            master_key,
            root: env("CARRY_ROOT").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(DEFAULT_ROOT)),
            state: env("CARRY_STATE"),
            timeout_secs,
            json: g.json,
            readonly: p.readonly,
            profile: profile.as_ref().and(profile_name),
            tool_pubkey: env("CARRY_TOOL_PUBKEY"),
            signing_key,
            poll_secs: env("CARRY_POLL_SECS")
                .map(|v| parse_u64(&v, "CARRY_POLL_SECS"))
                .transpose()?
                .unwrap_or(DEFAULT_POLL_SECS),
            restore_budget_ms: env("CARRY_RESTORE_BUDGET_MS")
                .map(|v| parse_u64(&v, "CARRY_RESTORE_BUDGET_MS"))
                .transpose()?
                .unwrap_or(DEFAULT_RESTORE_BUDGET_MS),
            reload_signal: env("CARRY_RELOAD_SIGNAL"),
            no_encryption: env("CARRY_NO_ENCRYPTION").map(|v| v == "1" || v == "true").unwrap_or(false),
        })
    }
}

// ---------- profile management ----------

pub fn profile_list() -> Result<(Option<String>, Vec<String>)> {
    let cfg = load_config(&config_path())?;
    Ok((cfg.current_profile, cfg.profiles.keys().cloned().collect()))
}

pub fn profile_current(flag: Option<&str>) -> Result<String> {
    if let Some(p) = flag.map(str::to_string).or_else(|| env("CARRY_PROFILE")) {
        return Ok(p);
    }
    load_config(&config_path())?
        .current_profile
        .ok_or_else(|| CarryError::NotFound("no current profile set".into()))
}

/// Set `current-profile`, preserving every other key in the file. Creates the file if missing.
pub fn profile_use(name: &str) -> Result<String> {
    let path = config_path();
    let cfg = load_config(&path)?;
    if !cfg.profiles.contains_key(name) {
        return Err(CarryError::NotFound(format!("profile '{name}'")));
    }
    let mut doc: Value = match std::fs::read_to_string(&path) {
        Ok(t) if !t.trim().is_empty() => serde_yaml::from_str(&t)
            .map_err(|e| CarryError::Usage(format!("invalid config {}: {e}", path.display())))?,
        _ => Value::Mapping(Mapping::new()),
    };
    let map = doc
        .as_mapping_mut()
        .ok_or_else(|| CarryError::Usage("config root must be a mapping".into()))?;
    map.insert(Value::from("current-profile"), Value::from(name));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| CarryError::Internal(anyhow!(e)))?;
    }
    let text = serde_yaml::to_string(&doc).map_err(|e| CarryError::Internal(anyhow!(e)))?;
    let tmp = path.with_extension("yaml.tmp");
    std::fs::write(&tmp, text).map_err(|e| CarryError::Internal(anyhow!(e)))?;
    std::fs::rename(&tmp, &path).map_err(|e| CarryError::Internal(anyhow!(e)))?;
    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Only test in the binary that mutates process env.
    #[test]
    fn precedence_flag_env_profile_default() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.yaml");
        std::fs::write(
            &p,
            "current-profile: dev\nprofiles:\n  dev: {repo: s3://prof/x, region: eu-1, credentials: {env: DEVS3}}\n",
        )
        .unwrap();
        std::env::set_var("CARRY_CONFIG", &p);
        std::env::remove_var("CARRY_PROFILE");
        std::env::remove_var("AWS_ACCESS_KEY_ID");
        std::env::set_var("DEVS3_ACCESS_KEY_ID", "AKID");
        std::env::set_var("CARRY_REPO", "s3://env/x");
        let mut g = GlobalArgs::default();
        let s = Settings::resolve(&g).unwrap();
        assert_eq!(s.repo.as_deref(), Some("s3://env/x"));
        assert_eq!(s.region.as_deref(), Some("eu-1"));
        assert_eq!(s.access_key_id.as_deref(), Some("AKID"));
        assert_eq!(s.timeout_secs, DEFAULT_TIMEOUT_SECS);
        assert!(!format!("{s:?}").contains("AKID"));
        g.repo = Some("s3://flag/x".into());
        assert_eq!(Settings::resolve(&g).unwrap().repo.as_deref(), Some("s3://flag/x"));
        std::env::remove_var("CARRY_REPO");
        g.repo = None;
        assert_eq!(Settings::resolve(&g).unwrap().repo.as_deref(), Some("s3://prof/x"));

        // Launcher settings file: below real env, above the profile; must be private.
        use std::os::unix::fs::PermissionsExt;
        let lf = d.path().join("launcher.env");
        std::fs::write(&lf, "CARRY_REPO=s3://launcher/x\nCARRY_KEY=lk\n").unwrap();
        std::fs::set_permissions(&lf, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::env::set_var("CARRY_LAUNCHER_ENV", &lf);
        std::env::remove_var("CARRY_KEY");
        let s = Settings::resolve(&g).unwrap();
        assert_eq!((s.repo.as_deref(), s.master_key.as_deref()), (Some("s3://launcher/x"), Some("lk")));
        std::env::set_var("CARRY_REPO", "s3://env/x");
        assert_eq!(Settings::resolve(&g).unwrap().repo.as_deref(), Some("s3://env/x"));
        std::env::remove_var("CARRY_REPO");
        std::fs::set_permissions(&lf, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(Settings::resolve(&g).unwrap().repo.as_deref(), Some("s3://prof/x"));
        std::env::remove_var("CARRY_LAUNCHER_ENV");
    }
}
