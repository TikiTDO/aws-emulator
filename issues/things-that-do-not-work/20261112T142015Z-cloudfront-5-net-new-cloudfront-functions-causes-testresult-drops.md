Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.function-test-rewrites-tenant-directory-uris-to-index-document cloudfront.function-duplicate-name-rejected cloudfront.function-test-throwing-rewrite-fails-with-empty-output cloudfront.function-with-syntax-error-publishes-then-fails-at-test cloudfront.function-development-edit-leaves-live-rewrite-until-republished cloudfront.function-development-to-live-publish-lifecycle cloudfront.function-live-stage-before-publish-rejected
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.function-test-rewrites-tenant-directory-uris-to-index-document|run#4|KeyError|assert res["FunctionErrorMessage"] == "", f"{uri!r} raised {res['FunctionErrorMessage']!r}"
Issue-Key: cloudfront.function-duplicate-name-rejected|run#8|KeyError|assert result["FunctionErrorMessage"] == "", (
Issue-Key: cloudfront.function-test-throwing-rewrite-fails-with-empty-output|run#7|KeyError|assert json.loads(res["FunctionOutput"]) == {}, (
Issue-Key: cloudfront.function-with-syntax-error-publishes-then-fails-at-test|run#10|KeyError|assert json.loads(res["FunctionOutput"]) == {}, (
Issue-Key: cloudfront.function-development-edit-leaves-live-rewrite-until-republished|run#17|AssertionError|assert code == "PreconditionFailed", f"testing LIVE with the DEVELOPMENT ETag returned {code!r}"
Issue-Key: cloudfront.function-development-to-live-publish-lifecycle|run#20|AssertionError|assert live_desc.get("ETag") == dev_etag, (
Issue-Key: cloudfront.function-live-stage-before-publish-rejected|run#18|AssertionError|assert sorted(stages) == ["DEVELOPMENT", "LIVE"], f"ListFunctions lists {name!r} at {stages!r}"
Issue-Commit: 1814b3996c94d6e0656759171876e93c26063e2d

# 5 net-new CloudFront Functions causes: TestResult drops FunctionErrorMessage/FunctionOutput, stale If-Match answers InvalidIfMatchVersion, PublishFunction rotates the ETag, ListFunctions ignores the unset Stage filter

