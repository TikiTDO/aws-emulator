Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__kms.cfn-kms-key-create-update-in-place-multi-region-change-refused-then-delete
Issue-Key: cloudformation__kms.cfn-kms-key-create-update-in-place-multi-region-change-refused-then-delete|run#27|AssertionError|assert _REFUSED.fullmatch(failed[0][2] or ""), (
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# The UPDATE_FAILED reason for a MultiRegion change on AWS::KMS::Key is emulator-authored instead of KMS's replacement behaviour

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Cause 1 of 1: changing MultiRegion on an existing KMS key produces an emulator-worded NotImplemented-style refusal instead of the behaviour KMS/CloudFormation define.

AWS: recorded behavior only. MultiRegion is an immutable property of AWS::KMS::Key: a change to it is answered by replacing the key (CloudFormation creates a new key and, subject to the deletion/update-replace policy, schedules the old one), not by refusing the update with a 'this emulator does not …' sentence. Where the update is refused, the reason is a KMS-worded refusal naming the property, wrapped in CloudFormation's 'Resource handler returned message: "…" (RequestToken: <token>, HandlerErrorCode: <code>)' envelope.

Emulator: the reason is 'Resource handler returned message: "this emulator does not change the MultiRegion property of an existing AWS::KMS::Key, which KMS answers by replacing the key; the resource was left as it is." (RequestToken: <token>, HandlerErrorCode: InvalidRequest)'. The quoted sentence is fixed emulator text that names the emulator itself; its only variable parts are the request token and the resource type. No AWS error code or message has that shape.

Why: a client that keys off the failure reason (CI gates, retry/fix-forward logic, operators reading the console) gets a message AWS never emits, and the replacement KMS actually performs — a new key id with the new MultiRegion setting — never happens, so the key cannot be converted at all.

Fix: the AWS::KMS::Key resource handler's update classification in the CloudFormation provisioner — treat MultiRegion as a replacement trigger (and, where it must refuse, use the KMS-worded refusal rather than emulator-authored text).

Traces: the failing check in trace_ids.

## Evidence
Expected: the Key's UPDATE_FAILED reason is the MultiRegion refusal AWS words (or the update replaces the key).
Actual: AssertionError: the Key's UPDATE_FAILED reason must be the MultiRegion refusal, got 'Resource handler returned message: "this emulator does not change the MultiRegion property of an existing AWS::KMS::Key, which KMS answers by replacing the key; the resource was left as it is." (RequestToken: bd940ae4-c43e-334a-ae5e-776b9be2e5dc, HandlerErrorCode: InvalidRequest)'

## Steps to Reproduce
1. CreateStack a template with an AWS::KMS::Key (MultiRegion: false); wait for CREATE_COMPLETE.
2. UpdateStack setting MultiRegion: true.
3. DescribeStackEvents: the Key's UPDATE_FAILED reason is the emulator-authored sentence above.
