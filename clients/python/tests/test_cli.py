"""The console script's client half: the verbs that act on a running server.

Nothing here spawns a binary — the point of these commands is what they do to a
server that is *already* there, and their failure path (nobody home) is the one
that must stay legible, since it is what a stray-server hunt runs into first.
"""

import socket

from clausters import _cli


def _free_port() -> int:
    probe = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    probe.bind(("127.0.0.1", 0))
    port = probe.getsockname()[1]
    probe.close()
    return port


def test_a_command_word_is_not_forwarded_to_the_binary(monkeypatch):
    # The two namespaces cannot collide: every server flag starts with a dash,
    # so a leading word is ours and anything else is the binary's, untouched.
    seen = {}

    def fake_client_main(argv):
        seen["argv"] = argv
        return 0

    monkeypatch.setattr(_cli, "client_main", fake_client_main)
    monkeypatch.setattr(_cli, "server_path", lambda: "/nonexistent")
    assert _cli.main(["status", "--port", "57130"]) == 0
    assert seen["argv"] == ["status", "--port", "57130"]


def test_a_flag_still_goes_to_the_server(monkeypatch):
    forwarded = {}
    monkeypatch.setattr(_cli, "server_path", lambda: "/nonexistent")
    monkeypatch.setattr(_cli, "_ensure_executable", lambda path: None)
    monkeypatch.setattr(_cli.os, "execv",
                        lambda path, argv: forwarded.setdefault("argv", argv))
    _cli.main(["--workers", "3"])
    assert forwarded["argv"][1:] == ["--workers", "3"]


def test_it_reports_when_no_server_answers(capsys):
    code = _cli.client_main(["status", "--port", str(_free_port())])
    assert code == 1
    assert "no server answers" in capsys.readouterr().err


def test_a_bad_port_is_a_usage_error(capsys):
    assert _cli.client_main(["stop", "--port", "nope"]) == 2
    assert "takes a number" in capsys.readouterr().err


def test_a_stray_argument_is_a_usage_error(capsys):
    assert _cli.client_main(["stop", "--port", "57110", "extra"]) == 2
    assert "unexpected argument" in capsys.readouterr().err
