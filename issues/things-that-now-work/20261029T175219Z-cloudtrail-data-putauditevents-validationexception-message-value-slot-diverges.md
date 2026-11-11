Issue-Source: trace-sweep-divergence
Issue-Trace: cloudtraildata-audit-event-batch-size-ceiling
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudtraildata-audit-event-batch-size-ceiling cloudtraildata-audit-event-constraint-rejections cloudtraildata-audit-events-batch-exceeds-max-size-rejected
Issue-Shape-Id: com.amazonaws.cloudtraildata#CloudTrailDataService
Issue-Key: cloudtrail-data|PutAuditEvents|4xx->4xx|$.message
Issue-Commit: 646959a788c6809f2c0136ca28665b584f5bac57

# cloudtrail-data PutAuditEvents ValidationException message value slot diverges from AWS's constraint-violation template

> _Note: this issue was identified against the code base at commit `646959a788c6809f2c0136ca28665b584f5bac57`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutAuditEvents

## Description
One net-new WIRE root cause, in the shared constraint-violation message serializer as reached by PutAuditEvents request validation. The wire replay clusters 5 failing steps across 3 fixtures under a single divergence key (service=cloudtrail-data|PutAuditEvents|status=400->400|keys=$.message): the HTTP status (400), the error code and every other response key match, and the ONLY diverging key is $.message.

=== Cause 1 of 1: the constraint-violation message does not render the rejected value the way AWS does, so $.message diverges for every modelled-constraint refusal of PutAuditEvents ===

AWS: PutAuditEvents' auditEvents member is a list with a modelled length ceiling, and its elements carry modelled constraints of their own. Smithy (services/service-com-amazonaws-cloudtraildata-cloudtraildataservice/smithy-model/com-amazonaws-cloudtraildata-cloudtraildataservice.json): PutAuditEventsRequest$auditEvents -> AuditEvents, required, list, @length {min 1, max 100}, member AuditEvent; AuditEvent$id -> Uuid, required, @length 1..128, @pattern "^[-_A-Za-z0-9]+$"; AuditEvent$eventData required String; AuditEvent$eventDataChecksum optional String; PutAuditEventsRequest$channelArn @pattern "^arn:.*$" (httpQuery channelArn); PutAuditEventsRequest$externalId @length 2..1224 @pattern "^[\\w+=,.@:\\/-]*$". ValidationException is NOT among the operation's modelled errors (ChannelInsufficientPermission, ChannelNotFound, ChannelUnsupportedSchema, DuplicatedAuditEventId, InvalidChannelARN, UnsupportedOperationException), so a modelled-constraint refusal is produced by the shared constraint layer ahead of the handler. Violating a constraint is refused with HTTP 400 and error code ValidationException, whose message follows the standard constraint-violation template. Its FIXED text is "<N> validation error(s) detected: Value '<value>' at '<member path>' failed to satisfy constraint: " followed by the per-constraint sentence — "Member must have length less than or equal to <max>" for a @length ceiling, "Member must satisfy regular expression pattern: <pattern>" for a @pattern. Its VARIABLE parts all come from the request and the model: the violation count, the member path (the 1-based element path 'auditEvents.<n>.member.id' for an element-level violation, the plain member name 'auditEvents' for the list-level one), the modelled limit or pattern, and the rejected VALUE taken from the request. Recorded behavior only for how a LIST value is rendered into the '<value>' slot: the recorded response echoes the whole rejected list in Java toString form — "[" + a comma-joined "AuditEvent(id=<id>, eventData=<eventData>, eventDataChecksum=<checksum-or-null>)" per element, one per event actually sent, in request order, with id and eventData verbatim from the request and eventDataChecksum rendered as 'null' when the caller omitted that member + "]".

Emulator: answers the same status (400) and the same error code, with the same member path and the same modelled limit, and then fills the '<value>' slot with something the caller never sent. For the list-level @length violation it substitutes a synthesized cardinality phrase of the form '<n> items' (e.g. "Value '101 items' at 'auditEvents' ...") in place of the rendered list; the replay's expected payload for the same step carries the full "[AuditEvent(id=..., eventData={...}, eventDataChecksum=null), AuditEvent(...), ...]" rendering. The replay reports the divergence as "$.message: value_mismatch" on every one of the 5 clustered steps, which span the batch-size ceiling fixtures and the element-level constraint-rejection fixture — i.e. the defect is in the shared slot-formatting code, not in any one operation-level check.

