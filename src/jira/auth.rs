//! Authentication backends (P1: API token, P5: OAuth 2.0 3LO).
//!
//! Both implement a common `Auth` trait so the client is agnostic:
//! - `ApiTokenAuth` — Basic `email:token`; base `https://<site>.atlassian.net`.
//! - `OAuthAuth` — Bearer + refresh; base `https://api.atlassian.com/ex/jira/{cloudId}`
//!   (cloudId resolved via accessible-resources).
//!
//! Note the differing base-URL shapes: callers must use `Auth::api_base()`
//! rather than assuming a site URL.
