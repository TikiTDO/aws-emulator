Issue-Source: trace-sweep-divergence
Issue-Trace: resource-groups-tagging-api-tag-resources-validation-error-paths
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resource-groups-tagging-api-tag-resources-validation-error-paths
Issue-Shape-Id: com.amazonaws.resourcegroupstaggingapi#ResourceGroupsTaggingAPI_20170126
Issue-Key: resourcegroupstaggingapi|TagResources|4xx->4xx|$.Message
Issue-Commit: 5b71589754f95f05f88c1a0ee9ccd691bbc5f701

# TagResources InvalidParameterException message for a non-ARN value does not use AWS wording

> _Note: this issue was identified against the code base at commit `5b71589754f95f05f88c1a0ee9ccd691bbc5f701`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResources

## Description
One cause.

Cause 1 of 1: TagResources correctly refuses a ResourceARNList value that is not an ARN with HTTP 400 / InvalidParameterException, but the error MESSAGE is the emulator's own phrasing instead of AWS's (fixture resource-groups-tagging-api-tag-resources-validation-error-paths, step task#2).

AWS: a value in ResourceARNList that cannot be parsed as an ARN is a request-level parameter fault, refused with HTTP 400 and error code InvalidParameterException, whose message is the ARN-validation form: the offending value followed by the fixed text " is not a valid AmazonResourceName (ARN)". The fixed text is the whole sentence after the value; the only variable part is the value, taken verbatim from the request's ResourceARNList element. Smithy: TagResources.ResourceARNList (required, ResourceARNListForTagUntag @length 1..20, member ResourceARN @length 1..1011) and modeled error InvalidParameterException (client, HTTP 400), documented as "The request failed because of one of the following reasons: ... A provided string parameter is malformed. ... The partition specified in an ARN parameter in the request doesn't match the partition where you invoked the operation." The model does not state the message text, so the message form above is recorded behavior from the fixture's expected response (status 400 and code InvalidParameterException are modeled and already match).

Emulator: returns 400 with code InvalidParameterException but Message = "ResourceARNList contains a value that is not an Amazon Resource Name: <value>" — a different sentence (it names the input member and places the value at the END after a colon), where AWS leads with the value and uses the fixed phrase "is not a valid AmazonResourceName (ARN)". Note the emulator even spells the concept differently ("Amazon Resource Name" vs AWS's single token "AmazonResourceName").

Why: a client that asserts on, logs, or pattern-matches the InvalidParameterException message to recover the rejected identifier gets a string in a shape that never occurs against AWS. Message text is part of this error's contract, so anything keyed on it (user-facing error surfacing, retry/classification heuristics that distinguish a malformed-ARN refusal from a partition or region refusal, recorded-response comparisons in client test suites) diverges even though status and code are right. It also hides that the three distinct refusal reasons AWS documents for an ARN parameter are supposed to be distinguishable from the message alone.

Fix: the shared ARN-validation message helper for this service's validation path — not_an_arn_message in services/service-com-amazonaws-resourcegroupstaggingapi-resourcegroupstaggingapi-20170126/src/state.rs, reached from addressable_arn and from the TagResources handler's request-level validation loop. It must render AWS's form (value first, then the fixed "is not a valid AmazonResourceName (ARN)" text) rather than a member-named sentence. UntagResources and any other operation taking ResourceARNList share the same helper and the same wording.

Note: open issue 20261022T140612Z-resourcegroupstaggingapi-resource-groups-tagging-api-error-contract-diverges-malformed.md covers nearby Resource Groups Tagging API error-contract causes, including the earlier state of this same step where TagResources answered 200 instead of 400. The status and the error code are now correct, so that cause's classification key (TagResources 4xx->2xx on $.__type/status) no longer reproduces; this message-wording divergence on the same step is a distinct, unowned cluster (replay reports it with owned_by empty and no stub_path) and is filed here.

Traces: resource-groups-tagging-api-tag-resources-validation-error-paths#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false; 5 fixtures replayed, 29 steps, 19 passed, 10 mismatched, 0 errored, 0 blocked, score 65.5%, 4 divergence clusters). This cluster: service=resourcegroupstaggingapi, operation=TagResources, verdict=mismatch, stub_path empty, owned_by empty, affected_count=1.

Fixture resource-groups-tagging-api-tag-resources-validation-error-paths, step task#2:
  expected status 400, actual status 400 (matches)
  divergence_key: service=resourcegroupstaggingapi|TagResources|status=400->400|keys=$.Message
  $.Message: value_mismatch
    expected: '<value> is not a valid AmazonResourceName (ARN)'
    actual:   'ResourceARNList contains a value that is not an Amazon Resource Name: <value>'
  (in the recording <value> is the probe's non-ARN string; the value is request-supplied and is NOT part of the requirement — the fixed text around it is.)

Model evidence (services/service-com-amazonaws-resourcegroupstaggingapi-resourcegroupstaggingapi-20170126/smithy-model/com-amazonaws-resourcegroupstaggingapi-resourcegroupstaggingapi-20170126.json), via smithy_query:
  TagResources: input ResourceARNList (required, ResourceARNListForTagUntag @length 1..20, member ResourceARN @length 1..1011), Tags (required, TagMap @length 1..50); errors InternalServiceException(500, server), InvalidParameterException(400, client), ThrottledException(400, client); awsJson RPC, X-Amz-Target ResourceGroupsTaggingAPI_20170126.TagResources.
  InvalidParameterException documentation lists "A provided string parameter is malformed" and the partition-mismatch case; it does not fix the message text, hence "recorded behavior only" for the wording.

Source evidence:
  src/state.rs:409-413 — not_an_arn_message(value) = format!("ResourceARNList contains a value that is not an Amazon Resource Name: {value}"), the emitted text.
  src/state.rs:390-393 — addressable_arn: parse_arn failure -> Err(not_an_arn_message(value)).
  src/ops/tag_resources.rs:202-222 — request-level validation loop maps that Err into InvalidParameterException (400) for the whole call, so the status/code are right and only the message diverges.

## Steps to Reproduce
1. Build the emulator from this checkout and replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
2. Re-run the bound step resource-groups-tagging-api-tag-resources-validation-error-paths#task#2: POST / with header X-Amz-Target: ResourceGroupsTaggingAPI_20170126.TagResources and body {"ResourceARNList":["<some-string-that-is-not-an-arn>"],"Tags":{"<key>":"<value>"}}.
3. Expect HTTP 400 with __type InvalidParameterException and Message equal to the offending value followed by " is not a valid AmazonResourceName (ARN)".
4. Observe HTTP 400 with __type InvalidParameterException but Message "ResourceARNList contains a value that is not an Amazon Resource Name: <the offending value>".
5. The status and error code already match; only $.Message diverges.
