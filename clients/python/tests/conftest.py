"""What every test of the package shares.

The audio editor's working folders are made under the user's cache by the
GUI host that holds the editor, and a test that opens an editor without
closing it would leave one there for the next editor to sweep. The suite gets
a cache of its own instead, removed when it ends.
"""

import os
import shutil
import tempfile

import pytest

_CACHE = tempfile.mkdtemp(prefix="clausters-test-cache-")
os.environ["CLAUSTERS_CACHE_DIR"] = _CACHE


def pytest_sessionfinish(session, exitstatus):
    shutil.rmtree(_CACHE, ignore_errors=True)


@pytest.fixture(scope="module")
def gui_host(tmp_path_factory):
    """**A real GUI host, no window on screen** (``[gui] headless``): what opens
    an application the host holds. Skipped where no host is built.

    One per module: an application holds an engine on the machine's output
    for as long as its host lives, and the device CI gives it (ALSA's null
    output) is written to as fast as it is filled, so a host left for the
    whole session keeps a core busy under every later test."""
    import os

    from clausters._cli import gui_path
    from clausters.gui.host import GuiHost

    try:
        gui_path()
    except (OSError, RuntimeError):
        pytest.skip("no GUI host built")
    config = tmp_path_factory.mktemp("gui") / "config.toml"
    config.write_text("[gui]\nheadless = true\n")
    before = os.environ.get("CLAUSTERS_CONFIG")
    os.environ["CLAUSTERS_CONFIG"] = str(config)
    host = GuiHost(port=57_871).boot(adopt_ambient=False)
    if before is None:
        del os.environ["CLAUSTERS_CONFIG"]
    else:
        os.environ["CLAUSTERS_CONFIG"] = before
    yield host
    host.stop()


def opened(host, editor):
    """``editor``, its window open on ``host``; every test closes its own, and a
    window left open goes with the host."""
    editor.open(host)
    return editor