> _Note: this issue was identified against the code base at commit `1814b3996c94d6e0656759171876e93c26063e2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestFunction

## Description
Five net-new root causes in the CloudFront Functions surface (CreateFunction/UpdateFunction/PublishFunction/DescribeFunction/TestFunction/ListFunctions). All five are in the `cloudfront` service and all are grounded in failing behavioral checks that are neither OWNED by an open issue nor blocked by an unimplemented scaffold stub. A fixer must close all five before submitting.

--------------------------------------------------------------------
Cause 1 of 5: TestFunction's TestResult omits FunctionErrorMessage when the function ran without error; AWS always returns it as an EMPTY STRING.
Failing checks: cloudfront.function-test-rewrites-tenant-directory-uris-to-index-document, cloudfront.function-duplicate-name-rejected

AWS: Smithy models TestResult.FunctionErrorMessage (target sensitiveStringType, sensitive) — "If the result of testing the function was an error, this field contains the error message." The model marks no member of TestResult required, but the recorded behavior shows real CloudFront ALWAYS serializes the member on a 200 TestFunction response and sets it to the empty string when the run raised nothing (recorded behavior only for the empty-vs-absent distinction; the suite's own comment states "Real CloudFront reports 'no error' as an EMPTY STRING, not an absent/None field"). So the requirement is: a TestFunction that returns 200 carries FunctionErrorMessage; its value is "" when the run returned normally and the thrown/raised text when it did not.
Emulator: the member is absent from the XML payload entirely for a successful run, so botocore's parsed TestResult has no `FunctionErrorMessage` key and the client raises KeyError: 'FunctionErrorMessage'. Observed in both checks at the first post-test assertion.
Why: every client that reads `TestResult["FunctionErrorMessage"]` (or `.function_error_message` in a typed SDK and then compares it to "") to decide whether a function test succeeded crashes or mis-branches against the emulator. The normal "did my function pass?" idiom — compare the error message to the empty string — is unavailable, so a passing test is indistinguishable from a malformed response.
Fix: TestFunction's handler/response construction — the outcome-to-TestResult mapping that decides which TestResult members are populated.
Traces: services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/test_function.rs lines 264-298 build `(function_output, function_error_message)` from `run.outcome` and leave `function_error_message` as `None` on `FunctionOutcome::Returned`.

--------------------------------------------------------------------
Cause 2 of 5: TestFunction's TestResult omits FunctionOutput when the function threw or failed to compile; AWS returns an empty JSON object document.
Failing checks: cloudfront.function-test-throwing-rewrite-fails-with-empty-output, cloudfront.function-with-syntax-error-publishes-then-fails-at-test

AWS: Smithy models TestResult.FunctionOutput (target sensitiveStringType, sensitive) — "The event object returned by the function." Recorded behavior: a function that mutates the request and then throws, and a function whose source does not parse at all, both return 200 with FunctionOutput present and carrying an EMPTY JSON OBJECT document (so `json.loads(FunctionOutput) == {}`) plus FunctionErrorMessage carrying the engine's text (containing "Error: <thrown message>" for a throw, and "SyntaxError" for unparseable source). The fixed requirement is the pairing: on a failed run FunctionOutput is present and is an empty JSON object — no partial mutation leaks past the throw — and FunctionErrorMessage carries the failure text; the failure text itself is a value produced by the run, not a literal.
Emulator: FunctionOutput is absent from the payload for a failed run, so the parsed TestResult has no `FunctionOutput` key and the client raises KeyError: 'FunctionOutput'. Both checks fail at `json.loads(res["FunctionOutput"]) == {}`.
Why: a client cannot read the output of a failed test at all. Code that reads FunctionOutput unconditionally (the documented shape of the response) crashes, and code that treats a missing FunctionOutput as "no mutation observed" cannot distinguish "the function threw and nothing leaked" from "the service returned a truncated response".
Fix: the same TestFunction outcome-to-TestResult mapping as Cause 1 — the failed branch must still emit FunctionOutput as an empty JSON object document rather than omitting it.
Traces: services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/test_function.rs lines 264-284, `FunctionOutcome::Failed(message) => (None, Some(message))`.

--------------------------------------------------------------------
Cause 3 of 5: TestFunction refuses a well-formed but non-current If-Match version with InvalidIfMatchVersion/400 instead of PreconditionFailed/412.
Failing check: cloudfront.function-development-edit-leaves-live-rewrite-until-republished

AWS: a TestFunction that names Stage=LIVE while carrying the DEVELOPMENT stage's version token is refused with error code `PreconditionFailed`, HTTP 412, and the message "The request failed because it didn't meet the preconditions in one or more request-header fields." That sentence is entirely FIXED text for this error class — it has no variable parts. Smithy: PreconditionFailed is modeled in this service (errorKind client, httpError 412, "The precondition in one or more of the request fields evaluated to false") and is the modeled refusal for a stale version token on its sibling version-gated operations (e.g. PublishFunction lists both InvalidIfMatchVersion/400 and PreconditionFailed/412). TestFunction's own error list in the model names only InvalidIfMatchVersion/400, so for TestFunction specifically the 412/PreconditionFailed outcome is recorded behavior only — the recording is authoritative over the operation's error list here. InvalidIfMatchVersion remains correct for a token that is missing or not a version token at all; a well-formed token that is not the current one is the failed precondition.
Emulator: answers error code 'InvalidIfMatchVersion' (HTTP 400). The handler's comment states the choice explicitly ("This operation models NO failed-precondition error, so both a header that carries no version token at all and one carrying a token that is not the current one are 'The If-Match version is missing or not valid'"), i.e. the two conditions are deliberately collapsed onto one code.
Why: a client that distinguishes a retryable optimistic-concurrency conflict (412 PreconditionFailed — re-read the ETag and retry) from a malformed request (400 InvalidIfMatchVersion — a client bug, do not retry) takes the wrong branch. Against the emulator a stale-ETag test looks like a permanently bad request, so retry logic that re-describes the function and retries never runs.
Fix: TestFunction's If-Match validation — split the missing/not-a-token case from the not-current case, as PublishFunction's handler already does, and return the modeled PreconditionFailed with its fixed message for the latter.
Traces: services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/test_function.rs lines 200-215; compare services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/publish_function.rs lines 177-196, which already branches IfMatch::Invalid vs IfMatch::Stale.

--------------------------------------------------------------------
Cause 4 of 5: PublishFunction rotates the function's version token, so DescribeFunction reports different ETags for the DEVELOPMENT and LIVE stages of the same function.
Failing check: cloudfront.function-development-to-live-publish-lifecycle

AWS: PublishFunction copies the DEVELOPMENT stage to LIVE and does NOT mint a new version token: DescribeFunction(Name, Stage="DEVELOPMENT") and DescribeFunction(Name, Stage="LIVE") return the SAME ETag immediately after a publish, and that ETag equals the one CreateFunction/UpdateFunction last returned. Smithy: DescribeFunction output carries ETag as the `ETag` HTTP header; PublishFunction models no new-version output member at all (its only output is FunctionSummary). Recorded behavior: the token is unchanged across the publish. Only a subsequent UpdateFunction mints a new token (for DEVELOPMENT).
Emulator: the LIVE stage gets a freshly minted token. The check observed DEVELOPMENT and LIVE reporting two different values after a single publish (the specific token strings are per-run values and are not part of the requirement; the requirement is the equality).
Why: the documented publish flow is describe -> publish -> use the same ETag for the next version-gated call. A client that holds the ETag across a publish and then calls UpdateFunction, DeleteFunction or TestFunction with it is refused by the emulator for a precondition real AWS considers satisfied, and a client that compares the two stages' ETags to detect "is LIVE the same code as DEVELOPMENT?" always concludes they diverge.
Fix: PublishFunction's state mutation — the publish must copy the DEVELOPMENT stage record including its version token rather than generating a new one for the LIVE record.
Traces: services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/publish_function.rs line 201 (`store.publish_function(&input.name, now)`) and the `publish_function` routine it calls in services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/state.rs.

--------------------------------------------------------------------
Cause 5 of 5: ListFunctions with no Stage filter lists only the DEVELOPMENT stage, instead of every stage each function holds.
Failing check: cloudfront.function-live-stage-before-publish-rejected

AWS: Smithy models ListFunctions.Stage as an OPTIONAL httpQuery member of the FunctionStage enum (DEVELOPMENT | LIVE), documented as "An optional filter to return only the functions that are in the specified stage." A request that supplies no Stage therefore applies NO filter: a published function appears once per stage it holds, so paging the whole collection yields one FunctionSummary with FunctionMetadata.Stage == "DEVELOPMENT" and one with "LIVE" for the same Name. An unpublished function appears once, at DEVELOPMENT.
Emulator: an absent Stage is substituted with the DEVELOPMENT default and then used as a filter, so a published function is listed once, at DEVELOPMENT only. The check paged all of ListFunctions and found the function listed at ['DEVELOPMENT'] where AWS lists ['DEVELOPMENT', 'LIVE'].
Why: a client enumerating functions to find what is live (the standard way to answer "which functions are published?") sees no LIVE entries at all against the emulator, and FunctionList.Quantity under-reports the collection. Code that reconciles the listing against DescribeFunction(Stage="LIVE") — which does report the LIVE stage — sees the two views contradict each other.
Fix: ListFunctions' handler — when Stage is absent, emit one item per stage record each function holds instead of defaulting the filter to DEVELOPMENT; keep the single-stage projection when Stage is supplied.
Traces: services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/list_functions.rs lines 165-204, where `input.stage.as_deref().unwrap_or(crate::state::DEFAULT_FUNCTION_STAGE)` turns an absent filter into a DEVELOPMENT-only filter and the loop then `continue`s past any function lacking that stage record.

## Evidence
Suite: simparity-behavioral-aws-v1, 408/632 passed. Seven failing checks under `### cloudfront` carry neither an OWNED-by marker nor an ALREADY-FILED (unimplemented-stub) marker; all seven are clustered here.

