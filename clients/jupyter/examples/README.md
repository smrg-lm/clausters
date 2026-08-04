# Notebook examples

These are notebooks, and what the repository carries is the **`.py`** half of
each one: a plain script of `# %%` cells. A `.ipynb` records its own output and
execution counts, so it changes when you merely open it — the `.py` is what
diffs, reviews and stays the same file after a run. The notebook beside it is
generated (and git-ignored); jupytext keeps the two in step in both directions,
so you can edit either.

## Install

```sh
pip install clausters-jupyter jupyterlab jupytext
```

`clausters-jupyter` brings the client, the audio server and the browser half
with it — there is nothing native to build and no server to start by hand.

## Run

Generate the notebooks once, then open JupyterLab here:

```sh
jupytext --sync nb_*.py
jupyter lab
```

Both halves are the same notebook from then on: edit the `.py` in your editor
or the `.ipynb` in Jupyter, and saving either updates the other. After pulling
new changes, `jupytext --sync nb_*.py` again.

Opening the `.py` directly also works — in JupyterLab, right-click it and
choose *Open With → Notebook*.

## What they need

**A browser with WebGPU and Web Audio** (Chrome, or Firefox with
`dom.webgpu.enabled`): the GUI host and the audio engine both run in the tab,
so the kernel may be anywhere — including JupyterHub, Colab or a remote VS
Code. A browser starts no audio until the page is clicked.

One of them uses the **native** backend instead, which boots a real `clausters`
server on the kernel's machine for its full capability (Faust, shared memory,
the machine's audio devices). That one is local-only, and says so: the sound
comes out of the kernel's speakers, and the page opens its own WebSocket to
that server from the browser.

**Each example documents itself** — the docstring in its first cell says what it
shows, what it needs and how to run it. Start with `nb_verbs.py`, the shortest:
one import, and the ordinary verbs draw where you are looking.
