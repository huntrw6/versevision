//! Scripture references. Not written yet (S2): the entry point for the line-level oracle says so.

use serde_json::{Map, Value};

use crate::plan::PlanError;

/// `parse_references(line, aliases)` as `oracle/references.jsonl` records it (`{"references": [...], "issues": [...]}`).
/// S2 gives this real types; until then the oracle runner calls it and counts the error as "not passing yet".
pub fn parse_references(
    _line: &str,
    _aliases: Option<&Map<String, Value>>,
) -> Result<Value, PlanError> {
    Err(PlanError::NotImplemented("reference parsing arrives in S2"))
}