Cause 1 — FunctionErrorMessage absent on success:
  [error] cloudfront.function-test-rewrites-tenant-directory-uris-to-index-document
    check_..._function_test_rewrites_tenant_directory_uris_to_index_document.py line 30, in run
      assert res["FunctionErrorMessage"] == "", f"{uri!r} raised {res['FunctionErrorMessage']!r}"
    KeyError: 'FunctionErrorMessage'
  [error] cloudfront.function-duplicate-name-rejected
    check_..._function_duplicate_name_rejected.py line 45, in run
      assert result["FunctionErrorMessage"] == "", ...
    KeyError: 'FunctionErrorMessage'
  Expected: 200 with TestResult.FunctionErrorMessage present and equal to "" for a run that returned normally.
  Actual:   200 with the member omitted from the response payload.

Cause 2 — FunctionOutput absent on failure:
  [error] cloudfront.function-test-throwing-rewrite-fails-with-empty-output
    check_..._function_test_throwing_rewrite_fails_with_empty_output.py line 39, in run
      assert json.loads(res["FunctionOutput"]) == {}, ...
    KeyError: 'FunctionOutput'
  [error] cloudfront.function-with-syntax-error-publishes-then-fails-at-test
    check_..._function_with_syntax_error_publishes_then_fails_at_test.py line 41, in run
      assert json.loads(res["FunctionOutput"]) == {}, ...
    KeyError: 'FunctionOutput'
  Expected: 200 with FunctionOutput present and parsing to {} , and FunctionErrorMessage containing "Error: tenant lookup failed" (the thrown text) resp. "SyntaxError" (the parse failure) — the control function in each check, with the same event, returns the rewritten URI, so the engine itself runs.
  Actual:   200 with FunctionOutput omitted entirely.

