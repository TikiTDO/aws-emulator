Issue-Source: trace-sweep-divergence
Issue-Trace: resource-groups-tagging-api-tag-resources-validation-error-paths
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resource-groups-tagging-api-tag-resources-validation-error-paths resource-groups-tagging-api-get-resources-query-error-paths
Issue-Shape-Id: com.amazonaws.resourcegroupstaggingapi#ResourceGroupsTaggingAPI_20170126
Issue-Key: resourcegroupstaggingapi|TagResources|4xx->2xx|$.Message
Issue-Key: resourcegroupstaggingapi|TagResources|4xx->2xx|$.__type
Issue-Key: resourcegroupstaggingapi|TagResources|4xx->2xx|status
Issue-Key: resourcegroupstaggingapi|TagResources|2xx->2xx|$.FailedResourcesMap.arn
Issue-Key: resourcegroupstaggingapi|GetResources|4xx->4xx|$.Message
Issue-Key: resourcegroupstaggingapi|GetResources|4xx->4xx|$.__type
Issue-Key: resourcegroupstaggingapi|TagResources|4xx->4xx|$.message
Issue-Commit: 1e44d2b2032d56c599b46458071bcd07c3617563

# Resource Groups Tagging API error contract diverges: malformed ARNs and bad pagination tokens misclassified

> _Note: this issue was identified against the code base at commit `1e44d2b2032d56c599b46458071bcd07c3617563`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResources

## Description
Four distinct divergences in the Resource Groups Tagging API error contract, all in how the emulator classifies a bad request or a resource it cannot tag. All four must be closed before this issue is submitted.

