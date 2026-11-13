Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iam__sts.asgeni-eni-create-dry-run-unauthorized-for-secrets-only-role
Issue-Key: ec2__iam__sts.asgeni-eni-create-dry-run-unauthorized-for-secrets-only-role|_poll#12|Timeout|time.sleep(interval_s)
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# EC2 DryRun authorization never refuses an unauthorized principal: the ENI create dry-run check never converges and hits the 60 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateNetworkInterface

## Description
ONE root cause (cause 1 of 1).

AWS: an EC2 request with DryRun=true is authorized but not executed. For a principal that holds the action it answers the client error DryRunOperation, HTTP 412; for a principal that does NOT hold it (here a secrets-only role lacking ec2:CreateNetworkInterface) it answers UnauthorizedOperation, HTTP 403, carrying the encoded authorization failure message. The dry-run pair is the documented way to probe permissions without side effects.
Emulator: neither verdict is produced for the unauthorized principal — the check establishes its controls and then polls for the UnauthorizedOperation refusal, which never arrives, so it runs to the 60 s harness cap with no divergent response captured. EC2 performs no IAM authorization on the dry-run path.
Why: permission pre-flighting is exactly what DryRun is for; a tool that uses it to decide whether an operation is permitted gets the wrong answer, and the 403/UnauthorizedOperation branch clients must handle never runs.
Fix: the EC2 DryRun handling in the CreateNetworkInterface handler (and the shared dry-run path): authorize the caller first, answering UnauthorizedOperation/403 when the action is not permitted and DryRunOperation/412 when it is. Related but NOT covering this: open issue 20261023T102725Z-ec2-skips-input-validation-and-accepts-requests-aws.md, which is about input validation, not authorization.
Traces: ec2__iam__sts.asgeni-eni-create-dry-run-unauthorized-for-secrets-only-role.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1, check ec2__iam__sts.asgeni-eni-create-dry-run-unauthorized-for-secrets-only-role: "Error: Check timed out after 60.0s".

The check's helpers are a bounded poll plus an error-code extractor whose failure mode is "the call succeeded; real AWS refuses it, so there is no error code" / "the call succeeded; real AWS refuses it, so there is no error" — i.e. it waits for the UnauthorizedOperation refusal that the emulator never produces. No divergent response body was captured.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-replay ec2__iam__sts.asgeni-eni-create-dry-run-unauthorized-for-secrets-only-role.
3. Manual repro: assume a role whose only grant is secretsmanager:*, then call ec2.create_network_interface(SubnetId=..., DryRun=True) with those credentials — the emulator does not answer UnauthorizedOperation/403 (AWS does), and an authorized principal should get DryRunOperation/412.
