"""Native plugins on a native server: the catalog of what is installed, one
plugin as an object, and the def that instantiates it.

The information has three layers. The **catalog** (`PluginCatalog`, from
`Server.plugins`) is what the server's discovery found. A **plugin**
(`Plugin`) is one entry of it: what it is called, its ports and parameters,
what it supports and what it declares itself to be -- with its identity
inside it, so a program names a plugin by what it is called and never types
its native id. A **def** (`PluginDef`) is the decision about one plugin:
which buses its ports use and the state it starts from, which is what the
server stores and what a `Synth` instantiates.

The vocabulary, the search and the port rule are the shared core's
(`clausters_core::plugin_catalog`), so every client reads a catalog the same
way and a def that does not fit its plugin is refused before it is sent.
"""
import json

from .. import _native
from ._wire import resolve as _resolve, send_def


class Plugin:
    """**One installed native plugin**, read-only, as the server's discovery
    reports it.

    Its identity is `format`, `path` (the binary, on the server's machine) and
    `id` (the plugin's native id within the binary); `key` is the three in one
    string. `name`, `vendor`, `version`, `description` and `url` are what it
    says about itself.

    **What it is** is read from what it declares in either format's vocabulary
    (`features`, as declared): `kind` is one of ``"instrument"``,
    ``"effect"``, ``"note_effect"``, ``"analyzer"`` or ``"other"``, `tags` the
    common words for what it does (``"synth"``, ``"reverb"``, ``"eq"``, ...)
    and `channels` its layout (``"mono"``, ``"stereo"``, ``"surround"``,
    ``"ambisonic"``). Data a plugin loads from outside (an impulse response,
    samples) has no standard declaration; it lives in the plugin's state.

    `inputs` and `outputs` are its audio ports, `note_inputs` and
    `note_outputs` its note ports and `parameters` its parameters, each a
    dict as discovery reports it. `controls` maps the names a node's controls
    answer to -- ``in``/``out`` for the main audio ports, ``in_<name>`` and
    ``out_<name>`` for the others, a parameter's name -- to the native-id
    names (``param:<id>``, ``in:<id>``, ``out:<id>``), which always work too.
    """

    def __init__(self, entry: dict):
        self._entry = entry

    @property
    def format(self) -> "str":
        """``"clap"`` or ``"vst3"``."""
        return self._entry.get("format")

    @property
    def path(self) -> "str":
        """The binary, on the server's machine."""
        return self._entry.get("path")

    @property
    def id(self) -> "str":
        """The plugin's native id within its binary."""
        return self._entry.get("id")

    @property
    def key(self) -> "str":
        """Its identity in one string: ``format:path#id``."""
        return self._entry.get("key")

    @property
    def name(self) -> "str":
        """What it is called."""
        return self._entry.get("name")

    @property
    def vendor(self) -> "str":
        """Who makes it."""
        return self._entry.get("vendor")

    @property
    def version(self) -> "str":
        """The vendor's version string."""
        return self._entry.get("version")

    @property
    def description(self) -> "str":
        """Its own one-line description, when it has one."""
        return self._entry.get("description")

    @property
    def url(self) -> "str":
        """The plugin's or vendor's web address."""
        return self._entry.get("url")

    @property
    def kind(self) -> "str":
        """What it is: ``"instrument"``, ``"effect"``, ``"note_effect"``, ``"analyzer"`` or ``"other"``."""
        return self._entry.get("kind")

    @property
    def channels(self) -> "str | None":
        """Its channel layout: ``"mono"``, ``"stereo"``, ``"surround"`` or ``"ambisonic"``."""
        return self._entry.get("channels")

    @property
    def latency(self) -> "int":
        """Its declared latency, in samples."""
        return self._entry.get("latency")

    @property
    def features(self) -> list:
        """What it declares itself to be, in its format's vocabulary, as declared."""
        return list(self._entry.get("features", ()))

    @property
    def inputs(self) -> list:
        """Its audio inputs, in processing order."""
        return list(self._entry.get("inputs", ()))

    @property
    def outputs(self) -> list:
        """Its audio outputs, in processing order."""
        return list(self._entry.get("outputs", ()))

    @property
    def note_inputs(self) -> list:
        """Its note inputs."""
        return list(self._entry.get("note_inputs", ()))

    @property
    def note_outputs(self) -> list:
        """Its note outputs."""
        return list(self._entry.get("note_outputs", ()))

    @property
    def parameters(self) -> list:
        """Its parameters, each with ``id``, ``name``, ``module``, ``flags``, ``min``, ``max``, ``step`` and ``default`` in the format's native units."""
        return list(self._entry.get("parameters", ()))

    @property
    def tags(self) -> tuple:
        """What it does, in the common vocabulary (``"synth"``, ``"reverb"``, ...)."""
        return tuple(self._entry.get("tags", ()))

    @property
    def controls(self) -> dict:
        """The names its node's controls answer to, each with the native-id
        name it stands for."""
        return dict(self._entry.get("controls", {}))

    @property
    def has_state(self) -> bool:
        """Whether the plugin saves and loads its state."""
        return bool(self._entry.get("state"))

    @property
    def has_editor(self) -> bool:
        """Whether the plugin has a native editor window."""
        return bool(self._entry.get("editor"))

    def data(self) -> dict:
        """The catalog's entry for it, as JSON writes it."""
        return json.loads(json.dumps(self._entry))

    def __eq__(self, other):
        return isinstance(other, Plugin) and other.key == self.key

    def __hash__(self):
        return hash(self.key)

    def __repr__(self):
        return f"<Plugin {self.format} {self.name!r} by {self.vendor!r}: {self.kind}>"


