"""Endpoint adapters for the shared Rust clipboard exchange.

The table holds service handles, never publication policy or document state.
An exchange outlives its source windows; external replacement is reconciled
when the next clipboard action enters the service.
"""

import weakref
import threading

from ... import _native
from .application import _service_instance
from ._cleanup import release_buffer

_EXCHANGES = weakref.WeakKeyDictionary()
_EXCHANGES_LOCK = threading.RLock()


class _Lease:
    def __init__(self, core, lease, instance):
        self.core, self.lease, self.instance = core, lease, instance


def _released(server, core, answer, instance):
    for source in answer.get("released") or ():
        if source["server"] == instance:
            release_buffer(server, core, source)


def bind_exchange(server, core):
    """Route publication/readers to the composition that owns their resources."""
    with _EXCHANGES_LOCK:
        instance = _service_instance(server)
        held = _EXCHANGES.get(server)
        if held is not None and held[1] is not core:
            action = "clear" if held[0] == instance else "invalidate"
            answer = held[1].call("exchange", action=action, server=held[0])
            _released(server, held[1], answer, instance)
        _EXCHANGES[server] = (dict(instance), core)


def exchange(server, action, **args):
    with _EXCHANGES_LOCK:
        instance = _service_instance(server)
        if action == "release" and isinstance(args.get("lease"), _Lease):
            lease = args["lease"]
            if lease.instance != instance:
                lease.core.call("exchange", action="invalidate", server=lease.instance)
            answer = lease.core.call("exchange", action="release", lease=lease.lease)
            _released(server, lease.core, answer, instance)
            return answer
        held = _EXCHANGES.get(server)
        if held is None or held[0] != instance:
            if held is not None:
                held[1].call("exchange", action="invalidate", server=held[0])
            held = (dict(instance), _native.EditingCore())
            _EXCHANGES[server] = held
        answer = held[1].call("exchange", action=action, server=instance, **args)
        _released(server, held[1], answer, instance)
        if "error" in answer:
            raise ValueError(answer["error"])
        if answer.get("lease") is not None:
            answer["lease"] = _Lease(held[1], answer["lease"], dict(instance))
        return answer
