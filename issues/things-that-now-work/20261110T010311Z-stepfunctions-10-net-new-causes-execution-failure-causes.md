Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.lambda-task-inputpath-missing-field-states-runtime stepfunctions.map-invalid-input-path-fails-states-runtime stepfunctions.sync-pass-inputpath-missing-field-states-runtime stepfunctions.sync-pass-outputpath-missing-field-states-runtime stepfunctions.test-state-runtime-cause-omits-execution-prefix stepfunctions.wait-outputpath-is-applied-after-the-wait-completes stepfunctions.choice-variable-path-invalid-reference-rejected stepfunctions.map-itemselector-invalid-context-path-rejected stepfunctions.send-task-success-output-256kib-boundary stepfunctions.deep-sfn-wait-secondspath-array-index-unresolved-rejected stepfunctions.deep-sfn-wait-timestamppath-absent-and-array-index-unresolved-rejected stepfunctions.wait-inputpath-rebases-secondspath-and-timestamppath stepfunctions.array-get-item-negative-index-rejected stepfunctions.deep-sfn-language-fail-error-path-intrinsic-error-rejected stepfunctions.deep-sfn-language-format-absent-path-argument-rejected stepfunctions.deep-sfn-language-string-to-json-malformed-rejected stepfunctions.parallel-branch-and-whole-output-over-256kib-rejected stepfunctions.pass-result-path-doubling-output-over-256kib-rejected stepfunctions.sfn-jsonata-pass-state-output-over-256kib-rejected stepfunctions.map-non-array-input-fails-states-runtime stepfunctions.deep-sfn-language-map-items-path-and-task-timing-path-wrong-type-rejected stepfunctions.deep-sfn-language-fail-cause-path-non-string-rejected stepfunctions.pass-jsonata-runtime-error-emits-evaluation-failed-then-execution-failed stepfunctions.jsonata-wait-field-runtime-type-and-format-errors-are-catchable stepfunctions.jsonata-parallel-evaluation-failure-timing-and-catch stepfunctions.choice-jsonata-evaluation-order-condition-output-assign stepfunctions.catch-resultpath-unresolvable-path-reference-path-conflict stepfunctions.sync-resultpath-over-array-reference-path-conflict stepfunctions.deep-sfn-language-fail-long-error-untrimmed-in-sync-and-logs-trimmed-in-standard-history
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.lambda-task-inputpath-missing-field-states-runtime|run#12|AssertionError|assert bad.get("cause") == expected_cause, (
Issue-Key: stepfunctions.map-invalid-input-path-fails-states-runtime|run#11|AssertionError|assert bad.get("cause") == expected_cause, (
Issue-Key: stepfunctions.sync-pass-inputpath-missing-field-states-runtime|run#11|AssertionError|assert bad.get("cause") == expected_cause, (
Issue-Key: stepfunctions.sync-pass-outputpath-missing-field-states-runtime|run#11|AssertionError|assert bad.get("cause") == expected_cause, (
Issue-Key: stepfunctions.test-state-runtime-cause-omits-execution-prefix|run#11|AssertionError|assert ts_bad.get("cause") == tail, (
Issue-Key: stepfunctions.wait-outputpath-is-applied-after-the-wait-completes|run#10|AssertionError|assert desc.get("cause") == (
Issue-Key: stepfunctions.choice-variable-path-invalid-reference-rejected|run#3|AssertionError|assert bad_desc.get("cause") == expected_cause, (
Issue-Key: stepfunctions.map-itemselector-invalid-context-path-rejected|run#22|AssertionError|assert bad.get("cause") == expected_cause, (
Issue-Key: stepfunctions.send-task-success-output-256kib-boundary|run#12|AssertionError|assert final["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.deep-sfn-wait-secondspath-array-index-unresolved-rejected|run#14|AssertionError|assert bad.get("cause") == _EXPECTED_CAUSE, (
Issue-Key: stepfunctions.deep-sfn-wait-timestamppath-absent-and-array-index-unresolved-rejected|_assert_fails_unresolved#4|AssertionError|assert desc.get("cause") == expected_cause, (
Issue-Key: stepfunctions.wait-inputpath-rebases-secondspath-and-timestamppath|run#13|AssertionError|assert seconds_bad.get("cause") == (
Issue-Key: stepfunctions.array-get-item-negative-index-rejected|run#16|AssertionError|assert bad.get("cause") == expected_cause, (
Issue-Key: stepfunctions.deep-sfn-language-fail-error-path-intrinsic-error-rejected|run#10|AssertionError|assert want_fragment in (bad.get("cause") or ""), (
Issue-Key: stepfunctions.deep-sfn-language-format-absent-path-argument-rejected|run#15|AssertionError|assert bad.get("cause") == expected_cause, (
Issue-Key: stepfunctions.deep-sfn-language-string-to-json-malformed-rejected|run#11|AssertionError|assert bad.get("cause") == expected_cause, (
Issue-Key: stepfunctions.parallel-branch-and-whole-output-over-256kib-rejected|run#9|AssertionError|assert result.get("cause") == expected_cause_branch, (
Issue-Key: stepfunctions.pass-result-path-doubling-output-over-256kib-rejected|run#13|AssertionError|assert bad.get("cause") == expected_cause, (
Issue-Key: stepfunctions.sfn-jsonata-pass-state-output-over-256kib-rejected|run#10|AssertionError|assert bad.get("cause") == expected_cause, (
Issue-Key: stepfunctions.map-non-array-input-fails-states-runtime|run#11|AssertionError|assert bad.get("cause") == _RUNTIME_CAUSE, (
Issue-Key: stepfunctions.deep-sfn-language-map-items-path-and-task-timing-path-wrong-type-rejected|_check_map_items_path#2|AssertionError|assert bad.get("cause") == 'Reference path "$.Number" must point to array.', (
Issue-Key: stepfunctions.deep-sfn-language-fail-cause-path-non-string-rejected|run#9|AssertionError|assert bad.get("cause") == (
Issue-Key: stepfunctions.pass-jsonata-runtime-error-emits-evaluation-failed-then-execution-failed|run#10|AssertionError|assert desc.get("cause") == expected_cause, f"expected cause {expected_cause!r}, got {desc.get('cause')!r}"
Issue-Key: stepfunctions.jsonata-wait-field-runtime-type-and-format-errors-are-catchable|run#7|AssertionError|assert exe.get("cause") == (
Issue-Key: stepfunctions.jsonata-parallel-evaluation-failure-timing-and-catch|_assert_eval_failure#4|AssertionError|assert cause == expected_cause, f"unexpected cause for field {field!r}: {cause!r}"
Issue-Key: stepfunctions.choice-jsonata-evaluation-order-condition-output-assign|_assert_evaluation_error#11|AssertionError|assert f"specified for the field '{location}'" in cause, (
Issue-Key: stepfunctions.catch-resultpath-unresolvable-path-reference-path-conflict|run#4|AssertionError|assert result.get("error") == _REFERENCE_PATH_CONFLICT, (
Issue-Key: stepfunctions.sync-resultpath-over-array-reference-path-conflict|run#9|AssertionError|assert bad.get("error") == "States.ReferencePathConflict", (
Issue-Key: stepfunctions.deep-sfn-language-fail-long-error-untrimmed-in-sync-and-logs-trimmed-in-standard-history|run#13|AssertionError|assert got == _STANDARD_TRIM, (
Issue-Commit: 4d67debdc5b6a4a4b59f91b6ecdc4b307fee9666

