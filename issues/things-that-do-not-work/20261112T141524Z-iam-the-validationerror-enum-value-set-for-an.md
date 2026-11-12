Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.unrecognized-ssh-public-key-encoding-rejected
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.unrecognized-ssh-public-key-encoding-rejected|run#17|AssertionError|assert message_update == expected, f"expected message {expected!r}, got {message_update!r}"
Issue-Commit: e14db27dbe62f4f0b2a12e088b6709fa0d2b11d3

# iam: the ValidationError enum-value-set for an invalid UpdateSSHPublicKey Status is rendered in model-declaration order, not AWS's order

> _Note: this issue was identified against the code base at commit `e14db27dbe62f4f0b2a12e088b6709fa0d2b11d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateSSHPublicKey

## Description
One net-new root cause, reached by a behavioral check in the `iam` suite section that no open issue carries.

Cause 1 of 1: the query-protocol enum-constraint validation message renders the accepted-value set in the wrong order for `statusType`.

AWS: Smithy `com.amazonaws.iam#UpdateSSHPublicKey` input member `Status` targets `com.amazonaws.iam#statusType`, an enum whose declaration order in the model is `[Active, Inactive, Expired]`. For a `Status` outside that set AWS refuses the call with HTTP 400, error code `ValidationError`, and the message `1 validation error detected: Value at 'status' failed to satisfy constraint: Member must satisfy enum value set: [Expired, Active, Inactive]`. Recorded behavior only for the ordering: the model does not state the rendering order, and AWS's wire order for this enum is NOT the model's declaration order. Fixed text of that message is everything except two variable parts — the member path (`status`, taken from the request's member name) and the bracketed value set (taken from the targeted enum shape, rendered in AWS's order for that shape). The sibling member on the same check, `GetSSHPublicKey.Encoding` -> `com.amazonaws.iam#encodingType`, is rendered `[SSH, PEM]` and the emulator already matches it, so the divergence is specific to how the value set is ordered, not to the message template.

Emulator: status 400 and code `ValidationError` are correct and every other byte of the message matches, but the bracketed set comes out as `[Active, Inactive, Expired]` — the model's declaration order. Full emulator message: `1 validation error detected: Value at 'status' failed to satisfy constraint: Member must satisfy enum value set: [Active, Inactive, Expired]`.

Why: a client or operator tool that asserts on, logs or surfaces the validation message — the only part of the response that says which member failed and what values it accepts — never matches AWS's text, and an operator reading the refusal is shown a different accepted-value list than AWS prints. Because the same formatter serves every enum-constrained member of the service, every enum whose AWS wire order differs from its model order is misrendered the same way, not just `statusType`.

Fix: the shared query-protocol enum-constraint validation message formatter for the iam service (the ordering of the rendered value set it emits), not the UpdateSSHPublicKey handler itself; the handler's status, error code and message template are already right.

Related: open issue 20261111T103332Z-iam-7-net-new-causes-deleteaccountpasswordpolicy-is-unrouted.md describes the same statusType ordering divergence as its Cause 5, but it is grounded only in `iam.update-ssh-public-key-invalid-status-value-rejected` and does NOT carry this check's replay key, so this check is not covered by it. Fixing the formatter once should close both.

Traces: iam.unrecognized-ssh-public-key-encoding-rejected.

## Evidence
Behavioral suite replay (simparity-behavioral-aws-v1) against the emulator built from this checkout.

[mismatch] iam.unrecognized-ssh-public-key-encoding-rejected
  File "check_iam_unrecognized_ssh_public_key_encoding_rejected.py", line 80, in run
    assert message_update == expected, f"expected message {expected!r}, got {message_update!r}"
  AssertionError: expected message "1 validation error detected: Value at 'status' failed to satisfy constraint: Member must satisfy enum value set: [Expired, Active, Inactive]", got "1 validation error detected: Value at 'status' failed to satisfy constraint: Member must satisfy enum value set: [Active, Inactive, Expired]"

Expected (AWS): HTTP 400, Error.Code `ValidationError`, Error.Message `1 validation error detected: Value at 'status' failed to satisfy constraint: Member must satisfy enum value set: [Expired, Active, Inactive]`.
Actual (emulator): HTTP 400, Error.Code `ValidationError` (both correct), Error.Message `1 validation error detected: Value at 'status' failed to satisfy constraint: Member must satisfy enum value set: [Active, Inactive, Expired]`.

The divergence is confined to the bracketed value set's order. The earlier assertions of the same check passed, which pins the scope:
- Error.Code for the invalid `Encoding` is `ValidationError` — matches.
- Message for the invalid `Encoding` is `1 validation error detected: Value at 'encoding' failed to satisfy constraint: Member must satisfy enum value set: [SSH, PEM]` — matches byte for byte, so the message template and the member-path rendering are correct.
- Error.Code for the invalid `Status` is `ValidationError` — matches.

Model grounding (smithy_query on services/service-com-amazonaws-iam-awsidentitymanagementv20100508/smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json):
- com.amazonaws.iam#statusType: type enum, values [Active, Inactive, Expired] (declaration order). AWS's validation message prints [Expired, Active, Inactive].
- com.amazonaws.iam#encodingType: type enum, values [SSH, PEM]. AWS's validation message prints [SSH, PEM] — the orders coincide here, which is why that leg passes and masks the defect for this enum.

No per-run value is involved: the message has no id, name, timestamp or request id in it, so the whole string is a requirement.

## Steps to Reproduce
1. git_clone the repo; build and boot the emulator from the checkout.
2. iam CreateUser with any unique user name.
3. iam UploadSSHPublicKey for that user with a valid ssh-rsa OpenSSH public-key body; keep the returned SSHPublicKey.SSHPublicKeyId.
4. iam GetSSHPublicKey(UserName, SSHPublicKeyId, Encoding="BOGUS-ENCODING") and read Error.Message — it already matches AWS (`... Value at 'encoding' ... enum value set: [SSH, PEM]`), confirming the template is right.
5. iam UpdateSSHPublicKey(UserName, SSHPublicKeyId, Status="BogusStatus") and read Error.Message. The emulator renders the bracketed set as `[Active, Inactive, Expired]`; AWS renders `[Expired, Active, Inactive]`.
6. Replay the behavioral check iam.unrecognized-ssh-public-key-encoding-rejected to confirm the fix (it also asserts that the rejected update left the key's Status at `Active` and that a valid Status="Inactive" update then takes effect).
