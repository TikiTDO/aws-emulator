Issue-Source: trace-sweep-divergence
Issue-Trace: sfn-validation-reason-labeled-arn-and-completed-map-run
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sfn-validation-reason-labeled-arn-and-completed-map-run stepfunctions-alias-invalid-routing-weights
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: states|UpdateMapRun|4xx->4xx|$.message
Issue-Key: states|CreateStateMachineAlias|4xx->4xx|$.message
Issue-Commit: 07e872c7593558b582731e73c173338530c40226

# states: UpdateMapRun and CreateStateMachineAlias 400 refusals drop the identifying value from AWS's message (2 causes)

> _Note: this issue was identified against the code base at commit `07e872c7593558b582731e73c173338530c40226`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateMapRun

## Description
This issue bundles TWO distinct net-new wire causes on service `states` (Step Functions, sdkId SFN) that share one fault: a 400-class ValidationException is raised with the correct HTTP status and error code, but the emulator substitutes its own prose for AWS's message AND drops the one variable value in that message that identifies WHICH resource or WHICH request value was rejected. The error code is shared by many rules in this service, so the message text is the only part of the refusal that says what was wrong. BOTH causes must be closed before submitting; re-replay the fixture#step bindings in Issue-Trace/Issue-Step and in trace_ids.

Related (does NOT cover these): the open issue `20261029T171911Z-states-step-functions-refusal-messages-are-emulator-authored-instead.md` describes the same family of wording defects, but its replay pointer carries only the CreateStateMachine and UpdateStateMachine keys (Issue-Key: states|CreateStateMachine|4xx->4xx|$.message and states|UpdateStateMachine|4xx->4xx|$.message) and its Issue-Trace-Ids list bare fixture ids with no step half, so neither of the two steps below is owned by it or exercised by its replay. They are filed here with their own fixture#step bindings.

=== Cause 1 of 2: UpdateMapRun's completed-Map-Run refusal omits the Map Run ARN ===
AWS: Smithy models UpdateMapRun input {mapRunArn (required, LongArn, @length 1..2000), maxConcurrency (@range min 0), toleratedFailurePercentage (@range 0..100), toleratedFailureCount (@range min 0)} with errors InvalidArn (400), ResourceNotFound (404) and ValidationException (400, message + reason). The model does not state the message text, so the template is recorded behavior only: HTTP 400, ValidationException, message `Cannot update completed MapRun <mapRunArn>`. Fixed text: `Cannot update completed MapRun ` (note: `MapRun`, one word, no article). Variable part: the Map Run ARN, taken from the request's mapRunArn member.
Emulator: answers 400 with message `Cannot update a completed Map Run` — reworded (`a completed Map Run`, two words with an article) and carrying no ARN at all.
Why: a client that updates many Map Runs concurrently (a fan-out tuner raising maxConcurrency across a fleet of Map Runs) cannot tell from this refusal WHICH Map Run had already reached a terminal state, so it can neither skip that one nor attribute the failure; and any client or test asserting AWS's sentence fails on the rewording.
Fix: UpdateMapRun's handler/validation path in the Step Functions service crate — emit the ValidationException with AWS's fixed text and interpolate the requested mapRunArn.
Traces: sfn-validation-reason-labeled-arn-and-completed-map-run#task#8 (UpdateMapRun, expected 400, actual 400, message only).

