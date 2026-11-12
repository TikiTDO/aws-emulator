Issue-Source: trace-sweep-divergence
Issue-Trace: cloudtrail-trail-lifecycle-error-paths
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudtrail-trail-lifecycle-error-paths cloudtrail-event-configuration-trail-and-parameter-errors cloudtrail-event-configuration-trail-not-found-error
Issue-Shape-Id: com.amazonaws.cloudtrail#CloudTrail_20131101
Issue-Key: cloudtrail|GetTrail|4xx->2xx|$.Message
Issue-Key: cloudtrail|GetTrail|4xx->2xx|$.__type
Issue-Key: cloudtrail|GetTrail|4xx->2xx|status
Issue-Key: cloudtrail|UpdateTrail|4xx->2xx|$.Message
Issue-Key: cloudtrail|UpdateTrail|4xx->2xx|$.__type
Issue-Key: cloudtrail|UpdateTrail|4xx->2xx|status
Issue-Key: cloudtrail|StartLogging|4xx->2xx|$
Issue-Key: cloudtrail|StartLogging|4xx->2xx|status
Issue-Key: cloudtrail|StopLogging|4xx->2xx|$
Issue-Key: cloudtrail|StopLogging|4xx->2xx|status
Issue-Key: cloudtrail|GetEventSelectors|4xx->2xx|$.Message
Issue-Key: cloudtrail|GetEventSelectors|4xx->2xx|$.__type
Issue-Key: cloudtrail|GetEventSelectors|4xx->2xx|status
Issue-Key: cloudtrail|PutEventSelectors|4xx->2xx|$.Message
Issue-Key: cloudtrail|PutEventSelectors|4xx->2xx|$.__type
Issue-Key: cloudtrail|PutEventSelectors|4xx->2xx|status
Issue-Key: cloudtrail|GetInsightSelectors|4xx->4xx|$.Message
Issue-Key: cloudtrail|GetInsightSelectors|4xx->4xx|$.__type
Issue-Key: cloudtrail|DeleteTrail|4xx->2xx|$
Issue-Key: cloudtrail|DeleteTrail|4xx->2xx|status
Issue-Key: cloudtrail|GetEventConfiguration|4xx->4xx|$.Message
Issue-Key: cloudtrail|PutEventConfiguration|4xx->4xx|$.Message
Issue-Commit: 07e872c7593558b582731e73c173338530c40226

# CloudTrail never rejects an unknown trail: lifecycle and selector operations return success, and TrailNotFoundException echoes a resolved ARN instead of the requested name

> _Note: this issue was identified against the code base at commit `07e872c7593558b582731e73c173338530c40226`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTrail

## Description
The emulator does not resolve the trail named by a request before acting on it. Every CloudTrail operation that takes a trail identifier is modelled with TrailNotFoundException, and AWS resolves the identifier FIRST: a trail that does not exist is refused before any read, mutation, logging-state change or selector access happens. In this emulator the trail-identifier lookup either silently succeeds against no trail at all (returning HTTP 200 and, for the mutating operations, reporting work that was never done) or resolves far enough to raise some LATER error, and in the two places where TrailNotFoundException IS raised the message is rendered from the emulator's resolved trail ARN instead of the identifier the caller supplied.

Two requirements hold across all ten causes.

(1) Wire shape of the refusal. The recorded responses refuse an unknown trail with HTTP 400 and a JSON body carrying __type 'TrailNotFoundException' and a 'Message' member. (Note for the fixer: the Smithy model gives TrailNotFoundException httpError 404 / awsQueryError code TrailNotFound, but the recorded wire responses for this JSON protocol are 400 — the recording governs, so emit 400.)

(2) Message template. The message is 'Unknown trail: <trail identifier> for the user: <account id>'. Its FIXED text is 'Unknown trail: ' and ' for the user: '. Its variable parts are (a) the trail identifier ECHOED EXACTLY AS THE REQUEST SUPPLIED IT — a bare trail name when the caller passed a bare name, the ARN when the caller passed an ARN; never the emulator's own expansion of one into the other — and (b) the account id of the calling principal, taken from emulator state. The recorded fixtures exercise both forms against the same trail, so a handler that always renders the ARN (or always the bare name) fails half of them.

There are 10 distinct causes in this issue; all 10 must be closed.

