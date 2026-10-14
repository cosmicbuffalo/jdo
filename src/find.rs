//! Finding any ticket, on the board or not.
//!
//! [`Finder`] is the `f` dialog: a query box over a result list. Typing
//! re-ranks what's already known (the board's cards and the last search's
//! results) at once with a fuzzy match on key + summary, and — after a short
//! pause — asks Jira for a text search over the board's project
//! ([`jql`]), plus a direct lookup when the query names an issue
//! ([`direct_key`]). Jira's answer is merged back through the same ranking.
//! An empty query lists the tickets you created most recently.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::create::looks_like_key;
use crate::model::Issue;

/// How long typing must pause before Jira is asked.
pub const DEBOUNCE: Duration = Duration::from_millis(200);
/// Results Jira is asked for (and the most the list shows).
pub const MAX_RESULTS: usize = 40;

pub struct Finder {
    pub query: String,
    /// What the list shows, best first.
    pub results: Vec<Issue>,
    pub highlight: usize,
    /// When the query changed without being searched yet.
    pub dirty_since: Option<Instant>,
    /// Number of the newest search sent; replies to older ones are dropped.
    pub seq: u64,
    pub searching: bool,
    pub error: Option<String>,
    /// Jira's results for the last search that came back.
    found: Vec<Issue>,
    /// The query `found` answers.
    pub found_for: Option<String>,
}

/// What a key press in the finder asks for.
pub enum FindOutcome {
    None,
    Open(Issue),
    Close,
}

impl Finder {
    /// Opens searching right away for your recent tickets.
    pub fn new() -> Self {
        Self {
            query: String::new(),
            results: Vec::new(),
            highlight: 0,
            dirty_since: Some(Instant::now() - DEBOUNCE),
            seq: 0,
            searching: false,
            error: None,
            found: Vec::new(),
            found_for: None,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent, board: &[&Issue]) -> FindOutcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let last = self.results.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc => return FindOutcome::Close,
            KeyCode::Enter => {
                return match self.results.get(self.highlight) {
                    Some(issue) => FindOutcome::Open(issue.clone()),
                    None => FindOutcome::None,
                };
            }
            KeyCode::Up => self.highlight = self.highlight.saturating_sub(1),
            KeyCode::Char('p') if ctrl => self.highlight = self.highlight.saturating_sub(1),
            KeyCode::Down => self.highlight = (self.highlight + 1).min(last),
            KeyCode::Char('n') if ctrl => self.highlight = (self.highlight + 1).min(last),
            KeyCode::PageUp => self.highlight = self.highlight.saturating_sub(10),
            KeyCode::PageDown => self.highlight = (self.highlight + 10).min(last),
            KeyCode::Backspace => {
                self.query.pop();
                self.edited(board);
            }
            KeyCode::Char('u') if ctrl => {
                self.query.clear();
                self.edited(board);
            }
            KeyCode::Char('w') if ctrl => {
                let kept = self.query.trim_end().rfind(' ').map(|i| i + 1).unwrap_or(0);
                self.query.truncate(kept);
                self.edited(board);
            }
            KeyCode::Char(c) if !ctrl => {
                self.query.push(c);
                self.edited(board);
            }
            _ => {}
        }
        FindOutcome::None
    }

    fn edited(&mut self, board: &[&Issue]) {
        self.dirty_since = Some(Instant::now());
        self.highlight = 0;
        self.rerank(board);
    }

    /// Re-rank what's known for the current query, without asking Jira.
    pub fn rerank(&mut self, board: &[&Issue]) {
        let selected = self.results.get(self.highlight).map(|i| i.key.clone());
        let fresh = self.found_for.as_deref() == Some(self.query.trim());
        // An empty query lists Jira's answer (your recent tickets) as is.
        self.results = if self.query.trim().is_empty() {
            if fresh { self.found.clone() } else { Vec::new() }
        } else {
            let server: &[Issue] = if fresh { &self.found } else { &[] };
            let known: Vec<&Issue> = board.iter().copied().chain(self.found.iter()).collect();
            rank(&self.query, server, &known)
        };
        self.highlight = selected
            .and_then(|key| self.results.iter().position(|i| i.key == key))
            .unwrap_or(self.highlight)
            .min(self.results.len().saturating_sub(1));
    }

    /// The query to send to Jira, once typing has paused. Marks it sent.
    pub fn due(&mut self, now: Instant) -> Option<(u64, String)> {
        let since = self.dirty_since?;
        if now.duration_since(since) < DEBOUNCE {
            return None;
        }
        self.dirty_since = None;
        self.seq += 1;
        self.searching = true;
        Some((self.seq, self.query.trim().to_string()))
    }

    /// Jira's answer to search `seq` (for `query`).
    pub fn on_results(&mut self, seq: u64, query: String, result: Result<Vec<Issue>, String>, board: &[&Issue]) {
        if seq != self.seq {
            return;
        }
        self.searching = false;
        match result {
            Ok(found) => {
                self.error = None;
                self.found = found;
                self.found_for = Some(query);
            }
            Err(message) => self.error = Some(message),
        }
        self.rerank(board);
    }

    /// Show `issue`'s latest values (e.g. after editing it from the results).
    pub fn update(&mut self, issue: &Issue) {
        for slot in self.results.iter_mut().chain(self.found.iter_mut()).filter(|i| i.key == issue.key) {
            *slot = issue.clone();
        }
    }

    /// Waiting on Jira, or about to ask.
    pub fn is_busy(&self) -> bool {
        self.searching || self.dirty_since.is_some()
    }
}

