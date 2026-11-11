Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__stepfunctions.lambda-invoke-retry-full-jitter-caps-scheduled-gaps
Issue-Key: lambda__stepfunctions.lambda-invoke-retry-full-jitter-caps-scheduled-gaps|_run_to_terminal#9|Timeout|return last
Issue-Commit: f4372f081be041f1c06808452ec2531466fafcb7

# A Step Functions Lambda task with a full-jitter Retry never completes its retry ladder

> _Note: this issue was identified against the code base at commit `f4372f081be041f1c06808452ec2531466fafcb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Cause 1 of 1: a Task state whose Retry uses JitterStrategy FULL never produces an observable set of retry gaps, and the execution never reaches a terminal state within the step budget.
AWS: recorded behavior only - a Task Retrier with IntervalSeconds, MaxAttempts, BackoffRate and JitterStrategy=FULL schedules each retry at a random delay in [0, computed backoff], so the gaps between the TaskScheduled history events are each capped by the computed backoff for that attempt, and the execution ends in a terminal state after MaxAttempts.
Emulator: the step never reaches a verdict - the execution's retry ladder never finishes and the check ends `Check timed out after 60.0s`; no scheduled gaps and no terminal status are observable.
Why: a client that relies on bounded, jittered retries cannot verify the delay cap, and an execution that neither succeeds nor fails leaves it with nothing to act on.
Fix: the Retry/backoff scheduler of the Step Functions Task state (its jitter computation and the execution-history events it emits).

Open issue 20261109T210043Z-lambda-stepfunctions-8-net-new-causes-step-functions-ignores-task.md covers neighbouring Task-state causes but carries none of this check's replay key.

Traces: lambda__stepfunctions.lambda-invoke-retry-full-jitter-caps-scheduled-gaps.

## Evidence
Expected: each retry's scheduled gap is capped by the computed backoff and the execution reaches a terminal state.
Actual: Error: Check timed out after 60.0s - the execution never finishes its retry ladder.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. CreateStateMachine with a Task state invoking a failing Lambda and a Retrier carrying IntervalSeconds, BackoffRate, MaxAttempts and JitterStrategy: FULL.
3. StartExecution and poll GetExecutionHistory for the TaskScheduled events and the terminal status.
4. Re-replay lambda__stepfunctions.lambda-invoke-retry-full-jitter-caps-scheduled-gaps.
