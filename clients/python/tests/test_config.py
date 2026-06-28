"""Config-file layering tests.

The shared TOML config has two layers, the project file overriding the user file
field by field, and an explicit argument winning over both. These tests mirror
the Rust `config` tests so the two implementations agree on the precedence.
"""

from pathlib import Path

import pytest

import clausters.config as config
from clausters.defs.server import Server, ServerOptions


@pytest.fixture(autouse=True)
def _reset_config_cache():
    """Clear the module-level config cache around each test so a test's
    temporary config never leaks into another (or into other test files)."""
    config._cache = None
    yield
    config._cache = None


def _write(dir_path, name, text):
    p = Path(dir_path) / name
    p.write_text(text)
    return p


def test_project_overrides_user_field_by_field(tmp_path, monkeypatch):
    user_dir = tmp_path / "user"
    user_dir.mkdir()
    proj_dir = tmp_path / "proj"
    proj_dir.mkdir()
    _write(user_dir, "config.toml", "[server]\nsample_rate = 48000\naudio_buses = 128\n")
    _write(proj_dir, "clausters.toml", "[server]\nsample_rate = 96000\n")
    monkeypatch.setenv("CLAUSTERS_CONFIG", str(user_dir / "config.toml"))

    cfg = config.load_config_from(proj_dir)
    # The project's value wins where set; the user's is kept where absent.
    assert cfg["server"]["sample_rate"] == 96000
    assert cfg["server"]["audio_buses"] == 128


def test_missing_config_is_empty(tmp_path, monkeypatch):
    monkeypatch.setenv("CLAUSTERS_CONFIG", str(tmp_path / "does-not-exist.toml"))
    monkeypatch.delenv("XDG_CONFIG_HOME", raising=False)
    assert config.load_config_from(tmp_path) == {}


def test_server_defaults_come_from_config(tmp_path, monkeypatch):
    _write(
        tmp_path,
        "config.toml",
        '[server]\nsample_rate = 44100\naudio_buses = 64\n'
        '[client]\nhost = "10.0.0.5"\nport = 7000\nlatency = 0.25\n',
    )
    monkeypatch.setenv("CLAUSTERS_CONFIG", str(tmp_path / "config.toml"))
    monkeypatch.delenv("XDG_CONFIG_HOME", raising=False)
    config.load_config(refresh=True)

    opts = ServerOptions()
    assert opts.sample_rate == 44100
    assert opts.audio_buses == 64

    # A bare interface object so the Server opens no socket.
    server = Server(interface=object())
    assert server.target.host == "10.0.0.5"
    assert server.target.port == 7000
    assert server.latency == 0.25

    # An explicit argument still wins over the config.
    assert Server(host="9.9.9.9", interface=object()).target.host == "9.9.9.9"
