# Audio and GUI interference procedure

This component scenario has required manual execution because audio devices and GPU/display configurations are physical prerequisites. Complete and retain the selected audio and GUI controls before claiming combined coverage. Keep the same source commit, builds, load, sample rate, callback sizes, physical viewport, scale and action sequence across controls and combined trials.

Warm up each condition for 10 seconds. Collect three paired 60-second repetitions of audio only, GUI only and both together, alternating condition order between repetitions. Keep the audio server/driver and GUI driver in the same invocation when sandbox network isolation applies. Record actual PID/binary identities, startup/shutdown commands, ports and cleanup. Reserve the machine so other agents do not compile during capture.

Capture audio deadlines/xruns and output correctness plus GUI frame/interaction behavior, process CPU/RSS and available GPU/upload counters. Retain raw files per condition/repetition. No missing counter becomes zero, and no average replaces an audio deadline miss. Mark unsupported metrics explicitly; if essential device evidence is unavailable, leave the check skipped.

Attach combined-device evidence with controls, raw captures, exact input timeline, thermal/background-load observations, source/fixture hashes and exclusions. Report observed interference separately for audio and GUI. First-run performance figures are descriptive, with no invented cross-machine threshold. Correctness and recovery failures are failures even when average throughput improves.
