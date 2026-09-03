//! Mapping between Jira and the view model (P1).
//!
//! `board_configuration` → `Board`/`Column` (columns and their status ids);
//! each issue → `Issue`, placed into the column whose `status_ids` contains the
//! issue's status. Intra-column ordering has no Jira analog — the MVP orders by
//! rank/updated (a sync policy noted in the plan).
