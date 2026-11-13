Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__mq.e2e-rabbitmq-esm-absent-queue-delivers-nothing-lifecycle
Issue-Key: lambda__mq.e2e-rabbitmq-esm-absent-queue-delivers-nothing-lifecycle|_poll#4|Timeout|last = probe()
Issue-Commit: 65a54ca45d4af5e17b89290ffb091f639a3756dd

# The Amazon MQ (RabbitMQ) event-source-mapping absent-queue lifecycle never reaches a verdict: the scenario hangs to the check's 60 s cap

> _Note: this issue was identified against the code base at commit `65a54ca45d4af5e17b89290ffb091f639a3756dd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
ONE root cause. Cause 1 of 1: the end-to-end Amazon MQ / RabbitMQ event-source-mapping lifecycle never completes against the emulator — no step of the scenario settles, so the check is killed by the harness at its 60 s budget with no response to compare. Nothing downstream of the hang is observable.

AWS: recorded behavior only (the lambda Smithy model constrains CreateEventSourceMapping's members but says nothing about convergence time). On AWS every step of this lifecycle settles well inside the budget: the broker and the mapping reach their steady states, a mapping pointed at a queue that does not exist on the broker is created and then simply delivers nothing, and the control mapping on an existing queue delivers. Each API call answers and each state converges, so the scenario reaches a verdict.

Emulator: the check reaches no verdict at all. The replay detail is exactly "Error: Check timed out after 60.0s" — the scenario is still waiting when the cap fires, so no step's status or error is observable. Hypothesis: a status the emulator never advances (the broker or the mapping staying in a transient state), or a poll that never yields; the recording does not say which, only that nothing settles.

Why: a client cannot use the emulator to validate an Amazon MQ trigger at all. The intended negative — that a mapping naming a queue absent from the broker is accepted and then delivers nothing — is indistinguishable from "the whole pipeline is inert", so a real mis-wiring would be reported the same way as correct behaviour. A caller waiting for the mapping or broker to settle waits forever rather than getting an answer.

Fix: the Amazon MQ event-source-mapping path in the lambda service crate (the mapping's lifecycle/state convergence and its RabbitMQ poller, not request validation) — and whichever broker state the mapping waits on in the mq service crate — must converge to a terminal state so each step answers. The sibling check lambda__mq.refarch-mq-esm-rejected-when-rabbitmq-broker-does-not-exist is already owned by 20261024T112540Z-lambda-createeventsourcemapping-accepts-an-eventsourcearn-naming-a-nonexistent; that issue covers the nonexistent-BROKER refusal and does not cover this lifecycle never settling.

Traces: lambda__mq.e2e-rabbitmq-esm-absent-queue-delivers-nothing-lifecycle.

## Evidence
Expected (AWS): every step of the lifecycle answers and converges inside the check budget — the broker and the event source mapping reach steady state, the mapping whose queue is absent from the broker delivers no invocation, and the control mapping delivers; the check reaches a pass/fail verdict.

Actual (emulator): no verdict. Replay report detail, verbatim: "Error: Check timed out after 60.0s". No HTTP status, error code or message was produced to compare, because the scenario never got that far.

## Steps to Reproduce
1. Run the lambda__mq.e2e-rabbitmq-esm-absent-queue-delivers-nothing-lifecycle scenario: provision a RabbitMQ broker and its credentials secret, CreateFunction the consumer, then CreateEventSourceMapping with the broker's EventSourceArn and Queues naming a queue that does not exist on the broker, plus a control mapping on a queue that does.
2. Poll the broker/mapping for their steady states and then for deliveries.
3. Observe that the scenario never settles and is cut off at the 60 s cap.
