//! Bible version names and the quote check. Not written yet (S2): the entry point for the line-level oracle says so.

use serde_json::Value;

use crate::plan::PlanError;

/// One line of `oracle/versions.jsonl` (`kind` = `version`, `version_available` or `quote_score`): the `out` it should give.
/// S2 gives this real types; until then the oracle runner calls it and counts the error as "not passing yet".
pub fn version_line(_row: &Value) -> Result<Value, PlanError> {
    Err(PlanError::NotImplemented(
        "version registry and quote check arrive in S2",
    ))
}
