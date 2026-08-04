# clausters-jupyter

The Clausters GUI in a Jupyter notebook cell, driven by the ordinary
`clausters` Python client.

The verbs draw where you are looking:

```python
import clausters_jupyter          # once, at the top

plot(sine(440) * 0.5)
scope(bus=0)
```

Two backends. `page` (the default) runs both the GUI host and the audio engine
in the cell as wasm, so it works with a remote kernel — JupyterHub, Colab, a
remote VS Code — and sounds where you are. `native` boots a local
`clausters` server with its full capability (Faust JIT, shared memory, mmap
bulk) and draws its GUI in the cell; it is local-only, since the audio comes
out of the kernel's machine.

See the notebook chapter of the Clausters Python client's book, and
[`examples/`](examples/) for notebooks to open — their README has the two
commands that install and run them.
