//! Authentication backends.
//!
//! Both implement [`Auth`] so the client is agnostic. P1 ships `ApiTokenAuth`
//! (Basic `email:token`, base `https://<site>.atlassian.net`). `OAuthAuth`
//! (Bearer + refresh, base `https://api.atlassian.com/ex/jira/{cloudId}`)
//! arrives in P5 behind the same trait.

use base64::Engine;
use reqwest::blocking::RequestBuilder;

/// `Send` so the client (which owns a `Box<dyn Auth>`) can move to the worker thread.
pub trait Auth: Send {
    /// Base for REST calls, no trailing slash (e.g. `https://site.atlassian.net`).
    fn api_base(&self) -> &str;
    /// Base for browser links (e.g. issue URLs); usually the site URL.
    fn browse_base(&self) -> &str;
    /// Attach auth + accept headers to a request.
    fn apply(&self, req: RequestBuilder) -> RequestBuilder;
}

pub struct ApiTokenAuth {
    base: String,
    header: String,
}

impl ApiTokenAuth {
    pub fn new(base: &str, email: &str, token: &str) -> Self {
        let encoded = base64::engine::general_purpose::STANDARD.encode(format!("{email}:{token}"));
        Self {
            base: base.trim_end_matches('/').to_string(),
            header: format!("Basic {encoded}"),
        }
    }
}

impl Auth for ApiTokenAuth {
    fn api_base(&self) -> &str {
        &self.base
    }

    fn browse_base(&self) -> &str {
        &self.base
    }

    fn apply(&self, req: RequestBuilder) -> RequestBuilder {
        req.header(reqwest::header::AUTHORIZATION, &self.header)
            .header(reqwest::header::ACCEPT, "application/json")
    }
}
