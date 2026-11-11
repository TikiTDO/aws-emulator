Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__logs.cfn-logs-log-stream-retained-replacement-keeps-events-then-not-found cloudformation__logs.cfn-logs-metric-filter-in-place-update-delete-first-replacement-ignores-retain-then-gone
Issue-Key: cloudformation__logs.cfn-logs-log-stream-retained-replacement-keeps-events-then-not-found|_assert_replaced_and_retained#26|AssertionError|assert (
Issue-Key: cloudformation__logs.cfn-logs-metric-filter-in-place-update-delete-first-replacement-ignores-retain-then-gone|_assert_filter_replaced#28|AssertionError|assert rows == want, f"Filter replacement events {rows!r}, expected {want!r}"
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# A replacing update of CloudWatch Logs resources is emitted as a bare create-then-delete and ignores UpdateReplacePolicy Retain

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Cause 1 of 1: replacement is not modelled as an update of the logical resource, and the retained old physical resource is deleted.

AWS: a replacing update is reported on the logical resource in this order, for both delete-before-create and create-before-delete types: UPDATE_IN_PROGRESS (ResourceStatusReason none, physical id = the OLD resource); DELETE_IN_PROGRESS with reason 'Requested update requires the replacement of the existing resource; deleting existing resource, then creating a new one.' (old id); DELETE_COMPLETE with reason 'Resource <old-physical-id> delete complete' (old id); CREATE_IN_PROGRESS with reason 'Resource creation Initiated' (new id); CREATE_COMPLETE (new id); UPDATE_COMPLETE (new id). Where the resource carries UpdateReplacePolicy: Retain, the delete pair is omitted entirely and the old log stream / metric filter keeps existing with its events.

Emulator: the events are only CREATE_IN_PROGRESS (empty physical id), CREATE_IN_PROGRESS 'Resource creation Initiated' (new id), CREATE_COMPLETE (new id), DELETE_IN_PROGRESS (no physical id), DELETE_COMPLETE (old id). There is no UPDATE_IN_PROGRESS, no UPDATE_COMPLETE, no replacement reason text, the ordering is create-then-delete rather than delete-then-create, and UpdateReplacePolicy Retain is ignored so the old resource is destroyed.

Why: a client watching stack events cannot tell that a replacement happened or which resource it replaced — it reads as an unrelated create plus an unrelated delete — and the data a template author explicitly asked to keep (the old log stream's events, the old metric filter) is deleted. Automation keyed on the documented reason strings never matches.

Fix: the CloudFormation update engine's replacement path — emit the logical resource's UPDATE_IN_PROGRESS/UPDATE_COMPLETE and the documented DELETE reason strings in the correct order, and honour UpdateReplacePolicy Retain by skipping the cleanup delete.

Traces: the two failing checks in trace_ids. The open issue 20261110T232635Z-cloudformation-ssm-a-replacement-update-reports-create-delete-resource-events.md records this for cloudformation__ssm and does not carry these replay keys.

## Evidence
Expected (cfn-logs-metric-filter-...): [('UPDATE_IN_PROGRESS', None, '<old>'), ('DELETE_IN_PROGRESS', 'Requested update requires the replacement of the existing resource; deleting existing resource, then creating a new one.', '<old>'), ('DELETE_COMPLETE', 'Resource <old> delete complete', '<old>'), ('CREATE_IN_PROGRESS', 'Resource creation Initiated', '<new>'), ('CREATE_COMPLETE', None, '<new>'), ('UPDATE_COMPLETE', None, '<new>')].
Actual: [('CREATE_IN_PROGRESS', None, ''), ('CREATE_IN_PROGRESS', 'Resource creation Initiated', '<new>'), ('CREATE_COMPLETE', None, '<new>'), ('DELETE_IN_PROGRESS', None, None), ('DELETE_COMPLETE', None, '<old>')].
Same shape for the log stream: AssertionError: Stream must announce its replacement, its events were [('CREATE_IN_PROGRESS', ...), ('CREATE_COMPLETE', ...), ('DELETE_IN_PROGRESS', ...), ('DELETE_COMPLETE', None, '<old stream>')].

## Steps to Reproduce
1. CreateStack a template with an AWS::Logs::LogGroup plus an AWS::Logs::LogStream (and separately an AWS::Logs::MetricFilter) carrying UpdateReplacePolicy: Retain and explicit names.
2. UpdateStack changing the name (a replacement trigger).
3. DescribeStackEvents for the logical id: no UPDATE_* events, create-then-delete ordering, and the old resource deleted despite Retain.