Why: an HTTP status, an error code and an error message are all requirements, and here only the message is wrong — which is the worst case for a client, because the refusal looks structurally correct. AWS's message is self-describing: it names exactly which audit events were in the rejected batch, so a caller can locate the offending records and resplit the batch, and a recorder or conformance test can derive the expected text from its own request. The emulator discards that and reports only a count, so any consumer parsing the value slot out of a ValidationException sees a phrase shape ('<n> items') AWS never produces for a list member, and no client assertion derived from the request can be satisfied.

Fix: the shared constraint-violation message serializer reached by this operation's request validation — specifically the branch that formats the '<value>' slot. The LIST path must render the collection's elements (element-per-entry, request order, element toString form with eventDataChecksum printed as 'null' only when omitted) instead of a count; the scalar path, which already renders a value verbatim, must not change. The constraint ENFORCEMENT itself (the inclusive 100 ceiling, @length/@pattern on AuditEvent$id), the violation count, the member paths, the modelled limit, the ARN-syntax-before-constraint-before-channel-resolution ordering, the status and the error code are all already correct and must not regress.

Note on existing issues: no OPEN issue carries this wire replay key (the replay reports owned_by empty for the cluster), so this cause is unbound by any existing pointer. Two open issues describe the same serializer from the BEHAVIORAL corpus and do not cover these fixture steps: 20261027T001848Z-cloudtrail-data-putauditevents-validationexception-for-an-over-length.md (the same list-value slot, grounded in behavioral check ids only) and 20261028T004339Z-cloudtrail-data-putauditevents-validationexception-renders-the-modelled-pattern.md (the '<pattern>' slot of the same template, where the modelled @pattern must be rendered with its leading '^' and trailing '$' stripped). 20261022T215433Z-cloudtrail-data-putauditevents-rejection-fidelity-channel-arn-syntax.md covers channel-ARN syntax and cross-account existence leakage, not the value slot. A fixer holding any of those should treat this as the same work item; it is filed here so the wire steps are not closed out unrun.

Traces: cloudtraildata-audit-event-batch-size-ceiling#task#2 (representative), cloudtraildata-audit-event-batch-size-ceiling#task#5, cloudtraildata-audit-event-constraint-rejections#task#1, cloudtraildata-audit-event-constraint-rejections#task#3, cloudtraildata-audit-events-batch-exceeds-max-size-rejected#task#1.

## Evidence
Wire replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, /__admin/health 200; 14 steps replayed, 9 passed, 5 mismatched, score 64.3%). The whole corpus produced exactly ONE divergence cluster:

  divergence_key: service=cloudtrail-data|PutAuditEvents|status=400->400|keys=$.message
  verdict:        mismatch
  expected_status 400, actual_status 400  (status and error code MATCH)
  diverging key:  $.message only
  affected (5):   cloudtraildata-audit-event-batch-size-ceiling#task#2,
                  cloudtraildata-audit-event-batch-size-ceiling#task#5,
                  cloudtraildata-audit-event-constraint-rejections#task#1,
                  cloudtraildata-audit-event-constraint-rejections#task#3,
                  cloudtraildata-audit-events-batch-exceeds-max-size-rejected#task#1
  stub_path:      (empty — not blocked by an unimplemented operation)
  owned_by:       (empty — no open issue carries this replay key)

Replay detail, verbatim: "$.message: value_mismatch (expected '1 validation error detected: Value '[AuditEvent(id=sim2real-over-limit-001, eventData={\"version\": \"1.0\", \"userIdentity\": {\"type\": \"CustomIdentity\", \"principalId\": \"sim2real-behav\"}, \"eventSource\": \"sim2real.behavioral\", \"eventName\": \"Sim2RealProbeEvent\", \"eventTime\": \"2026-01-01T00:00:00Z\", \"UID\": \"sim2real-over-limit-uid-001\"}, eventDataChecksum=null), AuditEvent(id=sim2real-over-limit-002, ...), ... AuditEvent(" — the expected value slot continues element by element, one AuditEvent(...) per event sent, in request order.

Expected (AWS, recorded):  HTTP 400, ValidationException, Message
  "1 validation error detected: Value '[AuditEvent(id=<id-1>, eventData=<eventData-1>, eventDataChecksum=null), ... , AuditEvent(id=<id-N>, eventData=<eventData-N>, eventDataChecksum=null)]' at 'auditEvents' failed to satisfy constraint: Member must have length less than or equal to 100"
