Issue-Source: trace-sweep-divergence
Issue-Trace: devicefarm-device-pool-rule-and-reference-errors
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: devicefarm-device-pool-rule-and-reference-errors devicefarm-upload-lifecycle
Issue-Shape-Id: com.amazonaws.devicefarm#DeviceFarm_20150623
Issue-Key: devicefarm|CreateDevicePool|4xx->2xx|$.__type
Issue-Key: devicefarm|CreateDevicePool|4xx->2xx|$.message
Issue-Key: devicefarm|CreateDevicePool|4xx->2xx|status
Issue-Key: devicefarm|CreateDevicePool|2xx->4xx|$.devicePool
Issue-Key: devicefarm|CreateDevicePool|2xx->4xx|status
Issue-Key: devicefarm|GetDevicePool|4xx->4xx|$.__type
Issue-Key: devicefarm|GetDevicePool|4xx->4xx|$.message
Issue-Key: devicefarm|GetUpload|4xx->2xx|$.__type
Issue-Key: devicefarm|GetUpload|4xx->2xx|$.message
Issue-Key: devicefarm|GetUpload|4xx->2xx|status
Issue-Commit: 0e1e74bc7be9f02744aed6259c3be4e3b3293a9a

# DeviceFarm device-pool and upload reference checks diverge: wrong status, error class and message

