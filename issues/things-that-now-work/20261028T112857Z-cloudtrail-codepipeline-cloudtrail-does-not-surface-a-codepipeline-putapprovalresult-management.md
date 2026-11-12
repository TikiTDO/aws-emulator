Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__codepipeline.a-manual-approval-decision-is-audited-with-the-reviewer-role-not-the-pipeline
Issue-Key: cloudtrail__codepipeline.a-manual-approval-decision-is-audited-with-the-reviewer-role-not-the-pipeline|_await#30|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Commit: aeaf03eb4bcbb7a6c4e9492334e38170d85c4ec6

# CloudTrail does not surface a CodePipeline PutApprovalResult management event, so a manual-approval decision is never auditable (check exhausts its budget)

> _Note: this issue was identified against the code base at commit `aeaf03eb4bcbb7a6c4e9492334e38170d85c4ec6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
LookupEvents

## Description
One root cause. Cause 1 of 1: no CloudTrail management event for CodePipeline's PutApprovalResult is ever available through LookupEvents, so the reviewer behind a manual-approval decision cannot be audited.

AWS: CodePipeline's Smithy model declares the CloudTrail event source codepipeline.amazonaws.com, and PutApprovalResult is a management event CloudTrail records. LookupEvents with LookupAttributes=[{AttributeKey: 'EventName', AttributeValue: 'PutApprovalResult'}] returns the matching Event(s), each with a CloudTrailEvent JSON document whose eventSource is 'codepipeline.amazonaws.com', whose eventName is 'PutApprovalResult', and whose userIdentity names the principal that MADE the decision (the reviewer role that called PutApprovalResult) — not the pipeline's service role — together with the requestParameters identifying the pipeline, stage and action.
Emulator: the check never completes. It drives a pipeline to its approval stage, calls PutApprovalResult as the reviewer, and then polls LookupEvents for a PutApprovalResult record; it stops at the suite's 60 s per-check ceiling ('Error: Check timed out after 60.0s') with no record observed, so neither the event nor its userIdentity attribution can be asserted. Hypothesis (neither the model nor the recording states the mechanism): either no CodePipeline management events are written to the CloudTrail store at all, or the pipeline never reaches the approval stage so the decision is never made; the check cannot distinguish the two because it observes nothing.
Why: the audit trail is the whole point of a manual approval gate. A client gets no record of who released a change — LookupEvents answers as if the decision never happened — so compliance evidence, four-eyes enforcement and after-the-fact investigation are all impossible, and the absence is silent rather than an error.
Fix: CloudTrail management-event recording for the codepipeline crate's mutating operations (PutApprovalResult in particular — the event's eventSource/eventName/requestParameters and a userIdentity taken from the CALLER of PutApprovalResult), and the LookupEvents EventName-attribute lookup that must return it. The approval stage must also be reachable, which the codepipeline execution-plane issues filed in this same sweep cover.
Traces: cloudtrail__codepipeline.a-manual-approval-decision-is-audited-with-the-reviewer-role-not-the-pipeline.

## Evidence
Quoted verbatim from the replay report:
  #### [mismatch] cloudtrail__codepipeline.a-manual-approval-decision-is-audited-with-the-reviewer-role-not-the-pipeline
  Error: Check timed out after 60.0s
The check's own assertion text, had it reached its deadline, is "DATAPLANE_UNIMPLEMENTED: no PutApprovalResult record satisfying <predicate> within <budget>s; expected 1, actual 0 of <n> examined" — i.e. expected exactly one PutApprovalResult CloudTrail record whose userIdentity is the reviewer role, actual none. Per-service pass rate for cloudtrail__codepipeline in this run: 0/1 (0%).

## Steps to Reproduce
Boot the emulator and point botocore at it; cp = client('codepipeline'), ct = client('cloudtrail'), iam = client('iam'), s3 = client('s3').
1. Create an artifact bucket with a source object and a pipeline role; create a pipeline whose stages include a Review stage with a {category Approval, owner AWS, provider Manual, version '1'} action named ManualApproval.
2. Create a separate reviewer role, assume it, and start a pipeline execution.
3. When the approval action reaches InProgress, read its token from cp.get_pipeline_state and call cp.put_approval_result(pipelineName=P, stageName='Review', actionName='ManualApproval', result={'summary':'ok','status':'Approved'}, token=T) AS THE REVIEWER.
4. Poll ct.lookup_events(LookupAttributes=[{'AttributeKey':'EventName','AttributeValue':'PutApprovalResult'}]) and parse each Events[].CloudTrailEvent -> expect one record with eventSource 'codepipeline.amazonaws.com', eventName 'PutApprovalResult' and userIdentity naming the reviewer role (not the pipeline service role); observe no record at all.
5. Re-replay cloudtrail__codepipeline.a-manual-approval-decision-is-audited-with-the-reviewer-role-not-the-pipeline.