Actual (emulator):         HTTP 400, ValidationException, Message with the same fixed text but the value slot replaced by a synthesized count summary of the form
  "1 validation error detected: Value '<n> items' at 'auditEvents' failed to satisfy constraint: Member must have length less than or equal to 100"
(the '<n> items' rendering is the emulator's own phrase; it is recorded verbatim as "Value '101 items'" in the behavioral evidence of open issue 20261027T001848Z for the identical serializer branch.)

Which parts of the message are requirements:
  fixed text  -> "<N> validation error detected: Value '" ... "' at '" ... "' failed to satisfy constraint: Member must have length less than or equal to " ... (plural "validation errors detected" when N > 1); for an element-level @pattern violation the trailing sentence is "Member must satisfy regular expression pattern: <pattern>".
  from request -> the violation count, the member path ('auditEvents' for the list-level violation, 1-based 'auditEvents.<n>.member.id' for an element-level one), and the rejected VALUE: every element's id and eventData exactly as sent, in request order, with eventDataChecksum printed as 'null' only where the caller omitted it.
  from model   -> the limit (100) / the pattern.
Nothing per-run is part of this requirement: the ids, eventData JSON, UIDs, timestamps and batch size in the recorded payload are that run's fill-ins, not fixed text, and no request id or clock value participates.

Matched on the same steps and must not regress: HTTP 400; the ValidationException error code; the member path; the violation count; the modelled limit in the fixed text; and the positive controls in the same fixtures where a 100-event batch clears constraint validation and is refused by channel resolution instead.

Model evidence (smithy_query, PutAuditEvents): input auditEvents -> AuditEvents, required, list, @length {min 1, max 100}; channelArn required @pattern ^arn:.*$ httpQuery; externalId @length 2..1224 @pattern ^[\w+=,.@:\/-]*$; errors ChannelInsufficientPermission, ChannelNotFound, ChannelUnsupportedSchema, DuplicatedAuditEventId, InvalidChannelARN, UnsupportedOperationException (all client/400) — ValidationException is NOT modelled on the operation, confirming the message comes from the shared constraint layer rather than the handler's own error construction.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from this checkout (the wire replay confirms /__admin/health answers 200 and the data listener accepts).
2. Point a botocore cloudtrail-data client at it in us-west-2, built with Config(parameter_validation=False) so the client-side @length/@pattern checks do not pre-empt the service.
3. Resolve the emulator's own account: account = sts.get_caller_identity()['Account'], and build a well-formed same-account channel ARN that was never created: absent = f"arn:aws:cloudtrail:us-west-2:{account}:channel/{uuid4()}".
4. Positive control (must keep passing): put_audit_events(channelArn=absent, auditEvents=[{'id': <uuid-hex>, 'eventData': <json string>} for exactly 100 events]) -> ClientError with Error.Code 'ChannelNotFound' and Error.Message f'The channel {absent} could not be found.' This proves the over-limit call below is refused by the constraint layer and not by an off-by-one ceiling.
5. Repeat with 101 events. Expect HTTP 400, Error.Code 'ValidationException', and Error.Message "1 validation error detected: Value '<the rejected list rendered as [AuditEvent(id=..., eventData=..., eventDataChecksum=null), ...] for every event in request order>' at 'auditEvents' failed to satisfy constraint: Member must have length less than or equal to 100".
6. Observe instead a message whose value slot is a synthesized count summary ('<n> items'): correct status, code, member path, count and limit, with the rejected list value never rendered.
7. Generalise the fix past one batch size: repeat with 150 events, and with 101 events where some elements carry an explicit eventDataChecksum, and confirm every element is rendered in request order with the supplied checksum printed (and 'null' only where the member was omitted) rather than any cardinality phrase.
8. Exercise the element-level branch of the same serializer: send one element with an id of 129 characters (@length 1..128) and one with id 'sim2real bad id!' (@pattern ^[-_A-Za-z0-9]+$) and confirm the value slot carries the rejected scalar verbatim at the 1-based path 'auditEvents.1.member.id'.
9. Re-replay the wire corpus and confirm the cluster service=cloudtrail-data|PutAuditEvents|status=400->400|keys=$.message is gone, i.e. all of cloudtraildata-audit-event-batch-size-ceiling#task#2, cloudtraildata-audit-event-batch-size-ceiling#task#5, cloudtraildata-audit-event-constraint-rejections#task#1, cloudtraildata-audit-event-constraint-rejections#task#3 and cloudtraildata-audit-events-batch-exceeds-max-size-rejected#task#1 pass.