> _Note: this issue was identified against the code base at commit `0e1e74bc7be9f02744aed6259c3be4e3b3293a9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDevicePool

## Description
The devicefarm emulator decides device-pool and upload reference/validation failures wrongly in four distinct ways: a rule the service refuses is accepted, a create the service serves is refused, a malformed ARN is reported as a missing resource instead of a bad argument, and a deleted upload still reads back as live. FOUR CAUSES, all must be closed before this issue is submitted.

Cause 1 of 4: CreateDevicePool accepts a device-pool rule whose operand is not a quoted string instead of refusing with ArgumentException (fixture devicefarm-device-pool-rule-and-reference-errors, step task#2).
AWS: Smithy com.amazonaws.devicefarm#CreateDevicePool takes projectArn (AmazonResourceName, required, @length 32..1011, pattern ^arn:aws:devicefarm:.+$), name (Name, required, @length 0..256), rules (Rules, required) and optional description/maxDevices; it models ArgumentException (errorKind client, HTTP 400, doc 'An invalid argument was specified.'). Each Rule is {attribute: DeviceAttribute enum (ARN, PLATFORM, FORM_FACTOR, MANUFACTURER, APPIUM_VERSION, OS_VERSION, MODEL, AVAILABILITY, FLEET_TYPE, INSTANCE_ARN, INSTANCE_LABELS, REMOTE_ACCESS_ENABLED, REMOTE_DEBUG_ENABLED), operator: RuleOperator enum (EQUALS, LESS_THAN, LESS_THAN_OR_EQUALS, GREATER_THAN, GREATER_THAN_OR_EQUALS, IN, NOT_IN, CONTAINS), value: String}, and the model's own Rule documentation states the operand is 'The rule's stringified attribute. For example, specify the value as "\"abc\""' — i.e. the value member must carry a JSON-quoted string (a JSON array of quoted strings for IN/NOT_IN). A rule whose operand is bare text fails with HTTP 400 and awsJson error __type 'ArgumentException'. The recorded message's FIXED text is 'Invalid request, please check the arn and the device pool rule. Parse error for attribute operand: ' + the offending operand + '. Rule requires a String with \" on both sides for example: \"ANDROID\", \"4.4.2\", \"PHONE\", \"iPad 5th Gen\".' — the only variable part is the operand value echoed from the request's rule (in this recording the bare word ANDROID); the four quoted examples are part of the fixed text, not of this request.
Emulator: answers HTTP 200 and mints the pool; $.__type and $.message are absent entirely (expected __type 'ArgumentException' and the message above). src/ops/create_device_pool.rs validates only the projectArn's ARN form (lines 165-170), the project's presence (176-184) and the store's pool quota (188-198), then stores input.rules verbatim (line 213) — nothing inspects a Rule's attribute/operator/value at all.
Why: a client that makes the documented mistake (value 'ANDROID' instead of '"ANDROID"') is told its pool was created. Against AWS the call fails and the client fixes the quoting; against the emulator it gets a pool whose rules select nothing, and any test asserting the ArgumentException for a malformed rule fails. It is also a wrong-success that leaves a pool in state the real service never created.
Fix: CreateDevicePool's input validation, in its handler before anything is minted — parse every Rule.value for its attribute/operator and refuse an unparseable operand with ArgumentException carrying AWS's message template. Shared with UpdateDevicePool, so the rule parser belongs beside the other shared helpers in src/state.rs.
Traces: devicefarm-device-pool-rule-and-reference-errors#task#2 (CreateDevicePool, expected 400 ArgumentException, got 200).

Cause 2 of 4: CreateDevicePool refuses a pool whose projectArn names a project the store does not hold, instead of serving the create (fixture devicefarm-device-pool-rule-and-reference-errors, step task#3).
AWS: recorded behavior only — the model declares NotFoundException for CreateDevicePool but fixes nothing about when it fires, and the recording shows this step answering HTTP 200 with devicePool { arn minted under the project id named by the request's projectArn, name as supplied, rules exactly the (empty) list supplied, type 'PRIVATE' }. So the requirement for this step is a 200 carrying the created pool, with the pool's identity derived from the request's projectArn; the specific arn/name values in the recording (project id sim2real-probe-df-missing-..., pool guid, name sim2real-probe-df-orphan-pool-...) are this run's generated ids filling that template and are not requirements. Hypothesis: a project-scoped create is gated on the projectArn's FORM rather than on the project record still being present — consistent with device pools outliving their project (the emulator's own GetDevicePool doc notes a pool still reads back after its project is deleted).
Emulator: answers HTTP 400 with no $.devicePool member. For a well-formed project ARN the only 400 branch reachable is the project-existence gate in src/ops/create_device_pool.rs:176-184, which returns NotFoundException with 'No project was found for arn <projectArn>' (src/state.rs:611-613).
Why: the client cannot create the pool at all, so every downstream step that needs the pool's ARN (GetDevicePool, ScheduleRun, UpdateDevicePool) has nothing to address. A harness that creates pools against a project whose record the emulator has dropped — or whose project was deleted — aborts on a call AWS serves, which reads as a broken account rather than as an emulator gap.
Fix: the reference check in CreateDevicePool's handler (src/ops/create_device_pool.rs:176-184) must stop gating creation on the project record's presence and keep only the ARN-form judgement, minting the pool under the project id carried by the request's projectArn.
Traces: devicefarm-device-pool-rule-and-reference-errors#task#3 (CreateDevicePool, expected 200 with devicePool, got 400).

Cause 3 of 4: GetDevicePool reports a malformed device-pool ARN as NotFoundException instead of ArgumentException (fixture devicefarm-device-pool-rule-and-reference-errors, step task#4).
AWS: Smithy com.amazonaws.devicefarm#GetDevicePool takes arn (AmazonResourceName, required, @length 32..1011, pattern ^arn:aws:devicefarm:.+$) and models BOTH ArgumentException and NotFoundException (both errorKind client, HTTP 400). A devicepool ARN's resource id is two-part, '<project-id>/<pool-id>'; an ARN whose devicepool resource id does not have that form is a bad argument rather than a missing pool, and fails with HTTP 400, __type 'ArgumentException' and a message whose fixed text is 'Invalid arn ' followed by the arn member echoed verbatim from the request. The particular ARN in the recording (a devicepool ARN whose resource id is a single segment with no '/') is this run's value filling that template.
Emulator: answers HTTP 400 but with __type 'NotFoundException' and message 'No device pool was found for arn <arn>' (expected __type 'ArgumentException', message 'Invalid arn <arn>'). src/state.rs devicefarm_resource_id (lines 555-579) accepts any seven-field ARN of type 'devicepool' with a non-empty resource id and never requires the '<project-id>/<pool-id>' split, so src/ops/get_device_pool.rs:154 passes a single-segment id through its form check, the store lookup misses (lines 162-172) and the handler reports absence. Separately, when that form check DOES refuse, it words the refusal '<arn> is not a device pool ARN' (get_device_pool.rs:156), which is not AWS's fixed text 'Invalid arn <arn>' either — so both the classification and the ArgumentException wording must change for this step to pass.
Why: the two errors mean opposite things to a client. ArgumentException says the ARN itself is wrong — fix the input, never retry. NotFoundException says the pool is gone — recreate it, or keep polling until it appears. Reporting a typo'd ARN as 'not found' sends clients into recreate/poll loops against an ARN that can never resolve, and masks the actual input bug.
Fix: the shared ARN-form validation in src/state.rs (devicefarm_resource_id / is_device_pool_arn) must require the two-segment resource id for project-scoped types, and GetDevicePool's ArgumentException branch must carry AWS's 'Invalid arn <arn>' message. Validation plus the handler's error construction; the success serialization already matches.
Traces: devicefarm-device-pool-rule-and-reference-errors#task#4 (GetDevicePool, 400 vs 400, __type and message value_mismatch).

Cause 4 of 4: GetUpload returns 200 for an upload that has been deleted instead of the modeled NotFoundException (fixture devicefarm-upload-lifecycle, step task#5).
AWS: Smithy com.amazonaws.devicefarm#GetUpload takes arn (required, @length 32..1011, pattern ^arn:aws:devicefarm:.+$), outputs upload (Upload) and models NotFoundException (errorKind client, HTTP 400, doc 'The specified entity was not found.'); DeleteUpload removes the upload the arn names. After a successful DeleteUpload, GetUpload on that ARN fails with HTTP 400, __type 'NotFoundException' and a message whose fixed text is 'No upload found for arn ' — note 'found', with NO 'was' — followed by the request's own arn member. The ARN in the recording is this environment's placeholder account and ids filling that template, not a requirement.
Emulator: answers HTTP 200 with a live upload body; $.__type and $.message are absent (expected __type 'NotFoundException' and the message above). The record is still present because DeleteUpload is an unimplemented scaffold that answers 501 and removes nothing (src/ops/delete_upload.rs:152 — that operation's own blocked step is already filed as an unimplemented-operation issue; this cause is GetUpload's wrong answer, which is not), so src/ops/get_upload.rs:161 still finds the upload. Secondly, when GetUpload does miss it words the failure 'No upload was found for arn <arn>' via the service-wide helper src/state.rs:611-613 (arn_not_found_message = 'No {entity} was found for arn {arn}'), which does not match AWS's fixed text 'No upload found for arn ' — so implementing deletion alone converts this step into a message-wording mismatch. Both must be corrected. The same wording class is noted for GetNetworkProfile in open issue 20261030T150856Z-devicefarm-getnetworkprofile-serves-stale-network-profile-state-after-updatenetworkprofile.md, which covers network profiles only and does not own this step.
Why: a deleted upload that still reads back as live is a wrong-success. Cleanup verification passes while the upload persists, teardown loops that poll for NotFoundException never terminate, and a client branching on NotFoundException to decide 'upload again vs reuse' reuses an upload the caller believes it destroyed — then schedules a run against content that is gone. A client matching on the message text to tell a deleted upload from a bad ARN also misclassifies the failure when the wording differs.
Fix: DeleteUpload's handler must remove the record from the service state store (state.uploads) and refuse an unknown ARN with NotFoundException itself; and GetUpload's not-found branch must word its message with AWS's fixed text 'No upload found for arn ' + the request's arn, which means the per-entity wording cannot keep coming from the one 'No {entity} was found for arn {arn}' helper. Handler/state plus the error-message construction.
Traces: devicefarm-upload-lifecycle#task#5 (GetUpload, expected 400 NotFoundException, got 200).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, 44 steps: 16 passed, 15 mismatched, 13 blocked, 6 divergence clusters, score 36.4%). All four causes below are mismatch clusters with no stub_path and no owning issue.

Cause 1 — fixture devicefarm-device-pool-rule-and-reference-errors, step task#2, operation CreateDevicePool, verdict mismatch, expected_status 400 / actual_status 200:
  detail: "status: expected 400, got 200\n$.__type: missing (expected 'ArgumentException', got None)\n$.message: missing (expected 'Invalid request, please check the arn and the device pool rule. Parse error for attribute operand: ANDROID. Rule requires a String with \\\" on both sides for example: \\\"ANDROID\\\", \\\"4.4.2\\\", \\\"PHONE\\\", \\\"iPad 5th Gen\\\".', got None)"
  divergence_key: service=devicefarm|CreateDevicePool|status=400->200|keys=$.__type,$.message,status
  Expected: HTTP 400, __type 'ArgumentException', message = fixed text 'Invalid request, please check the arn and the device pool rule. Parse error for attribute operand: ' + the request's unparseable operand + '. Rule requires a String with \" on both sides for example: \"ANDROID\", \"4.4.2\", \"PHONE\", \"iPad 5th Gen\".'  Actual: HTTP 200 with a created devicePool.

Cause 2 — fixture devicefarm-device-pool-rule-and-reference-errors, step task#3, operation CreateDevicePool, verdict mismatch, expected_status 200 / actual_status 400:
  detail: "status: expected 200, got 400\n$.devicePool: missing (expected {'arn': 'arn:aws:devicefarm:us-west-2:000000000000:devicepool:sim2real-probe-df-missing-d666f77d/e460bffe-6c66-4c80-8531-0981b7ac7e97', 'name': 'sim2real-probe-df-orphan-pool-dce0d385', 'rules': [], 'type': 'PRIVATE'}, got None)"
  divergence_key: service=devicefarm|CreateDevicePool|status=200->400|keys=$.devicePool,status
  Expected: HTTP 200 with devicePool minted under the project id of the request's projectArn, name and rules as supplied, type 'PRIVATE' (the recorded arn/name strings are this run's generated ids). Actual: HTTP 400 with no devicePool.

Cause 3 — fixture devicefarm-device-pool-rule-and-reference-errors, step task#4, operation GetDevicePool, verdict mismatch, expected_status 400 / actual_status 400:
  detail: "$.__type: value_mismatch (expected 'ArgumentException', got 'NotFoundException')\n$.message: value_mismatch (expected 'Invalid arn arn:aws:devicefarm:us-west-2:000000000000:devicepool:sim2real-probe-df-missing-d666f77d-not-a-valid-pool-arn-format', got 'No device pool was found for arn arn:aws:devicefarm:us-west-2:000000000000:devicepool:sim2real-probe-df-missing-d666f77d-not-a-valid-pool-arn-format')"
  divergence_key: service=devicefarm|GetDevicePool|status=400->400|keys=$.__type,$.message
  Expected: __type 'ArgumentException', message fixed text 'Invalid arn ' + the request's arn. Actual: __type 'NotFoundException', message 'No device pool was found for arn ' + the request's arn. The ARN's devicepool resource id is a single segment with no '/<pool-id>'.

Cause 4 — fixture devicefarm-upload-lifecycle, step task#5, operation GetUpload, verdict mismatch, expected_status 400 / actual_status 200:
  detail: "status: expected 400, got 200\n$.__type: missing (expected 'NotFoundException', got None)\n$.message: missing (expected 'No upload found for arn arn:aws:devicefarm:us-west-2:000000000000:upload:0000000000000001-emulated/0000000000000001-emulated', got None)"
  divergence_key: service=devicefarm|GetUpload|status=400->200|keys=$.__type,$.message,status
  Expected: HTTP 400, __type 'NotFoundException', message fixed text 'No upload found for arn ' + the request's arn (the recorded ARN's account and ids are this environment's placeholders). Actual: HTTP 200 with a live upload body.

Model evidence (services/service-com-amazonaws-devicefarm-devicefarm-20150623/smithy-model/com-amazonaws-devicefarm-devicefarm-20150623.json):
  CreateDevicePool: input projectArn (required, @length 32..1011, pattern ^arn:aws:devicefarm:.+$), name (required, @length 0..256), rules (Rules, required), description, maxDevices; output devicePool: DevicePool; errors ArgumentException, LimitExceededException, NotFoundException, ServiceAccountException (all client, HTTP 400).
  GetDevicePool / GetUpload: input arn (required, @length 32..1011, pattern ^arn:aws:devicefarm:.+$); output devicePool / upload; same four modeled errors.
  Rule: { attribute: DeviceAttribute enum, operator: RuleOperator enum, value: String }; Rule.attribute doc: 'The rule's stringified attribute. For example, specify the value as "\"abc\""'.
  ArgumentException doc 'An invalid argument was specified.'; NotFoundException doc 'The specified entity was not found.'

Emulator source evidence:
  src/ops/create_device_pool.rs:165-170 (projectArn form only), 176-184 (project-existence NotFoundException gate), 188-198 (quota), 213 (rules stored verbatim, never validated).
  src/ops/get_device_pool.rs:154-159 (form check + '<arn> is not a device pool ARN' wording), 162-172 (store lookup, NotFoundException).
  src/ops/get_upload.rs:160-179 (store lookup, NotFoundException via arn_not_found_message).
  src/ops/delete_upload.rs:152 (Scaffolded::not_implemented -> 501, removes nothing).
  src/state.rs:555-579 (devicefarm_resource_id accepts a one-segment resource id for project-scoped types), 611-613 (arn_not_found_message = 'No {entity} was found for arn {arn}').

## Steps to Reproduce
1. Clone the repository and boot the emulator built from that checkout.
2. Replay the wire fixture corpus (trace set simparity-wire-aws-v1).
3. Cause 1 — fixture devicefarm-device-pool-rule-and-reference-errors, step task#2: call CreateDevicePool against an existing project with a rule whose value is bare text rather than a JSON-quoted string (e.g. attribute PLATFORM, operator EQUALS, value ANDROID). Observe HTTP 200 with a created devicePool instead of HTTP 400 with __type 'ArgumentException' and the 'Parse error for attribute operand: <operand>. Rule requires a String with \" on both sides ...' message.
4. Cause 2 — same fixture, step task#3: call CreateDevicePool with a well-formed projectArn whose project record the store does not hold, name set and rules empty. Observe HTTP 400 (NotFoundException from the project-existence gate) instead of HTTP 200 carrying devicePool { arn minted under that project id, the supplied name, rules [], type 'PRIVATE' }.
5. Cause 3 — same fixture, step task#4: call GetDevicePool with a devicepool ARN whose resource id is a single segment (no '/<pool-id>'). Observe __type 'NotFoundException' with 'No device pool was found for arn <arn>' instead of __type 'ArgumentException' with 'Invalid arn <arn>'.
6. Cause 4 — fixture devicefarm-upload-lifecycle, step task#5: CreateUpload, then DeleteUpload on that ARN, then GetUpload on the same ARN. Observe HTTP 200 with a live upload body instead of HTTP 400 with __type 'NotFoundException' and message 'No upload found for arn <that arn>'. Note that even once deletion is implemented, the emulator's miss message reads 'No upload was found for arn <arn>', which still diverges from AWS's fixed text.
7. All four steps pass only when rule operands are validated, the project-existence gate on CreateDevicePool is removed, project-scoped ARN forms require the two-segment resource id and refuse with ArgumentException/'Invalid arn <arn>', and DeleteUpload removes the record while GetUpload words its NotFoundException as AWS does.
