Issue-Source: trace-sweep-divergence
Issue-Trace: sfn-validation-reason-labeled-arn-and-completed-map-run
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sfn-validation-reason-labeled-arn-and-completed-map-run
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: states|DescribeMapRun|2xx->2xx|$.status
Issue-Commit: 342b40951f6ce3f2662763d4a3c08d3bb913fe47

# DescribeMapRun reports a completed Map Run as FAILED instead of SUCCEEDED

> _Note: this issue was identified against the code base at commit `342b40951f6ce3f2662763d4a3c08d3bb913fe47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeMapRun

## Description
A Map Run whose child executions all completed is described with the terminal status FAILED where AWS reports SUCCEEDED. 1 cause.

Cause 1 of 1: the Map Run's terminal status is inverted.
AWS: Smithy: DescribeMapRun.output.status is required and targets MapRunStatus, an enum whose values are exactly [RUNNING, SUCCEEDED, FAILED, ABORTED]. A Map Run that finishes with no item failure in excess of its tolerated failure count/percentage (DescribeMapRun also returns toleratedFailureCount/toleratedFailurePercentage, itemCounts and executionCounts) is terminal SUCCEEDED; FAILED is the status for a run whose failures exceeded that tolerance. In the recorded run the Map state's child executions complete and AWS answers 200 with status SUCCEEDED.
Emulator: the same DescribeMapRun call answers 200 with status FAILED. Hypothesis: the Map Run inherits the terminal status of the parent execution record, which the emulator also leaves FAILED on this fixture, rather than deriving its own status from its item/execution counts against the tolerated-failure settings.
Why: this inverts the outcome of a Distributed Map fan-out. A client (or the console) polling DescribeMapRun for SUCCEEDED before consuming the fan-out's results never proceeds, or takes its failure path and re-runs work that in fact completed; a redrive tool offers to redrive a Map Run AWS considers finished successfully.
Fix: the Map Run state record's terminal-status derivation in the states execution interpreter — DescribeMapRun's status must be computed from the Map Run's own item/execution counts and its tolerated-failure configuration, not copied from the parent execution's status.
Traces: sfn-validation-reason-labeled-arn-and-completed-map-run#task#7

Note on already-filed issues: the open 20261029T171911Z-states-step-functions-response-construction-map-run-arns-carry.md owns the Map Run ARN/state-machine-ARN construction asserted on ListMapRuns in the same fixture, and 20261103T052008Z-states-execution-outcome-error-name-and-cause-text-diverge.md owns the parent execution's FAILED-vs-SUCCEEDED status on DescribeExecution. Neither carries the replay key for this DescribeMapRun cluster, which the replay reports with no owner.

## Evidence
Mismatch cluster from the replay of simparity-wire-aws-v1 against the emulator built from this checkout.

sfn-validation-reason-labeled-arn-and-completed-map-run#task#7, DescribeMapRun, expected HTTP 200 -> got HTTP 200:
  $.status: expected 'SUCCEEDED', got 'FAILED'

Both values are members of the modelled MapRunStatus enum [RUNNING, SUCCEEDED, FAILED, ABORTED], so this is a wrong value rather than an unmodelled one. No other member of the response diverged on this step (the mapRunArn/stateMachineArn construction diverges on the same fixture's ListMapRuns step task#6, which an open issue already owns).

## Steps to Reproduce
1. Build the emulator from this checkout and replay the wire corpus simparity-wire-aws-v1.
2. Fixture sfn-validation-reason-labeled-arn-and-completed-map-run: it creates a state machine with a DISTRIBUTED-mode Map state, starts an execution over its items and waits for the Map Run to reach a terminal state.
3. At step task#7 the fixture calls DescribeMapRun with the Map Run ARN. Compare $.status: the recording carries 'SUCCEEDED'; the emulator answers 'FAILED'.
