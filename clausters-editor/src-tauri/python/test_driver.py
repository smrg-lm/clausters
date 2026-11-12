"""The driver's protocol, spoken to as the application speaks to it.

Each test starts `driver.py` as a subprocess, writes JSON requests to its stdin
one per line, and reads the JSON messages it writes back -- the whole protocol
the Rust side carries, with nothing of the application around it. Standard
library only, so it runs wherever the driver does:

    python3 -m unittest discover -s src-tauri/python -p 'test_*.py'
"""

import json
import os
import queue
import signal
import subprocess
import sys
import threading
import unittest

DRIVER = os.path.join(os.path.dirname(os.path.abspath(__file__)), "driver.py")
TIMEOUT = 20.0


class Driver:
    """One session: the process, and the messages it has written so far."""

    def __init__(self):
        self.proc = subprocess.Popen(
            [sys.executable, "-u", DRIVER],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            env={**os.environ, "PYTHONIOENCODING": "utf-8"},
        )
        self.messages = queue.Queue()
        threading.Thread(target=self._read, daemon=True).start()
        self.ready = self.next()

    def _read(self):
        for line in self.proc.stdout:
            self.messages.put(json.loads(line))
        self.messages.put(None)

    def send(self, **request):
        self.proc.stdin.write(json.dumps(request) + "\n")
        self.proc.stdin.flush()

    def next(self):
        message = self.messages.get(timeout=TIMEOUT)
        if message is None:
            raise AssertionError("the driver ended")
        return message

    def printed(self, expected):
        """Waits for `expected` on the session's output: a print arrives as one
        message per write, the text and its newline apart."""
        got = ""
        while got != expected:
            message = self.next()
            if message["type"] != "out":
                raise AssertionError(f"{message!r} before the output {expected!r}")
            got += message["text"]

    def until_done(self, id):
        """Every message up to and including request `id`'s `done`."""
        seen = []
        while True:
            message = self.next()
            seen.append(message)
            if message["type"] == "done" and message["id"] == id:
                return seen

    def close(self):
        if self.proc.poll() is None:
            self.proc.stdin.close()
            try:
                self.proc.wait(timeout=TIMEOUT)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
        self.proc.stdout.close()
        self.proc.stderr.close()


def text(messages, kind):
    return "".join(m["text"] for m in messages if m["type"] == kind)


class TheDriver(unittest.TestCase):
    def setUp(self):
        self.driver = Driver()
        self.addCleanup(self.driver.close)

    def test_it_says_it_is_ready_with_its_version(self):
        self.assertEqual(self.driver.ready["type"], "ready")
        self.assertIn(sys.version.split()[0], self.driver.ready["text"])

    def test_an_evaluation_prints_and_answers_its_last_expression(self):
        self.driver.send(id=1, op="eval", code="print('hello')\n2 + 3", line=1)
        seen = self.driver.until_done(1)
        self.assertEqual(text(seen, "out"), "hello\n")
        self.assertEqual(text(seen, "result"), "5")
        self.assertTrue(all(m["id"] == 1 for m in seen))

    def test_the_session_keeps_its_names(self):
        self.driver.send(id=1, op="eval", code="x = 41", line=1)
        self.driver.until_done(1)
        self.driver.send(id=2, op="eval", code="x + 1", line=1)
        self.assertEqual(text(self.driver.until_done(2), "result"), "42")

    def test_an_error_is_reported_at_the_file_s_line_and_the_session_goes_on(self):
        self.driver.send(id=1, op="eval", code="a = 1\n1 / 0", file="piece.py", line=10)
        err = text(self.driver.until_done(1), "err")
        self.assertIn("ZeroDivisionError", err)
        self.assertIn('File "piece.py", line 11', err)
        self.assertNotIn("driver.py", err, "the driver's own frames are dropped")
        self.driver.send(id=2, op="eval", code="a", line=1)
        self.assertEqual(text(self.driver.until_done(2), "result"), "1")

    def test_a_syntax_error_is_reported(self):
        self.driver.send(id=1, op="eval", code="def (:", line=1)
        self.assertIn("SyntaxError", text(self.driver.until_done(1), "err"))

    def test_input_is_refused_rather_than_waited_on(self):
        self.driver.send(id=1, op="eval", code="input()", line=1)
        self.assertIn("input() is not available", text(self.driver.until_done(1), "err"))

    @unittest.skipUnless(hasattr(signal, "SIGINT") and os.name == "posix", "SIGINT")
    def test_an_interrupt_stops_the_running_code_and_the_session_goes_on(self):
        self.driver.send(
            id=1, op="eval", code="import time\nprint('started')\nwhile True: time.sleep(0.01)",
            line=1,
        )
        self.driver.printed("started\n")
        self.driver.proc.send_signal(signal.SIGINT)
        self.assertIn("KeyboardInterrupt", text(self.driver.until_done(1), "err"))
        self.driver.send(id=2, op="eval", code="'alive'", line=1)
        self.assertEqual(text(self.driver.until_done(2), "result"), "'alive'")

    def test_help_is_answered_while_code_runs(self):
        self.driver.send(
            id=1, op="eval", code="import time\nprint('started')\ntime.sleep(1.5)", line=1,
        )
        self.driver.printed("started\n")
        self.driver.send(id=2, op="help", expr="len")
        answer = self.driver.next()
        self.assertEqual((answer["type"], answer["id"]), ("help", 2),
                         "the help came before the evaluation ended")
        self.assertIn("len", answer["text"])
        self.driver.until_done(1)

    def test_a_query_is_answered_with_or_without_jedi(self):
        self.driver.send(id=1, op="complete", source="import os\nos.pa", line=2, column=5)
        answer = self.driver.next()
        while answer["type"] == "info":  # the one note that jedi is missing
            answer = self.driver.next()
        self.assertEqual((answer["type"], answer["id"]), ("complete", 1))
        self.assertIsInstance(answer["data"], list)

    def test_a_request_that_is_not_json_is_reported(self):
        self.driver.proc.stdin.write("not json\n")
        self.driver.proc.stdin.flush()
        message = self.driver.next()
        self.assertEqual(message["type"], "err")
        self.assertIn("Invalid request", message["text"])

    def test_closing_its_input_ends_it(self):
        self.driver.proc.stdin.close()
        self.assertEqual(self.driver.proc.wait(timeout=TIMEOUT), 0)


if __name__ == "__main__":
    unittest.main()
