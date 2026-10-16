import plistlib

from versebridge import autostart


ARGS = ["/home/u/.local/bin/versebridge", "serve", "--host", "127.0.0.1", "--port", "8777", "--no-open"]


def test_launch_agent_runs_at_login_and_restarts():
    data = plistlib.loads(autostart.launch_agent_plist(ARGS, "/tmp/serve.log"))
    assert data["Label"] == "org.versebridge.serve" and data["RunAtLoad"] is True
    assert data["ProgramArguments"] == ARGS and data["KeepAlive"] == {"SuccessfulExit": False}


def test_systemd_unit_quotes_paths_with_spaces():
    unit = autostart.systemd_unit(["/opt/my apps/versebridge", "serve", "--port", "8777"])
    assert 'ExecStart="/opt/my apps/versebridge" serve --port 8777' in unit
    assert "WantedBy=default.target" in unit and "Restart=on-failure" in unit


def test_windows_task_command_quotes_spaces():
    assert autostart.windows_task_command(["C:\\Program Files\\vb\\versebridge.exe", "serve"]) == '"C:\\Program Files\\vb\\versebridge.exe" serve'


def test_network_address_is_refused():
    import pytest
    from versebridge.errors import ConfigurationError
    with pytest.raises(ConfigurationError):
        autostart.enable("0.0.0.0", 8777)
    with pytest.raises(ConfigurationError):
        autostart.enable("127.0.0.1", 0)


def test_keys_file_is_read_without_environment(tmp_path, monkeypatch):
    from versebridge.credentials import provider_api_key
    keys = tmp_path / "keys.env"
    keys.write_text("# comment\nESV_API_KEY=abc123\nOTHER=\"quoted\"\n")
    monkeypatch.setenv("VERSEBRIDGE_KEYS", str(keys))
    monkeypatch.delenv("ESV_API_KEY", raising=False)
    assert provider_api_key("ESV_API_KEY", "ESV_API_KEY") == "abc123"
    monkeypatch.setenv("ESV_API_KEY", "fromenv")
    assert provider_api_key("ESV_API_KEY", "ESV_API_KEY") == "fromenv"
