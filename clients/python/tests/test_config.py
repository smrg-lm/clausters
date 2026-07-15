"""Config-file layering tests.

The shared TOML config has two layers, the project file overriding the user file
field by field, and an explicit argument winning over both. These tests mirror
the Rust `config` tests so the two implementations agree on the precedence.
"""

from pathlib import Path

import pytest

import clausters.config as config
from clausters.defs.server import DEFAULT_MAX_NODES, Server, ServerInfo, ServerOptions


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


def test_server_options_boot_config(tmp_path, monkeypatch):
    """The boot-time channel and pool options come from ``[server]`` and are
    emitted as CLI flags, so a launched server matches the object."""
    _write(
        tmp_path,
        "config.toml",
        "[server]\noutputs = 1\ninputs = 2\nmax_nodes = 2048\n"
        "max_buffers = 64\nmax_graph_children = 32\nmax_ugen_inputs = 16\n",
    )
    monkeypatch.setenv("CLAUSTERS_CONFIG", str(tmp_path / "config.toml"))
    monkeypatch.delenv("XDG_CONFIG_HOME", raising=False)
    config.load_config(refresh=True)

    opts = ServerOptions()
    assert (opts.outputs, opts.inputs) == (1, 2)
    assert opts.max_nodes == 2048
    assert opts.max_ugen_inputs == 16

    args = opts.args()
    for flag, value in [
        ("--outputs", "1"),
        ("--inputs", "2"),
        ("--max-nodes", "2048"),
        ("--max-buffers", "64"),
        ("--max-graph-children", "32"),
        ("--max-ugen-inputs", "16"),
    ]:
        assert value == args[args.index(flag) + 1], flag


def test_server_options_outputs_flag_omitted_by_default(monkeypatch):
    """With no ``outputs`` set the server follows the device, so no
    ``--outputs`` flag is emitted; ``--inputs`` still defaults to 0."""
    monkeypatch.delenv("CLAUSTERS_CONFIG", raising=False)
    monkeypatch.delenv("XDG_CONFIG_HOME", raising=False)
    config.load_config(refresh=True)
    opts = ServerOptions()
    assert opts.outputs is None
    args = opts.args()
    assert "--outputs" not in args
    assert args[args.index("--inputs") + 1] == "0"


def test_server_info_capacity_fields_default_for_old_servers():
    """A pre-S7 server reports only the six original fields; the appended
    capacity fields fall back to the defaults on the dataclass."""
    info = ServerInfo(
        audio_buses=128,
        control_buses=1024,
        channels=2,
        block_size=64,
        nominal_sample_rate=48000.0,
        actual_sample_rate=48000.0,
    )
    assert info.input_channels == 0
    assert info.max_nodes == DEFAULT_MAX_NODES
    assert info.max_ugen_inputs == 32
