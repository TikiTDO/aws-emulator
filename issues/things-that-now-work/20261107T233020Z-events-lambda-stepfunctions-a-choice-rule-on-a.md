Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__stepfunctions.refarch-sfn-choice-on-missing-status-path-rejected-at-runtime
Issue-Key: events__lambda__stepfunctions.refarch-sfn-choice-on-missing-status-path-rejected-at-runtime|run#19|AssertionError|assert "$.status" in cause, (
Issue-Commit: 69a812023dc7b19082ccb88a67a077b5f3204d48

# events__lambda__stepfunctions: a Choice rule on a path absent from the input fails with a States.Runtime cause that names no path

> _Note: this issue was identified against the code base at commit `69a812023dc7b19082ccb88a67a077b5f3204d48`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
One net-new root cause, 1 failing check.

Cause 1 of 1: when the job-poller's Choice state cannot resolve the path its rule compares ($.status, absent from the CheckStatus payload), the execution does fail with States.Runtime, but the cause does not name the path that could not be resolved.
  AWS: recorded behavior only (the Amazon States Language is not in the Smithy model; DescribeExecution models cause as SensitiveCause). The recording shows the execution FAILED with error States.Runtime and a cause CONTAINING the unresolved path exactly as the definition writes it ('$.status'), so the operator learns which field the poller's router needed. Hypothesis for the full template: "An error occurred while executing the state '<state name>' (entered at the event id #<id>). Invalid path '<path>' : No results for path: $['<field>']" — state name, event id and path are values from the definition and the run; what the recording fixes as the requirement is that the referenced path appears in the cause.
  Emulator: the cause is "An error occurred while executing the state 'JobComplete' (entered at the event id #14). The choice state's condition path references an invalid value." — a fixed, emulator-authored sentence with no request-derived value; '$.status' never appears.
  Why: in a Wait/CheckStatus poller the whole point of this failure is to tell the operator which field the upstream task stopped returning. With the path omitted, the message is indistinguishable for every Choice in the machine, and any Catch/alerting that classifies on AWS's path-naming cause never matches.
  Fix: the Choice state's path-resolution diagnostics in the sfn service crate's interpreter (services/service-com-amazonaws-sfn-awsstepfunctions/src/state/asl.rs — the cause built when a Variable/*Path reference does not resolve). The same cause is filed under the stepfunctions heading for its two checks there.
  Traces: events__lambda__stepfunctions.refarch-sfn-choice-on-missing-status-path-rejected-at-runtime.

## Evidence
AssertionError: the cause should name the path that could not be resolved; got "An error occurred while executing the state 'JobComplete' (entered at the event id #14). The choice state's condition path references an invalid value."

Expected: a States.Runtime cause containing '$.status'. The state name and event id are per-definition/per-run values; the requirement is the presence of the referenced path.

## Steps to Reproduce
1. Create a Submit/Wait/CheckStatus/Choice state machine whose Choice rule compares $.status and whose CheckStatus Lambda returns a payload WITHOUT the status member (e.g. return {"id": event["id"]}).
2. StartExecution and poll DescribeExecution: status FAILED, error States.Runtime — and the cause must name '$.status'. The emulator's cause names only the state and event id.
