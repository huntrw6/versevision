import plistlib

from versevision import autostart


ARGS = ["/home/u/.local/bin/versevision", "serve", "--host", "127.0.0.1", "--port", "8777", "--no-open"]


def test_launch_agent_runs_at_login_and_restarts():
    data = plistlib.loads(autostart.launch_agent_plist(ARGS, "/tmp/serve.log"))
    assert data["Label"] == "org.versevision.serve" and data["RunAtLoad"] is True
    assert data["ProgramArguments"] == ARGS and data["KeepAlive"] == {"SuccessfulExit": False}


def test_systemd_unit_quotes_paths_with_spaces():
    unit = autostart.systemd_unit(["/opt/my apps/versevision", "serve", "--port", "8777"])
    assert 'ExecStart="/opt/my apps/versevision" serve --port 8777' in unit
    assert "WantedBy=default.target" in unit and "Restart=on-failure" in unit


def test_windows_task_command_quotes_spaces():
    assert autostart.windows_task_command(["C:\\Program Files\\vb\\versevision.exe", "serve"]) == '"C:\\Program Files\\vb\\versevision.exe" serve'


def test_network_address_is_refused():
    import pytest
    from versevision.errors import ConfigurationError
    with pytest.raises(ConfigurationError):
        autostart.enable("0.0.0.0", 8777)
    with pytest.raises(ConfigurationError):
        autostart.enable("127.0.0.1", 0)


def test_keys_file_is_read_without_environment(tmp_path, monkeypatch):
    from versevision.credentials import provider_api_key
    keys = tmp_path / "keys.env"
    keys.write_text("# comment\nESV_API_KEY=abc123\nOTHER=\"quoted\"\n")
    monkeypatch.setenv("VERSEVISION_KEYS", str(keys))
    monkeypatch.delenv("ESV_API_KEY", raising=False)
    assert provider_api_key("ESV_API_KEY", "ESV_API_KEY") == "abc123"
    monkeypatch.setenv("ESV_API_KEY", "fromenv")
    assert provider_api_key("ESV_API_KEY", "ESV_API_KEY") == "fromenv"


def test_the_web_page_toggle_never_stops_the_page_that_is_running(tmp_path, monkeypatch):
    """Unchecking "start at login" used to bootout the very launchd job that serves the page, so the page died."""
    calls = []
    monkeypatch.setattr(autostart, "platform_name", lambda: "macos")
    monkeypatch.setattr(autostart, "_launch_agent_path", lambda: tmp_path / "org.versevision.serve.plist")
    monkeypatch.setattr(autostart, "_log_path", lambda: tmp_path / "serve.log")
    monkeypatch.setattr(autostart, "_run", lambda command, check=True: calls.append(command) or type("R", (), {"returncode": 0, "stdout": "", "stderr": ""})())
    autostart.enable("127.0.0.1", 8777, start_now=False)
    assert (tmp_path / "org.versevision.serve.plist").exists() and calls == []            # the login item is written; nothing is started or restarted
    autostart.disable(keep_running=True)
    assert not (tmp_path / "org.versevision.serve.plist").exists() and calls == []        # the login item is gone; the running page is not touched
    autostart.enable("127.0.0.1", 8777)                                                    # the command line still starts it now
    assert any(c[1] == "bootstrap" for c in calls)
    calls.clear()
    autostart.disable()                                                                    # ...and the command line disable still stops it
    assert any(c[1] == "bootout" for c in calls)
