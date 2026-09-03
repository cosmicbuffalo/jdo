//! Clipboard integration (P4): copy the selected issue URL so you can open it
//! in the Jira web GUI. Will use `arboard` with an OSC-52 escape fallback for
//! SSH sessions.

/// Copy `text` to the system clipboard. Stub until P4.
pub fn copy(_text: &str) -> anyhow::Result<()> {
    anyhow::bail!("clipboard not implemented yet (P4)")
}
