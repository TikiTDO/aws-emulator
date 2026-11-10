Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sqs.async-event-that-fails-once-then-succeeds-on-retry-is-recorded-on-success-only lambda__sqs.async-retry-budget-of-one-runs-the-handler-twice-and-reports-two-invokes lambda__sqs.dotnet-sqs-batch-item-failures-deletes-only-successes lambda__sqs.go-handler-sqs-batch-item-failures-deletes-only-the-successes lambda__sqs.java-sqs-esm-partial-batch-response-deletes-only-the-successful-records
Issue-Key: lambda__sqs.async-event-that-fails-once-then-succeeds-on-retry-is-recorded-on-success-only|_await_messages#14|AssertionError|raise AssertionError(f"{description}: {len(got)} of {count} message(s) arrived within "
Issue-Key: lambda__sqs.async-retry-budget-of-one-runs-the-handler-twice-and-reports-two-invokes|_await_messages#10|AssertionError|raise AssertionError(f"{description}: {len(got)} of {count} message(s) arrived within "
Issue-Key: lambda__sqs.dotnet-sqs-batch-item-failures-deletes-only-successes|_poll#12|AssertionError|raise AssertionError(f"timed out after {timeout_s}s waiting until {what}; last={last!r}")
Issue-Key: lambda__sqs.go-handler-sqs-batch-item-failures-deletes-only-the-successes|_poll#16|AssertionError|raise AssertionError(f"timed out after {timeout_s}s waiting until {what}; last={last!r}")
Issue-Key: lambda__sqs.java-sqs-esm-partial-batch-response-deletes-only-the-successful-records|run#20|AssertionError|assert dead == [_POISON], (
Issue-Commit: cd6d73564e5f4cf53934bca1327f08de31f2ee5f

# lambda__sqs: 2 net-new causes — asynchronous invocations are never retried (so no OnSuccess/OnFailure record follows a retry), and an SQS event-source mapping ignores the handler's ReportBatchItemFailures partial-batch response