# stepfunctions: 10 net-new causes — execution-failure causes (paths, intrinsics, size limits, JSONata, ResultPath) use emulator-invented text and wrong error names

> _Note: this issue was identified against the code base at commit `4d67debdc5b6a4a4b59f91b6ecdc4b307fee9666`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeExecution

## Description
Ten distinct root causes, all in how the emulator builds a failed execution's `error`/`cause` for Step Functions runtime failures. AWS's causes are fixed sentences with a fixed wrapper; the emulator substitutes its own prose, so every client that matches on the error name or the cause text breaks. Each cause below is independent and must be closed separately.

Cause 1 of 10: path-not-found causes are rendered wrong.
AWS: a JSONPath field (InputPath, OutputPath, Parameters, Map ItemsPath/ItemSelector, Choice condition path) that selects nothing fails the execution States.Runtime with cause `An error occurred while executing the state '<StateName>' (entered at the event id #<N>). Invalid path '<path-as-written>' : No results for path: $['missing']` — fixed text, with the state name, the entering event id, the path as written in the definition, and the resolved path rendered in BRACKET notation; the Choice-state variant is `Invalid path '<path>': The choice state's condition path references an invalid value.` with NO space before the colon; the context-object variant is `The JSONPath '<path>' specified for the field '<field>' could not be found in the input '<full context object JSON>'` with the state/event-id prefix and the input NOT truncated.
Emulator: renders the resolved path in dot notation (`No results for path: $.missing`), inserts a space before the colon in the Choice message (`Invalid path '$.v' : ...`), and for the Map ItemSelector context path it drops the `An error occurred while executing the state ...` prefix entirely and truncates the context object with `…`.
Why: a client that asserts on the documented cause, or parses it to recover the failing member, sees text AWS never emits; the truncated context object also loses the data the message exists to show.
Fix: the cause builder for JSONPath resolution failures (path rendering + the state/event-id wrapper + no truncation) in the Step Functions interpreter's path-evaluation component.
Traces: stepfunctions.lambda-task-inputpath-missing-field-states-runtime, stepfunctions.map-invalid-input-path-fails-states-runtime, stepfunctions.sync-pass-inputpath-missing-field-states-runtime, stepfunctions.sync-pass-outputpath-missing-field-states-runtime, stepfunctions.test-state-runtime-cause-omits-execution-prefix, stepfunctions.wait-outputpath-is-applied-after-the-wait-completes, stepfunctions.choice-variable-path-invalid-reference-rejected, stepfunctions.map-itemselector-invalid-context-path-rejected

