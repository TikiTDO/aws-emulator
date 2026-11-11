Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecr__iam__s3__ssm__sts.cdk-bootstrap-deploy-role-refuses-an-external-id-unless-the-protection-is-off
Issue-Key: cloudformation__ecr__iam__s3__ssm__sts.cdk-bootstrap-deploy-role-refuses-an-external-id-unless-the-protection-is-off|_assert_bootstrap_roles#18|AssertionError|assert trust == want, f"{role} trust statements are {trust!r}, expected {want!r}"
Issue-Commit: 94350cf775acbc496950503935930c48964de18f

# cloudformation__ecr__iam__s3__ssm__sts: IAM stores a bare account-id principal verbatim in a role trust policy instead of canonicalizing it to arn:<partition>:iam::<account>:root

> _Note: this issue was identified against the code base at commit `94350cf775acbc496950503935930c48964de18f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRole

## Description
Cause 1 of 1: an account principal written as a bare account id is not canonicalized when the trust policy is stored, so the policy reads back in a form AWS never returns.
AWS: when a role trust policy (or any resource policy) names an account principal as a bare 12-digit account id — `"Principal": {"AWS": "<account-id>"}` — IAM canonicalizes it on write and always returns it as the account root ARN, `"Principal": {"AWS": "arn:<partition>:iam::<account-id>:root"}`. GetRole/AssumeRolePolicyDocument therefore shows the root ARN form regardless of which of the two equivalent spellings the caller submitted; the rest of the statement (Effect, Action, Condition) is returned unchanged. This is how the CDK bootstrap template's deploy/lookup/file-publishing/image-publishing roles read back, including the `sts:AssumeRole` statement guarded by `Condition: {"Null": {"sts:ExternalId": "true"}}` and the companion `sts:TagSession` statement.
Emulator: the bare account id is stored and returned verbatim — `{'Effect': 'Allow', 'Action': ['sts:AssumeRole'], 'Principal': {'AWS': ['<account-id>']}, 'Condition': {'Null': {'sts:ExternalId': ['true']}}}` where AWS returns `'Principal': {'AWS': ['arn:aws:iam::<account-id>:root']}`. Both statements of the deploy role diverge in exactly that one field; the condition and actions are correct.
Why: any client that reads a trust policy back and compares or matches principals — CDK's own bootstrap-version and trust reconciliation, policy linters, drift detection, and every `cdk bootstrap --trust`/`--untrust` flow — compares against the root-ARN form AWS guarantees and finds no match, so it concludes the role is untrusted or mis-trusted and either re-writes the policy on every run or refuses to deploy.
Fix: IAM's policy-document normalization on write (the principal canonicalization step shared by CreateRole/UpdateAssumeRolePolicy and the resource-policy writers), so a bare account id is expanded to the account root ARN before it is stored and projected.
Related open issue: 20261111T033513Z-cloudformation-ecr-iam-s3-ssm-iam-does-not-canonicalize-an-account-principal-in.md reports the same normalization fault from the `cloudformation__ecr__iam__s3__ssm` section; it does not carry this check id, so this check is not covered by it.
Traces: cloudformation__ecr__iam__s3__ssm__sts.cdk-bootstrap-deploy-role-refuses-an-external-id-unless-the-protection-is-off.

## Evidence
Behavioral suite replay (simparity-behavioral-aws-v1), `### cloudformation__ecr__iam__s3__ssm__sts` section (0/6 passed).

[mismatch] cloudformation__ecr__iam__s3__ssm__sts.cdk-bootstrap-deploy-role-refuses-an-external-id-unless-the-protection-is-off
  AssertionError: deploy-role trust statements are [{'Effect': 'Allow', 'Action': ['sts:AssumeRole'], 'Principal': {'AWS': ['000000000000']}, 'Condition': {'Null': {'sts:ExternalId': ['true']}}}, {'Effect': 'Allow', 'Action': ['sts:TagSession'], 'Principal': {'AWS': ['000000000000']}}], expected [{'Effect': 'Allow', 'Action': ['sts:AssumeRole'], 'Condition': {'Null': {'sts:ExternalId': ['true']}}, 'Principal': {'AWS': ['arn:aws:iam::000000000000:root']}}, {'Effect': 'Allow', 'Action': ['sts:TagSession'], 'Principal': {'AWS': ['arn:aws:iam::000000000000:root']}}]

Only the Principal.AWS field diverges, in both statements, and in exactly the bare-id-vs-root-ARN way. The account id above is this environment's per-run value; the requirement is the canonical `arn:<partition>:iam::<account>:root` spelling.

## Steps to Reproduce
1. git_clone the repo; build and boot the emulator from the checkout.
2. CreateRole (or deploy the CDK bootstrap toolkit stack) with an AssumeRolePolicyDocument whose statement uses `"Principal": {"AWS": "<account-id>"}` — the bare 12-digit id, not the root ARN.
3. GetRole and read AssumeRolePolicyDocument.Statement[*].Principal.AWS.
4. AWS returns `arn:<partition>:iam::<account-id>:root`; the emulator returns the bare account id it was given.
5. Replay cloudformation__ecr__iam__s3__ssm__sts.cdk-bootstrap-deploy-role-refuses-an-external-id-unless-the-protection-is-off.
