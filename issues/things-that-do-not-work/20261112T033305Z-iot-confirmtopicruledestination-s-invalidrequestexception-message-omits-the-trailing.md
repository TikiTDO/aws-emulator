Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.e2e-topic-rule-destination-confirmation-flow iot.topic-rule-destination-confirmation-rejected-without-valid-token
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot.e2e-topic-rule-destination-confirmation-flow|run#13|AssertionError|assert message == expected, f"ConfirmTopicRuleDestination message {message!r} != {expected!r}"
Issue-Key: iot.topic-rule-destination-confirmation-rejected-without-valid-token|run#9|AssertionError|assert message == "Invalid confirmation token.", f"expected 'Invalid confirmation token.', got {message!r}"
Issue-Commit: 03f1d5047411e59a2cbababc00e134c537b99208

# iot: ConfirmTopicRuleDestination's InvalidRequestException message omits the trailing period AWS sends ('Invalid confirmation token' vs 'Invalid confirmation token.')

> _Note: this issue was identified against the code base at commit `03f1d5047411e59a2cbababc00e134c537b99208`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ConfirmTopicRuleDestination

## Description
Cause 1 of 1: the refusal message AWS IoT returns when ConfirmTopicRuleDestination is called with a confirmation token that no outstanding destination challenge carries is reproduced without its terminating period, so the error message text diverges from AWS.

AWS: Smithy (services/service-com-amazonaws-iot-awsiotservice/smithy-model/com-amazonaws-iot-awsiotservice.json) models ConfirmTopicRuleDestination as GET /confirmdestination/{confirmationToken+} with required input member confirmationToken (ConfirmationToken, @length 1..2048) and error InvalidRequestException (client, HTTP 400, "The request is not valid."). The model does not state the message text; recorded behavior only: for a bogus/unissued confirmation token, real AWS answers HTTP 400 with Error.Code "InvalidRequestException" and the message "Invalid confirmation token." — this message is entirely FIXED text, including the final period; it interpolates no request- or state-derived value (not the token, which is never echoed). Both recorded checks assert that exact string.

Emulator: answers the right status (400) and the right code (InvalidRequestException), but the message is "Invalid confirmation token" — the same sentence with the trailing '.' missing. Replay output: AssertionError: ConfirmTopicRuleDestination message 'Invalid confirmation token' != 'Invalid confirmation token.' and AssertionError: expected 'Invalid confirmation token.', got 'Invalid confirmation token'. Everything else in these two checks is faithful: creation returns IN_PROGRESS, the rejected confirmation leaves the destination IN_PROGRESS/not ENABLED, and a second destination still creates IN_PROGRESS.

Why: the message is part of the refusal contract. A client that branches, asserts, logs-and-matches, or surfaces the operator-facing string (parity/compliance tests, canned error maps keyed on the message, user-visible text) sees a string AWS never sends, so a code path verified against AWS silently takes the wrong branch or reports an unknown error against the emulator while the status and code look correct.

Fix: belongs in the IoT state layer's shared message constant used by the ConfirmTopicRuleDestination handler's token-lookup refusal — the text should be AWS's complete sentence, terminating period included; no other part of the handler's behavior needs to change.

Traces: iot.e2e-topic-rule-destination-confirmation-flow and iot.topic-rule-destination-confirmation-rejected-without-valid-token. Handler site: services/service-com-amazonaws-iot-awsiotservice/src/ops/confirm_topic_rule_destination.rs lines 184-194, which builds InvalidRequestException from crate::state::INVALID_CONFIRMATION_TOKEN_MESSAGE (defined in services/service-com-amazonaws-iot-awsiotservice/src/state.rs). Related open issues touch the same destination lifecycle but a different cause and do not cover this message: 20261029T175031Z-iot-cfn-deployed-aws-iot-topicrule-is-not.md, 20261107T063615Z-iot-the-unconfirmed-http-topic-rule-destination-delivery.md, 20261107T102706Z-iot-the-http-topic-rule-destination-lifecycle-create.md.

## Evidence
Check iot.topic-rule-destination-confirmation-rejected-without-valid-token:
  request: ConfirmTopicRuleDestination(confirmationToken="sim2real-bogus-token-<unique>")
  expected (AWS): HTTP 400, Error.Code=InvalidRequestException, Error.Message="Invalid confirmation token."
  actual (emulator): HTTP 400, Error.Code=InvalidRequestException, Error.Message="Invalid confirmation token"
  assertion: AssertionError: expected 'Invalid confirmation token.', got 'Invalid confirmation token'

Check iot.e2e-topic-rule-destination-confirmation-flow:
  request: ConfirmTopicRuleDestination(confirmationToken="not-a-real-confirmation-token")
  assertion: AssertionError: ConfirmTopicRuleDestination message 'Invalid confirmation token' != 'Invalid confirmation token.'

Fixed vs variable parts of the message: the whole string "Invalid confirmation token." is fixed text; there are no variable parts taken from the request or from emulator state. The divergence is the missing final '.' only — status code and error code already match.

## Steps to Reproduce
1. Boot the emulator and point an IoT client at it.
2. iot.create_topic_rule_destination(destinationConfiguration={"httpUrlConfiguration": {"confirmationUrl": "https://example.com/sim2real-iot-confirm"}}) -> status IN_PROGRESS.
3. iot.confirm_topic_rule_destination(confirmationToken="sim2real-bogus-token-abc") with a token no outstanding challenge carries.
4. Inspect the ClientError: Error.Code is InvalidRequestException (correct) and Error.Message is "Invalid confirmation token" instead of AWS's "Invalid confirmation token.".
5. Re-replay iot.topic-rule-destination-confirmation-rejected-without-valid-token and iot.e2e-topic-rule-destination-confirmation-flow to confirm the fix.
