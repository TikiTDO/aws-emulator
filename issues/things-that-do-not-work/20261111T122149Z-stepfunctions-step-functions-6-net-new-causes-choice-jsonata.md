Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.choice-jsonata-evaluation-order-condition-output-assign stepfunctions.deep-sfn-language-map-items-path-and-task-timing-path-wrong-type-rejected stepfunctions.deep-sfn-language-string-to-json-malformed-rejected stepfunctions.jsonata-wait-field-runtime-type-and-format-errors-are-catchable stepfunctions.map-itemselector-invalid-context-path-rejected stepfunctions.redrive-of-jsonata-parallel-evaluation-failure-fails-again
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.choice-jsonata-evaluation-order-condition-output-assign|_assert_evaluation_error#13|AssertionError|assert f"specified for the field '{location}'" in cause, (
Issue-Key: stepfunctions.deep-sfn-language-map-items-path-and-task-timing-path-wrong-type-rejected|_check_task_heartbeat_seconds_path#3|AssertionError|assert bad.get("cause") == (
Issue-Key: stepfunctions.deep-sfn-language-string-to-json-malformed-rejected|run#11|AssertionError|assert bad.get("cause") == expected_cause, (
Issue-Key: stepfunctions.jsonata-wait-field-runtime-type-and-format-errors-are-catchable|run#17|AssertionError|assert exe.get("cause") == (
Issue-Key: stepfunctions.map-itemselector-invalid-context-path-rejected|run#22|AssertionError|assert bad.get("cause") == expected_cause, (
Issue-Key: stepfunctions.redrive-of-jsonata-parallel-evaluation-failure-fails-again|run#13|AssertionError|assert second.get("cause") == original_cause, (
Issue-Commit: b088d29fc6cbae0dbe15d6c5c29ce12fa9f3ea19

# Step Functions: 6 net-new causes — Choice JSONata Output/Assign evaluation order, timing-path and Wait/Timestamp and StringToJson cause wording (input leaked), context-object member order, and redrive renumbering the entered event id

