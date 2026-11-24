from __future__ import annotations

from versevision.config import AppConfig
from versevision.bibles import ProviderRegistry
from versevision.propresenter.api_client import ProPresenterApiClient
from versevision.propresenter.selection import select_unique_presentation
from versevision.references import parse_references
from versevision.slides.planner import placeholder_passages, plan_slides
from versevision.versions.resolver import resolve_versions
from .result import DryRunResult, TargetResult


async def dry_run_import(
    source: str,
    config: AppConfig,
    *,
    default_version: str | None = None,
    non_interactive: bool = False,
    target_uuid: str | None = None,
    target_name: str | None = None,
    library: str | None = None,
    validate_target: bool = True,
    no_fetch: bool = True,
    provider_registry: ProviderRegistry | None = None,
) -> DryRunResult:
    parsed = parse_references(source)
    references = resolve_versions(
        parsed.references,
        cli_default=default_version,
        saved_default=config.bible.default_version,
        non_interactive=non_interactive,
    )
    owns_registry = False
    if no_fetch:
        passages = placeholder_passages(references)
    else:
        registry = provider_registry or ProviderRegistry(config.bible)
        owns_registry = provider_registry is None
        try:
            passages = [await registry.fetch(reference) for reference in references]
        finally:
            if owns_registry:
                await registry.aclose()
    plan = plan_slides(passages, config.slides)
    target = None
    if validate_target and (target_uuid or target_name):
        pp = config.propresenter
        async with ProPresenterApiClient(pp.host, pp.port, pp.timeout_seconds) as client:
            selected = select_unique_presentation(
                await client.presentations(), uuid=target_uuid, name=target_name, library=library
            )
            metadata = await client.presentation(selected.uuid)
            target = TargetResult(
                library_name=selected.library_name or "",
                presentation_name=selected.name,
                uuid=selected.uuid,
                path=metadata.presentation_path,
            )
    return DryRunResult(
        references=references,
        issues=parsed.issues,
        slide_plan=plan,
        target=target,
        fetched=not no_fetch,
        files_changed=False,
    )
