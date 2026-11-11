Issue-Source: trace-sweep-divergence
Issue-Trace: iot-topic-rule-destination-confirm-invalid-token-error
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-topic-rule-destination-confirm-invalid-token-error iot-topic-rule-destination-errors iot-topic-rule-destination-invalid-url-error iot-topic-rule-destination-get-nonexistent iot-topic-rule-destination-get-nonexistent-error iot-topic-rule-destination-http-lifecycle
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|ConfirmTopicRuleDestination|4xx->4xx|$.message
Issue-Key: iot|CreateTopicRuleDestination|4xx->4xx|$.message
Issue-Key: iot|GetTopicRuleDestination|4xx->4xx|$.message
Issue-Key: iot|GetTopicRuleDestination|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: iot|GetTopicRuleDestination|4xx->4xx|status
Issue-Key: iot|CreateTopicRuleDestination|2xx->2xx|$.topicRuleDestination.statusReason
Issue-Key: iot|GetTopicRuleDestination|2xx->2xx|$.topicRuleDestination.statusReason
Issue-Key: iot|GetTopicRuleDestination|4xx->2xx|$.message
Issue-Key: iot|GetTopicRuleDestination|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot|GetTopicRuleDestination|4xx->2xx|status
Issue-Commit: c4f88ade9ad2196ad9bb27ac1fdf21ca59c18d70

# iot: TopicRuleDestination plane diverges on 7 causes — error class, error wording, statusReason and post-delete readability