Cause 2 of 10: the 256 KiB output limit is off by one.
AWS: a state's output of exactly 262144 bytes is within the limit and the execution continues; only MORE than 262144 bytes fails States.DataLimitExceeded. Smithy: SendTaskSuccess.output @length max 262144, so an exactly-maximal payload is a legal input.
Emulator: a SendTaskSuccess output of exactly 262144 bytes ends the execution FAILED States.DataLimitExceeded with cause `state 'T' produced a document of at least 262145 bytes, more than the 262144 a state's output may carry` — it counts one byte too many at the boundary.
Why: a worker returning a maximal-but-legal result has its execution killed.
Fix: the boundary comparison in the output-size check of the task-result handler.
Traces: stepfunctions.send-task-success-output-256kib-boundary

Cause 3 of 10: Wait SecondsPath/TimestampPath unresolved causes use the generic path message.
AWS: when a Wait state's SecondsPath or TimestampPath selects no value the cause is `An error occurred while executing the state '<StateName>' (entered at the event id #<N>). The SecondsPath parameter does not reference an input value: <path>` (likewise `The TimestampPath parameter does not reference an input value: <path>`) — fixed text naming the field, with the path from the definition.
Emulator: emits the generic `Invalid path '<path>' : No results for path: <path>. The 'SecondsPath' field selected no value.`
Why: a client matching the documented Wait-field diagnostic never matches.
Fix: the Wait state's SecondsPath/TimestampPath evaluation in the interpreter.
Traces: stepfunctions.deep-sfn-wait-secondspath-array-index-unresolved-rejected, stepfunctions.deep-sfn-wait-timestamppath-absent-and-array-index-unresolved-rejected, stepfunctions.wait-inputpath-rebases-secondspath-and-timestamppath

Cause 4 of 10: intrinsic-function failures are not reported as intrinsic failures.
AWS: an intrinsic that cannot be evaluated fails the execution with cause `An error occurred while executing the state '<StateName>' (entered at the event id #<N>). There was an error while evaluating the intrinsic function: <the intrinsic call as written>. <the per-intrinsic detail>` — e.g. `Invalid arguments in States.ArrayGetItem` for a negative index, `The function 'States.Format('{}-{}', $.a, $.missing)' had the following error: The JsonPath argument for the field '$.missing' could not be found in the input '<input JSON>'` for States.Format, and the parser detail for States.StringToJson.
Emulator: emits its own one-liners with no wrapper and no intrinsic name, e.g. `'States.ArrayGetItem($.grades, -1)': the index must not be negative`, `'States.StringToJson($.bad)': the argument is not JSON: key must be a string at line 1 column 2`, and for States.Format a bare `Invalid path '$.missing' : No results for path: $.missing`.
Why: the failing intrinsic is no longer identifiable from the cause, and ErrorPath/CausePath machines built on it mis-report.
Fix: the intrinsic-function evaluator's error reporting.
Traces: stepfunctions.array-get-item-negative-index-rejected, stepfunctions.deep-sfn-language-fail-error-path-intrinsic-error-rejected, stepfunctions.deep-sfn-language-format-absent-path-argument-rejected, stepfunctions.deep-sfn-language-string-to-json-malformed-rejected

