Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs__s3.log-cwl-plumbing-export-objects-hold-exactly-the-streams-events-under-prefix
Issue-Key: logs__s3.log-cwl-plumbing-export-objects-hold-exactly-the-streams-events-under-prefix|run#22|AssertionError|assert not extra, (
Issue-Commit: c0ced5d18fa4eb624de2042a998eca41dbf9ea10

# CreateExportTask writes event timestamps with two fractional digits instead of milliseconds

> _Note: this issue was identified against the code base at commit `c0ced5d18fa4eb624de2042a998eca41dbf9ea10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateExportTask

## Description
One net-new root cause.

Cause 1 of 1: the objects a log-group export writes to S3 render each event's timestamp with a truncated fraction, so the exported lines are not the lines the log events hold.
AWS: recorded behavior only for the object body (the Smithy model covers CreateExportTask's parameters, not the rendering). The recording shows each exported line as '<ISO-8601 timestamp with three fractional-second digits>Z <message>' — the millisecond precision of the event's timestamp is preserved.
Emulator: the fraction is emitted with two digits, dropping the trailing zero — lines read '…T03:54:53.63Z <message>' where the event's timestamp requires '…T03:54:53.630Z <message>'. The timestamps and message text shown are per-run values; the requirement is the three-digit millisecond fraction.
Why: a consumer that parses exported lines with a fixed millisecond format, or that joins exported lines back to the events by timestamp, fails to match any line; byte-exact comparisons against the events that were written always differ.
Fix: the export-task writer's timestamp formatting — render the event timestamp with three fractional-second digits (zero-padded) rather than trimming trailing zeros.
Traces: logs__s3.log-cwl-plumbing-export-objects-hold-exactly-the-streams-events-under-prefix.

## Evidence
Check logs__s3.log-cwl-plumbing-export-objects-hold-exactly-the-streams-events-under-prefix [mismatch]:
AssertionError: export <taskId> contains lines that were never written as '<ISO ts> <message>': ['2026-11-11T03:54:53.63Z <stream> keep-0', '2026-11-11T03:54:54.63Z <stream> keep-1', '2026-11-11T03:54:55.63Z <stream> keep-2']; expected ['2026-11-11T03:54:53.630Z <stream> keep-0', '2026-11-11T03:54:54.630Z <stream> keep-1', '2026-11-11T03:54:55.630Z <stream> keep-2']

Requirement: the three-digit millisecond fraction in the rendered timestamp; the clock values and message text are per-run.

## Steps to Reproduce
1. Create a log group and stream and PutLogEvents with timestamps whose millisecond component ends in 0 (e.g. ...630).
2. CreateExportTask for that group to an S3 prefix and wait for COMPLETED.
3. Read the exported object and compare each line to '<ISO ts with milliseconds>Z <message>': the fraction has two digits.
