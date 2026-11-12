Issue-Source: trace-sweep-divergence
Issue-Trace: cost-optimization-hub-efficiency-metrics-workflow
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cost-optimization-hub-efficiency-metrics-workflow
Issue-Shape-Id: com.amazonaws.costoptimizationhub#CostOptimizationHubService
Issue-Key: cost-optimization-hub|ListEfficiencyMetrics|4xx->4xx|$.message
Issue-Commit: 2e3afc6c699aa7e5b0dfd114974fd7ed6d393583

# ListEfficiencyMetrics refuses an inverted timePeriod with the wrong ValidationException message text

> _Note: this issue was identified against the code base at commit `2e3afc6c699aa7e5b0dfd114974fd7ed6d393583`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListEfficiencyMetrics

## Description
Cause 1 of 1: when ListEfficiencyMetrics is called with a timePeriod whose start is not before its end, the emulator correctly refuses the request with HTTP 400 and a ValidationException, but the error's `message` is worded differently from AWS's, so a client reading the message is told the wrong rule. Fixture cost-optimization-hub-efficiency-metrics-workflow, step task#5 (cluster also covers task#8).

AWS: ListEfficiencyMetrics takes a required `timePeriod` (shape TimePeriod, documented "Specifies a date range for retrieving efficiency metrics. The start date is inclusive and the end date is exclusive."; the member doc adds that dates are given "in either YYYY-MM-DD format or YYYY-MM format depending on the desired granularity") and a required `granularity` (GranularityType: Daily | Monthly). Smithy: ListEfficiencyMetrics declares errors ValidationException (errorKind client, httpError 400), AccessDeniedException (403), ThrottlingException (429), InternalServerException (500); the refusal class and status are therefore modeled, but the model states no message text for this rule, so the WORDING is recorded behavior only. The recording shows HTTP 400 with awsJson `__type` = "com.amazonaws.costoptimizationhub#ValidationException" and `message` = "Invalid TimePeriod specified. End month must be after start month." In that message everything is FIXED text — AWS names the input member in its shape capitalization (`TimePeriod`, not the wire member spelling `timePeriod`), and states the rule in the unit the requested granularity reports in (for Monthly granularity, months: the end month must be AFTER the start month). No part of it is taken from the request or from state: the offending start and end values are not interpolated, so no per-run date, account id or timestamp appears in it. The companion diagnosis members AWS can carry (`reason`, `fields`) are not part of this divergence — only the top-level `message` differs.

Emulator: it answers HTTP 400 with __type "com.amazonaws.costoptimizationhub#ValidationException" (both correct) but `message` = "Invalid timePeriod specified. The start date must be earlier than the end date." Replay detail verbatim: "$.message: value_mismatch (expected 'Invalid TimePeriod specified. End month must be after start month.', got 'Invalid timePeriod specified. The start date must be earlier than the end date.')". Three things differ from AWS's fixed text: the member is spelled `timePeriod` instead of `TimePeriod`; the subject is "The start date" / "the end date" instead of AWS's granularity-unit phrasing "End month" / "start month"; and the relation is stated as "must be earlier than" on the start rather than "must be after" on the end. The emulator's text is a single granularity-agnostic sentence, so it does not restate the rule in the unit the request's `granularity` asked for.

Why: a client relying on AWS's behavior mis-handles the refusal. The HTTP status and error code are right, so generic retry/catch logic survives, but anything that reads the message gets the wrong thing: SDK-level or test-level assertions that match AWS's wording (including the capitalized member name `TimePeriod`) do not match, log-scraping and error-classification rules keyed on "End month must be after start month" never fire, and a user-facing surface that echoes the service message tells the operator to change the start date when AWS tells them the end month must move. A conformance test that asserts the recorded message passes against AWS and fails against the emulator.

