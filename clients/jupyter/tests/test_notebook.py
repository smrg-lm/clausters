"""What `notebook` wires, per backend.

The two backends differ in exactly two places — where the audio runs and how
the host in the page reaches it — and both are decided here, before any widget
exists. These check that decision without a kernel, a browser or a server
process: no cell is displayed, so no widget is ever made.
"""

import pytest

from clausters import gui as gui_module
from clausters.base.main import main
from clausters.defs import Server, ServerOptions

from clausters_jupyter import session as session_module
from clausters_jupyter.session import notebook


@pytest.fixture(autouse=True)
def _fresh():
    """`notebook` installs a process-wide session; undo it between tests."""
    yield
    session_module._current = None
    session_module._bridge = None
    session_module._autowired = False
    gui_module.set_ambient_host(None)
    main.current_session = None


def _attached() -> Server:
    """A handle to a server nobody booted: TCP connects lazily, so building
    one reaches the network not at all."""
    return Server(host="127.0.0.1", port=57110)


def test_the_page_backend_has_the_engine_and_no_url():
    session = notebook("page")
    bridge = session.gui_host._osc.link
    assert bridge.has_engine
    widget_engine, widget_url = _traits(bridge)
    assert widget_engine is True
    assert widget_url == "", "there is no socket to the engine: it is in the page"


def test_the_native_backend_points_the_host_at_the_ws_port():
    session = notebook("native", server=_attached())
    bridge = session.gui_host._osc.link
    assert not bridge.has_engine
    widget_engine, widget_url = _traits(bridge)
    assert widget_engine is False, "the engine's assets are not worth sending"
    assert widget_url == "ws://127.0.0.1:57120"


def test_the_native_leg_can_be_declined():
    session = notebook("native", server=_attached(), server_url="")
    _, widget_url = _traits(session.gui_host._osc.link)
    assert widget_url == ""


def test_the_page_backend_refuses_the_native_arguments():
    with pytest.raises(ValueError, match="native"):
        notebook("page", server_url="ws://127.0.0.1:57120")


def test_audio_refuses_when_the_audio_is_not_in_the_page():
    notebook("native", server=_attached())
    with pytest.raises(RuntimeError, match="no in-page engine"):
        session_module.audio()


def test_the_native_server_is_booted_with_ws_on():
    """``ws`` is forced rather than defaulted: without it the host in the page
    has no audio leg at all, and the failure looks like a broken widget.

    The stub takes ``boot``'s **real** shape -- an instance method, reading the
    options off the handle it was built for. The one it replaced took them as
    an argument, which `Server.boot` has never accepted, so the call site was a
    TypeError this suite agreed with: nothing here booted a server, and nothing
    else runs the native example. It is the whole reason a stub must be shaped
    like the thing it stands in for.
    """
    booted = {}

    def fake_boot(self, **kwargs):
        booted["options"] = self.options
        booted["kwargs"] = kwargs
        return _attached()

    original = Server.boot
    Server.boot = fake_boot
    try:
        notebook("native", options=ServerOptions(workers=2))
    finally:
        Server.boot = original
    assert booted["options"].ws is True
    assert booted["options"].workers == 2, "the rest of the options survive"
    # This session installs itself as the ambient one; taking the default
    # server slot as well would leave it behind when the session is replaced.
    assert booted["kwargs"] == {"adopt_default": False}


def test_a_chosen_ws_port_reaches_the_page():
    """``ws=<port>`` moves the server's socket, so it has to move the URL too:
    the page opens that connection itself and cannot ask where it went."""
    original = Server.boot
    Server.boot = lambda self, **kwargs: _attached()
    try:
        session = notebook("native", options=ServerOptions(ws=9000))
    finally:
        Server.boot = original
    assert _traits(session.gui_host._osc.link)[1] == "ws://127.0.0.1:9000"


def _traits(bridge):
    """Make one widget the way a displayed window would, and read it back.

    A widget needs no kernel to exist — anywidget only opens a comm when it is
    displayed — so this is the cheapest way to see what the bridge decided.
    """
    widget = bridge._factory()
    return widget.engine, widget.server_url


def test_an_explicit_backend_replaces_the_one_the_import_wired():
    """`import clausters_jupyter` wires the default, so an explicit call runs
    second. Returning the auto-wired session would hand back a backend nobody
    asked for; it is replaced instead, which costs nothing before a cell shows
    anything."""
    session_module.notebook(_autowiring=True)
    assert session_module._bridge.has_engine

    session = notebook("native", server=_attached())
    assert session is session_module.current()
    assert not session_module._bridge.has_engine
    assert _traits(session.gui_host._osc.link)[1] == "ws://127.0.0.1:57120"


def test_a_backend_cannot_change_under_a_cell_that_is_showing_one():
    session_module.notebook(_autowiring=True)
    bridge = session_module._bridge
    bridge.widget_for(1000)             # a cell is displaying a window now
    with pytest.raises(RuntimeError, match="already running"):
        notebook("native", server=_attached())


def test_an_explicit_call_is_not_replaced_by_a_later_one():
    first = notebook("page")
    assert notebook("page") is first


def test_the_widgets_of_one_notebook_share_its_session_id():
    session = notebook("page")
    bridge = session.gui_host._osc.link
    first, second = bridge._factory(), bridge._factory()
    assert first.session == second.session == bridge.session
