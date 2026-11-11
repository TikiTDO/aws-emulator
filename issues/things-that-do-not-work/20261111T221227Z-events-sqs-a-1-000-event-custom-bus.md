Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__sqs.custom-bus-fanout-is-at-least-once-and-order-independent
Issue-Key: events__sqs.custom-bus-fanout-is-at-least-once-and-order-independent|run#16|Timeout|with ThreadPoolExecutor(max_workers=12) as pool:
Issue-Commit: 83a487fe0db4ce5c414ce564b50d725b494461fa

# events__sqs: a 1,000-event custom-bus fan-out across four pattern rules never reaches a verdict — the check times out with no delivery convergence

> _Note: this issue was identified against the code base at commit `83a487fe0db4ce5c414ce564b50d725b494461fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
One net-new root cause of service `events__sqs`, grounded in the one failing behavioral check in that section the replay report left unmarked (no ALREADY-FILED marker, no OWNED marker).

Cause 1 of 1 — the EventBridge custom-bus fan-out path does not converge for a bulk workload: the check that publishes 1,000 events to a custom bus with four rules (one SQS queue each) never reaches its verdict and is killed by the suite's per-check budget.

AWS: recorded behavior only — the model constrains the shapes, not throughput or the matcher set. PutEvents on a custom bus accepts entries in batches of up to 10 and answers FailedEntryCount 0 with an EventId per accepted entry; every ENABLED rule on that bus whose EventPattern matches an accepted event delivers the event to each of its targets at least once (never fewer than once), independent of order. The four patterns the recording exercises are `source` + `detail.tenant` [{prefix}], `source` + `detail.n` [{numeric [">=", 500]}], `source` + `detail-type`, and `source` + `detail.tenant` [{anything-but}]; the prefix and anything-but patterns are complementary, so over the same 1,000 events they must partition exactly what the match-everything rule receives (1,000 events split 50 / 950, with the numeric rule taking 500). Each delivery arrives on the target queue as the EventBridge envelope whose `detail` is the published detail verbatim.

Emulator: the check never produces a verdict. Its only observable is `Check timed out after 60.0s` — the run is cut off while still inside the publish/warm-up/drain phase, so no per-rule count is ever reported. The check's own internal waits are an observable warm-up (it publishes throwaway events and polls each of the four queues until each has proven it can receive one) and a drain that gives up after 8 consecutive empty receives per worker; both are bounded well above the 60 s the run was allowed, so the emulator produced no convergence inside that window. Hypothesis (neither the model nor the recording states which): either the warm-up never confirms all four queues — the most likely candidate being a rule whose matcher is not evaluated at all (`anything-but` or the `numeric` range), so its queue never receives anything and the warm-up loop spins — or the custom-bus delivery path is too slow to drain 2,500 deliveries, i.e. the bus serializes per-event fan-out.

Why: a client that publishes a batch of events to a custom bus and reads its subscriber queues gets either nothing on some rule or deliveries so late they look lost. Fan-out at even modest volume is the core use of a custom bus; sibling checks in this service pass at single-event volume, so the defect is only visible in bulk and silently breaks anything that measures completeness of a fan-out.

Fix: the eventbridge/events service's custom-bus delivery engine — rule matching for the `anything-but` and `numeric` pattern forms and the per-rule target delivery loop (and its throughput: deliveries must be dispatched without a per-event serialization bottleneck).

Traces: events__sqs.custom-bus-fanout-is-at-least-once-and-order-independent

## Evidence
Check events__sqs.custom-bus-fanout-is-at-least-once-and-order-independent — verdict mismatch.

Error: Check timed out after 60.0s

No assertion was reached, so the emulator reported no per-rule counts. What the check would have asserted, had it converged: for each of the four rules, no expected event missing, no foreign event delivered, total >= expected; the match-everything rule receives all 1,000 published ids; and the prefix / anything-but rules' id sets are disjoint and together equal exactly what the match-everything rule received (expected sizes 50 / 500 / 1000 / 950). The counts are properties of the published workload, not values from one run; the bus name, queue urls and event ids are per-run values.

Note on attribution: the timeout is the suite's per-check budget, and the check's own waits (a bounded warm-up that polls each of the four queues for a first arrival, then a drain that stops after 8 consecutive empty receives) are longer than that budget — so the precise stall point is not in the output. It is recorded here as it was observed rather than guessed at; a fixer should re-replay this check id and read where it stalls before changing anything.

## Steps to Reproduce
1. Boot the emulator and replay events__sqs.custom-bus-fanout-is-at-least-once-and-order-independent.
2. events:CreateEventBus with a fresh name. Create four SQS queues and give each a policy allowing events.amazonaws.com sqs:SendMessage with aws:SourceArn ArnLike arn:aws:events:<region>:<account>:rule/<bus>/*.
3. events:PutRule four ENABLED rules on that bus, one per pattern: {"source":[S],"detail":{"tenant":[{"prefix":"t-000"}]}}, {"source":[S],"detail":{"n":[{"numeric":[">=",500]}]}}, {"source":[S],"detail-type":[T]}, {"source":[S],"detail":{"tenant":[{"anything-but":["t-000"]}]}}; events:PutTargets one queue per rule (FailedEntryCount must be 0).
4. Publish two throwaway events (tenant t-000 and t-001) and poll all four queues until each has received at least one message — this is the warm-up; observe whether every one of the four confirms. Then drain each queue to empty.
5. events:PutEvents 1,000 entries in batches of 10 on that bus, Source S / DetailType T / Detail {"tenant":"t-%03d" (i mod 20),"n":i,"id":...}; every batch must answer FailedEntryCount 0 with an EventId per entry.
6. Drain all four queues and count distinct published ids per rule. Expect 50 / 500 / 1000 / 950, no foreign ids, and the prefix and anything-but sets disjoint and covering the match-everything set. Observed: the run does not converge — it is still publishing/warming/draining when the budget ends.
