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

/// Bearer-token auth. Used by OAuth 2.0 (P5), and usable now with a manually
/// supplied `JDO_BEARER` token to validate the read path. OAuth access tokens
/// address Jira via `https://api.atlassian.com/ex/jira/{cloudId}` rather than
/// the site URL, so `api_base` and `browse_base` differ.
pub struct OAuthAuth {
    api_base: String,
    browse_base: String,
    header: String,
}

impl OAuthAuth {
    pub fn new(cloud_id: &str, browse_base: &str, bearer: &str) -> Self {
        Self {
            api_base: format!("https://api.atlassian.com/ex/jira/{cloud_id}"),
            browse_base: browse_base.trim_end_matches('/').to_string(),
            header: format!("Bearer {bearer}"),
        }
    }
}

impl Auth for OAuthAuth {
    fn api_base(&self) -> &str {
        &self.api_base
    }

    fn browse_base(&self) -> &str {
        &self.browse_base
    }

    fn apply(&self, req: RequestBuilder) -> RequestBuilder {
        req.header(reqwest::header::AUTHORIZATION, &self.header)
            .header(reqwest::header::ACCEPT, "application/json")
    }
}
