Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns.publish-to-topic-whose-key-is-pending-deletion-fails-kms-invalid-state
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns.publish-to-topic-whose-key-is-pending-deletion-fails-kms-invalid-state|run#14|AssertionError|assert (code, message) == ("KMSInvalidState", expected), (
Issue-Commit: 56af4a38801370870cefc18c3ef22e7084182017

# sns: Publish to an SSE-KMS topic ignores the CMK's PendingDeletion and Disabled states — no KMSInvalidState / KMSDisabled refusal and no relayed KMS error envelope

> _Note: this issue was identified against the code base at commit `56af4a38801370870cefc18c3ef22e7084182017`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
ONE net-new root cause (with two refusal classes it must produce), grounded in the one failing check of this suite's `sns` service that the replay report marks neither ALREADY FILED nor OWNED.

Related OPEN issues, named for context only — neither carries this check's replay key, so neither covers this failure: 20261027T030148Z-sns-6-net-new-behavioral-root-causes-no.md (a sibling key-state cause) and 20261102T133928Z-kms-sns-publish-to-an-sse-kms-topic-b9e489.md (the DISABLED-key case on the `kms__sns` heading).

Cause 1 of 1: Publish to a topic whose KmsMasterKeyId names a CMK that is PendingDeletion (and, after CancelKeyDeletion, Disabled) is accepted instead of relaying KMS's refusal.

AWS: Smithy (com.amazonaws.sns#Publish) models KMSInvalidStateException (awsQueryError code `KMSInvalidState`, httpError 400) and KMSDisabledException (code `KMSDisabled`, httpError 400). Recorded behavior: with the key Enabled Publish succeeds; after kms:ScheduleKeyDeletion (DescribeKey KeyState="PendingDeletion") Publish is refused 400 `KMSInvalidState` whose Message is KMS's own error relayed verbatim — fixed text "<key ARN> is pending deletion. (Service: AWSKMS; Status Code: 400; Error Code: KMSInvalidStateException; Request ID: <kms request id>; Proxy: null)", where the key ARN is the topic's resolved KmsMasterKeyId and the request id is the per-call KMS request id (the only per-run token; the check masks it). After kms:CancelKeyDeletion the key is Disabled and Publish is refused 400 `KMSDisabled` with the same relayed envelope reading "<key ARN> is disabled. (... Error Code: DisabledException; ...)". After kms:EnableKey Publish succeeds again.

Emulator: every Publish in both broken states returns 200 with a MessageId — the check's bounded poll reports ('<succeeded>', None) for the PendingDeletion leg, i.e. no refusal code and no message at all, so neither refusal class nor the relayed KMS envelope exists. Hypothesis: the topic's KmsMasterKeyId is a stored attribute only and no KMS call is made per publish, so no key state can reach the publish path.

Why: scheduling a key for deletion is how an operator permanently stops an encrypted topic; a client told the publish succeeded believes a message was enveloped under a key in a state that cannot encrypt. The two codes are also semantically different for callers — `KMSInvalidState` is permanent until the key state changes, `KMSDisabled` is reversible — and the relayed KMS envelope is what operators grep for to find which key blocked delivery.

Fix: the SNS Publish handler's encryption/key-resolution path (shared with PublishBatch) — resolve the topic's CMK and request a data key from KMS per publish, mapping KMS's KMSInvalidStateException onto `KMSInvalidState` and DisabledException onto `KMSDisabled`, carrying KMS's message through unchanged in the relayed "(Service: AWSKMS; Status Code: 400; Error Code: ...; Request ID: ...; Proxy: null)" envelope.

Traces: sns.publish-to-topic-whose-key-is-pending-deletion-fails-kms-invalid-state (mismatch). Handler component: services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/ops/publish.rs.

## Evidence
Failing check (suite service heading `### sns`): sns.publish-to-topic-whose-key-is-pending-deletion-fails-kms-invalid-state

AssertionError: "Publish while the key is pending deletion must fail KMSInvalidState 'arn:aws:kms:<region>:<account>:key/<key id> is pending deletion. (Service: AWSKMS; Status Code: 400; Error Code: KMSInvalidStateException; Request ID: <a KMS request id>; Proxy: null)'; got '<succeeded>' None" (the recorded run's key ARN and region/account are request/state values filling that template, not part of it).

Expected (PendingDeletion leg): HTTP 400, Error.Code `KMSInvalidState`, Message = the relayed KMS envelope above.
Expected (post-CancelKeyDeletion, key Disabled): HTTP 400, Error.Code `KMSDisabled`, Message "<key ARN> is disabled. (Service: AWSKMS; Status Code: 400; Error Code: DisabledException; Request ID: <kms request id>; Proxy: null)".
Actual: HTTP 200 with a MessageId; the poll's returned code is the check's '<succeeded>' sentinel and the message is None.
Control: the Publish before ScheduleKeyDeletion returned a MessageId, so the topic itself was serving.

## Steps to Reproduce
1. kms:CreateKey; sns:CreateTopic(Attributes={"KmsMasterKeyId": "<key id>"}).
2. sns:Publish(TopicArn=<topic>, Message="before") — succeeds (positive control).
3. kms:ScheduleKeyDeletion(KeyId=<key>, PendingWindowInDays=7); kms:DescribeKey shows KeyState="PendingDeletion".
4. sns:Publish(TopicArn=<topic>, Message="probe") — EXPECT HTTP 400 Error.Code `KMSInvalidState` with the relayed "<key ARN> is pending deletion. (Service: AWSKMS; Status Code: 400; Error Code: KMSInvalidStateException; Request ID: <id>; Proxy: null)" message; OBSERVE HTTP 200 with a MessageId.
5. kms:CancelKeyDeletion(KeyId=<key>) (KeyState becomes "Disabled"); sns:Publish — EXPECT HTTP 400 `KMSDisabled` with the relayed "... is disabled. (... Error Code: DisabledException; ...)" message; OBSERVE HTTP 200.
6. kms:EnableKey(KeyId=<key>); sns:Publish succeeds again.
Replay: sns.publish-to-topic-whose-key-is-pending-deletion-fails-kms-invalid-state.
