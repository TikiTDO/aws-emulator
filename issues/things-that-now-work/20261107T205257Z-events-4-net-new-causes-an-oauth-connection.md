Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.api-destination-oauth-connection-fetches-a-client-credentials-token-and-sends-it-as-the-bearer-header events.disabled-rule-with-a-dead-letter-configured-target-drops-the-event-instead-of-dead-lettering-it events.input-transformer-plain-text-template-does-not-escape-strings-and-dead-letters-invalid-json events.sqs-target-encrypted-with-a-key-events-cannot-use-dead-letters-until-the-key-policy-grants-it
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events.api-destination-oauth-connection-fetches-a-client-credentials-token-and-sends-it-as-the-bearer-header|_create_connection#5|AssertionError|assert time.monotonic() < deadline, f"the connection is {state!r}, never AUTHORIZED"
Issue-Key: events.disabled-rule-with-a-dead-letter-configured-target-drops-the-event-instead-of-dead-lettering-it|run#30|AssertionError|assert attrs["ERROR_CODE"] == "NO_PERMISSIONS", (
Issue-Key: events.input-transformer-plain-text-template-does-not-escape-strings-and-dead-letters-invalid-json|_drain#28|AssertionError|raise AssertionError(
Issue-Key: events.sqs-target-encrypted-with-a-key-events-cannot-use-dead-letters-until-the-key-policy-grants-it|_await#22|AssertionError|raise AssertionError(f"no matching message reached the queue within {timeout_s}s")
Issue-Commit: 959ef406fada22402fcbc8404625fdf48db790f8

# events: 4 net-new causes — an OAUTH connection never reaches AUTHORIZED, dead-letter ERROR_CODE is NO_RESOURCE where AWS says NO_PERMISSIONS, a plain-text InputTemplate rendering invalid JSON is never dead-lettered, and a KMS-encrypted SQS target is never refused

