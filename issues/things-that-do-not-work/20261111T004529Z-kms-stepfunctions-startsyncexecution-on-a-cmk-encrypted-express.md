Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__stepfunctions.xsvc-sfn-express-cmk-start-sync-execution-key-policy-deny-and-disabled-key
Issue-Key: kms__stepfunctions.xsvc-sfn-express-cmk-start-sync-execution-key-policy-deny-and-disabled-key|_until_refused#15|Timeout|time.sleep(3.0)
Issue-Commit: f28dfafa5fc9e548e0b0df8415a880c6df425385

# kms__stepfunctions: StartSyncExecution on a CMK-encrypted EXPRESS machine stays refused for a caller the key policy allows — the key-policy-deny check never reaches its positive control

> _Note: this issue was identified against the code base at commit `f28dfafa5fc9e548e0b0df8415a880c6df425385`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartSyncExecution

## Description
Cause 1 of 1: a caller the CMK key policy explicitly grants kms:Decrypt/kms:DescribeKey cannot run StartSyncExecution against the CMK-encrypted EXPRESS machine, so the check's positive control never passes and the run is cut off at its budget.
AWS: recorded behavior only for the timing; the semantics are Step Functions' documented CMK handling. With a key policy whose statements are {root: kms:* } plus {Principal: <caller role ARN>, Action: [kms:Decrypt, kms:DescribeKey], Resource: *}, StartSyncExecution(includedData=ALL_DATA) by that caller SUCCEEDS and the response's `input` is the posted input verbatim. KmsAccessDeniedException is reserved for a caller whose decrypt grant is actually missing or explicitly denied; it is not the steady state of a granted caller. The recording reaches that positive control in seconds and then goes on to assert that an explicit key-policy Deny yields KmsAccessDeniedException with wording distinct from a plain missing grant, and that disabling the key yields KmsInvalidStateException, both recovering afterwards.
Emulator: the check is killed by its budget — `Check timed out after 60.0s` — before the first assertion can be graded. Its StartSyncExecution helper retries ONLY while the response is AccessDeniedException or KmsAccessDeniedException (the propagation window), so a run that spends the whole budget in that loop means the emulator kept refusing the granted caller with one of those two codes. Hypothesis: the key policy's Allow for a non-root principal is not consulted when Step Functions decrypts execution data for StartSyncExecution, so the granted caller is refused indefinitely. The error code and message of the refusal are whatever the emulator emits in that loop and are not surfaced in the replay output.
Why: the CMK path is unusable from a least-privilege caller: a client that grants exactly kms:Decrypt/kms:DescribeKey on its key, as AWS documents, is still refused, and because the refusal uses the same codes AWS uses for a genuinely missing grant, the client is sent to re-check a policy that is already correct. Downstream, no distinction between key-policy Deny (KmsAccessDeniedException) and a disabled key (KmsInvalidStateException) can be observed at all.
Fix: the Step Functions StartSyncExecution handler's KMS authorization for CMK-encrypted machines — key-policy evaluation for the calling principal (not just the execution role / key owner) and the classification of the resulting refusal. The sibling check xsvc-sfn-express-cmk-start-sync-execution-caller-decrypt-required-unless-metadata-only is owned by 20261109T164957Z-kms-stepfunctions-5-net-new-causes-cmk-key.md; that issue does not carry this check's key, so this one is filed separately.
Traces: kms__stepfunctions.xsvc-sfn-express-cmk-start-sync-execution-key-policy-deny-and-disabled-key.

## Evidence
Check kms__stepfunctions.xsvc-sfn-express-cmk-start-sync-execution-key-policy-deny-and-disabled-key, verdict mismatch:

Error: Check timed out after 60.0s

Expected: with the key policy allowing {root: kms:*} and {<caller role ARN>: [kms:Decrypt, kms:DescribeKey]}, the positive control `StartSyncExecution(stateMachineArn=<cmk express machine>, input={"marker": <marker>}, includedData="ALL_DATA")` as the caller returns status SUCCEEDED with input == the posted body; then an explicit key-policy Deny yields KmsAccessDeniedException (wording distinct from a plain missing grant) and a disabled key yields KmsInvalidStateException, both recovering.
Actual: the check never gets past its positive control within the 60 s budget. StartSyncExecution is retried only while it answers AccessDeniedException / KmsAccessDeniedException, so the granted caller was still being refused with one of those codes when the budget expired; no assertion was graded.

## Steps to Reproduce
1. Create a CMK and a Step Functions execution role granting its use; create an EXPRESS state machine with encryptionConfiguration type CUSTOMER_MANAGED_KMS_KEY on that key.
2. Create a caller role and put a key policy on the CMK with {Sid: root, Principal: arn:aws:iam::<acct>:root, Action: kms:*, Resource: *} and {Sid: allowcaller, Principal: <caller role ARN>, Action: [kms:Decrypt, kms:DescribeKey], Resource: *}.
3. Assume the caller role and call StartSyncExecution(stateMachineArn=<machine>, input='{"marker":"..."}', includedData='ALL_DATA').
4. AWS returns SUCCEEDED with the input echoed; the emulator keeps refusing with AccessDeniedException / KmsAccessDeniedException, so a bounded retry loop never completes (here, the 60 s check budget expires).
