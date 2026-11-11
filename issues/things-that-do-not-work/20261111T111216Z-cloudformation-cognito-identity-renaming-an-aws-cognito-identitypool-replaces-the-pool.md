Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cognito-identity.cfn-cognito-identity-pool-in-place-rename-keeps-developer-identities-then-not-found
Issue-Key: cloudformation__cognito-identity.cfn-cognito-identity-pool-in-place-rename-keeps-developer-identities-then-not-found|run#20|AssertionError|assert _physical_ids(cfn, ref["stack"])["Pool"] == pool_id, "a rename keeps the pool"
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# Renaming an AWS::Cognito::IdentityPool replaces the pool instead of updating it in place

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Cause 1 of 1: a change to IdentityPoolName is treated as a replacement, so the pool (and everything stored in it) is thrown away.

AWS: recorded behavior only. IdentityPoolName is a mutable property of AWS::Cognito::IdentityPool: CloudFormation applies the rename through Cognito Identity UpdateIdentityPool, the IdentityPoolId (the physical resource id) is unchanged, and the developer identities, logins and role attachments registered against the pool survive the update.

Emulator: after the rename the stack's physical resource id for the pool is a different IdentityPoolId — the handler created a new pool rather than updating the existing one ('a rename keeps the pool').

Why: a client that renames a pool through CloudFormation silently loses the pool's identities and any credentials issued against it, and every consumer holding the old IdentityPoolId (role attachments, app config, GetId/GetCredentialsForIdentity callers) breaks. Replacement also means the old pool is deleted, destroying customer data an in-place update would have kept.

Fix: the AWS::Cognito::IdentityPool resource handler's update classification in the CloudFormation provisioner — IdentityPoolName must be an in-place (UpdateIdentityPool) change, not a replacement trigger.

Traces: the failing check in trace_ids.

## Evidence
Expected: after the rename DescribeStackResources still reports the original IdentityPoolId as the pool's PhysicalResourceId.
Actual: AssertionError: a rename keeps the pool — the physical id changed.

## Steps to Reproduce
1. CreateStack a template with an AWS::Cognito::IdentityPool; note the PhysicalResourceId and register a developer identity in it.
2. UpdateStack changing only IdentityPoolName.
3. DescribeStackResources: the pool's PhysicalResourceId is a new IdentityPoolId.