Cause 1 of 10 — GetTrail on an unknown trail returns 200.
AWS: Smithy binds GetTrail (input Name, required) error TrailNotFoundException (errorKind client, awsQueryError code TrailNotFound). AWS resolves Name and refuses an absent trail. Recorded response: 400 / __type TrailNotFoundException / Message per the template above, with the identifier in the bare-name form the request used.
Emulator: HTTP 200 with no __type and no Message at all.
Why: a client cannot tell a missing trail from an existing one; the TrailNotFound branch of its error handling is dead code, and a caller that treats 200 as proof of existence proceeds against a trail that is not there.
Fix: GetTrail's handler must resolve Name in state and raise TrailNotFoundException before building a response.
Traces: cloudtrail-trail-lifecycle-error-paths#task#2

Cause 2 of 10 — UpdateTrail on an unknown trail returns 200 and reports a successful update.
AWS: Smithy binds UpdateTrail (Name required) error TrailNotFoundException. Recorded response: 400 / TrailNotFoundException / the template message, identifier in the ARN form the request supplied.
Emulator: HTTP 200, no __type, no Message.
Why: a mutation against a nonexistent trail is reported as applied. Configuration-management clients conclude the trail now has the settings they sent, when no trail exists to hold them.
Fix: UpdateTrail's handler must resolve Name and raise TrailNotFoundException before applying or persisting any member.
Traces: cloudtrail-trail-lifecycle-error-paths#task#3

Cause 3 of 10 — StartLogging on an unknown trail returns 200 with an empty body.
AWS: Smithy binds StartLogging (Name required) error TrailNotFoundException. StartLoggingResponse is empty on success, so the ONLY signal available to a caller is the status and error body. Recorded response: 400 / TrailNotFoundException / the template message, ARN form as supplied.
Emulator: HTTP 200 with body {} — indistinguishable from a real successful start.
Why: this is the worst shape of the defect: because success is an empty body, a client that enabled logging on a misspelled or deleted trail receives exactly the success it would get for a real one and believes auditing is on.
Fix: StartLogging's handler must resolve Name and raise TrailNotFoundException before returning its empty success body.
Traces: cloudtrail-trail-lifecycle-error-paths#task#4

Cause 4 of 10 — StopLogging on an unknown trail returns 200 with an empty body.
AWS: Smithy binds StopLogging (Name required) error TrailNotFoundException; StopLoggingResponse is likewise empty. Recorded response: 400 / TrailNotFoundException / the template message, ARN form as supplied.
Emulator: HTTP 200 with body {}.
Why: same empty-body confusion as Cause 3, in the direction that matters for incident response — a caller told it stopped logging on a trail that does not exist.
Fix: StopLogging's handler must resolve Name and raise TrailNotFoundException before returning success.
Traces: cloudtrail-trail-lifecycle-error-paths#task#5

Cause 5 of 10 — GetEventSelectors on an unknown trail returns 200.
AWS: Smithy binds GetEventSelectors (TrailName required) error TrailNotFoundException. Recorded response: 400 / TrailNotFoundException / the template message, ARN form as supplied.
Emulator: HTTP 200, no __type, no Message.
Why: a caller reading the selectors of a trail that does not exist gets a selector view instead of a refusal, so it cannot distinguish 'trail absent' from 'trail present with default selectors'.
Fix: GetEventSelectors' handler must resolve TrailName and raise TrailNotFoundException first.
Traces: cloudtrail-trail-lifecycle-error-paths#task#6

Cause 6 of 10 — PutEventSelectors on an unknown trail returns 200.
AWS: Smithy binds PutEventSelectors (TrailName required) error TrailNotFoundException. Recorded response: 400 / TrailNotFoundException / the template message, ARN form as supplied.
Emulator: HTTP 200, no __type, no Message.
Why: selectors are reported written to a trail that does not exist; data-event coverage is believed configured when nothing holds the configuration.
Fix: PutEventSelectors' handler must resolve TrailName and raise TrailNotFoundException before validating or persisting selectors.
Traces: cloudtrail-trail-lifecycle-error-paths#task#7

