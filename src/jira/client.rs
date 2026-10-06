//! Blocking Jira Cloud REST client.
//!
//! Reads go through the Agile API (board configuration + issues) and the
//! Platform API (single issue, transitions, comments). Writes so far are
//! workflow transitions. All requests route through [`super::auth::Auth`] so
//! the API-token and OAuth backends are transparent.

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use serde_json::Value;

use super::auth::Auth;
use super::wire::{
    BoardConfig, BoardInfo, CommentPage, ErrorBody, IssuePage, Myself, RawIssue, Sprint,
    SprintPage, TransitionList,
};

pub struct JiraClient {
    http: Client,
    auth: Box<dyn Auth>,
}

/// A rejected write. Jira reports workflow-validator and field failures as
/// `errorMessages` plus per-field `errors`, which forms surface next to the
/// offending field.
#[derive(Debug, Clone, Default)]
pub struct JiraError {
    pub messages: Vec<String>,
    pub field_errors: BTreeMap<String, String>,
}

impl fmt::Display for JiraError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts: Vec<String> = self.messages.clone();
        parts.extend(self.field_errors.iter().map(|(k, v)| format!("{k}: {v}")));
        if parts.is_empty() {
            write!(f, "request failed")
        } else {
            write!(f, "{}", parts.join("; "))
        }
    }
}

impl JiraError {
    fn message(text: impl Into<String>) -> Self {
        Self {
            messages: vec![text.into()],
            field_errors: BTreeMap::new(),
        }
    }
}

impl JiraClient {
    pub fn new(auth: Box<dyn Auth>) -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("jdo/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("build HTTP client")?;
        Ok(Self { http, auth })
    }

    pub fn browse_base(&self) -> &str {
        self.auth.browse_base()
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.auth.api_base(), path)
    }

    fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str, what: &str) -> Result<T> {
        let resp = self
            .auth
            .apply(self.http.get(url))
            .send()
            .with_context(|| format!("GET {url}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().unwrap_or_default();
            bail!("{what} failed: HTTP {status} — {}", truncate(&body, 300));
        }
        resp.json::<T>().with_context(|| format!("parse {what}"))
    }

    /// POST a JSON body, mapping a non-2xx response to a structured [`JiraError`].
    fn post_json(&self, url: &str, body: &Value) -> std::result::Result<(), JiraError> {
        let resp = self
            .auth
            .apply(self.http.post(url))
            .json(body)
            .send()
            .map_err(|e| JiraError::message(format!("network error: {e}")))?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        let text = resp.text().unwrap_or_default();
        match serde_json::from_str::<ErrorBody>(&text) {
            Ok(body) if !body.error_messages.is_empty() || !body.errors.is_empty() => {
                Err(JiraError {
                    messages: body.error_messages,
                    field_errors: body.errors,
                })
            }
            _ => Err(JiraError::message(format!(
                "HTTP {status} — {}",
                truncate(&text, 300)
            ))),
        }
    }

    /// Board columns + their status mapping.
    pub fn board_configuration(&self, board_id: u64) -> Result<BoardConfig> {
        let url = self.url(&format!("/rest/agile/1.0/board/{board_id}/configuration"));
        self.get_json(&url, "board configuration")
    }

    pub fn board_info(&self, board_id: u64) -> Result<BoardInfo> {
        self.get_json(&self.url(&format!("/rest/agile/1.0/board/{board_id}")), "board")
    }

    pub fn active_sprints(&self, board_id: u64) -> Result<Vec<Sprint>> {
        let url = self.url(&format!("/rest/agile/1.0/board/{board_id}/sprint?state=active"));
        Ok(self.get_json::<SprintPage>(&url, "active sprints")?.values)
    }

    /// All issues on the board matching `jql` (if any), following pagination.
    pub fn board_issues(&self, board_id: u64, fields: &[&str], jql: Option<&str>) -> Result<Vec<RawIssue>> {
        let jql = jql.map(|q| format!("&jql={}", encode(q))).unwrap_or_default();
        self.paged_issues(&format!("/rest/agile/1.0/board/{board_id}/issue"), fields, &jql)
    }

    /// Issues in one sprint, as shown on the board.
    pub fn sprint_issues(&self, board_id: u64, sprint_id: u64, fields: &[&str]) -> Result<Vec<RawIssue>> {
        self.paged_issues(&format!("/rest/agile/1.0/board/{board_id}/sprint/{sprint_id}/issue"), fields, "")
    }

    fn paged_issues(&self, path: &str, fields: &[&str], extra: &str) -> Result<Vec<RawIssue>> {
        let mut all = Vec::new();
        let mut start = 0u32;
        let field_param = fields.join(",");
        loop {
            let url = self.url(&format!(
                "{path}?startAt={start}&maxResults=50&fields={field_param}{extra}"
            ));
            let page: IssuePage = self.get_json(&url, "board issues")?;
            let got = page.issues.len() as u32;
            all.extend(page.issues);
            start += got;
            if got == 0 || start >= page.total {
                break;
            }
        }
        Ok(all)
    }

    /// One issue with the given field projection.
    pub fn issue(&self, key: &str, fields: &[&str]) -> Result<RawIssue> {
        let url = self.url(&format!(
            "/rest/api/3/issue/{key}?fields={}",
            fields.join(",")
        ));
        self.get_json(&url, "issue")
    }

    /// Transitions available from the issue's current status, including the
    /// fields on each transition's screen.
    pub fn transitions(&self, key: &str) -> Result<TransitionList> {
        let url = self.url(&format!(
            "/rest/api/3/issue/{key}/transitions?expand=transitions.fields"
        ));
        self.get_json(&url, "transitions")
    }

    /// Perform a transition. `body` is the full request:
    /// `{"transition": {"id"}, "fields": {...}, "update": {...}}`.
    pub fn do_transition(&self, key: &str, body: &Value) -> std::result::Result<(), JiraError> {
        let url = self.url(&format!("/rest/api/3/issue/{key}/transitions"));
        self.post_json(&url, body)
    }

    /// Comments, oldest first (Jira's default order).
    pub fn comments(&self, key: &str) -> Result<CommentPage> {
        let url = self.url(&format!(
            "/rest/api/3/issue/{key}/comment?maxResults=100"
        ));
        self.get_json(&url, "comments")
    }

    pub fn myself(&self) -> Result<Myself> {
        self.get_json(&self.url("/rest/api/3/myself"), "current user")
    }
}

/// Percent-encode a query-string value.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}
