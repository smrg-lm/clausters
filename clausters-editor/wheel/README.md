# clausters-editor

A lightweight Python editor with an interactive session: an editor, a *post*
window with the output and a Markdown documentation viewer. It works with any
Python; installed with `pip install clausters[editor]` it opens on the
environment clausters is installed in.

```sh
pip install clausters-editor
clausters-editor
```

The session runs on the interpreter the package is installed in, until an
environment is chosen in Session -> Python Environments. The package installs
[Jedi](https://jedi.readthedocs.io/) beside it: the editor's completions,
signatures and hover help come from it.

## Requirements

The editor draws its window with the system's web view, which the package does
not carry:

- **Linux**: WebKitGTK 4.1, the `libwebkit2gtk-4.1-0` library. Desktop
  distributions from 2022 on ship it (Ubuntu 22.04, Debian 12, Fedora, Arch,
  openSUSE Tumbleweed); when it is missing, `clausters-editor` says how to
  install it on the distribution it runs on.
- **macOS**: WKWebView, part of the system.
- **Windows**: WebView2, part of Windows 10 and 11.

## License

GPL-3.0-or-later. The license of every package the application is built with
is in `licenses/THIRD-PARTY.txt`, installed with it.