> _Note: this issue was identified against the code base at commit `b088d29fc6cbae0dbe15d6c5c29ce12fa9f3ea19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeExecution

## Description
Six distinct root causes in the Step Functions interpreter's failure reporting. All six are reached through DescribeExecution's `error`/`cause` (Smithy: com.amazonaws.sfn#DescribeExecutionOutput.error -> SensitiveError @length 0..256, .cause -> SensitiveCause @length 0..32768; the per-state equivalent is EvaluationFailedEventDetails{error,cause,location,state}). A fixer must close all six before submitting.

--- Cause 1 of 6: a JSONata Choice rule evaluates Assign before Output, so the wrong field is named.
AWS: on a matched Choice rule the fields are evaluated in a fixed order — Condition, then that rule's Output, then that rule's Assign; on the Default path there is no per-rule Output/Assign, so the state's own Output is evaluated before its own Assign. The FIRST field to fail is the one reported, as States.QueryEvaluationError with a cause naming that field's location ('Choices[<n>]/Condition', 'Choices[<n>]/Output', 'Choices[<n>]/Assign/<key>', or the state-level 'Output' / 'Assign/<key>'), quoting the offending expression, and ending in the JSONata engine's own diagnostic. Fixed text of the cause: "An error occurred while executing the state '<state>' (entered at the event id #<id>). The JSONata expression '<expr>' specified for the field '<location>' threw an error during evaluation. <jsonata diagnostic>"; the state name, event id, expression, location and diagnostic are the variable parts. (Reference: AWS docs, Choice state; recorded behavior for the ordering.)
Emulator: with the rule's Condition good and BOTH its Output and its Assign bad, the emulator reports location 'Choices[0]/Assign/z' and quotes the Assign expression, i.e. it evaluated Assign first. Output is never attempted.
Why: a client cannot tell which field is broken. The reported location and expression point at the wrong field, so an operator debugging a JSONata Choice chases the Assign while the real defect is in the Output — and a test asserting the documented precedence fails against the emulator.
Fix: the Choice state's JSONata field-evaluation order in the interpreter (evaluate Condition, then the matched rule's Output, then its Assign; on the Default path the state's own Output before its own Assign) and the EvaluationFailed location it reports.
Traces: stepfunctions.choice-jsonata-evaluation-order-condition-output-assign

--- Cause 2 of 6: HeartbeatSecondsPath / TimeoutSecondsPath parse-failure cause uses the emulator's own wording.
AWS: when a Task's HeartbeatSecondsPath or TimeoutSecondsPath resolves to a value that is not a whole number, the execution fails States.Runtime before the task is dispatched, with the cause "An error occurred while executing the state '<state>' (entered at the event id #<id>). The <Field>Path parameter cannot be parsed as a long value: <path> == <value as JSON>". The fixed text is " parameter cannot be parsed as a long value: " and the " == " separator; the variable parts are the state name, the entered event id, which of the two fields it is, the reference path as written in the definition, and the resolved value rendered as JSON. (Recorded behavior only — the sfn Smithy model does not carry ASL cause strings.)
Emulator: the error code States.Runtime is right, but the cause reads "... The value referenced by HeartbeatSecondsPath must be a positive whole number of seconds, but the value was: <value>" — different fixed text, and it drops the reference path entirely, so the message no longer says WHICH path failed.
Why: the error message is part of the contract. A client that matches on AWS's wording (log scrapers, retry classifiers, assertions) does not match, and an operator loses the reference path needed to find the offending field in a large definition. TimeoutSecondsPath has the same defect and the same required wording; the check stopped at HeartbeatSecondsPath, so the TimeoutSecondsPath branch is untested but shares the formatter.
Fix: the Task state's timing-path resolution/validation in the interpreter and the States.Runtime cause formatter it calls, for both HeartbeatSecondsPath and TimeoutSecondsPath.
Traces: stepfunctions.deep-sfn-language-map-items-path-and-task-timing-path-wrong-type-rejected (its Map ItemsPath leg — cause 'Reference path "$.Number" must point to array.' — passes; the timing-path leg is what fails)

--- Cause 3 of 6: the States.StringToJson failure cause leaks the unparsable input instead of redacting the parser source.
AWS: States.StringToJson over text that is not valid JSON fails the execution States.Runtime with "An error occurred while executing the state '<state>' (entered at the event id #<id>). There was an error while evaluating the intrinsic function: States.StringToJson(<arg>). Cannot convert String to Json in States.StringToJson, caused by: <parser diagnostic>\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: <l>, column: <c>]". The trailing source locator is FIXED text: AWS deliberately emits the literal 'REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled)' in place of the document; only the line and column, the parser diagnostic, the state name, the entered event id and the intrinsic's argument expression vary. (Recorded behavior only.)
Emulator: the same execution fails States.Runtime with the correct prefix but ends "\n at [Source: (String)\"<the input text>\"; line: 1, column: 2]" — it substitutes the caller's data for the redaction marker.
Why: two defects at once. The message diverges from AWS's fixed text, and it publishes execution payload content into DescribeExecution.cause — a field whose contents are otherwise sensitive/redacted — so data a customer passed into States.StringToJson is echoed to anyone who can read the execution's failure. A client relying on AWS's redaction treats the cause as non-sensitive and may log or display it.
Fix: the intrinsic-function evaluator's error path for States.StringToJson — the parser-source locator must be emitted as the fixed REDACTED form, never the parsed document.
Traces: stepfunctions.deep-sfn-language-string-to-json-malformed-rejected

--- Cause 4 of 6: a JSONata Wait Timestamp that is a string but not ISO-8601 is reported as a TYPE error instead of a parse error.
AWS: for a JSONata Wait state, a Seconds or Timestamp expression of the wrong JSON type fails States.QueryEvaluationError with "... The JSONata expression '<expr>' specified for the field '<field>' returned an unexpected result type. Expected '<expected type>', but was '<actual type>' for value: <value>". But a Timestamp that IS a string and merely fails to parse is a different message: "An error occurred while executing the state '<state>' (entered at the event id #<id>). The Timestamp field cannot be parsed as an ISO-8601 extended offset date-time format string: <value>". Fixed text: "The Timestamp field cannot be parsed as an ISO-8601 extended offset date-time format string: "; variable: state name, entered event id, and the offending value as written. (Reference: AWS docs, Wait state; recorded behavior for the wording.)
Emulator: for a string Timestamp such as a date with no time or offset, the emulator reports the type-mismatch form instead — "... specified for the field 'Timestamp' returned an unexpected result type. Expected 'an ISO 8601 instant', but was 'string' for value: ..." — inventing the expected-type token 'an ISO 8601 instant', which is not a JSON type and is not a string AWS ever emits. (The Seconds wrong-type and numeric-Timestamp legs of the same check pass, so only the parse-failure branch is wrong.)
Why: the two failure modes are distinguishable at AWS and a client classifying them (bad shape vs bad format) gets the wrong class, and no message match succeeds.
Fix: the Wait state's JSONata field coercion in the interpreter — separate the type check from the ISO-8601 parse of a string Timestamp and emit the parse-failure cause for the latter.
Traces: stepfunctions.jsonata-wait-field-runtime-type-and-format-errors-are-catchable

--- Cause 5 of 6: the context object embedded in an unresolved-JSONPath cause serializes Execution's members in the wrong order.
AWS: when a JSONPath that reads the context object ($$) cannot be resolved, the execution fails States.Runtime with "An error occurred while executing the state '<state>' (entered at the event id #<id>). The JSONPath '<path>' specified for the field '<field>' could not be found in the input '<the full context object as JSON>'". The embedded context object has a fixed shape AND a fixed member order; under Execution the order is Id, Input, StartTime, Name, RoleArn, RedriveCount. The ARNs, names, timestamps and item values inside it are values from the request and from state, not part of the requirement — the ORDER is.
Emulator: every value matches, but Execution is serialized as Id, Input, Name, RoleArn, StartTime, RedriveCount — StartTime is emitted after RoleArn instead of immediately after Input.
Why: the cause is a single string, so a client cannot re-parse-and-compare member-wise; any exact comparison against AWS's rendering fails, and a snapshot of this message recorded against AWS does not match the emulator.
Fix: the context-object serializer used by the States.Runtime unresolved-path cause formatter — emit Execution's members in AWS's order.
Traces: stepfunctions.map-itemselector-invalid-context-path-rejected

--- Cause 6 of 6: redriving an execution renumbers the "entered at the event id #N" in the reproduced cause.
AWS: RedriveExecution on a Standard execution that failed from a deterministic JSONata evaluation error re-evaluates the same expression and fails identically: DescribeExecution reports the SAME error and the SAME cause string as the original attempt, with redriveCount 1 and redriveStatus REDRIVABLE (Smithy: DescribeExecutionOutput.redriveCount/.redriveStatus). Because the redrive does not re-enter the state (the history after ExecutionRedriven is EvaluationFailed, ParallelStateFailed, ExecutionFailed, with no new StateEntered), the "(entered at the event id #<id>)" clause still names the state's ORIGINAL StateEntered event id.
Emulator: the redriven run reproduces the error name and the whole cause except that clause, which is renumbered to an event id from the redrive segment (the original said #2, the redrive said #7). The two causes therefore differ for an expression whose evaluation is deterministic.
Why: a client comparing the pre- and post-redrive failure to decide whether a redrive changed anything sees a difference that AWS does not produce, and the event id in the message no longer points at the StateEntered event a caller would look up in GetExecutionHistory.
Fix: the redrive path's cause formatter — resolve "entered at the event id" from the state's StateEntered event for the attempt being reported, which on a redrive that does not re-enter the state is the original entry event, not the current event counter.
Traces: stepfunctions.redrive-of-jsonata-parallel-evaluation-failure-fails-again

## Evidence
Cause 1 — stepfunctions.choice-jsonata-evaluation-order-condition-output-assign
  Expected: cause naming field 'Choices[0]/Output' and quoting the Output expression.
  Actual: "An error occurred while executing the state 'C' (entered at the event id #2). The JSONata expression '$states.input.assignBad ? ($states.input.a + 1) : 'rule-assign'' specified for the field 'Choices[0]/Assign/z' threw an error during evaluation. T2001: The left side of the \"+\" operator must evaluate to a number"
  (error code States.QueryEvaluationError is correct in both.)

Cause 2 — stepfunctions.deep-sfn-language-map-items-path-and-task-timing-path-wrong-type-rejected
  Expected: "An error occurred while executing the state 'T' (entered at the event id #2). The HeartbeatSecondsPath parameter cannot be parsed as a long value: $.hb == \"not-a-number\""
  Actual:   "An error occurred while executing the state 'T' (entered at the event id #2). The value referenced by HeartbeatSecondsPath must be a positive whole number of seconds, but the value was: \"not-a-number\""
  (status FAILED and error States.Runtime are correct; only the cause text diverges, and the reference path '$.hb' is missing.)

Cause 3 — stepfunctions.deep-sfn-language-string-to-json-malformed-rejected
  Expected tail: "... caused by: Unexpected character ('n' (code 110)): was expecting double-quote to start field name\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: 1, column: 2]"
  Actual tail:   "... caused by: Unexpected character ('n' (code 110)): was expecting double-quote to start field name\n at [Source: (String)\"{not valid json\"; line: 1, column: 2]"
  The emulator substituted the caller's input text for the fixed REDACTED locator.

Cause 4 — stepfunctions.jsonata-wait-field-runtime-type-and-format-errors-are-catchable
  Expected: "An error occurred while executing the state 'W' (entered at the event id #2). The Timestamp field cannot be parsed as an ISO-8601 extended offset date-time format string: <value>"
  Actual:   "An error occurred while executing the state 'W' (entered at the event id #2). The JSONata expression '$states.input.t' specified for the field 'Timestamp' returned an unexpected result type. Expected 'an ISO 8601 instant', but was 'string' for value: \"2024-11-22\""

Cause 5 — stepfunctions.map-itemselector-invalid-context-path-rejected
  Expected Execution member order inside the embedded context object: "Id", "Input", "StartTime", "Name", "RoleArn", "RedriveCount".
  Actual:   "Id", "Input", "Name", "RoleArn", "StartTime", "RedriveCount".
  Everything else in the cause (the prefix, the JSONPath '$$.Map.Item.Invalid', the field 'bad.$', the StateMachine/State/Map members) matched.

Cause 6 — stepfunctions.redrive-of-jsonata-parallel-evaluation-failure-fails-again
  First run cause:    "An error occurred while executing the state 'Fan' (entered at the event id #2). The JSONata expression '$number('not-a-number')' specified for the field 'Arguments' threw an error during evaluation. D3030: Unable to cast value to a number: \"not-a-number\""
  Redriven run cause: identical except "(entered at the event id #7)".
  AWS requires the two causes to be equal.

## Steps to Reproduce
All six use a STANDARD state machine and an execution role whose trust policy allows states.amazonaws.com to sts:AssumeRole.

Cause 1: CreateStateMachine with QueryLanguage JSONata and a Choice state 'C' whose single rule has Condition, Output and Assign{z} all JSONata expressions that can be made to throw (e.g. "{% $states.input.outBad ? ($states.input.o + 1) : 'rule-output' %}"), Default to a Pass. StartExecution with the Condition good and both Output and Assign bad. DescribeExecution: expect error States.QueryEvaluationError with cause naming 'Choices[0]/Output'; the emulator names 'Choices[0]/Assign/z'.

Cause 2: CreateActivity; CreateStateMachine with a Task 'T' whose Resource is that activity ARN, HeartbeatSecondsPath "$.hb", TimeoutSeconds 120. StartExecution with input {"hb": "not-a-number"}. DescribeExecution: error States.Runtime, cause must read "The HeartbeatSecondsPath parameter cannot be parsed as a long value: $.hb == \"not-a-number\"". Repeat with a Task using TimeoutSecondsPath "$.to" and input {"to": "nope"}.

Cause 3: CreateStateMachine with Pass state 'Stj' whose Parameters are {"o.$": "States.StringToJson($.bad)"}. StartExecution with input {"bad": "{not valid json"}. DescribeExecution: the cause's source locator must be the fixed REDACTED form; the emulator embeds the input string.

Cause 4: CreateStateMachine with QueryLanguage JSONata and Wait state 'W' with Timestamp "{% $states.input.t %}". StartExecution with input {"t": "2024-11-22"} (a string that is not an ISO-8601 extended offset date-time). DescribeExecution: error States.QueryEvaluationError, cause must be the "Timestamp field cannot be parsed as an ISO-8601 extended offset date-time format string" form.

Cause 5: CreateStateMachine with a Map state 'BadMap' whose ItemSelector contains a field "bad.$": "$$.Map.Item.Invalid". StartExecution with input {"mode":"bad","items":[10,20,30]}. DescribeExecution: compare the embedded context object's Execution member order against Id, Input, StartTime, Name, RoleArn, RedriveCount.

Cause 6: CreateStateMachine with QueryLanguage JSONata and a Parallel state 'Fan' whose Arguments is "{% $number('not-a-number') %}" and one Pass branch. StartExecution, wait for FAILED, then RedriveExecution on the same executionArn and wait for FAILED again. DescribeExecution after the redrive: cause must be byte-identical to the first failure's cause; the emulator changes the "entered at the event id #N" clause.
