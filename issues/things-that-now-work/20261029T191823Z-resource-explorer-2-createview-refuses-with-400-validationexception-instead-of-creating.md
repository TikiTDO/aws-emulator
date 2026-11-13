Issue-Source: trace-sweep-divergence
Issue-Trace: resource-explorer-2-view-scoped-list-resources
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resource-explorer-2-view-scoped-list-resources
Issue-Shape-Id: com.amazonaws.resourceexplorer2#ResourceExplorer
Issue-Key: resource-explorer-2|CreateView|2xx->4xx|status
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# CreateView refuses with 400 ValidationException instead of creating the view

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateView

## Description
One root cause (Cause 1 of 1).

Cause 1 of 1: `CreateView` answers a 4xx client refusal on a request the recorded AWS wire accepted, so a view can never be created and every step that depends on one is unreachable.

AWS: Smithy model `com.amazonaws.resourceexplorer2#CreateView` — HTTP POST /CreateView, success status 200 (defaulted), input `CreateViewInput{ViewName: ViewName @required @pattern ^[a-zA-Z0-9\-]{1,64}$, ClientToken: String @length 1..2048 @idempotencyToken, IncludedProperties: IncludedPropertyList, Scope: String @length 1..2048, Filters: SearchFilter (sensitive), Tags: TagMap (sensitive)}`, output `CreateViewOutput{View: View}`. The only modeled refusals are `AccessDeniedException` (403), `ConflictException` (409), `InternalServerException` (500), `ServiceQuotaExceededException` (402), `ThrottlingException` (429), `UnauthorizedException` (401) and `ValidationException` (400) — and the model documents each: `ConflictException` for a duplicate `ViewName` in the Region or an `@idempotencyToken` replayed with different parameters, `ValidationException` only for "an invalid value for one of the operation's parameters". Recorded behavior only for this step: the fixture's recorded response for this CreateView call is HTTP 200 carrying the created `View`, so for a well-formed request naming a Region the account has turned Resource Explorer on in, 200 + `View` is the required outcome; no 400 is permitted for a request whose members satisfy the modeled constraints.

Emulator: the same request is answered HTTP 400 (`status: expected 200, got 400`), i.e. the operation's modeled `ValidationException` (the only 400 in its error set). The handler's sole hand-written 400 path is a precondition on service state rather than on any request member: `CreateView::handle` calls `guard.live_index(&region)` and, when the request's Region holds no index, returns `ValidationException` with the message `Resource Explorer is not turned on in Region {region}: create an index in this Region with CreateIndex before creating a view.` In that string the fixed text is emulator-invented — it is not AWS's wording for `ValidationException` ("You provided an invalid value for one of the operation's parameters. Check the syntax for the operation, and try again."), and the only request-derived variable part is the Region token. Hypothesis: this gate is what fires here — the recording's account had Resource Explorer already turned on in the Region (an account-level, out-of-band precondition the fixture never re-creates in its own steps), while the emulator's store starts with no index for that Region, so the first step of the fixture is refused. The emulator's framework-level member validation (ViewName pattern, ClientToken/Scope length) is the only other 400 source, and the recorded request satisfied those constraints on real AWS.

Why: a client that creates a view as the entry point to Resource Explorer (the documented way to scope `Search`/`ListResources`) gets a hard 400 instead of a `View`. Because `ValidationException` is the "your parameters are wrong" class, the SDK/caller blames its own input and retries differently or aborts, rather than discovering a missing-index precondition; and since no view can be created, view ARNs, the Region default view, view-scoped queries, `GetView`/`ListViews`/`UpdateView` and the rest of the fixture are all unreachable — every later step in the trace cascades off this one.

