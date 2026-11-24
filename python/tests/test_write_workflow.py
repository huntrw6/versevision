import asyncio
from pathlib import Path

import httpx
import respx

from versevision.config import AppConfig
from versevision.presentation.base import PlanWriteResult
from versevision.slides.models import PlannedSlide, SlidePlan
from versevision.workflow.result import TargetResult
from versevision.workflow.write_workflow import verify_import_via_api


@respx.mock
def test_api_verification_requires_generated_body_and_counts(tmp_path: Path) -> None:
    respx.get("http://127.0.0.1:50001/v1/presentation/target").mock(return_value=httpx.Response(200, json={
        "presentation": {
            "id": {"uuid": "target", "name": "Verses", "index": 0},
            "groups": [{"name": "Test 1:1", "slides": [{
                "enabled": True, "text": "Test 1:1 (TST)\rSynthetic generated body.", "label": "Test 1:1 (TST)"
            }]}],
            "presentation_path": str(tmp_path / "Verses.pro"),
        }
    }))
    expected = PlanWriteResult(
        path=tmp_path / "Verses.pro", backup_path=tmp_path / "backup", cue_uuids=["cue"],
        cue_count_before=0, cue_count_after=1, group_count_before=0, group_count_after=1,
        output_sha256="a", original_sha256="b",
    )
    plan = SlidePlan(slides=[PlannedSlide(
        body_text="Synthetic generated body.", reference_text="Test 1:1", version_text="TST",
        source_passage_index=0, verse_keys=["TST.1.1"],
    )])
    target = TargetResult(
        library_name="Test", presentation_name="Verses", uuid="target", path=tmp_path / "Verses.pro"
    )

    valid, message = asyncio.run(verify_import_via_api(
        AppConfig(), target, expected, plan, timeout_seconds=0.2,
    ))

    assert valid is True
    assert "every generated slide" in message
