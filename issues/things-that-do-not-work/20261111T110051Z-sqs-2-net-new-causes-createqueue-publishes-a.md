Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sqs.client-on-the-queue-url-host-sends-and-receives-on-that-queue sqs.standard-queue-holds-more-than-one-thousand-messages
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs.client-on-the-queue-url-host-sends-and-receives-on-that-queue|run#5|AssertionError|assert not (p := ctx.published_url_mismatch(url)), p
Issue-Key: sqs|SendMessageBatch|OverLimit
Issue-Commit: 56af4a38801370870cefc18c3ef22e7084182017

# sqs: 2 net-new causes — CreateQueue publishes a QueueUrl on a real-AWS host the emulator does not serve, and a queue is capped at 1000 messages with an unmodeled OverLimit refusal

> _Note: this issue was identified against the code base at commit `56af4a38801370870cefc18c3ef22e7084182017`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateQueue

## Description
Two net-new root causes in sqs, both grounded in failing behavioral checks the replay report left unmarked (no ALREADY-FILED marker, no OWNED marker). Both must be closed.

Cause 1 of 2 — CreateQueue answers a QueueUrl whose host is a production AWS endpoint rather than the endpoint the request arrived on.
AWS: Smithy (services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json) models CreateQueue's output as CreateQueueResult.QueueUrl (String) and does not constrain its form, so this is the endpoint contract plus recorded behavior: the QueueUrl AWS returns is an address on the endpoint the request was made against, and it is the ONLY handle every queue-addressed operation takes (SendMessage, ReceiveMessage, DeleteMessage, GetQueueAttributes, SetQueueAttributes, PurgeQueue, DeleteQueue). A client is required to hand the returned string straight back, so the host it names must be the host that serves the queue.
Emulator: the url is composed from region, account and queue name alone — `https://sqs.<region>.amazonaws.com/<account-id>/<queue-name>` — with no reference to the endpoint the request arrived on. The check's own harness reports it: "the emulator published https://sqs.<region>.amazonaws.com/<account>/<queue-name>: its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://<own host>:<port>/<resource path>".
Why: a client that does the normal thing — keep CreateQueue's QueueUrl and address a client at it — is pointed at real AWS. Depending on egress it either leaves the sandbox and signs against a real account, or fails name resolution/connect; in neither case does it reach the queue it just created. Every create-then-use flow (SDK, CLI, IaC, worker bootstrap) breaks although CreateQueue reported 200.
Fix: the shared queue-url builder used by the CreateQueue handler (and by every operation that echoes a queue url) — derive scheme/host/port from the request's own endpoint (Host header / configured listener) and keep only the `/<account-id>/<queue-name>` path from state.
Traces: sqs.client-on-the-queue-url-host-sends-and-receives-on-that-queue. Related but NOT covering it: 20261109T191027Z-sqs-queue-url-addressing-diverges-from-aws-createqueue.md and 20261111T055733Z-sqs-queue-urls-returned-by-the-read-operations.md record this host defect against WIRE fixture steps (CreateQueue/GetQueueUrl/ListQueues replay keys); neither carries this behavioral check's replay key, and the report marked this check unowned.

Cause 2 of 2 — a standard queue is capped at 1000 stored messages and the 1001st send is refused with an emulator-authored error that AWS does not model.
AWS: a standard SQS queue has NO limit on the number of messages it can store; the related quota is 120,000 messages IN FLIGHT (received but not deleted) for a standard queue, and that one is reported as OverLimit from ReceiveMessage, not from a send. Smithy models SendMessageBatch's errors as exactly BatchEntryIdsNotDistinct, BatchRequestTooLong, EmptyBatchRequest, InvalidAddress, InvalidBatchEntryId, InvalidSecurity, the Kms* family, QueueDoesNotExist, RequestThrottled, TooManyEntriesInBatchRequest and UnsupportedOperation — there is no OverLimit among them, so no AWS client can even expect that error code from this operation. A send of the 1001st message is a plain success: the batch comes back with every entry under `Successful` and nothing under `Failed`.
Emulator: SendMessageBatch is refused with error code `OverLimit` and the message "This emulator holds at most 1000 messages per queue; queue <queue-name> cannot take 1 more. Delete messages from it first." — text that names the emulator itself, so it is not AWS wording at all; the queue name is the only request-derived part of it.
Why: the cap silently changes queue semantics. A producer that buffers work faster than its consumer drains it — a backlog build-up, a replay, a load or soak test, a DLQ accumulating poison messages — starts failing at an arbitrary depth AWS does not have, and it fails with a code (`OverLimit`) that is not in SendMessageBatch's modeled error set, so SDK retry/classification logic treats it as an unexpected fault rather than throttling or a validation error. The message also leaks that the service is an emulator into client-visible output.
Fix: the SQS queue state's per-queue message storage/accounting (the depth guard that SendMessage/SendMessageBatch consult) — remove the 1000-message ceiling so stored depth is unbounded, and keep enforcement only where AWS has it: the in-flight limit on receive, surfaced as the modeled OverLimit for ReceiveMessage with AWS's wording.
Traces: sqs.standard-queue-holds-more-than-one-thousand-messages.