=== Cause 2 of 2: CreateStateMachineAlias's routing-weight refusal is reworded and reports the computed sum instead of the received weights ===
AWS: Smithy models CreateStateMachineAlias input {name (required, CharacterRestrictedName @length 1..80, @pattern ^(?=.*[a-zA-Z_\-\.])[a-zA-Z0-9_\-\.]+$), routingConfiguration (required, @length 1..2), description}, with ValidationException (400) among its errors; RoutingConfigurationListItem.weight is a VersionWeight @range 0..100 whose documentation states "The sum of the weights in the routing configuration must be equal to 100." The message template is recorded behavior only: HTTP 400, message `Sum of routing configuration weights must equal 100. Received: [<weights>]`. Fixed text: `Sum of routing configuration weights must equal 100. Received: [` and the closing `]`. Variable part: the individual weight values from the request's routingConfiguration, in request order, comma-separated — NOT their sum.
Emulator: answers 400 with `the weights of a routing configuration must add up to 100, got <sum>` — emulator-authored wording, and it renders the computed SUM where AWS lists each received weight.
Why: routingConfiguration accepts up to two entries, so the bracketed per-entry list is what tells the caller which entry carries the wrong weight; a single sum cannot distinguish [40,50] from [90]. Clients and tests matching AWS's sentence also fail outright.
Fix: CreateStateMachineAlias (and the same check on UpdateStateMachineAlias) validation message construction — render AWS's fixed sentence with the bracketed list of the weights as received.
Traces: stepfunctions-alias-invalid-routing-weights#task#5 (CreateStateMachineAlias, expected 400, actual 400, message only).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, 537 steps, 493 passed). Both divergence clusters are verdict=mismatch, service=states, expected_status == actual_status (400), $.message only, and both carry an EMPTY owned_by and an empty stub_path — i.e. no filed issue carries their replay key.

Cause 1 — sfn-validation-reason-labeled-arn-and-completed-map-run#task#8, UpdateMapRun, divergence_key `service=states|UpdateMapRun|status=400->400|keys=$.message`:
  $.message: value_mismatch (expected 'Cannot update completed MapRun arn:aws:states:us-west-2:000000000000:mapRun:sim2real-probe-sfn-map-99bc17e1-477e-4b9e-9ec0-ad79f304b156/M:1322e1f6-9176-4b1f-9902-48b9b3459f5d', got 'Cannot update a completed Map Run')
Fixed text of the requirement: `Cannot update completed MapRun `. The ARN in the recording is this run's generated Map Run ARN (account, region, state-machine name suffix, map-state label and run uuid all vary per run) and is NOT part of the template — the requirement is that the mapRunArn from the request is interpolated.

Cause 2 — stepfunctions-alias-invalid-routing-weights#task#5, CreateStateMachineAlias, divergence_key `service=states|CreateStateMachineAlias|status=400->400|keys=$.message`:
  $.message: value_mismatch (expected 'Sum of routing configuration weights must equal 100. Received: [42]', got 'the weights of a routing configuration must add up to 100, got 42')
Fixed text of the requirement: `Sum of routing configuration weights must equal 100. Received: [` ... `]`. The value 42 is this fixture's request weight filling the variable slot; the requirement is that the received weights are listed per entry, not summed.

## Steps to Reproduce
Cause 1 (UpdateMapRun):
1. sfn:CreateStateMachine with a DISTRIBUTED Map state (ItemProcessor.ProcessorConfig.Mode=DISTRIBUTED).
2. sfn:StartExecution and let the execution reach a terminal state, so its Map Run completes.
3. sfn:ListMapRuns(executionArn=<it>) to read the mapRunArn.
4. sfn:UpdateMapRun(mapRunArn=<that arn>, maxConcurrency=1).
Expect: HTTP 400 ValidationException, message `Cannot update completed MapRun <the mapRunArn sent in the request>`.
Observed: HTTP 400 with `Cannot update a completed Map Run` — reworded and with no ARN.
Replay: fixture sfn-validation-reason-labeled-arn-and-completed-map-run, step task#8.

Cause 2 (CreateStateMachineAlias):
1. sfn:CreateStateMachine(...) then sfn:PublishStateMachineVersion to get a version ARN.
2. sfn:CreateStateMachineAlias(name=<alias>, routingConfiguration=[{stateMachineVersionArn:<version arn>, weight:42}]).
Expect: HTTP 400, message `Sum of routing configuration weights must equal 100. Received: [42]` (the bracketed list is the weights as received, in request order).
Observed: HTTP 400 with `the weights of a routing configuration must add up to 100, got 42`.
Positive control: a two-entry routingConfiguration of weights 40 and 50 must report `Received: [40,50]`, which the summing form cannot produce.
Replay: fixture stepfunctions-alias-invalid-routing-weights, step task#5.
