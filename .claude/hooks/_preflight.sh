# Sourced by the hooks in this directory. Not executable on its own.
#
# Every hook here reads its input with jq and shells out to a tool it does not
# ship (rustfmt, cargo, GNU stat/find). Miss any of them and the hook does not
# error — it parses nothing, matches nothing and exits 0. The protection is off
# and looks exactly like the protection being on, which is the same silent
# failure mode `check-stale-bin.sh` exists to prevent.
#
# So a hook whose dependencies are missing says so, on stderr, and then still
# gets out of the way. It never blocks: a machine that cannot run the check is
# not a reason to stop the work.

# `hook_missing_tools jq rustfmt ...` -> the ones not on the PATH, space
# separated, empty if all are present.
hook_missing_tools() {
    local tool missing=""
    for tool in "$@"; do
        command -v "$tool" >/dev/null 2>&1 || missing="$missing $tool"
    done
    printf '%s' "${missing# }"
}

# GNU coreutils/findutils: `stat -c` and `find -newerXY`, both of which the
# BSD/macOS versions reject. Checked as a capability rather than by platform.
hook_has_gnu_file_tools() {
    stat -c %Z /dev/null >/dev/null 2>&1 &&
        find /dev/null -newermc /dev/null >/dev/null 2>&1
}

# `hook_warn_once <key> <line>...` — print the lines on stderr at most once
# every 12 hours per key, and return 0 when it did print.
#
# Twelve hours, not every invocation: a broken setup has to surface the same
# day someone clones, but a message on every single edit is noise, and noise is
# how a hook ends up disabled.
hook_warn_once() {
    # The key becomes a filename, so anything that is not a plain word has to
    # go — a `/` in it (a missing tool reported as "GNU-stat/find", say) would
    # point the stamp at a directory that does not exist, the write would fail,
    # and the warning would then repeat on every single invocation: exactly the
    # noise the throttle is for.
    local key="${1//[^A-Za-z0-9._-]/-}"
    shift
    local stamp="${TMPDIR:-/tmp}/.clausters-hook-warn-$(id -u)-$key"
    if [ -f "$stamp" ] && [ -z "$(find "$stamp" -mmin +720 2>/dev/null)" ]; then
        return 1
    fi
    # stderr silenced *before* the write, so a redirection that fails anyway
    # (read-only TMPDIR) does not print a shell error of its own.
    : 2>/dev/null >"$stamp"
    printf '%s\n' "$@" >&2
    return 0
}
