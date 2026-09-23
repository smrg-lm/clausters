# The Python session

The session is a single `python3` process that starts with the editor. Every
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

[Back to the start](index.md#shortcuts)
