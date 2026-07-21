from __future__ import annotations

import asyncio
import re
import time

from versebridge.config import AppConfig
from versebridge.presentation.base import PlanWriteResult
from versebridge.propresenter import ProPresenterApiClient
from versebridge.slides.models import SlidePlan
from .result import TargetResult


def _normalized(value: str) -> str:
    return re.sub(r"\s+", " ", value).strip().casefold()


async def verify_import_via_api(
    config: AppConfig,
    target: TargetResult,
    expected: PlanWriteResult,
    plan: SlidePlan,
    *,
    timeout_seconds: float = 20.0,
) -> tuple[bool, str]:
    deadline = time.monotonic() + timeout_seconds
    last_problem = "ProPresenter did not refresh the presentation"
    while time.monotonic() < deadline:
        try:
            pp = config.propresenter
            async with ProPresenterApiClient(pp.host, pp.port, pp.timeout_seconds) as client:
                metadata = await client.presentation(target.uuid)
            slides = [slide for group in metadata.groups for slide in group.slides]
            if len(slides) < expected.cue_count_after:
                last_problem = f"API reports {len(slides)} slides; expected at least {expected.cue_count_after}"
            elif len(metadata.groups) < expected.group_count_after:
                last_problem = f"API reports {len(metadata.groups)} groups; expected at least {expected.group_count_after}"
            else:
                visible = [_normalized(slide.text) for slide in slides]
                missing = [item.body_text for item in plan.slides if not any(_normalized(item.body_text) in text for text in visible)]
                if not missing:
                    return True, "ProPresenter API reports every generated slide"
                last_problem = f"API did not report {len(missing)} generated slide body/bodies"
        except Exception as exc:
            last_problem = str(exc)
        await asyncio.sleep(0.5)
    return False, last_problem
