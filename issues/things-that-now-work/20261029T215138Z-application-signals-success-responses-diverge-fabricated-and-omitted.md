Issue-Source: trace-sweep-divergence
Issue-Trace: application-signals-slo-exclusion-windows
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: application-signals-slo-exclusion-windows application-signals-instrumentation-config-errors application-signals-instrumentation-config-lifecycle application-signals-slo-budget-report-batch application-signals-discovery-and-service-topology
Issue-Shape-Id: com.amazonaws.applicationsignals#ApplicationSignals
Issue-Key: application-signals|BatchUpdateExclusionWindows|2xx->4xx|$.Errors
Issue-Key: application-signals|BatchUpdateExclusionWindows|2xx->4xx|$.SloIds
Issue-Key: application-signals|BatchUpdateExclusionWindows|2xx->4xx|status
Issue-Key: application-signals|ListServiceLevelObjectiveExclusionWindows|2xx->2xx|$.ExclusionWindows[*]
Issue-Key: application-signals|CreateInstrumentationConfiguration|2xx->2xx|$.ARN
Issue-Key: application-signals|CreateInstrumentationConfiguration|2xx->2xx|$.LocationHash
Issue-Key: application-signals|DeleteInstrumentationConfiguration|2xx->4xx|$.DeletionStatus
Issue-Key: application-signals|DeleteInstrumentationConfiguration|2xx->4xx|status
Issue-Key: application-signals|GetInstrumentationConfigurationStatus|2xx->2xx|$.Status
Issue-Key: application-signals|BatchGetServiceLevelObjectiveBudgetReport|2xx->2xx|$.Errors[*].ErrorCode
Issue-Key: application-signals|BatchGetServiceLevelObjectiveBudgetReport|2xx->2xx|$.Errors[*].Name
Issue-Key: application-signals|GetService|2xx->2xx|$.Service.KeyAttributes
Issue-Key: application-signals|GetService|2xx->2xx|$.Service.MetricReferences
Issue-Commit: 89c27cbcf326b5c1a1abaeca904759cc614d9072

# application-signals success responses diverge: fabricated and omitted members, a non-AWS location hash, and a valid exclusion-window update refused 400

> _Note: this issue was identified against the code base at commit `89c27cbcf326b5c1a1abaeca904759cc614d9072`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchUpdateExclusionWindows

## Description
Seven wire-fixture clusters show application-signals diverging on NON-refusal paths: two requests AWS answers 200 are answered 400/404 by the emulator, and five successful responses carry member sets or values the recorded AWS responses do not. This issue bundles SEVEN causes; all seven must be closed.

