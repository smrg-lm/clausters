"""The areas, and what arming one means for the others — `clausters.log`.

The web client's twin is `clients/web/tests/log.test.ts`. The module is the same
one in two languages: same three areas, same `watch`/`unwatch`, same environment
spelling, and the same rule that an ancestor answers for its children without
printing a line twice.
"""

import io
import logging

import pytest

from clausters.log import unwatch, watch


@pytest.fixture(autouse=True)
def _quiet():
    """Leave the process as it was found, whatever a case armed."""
    unwatch()
    yield
    unwatch()


def _area(name: str) -> logging.Logger:
    return logging.getLogger(f"clausters.{name}")


def test_an_area_is_silent_until_it_is_watched():
    printed = io.StringIO()
    _area("gui").debug("nobody asked")
    assert printed.getvalue() == ""


def test_watching_an_area_prints_it_and_leaves_the_others_quiet():
    printed = io.StringIO()
    watch("gui", printed)
    _area("gui").debug("a host command")
    _area("server").debug("not this one")
    assert printed.getvalue() == "clausters.gui: a host command\n"


def test_an_ancestor_answers_for_its_children_and_arming_both_prints_once():
    # A record propagates up, so a handler at each level prints every line of the
    # narrower area twice -- which is what `CLAUSTERS_LOG=gui,gui.editing` did,
    # and a doubled log is one a reader stops trusting.
    printed = io.StringIO()
    watch("gui", printed)
    watch("gui.editing", printed)
    _area("gui.editing").debug("a joint")
    assert printed.getvalue() == "clausters.gui.editing: a joint\n"


def test_watching_the_root_catches_every_area():
    printed = io.StringIO()
    watch("", printed)
    _area("server").debug("one")
    _area("gui.editing").debug("two")
    assert printed.getvalue().count("\n") == 2
