"""The window-returning types display as their cell's canvas.

Run against a real IPython shell, because the thing that went wrong here is
invisible without one: a formatter can be registered, be called, and still
produce a result with no mimetypes, which the front end renders as nothing at
all -- no error, no output.
"""

import pytest

pytest.importorskip("IPython")

WIDGET_VIEW = "application/vnd.jupyter.widget-view+json"


@pytest.fixture(scope="module")
def shell():
    from IPython.testing.globalipapp import start_ipython

    ip = start_ipython()
    import clausters_jupyter

    clausters_jupyter.notebook()
    return ip


def _plot(n=5000):
    from clausters import plot

    return plot([i / n for i in range(n)], n=n)


def test_a_plot_window_formats_as_a_widget_view(shell):
    data, _ = shell.display_formatter.format(_plot())
    assert WIDGET_VIEW in data, (
        "the front end renders this key and nothing else here; without it the "
        "cell shows nothing and says nothing")
    assert data[WIDGET_VIEW]["model_id"]


def test_displaying_the_same_window_twice_is_the_same_canvas(shell):
    win = _plot()
    first, _ = shell.display_formatter.format(win)
    second, _ = shell.display_formatter.format(win)
    assert first[WIDGET_VIEW]["model_id"] == second[WIDGET_VIEW]["model_id"]


def test_two_windows_get_two_canvases(shell):
    a, _ = shell.display_formatter.format(_plot())
    b, _ = shell.display_formatter.format(_plot())
    assert a[WIDGET_VIEW]["model_id"] != b[WIDGET_VIEW]["model_id"]


def test_unregistering_restores_the_plain_repr(shell):
    from clausters_jupyter import formatters

    win = _plot()
    formatters.unregister()
    try:
        data, _ = shell.display_formatter.format(win)
        assert WIDGET_VIEW not in data
    finally:
        formatters.register(_bridge())


def _bridge():
    import clausters_jupyter

    return clausters_jupyter.current().gui_host._osc.link