class PluginCatalog:
    """**What is installed**: the plugins the server's discovery found, each a
    `Plugin`, and the binaries that failed (`errors`, each ``{"path",
    "error"}``). Made by `Server.plugins`; iterate it, index it or search it.
    """

    def __init__(self, data: dict):
        self._data = data
        self.plugins = [Plugin(entry) for entry in data.get("plugins", ())]
        self.errors = list(data.get("errors", ()))

    @classmethod
    def from_rows(cls, rows: list) -> "PluginCatalog":
        """The catalog of `/plugin_scan` rows (`Server.scan_plugins`), read by
        the core."""
        return cls(_native.plugin_catalog(rows))

    def __iter__(self):
        return iter(self.plugins)

    def __len__(self):
        return len(self.plugins)

    def __getitem__(self, index):
        return self.plugins[index]

    @staticmethod
    def _query(name, vendor, format, kind, tag) -> dict:
        query = {"name": name, "vendor": vendor, "format": format, "kind": kind, "tag": tag}
        return {key: value for key, value in query.items() if value is not None}

    def find(self, name: "str | None" = None, *, vendor: "str | None" = None,
             format: "str | None" = None, kind: "str | None" = None,
             tag: "str | None" = None) -> Plugin:
        """**The one plugin these name.** ``name`` is part of the name, any
        case, and a whole-name match wins over the rest; `vendor`, `format`
        (``"clap"`` or ``"vst3"``), `kind` and `tag` must match too. `ValueError`
        when none matches, or when several still do -- the message names them,
        so a format or a vendor can say which."""
        index = _native.plugin_find(self._data.get("plugins", []),
                                    self._query(name, vendor, format, kind, tag))
        return self.plugins[index]

    def filter(self, name: "str | None" = None, *, vendor: "str | None" = None,
               format: "str | None" = None, kind: "str | None" = None,
               tag: "str | None" = None) -> "list[Plugin]":
        """**Every plugin these hold for**, in catalog order (`find`'s terms)."""
        indices = _native.plugin_filter(self._data.get("plugins", []),
                                        self._query(name, vendor, format, kind, tag))
        return [self.plugins[i] for i in indices]

    def instruments(self) -> "list[Plugin]":
        """The instruments."""
        return self.filter(kind="instrument")

    def effects(self) -> "list[Plugin]":
        """The effects."""
        return self.filter(kind="effect")

    def __repr__(self):
        return f"<PluginCatalog: {len(self.plugins)} plugins, {len(self.errors)} errors>"


class PluginDef:
    """**A persistent recipe for one native plugin**: which plugin, which
    buses its audio ports use, and the state it starts from. Each `Synth` of
    it is an isolated instance with its own helper process.

    Built from a `Plugin` (``PluginDef(name, plugin)``), its ports are routed
    by `inputs` and `outputs`: ``None`` leaves them unconnected (an input
    reads silence), a bus number puts the main port there and leaves the
    auxiliary ones (a sidechain, separate outputs) unconnected -- an insert,
    as a track takes one -- and ``{port: bus}`` routes the ports it names --
    by their control name (``"in"``, ``"in_sidechain"``) or native-id name
    (``"in:10"``) -- and leaves the rest unconnected. A port's channels take
    consecutive buses from its own. Outputs default to bus 0
    and inputs to unconnected; a node changes them as controls (``in``,
    ``out``). A port the plugin does not have is refused here, before
    anything is sent. `state` is optional initial-state bytes.

    A `reference` dict (`reference` attribute) is the stored form: format,
    binary path, native id, every audio port's ``id``, ``channels`` and
    ``bus``, and optional ``state`` as integers; a stored def is rebuilt from
    one.

    A node's controls are named as the plugin's `controls` say, and by their
    native ids: parameters ``param:<native-id>`` (CLAP plain-value units,
    VST3 normalized `[0, 1]` values), ports ``in:<native-id>`` and
    ``out:<native-id>``. A negative bus leaves a port unconnected.
    """
    def __init__(self, name: str, plugin: "Plugin | dict", *, inputs=None, outputs=0,
                 state: "bytes | None" = None):
        self.name = name
        if isinstance(plugin, Plugin):
            #: The plugin it was built from; `None` for a stored reference.
            self.plugin = plugin
            self.reference = _native.plugin_reference(plugin.data(), inputs, outputs, state)
        else:
            if inputs is not None or outputs != 0 or state is not None:
                raise ValueError("a reference routes its own ports: pass inputs, outputs "
                                 "and state with a Plugin")
            self.plugin = None
            self.reference = plugin

    def dump_def(self) -> str:
        """Serialize the reference, including any initial-state bytes."""
        return json.dumps(self.reference)

    def send(self, server=None, *, wait: bool = True, timeout: float = 10.0) -> str:
        """Send asynchronously; `wait=False` requires a subsequent sync barrier."""
        return send_def(_resolve(server), "plugin", [self.name, self.dump_def()],
                        self.name, wait, timeout)