Cause 3 — stale If-Match on TestFunction:
  [mismatch] cloudfront.function-development-edit-leaves-live-rewrite-until-republished
    check_..._function_development_edit_leaves_live_rewrite_until_republished.py line 40, in run
      assert code == "PreconditionFailed", f"testing LIVE with the DEVELOPMENT ETag returned {code!r}"
    AssertionError: testing LIVE with the DEVELOPMENT ETag returned 'InvalidIfMatchVersion'
  Expected: error code PreconditionFailed, HTTP 412, message (fixed text, no variable parts) "The request failed because it didn't meet the preconditions in one or more request-header fields."
  Actual:   error code InvalidIfMatchVersion, HTTP 400.

Cause 4 — ETag rotated by publish:
  [mismatch] cloudfront.function-development-to-live-publish-lifecycle
    check_..._function_development_to_live_publish_lifecycle.py line 65, in run
      assert live_desc.get("ETag") == dev_etag, ...
    AssertionError: publish must not rotate the ETag: DEVELOPMENT was 'E7IYCGPKTANSF3', LIVE is 'E0LC535G268HO1'
  Expected: DescribeFunction(Stage="LIVE")["ETag"] == the ETag CreateFunction returned, immediately after PublishFunction with that same ETag. (The two token strings above are per-run values; the requirement is the equality, not the literals.)
  Actual:   two different tokens.

