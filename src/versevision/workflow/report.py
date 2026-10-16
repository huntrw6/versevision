"""Turn a plan into a plain structure for people, scripts and the web page."""
from __future__ import annotations

from typing import Any

from versevision.workflow.notes_workflow import NotesImportResult


def build_report(plan: NotesImportResult, *, source_name: str = "") -> dict[str, Any]:
    slides = []
    for number, slide in enumerate(plan.slide_plan.slides, 1):
        if slide.kind == "point":
            slides.append({"n": number, "kind": "point", "title": slide.body_text.split("\n")[0], "text": slide.body_text})
        else:
            title = f"{slide.reference_text} ({slide.version_text})" if slide.version_text else slide.reference_text
            slides.append({"n": number, "kind": "verse", "title": title, "text": slide.body_text})

    def rows(entries, status):
        return [{"line": e.line, "text": e.text, "detail": e.problem, "status": status} for e in entries]

    flagged = rows(plan.failed, "not_added") + rows(plan.review, "needs_review")
    flagged.sort(key=lambda row: row["line"])
    return {
        "source": source_name,
        "summary": {
            "slides": len(slides), "verse_slides": sum(1 for s in slides if s["kind"] == "verse"),
            "point_slides": sum(1 for s in slides if s["kind"] == "point"),
            "flagged": len(flagged), "not_added": len(plan.failed), "needs_review": len(plan.review),
            "left_out": len(plan.skipped), "adjustments": len(plan.notices),
        },
        "slides": slides,
        "flagged": flagged,
        "adjustments": rows(plan.notices, "adjusted"),
        "left_out": rows(plan.skipped, "left_out"),
        "clean": not flagged,
    }
