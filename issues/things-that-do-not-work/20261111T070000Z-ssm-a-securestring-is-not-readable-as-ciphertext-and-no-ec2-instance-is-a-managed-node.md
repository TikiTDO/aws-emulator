Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__ssm.sdk-ssm-encrypted-parameter-used-in-ec2-launch-positive
Issue-Key: cloudformation__ec2__ssm.sdk-ssm-encrypted-parameter-used-in-ec2-launch-positive|run#9|AssertionError|assert ciphertext["Value"] != secret_value, "SecureString value was not encrypted at rest"
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# ssm: a SecureString read without WithDecryption must report ciphertext, and an EC2 instance the account launched must become a managed node

> _Note: this cause was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

> _Provenance: this was cause 4 of 5 of `20261022T235108Z-cloudformation-stack-outputs-carry-no-outputvalue-failing-resources.md`. Cause 3 of that issue (a CFN-created resource's physical id must be the identifier the owning service issued) is fixed and that report now sits under `issues/things-that-now-work/`; this cause is unfixed and is re-filed here so its check is not recorded as working. Its `Issue-Key` and its `Issue-Trace-Ids` moved with it._

## Operation
PutParameter, GetParameter, DescribeInstanceInformation

## Description
AWS: `PutParameter` with `Type=SecureString` stores the value encrypted under a KMS key; `GetParameter` without `WithDecryption` returns the ciphertext, so the plaintext is never recoverable from the stored form.

Emulator: the sealing path already exists — `services/service-com-amazonaws-ssm-amazonssm/src/state/secure_string.rs` seals a `SecureString` under the parameter's KMS key and `GetParameter` opens it only for a `WithDecryption` read (`value_to_open`) — so re-read that path first and decide whether anything is still missing in it. What the check now diverges on is a step it reaches LATER: it waits for the EC2 instance it launched to be reported `Online` by Systems Manager and times out at the suite's 60 s cap.

```
Error: Check timed out after 60.0s
... _wait_ssm_online#14|Timeout|time.sleep(interval_s)
```

That wait is a capability no component has: `DescribeInstanceInformation` reports the managed nodes this service's own store holds (`crate::state::ManagedNodeRecord`), and nothing registers an EC2 instance as one — so an instance a client launched in Amazon EC2 is never a managed node, however it is configured. The same wait is the second divergence of `cloudformation__ec2__ssm.deploy-ec2-3-group-rename-replaces-the-group-and-source-group-rule-still-drops-an-unrelated-instance` (carried by `20261111T070000Z-cloudformation-three-stacks-report-an-output-with-no-outputvalue.md`), so ONE capability closes both: the managed-node inventory Systems Manager reports has to follow the instances Amazon EC2 holds — that service is the component that OWNS them, so its own read is what a registration must be backed by, and a node reported for an instance nothing launched would be a resource no engine produced.

Why: a client relying on `SecureString` for secret material gets no confidentiality boundary it can observe end to end, and nothing that drives a node through Systems Manager (an association, a command, an inventory read) can be exercised at all.

## Evidence
```
AssertionError: SecureString value was not encrypted at rest
```
(the original divergence, against the commit above)

```
Error: Check timed out after 60.0s
```
(now, waiting for the launched instance to come Online in Systems Manager)

## Steps to Reproduce
1. `ssm:PutParameter` with `Type=SecureString`, then `ssm:GetParameter` without `WithDecryption` and compare the reported `Value` with the plaintext.
2. Launch an EC2 instance (directly or through a stack) and poll `ssm:DescribeInstanceInformation` for it: it is never reported, so no `PingStatus=Online` is ever observed.