## Evidence
Cause 1 — sqs.client-on-the-queue-url-host-sends-and-receives-on-that-queue, verdict mismatch.
The check creates a queue and, before using it, asserts the published url is addressable:
  AssertionError: the emulator published https://sqs.us-west-2.amazonaws.com/000000000000/<queue-name>: its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener
The path part `/<account-id>/<queue-name>` is right; the HOST is a production sqs endpoint rather than the endpoint the request was sent to. The account id, queue name, host and port above are per-run values; the requirement is that the returned QueueUrl's scheme/host/port are the endpoint that served the request. The remainder of the check — sending and receiving on a client built from that url's host — was blocked by this assertion.
Smithy citation: CreateQueue output member QueueUrl: String (unconstrained), and it is the input member of every queue-addressed operation.

Cause 2 — sqs.standard-queue-holds-more-than-one-thousand-messages, verdict error.
The check sends 1001 messages to a standard queue and then drains and counts them.
  Expected: all 1001 sends succeed (SendMessageBatch returns every entry under Successful, Failed empty), and the queue yields 1001 messages on drain.
  Actual: botocore.errorfactory.OverLimit: An error occurred (OverLimit) when calling the SendMessageBatch operation: This emulator holds at most 1000 messages per queue; queue <queue-name> cannot take 1 more. Delete messages from it first.
The emulator's fixed text is "This emulator holds at most 1000 messages per queue; queue " + " cannot take 1 more. Delete messages from it first.", with the queue name taken from the request — none of it is AWS wording, and `OverLimit` is not in SendMessageBatch's modeled error set.
Smithy citation: SendMessageBatch's modeled errors are BatchEntryIdsNotDistinct, BatchRequestTooLong, EmptyBatchRequest, InvalidAddress, InvalidBatchEntryId, InvalidSecurity, KmsAccessDenied, KmsDisabled, KmsInvalidKeyUsage, KmsInvalidState, KmsNotFound, KmsOptInRequired, KmsThrottled, QueueDoesNotExist, RequestThrottled, TooManyEntriesInBatchRequest, UnsupportedOperation. OverLimit is absent.

## Steps to Reproduce
Cause 1: call sqs:CreateQueue(QueueName=<name>) against the emulator's endpoint and read CreateQueueResult.QueueUrl. Expected: its scheme/host/port are the endpoint the request was sent to, with path /<account-id>/<queue-name>. Observed: https://sqs.<region>.amazonaws.com/<account-id>/<queue-name>. Then build a fresh SQS client whose endpoint is that url's scheme://host[:port] and call SendMessage/ReceiveMessage with the url — it must reach the queue just created.

Cause 2: sqs:CreateQueue(QueueName=<name>) (standard, default attributes). Send 1001 messages, e.g. 100 SendMessageBatch calls of 10 entries plus one more. Expected: every call returns 200 with all entries under Successful and Failed empty, and GetQueueAttributes(ApproximateNumberOfMessages) plus a full drain account for 1001 messages. Observed: the send that would make the queue hold the 1001st message is refused HTTP 4xx with error code OverLimit and the message "This emulator holds at most 1000 messages per queue; queue <name> cannot take 1 more. Delete messages from it first." Separately confirm the in-flight quota is the only depth limit enforced: receive without deleting and check that OverLimit appears on ReceiveMessage at AWS's 120,000 in-flight bound, not on a send.
