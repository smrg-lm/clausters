"""Run each collected pytest file in a fresh process, retaining verbose output."""
import subprocess
import sys


def main(argv):
    # argv is the original Python/pytest invocation. Collection must succeed
    # before any test can be counted; failures in one file do not hide others.
    collect = subprocess.run([*argv, "--collect-only", "-q"], capture_output=True, text=True)
    print(collect.stdout, end="", flush=True)
    print(collect.stderr, end="", file=sys.stderr, flush=True)
    if collect.returncode:
        return collect.returncode
    files = sorted({line.split("::")[0] for line in collect.stdout.splitlines() if "::" in line})
    if not files:
        raise ValueError("No pytest files collected")
    failed = False
    for filename in files:
        print("isolated pytest file: " + filename, flush=True)
        result = subprocess.run([*argv, "-v", filename])
        if result.returncode < 0:
            import signal
            print("isolated signal: " + signal.Signals(-result.returncode).name, flush=True)
        failed |= result.returncode != 0
    return int(failed)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
