Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cognito-identity__sts.cfn-cognito-role-attachment-in-place-role-swap-retained-pool-move-then-not-found
Issue-Key: cloudformation__cognito-identity__sts.cfn-cognito-role-attachment-in-place-role-swap-retained-pool-move-then-not-found|run#16|AssertionError|assert _outputs(stack) == {"AttachedPool": pool_a}, f"Ref must return pool A {pool_a!r}"
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# Ref on AWS::Cognito::IdentityPoolRoleAttachment does not return the attached identity pool id

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Cause 1 of 1: the role attachment's Ref value is not the pool it is attached to.

AWS: recorded behavior only. The physical resource id of AWS::Cognito::IdentityPoolRoleAttachment is the IdentityPoolId it attaches roles to, so Fn::Ref on that resource resolves to the pool id (of the form '<region>:<uuid>'). After an in-place role swap the Ref still names the same pool; only moving the attachment to a different pool changes it.

Emulator: the stack output built from Ref on the role attachment does not carry the pool id of pool A — the check's read of the output does not equal the IdentityPoolId it attached to ('Ref must return pool A <region>:<uuid>').

Why: templates wire the attachment's Ref into other resources and outputs to name the pool that now has roles; a wrong Ref value means downstream resources, cross-stack exports and operator tooling point at the wrong thing (or at nothing), and the role-swap/pool-move semantics cannot be told apart.

Fix: the AWS::Cognito::IdentityPoolRoleAttachment resource handler — its physical resource id (and therefore Ref) must be the IdentityPoolId.

Traces: the failing check in trace_ids.

## Evidence
Expected: the stack output from Ref <RoleAttachment> == the IdentityPoolId of pool A, e.g. 'us-west-2:be228d9e-cdd6-4b0b-b961-ff22bb683fb7'.
Actual: AssertionError: Ref must return pool A 'us-west-2:be228d9e-cdd6-4b0b-b961-ff22bb683fb7' — the output did not match.

## Steps to Reproduce
1. CreateStack a template with two AWS::Cognito::IdentityPool resources and an AWS::Cognito::IdentityPoolRoleAttachment on pool A, with an Output exporting Ref on the attachment.
2. DescribeStacks: the output is not pool A's IdentityPoolId.
