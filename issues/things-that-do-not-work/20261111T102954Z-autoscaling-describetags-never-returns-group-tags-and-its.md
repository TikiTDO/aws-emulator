Issue-Source: trace-sweep-divergence
Issue-Trace: autoscaling-tags-create-describe-delete
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: autoscaling-tags-create-describe-delete autoscaling-tags-pagination-max-records autoscaling-describe-tags-malformed-filters-and-maxrecords-bounds autoscaling-pagination-invalid-next-token-errors autoscaling-static-catalogs-and-unfiltered-describes
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: autoscaling|DescribeTags|2xx->2xx|$.DescribeTagsResponse.DescribeTagsResult.Tags
Issue-Key: autoscaling|DescribeTags|2xx->2xx|$.DescribeTagsResponse.DescribeTagsResult.NextToken
Issue-Key: autoscaling|DescribeTags|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: autoscaling|DescribeTags|4xx->2xx|$.ErrorResponse
Issue-Key: autoscaling|DescribeTags|4xx->2xx|status
Issue-Key: autoscaling|DescribeTags|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# autoscaling DescribeTags never returns group tags and its filter, boolean and token refusals are re-worded (5 causes)

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTags

## Description
autoscaling DescribeTags is effectively unimplemented behind a 200: it never reports the tags the group actually carries (so it also never paginates them), and its three argument refusals are either missing or carry emulator-authored text/codes. 5 net-new causes, all on autoscaling DescribeTags. The autoscaling Smithy model is not present in this checkout (multi-service root, no services/service-com-amazonaws-autoscaling-autoscaling/smithy-model/*.json), so each cause is grounded in recorded behavior only; the fixed text of each message and its variable parts are named per cause.

Cause 1 of 5 — tags written with the group are never read back.
AWS: recorded behavior only. DescribeTags returns one TagDescription per tag, with ResourceId the group name, ResourceType the fixed 'auto-scaling-group', Key and Value as written and PropagateAtLaunch as applied ('false' when not requested).
Emulator: DescribeTagsResult.Tags is serialized empty ('') even though the group was just created/tagged.
Why: tag-driven workflows (cost allocation, inventory filters, 'does this group carry tag X' gates) see an untagged group, and a delete-tag flow cannot verify its own precondition.
Fix: the DescribeTags handler and the tag state it reads — group tags recorded by CreateAutoScalingGroup/CreateOrUpdateTags must be projected into TagDescriptions.
Traces: autoscaling-tags-create-describe-delete#task#6.

Cause 2 of 5 — tag pagination therefore never happens.
AWS: recorded behavior only. With two tags on the group and MaxRecords=1-style paging, the response carries the TagDescription page plus a NextToken for the remainder.
Emulator: Tags is empty and NextToken is absent entirely.
Why: a client paging tags terminates immediately on the first page and concludes the group has no tags; it never discovers the second page.
Fix: the DescribeTags handler's paging over the tag state (same component as cause 1).
Traces: autoscaling-tags-pagination-max-records#task#6.

Cause 3 of 5 — the unknown-filter-type refusal is re-worded.
AWS: recorded behavior only. Filtering on an unsupported Name is refused HTTP 400 ValidationError with Message 'Filter type <name> is not correct. Allowed Filter types are: auto-scaling-group key value propagate-at-launch' — the offending filter name is the only variable part; the allowed-type list is fixed text, space separated.
Emulator: HTTP 400 but Message "The filter criteria isn't valid. Valid filter types for the Name attribute of filters are: auto-scaling-group, key, value, propagate-at-launch." — different sentence and a comma-separated list.
Why: callers that match on the refusal text (and operators reading it) see a message AWS never emits, so error handling and runbooks keyed to AWS's wording do not fire.
Fix: the DescribeTags input validation's error message.
Traces: autoscaling-describe-tags-malformed-filters-and-maxrecords-bounds#task#3.

Cause 4 of 5 — a non-boolean propagate-at-launch filter value is accepted instead of refused.
AWS: recorded behavior only. A filter value that is not 'true'/'false' for the boolean filter is refused HTTP 400 ValidationError with the fixed Message 'Boolean parameter can only be true or false' (no variable part).
Emulator: HTTP 200 with no ErrorResponse at all.
Why: a malformed filter is silently honoured as a match-nothing (or match-all) filter, so a client gets a result set AWS would never have returned and never learns its request was invalid.
Fix: the DescribeTags input validation — parse boolean filter values and raise ValidationError.
Traces: autoscaling-describe-tags-malformed-filters-and-maxrecords-bounds#task#6.

Cause 5 of 5 — an invalid NextToken is refused with the wrong error code and message.
AWS: recorded behavior only. A malformed/unknown pagination token is refused HTTP 400 with Error.Code 'ValidationError' and Message 'NextToken: <token>' — the fixed prefix 'NextToken: ' plus the rejected token value.
Emulator: HTTP 400 with Error.Code 'InvalidNextToken' and Message "The token '<token>' was invalid".
Why: the error code is part of the contract SDKs retry/branch on; a client catching ValidationError does not catch InvalidNextToken, so a resumable-paging bug surfaces as an unhandled exception.
Fix: the DescribeTags pagination-token validation and its error classification.
Traces: autoscaling-pagination-invalid-next-token-errors#task#6 (cluster also covers autoscaling-static-catalogs-and-unfiltered-describes#task#6).

Related but not covering: the open issue 20261110T100847Z-autoscaling-refusal-fidelity-wrong-codes-and-emulator-authored.md owns other autoscaling refusal steps; the replay reported owned_by empty for all five clusters above.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 — autoscaling-tags-create-describe-delete#task#6, DescribeTags 200 vs 200:
  $.DescribeTagsResponse.DescribeTagsResult.Tags: type_mismatch (expected {'member': {'ResourceId': '<group>', 'PropagateAtLaunch': 'false', 'Value': 'true', 'Key': 'fidelity-check', 'ResourceType': 'auto-scaling-group'}}, got '')

Cause 2 — autoscaling-tags-pagination-max-records#task#6, DescribeTags 200 vs 200:
  $.DescribeTagsResponse.DescribeTagsResult.NextToken: missing (expected a token, got None)
  $.DescribeTagsResponse.DescribeTagsResult.Tags: type_mismatch (expected two TagDescription members with ResourceType 'auto-scaling-group', got '')

Cause 3 — autoscaling-describe-tags-malformed-filters-and-maxrecords-bounds#task#3, DescribeTags 400 vs 400:
  $.ErrorResponse.Error.Message: expected 'Filter type bogus is not correct. Allowed Filter types are: auto-scaling-group key value propagate-at-launch', got "The filter criteria isn't valid. Valid filter types for the Name attribute of filters are: auto-scaling-group, key, value, propagate-at-launch."

Cause 4 — same fixture #task#6, DescribeTags expected 400 got 200:
  $.ErrorResponse: missing (expected Error {Type 'Sender', Code 'ValidationError', Message 'Boolean parameter can only be true or false'})

Cause 5 — autoscaling-pagination-invalid-next-token-errors#task#6, DescribeTags 400 vs 400:
  $.ErrorResponse.Error.Code: expected 'ValidationError', got 'InvalidNextToken'
  $.ErrorResponse.Error.Message: expected 'NextToken: sim2real-probe-invalid-next-token', got "The token 'sim2real-probe-invalid-next-token' was invalid"

## Steps to Reproduce
1. Cause 1: CreateAutoScalingGroup (or CreateOrUpdateTags) with a tag, then DescribeTags — the Tags list is empty.
2. Cause 2: put two tags on the group and DescribeTags with MaxRecords smaller than the tag count — no page contents and no NextToken.
3. Cause 3: DescribeTags with Filters[0].Name='bogus' — 400 with re-worded message.
4. Cause 4: DescribeTags with Filters[0].Name='propagate-at-launch' and a non-boolean value — answered 200 instead of 400 ValidationError.
5. Cause 5: DescribeTags with NextToken='sim2real-probe-invalid-next-token' — 400 InvalidNextToken instead of ValidationError 'NextToken: <token>'.
