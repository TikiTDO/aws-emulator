Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.list-tags-for-resources-mixed-types-rejected
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53.list-tags-for-resources-mixed-types-rejected|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 788c8d3791606f1915c8ae07a8aed7e384a8b802

# ListTagsForResources InvalidInput validation message names the generic list path 'resourceIds.N.member' instead of the resource type's id field ('healthCheckId')

> _Note: this issue was identified against the code base at commit `788c8d3791606f1915c8ae07a8aed7e384a8b802`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResources

## Description
Cause 1 of 1: the InvalidInput validation message emitted when a ResourceIds entry exceeds the 64-character limit names the wrong field.

AWS: Smithy (services/service-com-amazonaws-route53-awsdnsv20130401/smithy-model/com-amazonaws-route53-awsdnsv20130401.json): ListTagsForResources POST /2013-04-01/tags/{ResourceType}, input ResourceType (TagResourceType enum: healthcheck | hostedzone, @httpLabel, required) and ResourceIds (TagResourceIdList, @length min 1 max 10) whose member targets TagResourceId with @length min 0 max 64 (member xmlName ResourceId); modeled error InvalidInput, httpError 400. The model states the 64-character constraint but not the wording of the message, so the wording is recorded behavior only: AWS refuses the whole call with HTTP 400, Error.Code `InvalidInput`, and a standard AWS constraint-violation message whose field path is the RESOURCE TYPE'S OWN ID FIELD, not the request list path. For ResourceType=healthcheck the recorded message is:
  "1 validation error detected: Value '<the offending resource id>' at 'healthCheckId' failed to satisfy constraint: Member must have length less than or equal to 64"
Fixed text of that template: "1 validation error detected: Value '", "' at '", "' failed to satisfy constraint: Member must have length less than or equal to 64". Variable parts taken from the request: the offending ResourceIds value (quoted verbatim, here a 100-character id) and the field name, which is derived from ResourceType — 'healthCheckId' for ResourceType=healthcheck (correspondingly the hosted-zone id field for ResourceType=hostedzone). The error count prefix ("1 validation error detected") reflects how many entries violated the constraint. No index of the offending list position appears, and no 'resourceIds' list path appears.

Emulator: the call IS refused with Error.Code `InvalidInput` (the code and status are correct), but the message names the generic Smithy list path of the offending member instead of the resource-type id field. Replay of route53.list-tags-for-resources-mixed-types-rejected recorded:
  got "1 validation error detected: Value 'zzzz…zzz' at 'resourceIds.2.member' failed to satisfy constraint: Member must have length less than or equal to 64"
versus the expected "… at 'healthCheckId' failed to satisfy …". The divergence is purely the field-path token: the emulator emits 'resourceIds.<index>.member' (a serializer/schema path including a positional index), AWS emits 'healthCheckId'.

Why: clients and test suites assert on the validation message to learn WHICH input field AWS objected to; the Route 53 convention is that a batched tag read reports the violation against the resource type's id field. A client parsing the message for the field name reads 'resourceIds.2.member', which corresponds to no documented Route 53 input field, so field-level error surfacing, user-facing form validation, and message-based assertions all break — and a caller cannot reproduce the same text against real AWS. Hypothesis: the emulator is letting the shared constraint-validation layer render the message from the Smithy member path rather than mapping it to the Route 53 id field name.

Fix: ListTagsForResources input validation / its InvalidInput message construction for this service — the constraint-violation message must render the field name from ResourceType (healthcheck -> 'healthCheckId', hostedzone -> the hosted-zone id field) rather than the generic schema path, with no list index. The handler itself only does per-id existence resolution and does not build this message, so the mapping belongs in the operation's validation layer that renders constraint failures for TagResourceIdList members.

Traces: route53.list-tags-for-resources-mixed-types-rejected. Related (does NOT cover this): open issue 20261024T160546Z-route53-route-53-validation-semantics-gaps-five-operations-answer.md, whose Cause 3 pins the all-or-nothing NoSuchHostedZone refusal for a different check (route53.list-tags-for-resources-mixed-valid-invalid-fails-whole-batch) and says nothing about the constraint-violation message text. Code citation for the handler: services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/list_tags_for_resources.rs lines 145-196 (handle body; it constructs InvalidInput only for an unknown resource type at lines 185-189 and never for the length constraint).

## Evidence
Check route53.list-tags-for-resources-mixed-types-rejected — verdict: mismatch (suite simparity-behavioral-ga-prod-v1, 76/118 passed).

Request sequence: CreateHealthCheck (HTTP/example.com:80, interval 30, failure threshold 3) -> ListTagsForResources(ResourceType='healthcheck', ResourceIds=[<hc_id>]) succeeds and echoes the id -> ListTagsForResources(ResourceType='healthcheck', ResourceIds=[<hc_id>, 'z'*100]).

Error.Code: InvalidInput (matches AWS; HTTP 400).
Error.Message expected (AWS): "1 validation error detected: Value 'zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz' at 'healthCheckId' failed to satisfy constraint: Member must have length less than or equal to 64"
Error.Message actual (emulator): "1 validation error detected: Value 'zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz' at 'resourceIds.2.member' failed to satisfy constraint: Member must have length less than or equal to 64"
(The 100-'z' id is the check's chosen value filling the template's Value slot; the requirement is the field-name slot, which must be 'healthCheckId' and is 'resourceIds.2.member'.)

Raw assertion:
  File "check_route53_list_tags_for_resources_mixed_types_rejected.py", line 50, in run
    assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
  AssertionError: expected message "... at 'healthCheckId' ...", got "... at 'resourceIds.2.member' ..."

Model confirmation (smithy_query query=shape on com-amazonaws-route53-awsdnsv20130401.json): TagResourceIdList = list, @length min 1 max 10, member -> TagResourceId (xmlName ResourceId) with @length min 0 max 64; TagResourceId = string @length 0..64; InvalidInput = client error, httpError 400, member `message`.

## Steps to Reproduce
1. Boot the emulator and point a botocore route53 client at it.
2. CreateHealthCheck with a fresh CallerReference and HealthCheckConfig {FullyQualifiedDomainName: 'example.com', Port: 80, Type: 'HTTP', ResourcePath: '/', RequestInterval: 30, FailureThreshold: 3}; keep the returned HealthCheck.Id.
3. Positive control: ListTagsForResources(ResourceType='healthcheck', ResourceIds=[<hc_id>]) — expect one ResourceTagSet whose ResourceId equals <hc_id>.
4. Call ListTagsForResources(ResourceType='healthcheck', ResourceIds=[<hc_id>, 'z'*100]).
5. Expect HTTP 400 with Error.Code 'InvalidInput' and Error.Message "1 validation error detected: Value '<the over-long id>' at 'healthCheckId' failed to satisfy constraint: Member must have length less than or equal to 64". Observed: the same code and status but field path 'resourceIds.2.member' in place of 'healthCheckId'.
6. Confirm the refusal was a no-op: ListTagsForResource(ResourceType='healthcheck', ResourceId=<hc_id>) still returns an empty Tags list.
7. Re-replay check route53.list-tags-for-resources-mixed-types-rejected to confirm the fix.
