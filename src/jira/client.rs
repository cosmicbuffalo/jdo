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

    /// Send a JSON body, mapping a non-2xx response to a structured [`JiraError`].
    fn write_json(&self, request: reqwest::blocking::RequestBuilder, body: &Value) -> std::result::Result<(), JiraError> {
        let resp = self
            .auth
            .apply(request)
            .json(body)
            .send()
            .map_err(|e| JiraError::message(format!("network error: {e}")))?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        Err(error_from(status, &resp.text().unwrap_or_default()))
    }

    /// Board columns + their status mapping.
    pub fn board_configuration(&self, board_id: u64) -> Result<BoardConfig> {
        let url = self.url(&format!("/rest/agile/1.0/board/{board_id}/configuration"));
        self.get_json(&url, "board configuration")
    }

    pub fn board_info(&self, board_id: u64) -> Result<BoardInfo> {
        self.get_json(&self.url(&format!("/rest/agile/1.0/board/{board_id}")), "board")
    }

    /// Active and future sprints, in the board's order.
    pub fn open_sprints(&self, board_id: u64) -> Result<Vec<Sprint>> {
        let url = self.url(&format!("/rest/agile/1.0/board/{board_id}/sprint?state=active,future"));
        Ok(self.get_json::<SprintPage>(&url, "sprints")?.values)
    }

    /// Creatable issue types for a project: `(id, name)`.
    pub fn create_issue_types(&self, project: &str) -> Result<Vec<(String, String)>> {
        let url = self.url(&format!("/rest/api/3/issue/createmeta/{project}/issuetypes"));
        let page: Value = self.get_json(&url, "create metadata")?;
        let types = page["issueTypes"].as_array().or_else(|| page["values"].as_array()).cloned().unwrap_or_default();
        Ok(types
            .iter()
            .filter_map(|t| Some((t["id"].as_str()?.to_string(), t["name"].as_str()?.to_string())))
            .collect())
    }

    /// The fields Jira offers (and requires) when creating this issue type.
    pub fn create_fields(&self, project: &str, issue_type: &str) -> Result<Vec<Value>> {
        let url = self.url(&format!("/rest/api/3/issue/createmeta/{project}/issuetypes/{issue_type}?maxResults=200"));
        let page: Value = self.get_json(&url, "create fields")?;
        Ok(page["fields"].as_array().or_else(|| page["values"].as_array()).cloned().unwrap_or_default())
    }

    /// People assigned to the team's issues in the last six months, as
    /// `(accountId, displayName)` — the team's roster as Jira sees it (the
    /// Teams service itself isn't reachable with an API token).
    pub fn team_members(&self, team_field: &str, team_id: &str) -> Result<Vec<(String, String)>> {
        let number = team_field.trim_start_matches("customfield_");
        let jql = format!("cf[{number}] = {team_id} AND assignee is not EMPTY AND updated >= -180d");
        let mut people: Vec<(String, String)> = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..5 {
            let mut url = self.url(&format!("/rest/api/3/search/jql?fields=assignee&maxResults=100&jql={}", encode(&jql)));
            if let Some(t) = &token {
                url.push_str(&format!("&nextPageToken={}", encode(t)));
            }
            let page: Value = self.get_json(&url, "team members")?;
            for issue in page["issues"].as_array().into_iter().flatten() {
                let who = &issue["fields"]["assignee"];
                if let (Some(id), Some(name)) = (who["accountId"].as_str(), who["displayName"].as_str()) {
                    if !people.iter().any(|(i, _)| i == id) {
                        people.push((id.to_string(), name.to_string()));
                    }
                }
            }
            token = page["nextPageToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        Ok(people)
    }

    /// Issues matching `jql`, at most `max` of them (one page).
    pub fn search(&self, jql: &str, fields: &[&str], max: u32) -> Result<Vec<RawIssue>> {
        let url = self.url(&format!(
            "/rest/api/3/search/jql?maxResults={max}&fields={}&jql={}",
            fields.join(","),
            encode(jql)
        ));
        let page: Value = self.get_json(&url, "search")?;
        let issues = page.get("issues").cloned().unwrap_or(Value::Array(Vec::new()));
        serde_json::from_value(issues).context("parse search results")
    }

    /// One issue, or `None` if there's no such issue (or it isn't visible).
    pub fn find_issue(&self, key: &str, fields: &[&str]) -> Result<Option<RawIssue>> {
        let url = self.url(&format!("/rest/api/3/issue/{}?fields={}", encode(key), fields.join(",")));
        let resp = self.auth.apply(self.http.get(&url)).send().with_context(|| format!("GET {url}"))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            let status = resp.status();
            bail!("issue failed: HTTP {status} — {}", truncate(&resp.text().unwrap_or_default(), 300));
        }
        Ok(Some(resp.json().context("parse issue")?))
    }

    /// Create an issue; returns its key.
    pub fn create_issue(&self, fields: Value) -> std::result::Result<String, JiraError> {
        let url = self.url("/rest/api/3/issue");
        let resp = self
            .auth
            .apply(self.http.post(url))
            .json(&serde_json::json!({ "fields": fields }))
            .send()
            .map_err(|e| JiraError::message(format!("network error: {e}")))?;
        let status = resp.status();
        let text = resp.text().unwrap_or_default();
        if status.is_success() {
            let body: Value = serde_json::from_str(&text).unwrap_or_default();
            return body["key"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| JiraError::message("Jira didn't return the new issue's key"));
        }
        Err(error_from(status, &text))
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
        self.write_json(self.http.post(url), body)
    }

    /// Set fields on an issue: `fields` is `{field_id: value, ...}`.
    pub fn update_fields(&self, key: &str, fields: Value) -> std::result::Result<(), JiraError> {
        let url = self.url(&format!("/rest/api/3/issue/{key}"));
        self.write_json(self.http.put(url), &serde_json::json!({ "fields": fields }))
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

/// Jira's error body as a [`JiraError`], or the raw response if it isn't one.
fn error_from(status: reqwest::StatusCode, text: &str) -> JiraError {
    match serde_json::from_str::<ErrorBody>(text) {
        Ok(body) if !body.error_messages.is_empty() || !body.errors.is_empty() => {
            JiraError { messages: body.error_messages, field_errors: body.errors }
        }
        _ => JiraError::message(format!("HTTP {status} — {}", truncate(text, 300))),
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