/// The issue a query names directly: `ABC-123` as typed, or a bare number in
/// the board's project.
pub fn direct_key(project: &str, query: &str) -> Option<String> {
    let query = query.trim().to_uppercase();
    if looks_like_key(&query) {
        return Some(query);
    }
    let numeric = !query.is_empty() && query.chars().all(|c| c.is_ascii_digit());
    (numeric && !project.is_empty()).then(|| format!("{project}-{query}"))
}

/// JQL for a query: every word as a prefix of something in the ticket's text
/// (summary, description, comments), newest activity first. An empty query
/// asks for the tickets you created, newest first.
pub fn jql(project: &str, query: &str) -> Option<String> {
    let scope = if project.is_empty() { String::new() } else { format!("project = \"{project}\" AND ") };
    if query.trim().is_empty() {
        return Some(format!("{scope}reporter = currentUser() ORDER BY created DESC"));
    }
    // Only letters and digits reach Jira, so nothing needs escaping.
    let words: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| format!("text ~ \"{}*\"", w.to_lowercase()))
        .collect();
    if words.is_empty() {
        return None;
    }
    Some(format!("{scope}{} ORDER BY updated DESC", words.join(" AND ")))
}

/// Best first: an exact key; then issues whose key + summary contain every
/// word (best score first); then the rest of Jira's matches (its order); then
/// known issues whose key + summary only have the words' letters in order.
/// Known issues that don't match at all are left out.
pub fn rank(query: &str, server: &[Issue], known: &[&Issue]) -> Vec<Issue> {
    let wanted = query.trim().to_uppercase();
    let mut seen = std::collections::HashSet::new();
    let mut scored: Vec<(u8, i64, Issue)> = Vec::new();
    let candidates = server.iter().map(|i| (i, true)).chain(known.iter().map(|i| (*i, false)));
    for (issue, from_jira) in candidates {
        if !seen.insert(issue.key.clone()) {
            continue;
        }
        let entry = match score(query, issue) {
            _ if issue.key == wanted || issue.key.ends_with(&format!("-{wanted}")) => (0, 0),
            Some(score) if contains_words(query, issue) => (1, -score),
            _ if from_jira => (2, 0),
            Some(score) => (3, -score),
            None => continue,
        };
        scored.push((entry.0, entry.1, issue.clone()));
    }
    // Stable, so ties keep Jira's order (then the board's).
    scored.sort_by_key(|(tier, score, _)| (*tier, *score));
    scored.into_iter().take(MAX_RESULTS).map(|(_, _, issue)| issue).collect()
}

/// Whether the key + summary contain every word of `query` as is.
fn contains_words(query: &str, issue: &Issue) -> bool {
    let text = format!("{} {}", issue.key, issue.summary).to_lowercase();
    query.to_lowercase().split_whitespace().all(|word| text.contains(word))
}

