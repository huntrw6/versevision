//! `workflow/import_workflow.py` and `workflow/write_workflow.py`: the older `import` command's dry run and its verification through the
//! typed API client. (The swap pipeline has its own verification, `pipeline::verify_via_api`.)

use serde::Serialize;
use versevision_core::bibles::BiblePassages;
use versevision_core::config::AppConfig;
use versevision_core::notes::trace::NoTrace;
use versevision_core::plan::SlidePlan;
use versevision_core::references::models::{ParseIssue, PassageReference};
use versevision_core::references::parse_references_with;
use versevision_core::references::pytext::casefold;
use versevision_core::slides::{placeholder_passages, plan_slides};
use versevision_core::versions::resolve_versions;

use crate::api_client::ProPresenterApiClient;
use crate::clock::Clock;
use crate::errors::{Kind, VvError, VvResult};
use crate::selection::select_unique_presentation;
use crate::writer::PlanWriteResult;

/// `TargetResult`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TargetResult {
    pub library_name: String,
    pub presentation_name: String,
    pub uuid: String,
    pub path: String,
}

/// `DryRunResult`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DryRunResult {
    pub references: Vec<PassageReference>,
    pub issues: Vec<ParseIssue>,
    pub slide_plan: Option<SlidePlan>,
    pub target: Option<TargetResult>,
    pub fetched: bool,
    pub files_changed: bool,
}

/// `dry_run_import`: parse, resolve versions, plan slides; with a target also find it in ProPresenter. Changes nothing.
#[allow(clippy::too_many_arguments)]
pub async fn dry_run_import<B: BiblePassages + ?Sized>(
    source: &str,
    config: &AppConfig,
    default_version: Option<&str>,
    non_interactive: bool,
    target_uuid: Option<&str>,
    target_name: Option<&str>,
    library: Option<&str>,
    validate_target: bool,
    no_fetch: bool,
    bible: Option<&B>,
) -> VvResult<DryRunResult> {
    let parsed = parse_references_with(source, None)
        .map_err(|e| VvError::new(Kind::Other("ValueError"), e.to_string()))?;
    let references = resolve_versions(
        &parsed.references,
        default_version,
        config.bible.default_version.as_deref(),
        non_interactive,
    )
    .map_err(|e| VvError::configuration(e.to_string()))?;
    let passages = if no_fetch {
        placeholder_passages(&references)
    } else {
        let bible = bible.ok_or_else(|| VvError::general("no Bible text source was given"))?;
        let mut out = Vec::new();
        for reference in &references {
            out.push(
                bible
                    .fetch(reference, &mut NoTrace)
                    .await
                    .map_err(|e| VvError::new(Kind::BibleProvider, e.to_string()))?,
            );
        }
        out
    };
    let plan = plan_slides(&passages, &config.slides);
    let mut target = None;
    if validate_target
        && (target_uuid.is_some_and(|u| !u.is_empty())
            || target_name.is_some_and(|n| !n.is_empty()))
    {
        let pp = &config.propresenter;
        let client = ProPresenterApiClient::new(&pp.host, pp.port, pp.timeout_seconds);
        let selected = select_unique_presentation(
            &client.presentations()?,
            target_uuid,
            target_name,
            library,
        )?;
        let metadata = client.presentation(&selected.uuid)?;
        target = Some(TargetResult {
            library_name: selected.library_name.clone().unwrap_or_default(),
            presentation_name: selected.name.clone(),
            uuid: selected.uuid.clone(),
            path: metadata.presentation_path.to_string_lossy().into_owned(),
        });
    }
    Ok(DryRunResult {
        references,
        issues: parsed.issues,
        slide_plan: Some(plan),
        target,
        fetched: !no_fetch,
        files_changed: false,
    })
}

fn normalized(value: &str) -> String {
    casefold(&versevision_core::pystr::split_ws(value).join(" "))
}

/// `verify_import_via_api`: ProPresenter reports every generated slide (polled every half second up to `timeout_seconds`).
pub fn verify_import_via_api(
    clock: &dyn Clock,
    config: &AppConfig,
    target: &TargetResult,
    expected: &PlanWriteResult,
    plan: &SlidePlan,
    timeout_seconds: f64,
) -> (bool, String) {
    let deadline = clock.monotonic() + timeout_seconds;
    let mut last_problem = "ProPresenter did not refresh the presentation".to_string();
    while clock.monotonic() < deadline {
        let pp = &config.propresenter;
        let client = ProPresenterApiClient::new(&pp.host, pp.port, pp.timeout_seconds);
        match client.presentation(&target.uuid) {
            Ok(metadata) => {
                let slides: Vec<&crate::api_client::ApiSlide> = metadata
                    .groups
                    .iter()
                    .flat_map(|g| g.slides.iter())
                    .collect();
                if slides.len() < expected.cue_count_after {
                    last_problem = format!(
                        "API reports {} slides; expected at least {}",
                        slides.len(),
                        expected.cue_count_after
                    );
                } else if metadata.groups.len() < expected.group_count_after {
                    last_problem = format!(
                        "API reports {} groups; expected at least {}",
                        metadata.groups.len(),
                        expected.group_count_after
                    );
                } else {
                    let visible: Vec<String> = slides.iter().map(|s| normalized(&s.text)).collect();
                    let missing = plan
                        .slides
                        .iter()
                        .filter(|item| {
                            let want = normalized(&item.body_text);
                            !visible.iter().any(|t| t.contains(&want))
                        })
                        .count();
                    if missing == 0 {
                        return (
                            true,
                            "ProPresenter API reports every generated slide".to_string(),
                        );
                    }
                    last_problem =
                        format!("API did not report {missing} generated slide body/bodies");
                }
            }
            Err(exc) => last_problem = exc.message,
        }
        clock.sleep(0.5);
    }
    (false, last_problem)
}
