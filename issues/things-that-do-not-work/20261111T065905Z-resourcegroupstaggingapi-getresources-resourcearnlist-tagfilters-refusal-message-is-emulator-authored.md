Issue-Source: trace-sweep-divergence
Issue-Trace: resource-groups-tagging-api-bpt-traj-resource-arn-list-route
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resource-groups-tagging-api-bpt-traj-resource-arn-list-route resource-groups-tagging-api-bpt-traj-tag-filter-key-and-value-mistakes
Issue-Shape-Id: com.amazonaws.resourcegroupstaggingapi#ResourceGroupsTaggingAPI_20170126
Issue-Key: resourcegroupstaggingapi|GetResources|4xx->4xx|$.Message
Issue-Commit: df0e92648e2e716917662ffab5a7edf9e8dd8fc2

# GetResources ResourceARNList+TagFilters refusal message is emulator-authored, not AWS's "Filters is not allowed when providing a resource list."

> _Note: this issue was identified against the code base at commit `df0e92648e2e716917662ffab5a7edf9e8dd8fc2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResources

## Description
One net-new cause in the Resource Groups Tagging API GetResources mutually-exclusive-parameter refusal, reported by replay of trace set simparity-wire-aws-v1 (cluster key resourcegroupstaggingapi|GetResources|status=400->400|keys=$.Message; the cluster also covers resource-groups-tagging-api-bpt-traj-tag-filter-key-and-value-mistakes#task#5).

Cause 1 of 1 - a GetResources request that carries BOTH ResourceARNList and TagFilters is correctly refused with HTTP 400 InvalidParameterException, but the refusal's Message is the emulator's own sentence instead of AWS's (fixture resource-groups-tagging-api-bpt-traj-resource-arn-list-route, step task#8; same cluster also covers resource-groups-tagging-api-bpt-traj-tag-filter-key-and-value-mistakes#task#5).
AWS: ResourceARNList is mutually exclusive with the filtering and pagination parameters. Smithy (services/service-com-amazonaws-resourcegroupstaggingapi-resourcegroupstaggingapi-20170126/smithy-model/com-amazonaws-resourcegroupstaggingapi-resourcegroupstaggingapi-20170126.json): GetResourcesInput.ResourceARNList -> ResourceARNListForGet (length 1..100), documented as "You can't specify both this parameter and the TagFilters parameter in the same request. If you do, you get an Invalid Parameter exception" (the same prohibition is documented for ResourceTypeFilters and for ResourcesPerPage/TagsPerPage/PaginationToken); GetResources models InvalidParameterException (client, HTTP 400). The request-level refusal AWS serves for that combination is HTTP 400 with error code InvalidParameterException and the message "Filters is not allowed when providing a resource list." - that sentence is entirely FIXED text, including its trailing period: it names no member, value, ARN or limit, so there is no variable part to substitute from the request or from state. Recorded response for this step: 400, InvalidParameterException, Message "Filters is not allowed when providing a resource list."
Emulator: answers the correct HTTP 400 and the correct __type, but Message is "ResourceARNList and TagFilters can't both be specified in the same request" - a different, emulator-authored sentence (different wording, no trailing period, and it enumerates the two member names where AWS speaks generically of "Filters" and "a resource list").
Why: the error message is part of the contract. A client, SDK retry/classification wrapper or test that matches on AWS's text - or a human reading the failure - sees an unrecognised sentence, so message-based branching and golden-output assertions that pass against AWS fail against the emulator even though the status and error code are right; the wording also wrongly implies only the TagFilters combination is prohibited, when the same single message covers ResourceTypeFilters and the pagination parameters too.
Fix: GetResources' input validation in the operation handler (the mutual-exclusion check that fires when ResourceARNList is combined with TagFilters / ResourceTypeFilters / ResourcesPerPage / TagsPerPage / PaginationToken) - emit InvalidParameterException with AWS's fixed sentence verbatim rather than a hand-written one.
Traces: resource-groups-tagging-api-bpt-traj-resource-arn-list-route#task#8 (cluster also covers resource-groups-tagging-api-bpt-traj-tag-filter-key-and-value-mistakes#task#5).

No OPEN issue covers this. The open GetResources issue 20261102T083644Z-resourcegroupstaggingapi-getresources-never-returns-resources-tagged-by-their-native.md owns the resource-discovery and ResourceTypeFilters-validation causes and explicitly excludes this message cluster. The issue that previously owned this message, 20261022T140612Z-resourcegroupstaggingapi-resource-groups-tagging-api-error-contract-diverges-malformed.md, is RESOLVED (status fixed) and therefore does not cover this still-failing step; this cluster is reported by replay with an empty owned_by and an empty stub_path.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up=true, bootstrap_signal=false, 11 fixtures replayed, 95 steps (80 passed, 15 mismatched, 0 errored, 0 blocked), score 84.2%, 5 divergence clusters. This is the only cluster carrying neither stub_path nor owned_by.

Cluster (divergence_key resourcegroupstaggingapi|GetResources|status=400->400|keys=$.Message), fixture resource-groups-tagging-api-bpt-traj-resource-arn-list-route step task#8, affected_count 2 (also resource-groups-tagging-api-bpt-traj-tag-filter-key-and-value-mistakes#task#5):
  expected_status 400, actual_status 400 (status matches)
  $.Message: value_mismatch
    expected: 'Filters is not allowed when providing a resource list.'
    actual:   "ResourceARNList and TagFilters can't both be specified in the same request"
  __type/InvalidParameterException and the 400 status are not reported as diverging - only the message text.

Model evidence: GetResources errors = InternalServiceException(500), InvalidParameterException(400), PaginationTokenExpiredException(400), ThrottledException(400). GetResourcesInput.ResourceARNList documentation: "You can't specify both this parameter and the ResourceTypeFilters parameter in the same request. If you do, you get an Invalid Parameter exception. You can't specify both this parameter and the TagFilters parameter in the same request. If you do, you get an Invalid Parameter exception. You can't specify both this parameter and any of the pagination parameters (ResourcesPerPage, TagsPerPage, PaginationToken) in the same request..."

Message anatomy: AWS's message is 100% fixed text ("Filters is not allowed when providing a resource list."); no request value, member name, ARN, id, count or timestamp appears in it, so nothing must be templated from the request or the emulator's state.

## Steps to Reproduce
Replay trace set simparity-wire-aws-v1 against the emulator built from this checkout (replay_wire_corpus), then re-run the bound steps.

Cause 1: resource-groups-tagging-api-bpt-traj-resource-arn-list-route#task#8 - POST / with X-Amz-Target: ResourceGroupsTaggingAPI_20170126.GetResources and a body specifying BOTH parameters, e.g. {"ResourceARNList":["arn:aws:lambda:<region>:<account>:function:<any-fn>"],"TagFilters":[{"Key":"<any-key>","Values":["<any-value>"]}]}.
  Expect: HTTP 400, __type InvalidParameterException, Message exactly "Filters is not allowed when providing a resource list."
  Observe: HTTP 400, __type InvalidParameterException, Message "ResourceARNList and TagFilters can't both be specified in the same request".
The same cluster member resource-groups-tagging-api-bpt-traj-tag-filter-key-and-value-mistakes#task#5 reproduces it from the TagFilters-mistakes trajectory.
