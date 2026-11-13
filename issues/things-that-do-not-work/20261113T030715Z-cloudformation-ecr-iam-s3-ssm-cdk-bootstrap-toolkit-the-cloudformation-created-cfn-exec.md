Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecr__iam__s3__ssm.cdk-bootstrap-custom-permissions-boundary-bounds-only-the-exec-role cloudformation__ecr__iam__s3__ssm.cdk-bootstrap-custom-permissions-boundary-with-a-path-bounds-the-exec-role
Issue-Key: cloudformation__ecr__iam__s3__ssm.cdk-bootstrap-custom-permissions-boundary-bounds-only-the-exec-role|_assert_bootstrap_roles#18|AssertionError|assert got.get("PermissionsBoundary") == want_boundary, (
Issue-Key: cloudformation__ecr__iam__s3__ssm.cdk-bootstrap-custom-permissions-boundary-with-a-path-bounds-the-exec-role|_assert_bootstrap_roles#19|AssertionError|assert got.get("PermissionsBoundary") == want_boundary, (
Issue-Commit: 00fa4fcb7857a2daae640c829f8c16fc60943323

# cdk bootstrap toolkit: the CloudFormation-created cfn-exec-role carries no PermissionsBoundary, so --custom-permissions-boundary is silently unenforced

> _Note: this issue was identified against the code base at commit `00fa4fcb7857a2daae640c829f8c16fc60943323`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause in the cloudformation__ecr__iam__s3__ssm group, found by replaying the behavioral suite.

Cause 1 of 1: an `AWS::IAM::Role` that a stack declares with a `PermissionsBoundary` property is created WITHOUT that boundary. The CDK bootstrap template's `CloudFormationExecutionRole` declares `PermissionsBoundary` (fed by the `InputPermissionsBoundary` parameter that `cdk bootstrap --custom-permissions-boundary <policy>` sets), and the stack reaches CREATE_COMPLETE, yet IAM reports the role as unbounded.

AWS: `iam:GetRole` on a role created with a permissions boundary returns a `Role` carrying the `PermissionsBoundary` member. Smithy (iam): `Role$PermissionsBoundary` targets `AttachedPermissionsBoundary` with members `PermissionsBoundaryType` (enum, documented as only ever `Policy`) and `PermissionsBoundaryArn` (the boundary policy's ARN). So for the toolkit's exec role AWS answers `PermissionsBoundary = {"PermissionsBoundaryType": "Policy", "PermissionsBoundaryArn": "<the boundary policy ARN named by the template>"}`, and only for that role — the other four toolkit roles (deploy-role, file-publishing-role, image-publishing-role, lookup-role) stay unbounded while `ApplyPermissionsBoundaryToAllRoles` is `false`. The boundary ARN is formed from the request's own values (`arn:aws:iam::<account>:policy/<path><policy-name>`), so a boundary policy created under a path keeps that path in the ARN. The boundary also takes part in IAM's authorization decision: `SimulatePrincipalPolicy` for the bounded role returns `explicitDeny`/allowed-false for an action the boundary does not permit, and `iam:GetPolicy` on the boundary reports `PermissionsBoundaryUsageCount = 1` while one role is bounded by it.

Emulator: the stack's parameters do echo `InputPermissionsBoundary = <policy name>` and `ApplyPermissionsBoundaryToAllRoles = "false"` correctly, but `iam:GetRole` on `cdk-<qualifier>-cfn-exec-role-<account>-<region>` returns a `Role` with no `PermissionsBoundary` member at all (botocore sees it as absent / `None`). The declared boundary is dropped between the template and IAM.

Why: a client that relies on CloudFormation (or on `cdk bootstrap --custom-permissions-boundary`) to enforce a delegation boundary gets an unbounded role that is in fact attached to `AdministratorAccess`. That is a silent privilege escalation: the guard rail the operator asked for does not exist, and nothing in the stack's own output reveals it. Any downstream check of the boundary (GetRole, GetPolicy's usage count, SimulatePrincipalPolicy) reads as "no boundary".

