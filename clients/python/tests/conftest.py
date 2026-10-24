"""What every test of the package shares.

The audio editor's working folders are made under the user's cache by the
GUI host that holds the editor, and a test that opens an editor without
closing it would leave one there for the next editor to sweep. The suite gets
a cache of its own instead, removed when it ends.
"""

import os
import shutil
import tempfile

_CACHE = tempfile.mkdtemp(prefix="clausters-test-cache-")
os.environ["CLAUSTERS_CACHE_DIR"] = _CACHE


def pytest_sessionfinish(session, exitstatus):
    shutil.rmtree(_CACHE, ignore_errors=True)
