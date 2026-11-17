#!/usr/bin/env python3
"""An editor of code over a post window: the two halves of an editor.

A ``code`` element holds a text drawn in the monospaced face, with its line
numbers and the line the caret is on, colored by its syntax (``language``
names it: keywords, names, strings and comments each in a color of the
theme, kept as you type), and edits it as a code editor does:
click to place the caret (two clicks a word, three a line, a press on a line's
number the whole line), drag or Shift and an arrow to select, Ctrl+X, Ctrl+C
and Ctrl+V, Ctrl+Z to undo a run of typing at once. **Tab indents** -- a level
at the caret, or every line selected -- and Shift+Tab takes a level off; Enter
keeps the line's indentation and adds one after a line that opens a block, as
``def`` does here. Tab stays in the editor: Ctrl+Tab moves on.

It types code as code is typed: a bracket or a quote brings the one that
closes it, which you then type over, and Ctrl+/ comments the lines selected
(again, uncomments them). Put the caret by a bracket to see its pair marked,
and select a word to see where else it stands. **Ctrl+F** opens a bar along
the bottom to find and replace: Enter finds the next match, Shift+Enter the
previous, Tab goes to the replacement, and Escape closes it; F3 goes on
finding with it closed. **Ctrl+Alt+G** goes to a line.

**Shift+Enter** evaluates the line (or the selection) and **Ctrl+Enter** the
block, the whole ``def`` or ``with`` under the caret, moving past it: the
editor runs nothing, it tells whoever owns it, and here that is this script,
which writes what it was asked to evaluate to the post window. Ctrl+. asks
for an interrupt.

**Language help** is asked of the owner too: a name begun (or Ctrl+Space)
asks for completions, which this script answers with the names the text
already holds, so typing ``ch`` offers ``chord``; walk them with Up and Down
and take one with Enter. A session would answer from what it holds, and
answer ``signature`` and ``inspected`` as well (the parameters of a call,
what a name is); this one leaves those unanswered.

Under it is a ``post`` window with what a session might have printed; drag the
divider between the two to give one more room. The editor reports when its
text stops being the one last saved (``"dirty"``), printed here, and marking
it saved again is one set.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the GUI binary

Run it cell by cell (Shift+Enter), or as a plain script --
``python clients/python/examples/panels/code.py``. No audio server is
involved, so this boots only the GUI host. Needs a display and a GPU adapter.
"""

# %%
import re
import sys

from clausters.gui import GuiHost, code, post, view

# %% [markdown]
# ## The code, and the post under it
# A window split in two: the editor takes two thirds.

# %%
TEXT = '''from clausters import Session

def chord(root, *steps):
    """The notes of a chord over root."""
    notes = [root]
    for step in steps:
        notes.append(notes[-1] + step)
    return notes

with Session() as s:
    for note in chord(60, 4, 3):
        s.synth("default", note=note)
'''

host = GuiHost().boot()
window = view(
    code(TEXT, language="python", name="code", weight=2.0),
    post(name="post", weight=1.0),
    split=True, title="code", w=720, h=520,
).open(host=host)
editor = window.widget("code")
out = window.widget("post")

# %% [markdown]
# ## What a session printed
# Written to the post window as a session would: a notice, output, a result.

# %%
out.set(info="session started\n")
out.set(out="[60, 64, 67]\n")
out.set(result="None")

# %% [markdown]
# ## Evaluated, completed, edited, and saved
# The editor asks to evaluate a line or a block; a session would run it, and
# this writes it to the post. It asks for completions, answered here from the
# names in the text. It also says when its text is no longer the one saved;
# type in it to see the event, and mark it saved with one set.

# %%
def completions(source, line, column):
    """The names the text holds that start with the word before the caret."""
    typed = re.search(r"\w*$", source.split("\n")[line - 1][:column]).group()
    names = sorted(set(re.findall(r"[A-Za-z_]\w*", source)) - {typed})
    return [{"name": n, "kind": "name", "typed": len(typed)}
            for n in names if n.startswith(typed)]


def on_code(tag, *payload):
    if tag == "evaluate":
        source, line = payload
        out.set(info=f"evaluate, from line {line}\n")
        out.set(out=source + "\n")
    elif tag == "complete":
        source, line, column, _explicit = payload
        editor.set(completions=completions(source, line, column))
    else:
        print("code", tag, *payload)


editor.on_event(on_code)
editor.set(saved=1)

# %% [markdown]
# ## Keep it open
# Close the window to end the run.

# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        window.wait()
    finally:
        print(editor.query().props["text"])
        host.stop()
else:
    print("the editor is up - window.wait(10) to hold it, host.stop() to end")
