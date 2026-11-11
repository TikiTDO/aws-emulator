Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cognito-idp.cfn-cognito-user-pool-in-place-admin-create-only-keeps-users-then-not-found
Issue-Key: cloudformation__cognito-idp.cfn-cognito-user-pool-in-place-admin-create-only-keeps-users-then-not-found|run#18|AssertionError|assert re.fullmatch(rf"{re.escape(ctx.region)}_[0-9a-zA-Z]+", pool_id), (
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# AWS::Cognito::UserPool physical resource id is the stack-derived name, not the <region>_<suffix> user pool id

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackResources

## Description
Cause 1 of 1: the user pool resource is tracked by a generated name instead of its Cognito pool id.

AWS: recorded behavior only. The physical resource id of AWS::Cognito::UserPool is the Cognito user pool id, which always has the form <region>_<alphanumeric suffix> (e.g. us-west-2_AbC123xyz) — that is also what Fn::Ref resolves to and what every Cognito Identity Provider API (DescribeUserPool, AdminCreateUser, ListUsers) takes as UserPoolId.

Emulator: the physical resource id is the CloudFormation-generated resource name instead: 'sim2real-cfnpool-adcdac0f163a'. It does not match <region>_<suffix>.

Why: the Ref value a template feeds to AWS::Cognito::UserPoolClient, to an API Gateway Cognito authorizer, or to application config is not a usable UserPoolId, so every Cognito call made with it is addressed to a pool id AWS would reject. Operators also cannot find the pool from the stack.

Fix: the AWS::Cognito::UserPool resource handler in the CloudFormation provisioner — return the Cognito-minted user pool id (<region>_<suffix>) as the physical resource id, and mint ids in that form in Cognito state.

Traces: the failing check in trace_ids.

## Evidence
Expected: the pool's PhysicalResourceId matches ^<region>_[0-9a-zA-Z]+$.
Actual: AssertionError: a user pool's physical id is its <region>_<suffix> pool id, not 'sim2real-cfnpool-adcdac0f163a'

## Steps to Reproduce
1. CreateStack a template with an AWS::Cognito::UserPool (AdminCreateUserConfig AllowAdminCreateUserOnly true); wait for CREATE_COMPLETE.
2. DescribeStackResources: the pool's PhysicalResourceId is the generated stack resource name, not a <region>_<suffix> pool id.
