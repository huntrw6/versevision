import json

from typer.testing import CliRunner

from versebridge.cli import app


runner = CliRunner()


def test_import_dry_run_json_has_explicit_no_write_result() -> None:
    result = runner.invoke(app, [
        "import", "--text", "John 3:16-18 NIV", "--dry-run", "--no-fetch",
        "--non-interactive", "--output", "json",
    ])

    assert result.exit_code == 0, result.output
    payload = json.loads(result.output)
    assert len(payload["slide_plan"]["slides"]) == 3
    assert payload["fetched"] is False
    assert payload["files_changed"] is False


def test_production_import_requires_an_explicit_target() -> None:
    result = runner.invoke(app, ["import", "--text", "John 3:16 NIV"])
    assert result.exit_code == 2
    assert "target-presentation" in result.output