Fix: `CreateView`'s handler and its validation component — `CreateView::handle` in services/service-com-amazonaws-resourceexplorer2-resourceexplorer/src/ops/create_view.rs (the `live_index` precondition block) together with the Region/index lookup it uses in services/service-com-amazonaws-resourceexplorer2-resourceexplorer/src/state.rs (`live_index`). The operation must return 200 with the stored `View` for a request whose members satisfy the modeled constraints, and must not turn account-level setup state the corpus never creates into a client-side `ValidationException`: either treat the request's Region as turned on (materializing the Region's index in state on demand, the way a recorded account already has one) or keep the gate only where a recording shows AWS refusing, and reserve 400 `ValidationException` for actual member-level faults, 409 `ConflictException` for duplicate names/idempotency conflicts.

Traces: resource-explorer-2-view-scoped-list-resources#setup#1 (CreateView, expected 200, got 400). Note: the open issue "ListResources is a scaffold stub" (20261029T144855Z) covers the 501 on `ListResources` and does NOT cover this CreateView refusal; the replay report marked this cluster neither stub-blocked nor owned.

## Evidence
Replay cluster (fixture `resource-explorer-2-view-scoped-list-resources`, step `setup#1`, service `resource-explorer-2`, operation `CreateView`, verdict `mismatch`, divergence_key `service=resource-explorer-2|CreateView|status=200->400|keys=status`, affected_count 1):

Expected (recorded AWS / Smithy): HTTP 200 with `CreateViewOutput{View: {ViewArn, ViewName, Owner, Scope, Filters, IncludedProperties, LastUpdatedAt}}`. Modeled refusals for this operation are 403/409/500/402/429/401/400 only, and 400 `ValidationException` means "You provided an invalid value for one of the operation's parameters".

Actual (emulator): HTTP 400 — `detail: "status: expected 200, got 400"`. The operation's only modeled 400 is `ValidationException`, whose only hand-written source in this handler is the missing-index precondition:

  services/service-com-amazonaws-resourceexplorer2-resourceexplorer/src/ops/create_view.rs lines 197-209:
    `if guard.live_index(&region).is_none() { return Err(CreateViewErrorModeled::ValidationException(ValidationException { message: format!("Resource Explorer is not turned on in Region {region}: create an index in this Region with CreateIndex before creating a view."), field_list: None }).into()); }`
  services/service-com-amazonaws-resourceexplorer2-resourceexplorer/src/state.rs line 330: `pub fn live_index(&self, region: &str) -> Option<&IndexEntry>` — returns `None` whenever the Region has no stored index at all.

Message-part breakdown of the diverging error string: fixed text = `Resource Explorer is not turned on in Region ...: create an index in this Region with CreateIndex before creating a view.` (emulator-invented; not AWS's `ValidationException` wording); variable part = the Region token taken from the request's SigV4 credential scope. No per-run value (request id, timestamp, generated view id) is involved in the divergence — the compared key is `status` alone (200 -> 400).

Downstream effect visible in the same replay: the fixture's remaining steps never reach a successful view, and the run reports steps_passed=0 for this trace set.

## Steps to Reproduce
1. Boot the emulator from this checkout and point an SDK/botocore client at it with service name `resource-explorer-2`, signing for the fixture's Region.
2. Replay fixture `resource-explorer-2-view-scoped-list-resources` and stop at step `setup#1`: a `CreateView` call with a `ViewName` matching `^[a-zA-Z0-9\-]{1,64}$` (optionally `Filters.FilterString`, `IncludedProperties`, `ClientToken`, `Tags`).
   Observed: HTTP 400 (modeled `ValidationException`), message `Resource Explorer is not turned on in Region <region>: create an index in this Region with CreateIndex before creating a view.`
   Expected: HTTP 200 with `View` populated (ViewArn/ViewName/Owner/Scope/LastUpdatedAt), exactly as the recording shows.
3. Equivalent minimal repro against a freshly booted emulator (no prior CreateIndex): `aws resource-explorer-2 create-view --view-name wire-probe-1 --endpoint-url <emulator>` — returns 400 ValidationException instead of the created view.
4. Confirm the precondition is the gate: call `CreateIndex` in the same Region first, then repeat the same `CreateView` — it then returns 200, showing the refusal is driven by stored index state and not by any member of the request.
5. Re-replay fixture `resource-explorer-2-view-scoped-list-resources`, step `setup#1`, to confirm the fix (and that the fixture's later steps stop cascading off this refusal).