Cause 7 of 10 — GetInsightSelectors on an unknown trail raises InsightNotEnabledException instead of TrailNotFoundException.
AWS: Smithy binds GetInsightSelectors both TrailNotFoundException and InsightNotEnabledException (client, 400, awsQueryError InsightNotEnabled). Trail existence is resolved BEFORE the Insights-enabled state is consulted, so an absent trail yields TrailNotFoundException, not InsightNotEnabledException. Recorded response: 400 / __type TrailNotFoundException / Message with the identifier in the BARE-NAME form the request supplied.
Emulator: 400 (status correct) but __type 'InsightNotEnabledException' and the message 'Trail <resolved ARN> does not have Insights enabled. Edit the trail settings to enable Insights, and then try the operation again.' — the handler skipped existence resolution and reported on the Insights flag of a trail it never found.
Why: the error code is the client's only machine-readable discriminator. A caller that handles InsightNotEnabled by enabling Insights will retry forever against a trail that does not exist, and a caller that handles TrailNotFound by creating the trail never runs.
Fix: GetInsightSelectors' handler must order its checks so the trail identifier is resolved (raising TrailNotFoundException) before the Insights-enabled state is evaluated.
Traces: cloudtrail-trail-lifecycle-error-paths#task#8

Cause 8 of 10 — DeleteTrail on an unknown trail returns 200 with an empty body.
AWS: Smithy binds DeleteTrail (Name required) error TrailNotFoundException; DeleteTrailResponse is empty. Recorded response: 400 / TrailNotFoundException / the template message, ARN form as supplied.
Emulator: HTTP 200 with body {}.
Why: delete is reported as having removed a trail that never existed, so cleanup and teardown logic cannot detect that it targeted the wrong identifier, and an idempotency test that distinguishes first-delete from second-delete cannot be written against the emulator.
Fix: DeleteTrail's handler must resolve Name and raise TrailNotFoundException before returning its empty success body.
Traces: cloudtrail-trail-lifecycle-error-paths#task#9

Cause 9 of 10 — GetEventConfiguration's TrailNotFoundException message renders the emulator's resolved ARN instead of the bare trail name the request supplied.
AWS: Smithy binds GetEventConfiguration (TrailName) error TrailNotFoundException. The message echoes the identifier AS SUPPLIED; the request here passed a bare trail name, so the message carries the bare name.
Emulator: status 400 and __type TrailNotFoundException are both correct, but Message interpolates the trail ARN the emulator synthesized from the name ('arn:aws:cloudtrail:<region>:<account>:trail/<name>') in the slot where AWS puts the caller's own bare name.
Why: the message is a requirement, not decoration: callers log it, assert on it and surface it to operators. An ARN appearing where the submitted name belongs also misleads an operator into thinking the request used an ARN, and defeats any correlation between the refusal and the identifier the client actually sent.
Fix: the shared trail-not-found error constructor must take the identifier slot from the request's TrailName/Name as received, not from the resolved/normalized ARN form.
Traces: cloudtrail-event-configuration-trail-and-parameter-errors#task#1

Cause 10 of 10 — PutEventConfiguration's TrailNotFoundException message renders the resolved ARN instead of the supplied bare trail name.
AWS: Smithy binds PutEventConfiguration (TrailName) error TrailNotFoundException; same echo-as-supplied rule as Cause 9.
Emulator: 400 / TrailNotFoundException correct, Message again carries the synthesized trail ARN in the identifier slot instead of the bare name the request supplied.
Why: as Cause 9 — and because the same wrong rendering appears on both the read and the write path, it confirms the defect is in the shared message builder rather than in one handler.
Fix: same shared trail-not-found error constructor; fix the identifier slot once and both paths follow.
Traces: cloudtrail-event-configuration-trail-and-parameter-errors#task#3

Relation to existing open issues: no open issue carries the replay keys for these steps (the replay report lists no owner for any of the ten clusters). The closest open items are 20261023T135552Z-cloudtrail-has-no-updatetrail-concurrency-control-cloudformation-never.md (UpdateTrail concurrency/authorization, not trail resolution) and 20261024T114720Z-cloudtrail-control-plane-fidelity-gettrailstatus-omits-its-deprecated.md (GetTrailStatus members, ListTags/LookupEvents limits); neither covers the missing trail-existence resolution or the identifier slot of the TrailNotFoundException message.

## Evidence
Wire replay of simparity-wire-aws-v1 (emulator up; /__admin/health 200). Ten mismatch clusters, all service=cloudtrail. Expected values are the recorded AWS responses; actual values are the emulator's.

