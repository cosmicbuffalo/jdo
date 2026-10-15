//! Configuration.
//!
//! Config lives at `$XDG_CONFIG_HOME/jdo/config.toml` (`[jira]` section). The
//! Jira site and board are required; nothing about any particular site is
//! built in. `JIRA_BASE_URL` overrides the site and `--board` / `JDO_BOARD` the
//! board. Secrets (`JIRA_EMAIL`, `JIRA_API_TOKEN`) come from the environment
//! only. Custom-field ids are optional overrides: anything not pinned here is
//! looked up by name from the site's field catalog (see [`crate::jira::fields`]).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

/// Settings after merging the file, the environment and the command line.
#[derive(Debug, Clone)]
pub struct JiraConfig {
    /// Site root, no trailing slash (e.g. `https://your-site.atlassian.net`).
    pub base_url: String,
    pub auth: AuthKind,
    /// Board id, from the board URL `.../boards/<id>`.
    pub board_id: u64,
    pub refresh_seconds: u64,
    pub fields: FieldOverrides,
}

/// The `[jira]` table as written; everything optional so the merge with the
/// environment and command line can report exactly what's missing.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct FileConfig {
    base_url: Option<String>,
    auth: AuthKind,
    board_id: Option<u64>,
    refresh_seconds: Option<u64>,
    fields: FieldOverrides,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthKind {
    #[default]
    ApiToken,
    OAuth,
}

/// `[jira.fields]`: custom-field ids pinned by hand. Each one left out is
/// resolved by name from the site's field catalog at startup.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FieldOverrides {
    pub story_points: Option<String>,
    pub acceptance_criteria: Option<String>,
    pub sprint: Option<String>,
    pub epic_link: Option<String>,
    pub team: Option<String>,
}

impl FieldOverrides {
    /// Every field is pinned, so the catalog needn't be fetched.
    pub fn complete(&self) -> bool {
        [&self.story_points, &self.acceptance_criteria, &self.sprint, &self.epic_link, &self.team]
            .iter()
            .all(|f| f.is_some())
    }
}

/// Resolved custom-field ids. An empty id means the site has no such field;
/// the features that need it stay out of the way.
#[derive(Debug, Clone, Default)]
pub struct FieldMap {
    pub story_points: String,
    pub acceptance_criteria: String,
    pub sprint: String,
    pub epic_link: String,
    pub team: String,
}

/// API-token credentials pulled from the environment.
pub struct Credentials {
    pub email: String,
    pub token: String,
}

const SETUP_HINT: &str = "see \"Setup\" in the jdo README";

/// Load config from the file (if present), then apply env and CLI overrides.
pub fn load(cli_board: Option<u64>) -> Result<JiraConfig> {
    let path = config_path();
    let file = match &path {
        Some(p) => read_file(p)?.unwrap_or_default(),
        None => FileConfig::default(),
    };
    let shown = path.as_ref().map_or_else(|| "~/.config/jdo/config.toml".into(), |p| p.display().to_string());

    let base_url = std::env::var("JIRA_BASE_URL")
        .ok()
        .or(file.base_url)
        .map(|url| url.trim().trim_end_matches('/').to_string())
        .filter(|url| !url.is_empty())
        .ok_or_else(|| {
            anyhow!(
                "no Jira site configured: set base_url = \"https://<your-site>.atlassian.net\" under [jira] in {shown}, \
                 or export JIRA_BASE_URL ({SETUP_HINT})"
            )
        })?;
    if !base_url.starts_with("https://") && !base_url.starts_with("http://") {
        bail!("base_url {base_url:?} should start with https:// ({SETUP_HINT})");
    }

    let env_board = match std::env::var("JDO_BOARD") {
        Ok(board) => Some(board.trim().parse::<u64>().with_context(|| format!("JDO_BOARD={board:?} isn't a board id"))?),
        Err(_) => None,
    };
    let board_id = cli_board.or(env_board).or(file.board_id).ok_or_else(|| {
        anyhow!(
            "no board configured: pass --board <id>, export JDO_BOARD, or set board_id under [jira] in {shown} \
             (the id is the number in the board's URL, .../boards/<id>; {SETUP_HINT})"
        )
    })?;

    Ok(JiraConfig {
        base_url,
        auth: file.auth,
        board_id,
        refresh_seconds: file.refresh_seconds.unwrap_or(25),
        fields: file.fields,
    })
}

/// `Ok(None)` when there's no config file; a file that doesn't parse is an
/// error rather than silently ignored.
fn read_file(path: &Path) -> Result<Option<FileConfig>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    parse(&text).with_context(|| format!("parse {}", path.display())).map(Some)
}

fn parse(text: &str) -> Result<FileConfig> {
    #[derive(Default, Deserialize)]
    struct Wrapper {
        #[serde(default)]
        jira: FileConfig,
    }

    Ok(toml::from_str::<Wrapper>(text)?.jira)
}

pub fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("jdo").join("config.toml"))
}

pub fn credentials() -> Result<Credentials> {
    let email = std::env::var("JIRA_EMAIL")
        .map_err(|_| anyhow!("JIRA_EMAIL is not set (required for API-token auth; {SETUP_HINT})"))?;
    let token = std::env::var("JIRA_API_TOKEN")
        .map_err(|_| anyhow!("JIRA_API_TOKEN is not set ({SETUP_HINT})"))?;
    Ok(Credentials { email, token })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_with_some_fields_pinned_parses() {
        let cfg = parse(
            r#"
            [jira]
            base_url = "https://example.atlassian.net"
            board_id = 42
            [jira.fields]
            story_points = "customfield_10016"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.board_id, Some(42));
        assert_eq!(cfg.fields.story_points.as_deref(), Some("customfield_10016"));
        assert!(cfg.fields.sprint.is_none(), "unpinned fields are left to the catalog");
        assert!(!cfg.fields.complete());
    }

    #[test]
    fn typos_are_reported_not_ignored() {
        assert!(parse("[jira]\nbord_id = 42\n").is_err());
        assert!(parse("[jira.fields]\nstory_point = \"customfield_1\"\n").is_err());
    }
}
