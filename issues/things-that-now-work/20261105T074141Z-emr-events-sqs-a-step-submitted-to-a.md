Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr__events__sqs.emr-step-status-change-delivers-failed-and-withholds-it-from-the-completed-rule
Issue-Key: emr__events__sqs.emr-step-status-change-delivers-failed-and-withholds-it-from-the-completed-rule|_poll#23|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: 33039e6d53cbc4640ae4d958d554409d2dbcb976

# emr__events__sqs: a step submitted to a WAITING EMR cluster never leaves PENDING, so no step reaches COMPLETED/FAILED and no 'EMR Step Status Change' event is ever published

> _Note: this issue was identified against the code base at commit `33039e6d53cbc4640ae4d958d554409d2dbcb976`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddJobFlowSteps

## Description
1 net-new root cause, on a check the replay report did NOT mark ALREADY FILED or OWNED. Note the open issue 20261029T184212Z-emr-clusters-accept-steps-but-never-execute-them.md describes the same inert step scheduler from the `emr` and `emr__glue__s3` groups; it does not carry this check id or this cross-service EventBridge scenario, so this report grounds the defect in the emr__events__sqs replay key. The companion open issue 20261104T080454Z-emr-events-sqs-ec2-createvpcendpoint-refuses-any-request-carrying-routetableids-with.md described an EARLIER blocker for this same check (CreateVpcEndpoint 501); in this replay the setup, the cluster provisioning and AddJobFlowSteps all succeed, and the check now dies deeper, on the step state machine.

--- Cause 1 of 1: a submitted step stays PENDING forever, so no terminal StepState and no step status-change event ever exist ---
AWS: Smithy: StepState = PENDING | CANCEL_PENDING | RUNNING | COMPLETED | CANCELLED | FAILED | INTERRUPTED, and Step.Status (StepStatus) carries State, StateChangeReason, FailureDetails and Timeline (StepTimeline: CreationDateTime, StartDateTime, EndDateTime). AddJobFlowSteps's own doc states 'Amazon EMR executes each step in the order listed' and 'For a step to be considered complete, the main function must exit with a zero exit code', so a `command-runner.jar` step that exits 0 must reach COMPLETED and one that exits non-zero must reach FAILED, each with a populated StepTimeline. Recorded behavior only for the event: each transition publishes an EventBridge event with source `aws.emr`, detail-type `EMR Step Status Change` and detail.stepId/detail.state, which a rule matching that pattern delivers to its SQS target, while a rule patterned on the other state receives nothing.
Emulator: DescribeStep answers, but the state never changes. The cluster does reach WAITING and AddJobFlowSteps returns two step ids, then DescribeStep returns the same non-terminal state for both steps until the check's 600 s poll is cut off by the run budget — `TimeoutError: both steps on j-0000000000001 reach terminal states: the run budget ended first; not graded`. The crate states this directly: services/service-com-amazonaws-emr-elasticmapreduce/src/state.rs, StepRecord::from_config — 'The step is PENDING and stays there: this emulator runs no Hadoop jar, so it reports no start, no end and no outcome for work nothing performed' (state: step_state::PENDING, start_date_time: None, end_date_time: None), and StepRecord::advance_to only materialises CANCEL_PENDING -> CANCELLED, never PENDING -> RUNNING -> COMPLETED/FAILED. With no transition there is also no `EMR Step Status Change` publication, so the positive-control queue stays empty and the absence leg of the check is ungradeable.
Why: the ordinary EMR usage — submit a step, wait for it to settle, branch on COMPLETED vs FAILED — blocks forever against the emulator, and every event-driven consumer (an EventBridge rule routing failed steps to a queue, an alarm, a reaper) sees nothing while the control plane reports a healthy WAITING cluster with accepted steps. A client cannot tell 'still running' from 'never scheduled'.
Fix: the EMR crate's step state machine (services/service-com-amazonaws-emr-elasticmapreduce/src/state.rs — StepRecord::from_config / StepRecord::advance_to, driving the projections behind src/ops/describe_step.rs and src/ops/list_steps.rs, fed by src/ops/add_job_flow_steps.rs): drive each submitted step PENDING -> RUNNING -> COMPLETED or FAILED from its command's exit status, populate StepTimeline's StartDateTime/EndDateTime (and FailureDetails on a failure), and publish an `aws.emr` / `EMR Step Status Change` event carrying stepId, clusterId, name and state to the EventBridge service state on each transition.
Traces: emr__events__sqs.emr-step-status-change-delivers-failed-and-withholds-it-from-the-completed-rule.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator; per-service pass rate emr__events__sqs 0/1 (0%).

[error] emr__events__sqs.emr-step-status-change-delivers-failed-and-withholds-it-from-the-completed-rule
  File check_emr__events__sqs_emr_step_status_change_delivers_failed_and_withholds_it_from_the_completed_rule.py, line 446, in run
    states = _poll(...)
  File ... line 91, in _poll
    raise TimeoutError(f"{description}: the run budget ended first; not graded")
  TimeoutError: both steps on j-0000000000001 reach terminal states: the run budget ended first; not graded

The earlier assertions in the same check PASSED, so the emulator got the setup right: DescribeCluster reached 'WAITING' and AddJobFlowSteps returned two step ids for the two submitted command-runner.jar steps (one `exit 0`, one `exit 7`). The poll that never completes is:
  {sid: DescribeStep(ClusterId=<cluster>, StepId=sid)['Step']['Status']['State'] for sid in step_ids}
  predicate: all states in {COMPLETED, FAILED, CANCELLED, INTERRUPTED}; timeout_s=600
Expected: the `exit 0` step COMPLETED and the `exit 7` step FAILED, then one `EMR Step Status Change` event per step delivered to the rule matching its own state (source 'aws.emr', detail.stepId = that step, detail.state = COMPLETED / FAILED) and nothing delivered to the other state's rule.
Actual: both steps keep the non-terminal state AddJobFlowSteps left them in and nothing is published; the poll is cut off by the run budget. Source confirmation: services/service-com-amazonaws-emr-elasticmapreduce/src/state.rs StepRecord::from_config sets state = step_state::PENDING with start_date_time/end_date_time None and documents that it 'stays there'; StepRecord::advance_to returns immediately unless the state is CANCEL_PENDING. (The cluster id and step ids above are this run's generated values, not part of the requirement.)

## Steps to Reproduce
Boot the emulator and replay emr__events__sqs.emr-step-status-change-delivers-failed-and-withholds-it-from-the-completed-rule from suite simparity-behavioral-aws-v1 (report heading emr__events__sqs):
1. Create the EMR service role and the EC2 instance profile in IAM (trusting elasticmapreduce.amazonaws.com and ec2.amazonaws.com), two SQS queues and two EventBridge rules on detail-type 'EMR Step Status Change', one patterned detail.state=['COMPLETED'] and one detail.state=['FAILED'], each targeting its own queue.
2. RunJobFlow with ReleaseLabel emr-6.15.0 and m5.xlarge instances; poll DescribeCluster until Status.State == 'WAITING'.
3. AddJobFlowSteps with two command-runner.jar steps, one whose command exits 0 and one whose command exits 7.
4. Poll DescribeStep for both returned step ids until each Status.State is terminal. Expect the first COMPLETED and the second FAILED, each with a populated Status.Timeline (CreationDateTime, StartDateTime, EndDateTime). Against the emulator both stay non-terminal and the poll never satisfies its predicate.
5. Then ReceiveMessage on each queue: expect exactly one aws.emr / 'EMR Step Status Change' event per step on the rule matching that step's state, and nothing on the other rule's queue. Against the emulator both queues stay empty.
