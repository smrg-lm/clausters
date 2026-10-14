"""Native plugin references for an isolated instance on a native server."""
import json
from ._wire import resolve as _resolve, send_def


class PluginDef:
    """A persistent recipe with native format, binary and plugin identities.

    Each input/output row has `id`, `channels` and `bus`. IDs come from
    `Server.scan_plugins`; every native audio port must be represented.
    `state` contains optional opaque initial-state bytes as JSON integers.
    Parameter controls use `param:<native-id>`: CLAP plain-value units,
    VST3 normalized `[0, 1]` values;
    port controls use `in:<native-id>` and `out:<native-id>`.
    """
    def __init__(self, name: str, reference: dict):
        self.name = name
        self.reference = reference

    def dump_def(self) -> str:
        """Serialize the reference, including any initial-state bytes."""
        return json.dumps(self.reference)

    def send(self, server=None, *, wait: bool = True, timeout: float = 10.0) -> str:
        """Send asynchronously; `wait=False` requires a subsequent sync barrier."""
        return send_def(_resolve(server), "plugin", [self.name, self.dump_def()],
                        self.name, wait, timeout)
