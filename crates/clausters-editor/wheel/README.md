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

The editor draws its window with the GPU, through the system's Vulkan, Metal
or Direct3D driver (OpenGL where none of them is there), and carries
everything else it draws with: its faces, its icons and the engraver of the
score editor. On Linux it also needs the system's ALSA library
(`libasound2`), which desktop distributions install.

## License

GPL-3.0-or-later. The license of every package the application is built with
is in `share/clausters-editor/licenses/THIRD-PARTY.txt`, installed with it.
