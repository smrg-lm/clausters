# clausters-editor

A Python editor with a **persistent interactive session**: the code you
evaluate stays alive in the session, and its output appears in the *post* window.

## Shortcuts

| Shortcut | Action |
|---|---|
| `Shift+Enter` | Evaluates the selection or the current line |
| `Ctrl+Enter` | Evaluates the selection or the top-level block under the cursor (a whole `def`, `for`, `class`), then moves to the line after it |
| `Ctrl+.` | Interrupts the execution (`KeyboardInterrupt`), then stops what the imported packages are playing ([Interrupting](session.md#interrupting)) |
| `Ctrl+D` | Help on the name under the cursor |
| `Ctrl+Shift+Space` | Shows the parameters of the call the cursor is in (`Esc` closes it) |
| `Ctrl+Space` | Completes the name being typed: from the file, and from what the session holds |
| `Ctrl+F` | In the documentation: finds text in the page (`Enter` / `Shift+Enter`: next / previous) |
| `Ctrl+Shift+F` | Searches every document for a text (the selection, if there is one); the find bar's *Page* / *All documents* switches between the two |
| `Ctrl+N` / `Ctrl+O` / `Ctrl+S` | New / open / save (`Ctrl+Shift+S`: save as) |
| `Ctrl+W` | Close tab |
| `Ctrl+Shift+P` | Clears the post window |
| `Tab` / `Shift+Tab` | `Tab` inserts one indentation level at the cursor, or indents the selected lines; `Shift+Tab` unindents. What a level is -- a tab, 2 or 4 spaces (the default) -- is in File → Preferences |

File → Open Recent reopens the files last opened or saved, and File → Preferences
can open the last session's files at startup.

## Try it

Each code block in the documentation has a copy button in its corner. Copy
this one, paste it in the editor and evaluate it with `Ctrl+Enter`:

```python
def greeting(name):
    return f"Hello, {name}"

greeting("world")
```

After evaluating it, `greeting` stays defined in the session: type
`greeting("again")` in the editor and press `Shift+Enter`.

Session → Evaluate Contiguous Lines as a Block makes `Ctrl+Enter` take the lines
around the cursor that no blank line separates, as one block:

```python
a = 1
b = 2
print(a + b)
```

A file with **cells** -- lines starting with `# %%`, as VS Code and Jupyter write
them -- is evaluated a cell at a time: `Ctrl+Enter` evaluates the cell the cursor
is in and moves to the next one.

```python
# %% Setup
x = 10

# %% Use it
x * 2
```

Hovering over a name already defined in the session (a variable, a function, a
method) shows its signature and its documentation; typing `(` or `,` inside a
call shows its parameters, with the current one highlighted. While typing, the
suggestions include what the session holds -- the names a `from module import *`
brought in, an object's attributes after its `.`, what a call returns
(`Server().boot(` shows `boot`'s parameters). All of it comes from
[Jedi](https://jedi.readthedocs.io/), which reads the file and the session together:
it is installed with the editor's package and in every environment the editor
creates; in another environment, `pip install jedi` turns it on.

When `Ctrl+D` finds no `.md` with the name it looks for, it shows the `pydoc`
help taken from the live session, so it also works with your own functions and
imported modules.

The documentation is hidden with the x on its bar (or View → Documentation) and
comes back with `F1` or `Ctrl+D`. `F1` and the house button open its start page,
which lists the chapters of every book the editor has: its own, and those of the
packages installed in the session's environment. The bar says which book and
chapter is on screen; the book's name there opens its whole table of contents.

The window holds up to four panels in two columns: the editor, the documentation,
the post window and, with View → Split Editor, a second editor beside the first
(View → Move Tab to Other Editor moves the current file across). Any panel can be
dragged by the handle at the start of its bar to another place; the places it can
go to are shown while dragging. A moved panel takes the size of the place it goes
to, and keeps its own height when it enters a column that had one panel.

A tab with unsaved changes shows a dot; closing it, or closing the window, asks
before those changes are lost. Undoing back to the saved text clears the dot.

More: [how the session works](session.md)
