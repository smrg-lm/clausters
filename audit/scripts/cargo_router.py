"""Route nested Cargo invocations to independent workspace targets."""
import os
from pathlib import Path
import sys
from controls import target_for

source = Path(os.environ["AUDIT_SOURCE"])
cwd = Path.cwd()
manifest = next((sys.argv[i + 1] for i, arg in enumerate(sys.argv[:-1]) if arg == "--manifest-path"), None)
if manifest:
    cwd = (cwd / manifest).resolve().parent
relative = str(cwd.relative_to(source))
os.environ["CARGO_TARGET_DIR"] = str(target_for(os.environ["AUDIT_TARGET"], relative))
os.execv(os.environ["AUDIT_REAL_CARGO"], [os.environ["AUDIT_REAL_CARGO"], *sys.argv[1:]])