Fix: the CloudFormation provisioner's `AWS::IAM::Role` arm — it must pass the template's `PermissionsBoundary` through on role creation (`iam:CreateRole`'s own `PermissionsBoundary` member) and must treat a later change or removal of the property as an in-place update via `iam:PutRolePermissionsBoundary` / `iam:DeleteRolePermissionsBoundary`; IAM's role state must then store it and `GetRole`'s serializer must emit the `AttachedPermissionsBoundary` structure. Related open issue (same root cause, different service group, so it does not cover these checks): `20261104T160000Z-cloudformation-iam-lambda-s3-six-causes-awaiting-the-lambda-provisioner-and-an-in-process-runtime.md`, whose "Cause 2 of 6" names `state/provisioner.rs::create_role` and `ROLE_IN_PLACE_PROPERTIES`.

Traces: cloudformation__ecr__iam__s3__ssm.cdk-bootstrap-custom-permissions-boundary-bounds-only-the-exec-role, cloudformation__ecr__iam__s3__ssm.cdk-bootstrap-custom-permissions-boundary-with-a-path-bounds-the-exec-role

## Evidence
Both failing checks stop at the same assertion in `_assert_bootstrap_roles`:

  assert got.get("PermissionsBoundary") == want_boundary
  AssertionError: cfn-exec-role permissions boundary is None, expected {'PermissionsBoundaryType': 'Policy', 'PermissionsBoundaryArn': '<boundary policy ARN>'}

- `cdk-bootstrap-custom-permissions-boundary-bounds-only-the-exec-role`: expected `{'PermissionsBoundaryType': 'Policy', 'PermissionsBoundaryArn': 'arn:aws:iam::<account>:policy/<policy-name>'}`; got `None`.
- `cdk-bootstrap-custom-permissions-boundary-with-a-path-bounds-the-exec-role`: expected the same structure with the path kept in the ARN (`arn:aws:iam::<account>:policy/cdk-boundary/with/<policy-name>`); got `None`.

The account id, qualifier and generated policy name in the recorded run are per-run values that fill the template; the requirement is the presence of the `PermissionsBoundary` structure with `PermissionsBoundaryType = "Policy"` and `PermissionsBoundaryArn` equal to the boundary ARN the template named (path included). Everything before the assertion passes: `CreatePolicy` returns the expected ARN, `cdk bootstrap` exits 0, the stack reports the two boundary parameters, and the role's ARN and trust policy match — only the boundary is missing. The checks never reach the follow-on assertions on `PermissionsBoundaryUsageCount = 1` and on `SimulatePrincipalPolicy` returning `explicitDeny` for an action outside the boundary, so those are blocked by the same cause.

## Steps to Reproduce
1. Create an IAM managed policy to use as a boundary (once with no path, once under a path such as `/cdk-boundary/with/`).
2. Run `cdk bootstrap aws://<account>/<region> --toolkit-stack-name <stack> --qualifier <q> --custom-permissions-boundary <policy-name>`; it exits 0 and the stack reaches CREATE_COMPLETE.
3. `cloudformation:DescribeStacks` on the toolkit stack — `InputPermissionsBoundary` is the policy name and `ApplyPermissionsBoundaryToAllRoles` is `false` (both correct today).
4. `iam:GetRole` on `cdk-<q>-cfn-exec-role-<account>-<region>`: the returned `Role` has no `PermissionsBoundary` member, where AWS returns `{"PermissionsBoundaryType": "Policy", "PermissionsBoundaryArn": "arn:aws:iam::<account>:policy/<path><policy-name>"}`.
5. `iam:GetPolicy` on the boundary ARN reports `PermissionsBoundaryUsageCount` 0 instead of 1, and `iam:SimulatePrincipalPolicy` for that role allows an action the boundary excludes (e.g. `sns:CreateTopic`) instead of returning `explicitDeny`.
6. Equivalently, deploy any stack with an `AWS::IAM::Role` carrying `PermissionsBoundary` and read it back with `iam:GetRole`.