Missing refusal entirely — expected 400, got 200:
- GetTrail (cloudtrail-trail-lifecycle-error-paths#task#2): expected __type 'TrailNotFoundException' + Message 'Unknown trail: <bare-name> for the user: <account>'; got status 200, __type missing, Message missing.
- UpdateTrail (#task#3): expected __type 'TrailNotFoundException' + Message 'Unknown trail: <trail-arn> for the user: <account>'; got 200, both members missing.
- GetEventSelectors (#task#6): expected __type 'TrailNotFoundException' + the ARN-form message; got 200, both members missing.
- PutEventSelectors (#task#7): expected __type 'TrailNotFoundException' + the ARN-form message; got 200, both members missing.
- StartLogging (#task#4): expected body {'__type': 'TrailNotFoundException', 'Message': 'Unknown trail: <trail-arn> for the user: <account>'}; got 200 with empty_body {}.
- StopLogging (#task#5): same expectation; got 200 with empty_body {}.
- DeleteTrail (#task#9): same expectation; got 200 with empty_body {}.

Wrong error code — status matched, code did not:
- GetInsightSelectors (#task#8): $.__type value_mismatch, expected 'TrailNotFoundException', got 'InsightNotEnabledException'; $.Message value_mismatch, expected 'Unknown trail: <bare-name> for the user: <account>', got 'Trail <trail-arn> does not have Insights enabled. Edit the trail settings to enable Insights, and then try the operation again.'

Wrong message identifier slot — status and code matched, message did not:
- GetEventConfiguration (cloudtrail-event-configuration-trail-and-parameter-errors#task#1, also cloudtrail-event-configuration-trail-not-found-error#task#1): $.Message value_mismatch, expected 'Unknown trail: <bare-name> for the user: <account>', got 'Unknown trail: arn:aws:cloudtrail:<region>:<account>:trail/<bare-name> for the user: <account>'.
- PutEventConfiguration (cloudtrail-event-configuration-trail-and-parameter-errors#task#3, also cloudtrail-event-configuration-trail-not-found-error#task#2): identical divergence on the same slot.

The concrete trail names, region, and the account id 000000000000 in the recorded strings are per-environment values filling the template; the requirement is the status 400, the __type TrailNotFoundException, the fixed text 'Unknown trail: ' / ' for the user: ', and that the identifier slot echo the request's own trail identifier rather than a resolved ARN.

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus simparity-wire-aws-v1.
2. Replay fixture cloudtrail-trail-lifecycle-error-paths, steps task#2 through task#9, and fixture cloudtrail-event-configuration-trail-and-parameter-errors, steps task#1 and task#3 (plus cloudtrail-event-configuration-trail-not-found-error, task#1 and task#2, in the same clusters).
3. Manually, against a trail name that was never created (call it GHOST):
   a. cloudtrail GetTrail(Name=GHOST) -> expect 400 TrailNotFoundException with Message 'Unknown trail: GHOST for the user: <account>'; observe 200.
   b. UpdateTrail(Name=<ARN of GHOST>) -> expect 400 TrailNotFoundException whose message echoes the ARN exactly as passed; observe 200.
   c. StartLogging(Name=<ARN of GHOST>) and StopLogging(Name=<ARN of GHOST>) -> expect 400 TrailNotFoundException; observe 200 with an empty body.
   d. GetEventSelectors(TrailName=<ARN of GHOST>) and PutEventSelectors(TrailName=<ARN of GHOST>, EventSelectors=[...]) -> expect 400 TrailNotFoundException; observe 200.
   e. GetInsightSelectors(TrailName=GHOST) -> expect 400 TrailNotFoundException; observe 400 InsightNotEnabledException with the 'does not have Insights enabled' message.
   f. DeleteTrail(Name=<ARN of GHOST>) -> expect 400 TrailNotFoundException; observe 200 with an empty body.
   g. GetEventConfiguration(TrailName=GHOST) and PutEventConfiguration(TrailName=GHOST, ...) -> expect the message to echo the bare name GHOST; observe the synthesized trail ARN in that slot.
4. Repeat (a) passing the ARN form and (b) passing the bare-name form to confirm the identifier slot echoes whichever form was supplied.
