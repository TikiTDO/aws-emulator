Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecr__iam__s3__ssm.cdk-bootstrap-image-asset-already-in-ecr-is-not-rebuilt-or-pushed-again cloudformation__ecr__iam__s3__ssm.cdk-bootstrap-termination-protection-survives-a-forced-re-bootstrap-and-refuses-delete
Issue-Key: cloudformation__ecr__iam__s3__ssm.cdk-bootstrap-image-asset-already-in-ecr-is-not-rebuilt-or-pushed-again|_assert_bootstrap_roles#37|AssertionError|assert trust == want, f"{role} trust statements are {trust!r}, expected {want!r}"
Issue-Key: cloudformation__ecr__iam__s3__ssm.cdk-bootstrap-termination-protection-survives-a-forced-re-bootstrap-and-refuses-delete|_assert_bootstrap_roles#19|AssertionError|assert trust == want, f"{role} trust statements are {trust!r}, expected {want!r}"
Issue-Commit: a5271544d134d49a9bef1d276b891cd8aa450a13

# IAM does not canonicalize an account principal in a role trust policy: GetRole returns the bare account id instead of arn:aws:iam::<account>:root

> _Note: this issue was identified against the code base at commit `a5271544d134d49a9bef1d276b891cd8aa450a13`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRole

## Description
One net-new root cause for this service heading - these two checks previously failed on the staging-bucket encryption assertion and now fail later, on the trust-policy projection of the bootstrap roles.

Cause 1 of 1: a trust-policy principal supplied as a bare account id is stored and returned verbatim instead of being canonicalized to the account's root ARN.
AWS:      IAM canonicalizes an AWS account principal when it stores a trust policy - a Principal {"AWS": "<account-id>"} is persisted and reported as {"AWS": "arn:aws:iam::<account-id>:root"} (IAM JSON policy elements: Principal; the same canonicalization applies whichever form CreateRole/UpdateAssumeRolePolicy received). So Smithy (iam) GetRole -> Role$AssumeRolePolicyDocument for a role whose template trusted the deploying account carries Principal.AWS = ["arn:aws:iam::<account>:root"] on every statement, including both the sts:AssumeRole statement (with its Condition {"Null": {"sts:ExternalId": "true"}}) and the sts:TagSession statement.
Emulator: the document is echoed exactly as submitted - Principal.AWS = ["<account-id>"] on both statements - so a reader comparing against the canonical root ARN sees no trusted account at all.
Why:      tooling that compares, edits or asserts on a trust policy (CDK's `cdk bootstrap --trust` / `--untrust` paths, policy linters, drift detection, least-privilege audits) matches on the canonical root ARN. Against this emulator such a tool concludes the account is not trusted, so a trust-removal or trust-audit step cannot work, and a read-modify-write of the document silently changes its canonical form.
Fix:      IAM's trust-policy normalization on role create/update, so the stored and reported AssumeRolePolicyDocument carries the canonical principal (and IAM's GetRole projection of it).
Traces:   cloudformation__ecr__iam__s3__ssm.cdk-bootstrap-image-asset-already-in-ecr-is-not-rebuilt-or-pushed-again, cloudformation__ecr__iam__s3__ssm.cdk-bootstrap-termination-protection-survives-a-forced-re-bootstrap-and-refuses-delete.

The open issue 20261110T232635Z-cloudformation-ecr-iam-s3-ssm-cdk-bootstrap-toolkit-fidelity-ecr-lifecyclepolicy-dropped-defaulted.md describes this same canonicalization defect as its Cause 5, but grounds it in a different check (cdk-bootstrap-untrust-removes-the-account-from-every-role-trust-in-place) and carries these two checks' replay keys only for the bucket-encryption assertion, which no longer fails. 20261104T152822Z-iam-role-and-policy-projections-5-net-new.md also mentions trust-policy projection defects and does not carry these checks' keys.

## Evidence
Checks cloudformation__ecr__iam__s3__ssm.cdk-bootstrap-image-asset-already-in-ecr-is-not-rebuilt-or-pushed-again and cloudformation__ecr__iam__s3__ssm.cdk-bootstrap-termination-protection-survives-a-forced-re-bootstrap-and-refuses-delete [both mismatch], at _assert_bootstrap_roles: `assert trust == want, f"{role} trust statements are {trust!r}, expected {want!r}"`:

Actual (emulator): deploy-role trust statements are
  [{'Effect': 'Allow', 'Condition': {'Null': {'sts:ExternalId': 'true'}}, 'Action': ['sts:AssumeRole'], 'Principal': {'AWS': ['<account-id>']}},
   {'Effect': 'Allow', 'Action': ['sts:TagSession'], 'Principal': {'AWS': ['<account-id>']}}]
Expected (AWS): the same statements with
   'Principal': {'AWS': ['arn:aws:iam::<account-id>:root']}

(The account id is a per-environment value; the requirement is the canonical arn:aws:iam::<account-id>:root form.)

## Steps to Reproduce
1. Run `cdk bootstrap` against the emulator (the toolkit template trusts the deploying account by bare account id).
2. Call iam:GetRole on cdk-<qualifier>-deploy-role-<account>-<region> (any cdk-<qualifier>-* role shows it).
3. Inspect AssumeRolePolicyDocument: Principal.AWS is the bare account id on both the sts:AssumeRole and sts:TagSession statements, not arn:aws:iam::<account>:root.
