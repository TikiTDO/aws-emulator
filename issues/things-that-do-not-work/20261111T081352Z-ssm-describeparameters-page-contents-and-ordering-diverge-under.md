Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-describe-parameters-filters-maxresults-and-pagination
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-describe-parameters-filters-maxresults-and-pagination
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|DescribeParameters|2xx->2xx|$.Parameters[*]
Issue-Commit: 34aa023e13a037c836015cae341f7a7e68727fe7

# SSM DescribeParameters page contents and ordering diverge under ParameterFilters and MaxResults paging

> _Note: this issue was identified against the code base at commit `34aa023e13a037c836015cae341f7a7e68727fe7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeParameters

## Description
DescribeParameters answers 200 on both sides, but the emulator puts the WRONG ParameterMetadata entries, in the wrong order, on a page requested with ParameterFilters + MaxResults + NextToken. 1 CAUSE.

Cause 1 of 1 - the page a filtered, size-limited DescribeParameters read returns is assembled by walking the whole parameter store from the beginning instead of applying the request's filters and resuming after the previous page's last entry.

AWS: Smithy (services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json) models DescribeParameters with input members Filters, ParameterFilters (ParameterStringFilterList), MaxResults (@range min 1, max 50) and NextToken, output Parameters (ParameterMetadataList) + NextToken, and the operation is @paginated {inputToken: NextToken, outputToken: NextToken, pageSize: MaxResults}; its modeled client errors are InvalidFilterKey, InvalidFilterOption, InvalidFilterValue and InvalidNextToken. The model does not state a result order, so for ordering this is recorded behavior only: the recording shows that each page carries exactly the ParameterMetadata entries matching the request's filters, in the service's own listing order (which is NOT the order the parameters were created in - the recorded page leads with the parameter created second, then the one created fourth, then the one created first), and that a page fetched with a NextToken continues that same listing rather than restarting it. Per-entry LastModifiedDate and LastModifiedUser are masked by the fixture as <shaped-not-asserted>, so they are not part of this divergence; Name, ARN, Type, Version, Tier, Policies and DataType are.

Emulator: for the same request it returns the parameters in creation order beginning with the first parameter created (created-1st, created-2nd, created-3rd, ...). Every asserted entry of the page differs from the recording ($.Parameters[0] through $.Parameters[4] all diverge), and the same wrong composition is returned for the following page too, which is why the two paged steps of the fixture fail identically. The effect is that the emulator serves a listing that neither honours the request's filter selection nor the cut point the NextToken names - it serves the head of the full store.

Why: a client that pages DescribeParameters to inventory its Parameter Store gets a page that is not the page it asked for. Entries its ParameterFilters excluded appear in the result, entries that should appear on this page are absent, and because the second page repeats the head of the store rather than continuing after the first page's last entry, a full walk re-reports parameters it already processed and silently never reaches the tail of the hierarchy. Anything built on that walk - a drift report, a bulk re-tagging pass, a config export - operates on the wrong set of parameters while every call looks successful (200).

Fix: the DescribeParameters handler's listing path - its ParameterFilters/Filters evaluation and its page-assembly/NextToken resume logic (the page must be cut from the filtered listing and resumed strictly after the previous page's last entry, not from the start of the store).

Traces: ssm-describe-parameters-filters-maxresults-and-pagination#task#8 (representative) and #task#9.

Not covered by the open issues that look adjacent: 20261103T215115Z-ssm-parameter-store-semantics-diverge-selector-echo-filter.md is behavioral and covers GetParametersByPath NextToken exactness, GetParametersByPath tag/Type filters and the DescribeParameters tag:<key> BeginsWith match - not the composition or ordering of a DescribeParameters page; 20261025T053713Z-ssm-addtagstoresource-is-an-unimplemented-501-stub-and.md covers the missing ParameterMetadata.LastModifiedUser field, which this fixture does not assert.

## Evidence
Replay of simparity-wire-aws-v1, fixture ssm-describe-parameters-filters-maxresults-and-pagination, steps task#8 and task#9 (verdict mismatch, divergence_key 'service=ssm|DescribeParameters|status=200->200|keys=$.Parameters[0],$.Parameters[1],$.Parameters[2],$.Parameters[3],$.Parameters[4]').

Status matched (expected 200, got 200); no error code or error message is involved. The payload diverged on every asserted list slot:
  $.Parameters[0]: expected the metadata of the parameter created 2nd (Name/ARN ending '-1', Type String, Version 1, Tier Standard, Policies [], DataType text), got the metadata of the parameter created 1st (name ending '-0').
  $.Parameters[1]: expected the parameter created 4th (name ending '-3'), got the parameter created 2nd (name ending '-1').
  $.Parameters[2]: expected the parameter created 1st (name ending '-0'), got the parameter created 3rd (name ending '-2').
  $.Parameters[3], $.Parameters[4]: likewise diverged.
The concrete generated parameter names, ARNs and the account/region in them are per-run values of this recording and are not themselves the requirement; the requirement is that the page carries the filtered entries in the service's listing order and that the NextToken page continues that listing. LastModifiedDate and LastModifiedUser were asserted as <shaped-not-asserted> and so are excluded.

The divergence carries no stub_path (the operation is implemented) and no owned_by (no open issue carries this replay key).

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Replay the simparity-wire-aws-v1 fixture ssm-describe-parameters-filters-maxresults-and-pagination and observe steps task#8 and task#9.
3. Manually: PutParameter several String parameters under a common generated prefix, then call DescribeParameters with ParameterFilters narrowing to a subset and MaxResults smaller than the match count; compare the returned Parameters list (membership and order) with the recording, then call DescribeParameters again with the returned NextToken and confirm the emulator restarts at the head of the parameter store instead of continuing after the previous page's last entry.