Fix: the operation's validation — specifically the refusal builder for an empty/inverted period in the ListEfficiencyMetrics handler's `timePeriod` check (the `empty_period()` refusal reached from `period()` when `start >= end`). It must emit AWS's fixed text, with the member named as `TimePeriod` and the rule stated on the end bound in the unit the requested granularity reports in, rather than the emulator's own date-ordering sentence; the status (400), __type and the attributed field diagnosis it already produces are correct and should not change. Note the three open cost-optimization-hub issues are unrelated and do not cover this message: 20261102T122202Z-...-listefficiencymetrics-does-not-gate-on-aws-compute-optimizer is about a MISSING opt-in precondition (400 vs 200) on a request whose timePeriod is valid, 20261106T133619Z-...-updatepreferences-... is about UpdatePreferences/GetPreferences, and 20261029T180732Z-...-listrecommendations-maintainability--r3 is a state.rs refactor.

Traces: cost-optimization-hub-efficiency-metrics-workflow#task#5 and #task#8 (one cluster of 2).

## Evidence
Fixture cost-optimization-hub-efficiency-metrics-workflow, step task#5 (cluster also affects task#8), operation ListEfficiencyMetrics, verdict mismatch.

Expected (recorded AWS response):
  HTTP 400
  $.__type  = "com.amazonaws.costoptimizationhub#ValidationException"
  $.message = "Invalid TimePeriod specified. End month must be after start month."

Actual (emulator):
  HTTP 400  (matches)
  $.__type  = "com.amazonaws.costoptimizationhub#ValidationException"  (matches)
  $.message = "Invalid timePeriod specified. The start date must be earlier than the end date."

Replay detail verbatim: "$.message: value_mismatch (expected 'Invalid TimePeriod specified. End month must be after start month.', got 'Invalid timePeriod specified. The start date must be earlier than the end date.')". Divergence key: service=cost-optimization-hub|ListEfficiencyMetrics|status=400->400|keys=$.message.

Message anatomy: the whole AWS sentence is fixed text — "Invalid TimePeriod specified. End month must be after start month." — with the input member named in its shape capitalization (TimePeriod) and the rule stated on the END bound in the granularity's unit (month, for Monthly granularity). Nothing in it is interpolated from the request or state, so neither the submitted start/end dates nor any per-run value belongs in it.

Model support: smithy_query(operation="ListEfficiencyMetrics", model_path="services/service-com-amazonaws-costoptimizationhub-costoptimizationhubservice/smithy-model/com-amazonaws-costoptimizationhub-costoptimizationhubservice.json") -> required members granularity (GranularityType enum) and timePeriod (TimePeriod); errors ValidationException errorKind=client httpError=400, AccessDeniedException 403, ThrottlingException 429, InternalServerException 500. smithy_query(query="doc", shape="TimePeriod") -> "Specifies a date range for retrieving efficiency metrics. The start date is inclusive and the end date is exclusive." The model fixes the status and error shape but not the message wording, so the wording requirement is the recorded response.

Emulator source carrying the diverging text: services/service-com-amazonaws-costoptimizationhub-costoptimizationhubservice/src/ops/list_efficiency_metrics.rs lines 813-820 (`empty_period()`), reached from `period()` at lines 545-553 (`if start >= end { return Err(empty_period()); }`), which the handler calls at line 243.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay fixture cost-optimization-hub-efficiency-metrics-workflow and stop at step task#5 (step task#8 is the same cluster) — the ListEfficiencyMetrics call whose timePeriod start is not before its end.
3. Equivalently, by hand: POST to the cost-optimization-hub endpoint with header X-Amz-Target: AWSCostOptimizationHubService.ListEfficiencyMetrics and a body setting granularity "Monthly" and a timePeriod in YYYY-MM form whose end month is not after its start month (e.g. start and end the same month, or end before start).
4. Observe HTTP 400 with __type "com.amazonaws.costoptimizationhub#ValidationException" and message "Invalid timePeriod specified. The start date must be earlier than the end date." Expected: the same 400 and __type, but message "Invalid TimePeriod specified. End month must be after start month."
