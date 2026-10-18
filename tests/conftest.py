import pytest


@pytest.fixture(autouse=True)
def _no_pacing(monkeypatch):
    """The fake ProPresenter reacts instantly, so the real pauses would only slow the tests down."""
    from versevision.workflow import pipeline
    monkeypatch.setattr(pipeline, "MIN_GAP_SECONDS", 0.0)