Cause 1 of 4 — TagResources accepts a string that is not an ARN and answers 200 instead of 400 InvalidParameterException (fixture resource-groups-tagging-api-tag-resources-validation-error-paths, step task#2; same cluster also covers task#3).
AWS: a value in ResourceARNList that cannot be parsed as an ARN is a request-level validation failure, refused with HTTP 400 and error code InvalidParameterException before any resource is touched. Smithy: TagResources.ResourceARNList targets ResourceARNListForTagUntag (len 1..20, member ResourceARN len 1..1011); the operation models InvalidParameterException (client, HTTP 400), documented as "A provided string parameter is malformed" and "The partition specified in an ARN parameter in the request doesn't match the partition where you invoked the operation". Recorded response: 400, __type InvalidParameterException, message e.g. "<value> is not a valid AmazonResourceName (ARN)" (message quoted as an example of the rule: it names the offending value and the ARN constraint).
Emulator: answers HTTP 200 with no __type and no Message at all. It treats an unparseable ARN as a PER-RESOURCE failure folded into FailedResourcesMap instead of a request-level refusal.
Why: a client that passes a typo'd or non-ARN identifier is told the call succeeded. SDK error handling never fires, no exception is raised, and the caller proceeds believing tags were applied to a resource that was never even addressable.
Fix: TagResources' input validation in the operation handler — reject an unparseable ARN in ResourceARNList with InvalidParameterException (400) before the tagging loop, rather than recording it as a per-ARN FailureInfo.
Traces: resource-groups-tagging-api-tag-resources-validation-error-paths#task#2 (and #task#3). Code site: services/service-com-amazonaws-resourcegroupstaggingapi-resourcegroupstaggingapi-20170126/src/ops/tag_resources.rs lines 202-206 (parse_arn failure -> malformed_arn_failure inserted into `failed`, then Ok(200)).

Cause 2 of 4 — TagResources silently succeeds for a resource that does not exist, omitting its FailedResourcesMap entry (fixture resource-groups-tagging-api-tag-resources-validation-error-paths, step task#5).
AWS: a well-formed ARN naming a resource that does not exist is reported, per resource, inside the 200 response's FailedResourcesMap: the ARN key maps to a FailureInfo whose ErrorCode is the host service's error code (ResourceNotFound) and whose StatusCode is 404. Smithy: TagResourcesOutput.FailedResourcesMap -> FailedResourcesMap (key ResourceARN, value FailureInfo{StatusCode:int, ErrorCode:enum, ErrorMessage}); FailureInfo is documented as carrying "any valid error code returned by the Amazon Web Services service that hosts the resource that the ARN key represents". Recorded response: 200 with FailedResourcesMap[<sns topic arn>] = {ErrorCode: "ResourceNotFound", StatusCode: 404, ErrorMessage: "Resource does not exist"} — the ErrorCode and StatusCode are the requirement; the ErrorMessage text is an example.
Emulator: answers 200 with that ARN absent from FailedResourcesMap, i.e. reported as successfully tagged. Its store creates a tag entry for any previously unseen, parseable ARN, so no existence check happens at all.
Why: FailedResourcesMap is the ONLY channel this operation has for partial failure, so a client that inspects it (the documented way to detect which resources were not tagged) sees an empty map and concludes every ARN was tagged. Tag-propagation and compliance workflows then treat untagged, nonexistent resources as tagged.
Fix: TagResources' handler plus the tagging store — resolve each parseable ARN against existing resource state and emit a FailureInfo{ErrorCode: ResourceNotFound, StatusCode: 404} for one that does not exist, instead of vivifying a new store entry.
Traces: resource-groups-tagging-api-tag-resources-validation-error-paths#task#5. Code site: .../src/ops/tag_resources.rs lines 215-221 (state.entry_mut(&parsed) creates the entry; only store capacity can fail).

Cause 3 of 4 — GetResources returns the wrong error code for an invalid pagination token: PaginationTokenExpiredException instead of InvalidParameterException (fixture resource-groups-tagging-api-get-resources-query-error-paths, step task#4).
AWS: a PaginationToken that cannot be interpreted (garbage, truncated, or minted for a different query) is a malformed parameter and is refused with error code InvalidParameterException. PaginationTokenExpiredException is reserved for the narrower case of a token that WAS valid and has aged out. Smithy: GetResources models both InvalidParameterException and PaginationTokenExpiredException (both client, HTTP 400); PaginationTokenExpiredException is documented as "The request failed because the specified PaginationToken has expired. A PaginationToken is valid for a maximum of 15 minutes." — expiry, not malformation. Recorded response: 400, __type InvalidParameterException (recorded message "Pagination token is invalid=<token>" quoted as an example).
Emulator: answers 400 with __type PaginationTokenExpiredException for every token it fails to decode, whatever the reason.
Why: the two codes mean different things to a client. Callers retry or restart on PaginationTokenExpiredException (expiry is transient and a fresh scan fixes it) but surface InvalidParameterException as a caller bug. Reporting expiry for a structurally invalid token sends clients into pointless retry loops and hides the real fault. The HTTP status is the same, so code is the only signal.
Fix: GetResources' pagination-token handling in the operation handler — distinguish an undecodable/foreign-fingerprint token (InvalidParameterException) from a decoded-but-aged-out one (PaginationTokenExpiredException).
Traces: resource-groups-tagging-api-get-resources-query-error-paths#task#4. Code site: .../src/ops/get_resources.rs lines 331-348 (decode_pagination_token returning None unconditionally raises PaginationTokenExpiredException).

Cause 4 of 4 — TagResources constraint-violation message is not in AWS's validation-error form (fixture resource-groups-tagging-api-tag-resources-validation-error-paths, step task#4).
AWS: an over-long tag value is refused by the standard constraint-validation serializer, which reports the violation at the MAP MEMBER path, echoes the offending map ENTRY, and lists the member's full modeled constraint set: "1 validation error detected: Value '{<key>=<value>}' at 'tags' failed to satisfy constraint: Map value must satisfy constraint: [Member must have length less than or equal to 256, Member must have length greater than or equal to 0, Member must satisfy regular expression pattern: [\s\S]*]". That message is built from the request and from the model, so its FORM is the requirement, not the recorded characters. Smithy: TagResources.Tags -> TagMap (len 1..50), value TagValue length 0..256, pattern ^[\s\S]*$ — exactly the three constraints AWS enumerates. Status 400 is unchanged and matches.
Emulator: emits "1 validation error detected: Value '<value truncated>' at 'tags.<key>' failed to satisfy constraint: Member must have length less than or equal to 256" — it paths into the individual entry ('tags.<key>' rather than 'tags'), echoes only the value rather than the '{key=value}' entry, truncates the value, and lists only the single violated constraint rather than the map value's whole constraint set.
Why: tooling and tests that parse the validation message to recover the offending member path and the constraint set read a path that does not exist in the model ('tags.<key>') and an incomplete constraint list. Severity is low on its own — status and classification are correct — but it is the same serializer that every constrained member of this service reports through.
Fix: the shared constraint-violation serializer used for this operation's modeled member constraints — path to the collection member, render the whole entry, and enumerate the target shape's full constraint set.
Traces: resource-groups-tagging-api-tag-resources-validation-error-paths#task#4.

No open issue covers any of these: the only other Resource Groups Tagging API issue on file is the bootstrap issue, which is already resolved.

## Evidence
Replay of trace set simparity-wire-aws-v1 (3 fixtures, 23 steps, 15 passed, 8 mismatched, score 65.2%, emulator_up=true, bootstrap_signal=false, state_reset_failures=0). All five reported clusters belong to service=resourcegroupstaggingapi; none carries a stub_path or an owned_by.

Cause 1 — fixture resource-groups-tagging-api-tag-resources-validation-error-paths step task#2 (cluster also covers task#3), operation TagResources, verdict mismatch:
  expected status 400, actual 200
  $.__type: missing (expected 'InvalidParameterException', got None)
  $.Message: missing (expected '<value> is not a valid AmazonResourceName (ARN)', got None)

Cause 2 — fixture resource-groups-tagging-api-tag-resources-validation-error-paths step task#5, operation TagResources, verdict mismatch (status 200 -> 200):
  $.FailedResourcesMap.arn:aws:sns:<region>:<account>:<topic>: missing
    expected {'ErrorCode': 'ResourceNotFound', 'ErrorMessage': 'Resource does not exist', 'StatusCode': 404}, got None

Cause 3 — fixture resource-groups-tagging-api-get-resources-query-error-paths step task#4, operation GetResources, verdict mismatch (status 400 -> 400):
  $.__type: value_mismatch (expected 'InvalidParameterException', got 'PaginationTokenExpiredException')
  $.Message: value_mismatch (expected 'Pagination token is invalid=<token>', got 'the specified PaginationToken is not valid for this request')

Cause 4 — fixture resource-groups-tagging-api-tag-resources-validation-error-paths step task#4, operation TagResources, verdict mismatch (status 400 -> 400):
  $.message: value_mismatch
    expected "1 validation error detected: Value '{<key>=<257-char value>}' at 'tags' failed to satisfy constraint: Map value must satisfy constraint: [Member must have length less than or equal to 256, Member must have length greater than or equal to 0, Member must satisfy regular expression pattern: [\s\S]*]"
    got      "1 validation error detected: Value '<value>... [257 bytes]' at 'tags.<key>' failed to satisfy constraint: Member must have length less than or equal to 256"

Model evidence (services/service-com-amazonaws-resourcegroupstaggingapi-resourcegroupstaggingapi-20170126/smithy-model/com-amazonaws-resourcegroupstaggingapi-resourcegroupstaggingapi-20170126.json):
  TagResources: input ResourceARNList (required, ResourceARNListForTagUntag len 1..20, member ResourceARN len 1..1011), Tags (required, TagMap len 1..50, value TagValue len 0..256 pattern ^[\s\S]*$); output FailedResourcesMap -> map<ResourceARN, FailureInfo{StatusCode:int default 0, ErrorCode:enum, ErrorMessage}>; errors InternalServiceException(500), InvalidParameterException(400), ThrottledException(400).
  GetResources: errors InternalServiceException(500), InvalidParameterException(400), PaginationTokenExpiredException(400), ThrottledException(400). PaginationTokenExpiredException doc: "The request failed because the specified PaginationToken has expired. A PaginationToken is valid for a maximum of 15 minutes."
  InvalidParameterException doc: "A required parameter is missing. A provided string parameter is malformed. An provided parameter value is out of range. ..."
  FailureInfo.ErrorCode doc: "any valid error code returned by the Amazon Web Services service that hosts the resource that you want to tag."

Source evidence:
  src/ops/tag_resources.rs:202-206 — unparseable ARN -> malformed_arn_failure in FailedResourcesMap, response still 200 (cause 1).
  src/ops/tag_resources.rs:215-221 — state.entry_mut(&parsed) creates a store entry for any new parseable ARN; only store capacity yields a failure, so a nonexistent resource is reported as tagged (cause 2).
  src/ops/get_resources.rs:331-348 — decode_pagination_token -> None raises PaginationTokenExpiredException unconditionally (cause 3).

## Steps to Reproduce
Replay trace set simparity-wire-aws-v1 against the emulator built from this checkout (replay_wire_corpus), then re-run the four bound steps:

Cause 1: resource-groups-tagging-api-tag-resources-validation-error-paths#task#2 — POST TagResources with X-Amz-Target ResourceGroupsTaggingAPI_20170126.TagResources and body {"ResourceARNList":["<not-an-arn>"],"Tags":{"k":"v"}}. Expect HTTP 400 with __type InvalidParameterException naming the invalid ARN; observe HTTP 200. Step task#3 of the same fixture is the same cause.

Cause 2: resource-groups-tagging-api-tag-resources-validation-error-paths#task#5 — POST TagResources with a well-formed ARN for a resource that was never created, e.g. {"ResourceARNList":["arn:aws:sns:<region>:<account>:<nonexistent-topic>"],"Tags":{"k":"v"}}. Expect HTTP 200 whose FailedResourcesMap contains that ARN with ErrorCode ResourceNotFound and StatusCode 404; observe HTTP 200 with that ARN absent from FailedResourcesMap.

Cause 3: resource-groups-tagging-api-get-resources-query-error-paths#task#4 — POST GetResources with body {"PaginationToken":"<garbage token>"}. Expect HTTP 400 with __type InvalidParameterException; observe HTTP 400 with __type PaginationTokenExpiredException.

Cause 4: resource-groups-tagging-api-tag-resources-validation-error-paths#task#4 — POST TagResources with a valid ARN and a tag whose value is 257 characters. Expect HTTP 400 whose message paths at 'tags', echoes the '{key=value}' entry and lists all three TagValue constraints; observe a message pathing at 'tags.<key>' with only the length<=256 constraint and a truncated value.

All four run against a freshly reset store (state_reset_failures was 0 for this replay, so no cross-fixture state contamination is implicated).