Cause 5 — ListFunctions ignores the unset filter:
  [mismatch] cloudfront.function-live-stage-before-publish-rejected
    check_..._function_live_stage_before_publish_rejected.py line 48, in run
      assert sorted(stages) == ["DEVELOPMENT", "LIVE"], f"ListFunctions lists {name!r} at {stages!r}"
    AssertionError: ListFunctions lists '<generated-name>' at ['DEVELOPMENT']
  Expected: after PublishFunction, paging ListFunctions with no Stage parameter yields the name twice, once with FunctionMetadata.Stage == "DEVELOPMENT" and once with "LIVE".
  Actual:   the name appears once, at DEVELOPMENT.
  (The earlier assertions in this check — DescribeFunction/TestFunction on LIVE before publish answering NoSuchFunctionExists with "The specified function does not exist.", and LIVE describing as UNASSOCIATED after publish — all passed, so only the listing is diverging.)

## Steps to Reproduce
All five reproduce against the booted emulator with a botocore/boto3 `cloudfront` client. `_REWRITE_SRC` below is the viewer-request rewrite the checks use:

    function handler(event) {
        var request = event.request;
        var uri = request.uri;
        if (uri.endsWith('/')) { request.uri += 'index.html'; }
        else if (!uri.includes('.')) { request.uri += '/index.html'; }
        return request;
    }

and EVENT is the viewer-request test event
    {"version":"1.0","context":{"eventType":"viewer-request"},"viewer":{"ip":"198.51.100.7"},
     "request":{"method":"GET","uri":"/basalt-tenant/","headers":{"host":{"value":"tenants.example.com"}},"cookies":{},"querystring":{"v":{"value":"1"}}}}

Cause 1:
 1. made = cf.create_function(Name=N, FunctionConfig={"Comment":"c","Runtime":"cloudfront-js-2.0"}, FunctionCode=_REWRITE_SRC)
 2. res = cf.test_function(Name=N, IfMatch=made["ETag"], EventObject=EVENT)["TestResult"]
 3. res["FunctionErrorMessage"] -> KeyError. AWS: the key is present and equals "".

Cause 2:
 1. Create a function whose handler mutates the request and then `throw new Error('tenant lookup failed');` (and, separately, one whose source is the unterminated `function handler(event) { var request = event.request; return request `).
 2. res = cf.test_function(Name=N, IfMatch=made["ETag"], EventObject=EVENT)["TestResult"]
 3. res["FunctionOutput"] -> KeyError. AWS: present, parses to {}, and res["FunctionErrorMessage"] carries "Error: tenant lookup failed" / contains "SyntaxError".

Cause 3:
 1. made = cf.create_function(...); cf.publish_function(Name=N, IfMatch=made["ETag"])
 2. dev = cf.update_function(Name=N, IfMatch=made["ETag"], FunctionConfig={...}, FunctionCode=<edited source>)["ETag"]
 3. cf.test_function(Name=N, IfMatch=dev, Stage="LIVE", EventObject=EVENT)
 4. Emulator raises InvalidIfMatchVersion/400; AWS raises PreconditionFailed/412 with "The request failed because it didn't meet the preconditions in one or more request-header fields."

Cause 4:
 1. made = cf.create_function(...); dev_etag = made["ETag"]
 2. cf.publish_function(Name=N, IfMatch=dev_etag)
 3. cf.describe_function(Name=N, Stage="LIVE")["ETag"] != dev_etag on the emulator; AWS returns the same token.

Cause 5:
 1. made = cf.create_function(...); cf.publish_function(Name=N, IfMatch=made["ETag"])
 2. Page cf.list_functions() with no Stage argument, following NextMarker to the end, collecting i["FunctionMetadata"]["Stage"] for every item whose Name == N.
 3. Emulator yields ['DEVELOPMENT']; AWS yields ['DEVELOPMENT', 'LIVE'].