Cause 5 of 10: States.DataLimitExceeded cause text is emulator prose.
AWS: an over-limit result fails with cause `The state/task '<name>' returned a result with a size exceeding the maximum number of bytes service limit.` — fixed text; `<name>` is the state name for a Pass/Parallel-branch state and the invoked resource ARN for a Task.
Emulator: `state 'B' produced a document of at least 264204 bytes, more than the 262144 a state's output may carry` (and the JSONata variant `'<expr>' produces more than the 262144 bytes left of what this may build`), naming the byte counts instead of the documented sentence.
Why: the documented quota message is absent, and the byte counts it prints are run-specific noise a client cannot match.
Fix: the size-limit failure path shared by the Pass/Parallel/JSONata output writers.
Traces: stepfunctions.parallel-branch-and-whole-output-over-256kib-rejected, stepfunctions.pass-result-path-doubling-output-over-256kib-rejected, stepfunctions.sfn-jsonata-pass-state-output-over-256kib-rejected

Cause 6 of 10: a Map given a non-array reports the wrong cause.
AWS: a Map state whose input (or ItemsPath target) is not an array fails States.Runtime with cause `Reference path "$" must point to array.` for the whole-input case, and the equivalent reference-path sentence naming the ItemsPath for the ItemsPath case.
Emulator: `the 'Map' state 'MapState' was given {"not":"an array"}, which is not the array of items a 'Map' state iterates` / `the 'Map' state 'M' was given 5, ...`.
Why: the documented message is absent and the emulator's version echoes the (possibly sensitive) input into the cause.
Fix: the Map state's items-resolution error path.
Traces: stepfunctions.map-non-array-input-fails-states-runtime, stepfunctions.deep-sfn-language-map-items-path-and-task-timing-path-wrong-type-rejected

Cause 7 of 10: Fail CausePath non-string rejection text.
AWS: a Fail state whose CausePath resolves to a non-string fails States.Runtime with cause `An error occurred while executing the state '<StateName>' (entered at the event id #<N>). The value referenced by CausePath must be a string, but the value was: <value>`.
Emulator: `'CausePath' selected 5, and a 'Fail' state's 'Cause' must be a string`.
Why: same class — the documented sentence and the state/event-id wrapper are missing.
Fix: the Fail state's ErrorPath/CausePath resolution.
Traces: stepfunctions.deep-sfn-language-fail-cause-path-non-string-rejected

