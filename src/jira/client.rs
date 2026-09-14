//! Blocking Jira Cloud REST client.
//!
//! P1 implements the read path against the Agile API. Writes (transitions,
//! field updates, comments) land in P3. All requests route through
//! [`super::auth::Auth`] so the API-token and OAuth backends are transparent.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;

use super::auth::Auth;
use super::wire::{BoardConfig, IssuePage, RawIssue};

pub struct JiraClient {
    http: Client,
    auth: Box<dyn Auth>,
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

    /// Board columns + their status mapping.
    pub fn board_configuration(&self, board_id: u64) -> Result<BoardConfig> {
        let url = format!(
            "{}/rest/agile/1.0/board/{}/configuration",
            self.auth.api_base(),
            board_id
        );
        self.get_json(&url, "board configuration")
    }

    /// All issues on the board, following pagination. `fields` is the field-id
    /// projection to request.
    pub fn board_issues(&self, board_id: u64, fields: &[&str]) -> Result<Vec<RawIssue>> {
        let mut all = Vec::new();
        let mut start = 0u32;
        let field_param = fields.join(",");
        loop {
            let url = format!(
                "{}/rest/agile/1.0/board/{}/issue?startAt={}&maxResults=50&fields={}",
                self.auth.api_base(),
                board_id,
                start,
                field_param
            );
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
