# pyright: basic
"""The format's own numbers, and how a reader keeps what it does not know.

Private: the names are `clausters.document`'s.
"""

#: The version an unedited document carries.
#:
#: One rather than zero, because zero is what an edit means by *unstated* when
#: it names the state it was made against -- the same reservation the GUI host's
#: acknowledgement uses.
FIRST_VERSION = 1

#: The session format this build writes -- the crate's ``session::FORMAT``.
#:
#: It moves when a reader that does not know the new shape would read a file
#: **wrongly**, never for an added field, which an older reader ignores and a
#: newer one defaults. **2** added a source whose samples are spans of other
#: sources, which is a tagged variant an older reader must fail on rather than
#: read as something else. **3** measures the multitrack in seconds where 2
#: measured it in beats, and states a tempo in beats per second; an older file is
#: migrated on reading (`SessionFile.read`). **4** added a source that is a
#: sequence of events held in the file itself, a tagged variant for the reason 2
#: gives. **5** names a sequence's curves as the multitrack names
#: a track's -- `automation` where it said `lanes` and `expression` -- which an
#: older reader would drop without a word.
#:
#: A literal rather than `clausters._native.session_format`, deliberately: a
#: `SessionFile` is plain data and nothing in it opens the library, so reading
#: the number from it would make writing a session need a native load it has
#: never needed. What keeps the two in step instead is
#: a **test** that asks the crate and compares -- which is the check that did not
#: exist when this said 1 and the crate had moved to 2.
SESSION_FORMAT = 5

def _unknown_fields(written: dict, *known: str) -> dict:
    """Whatever a newer writer wrote and this build has no field for.

    Carried, never read. A reader that dropped it would lose a multitrack the next
    version of this client wrote, which for a format with two writers in two
    languages is not hypothetical.
    """
    return {key: value for key, value in written.items() if key not in known}

