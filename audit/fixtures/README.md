# Deterministic fixtures

Generate external input data with Python's standard library:

```sh
python3 audit/fixtures/generate.py /tmp/clausters-audit-fixtures
```

The new destination receives two mono PCM16 WAV files, each one second at 48 kHz, at 1000 Hz and 125 Hz, with a manifest containing byte hashes. Integer square waves avoid platform-dependent floating-point transcendental rounding. These inputs test loading/display/scaling; they are not antialiasing or perceptual-quality references. The generator requires a new directory and never overwrites an existing dataset.

The waveform definition is original synthetic data under the repository license. Generation has no random seed because it uses no randomness. Retain the generator's audit-resource commit, parameters and manifest with a campaign that uses it. Longer/scaled datasets require a recorded generation command and hash, not an unexplained personal file. Existing Rust tests and benchmark have their own internal workloads, fixed by the application commit; they do not consume these WAV fixtures.
