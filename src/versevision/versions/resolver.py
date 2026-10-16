from __future__ import annotations

from versevision.errors import ConfigurationError
from versevision.references.models import PassageReference
from .registry import canonical_version


def resolve_versions(
    references: list[PassageReference], *, cli_default: str | None = None, saved_default: str | None = None,
    non_interactive: bool = False,
) -> list[PassageReference]:
    default_raw = cli_default or saved_default
    default = canonical_version(default_raw) if default_raw else None
    if default_raw and not default:
        raise ConfigurationError(f"Unknown Bible version: {default_raw}")
    resolved: list[PassageReference] = []
    missing: list[PassageReference] = []
    for reference in references:
        if reference.version:
            resolved.append(reference)
        elif default:
            resolved.append(reference.model_copy(update={"version": default, "warnings": [*reference.warnings, "default version applied"]}))
        else:
            missing.append(reference)
    if missing:
        mode = "non-interactive mode" if non_interactive else "this workflow"
        raise ConfigurationError(
            f"{len(missing)} reference(s) do not specify a Bible version and no default is configured for {mode}"
        )
    return resolved

