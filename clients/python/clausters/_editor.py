"""What clausters-editor calls into this package for, besides its book.

`interrupt` is the package's ``clausters_editor.interrupt`` entry point. The
editor calls it after its interrupt key (Ctrl+.) has stopped the code the
session was running, and only when this package is already imported there, so
the key that stops the code stops the sound too.
"""


def interrupt():
    """Panic every session not closed yet, and the default session -- each
    one's `clausters.base.environment.Environment.panic`. A session that fails
    (its server gone, say) does not keep the others sounding: each is tried,
    and the first failure is raised after all of them."""
    from .base.main import main
    from .session import _OPEN

    failed = None
    for environment in [*list(_OPEN), main]:
        try:
            environment.panic()
        except Exception as e:  # noqa: BLE001 -- reported once the rest are stopped
            failed = failed or e
    if failed is not None:
        raise failed
