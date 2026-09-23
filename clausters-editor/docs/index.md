# clausters-editor

A Python editor with a **persistent interactive session**: the code you
evaluate stays alive in the session, and its output appears in the *post* window.

## Shortcuts

| Shortcut | Action |
|---|---|
| `Shift+Enter` | Evaluates the selection or the current line |
| `Ctrl+Enter` | Evaluates the selection or the top-level block under the cursor (a whole `def`, `for`, `class`) |
| `Ctrl+.` | Interrupts the execution (`KeyboardInterrupt`) |
| `Ctrl+D` | Help on the name under the cursor |
| `Ctrl+Shift+Space` | Shows the parameters of the call the cursor is in (`Esc` closes it) |
| `Ctrl+N` / `Ctrl+O` / `Ctrl+S` | New / open / save (`Ctrl+Shift+S`: save as) |
| `Ctrl+W` | Close tab |

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

Hovering over a name already defined in the session (a variable, a function, a
method) shows its signature and its documentation; typing `(` or `,` inside a
call shows its parameters, with the current one highlighted.

The documentation is hidden with the x on its bar (or View → Documentation) and
comes back with `F1` or `Ctrl+D`.

The window holds up to four panels in two columns: the editor, the documentation,
the post window and, with View → Split Editor, a second editor beside the first
(View → Move Tab to Other Editor moves the current file across). Any panel can be
dragged by the handle at the start of its bar to another place; the places it can
go to are shown while dragging. A moved panel takes the size of the place it goes
to, and keeps its own height when it enters a column that had one panel.

A tab with unsaved changes shows a dot; closing it, or closing the window, asks
before those changes are lost. Undoing back to the saved text clears the dot.

More: [how the session works](session.md) · [help on `print`](python/print.md)
