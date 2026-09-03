//! Atlassian Document Format (ADF) support (P2/P3).
//!
//! `description` and `acceptance_criteria` are ADF (a rich JSON tree). Read:
//! `adf_to_lines` flattens paragraphs, headings, lists, code, and basic marks
//! into styled terminal lines. Write: `text_to_adf` converts edited plain
//! text / light markdown into a minimal ADF document (lossy in the MVP).