> _Note: this issue was identified against the code base at commit `cd6d73564e5f4cf53934bca1327f08de31f2ee5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Two net-new root causes of service `lambda__sqs`, each grounded in its own failing behavioral checks. A fixer must close both before submitting.

Cause 1 of 2 — an asynchronous invocation that fails is not retried, so neither destination record that depends on a retry is ever written.
AWS: Smithy (lambda): `PutFunctionEventInvokeConfig` carries `MaximumRetryAttempts` and `DestinationConfig` with `OnSuccess` and `OnFailure`. Recorded behavior: `MaximumRetryAttempts: 1` means one initial attempt plus one retry, about a minute apart. A handler that fails on its first attempt and succeeds on the retry runs twice and produces exactly ONE record — on the OnSuccess destination, reporting the successful invocation (and no OnFailure record). A handler that fails on both attempts produces exactly one OnFailure record whose invocation record reports two attempts.
Emulator: neither record arrives. The retried-then-succeeded event produces no OnSuccess record within 240 s (`the OnSuccess record of the retried event: 0 of 1 message(s) arrived within 240s`), and the exhausted-budget event produces no OnFailure record within 240 s (`the OnFailure record: 0 of 1 message(s) arrived within 240s`) — the async retry never happens, so no destination record follows it.
Why: a client relying on async retries to absorb transient failures loses the event, and the destination records it uses to audit outcomes never exist, so success and permanent failure are indistinguishable. Open issue 20261107T014900Z-lambda-sqs-the-asynchronous-invocation-plane-is-inert.md covers other lambda__sqs async checks and does not carry these two checks' replay keys.
Fix: the Lambda async-invoke plane's retry scheduler and its destination writer (the component that must re-run the handler per MaximumRetryAttempts and then write the OnSuccess/OnFailure invocation record).
Traces: lambda__sqs.async-event-that-fails-once-then-succeeds-on-retry-is-recorded-on-success-only, lambda__sqs.async-retry-budget-of-one-runs-the-handler-twice-and-reports-two-invokes

Cause 2 of 2 — an SQS event-source mapping with FunctionResponseTypes ReportBatchItemFailures ignores the handler's batchItemFailures, so the batch is treated as all-failed or all-succeeded instead of partially.
AWS: Smithy (lambda): `FunctionResponseType` has the single value `ReportBatchItemFailures`, accepted on CreateEventSourceMapping/UpdateEventSourceMapping. Recorded behavior: when the mapping declares it and the handler returns `{"batchItemFailures": [{"itemIdentifier": "<messageId>"}]}`, Lambda DELETES every record NOT named in that list and returns only the named record(s) to the queue — so the source queue settles at exactly the number of failed records, and only the failed record is eventually redriven to the DLQ at maxReceiveCount.
Emulator: the partial response is not applied. With one poison record in a batch, the source queue never settles at one remaining message (.NET handler: depth stayed at 2; Go handler: 3 records still outstanding after 150 s), and when the redrive policy fires the DLQ receives the WHOLE batch instead of just the poison record (Java handler: `the redrive target must receive exactly the poison body, got ['ok-0', 'ok-1', 'ok-2', 'ok-3', 'poison']`).
Why: partial-batch handling is the documented way to avoid reprocessing a whole batch for one bad record — successfully processed records are redelivered (duplicate side effects) and healthy records are dead-lettered alongside the poison one, so a consumer cannot isolate the failing message.
Fix: the SQS event-source-mapping poller's post-invocation bookkeeping in the lambda crate — it must parse the handler's batchItemFailures when FunctionResponseTypes includes ReportBatchItemFailures and delete exactly the un-named records.
Traces: lambda__sqs.dotnet-sqs-batch-item-failures-deletes-only-successes, lambda__sqs.go-handler-sqs-batch-item-failures-deletes-only-the-successes, lambda__sqs.java-sqs-esm-partial-batch-response-deletes-only-the-successful-records

## Evidence
Cause 1 — lambda__sqs.async-event-that-fails-once-then-succeeds-on-retry-is-recorded-on-success-only:
  AssertionError: the OnSuccess record of the retried event: 0 of 1 message(s) arrived within 240s
Cause 1 — lambda__sqs.async-retry-budget-of-one-runs-the-handler-twice-and-reports-two-invokes:
  AssertionError: the OnFailure record: 0 of 1 message(s) arrived within 240s

Cause 2 — lambda__sqs.dotnet-sqs-batch-item-failures-deletes-only-successes:
  AssertionError: timed out after 90s waiting until queue depth to settle at 1 remaining message; last=2
Cause 2 — lambda__sqs.go-handler-sqs-batch-item-failures-deletes-only-the-successes:
  AssertionError: timed out after 150s waiting until only the FAIL_MARKER message remains outstanding; last=3
Cause 2 — lambda__sqs.java-sqs-esm-partial-batch-response-deletes-only-the-successful-records:
  AssertionError: the redrive target must receive exactly the poison body, got ['ok-0', 'ok-1', 'ok-2', 'ok-3', 'poison']
(The body labels are this run's payloads; the requirement is that only the record named in batchItemFailures survives and is redriven.)

## Steps to Reproduce
1. Boot the emulator and replay the five checks named in trace_ids.
2. Cause 1a: create a function that fails on its first attempt and succeeds afterwards, PutFunctionEventInvokeConfig with MaximumRetryAttempts=1 and both OnSuccess and OnFailure SQS destinations, then Invoke with InvocationType='Event'. Expect exactly one OnSuccess record and no OnFailure record.
3. Cause 1b: same configuration with an always-failing handler. Expect exactly one OnFailure record reporting two attempts.
4. Cause 2: create a function (any runtime) that returns {"batchItemFailures": [{"itemIdentifier": <poison messageId>}]}, CreateEventSourceMapping with FunctionResponseTypes=['ReportBatchItemFailures'] on a queue with a redrive policy, and send 4 good messages plus 1 poison message.
5. Expect the source queue to settle with exactly the poison message outstanding, and the DLQ to receive only the poison body.
