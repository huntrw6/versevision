import json

from versevision.logging import AuditEntry, append_audit_entry, redact_fields


def test_redaction_and_audit_jsonl(tmp_path) -> None:
    assert redact_fields({"api_key": "private", "reference_count": 2}) == {
        "api_key": "[REDACTED]", "reference_count": 2,
    }
    path = tmp_path / "audit.jsonl"
    append_audit_entry(path, AuditEntry(
        application_version="0.1.0", reference_count=2, slide_count=3,
        versions=["NLT"], verification_result="dry-run",
    ))
    payload = json.loads(path.read_text(encoding="utf-8"))
    assert payload["verification_result"] == "dry-run"
    assert payload["slide_count"] == 3
