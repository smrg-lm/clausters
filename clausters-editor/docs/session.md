# The Python session

The session is a single Python process that starts with the editor: the
active environment's, else `python3` -- or, when the editor was installed with
`pip`, the Python of the environment it was installed in. Every
evaluation shares the same namespace, as in a console.

- If the last statement is an expression, its value is shown after `->`.
- Errors show the line number **in the file**, not in the fragment.
- `input()` is not available: standard input is used by the editor.
- **Restart** creates a new process and clears every variable.
- The **startup code** (File → Preferences) runs every time the session starts,
  for instance for the usual `import`s, and is shown in the post window.

To use another interpreter (for instance, one in a virtual environment), start
the editor with the `CLAUSTERS_EDITOR_PYTHON` variable:

```
CLAUSTERS_EDITOR_PYTHON=~/project/.venv/bin/python npm run tauri dev
```

## Interrupting

An infinite loop is stopped with `Ctrl+.`:

```python
import time
while True:
    time.sleep(0.1)
```

The key also stops what the session's packages are still doing on their own:
after interrupting the code, the editor calls the interrupt hook of every
package imported in the session that registers one. With clausters imported,
that is a panic: every session stops sounding -- its clocks, its timelines, its
server -- and the responders not marked `permanent` are freed.

A package registers its hook as an entry point in the
`clausters_editor.interrupt` group, a function called with no arguments:

```toml
[project.entry-points."clausters_editor.interrupt"]
mypackage = "mypackage._editor:interrupt"
```

The hook is called only when the package is already imported, so the key never
imports anything; one that fails is reported in the post window and does not
stop the others.

[Back to the start](index.md#shortcuts)
