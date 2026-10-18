"""Endpoint adapters for the shared Rust clipboard exchange.

The table holds service handles, never publication policy or document state.
An exchange outlives its source windows; external replacement is reconciled
when the next clipboard action enters the service.
"""

import weakref
import threading

from ... import _native
from .application import _service_instance

_EXCHANGES = weakref.WeakKeyDictionary()
_EXCHANGES_LOCK = threading.RLock()


def exchange(server, action, **args):
    with _EXCHANGES_LOCK:
        instance = _service_instance(server)
        held = _EXCHANGES.get(server)
        if held is None or held[0] != instance:
            if held is not None:
                held[1].call("exchange", action="invalidate", server=held[0])
                held[1].free()
            held = (dict(instance), _native.EditingCore())
            _EXCHANGES[server] = held
        answer = held[1].call("exchange", action=action, server=instance, **args)
        for source in answer.get("released") or ():
            if source["server"] == instance:
                server.send_msg("/buffer_free", int(source["buffer"]))
                server.buffers.free(int(source["buffer"]))
        if "error" in answer:
            raise ValueError(answer["error"])
        return answer
