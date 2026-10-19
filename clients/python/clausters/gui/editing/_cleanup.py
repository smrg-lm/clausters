"""Execute acknowledged resource cleanup chosen by the shared core."""
import threading
import weakref

from ... import _native
from ..._steps import run_steps
from .application import _service_instance


_EXECUTORS = weakref.WeakKeyDictionary()
_EXECUTORS_LOCK = threading.RLock()


def execute_steps(server, instance, steps):
    """Serialize indistinguishable command waits on one endpoint."""
    with _EXECUTORS_LOCK:
        lock = _EXECUTORS.setdefault(server, threading.RLock())
    with lock:
        if instance != _service_instance(server):
            raise RuntimeError("the batch belongs to another server instance")
        run_steps(server, _native.StepRunner(), steps)


def release_buffer(server, core, resource):
    """Return an id only after its exact cleanup ticket is confirmed."""
    if resource["server"] != _service_instance(server):
        return False
    release = core.call("cleanup", action="prepare", resource=resource)
    if release.get("ticket") is None:
        return False
    try:
        execute_steps(server, resource["server"], release.get("steps") or [])
    except Exception as error:
        core.call("cleanup", action="complete", ticket=release["ticket"], success=False, reason=str(error))
        return False
    answer = core.call("cleanup", action="complete", ticket=release["ticket"], success=True)
    if answer.get("accepted"):
        server.buffers.free(int(resource["buffer"]))
        return True
    return False
