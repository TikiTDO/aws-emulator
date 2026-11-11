Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__ssm.sdk-ssm-parameter-policy-restricts-ec2-cross-account-read-negative
Issue-Key: cloudformation__ec2__ssm.sdk-ssm-parameter-policy-restricts-ec2-cross-account-read-negative|_error#14|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# ssm: GetParameter answers a restricted parameter in full where the scoped read is refused, so no deny boundary on a parameter is observable

> _Note: this cause was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

> _Provenance: this was cause 5 of 5 of `20261022T235108Z-cloudformation-stack-outputs-carry-no-outputvalue-failing-resources.md`. Cause 3 of that issue (a CFN-created resource's physical id must be the identifier the owning service issued) is fixed and that report now sits under `issues/things-that-now-work/`; this cause is unfixed and is re-filed here so its check is not recorded as working. Its `Issue-Key` and its `Issue-Trace-Ids` moved with it._

## Operation
GetParameter

## Description
AWS: the scoped read in this scenario is refused with a `ClientError` (recorded behavior only for the exact code).

Emulator: `GetParameter` answers HTTP 200 with the whole parameter — `{'Parameter': {'Name': '/sim2real/restricted-...', 'Type': 'String', 'Value': 'sim2real-value-...', 'Version': 1, ...}}` — where a refusal was due.

What is already there, for whoever picks this up: `services/service-com-amazonaws-ssm-amazonssm/src/ops/get_parameter.rs` DOES scope a parameter named by ARN to the account and the Region the request was signed for (an ARN naming another account or Region resolves to nothing), and a `WithDecryption` read of a `SecureString` is already decided by the KMS key's own policy (`state::secure_string::open`, which answers `AccessDeniedException`). What this check needs is the step neither of those covers: the identity/resource-policy evaluation that decides a read of a parameter whose policy restricts it — the crate already holds a reading of a policy document for its own purposes in `src/state/identity_authorization.rs`, which is where the decision belongs rather than in a second copy.

Before implementing, establish WHICH refusal AWS answers here (the issue this was split from records only that it is a `ClientError`): a resource policy that does not allow the caller and a parameter in another account are not the same refusal, and answering one error code for another would be a re-worded refusal rather than a fix.

Why: authorization/parameter-policy scoping is not enforced, so a client cannot test or rely on a deny boundary.

## Evidence
```
AssertionError: expected a ClientError, but the call succeeded and returned
{'Parameter': {'Name': '/sim2real/restricted-b16533293ba2', 'Type': 'String',
 'Value': 'sim2real-value-ec448a8a85cc', 'Version': 1, ...},
 'ResponseMetadata': {'HTTPStatusCode': 200}}
```

## Steps to Reproduce
Replay the behavioral suite and re-run `cloudformation__ec2__ssm.sdk-ssm-parameter-policy-restricts-ec2-cross-account-read-negative`: put the restricted parameter, then read it from the scoped principal and expect a `ClientError`.
