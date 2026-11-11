Issue-Source: trace-sweep-divergence
Issue-Trace: sqs-dead-letter-queue-and-message-move
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sqs-dead-letter-queue-and-message-move sqs-dead-letter-queue-configuration sqs-list-queues-agent-shape-and-pagination sqs-queue-create-url-shape sqs-queue-lifecycle-basic sqs-queue-lifecycle-core sqs-queue-lifecycle-crud sqs-refarch-s3ev-queue-lookup-mistakes
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs|ListDeadLetterSourceQueues|2xx->2xx|$.queueUrls[*]
Issue-Key: sqs|ListQueues|2xx->2xx|$.QueueUrls[*]
Issue-Key: sqs|GetQueueUrl|2xx->2xx|$.QueueUrl
Issue-Commit: c43f13612f17253679972bb58b777ab0e011820f

# sqs: queue URLs returned by the read operations name an unserved real-AWS host (GetQueueUrl, ListQueues, ListDeadLetterSourceQueues)

> _Note: this issue was identified against the code base at commit `c43f13612f17253679972bb58b777ab0e011820f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQueueUrl

## Description
FIVE net-new causes, all in the sqs queue-URL emission path, all of them the SAME fault reached through a DIFFERENT operation: every operation that REPORTS a queue url answers a url whose scheme/host/port is a literal production SQS endpoint (https://sqs.<region>.amazonaws.com/...) rather than the endpoint the request arrived on, so the replay comparator flags each value host_not_own even though the /<account-id>/<queue-name> path matches the recording. All five must be closed.

General behaviour at stake (applies to every cause below). AWS: the queue url a response reports is an address on the endpoint the request was made against, and it is the ONLY handle the queue-addressed operations (GetQueueAttributes, SetQueueAttributes, SendMessage, ReceiveMessage, DeleteMessage, PurgeQueue, DeleteQueue, ListDeadLetterSourceQueues, StartMessageMoveTask) accept as input. A client is required to take the string it was given and send it straight back, so whatever host it names must be the host that serves the queue. The Smithy model types these members only as String (no form constraint), so the url's FORM is recorded behavior only plus the endpoint contract; what the model does fix is that these are the queue handles. Emulator: the url is composed from region + account + queue name with no reference to the request's own endpoint, and the read paths echo the string state minted at create time, so the production host propagates into every read. Why: a client doing the normal thing — discover a queue url and use it — is pointed at real AWS instead of the emulator; depending on egress it either leaves the sandbox (signing against a real account) or fails name resolution/connect, and never reaches the queue. Discovery-then-use flows (SDK, CLI, IaC, workers, redrive tooling) break even though each call reported 200. Fix: ONE shared queue-url builder that derives scheme/host/port from the request's own endpoint (the Host header / the configured data listener) and keeps only the /<account-id>/<queue-name> path from state; every operation that emits a queue url must render through it at RESPONSE time rather than echoing a stored absolute url.

Cause 1 of 5: ListDeadLetterSourceQueues reports source queue urls on an unserved host.
AWS: Smithy ListDeadLetterSourceQueues -> ListDeadLetterSourceQueuesResult.queueUrls (required, QueueUrlList of String, xmlName QueueUrl, xmlFlattened) — 'a list of source queue URLs that have the RedrivePolicy queue attribute configured with a dead-letter queue'; recorded behavior only for the url form. Each element must be an address on the endpoint answering the call, because redrive tooling feeds these urls back into GetQueueAttributes/StartMessageMoveTask.
Emulator: each element's host is a production sqs.<region>.amazonaws.com endpoint, not the endpoint the emulator is reached on (comparator verdict host_not_own on $.queueUrls[0]); HTTP 200 otherwise.
Why: DLQ source discovery hands the caller addresses it cannot reach, so every follow-up queue-addressed call escapes the sandbox or fails to connect.
Fix: the ListDeadLetterSourceQueues handler's url emission (services/service-com-amazonaws-sqs-amazonsqs/src/ops/list_dead_letter_source_queues.rs) must render each element through the shared endpoint-aware builder.
Traces: sqs-dead-letter-queue-and-message-move#task#6 (cluster also covers sqs-dead-letter-queue-configuration#task#6).

Cause 2 of 5: ListQueues reports queue urls on an unserved host (prefix/basic listing).
AWS: Smithy ListQueues -> ListQueuesResult.QueueUrls (QueueUrlList of String, xmlName QueueUrl, xmlFlattened) + NextToken; paginated (inputToken NextToken, outputToken NextToken, items QueueUrls, pageSize MaxResults). Recorded behavior only for the url form: every listed url is an address on the answering endpoint.
Emulator: the listed elements carry the production sqs.<region>.amazonaws.com host (host_not_own on $.QueueUrls[0], $.QueueUrls[1]); HTTP 200 otherwise.
Why: the normal 'list then operate' pattern — and every tool that enumerates queues — is handed unreachable addresses.
Fix: the ListQueues handler's url emission (src/ops/list_queues.rs) through the shared builder.
Traces: sqs-list-queues-agent-shape-and-pagination#task#4.

Cause 3 of 5: ListQueues reports queue urls on an unserved host across a paginated page as well.
AWS: same ListQueues signature and pagination config as Cause 2; a paged result's elements are addresses on the answering endpoint exactly like an unpaged one.
Emulator: the three-element page's urls all carry the production host (host_not_own on $.QueueUrls[0..2]); HTTP 200 otherwise. This is a separate replay cluster from Cause 2 (different response key set), so it is a separate cause to re-run even though it shares the builder.
Why: pagination-walking clients accumulate unreachable urls for every page they fetch.
Fix: same shared builder, applied on the paginated response assembly in src/ops/list_queues.rs so page contents and first-page contents render identically.
Traces: sqs-list-queues-agent-shape-and-pagination#task#5 (cluster also covers sqs-list-queues-agent-shape-and-pagination#task#6).

Cause 4 of 5: GetQueueUrl resolves a name to a url on an unserved host.
AWS: Smithy GetQueueUrl(QueueName required, QueueOwnerAWSAccountId) -> GetQueueUrlResult.QueueUrl (String); errors InvalidAddress(404), InvalidSecurity(403), QueueDoesNotExist(400, AWS.SimpleQueueService.NonExistentQueue), RequestThrottled(403), UnsupportedOperation(400). Recorded behavior only for the url form. GetQueueUrl exists precisely so a client holding only a NAME can obtain the usable address, so the returned url must be reachable on the endpoint that answered.
Emulator: 200 with the production sqs.<region>.amazonaws.com host (host_not_own on $.QueueUrl). The handler returns the absolute url stored on the queue record (src/ops/get_queue_url.rs:209, queue.url.clone()), which was minted at CreateQueue time from region+name.
Why: the single operation whose purpose is name -> usable address returns an address that is not usable, breaking every lookup-by-name bootstrap path.
Fix: GetQueueUrl must render the response url from the request's endpoint plus the stored path rather than echoing the stored absolute string; state should keep account-id/name, not a baked-in host.
Traces: sqs-queue-create-url-shape#task#2 (cluster also covers sqs-queue-lifecycle-basic#task#5, sqs-queue-lifecycle-core#task#2, sqs-queue-lifecycle-crud#task#2, sqs-refarch-s3ev-queue-lookup-mistakes#task#6).

Cause 5 of 5: ListQueues reports an unserved host in the single-queue lifecycle listing.
AWS: same ListQueues signature as Cause 2.
Emulator: the one listed url carries the production host (host_not_own on $.QueueUrls[0]); HTTP 200 otherwise. Separate replay cluster (single-element key set) from Causes 2 and 3.
Why: as Cause 2 — a lifecycle flow that creates, lists and then addresses the queue is sent off-box at the addressing step.
Fix: same shared builder in src/ops/list_queues.rs.
Traces: sqs-queue-lifecycle-core#task#5 (cluster also covers sqs-refarch-s3ev-queue-lookup-mistakes#task#5).

Related but NOT covering these: open issue 20261109T191027Z-sqs-queue-url-addressing-diverges-from-aws-createqueue.md carries the CreateQueue minting cause and the path-less-url InvalidAddress refusal, and open issue 20261109T185037Z-sqs-5-net-new-causes-the-query-protocol-does.md touches CreateQueue for other reasons; neither carries the replay keys of the five read-path causes above, so fixing CreateQueue alone will not close them — the read operations echo stored urls and must each be re-rendered.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up true, 283 steps, 26 mismatched). Five net-new clusters, all verdict=mismatch, service=sqs, none owned by an open issue and none stub-blocked:

Cause 1 — divergence_key service=sqs|ListDeadLetterSourceQueues|status=200->200|keys=$.queueUrls[0]; expected 200, actual 200; detail: "$.queueUrls[0]: host_not_own (expected 'https://sqs.<region>.amazonaws.com/<account>/<dlq-source-queue-name>', got 'https://sqs.<region>.amazonaws.com/<account>/<dlq-source-queue-name>')" — path matches the recording, HOST is a production endpoint instead of the endpoint the replay harness reached (the emulator's own data listener). affected_count 2: sqs-dead-letter-queue-and-message-move#task#6, sqs-dead-letter-queue-configuration#task#6.

Cause 2 — divergence_key service=sqs|ListQueues|status=200->200|keys=$.QueueUrls[0],$.QueueUrls[1]; expected 200, actual 200; detail: host_not_own on both elements, same host-vs-path split. affected_count 1: sqs-list-queues-agent-shape-and-pagination#task#4.

Cause 3 — divergence_key service=sqs|ListQueues|status=200->200|keys=$.QueueUrls[0],$.QueueUrls[1],$.QueueUrls[2]; expected 200, actual 200; detail: host_not_own on all three elements. affected_count 2: sqs-list-queues-agent-shape-and-pagination#task#5, #task#6.

Cause 4 — divergence_key service=sqs|GetQueueUrl|status=200->200|keys=$.QueueUrl; expected 200, actual 200; detail: "$.QueueUrl: host_not_own". affected_count 5: sqs-queue-create-url-shape#task#2, sqs-queue-lifecycle-basic#task#5, sqs-queue-lifecycle-core#task#2, sqs-queue-lifecycle-crud#task#2, sqs-refarch-s3ev-queue-lookup-mistakes#task#6.

Cause 5 — divergence_key service=sqs|ListQueues|status=200->200|keys=$.QueueUrls[0]; expected 200, actual 200; detail: host_not_own on the single element. affected_count 2: sqs-queue-lifecycle-core#task#5, sqs-refarch-s3ev-queue-lookup-mistakes#task#5.

(The specific queue names, account id and region in the recordings are per-run values and are not part of the requirement; the requirement is that the emitted url's scheme/host/port is the endpoint that answered the request and its path is /<account-id>/<queue-name>.)

Smithy citations (services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json): GetQueueUrl input {QueueName required, QueueOwnerAWSAccountId} -> output {QueueUrl: String}, errors InvalidAddress(httpError 404, awsQueryError InvalidAddress/404), QueueDoesNotExist(httpError 400, awsQueryError AWS.SimpleQueueService.NonExistentQueue/400), InvalidSecurity(403), RequestThrottled(403), UnsupportedOperation(400). ListQueues output {QueueUrls: QueueUrlList (xmlName QueueUrl, xmlFlattened), NextToken}, paginated {inputToken NextToken, outputToken NextToken, items QueueUrls, pageSize MaxResults}. ListDeadLetterSourceQueues input {QueueUrl required, NextToken, MaxResults} -> output {queueUrls: QueueUrlList required (xmlName QueueUrl, xmlFlattened), NextToken}. None of the three constrains the url's form, hence 'recorded behavior only' for the host requirement.

Source pointers: services/service-com-amazonaws-sqs-amazonsqs/src/ops/get_queue_url.rs:208-210 returns queue.url.clone() (the absolute url minted at create time); src/ops/list_queues.rs and src/ops/list_dead_letter_source_queues.rs emit the same stored strings; the minting builder lives in src/state.rs (state::queue_url).

## Steps to Reproduce
Prerequisite for all causes: boot the emulator and reach it on its data listener (not on sqs.<region>.amazonaws.com).

Cause 1: replay fixture sqs-dead-letter-queue-and-message-move, step task#6 (or sqs-dead-letter-queue-configuration#task#6). Equivalently: CreateQueue a DLQ and a source queue, SetQueueAttributes RedrivePolicy on the source pointing at the DLQ's ARN, then ListDeadLetterSourceQueues with the DLQ's QueueUrl — every element of queueUrls must have the scheme/host/port of the endpoint the request was sent to.

Cause 2: replay fixture sqs-list-queues-agent-shape-and-pagination, step task#4. Equivalently: create two queues sharing a prefix, call ListQueues with that QueueNamePrefix, and check each QueueUrls element's host.

Cause 3: replay fixture sqs-list-queues-agent-shape-and-pagination, step task#5 (then #task#6). Equivalently: create three prefixed queues and call ListQueues with MaxResults/NextToken paging; check every element of every page.

Cause 4: replay fixture sqs-queue-create-url-shape, step task#2 (or sqs-queue-lifecycle-core#task#2). Equivalently: CreateQueue a queue, then GetQueueUrl with only its QueueName, and send the returned QueueUrl straight back to GetQueueAttributes over the same connection — it must resolve on the emulator.

Cause 5: replay fixture sqs-queue-lifecycle-core, step task#5 (or sqs-refarch-s3ev-queue-lookup-mistakes#task#5). Equivalently: CreateQueue one queue, call ListQueues, and check QueueUrls[0]'s host.

All five are closed only when the emitted url's host is the endpoint that answered and its path is still /<account-id>/<queue-name>; re-replay all five fixture/step pairs above to confirm.
