//! The notes planner: `python/src/versevision/workflow/` (`notes_workflow.py`, `report.py`) and what it is built from.
//!
//! * [`notes::plan_notes`]: a document's text -> slides, decisions and the trace.
//! * [`points`]: the text of a sermon point as slides (numbering, case, period, line balance) and the header typo fix.
//! * [`versions`]: the version of each verse (nearest above, default, comparison pairs) and the check against a quoted wording.
//! * [`review::apply_review_choices`]: the slides after the choices made in the review.
//! * [`report::build_report`]: the plan as the review page reads it.
//!
//! Python raises where a setting cannot work (`number_style` with no `{n}`, an unknown default version); here that is a
//! [`crate::plan::PlanError`]. A verse that cannot be fetched is never an error of the run: it is a decision of tier `error`.

pub mod notes;
pub mod points;
pub mod report;
pub mod review;
pub mod versions;

pub use notes::{left_out_tier, plan_notes};
pub use points::{added_point_text, fix_label_typos, format_point, format_point_slides, LabelFix};
pub use report::build_report;
pub use review::apply_review_choices;
pub use versions::{check_version_against_quote, resolve_note_versions, ResolvedVerse};

#[cfg(test)]
mod tests;
