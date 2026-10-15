# Campaign record layout

This directory is a structural reference, not a fabricated campaign. Use prepare and run to create real manifests, captures, resolved scenarios, logs and numeric evidence. The runtime formats are defined in ../../formats.md. The template JSON deliberately keeps status=template and null values; it is not a second runtime schema.

The runner generates manifest.json, environment.json, report.md, findings.md, scenarios/, logs/, results/ and evidence/. Edit findings.md to record real review and limitations. Add a complete instrumentation note when a patch was used. attach copies external manual artifacts and records their hashes. seal checks coverage/derived data and creates hashes.json; no skipped required check can be accepted. archive verifies extraction and writes an archive checksum sidecar.

Measurements are never filled with placeholder zero values. Partial and failed collections retain their real status. No measurement, build or device access occurs by reading or copying these templates.


Failed or partial audits are valid historical evidence too. To retain them after review, use `seal RECORD --reviewer NAME --preserve-incomplete --reason "WHY COVERAGE/FAILURES ARE RETAINED"`, then verify/archive normally. This explicit mode keeps status failed/partial and review_outcome=evidence-only, sets quality_accepted=false, and never makes the record an accepted performance baseline. It does not bypass raw-data, patch or hash integrity checks. Default sealing still requires complete passing coverage.