--- Cause 1 of 7: BatchUpdateExclusionWindows refuses 400 a request AWS answers 200 ---
AWS: Smithy: BatchUpdateExclusionWindows PATCH /exclusion-windows; input SloIds (@required, 1..50), AddExclusionWindows / RemoveExclusionWindows (both optional, max 10); output SloIds (@required, min 1) and Errors (@required, BatchUpdateExclusionWindowsErrors, max 10) — i.e. per-ITEM error reporting; modeled errors ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400). Recorded behavior: the step is answered 200 with SloIds = [the named SLO's ARN] and Errors = [] (empty), and the SLO's window set read immediately afterwards is UNCHANGED — so AWS neither failed the call nor mutated the set, i.e. an exclusion-window update whose removal matches nothing is a successful no-op reported in SloIds, not a failure.
Emulator: 400 ValidationException; no SloIds and no Errors member at all.
Why: a client that reconciles exclusion windows by issuing its desired add/remove set gets a hard client error instead of success, so the whole batch appears rejected and the SLO ids it did process are never reported; retries never converge.
Fix: `ComAmazonawsApplicationsignalsApplicationsignalsState::update_exclusion_windows` in src/state.rs (the `ExclusionWindowFailure::UnmatchedRemoval` arm) and the all-ids-failed branch of src/ops/batch_update_exclusion_windows.rs that promotes a PER-ITEM failure into a top-level exception. Hypothesis for the mechanism: the step's RemoveExclusionWindows entry does not match the stored window member-for-member (`same_exclusion_window`) — plausibly because the stored window's StartTime/RecurrenceRule are themselves wrong, see Cause 2 — so the single SloId fails, `applied` is empty and the handler converts the per-item failure into a top-level 400. Even where a per-item failure is right, the modeled contract reports it in `Errors` beside the ids that were applied.
Traces: application-signals-slo-exclusion-windows#task#5.

--- Cause 2 of 7: listed exclusion windows drop RecurrenceRule and lose sub-second StartTime ---
AWS: Smithy: ListServiceLevelObjectiveExclusionWindows GET /slo/{Id}/exclusion-windows; ExclusionWindow members Window (@required: DurationUnit + Duration), StartTime (optional Timestamp, doc: "Defaults to current time if not specified"), RecurrenceRule (optional; its Expression is @required, length 1..1024), Reason. Recorded behavior: each listed window carries Window, Reason, a StartTime serialized as epoch seconds WITH millisecond precision (a fractional JSON number), and a `RecurrenceRule` member that is PRESENT with no Expression value for a window added without a recurrence.
Emulator: the same window is serialized with Window and Reason but NO `RecurrenceRule` member at all, and StartTime truncated to a whole number of epoch seconds (src/state.rs `stored_exclusion_window` stores `DateTime::from_secs(served_at.secs())`).
Why: the member set a client reads back is the identity a RemoveExclusionWindows must name member-for-member, so a client that lists windows and asks to remove one sends a window whose shape never matches what AWS would have handed it; the truncated StartTime also loses the sub-second anchor of the excluded period. NOTE: the ABSOLUTE StartTime value differs between recording and replay only because both AWS and the emulator default an omitted StartTime to the instant of the call — that difference is a clock value and is NOT part of the requirement. The requirement is the member set and the precision.
Fix: the ExclusionWindow serialization/storage in src/state.rs (`stored_exclusion_window` and the window projection the listing returns) — keep the RecurrenceRule member in the stored/served shape and keep sub-second precision on the defaulted StartTime.
Traces: application-signals-slo-exclusion-windows#task#4 (cluster also covers task#6 of the same fixture).

--- Cause 3 of 7: CreateInstrumentationConfiguration mints a LocationHash AWS would not mint ---
AWS: Smithy: CreateInstrumentationConfiguration output `LocationHash` (@required, length exactly 16) and `ARN` (@required, pattern ^arn:[^:]+:application-signals:[^:]+:[0-9]{12}:instrumentationConfig/.+/[0-9a-f]{16}$); the member is documented as a stable hash computed from the location that identifies the instrumentation point within the service, environment and signal type. Recorded behavior only for the algorithm, which AWS does not document: for a given Location the recording shows one specific 16-hex value, and the recording's own later steps address the configuration by that value.
Emulator: for the SAME request it mints a different 16-hex value — `crate::state::location_hash` renders the first 8 bytes of a SHA-256 over the CodeLocation's members (src/state.rs `location_hash`/`stable_hash`) — so both `LocationHash` and the `ARN` built from it diverge, while still satisfying the modeled constraints.
Why: the hash is the addressable identity of an instrumentation point. Any client or template that holds a LocationHash or ARN obtained from AWS (or from a previous environment) cannot resolve its configuration here, and nothing that records an ARN is portable — Cause 4 is exactly that failure happening inside the corpus.
Fix: `crate::state::location_hash` in src/state.rs (the hashing of a Location into the 16 hex characters the create returns and the store keys records by).
Traces: application-signals-instrumentation-config-errors#setup#1 (cluster also covers application-signals-instrumentation-config-lifecycle#setup#1).

--- Cause 4 of 7: DeleteInstrumentationConfiguration answers 404 for a configuration that exists ---
AWS: Smithy: DeleteInstrumentationConfiguration POST /delete-instrumentation-configuration, output DeletionStatus (@required, DynamicInstrumentationDeletionStatus), success code 200. Recorded behavior: deleting a configuration by the `LocationIdentifier.LocationHash` the create returned is answered 200 with DeletionStatus `DELETED`.
Emulator: 404 (ResourceNotFoundException) with no DeletionStatus member, i.e. a live configuration is not deletable by the hash that identifies it.
Why: the resource cannot be reaped by the identifier the service handed out, so a client's cleanup path fails and the configuration leaks. Hypothesis: this is the downstream effect of Cause 3 — the step names the LocationHash from the recording, which the emulator's own hashing never produces, so the lookup misses. It is filed and must be re-replayed separately because it is its own cluster and its own observable break (200+DELETED vs 404).
Fix: same component as Cause 3 (`crate::state::location_hash` and the hash-keyed lookup the delete handler performs); verify by replaying this step, not only Cause 3's.
Traces: application-signals-instrumentation-config-errors#teardown#5.

--- Cause 5 of 7: GetInstrumentationConfigurationStatus serializes Status in the wrong case ---
AWS: Smithy: GetInstrumentationConfigurationStatus POST /get-instrumentation-configuration-status, output Status (@required, InstrumentationConfigurationStatus). Recorded behavior: for a configuration that has been hit, the wire value is `Active`. NOTE the conflict a fixer must resolve: the on-disk model's enum for InstrumentationConfigurationStatus lists READY / ERROR / ACTIVE / DISABLED, and the recorded wire value differs from that spelling in CASE only. The recording is the wire truth for what this response serializes; a fixer should check whether the enum's entries carry explicit `value` traits distinct from their names rather than assuming the upper-case name is the wire token.
Emulator: `ACTIVE`.
Why: a client comparing Status against the value AWS sends never matches, so a poller waiting for the active state never observes it.
Fix: the status projection the GetInstrumentationConfigurationStatus handler serializes (and the enum rendering it uses) — the status member, not the event list beside it, which passed.
Traces: application-signals-instrumentation-config-lifecycle#task#3.

--- Cause 6 of 7: BatchGetServiceLevelObjectiveBudgetReport error entries use the wrong ErrorCode and add a Name ---
AWS: Smithy: BatchGetServiceLevelObjectiveBudgetReport POST /budget-report; output Errors (@required) of ServiceLevelObjectiveBudgetReportError whose Name, Arn, ErrorCode and ErrorMessage are all modeled @required, with ErrorCode an unconstrained string (no enum). Recorded behavior: for an SloIds entry naming an SLO that does not exist, the per-item entry's `ErrorCode` is the string `400` and the entry carries NO `Name` member — the item is identified by its Arn alone.
Emulator: ErrorCode `ResourceNotFoundException`, plus a `Name` member carrying the SLO name parsed out of the requested id. (The surrounding 200 status, Timestamp and Reports all matched.)
Why: ErrorCode is what a client branches on to decide whether an item is a caller mistake or a service failure; an exception NAME where AWS sends an HTTP-status-like string means no code table matches, and the extra Name changes the entry's shape.
Fix: the per-item error construction in the BatchGetServiceLevelObjectiveBudgetReport handler (its ErrorCode token and its member set), not the shared SLO not-found helper. Adjacent OPEN issue 20261028T155204Z-application-signals-batchgetservicelevelobjectivebudgetreport-wrong-behavior.md concerns DEDUP of entries for one missing SLO and does not cover the code or the member set.
Traces: application-signals-slo-budget-report-batch#task#3.

--- Cause 7 of 7: GetService fabricates KeyAttributes and an empty MetricReferences ---
AWS: Smithy: GetService POST /service, output Service (@required) of shape Service, whose KeyAttributes (Attributes map, 1..4) and MetricReferences (list) are both modeled @required. Recorded behavior: for a discovered service read inside the requested window the recorded 200 body's `Service` object carries NEITHER member — the recorded body is narrower than the model here, and it is the wire truth for this read.
Emulator: emits `Service.KeyAttributes` echoing the request's key attributes (Type/Name/Environment) and `Service.MetricReferences` as an EMPTY list.
Why: an empty MetricReferences list is a positive statement that the service has no associated metrics, where AWS sends no member at all; a client that tests for the member's presence, or that treats an empty list as "metrics known to be none", draws the opposite conclusion from what AWS told it.
Fix: the Service projection GetService serializes (src/state.rs's Service projection and the GetService handler) — an absent member must be omitted rather than substituted with an empty list or an echo of the request. Do not satisfy this by stripping members on other reads: the divergence is in what this response serializes for a service with no metrics in the window.
Traces: application-signals-discovery-and-service-topology#task#3.

## Evidence
Wire replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 62 steps, 48 passed, 14 mismatched, score 77.4%, emulator_up=true, bootstrap_signal=false. All seven clusters are verdict=mismatch with empty stub_path and no owning issue.

Cause 1 — application-signals-slo-exclusion-windows#task#5 (BatchUpdateExclusionWindows), expected 200 / actual 400:
  status: expected 200, got 400
  $.Errors: missing (expected [], got None)
  $.SloIds: missing (expected ['arn:aws:application-signals:us-west-2:<account>:slo/<slo-name>'], got None)

Cause 2 — application-signals-slo-exclusion-windows#task#4 (ListServiceLevelObjectiveExclusionWindows), expected 200 / actual 200:
  $.ExclusionWindows[0]: expected {'Reason': 'scheduled maintenance', 'RecurrenceRule': {'Expression': None}, 'StartTime': <epoch seconds with milliseconds>, 'Window': {'Duration': 2, 'DurationUnit': 'HOUR'}}
                         got      {'Window': {'DurationUnit': 'HOUR', 'Duration': 2}, 'StartTime': <whole epoch seconds>, 'Reason': 'scheduled maintenance'}
  cluster also covers task#6. The two StartTime values differ because each side defaulted an omitted StartTime to its own call time — only the missing RecurrenceRule member and the loss of sub-second precision are claimed here.

Cause 3 — application-signals-instrumentation-config-errors#setup#1 (CreateInstrumentationConfiguration), expected 200 / actual 200:
  $.LocationHash: value_mismatch (expected 'fc14dd5a04207b41', got '52db650af7aae0bf')
  $.ARN: value_mismatch — same ARN but for its trailing /<16-hex> segment
  cluster also covers application-signals-instrumentation-config-lifecycle#setup#1. (The hex values are one run's; the defect is that the emulator's hash function is not the service's.)

Cause 4 — application-signals-instrumentation-config-errors#teardown#5 (DeleteInstrumentationConfiguration), expected 200 / actual 404:
  status: expected 200, got 404
  $.DeletionStatus: missing (expected 'DELETED', got None)

Cause 5 — application-signals-instrumentation-config-lifecycle#task#3 (GetInstrumentationConfigurationStatus), expected 200 / actual 200:
  $.Status: value_mismatch (expected 'Active', got 'ACTIVE')

Cause 6 — application-signals-slo-budget-report-batch#task#3 (BatchGetServiceLevelObjectiveBudgetReport), expected 200 / actual 200:
  $.Errors[0].ErrorCode: value_mismatch (expected '400', got 'ResourceNotFoundException')
  $.Errors[0].Name: type_mismatch (expected None, got '<slo-name>')

Cause 7 — application-signals-discovery-and-service-topology#task#3 (GetService), expected 200 / actual 200:
  $.Service.KeyAttributes: type_mismatch (expected None, got {'Environment': '<env>', 'Name': '<service-name>', 'Type': 'Service'})
  $.Service.MetricReferences: type_mismatch (expected None, got [])

Model citations (services/service-com-amazonaws-applicationsignals-applicationsignals/smithy-model/com-amazonaws-applicationsignals-applicationsignals.json):
  BatchUpdateExclusionWindows PATCH /exclusion-windows — SloIds @required 1..50; AddExclusionWindows/RemoveExclusionWindows optional max 10; output SloIds @required min 1 and Errors @required max 10; errors ResourceNotFoundException(404)/ThrottlingException(429)/ValidationException(400).
  ExclusionWindow — Window @required; StartTime optional ("Defaults to current time if not specified"); RecurrenceRule optional with @required Expression (1..1024); Reason 1..1024.
  CreateInstrumentationConfiguration — LocationHash @required length 16..16; ARN @required pattern ...instrumentationConfig/.+/[0-9a-f]{16}$.
  DeleteInstrumentationConfiguration — output DeletionStatus @required; success code 200.
  InstrumentationConfigurationStatus — enum READY/ERROR/ACTIVE/DISABLED (recorded wire value 'Active').
  ServiceLevelObjectiveBudgetReportError — Name/Arn/ErrorCode/ErrorMessage all @required; ErrorCode an unconstrained string.
  Service — KeyAttributes (1..4) and MetricReferences both @required (recorded body carries neither).
Code citations: src/state.rs `location_hash`/`stable_hash` (SHA-256 based 16-hex hash), `stored_exclusion_window` (DateTime::from_secs truncation), `update_exclusion_windows` (ExclusionWindowFailure::UnmatchedRemoval); src/ops/batch_update_exclusion_windows.rs (the all-ids-failed promotion of a per-item failure to a top-level exception).

## Steps to Reproduce
Boot the emulator and point an application-signals client at its data listener.

Cause 1: CreateServiceLevelObjective(Name=N, SliConfig=...); BatchUpdateExclusionWindows(SloIds=[<its ARN>], AddExclusionWindows=[{Window:{DurationUnit:'HOUR',Duration:2}, Reason:'scheduled maintenance'}]); then BatchUpdateExclusionWindows again on the same SloIds with a RemoveExclusionWindows entry that does not match the stored window member-for-member. The second call answers 400 ValidationException instead of 200 with SloIds=[<ARN>], Errors=[].

Cause 2: after the add above, ListServiceLevelObjectiveExclusionWindows(Id=<ARN>). The returned window carries no RecurrenceRule member and a whole-second StartTime.

Cause 3: CreateInstrumentationConfiguration(InstrumentationType=PROBE, SignalType=SNAPSHOT, Service=S, Environment=E, Location.CodeLocation=L, CaptureConfiguration=...) and compare LocationHash/ARN with the hash AWS mints for the same L (see the fixture's recorded value).

Cause 4: using the LocationHash the RECORDING carries for that location, call DeleteInstrumentationConfiguration(..., LocationIdentifier.LocationHash=<recorded hash>): 404 instead of 200 DeletionStatus='DELETED'.

Cause 5: create a PROBE/SNAPSHOT configuration, report a status event that makes it active, then GetInstrumentationConfigurationStatus for the same identity: Status is 'ACTIVE' instead of 'Active'.

Cause 6: BatchGetServiceLevelObjectiveBudgetReport(Timestamp=<now>, SloIds=[<the ARN of an SLO that does not exist>]). Errors[0].ErrorCode is 'ResourceNotFoundException' instead of '400', and Errors[0] carries a Name member.

Cause 7: GetService(StartTime, EndTime, KeyAttributes={Type:'Service',Name:S,Environment:E}) for a discovered service with no metrics in the window: Service carries KeyAttributes and MetricReferences=[] where the recorded body carries neither.

Or replay the fixture steps named under Traces for each cause.
