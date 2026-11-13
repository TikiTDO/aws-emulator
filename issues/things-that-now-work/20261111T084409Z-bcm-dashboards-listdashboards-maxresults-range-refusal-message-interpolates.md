Issue-Source: trace-sweep-divergence
Issue-Trace: bcm-dashboards-list-dashboards-max-results-above-range-rejected
Issue-Step: task#1
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bcm-dashboards-list-dashboards-max-results-above-range-rejected
Issue-Shape-Id: com.amazonaws.bcmdashboards#AWSBCMDashboardsService
Issue-Key: bcm-dashboards|ListDashboards|4xx->4xx|$.message
Issue-Commit: c3de02674d16064d9f5c8a9be8af17d3d848d40e

# bcm-dashboards ListDashboards maxResults range refusal message interpolates the rejected value

> _Note: this issue was identified against the code base at commit `c3de02674d16064d9f5c8a9be8af17d3d848d40e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDashboards

## Description
Cause 1 of 1: the boundary range check that refuses an out-of-range `maxResults` on ListDashboards builds its own refusal message, embedding the rejected value in the sentence. AWS's message for this constraint class names the member and the modelled bound but does NOT echo the submitted value, so the emulator's 400 carries text no AWS client ever sees.

AWS: Smithy: ListDashboards input member `maxResults` -> MaxResults, integer, @default 20, @range min 1 max 100; modelled errors AccessDeniedException (403), ValidationException (400), ThrottlingException (429), InternalServerException (500). A `maxResults` above the modelled maximum is a client constraint violation, so AWS answers HTTP 400 with error code ValidationException (the model documents it as "The input parameters do not satisfy the requirements. Check the error message for specific validation details."). The exact wording of the constraint sentence is recorded behavior only: the recording shows the message "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100". Its FIXED text is "<n> validation error(s) detected: Value at '<member>' failed to satisfy constraint: Member must have value less than or equal to <max>"; its VARIABLE parts are the violation count, the member name taken from the request (`maxResults`) and the ceiling taken from the member's modelled @range maximum (100). The rejected value itself is NOT one of the variable parts for this member — AWS writes "Value at 'maxResults'", not "Value '<submitted>' at 'maxResults'".

Emulator: the status and the error code are right — HTTP 400, ValidationException — but the message is the emulator's own: "1 validation error detected: Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100". It inserts a "Value '<submitted>' " clause that AWS's message for this member does not contain (101 is merely the value this recording submitted; the defect is the extra interpolated clause, in any request that trips the ceiling, not that one number).

Why: the status and code are correct, so a client that only branches on those is unaffected; the breakage is for anything that reads the message. Clients and test suites that assert on the refusal text (a regex over "Value at 'maxResults' failed to satisfy constraint", a snapshot of the ValidationException message, or a log/alert matcher) fail against the emulator while passing against AWS, and a human reading the emulator's message is told a value-echoing format AWS does not use — which is also the wrong template to copy when adding the next range check.

Fix: the shared boundary constraint validator that enforces @range before the handler runs (ListDashboards' own handler never sees the request — services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/src/ops/list_dashboards.rs documents at its module head that `maxResults` outside 1..100 "is refused by the boundary validator before this handler runs", and its `handle` body only maps `crate::state::listing_page` refusals). The message formatter for an integer @range violation must emit "Value at '<member>'" and must not interpolate the submitted value; the status (400) and error shape (ValidationException) already match and need no change. No change belongs in the ListDashboards handler, its pagination projection or its serializer.

Traces: bcm-dashboards-list-dashboards-max-results-above-range-rejected#task#1 (cluster of 1, verdict mismatch, expected 400 / actual 400, divergence confined to $.message). Related open issue 20261103T200742Z-ec2-ssm-ssm-describeinstanceinformation-refusal-messages-are-emulator-authored-maxresults.md reports the same value-echoing habit for SSM DescribeInstanceInformation, but it is a different service and does not carry this fixture step's replay key, so it does not cover this divergence.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up=true ("/__admin/health answered 200 and the :41465 data listener accepts"), bootstrap_signal=false, fixtures_replayed=9, steps_total=49, steps_passed=42, steps_mismatched=7, steps_errored=0, steps_blocked=0, divergence_count=4, score_pct=85.7.

Divergence cluster (verdict=mismatch, service=bcm-dashboards, operation=ListDashboards, stub_path empty, owned_by empty, affected_count=1):
  fixture_id: bcm-dashboards-list-dashboards-max-results-above-range-rejected
  step_key:   task#1
  expected_status: 400, actual_status: 400
  divergence_key: service=bcm-dashboards|ListDashboards|status=400->400|keys=$.message
  detail: $.message: value_mismatch (expected "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100", got "1 validation error detected: Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100")

Expected (AWS, recorded):
  HTTP 400, ValidationException
  message: "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100"

Actual (emulator):
  HTTP 400, ValidationException
  message: "1 validation error detected: Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100"

The only diverging field is $.message; the HTTP status and the error code agree. Breaking the message down: fixed text "1 validation error detected: Value at '" + member + "' failed to satisfy constraint: Member must have value less than or equal to " + bound. Variable parts: the member name `maxResults`, taken from the request, and the bound `100`, taken from the member's modelled @range maximum. The emulator's surplus "Value '101' " clause carries the request's submitted value, which AWS's message for this member does not include.

Smithy consulted (smithy_query query="operation", operation="ListDashboards", model_path=services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/smithy-model/com-amazonaws-bcmdashboards-awsbcmdashboardsservice.json): input {maxResults -> MaxResults, integer, default 20, range min 1 max 100; nextToken -> NextPageToken length 1..8192 pattern ^[\S\s]*$}, output {dashboards required, nextToken}, errors AccessDeniedException 403 / InternalServerException 500 / ThrottlingException 429 / ValidationException 400, @readonly, @paginated(pageSize=maxResults, items=dashboards). The model fixes the bound (100) and the error shape; it does not fix the message wording, which is why the wording above is taken from the recorded response.

## Steps to Reproduce
1. Clone this checkout and boot the emulator from it; confirm the bcm-dashboards service answers (/__admin/health is 200).
2. Replay the corpus: replay_wire_corpus() for trace set simparity-wire-aws-v1. The summary reports steps_mismatched=7 in 4 clusters, all service=bcm-dashboards.
3. Locate the cluster with fixture_id=bcm-dashboards-list-dashboards-max-results-above-range-rejected and step_key=task#1 (operation ListDashboards, expected 400 / actual 400, verdict mismatch, owned_by empty, stub_path empty). Its detail is the single $.message value_mismatch quoted in the evidence.
4. Equivalently by hand: call ListDashboards with maxResults set above the modelled maximum of 100 (the fixture sends 101). Both AWS and the emulator answer HTTP 400 with error code ValidationException. Compare the `message` field: AWS writes "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100"; the emulator writes the same sentence with an extra "Value '<submitted>' " clause before "at 'maxResults'".
5. Confirm the handler is not the source: read services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/src/ops/list_dashboards.rs — the module doc (lines 9-10) and the `handle` body (lines 148-185) show the @range 1..100 check is applied by the shared boundary validator before `handle` runs, so the message is produced by that validator's @range formatter.
6. After fixing the @range message formatter so it emits "Value at '<member>'" without interpolating the submitted value, re-replay fixture bcm-dashboards-list-dashboards-max-results-above-range-rejected step task#1 and confirm the cluster disappears and the step passes.
