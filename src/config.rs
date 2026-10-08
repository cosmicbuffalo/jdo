//! Configuration.
//!
//! Config lives at `$XDG_CONFIG_HOME/jdo/config.toml` (`[jira]` section);
//! defaults are seeded from the live field discovery on `your-site.atlassian.net`.
//! Board can be overridden by `--board` / `JDO_BOARD`. Secrets
//! (`JIRA_EMAIL`, `JIRA_API_TOKEN`) come from the environment only.

use std::path::PathBuf;

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct JiraConfig {
    pub base_url: String,
    pub auth: AuthKind,
    /// Team board id, from the board URL `.../boards/<id>`. Overridable by
    /// `--board` / `JDO_BOARD`.
    pub board_id: u64,
    pub refresh_seconds: u64,
    pub fields: FieldMap,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthKind {
    #[default]
    ApiToken,
    OAuth,
}

/// Custom-field id overrides. Defaults match project DEMO and act as
/// fallbacks; runtime resolution (P1.x) will prefer the project's own catalog.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FieldMap {
    pub story_points: String,
    pub acceptance_criteria: String,
    pub sprint: String,
    pub epic_link: String,
    pub team: String,
}

impl Default for JiraConfig {
    fn default() -> Self {
        Self {
            base_url: "https://your-site.atlassian.net".into(),
            auth: AuthKind::ApiToken,
            // Default to the team board so `cargo run` works out of the box;
            // override with --board / JDO_BOARD / config.
            board_id: 123,
            refresh_seconds: 25,
            fields: FieldMap::default(),
        }
    }
}

impl Default for FieldMap {
    fn default() -> Self {
        Self {
            story_points: "customfield_10016".into(),
            acceptance_criteria: "customfield_10035".into(),
            sprint: "customfield_10020".into(),
            epic_link: "customfield_10014".into(),
            team: "customfield_10001".into(),
        }
    }
}

/// API-token credentials pulled from the environment.
pub struct Credentials {
    pub email: String,
    pub token: String,
}

/// Load config from file (if present) then apply env overrides.
pub fn load() -> Result<JiraConfig> {
    let mut cfg = read_file().unwrap_or_default();
    if let Ok(url) = std::env::var("JIRA_BASE_URL") {
        cfg.base_url = url;
    }
    if let Ok(board) = std::env::var("JDO_BOARD") {
        if let Ok(id) = board.parse() {
            cfg.board_id = id;
        }
    }
    Ok(cfg)
}

fn read_file() -> Option<JiraConfig> {
    let text = std::fs::read_to_string(config_path()?).ok()?;

    #[derive(Default, Deserialize)]
    struct Wrapper {
        #[serde(default)]
        jira: JiraConfig,
    }

    let wrapper: Wrapper = toml::from_str(&text).ok()?;
    Some(wrapper.jira)
}

fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("jdo").join("config.toml"))
}

pub fn credentials() -> Result<Credentials> {
    let email = std::env::var("JIRA_EMAIL")
        .map_err(|_| anyhow!("JIRA_EMAIL is not set (required for API-token auth)"))?;
    let token = std::env::var("JIRA_API_TOKEN")
        .map_err(|_| anyhow!("JIRA_API_TOKEN is not set"))?;
    Ok(Credentials { email, token })
}