> _Note: this issue was identified against the code base at commit `959ef406fada22402fcbc8404625fdf48db790f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConnection / PutTargets / PutEvents

## Description
Four causes under the `events` heading, all un-owned in the replay report. Related open issues that do not cover them: 20261102T091831Z-events-api-destinations-never-deliver-or-dead-letter.md, 20261104T120007Z-events-updateconnection-s-authparameters-without-authorizationtype-refusal-uses.md, 20261029T014025Z-events-3-net-new-behavioral-root-causes-an.md.

Cause 1 of 4: an OAUTH_CLIENT_CREDENTIALS connection never becomes AUTHORIZED.
AWS: Smithy ConnectionState = CREATING|UPDATING|DELETING|AUTHORIZED|DEAUTHORIZED|AUTHORIZING|DEAUTHORIZING|ACTIVE|FAILED_CONNECTIVITY. A connection created with AuthorizationType OAUTH_CLIENT_CREDENTIALS and valid OAuth parameters settles in AUTHORIZED; an API destination using it then fetches a client-credentials token from the authorization endpoint and sends it as the Authorization: Bearer <token> request header.
Emulator: the connection settles in DEAUTHORIZED and never reaches AUTHORIZED, so no token is ever fetched and the Bearer header is never sent.
Failing check: events.api-destination-oauth-connection-fetches-a-client-credentials-token-and-sends-it-as-the-bearer-header.

Cause 2 of 4: the dead-letter ERROR_CODE for a target EventBridge is not permitted to invoke is NO_RESOURCE instead of NO_PERMISSIONS.
AWS: while a rule is DISABLED a matching event never reaches the target's dead-letter queue; once the rule is ENABLED, the next matching event whose target invocation is refused for permissions is dead-lettered with message attributes ERROR_CODE=NO_PERMISSIONS, ERROR_MESSAGE = the target service's own AccessDenied message, RULE_ARN = the rule's ARN and TARGET_ARN = the target's ARN.
Emulator: the dead letter carries ERROR_CODE 'NO_RESOURCE'. Note the inverse mapping error in the same family: the owned events__sqs check for a DELETED queue target receives ERROR_CODE 'AWS.SimpleQueueService.NonExistentQueue' where AWS reports NO_RESOURCE — so the ERROR_CODE classification table itself is wrong in both directions, not just in one entry.
Failing check: events.disabled-rule-with-a-dead-letter-configured-target-drops-the-event-instead-of-dead-lettering-it.

Cause 3 of 4: a plain-text InputTemplate whose substitution yields invalid JSON is never dead-lettered.
AWS: Smithy InputTransformer.InputTemplate (required, length 1..8192) may be plain text as well as JSON; substituted string values are NOT escaped, so a plain-text template can render a body that is not valid JSON, and the resulting invocation is dead-lettered on the target's DeadLetterConfig queue.
Emulator: neither plain-text target dead-letters the event — "both plain-text targets dead-lettering the event: not met within 120.0s; received 0 message(s): []" — so the failure is silently dropped.
Failing check: events.input-transformer-plain-text-template-does-not-escape-strings-and-dead-letters-invalid-json.

Cause 4 of 4: an SQS target encrypted with a CMK that events.amazonaws.com may not use is delivered to instead of refused, so no NO_PERMISSIONS dead letter is produced.
AWS: when the target queue's KmsMasterKeyId names a key whose key policy does not grant events.amazonaws.com kms:GenerateDataKey and kms:Decrypt, the target invocation fails, the event is dead-lettered with ERROR_CODE=NO_PERMISSIONS, KMS's AccessDenied message (with the minted Request ID as its only variable part), TARGET_ARN = the encrypted queue and RULE_ARN = the rule, the encrypted queue stays empty, and the healthy sibling target still receives; after the key policy grants those two actions, the same target receives.
Emulator: the healthy sibling receives but no message ever reaches the dead-letter queue — "no matching message reached the queue within 120.0s" — i.e. the KMS key policy is not evaluated for the target write at all.
Failing check: events.sqs-target-encrypted-with-a-key-events-cannot-use-dead-letters-until-the-key-policy-grants-it.

Why: an API destination behind OAuth never authenticates (cause 1); operators keying alarms and replay tooling off DLQ ERROR_CODE route permission failures as missing-resource failures and vice versa (cause 2); a malformed transformer silently loses events instead of parking them for replay (cause 3); and an encryption boundary the client relies on is not enforced, so events land in a queue AWS would have refused and the expected dead letter never exists (cause 4).

Fix: the connection authorization state machine and its token fetch (cause 1); the dead-letter classification that maps a target invocation failure to ERROR_CODE/ERROR_MESSAGE (causes 2 and 4 — KMS authorization of the target write must be evaluated and routed into that same classification); and the input-transformer rendering/validation path that must dead-letter an invocation whose rendered input the target rejects (cause 3).

Traces: the four failing check ids in trace_ids.

## Evidence
Cause 1 — Expected: connection state AUTHORIZED and Authorization: Bearer <token> on the delivered request. Actual: AssertionError: the connection is 'DEAUTHORIZED', never AUTHORIZED.
Cause 2 — Expected: attrs["ERROR_CODE"] == "NO_PERMISSIONS". Actual: AssertionError: the always-failing target dead-lettered the post-enable event as 'NO_RESOURCE'.
Cause 3 — Expected: a dead-letter message for each of the two plain-text targets. Actual: AssertionError: both plain-text targets dead-lettering the event: not met within 120.0s; received 0 message(s): [].
Cause 4 — Expected: a dead letter with ERROR_CODE=NO_PERMISSIONS and KMS's AccessDenied ERROR_MESSAGE, the encrypted queue empty, the healthy sibling delivered. Actual: AssertionError: no matching message reached the queue within 120.0s (the DLQ never received anything; the healthy sibling did receive).

## Steps to Reproduce
Cause 1: CreateConnection AuthorizationType=OAUTH_CLIENT_CREDENTIALS with client id/secret and an authorization endpoint; CreateApiDestination on it; DescribeConnection and poll ConnectionState; PutEvents to a rule targeting the destination and inspect the request headers the endpoint received.
Cause 2: CreateEventBus; PutRule State=DISABLED with an SQS target whose queue policy denies events and a DeadLetterConfig queue; PutEvents (expect nothing on the DLQ); EnableRule; PutEvents again; read the DLQ message attributes.
Cause 3: PutTargets with InputTransformer whose InputTemplate is a bare plain-text sentence containing a substituted string value; PutEvents a matching event; poll both targets' DLQs.
Cause 4: CreateKey with a root-only key policy; create an SQS queue with KmsMasterKeyId = that key and a second unencrypted queue; PutTargets with the encrypted queue (DeadLetterConfig -> third queue) and the unencrypted one; PutEvents; read the DLQ; then add kms:GenerateDataKey + kms:Decrypt for events.amazonaws.com to the key policy and PutEvents again.