Cause 8 of 10: JSONata evaluation failures lose AWS's wrapper and field naming.
AWS: a JSONata field that throws fails States.QueryEvaluationError with cause `An error occurred while executing the state '<StateName>' (entered at the event id #<N>). The JSONata expression '<expr>' specified for the field '<Field>' threw an error during evaluation. <jsonata code>: <jsonata message>`; a wrong result type gives `... returned an unexpected result type. Expected '<type>', but was '<actual>' for value: <value>`; and a failing Choice rule names the field as `Choices[0]/Condition`.
Emulator: emits only the bare engine text, e.g. `evaluating '$states.input.x + 1' failed: T2001 @ 16: The left side of the \`+\` operator must evaluate to a number`, and for a Wait Seconds type error `... returned an unexpected type. Expected 'number' but was 'string'.` (wrong words, no `for value:` tail). No state name, no event id, and the field is never named for Choice rules or Arguments.
Why: a client cannot tell which field of which state failed, which is the entire point of the diagnostic.
Fix: the JSONata field-evaluation error wrapper in the interpreter (field path + state/event-id prefix + AWS's two sentence forms).
Traces: stepfunctions.pass-jsonata-runtime-error-emits-evaluation-failed-then-execution-failed, stepfunctions.jsonata-wait-field-runtime-type-and-format-errors-are-catchable, stepfunctions.jsonata-parallel-evaluation-failure-timing-and-catch, stepfunctions.choice-jsonata-evaluation-order-condition-output-assign

Cause 9 of 10: an inapplicable ResultPath is classified States.ResultPathMatchFailure instead of States.ReferencePathConflict.
AWS: when a ResultPath reference path cannot be applied to the state's result (e.g. `$.notExist.notExist` over an array, or a ResultPath over an array element), the execution fails with error `States.ReferencePathConflict`, and the same error appears on the StartSyncExecution response.
Emulator: fails with error `States.ResultPathMatchFailure` and cause `'$.notExist.notExist' names a member of [1,2], which is not a JSON object`.
Why: Catch/Retry rules written for States.ReferencePathConflict never match, so an execution a client expects to recover fails outright.
Fix: the error classification in the ResultPath application step.
Traces: stepfunctions.catch-resultpath-unresolvable-path-reference-path-conflict, stepfunctions.sync-resultpath-over-array-reference-path-conflict

Cause 10 of 10: an over-long ErrorPath value is rejected instead of trimmed.
AWS: a Fail state's resolved error name longer than the 256-character limit is TRIMMED for DescribeExecution — the first 253 characters plus `...` (256 total) — while the untrimmed value is what the synchronous response and the logs carry. Smithy: SensitiveError @length max 256.
Emulator: ends the execution with error `States.DataLimitExceeded` instead of trimming, so the Fail state's own error never appears.
Why: a client reading DescribeExecution sees a quota error rather than the (trimmed) error the state raised.
Fix: the error-name trimming in the execution-record writer / DescribeExecution serializer.
Traces: stepfunctions.deep-sfn-language-fail-long-error-untrimmed-in-sync-and-logs-trimmed-in-standard-history

## Evidence
All values below are from the replayed checks; names/ids/byte counts are per-run fillers, the fixed text is the requirement.

1) expected `... Invalid path '$.missing' : No results for path: $['missing']`; got `... No results for path: $.missing`. Choice: expected `Invalid path '$.v': The choice state's condition path references an invalid value.`; got `Invalid path '$.v' : ...`. ItemSelector: expected the full `An error occurred while executing the state 'BadMap' (entered at the event id #4). The JSONPath '$$.Map.Item.Invalid' specified for the field 'bad.$' could not be found in the input '<whole context object>'`; got that sentence with no prefix and the input cut at `...sim2real-be…`.
2) `an exactly 262144-byte SendTaskSuccess output ended 'FAILED': 'States.DataLimitExceeded' "state 'T' produced a document of at least 262145 bytes..."`.
3) expected `... The SecondsPath parameter does not reference an input value: $.seconds[0]`; got `... Invalid path '$.seconds[0]' : No results for path: $.seconds[0]. The 'SecondsPath' field selected no value.`
4) expected `... There was an error while evaluating the intrinsic function: States.ArrayGetItem($.grades, -1). Invalid arguments in States.ArrayGetItem`; got `'States.ArrayGetItem($.grades, -1)': the index must not be negative`.
5) expected `The state/task 'B' returned a result with a size exceeding the maximum number of bytes service limit.`; got `state 'B' produced a document of at least 264204 bytes, more than the 262144 a state's output may carry`.
6) expected `Reference path "$" must point to array.`; got `the 'Map' state 'MapState' was given {"not":"an array"}, which is not the array of items a 'Map' state iterates`.
7) expected `An error occurred while executing the state 'F' (entered at the event id #2). The value referenced by CausePath must be a string, but the value was: 5`; got `'CausePath' selected 5, and a 'Fail' state's 'Cause' must be a string`.
8) expected `An error occurred while executing the state 'W' (entered at the event id #2). The JSONata expression '$states.input.n' specified for the field 'Seconds' returned an unexpected result type. Expected 'number', but was 'string' for value: "10"`; got `... returned an unexpected type. Expected 'number' but was 'string'.` Pass/Output: expected `... The JSONata expression '$states.input.x + 1' specified for the field 'Output' threw an error during evaluation. T2001: The left side of the "+" operator must evaluate to a number`; got `evaluating '$states.input.x + 1' failed: T2001 @ 16: ...`.
9) expected error `States.ReferencePathConflict`; got `States.ResultPathMatchFailure`.
10) expected a 300-char ErrorPath trimmed to 253 chars + `...`; got error `States.DataLimitExceeded`.

## Steps to Reproduce
Replay the behavioral suite and re-run the listed check ids, e.g.:
1. CreateStateMachine with a Pass state whose InputPath is `$.missing`, StartExecution with `{}`, DescribeExecution — compare `cause` to AWS's bracket-notation text.
2. Register an activity task and SendTaskSuccess an output of exactly 262144 bytes — the execution must continue.
3. A Wait state with `SecondsPath: $.seconds[0]` over an input without that index.
4. A Pass state with `ResultSelector`/`Result` using `States.ArrayGetItem($.grades, -1)`.
5. A Parallel branch state producing >262144 bytes.
6. A Map state whose input is `{"not":"an array"}`.
7. A Fail state with `CausePath: $.c` and input `{"c": 5}`.
8. A JSONata Wait with `Seconds: {% $states.input.n %}` and input `{"n":"10"}`; a JSONata Pass with `Output: {% $states.input.x + 1 %}` over a string x.
9. A Task/Pass with `ResultPath: $.notExist.notExist` over input `[1,2]`.
10. A Fail state with an ErrorPath resolving to a 300-character string; read DescribeExecution.
