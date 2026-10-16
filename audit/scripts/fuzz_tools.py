"""Attest explicitly provisioned optional fuzz tools and versioned seeds."""
from pathlib import Path

TOOLCHAIN = "nightly-2026-10-15"
FUZZ_VERSION = "cargo-fuzz 0.13.2"


def attest(work, cargo_fuzz):
    from audit import command, digest, locked, read, write
    work = Path(work).resolve()
    read(work / "prepared.json")
    binary = Path(cargo_fuzz).resolve(strict=True)
    if not binary.is_relative_to(work / "tools"):
        raise ValueError("Provision cargo-fuzz inside the campaign tools directory")
    with locked(work):
        version = command([str(binary), "--version"])
        if version != FUZZ_VERSION:
            raise ValueError("Expected " + FUZZ_VERSION)
        rustc = command(["rustup", "run", TOOLCHAIN, "rustc", "--version"])
        source = work / "source"
        names = command(["git", "ls-files", "--", "audit/fuzz/seeds/"], source).splitlines()
        if not names:
            raise ValueError("No versioned fuzz seeds")
        destination = work / "setup/fuzz"
        if destination.exists():
            raise ValueError("Fuzz tool attestation already exists; use a new preparation")
        write(destination / "receipt.json", {
            "toolchain": TOOLCHAIN, "rustc": rustc, "cargo_fuzz": version,
            "executable": str(binary), "sha256": digest(binary),
            "seeds": {n: digest(source / n) for n in names},
            "installation": "Explicit external provisioning; this command installs nothing"})
        return destination / "receipt.json"