> _Note: this issue was identified against the code base at commit `c4f88ade9ad2196ad9bb27ac1fdf21ca59c18d70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTopicRuleDestination

## Description
The IoT topic-rule-destination plane (CreateTopicRuleDestination / GetTopicRuleDestination / ConfirmTopicRuleDestination / DeleteTopicRuleDestination) diverges from AWS in seven distinct ways: it invents its own error messages instead of AWS's, it picks the wrong modeled error shape (and therefore the wrong HTTP status) for a destination the caller cannot read, it never populates the modeled TopicRuleDestination.statusReason member, and it keeps serving a destination after DeleteTopicRuleDestination has removed it. This bundle carries 7 CAUSES; all seven must be fixed and re-replayed before it is closed.

Cause 1 of 7: ConfirmTopicRuleDestination rejects a bad token with the wrong message text.
  AWS: Smithy: ConfirmTopicRuleDestination input confirmationToken @httpLabel, required, @length min 1 max 2048; modeled error InvalidRequestException (errorKind client, httpError 400, doc "The request is not valid."). The recorded response is 400 with body member `message` = "Invalid confirmation token." — the whole string is fixed text for this error class, terminated by a period; there is no variable part (the token itself is not echoed).
  Emulator: returns 400 with `message` = "Invalid confirmation token" — same text with the trailing period dropped.
  Why: the error message is part of the contract. A client (or a test harness) matching AWS's message exactly, logging it, or asserting on it, sees a string AWS never emits, so message-based branching and golden-output comparisons fail against the emulator while passing against AWS.
  Fix: the ConfirmTopicRuleDestination handler's validation path, where the InvalidRequestException message for an unrecognised confirmation token is constructed.
  Traces: iot-topic-rule-destination-confirm-invalid-token-error#task#1 (also iot-topic-rule-destination-errors#task#4).

Cause 2 of 7: CreateTopicRuleDestination rejects a non-HTTPS confirmationUrl with the wrong message text.
  AWS: Smithy: CreateTopicRuleDestination POST /destinations, input destinationConfiguration (TopicRuleDestinationConfiguration, required); modeled error InvalidRequestException (client, httpError 400). The recorded response is 400 with `message` of the form "Invalid confirmationUrl: '<confirmationUrl>' is not a valid HTTPS URL" — fixed text "Invalid confirmationUrl: " and " is not a valid HTTPS URL", with the single-quoted URL taken verbatim from the request's httpUrlConfiguration.confirmationUrl.
  Emulator: returns 400 but with an entirely different, self-authored sentence of the form "confirmationUrl <url> is not a URL this destination's confirmation request can be sent to: its scheme is not https".
  Why: same class of breakage as Cause 1, but worse — the emulator's wording shares no prefix with AWS's, so no client that recognises AWS's validation message recognises the emulator's, and the URL is not quoted the way AWS quotes it.
  Fix: CreateTopicRuleDestination's input validation, where the HTTPS-scheme check builds its InvalidRequestException message.
  Traces: iot-topic-rule-destination-errors#task#1 (also iot-topic-rule-destination-invalid-url-error#task#1).

Cause 3 of 7: GetTopicRuleDestination does not validate the ARN syntax of its path label, so a non-ARN value is reported as a missing destination instead of a malformed ARN.
  AWS: Smithy: GetTopicRuleDestination GET /destinations/{arn+}, input member arn target AwsArn, @httpLabel, required; modeled error InvalidRequestException (client, httpError 400). For a value that is not an ARN at all the recorded response is 400 with `message` of the form "ARNs must start with 'arn:': <value>" — fixed text "ARNs must start with 'arn:': " followed by the offending arn value copied from the request path.
  Emulator: returns 400 with `message` of the form "Topic rule destination <value> not found" — i.e. it skips ARN-syntax validation entirely and goes straight to a lookup miss.
  Why: the caller cannot tell a client-side ARN formatting bug from a destination that genuinely does not exist. Code that retries or re-resolves on "not found" will retry forever on a request it should have fixed.
  Fix: GetTopicRuleDestination's input validation — reject a syntactically invalid AwsArn label before the state lookup runs.
  Traces: iot-topic-rule-destination-errors#task#2 (also iot-topic-rule-destination-get-nonexistent#task#1).

Cause 4 of 7: GetTopicRuleDestination on a well-formed ARN the account cannot read answers InvalidRequestException/400 instead of UnauthorizedException/401.
  AWS: Smithy: GetTopicRuleDestination errors include UnauthorizedException (errorKind client, httpError 401, doc "You are not authorized to perform this operation."). The recorded response is HTTP 401, header x-amzn-errortype = "UnauthorizedException:http://iot.amazonaws.com/doc/2015-10-02/", body `message` of the form "Access to TopicRuleDestination '<arn>' was denied" — fixed text "Access to TopicRuleDestination '" and "' was denied", with the single-quoted destination ARN taken from the request path. AWS does not disclose non-existence here; it denies access.
  Emulator: returns HTTP 400 with x-amzn-errortype = "InvalidRequestException" and `message` of the form "Topic rule destination <arn> not found".
  Why: three requirements are wrong at once — status (401 vs 400), error code (UnauthorizedException vs InvalidRequestException), and message. SDK error handling keys off the error code, so a client that catches UnauthorizedException never catches this, while the 4xx-but-not-401 status makes credential/permission diagnostics report a malformed request instead of an authorization failure. The emulator also leaks existence information AWS deliberately withholds. Note the x-amzn-errortype value AWS sends carries the shape's namespace suffix, which the emulator omits even for the code it does send.
  Fix: GetTopicRuleDestination's handler and error serializer — map an unreadable/absent destination ARN to UnauthorizedException (401) and emit the fully-qualified x-amzn-errortype.
  Traces: iot-topic-rule-destination-get-nonexistent-error#task#1.

Cause 5 of 7: CreateTopicRuleDestination omits statusReason from the destination it returns.
  AWS: Smithy: CreateTopicRuleDestination output topicRuleDestination (TopicRuleDestination); that shape's member statusReason (target String) is documented "Additional details or reason why the topic rule destination is in the current status." A freshly created HTTP destination is in status IN_PROGRESS — "created but has not been confirmed" per the TopicRuleDestinationStatus docs — and the recorded response is 200 with statusReason = "Awaiting confirmation. The confirmation message has not been sent to the destination endpoint yet." That sentence is fixed text for the newly-created, unconfirmed state; it has no variable part.
  Emulator: returns 200 with statusReason absent (None) on the created topicRuleDestination.
  Why: statusReason is how a client explains an IN_PROGRESS or ERROR destination to an operator or decides whether confirmation is still pending. With the member always missing, any consumer reading it gets null and reports no reason at all for a destination that is not yet usable.
  Fix: the CreateTopicRuleDestination handler/state — set statusReason alongside status when the destination record is created, and the output serializer must emit it.
  Traces: iot-topic-rule-destination-http-lifecycle#task#1.

Cause 6 of 7: GetTopicRuleDestination omits statusReason as well.
  AWS: Smithy: GetTopicRuleDestination output topicRuleDestination (TopicRuleDestination) with the same statusReason member; the TopicRuleDestinationStatus docs state that for an ERROR destination "You can call GetTopicRuleDestination for details about the error", i.e. the reason is expected to be readable back. The recorded 200 response carries a non-empty statusReason (recorded behavior only: the fixture asserts its presence/shape rather than a literal value, because the text tracks the destination's current status).
  Emulator: returns 200 with statusReason absent (None).
  Why: the reason set at create (or on a confirmation failure) is never readable back, so the documented way to get details about an ERROR or IN_PROGRESS destination yields nothing. This is the read side of the same gap as Cause 5 and needs fixing in both places to be observable.
  Fix: GetTopicRuleDestination's handler/serializer — project the stored statusReason into the response.
  Traces: iot-topic-rule-destination-http-lifecycle#task#2.

Cause 7 of 7: a destination deleted by DeleteTopicRuleDestination is still served by GetTopicRuleDestination.
  AWS: Smithy: DeleteTopicRuleDestination DELETE /destinations/{arn+} removes the destination; a subsequent GetTopicRuleDestination for that ARN is no longer readable and answers the modeled UnauthorizedException (client, httpError 401) — the recorded response is HTTP 401 with `message` of the form "Access to TopicRuleDestination '<arn>' was denied" (same fixed text as Cause 4, ARN from the request path).
  Emulator: answers HTTP 200 with a destination body, and no `message` or x-amzn-errortype header at all — the delete left the record readable in state.
  Why: this is a state-correctness defect, not just a wording one: a deleted destination remains visible indefinitely, so delete-then-verify teardown passes vacuously, re-creating the destination can collide with a record that should be gone, and a client sees a destination AWS would say it cannot read. Combined with Cause 4, the absent-destination path is wrong in both directions.
  Fix: DeleteTopicRuleDestination's state mutation (the destination must be removed from, or tombstoned in, the topic-rule-destination store) plus the GetTopicRuleDestination lookup that must then take the UnauthorizedException path from Cause 4.
  Traces: iot-topic-rule-destination-http-lifecycle#teardown#4.

Related open issues (neither covers these causes; both are about the confirmation/delivery trajectory hanging, not the wire shape of these responses): 20261107T063615Z-iot-the-unconfirmed-http-topic-rule-destination-delivery.md and 20261107T102706Z-iot-the-http-topic-rule-destination-lifecycle-create.md.

## Evidence
Replayed trace set simparity-wire-aws-v1 against the emulator (emulator_up=true, bootstrap_signal=false, 1104 steps, 407 passed, score 36.9%). Seven net-new divergence clusters, all service=iot, none stub-blocked and none owned by an open issue:

Cause 1 — ConfirmTopicRuleDestination, iot-topic-rule-destination-confirm-invalid-token-error#task#1 (cluster size 2):
  expected status 400, actual 400
  $.message: expected 'Invalid confirmation token.', got 'Invalid confirmation token'

Cause 2 — CreateTopicRuleDestination, iot-topic-rule-destination-errors#task#1 (cluster size 2):
  expected status 400, actual 400
  $.message: expected "Invalid confirmationUrl: 'http://example.com/insecure' is not a valid HTTPS URL", got "confirmationUrl http://example.com/insecure is not a URL this destination's confirmation request can be sent to: its scheme is not https"

Cause 3 — GetTopicRuleDestination, iot-topic-rule-destination-errors#task#2 (cluster size 2):
  expected status 400, actual 400
  $.message: expected "ARNs must start with 'arn:': not-a-real-destination-arn", got 'Topic rule destination not-a-real-destination-arn not found'

Cause 4 — GetTopicRuleDestination, iot-topic-rule-destination-get-nonexistent-error#task#1:
  status: expected 401, got 400
  $.message: expected "Access to TopicRuleDestination '<destination-arn>' was denied", got 'Topic rule destination <destination-arn> not found'
  $headers.x-amzn-errortype: expected 'UnauthorizedException:http://iot.amazonaws.com/doc/2015-10-02/', got 'InvalidRequestException'

Cause 5 — CreateTopicRuleDestination, iot-topic-rule-destination-http-lifecycle#task#1:
  expected status 200, actual 200
  $.topicRuleDestination.statusReason: missing — expected 'Awaiting confirmation. The confirmation message has not been sent to the destination endpoint yet.', got None

Cause 6 — GetTopicRuleDestination, iot-topic-rule-destination-http-lifecycle#task#2:
  expected status 200, actual 200
  $.topicRuleDestination.statusReason: missing — expected a non-empty reason (fixture asserts shape, not a literal), got None

Cause 7 — GetTopicRuleDestination after DeleteTopicRuleDestination, iot-topic-rule-destination-http-lifecycle#teardown#4:
  status: expected 401, got 200
  $.message: expected "Access to TopicRuleDestination '<destination-arn>' was denied", got None
  $headers.x-amzn-errortype: expected the UnauthorizedException error type, got None

(ARNs, URLs and generated destination names above are per-run values from the recordings; only the fixed message text, the status codes and the error codes are the requirement.)

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire fixture corpus (replay_wire_corpus).
3. Inspect these fixtures/steps:
   - iot-topic-rule-destination-confirm-invalid-token-error#task#1 — ConfirmTopicRuleDestination with an unrecognised confirmation token; compare $.message against 'Invalid confirmation token.' (Cause 1).
   - iot-topic-rule-destination-errors#task#1 — CreateTopicRuleDestination with an http:// confirmationUrl; compare $.message against AWS's "Invalid confirmationUrl: '<url>' is not a valid HTTPS URL" (Cause 2).
   - iot-topic-rule-destination-errors#task#2 — GetTopicRuleDestination with a non-ARN path value; compare $.message against "ARNs must start with 'arn:': <value>" (Cause 3).
   - iot-topic-rule-destination-get-nonexistent-error#task#1 — GetTopicRuleDestination for a well-formed but unreadable destination ARN; expect 401 / UnauthorizedException / "Access to TopicRuleDestination '<arn>' was denied" (Cause 4).
   - iot-topic-rule-destination-http-lifecycle#task#1 and #task#2 — create an HTTP destination then get it; check topicRuleDestination.statusReason is populated on both responses (Causes 5 and 6).
   - iot-topic-rule-destination-http-lifecycle#teardown#4 — DeleteTopicRuleDestination, then GetTopicRuleDestination for the same ARN; expect 401 UnauthorizedException, not 200 with the destination body (Cause 7).
4. Each listed step must pass on re-replay before this issue is closed.