/// How well every word of `query` matches the issue's key + summary: a whole
/// substring scores high (more at a word start), otherwise the word's letters
/// must appear in order, scoring runs and word starts. `None` if a word
/// doesn't match at all.
pub fn score(query: &str, issue: &Issue) -> Option<i64> {
    let haystack: Vec<char> = format!("{} {}", issue.key, issue.summary).to_lowercase().chars().collect();
    let text: String = haystack.iter().collect();
    let word_start = |i: usize| i == 0 || !haystack[i - 1].is_alphanumeric();
    let mut total = 0;
    for word in query.to_lowercase().split_whitespace() {
        let needle: Vec<char> = word.chars().collect();
        if let Some(byte) = text.find(word) {
            let at = text[..byte].chars().count();
            total += 100 + 10 * needle.len() as i64 + if word_start(at) { 50 } else { 0 };
            continue;
        }
        let mut next = 0;
        let mut previous: Option<usize> = None;
        for &c in &needle {
            let at = (next..haystack.len()).find(|&i| haystack[i] == c)?;
            total += 1;
            if previous == Some(at.wrapping_sub(1)) {
                total += 5;
            }
            if word_start(at) {
                total += 8;
            }
            previous = Some(at);
            next = at + 1;
        }
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(key: &str, summary: &str) -> Issue {
        Issue { key: key.into(), summary: summary.into(), ..Default::default() }
    }

    fn keys(issues: &[Issue]) -> Vec<&str> {
        issues.iter().map(|i| i.key.as_str()).collect()
    }

    #[test]
    fn a_query_naming_an_issue_is_looked_up_directly() {
        assert_eq!(direct_key("DEMO", "demo-1234").as_deref(), Some("DEMO-1234"));
        assert_eq!(direct_key("DEMO", " 1234 ").as_deref(), Some("DEMO-1234"));
        assert_eq!(direct_key("", "1234"), None);
        assert_eq!(direct_key("DEMO", "fix login"), None);
    }

    #[test]
    fn words_become_prefix_text_searches_in_the_project() {
        assert_eq!(
            jql("DEMO", "Retry upl").as_deref(),
            Some("project = \"DEMO\" AND text ~ \"retry*\" AND text ~ \"upl*\" ORDER BY updated DESC")
        );
        // Punctuation splits words and never reaches Jira.
        assert_eq!(
            jql("P", "o\"auth-v2").as_deref(),
            Some("project = \"P\" AND text ~ \"o*\" AND text ~ \"auth*\" AND text ~ \"v2*\" ORDER BY updated DESC")
        );
        assert_eq!(jql("P", "--"), None);
        assert_eq!(
            jql("P", "").as_deref(),
            Some("project = \"P\" AND reporter = currentUser() ORDER BY created DESC")
        );
    }

    #[test]
    fn fuzzy_matching_prefers_substrings_and_word_starts() {
        let login = issue("P-1", "Fix login redirect");
        assert!(score("login", &login).is_some());
        assert!(score("lgn rdr", &login).is_some(), "letters in order match");
        assert!(score("nigol", &login).is_none(), "out of order doesn't");
        assert!(score("login", &login) > score("lgn", &login));
        assert!(score("p-1", &login).is_some(), "the key is searchable");
        let word_start = issue("P-2", "redirect handling");
        let mid_word = issue("P-3", "misdirected mail");
        assert!(score("direct", &issue("P-4", "direct deposit")) > score("direct", &mid_word));
        assert!(score("redirect", &word_start).is_some());
    }

    #[test]
    fn ranking_puts_exact_keys_first_then_fuzzy_then_other_jira_matches() {
        // Jira found P-7 by its description; P-9 is only on the board.
        let server = vec![issue("P-7", "Search errors"), issue("P-5", "Wall clock retries")];
        let board = issue("P-9", "Walt's lint fixes");
        let unrelated = issue("P-10", "Unrelated");
        let ranked = rank("wal", &server, &[&board, &unrelated]);
        assert_eq!(keys(&ranked), ["P-5", "P-9", "P-7"], "summary matches beat a description-only match");

        // Scattered letters (w…a…l in "Swap all") rank below Jira's matches.
        let scattered = issue("P-11", "Swap all");
        let ranked = rank("wal", &server, &[&scattered, &board]);
        assert_eq!(keys(&ranked), ["P-5", "P-9", "P-7", "P-11"]);

        let ranked = rank("10", &[], &[&board, &unrelated]);
        assert_eq!(ranked[0].key, "P-10", "a bare number finds its key");
    }

    #[test]
    fn typing_reranks_known_issues_at_once_and_waits_to_ask_jira() {
        let board = [issue("P-1", "Fix login"), issue("P-2", "Add sprint picker")];
        let refs: Vec<&Issue> = board.iter().collect();
        let mut finder = Finder::new();
        let start = Instant::now();
        assert!(finder.due(start).is_some(), "opening asks for your recent tickets");
        for c in "sprint".chars() {
            finder.handle_key(KeyEvent::from(KeyCode::Char(c)), &refs);
        }
        assert_eq!(keys(&finder.results), ["P-2"]);
        assert!(finder.due(Instant::now()).is_none(), "still typing");
        let (seq, query) = finder.due(Instant::now() + DEBOUNCE).expect("pause sends");
        assert_eq!(query, "sprint");

        // A stale reply is ignored; the current one merges in.
        finder.on_results(seq - 1, "sprin".into(), Ok(vec![issue("P-3", "old")]), &refs);
        assert_eq!(keys(&finder.results), ["P-2"]);
        // Jira matched P-8 by its description, so it ranks after the title match.
        finder.on_results(seq, query, Ok(vec![issue("P-8", "Planning notes")]), &refs);
        assert_eq!(keys(&finder.results), ["P-2", "P-8"]);
        assert!(!finder.is_busy());
    }

    #[test]
    fn arrows_move_and_enter_opens_the_highlighted_result() {
        let board = [issue("P-1", "alpha one"), issue("P-2", "alpha two")];
        let refs: Vec<&Issue> = board.iter().collect();
        let mut finder = Finder::new();
        for c in "alpha".chars() {
            finder.handle_key(KeyEvent::from(KeyCode::Char(c)), &refs);
        }
        finder.handle_key(KeyEvent::from(KeyCode::Down), &refs);
        finder.handle_key(KeyEvent::from(KeyCode::Down), &refs);
        assert_eq!(finder.highlight, 1, "stops at the last result");
        let FindOutcome::Open(issue) = finder.handle_key(KeyEvent::from(KeyCode::Enter), &refs) else {
            panic!("enter opens")
        };
        assert_eq!(issue.key, "P-2");
        assert!(matches!(finder.handle_key(KeyEvent::from(KeyCode::Esc), &refs), FindOutcome::Close));
    }
}
