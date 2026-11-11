Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__lambda__logs__s3.cdk-default-deploy-function-writes-granted-table-and-carries-cdk-metadata
Issue-Key: cloudformation__dynamodb__lambda__logs__s3.cdk-default-deploy-function-writes-granted-table-and-carries-cdk-metadata|run#16|AssertionError|assert rc == 0, f"`cdk deploy` exited {rc}; output tail {out[-800:]!r}"
Issue-Commit: 86c9f1e4438a61fbe37b19dfcd3d77efce3d0679

# CloudFormation rejects the AWS::CDK::Metadata resource type, so every `cdk deploy` fails at changeset creation (1 cause)

> _Note: this issue was identified against the code base at commit `86c9f1e4438a61fbe37b19dfcd3d77efce3d0679`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChangeSet

## Description
Cause 1 of 1 — CloudFormation refuses a template containing the AWS::CDK::Metadata resource that the CDK emits by default.
AWS: Smithy (com.amazonaws.cloudformation) CreateChangeSet; recorded behavior only for this rule. `AWS::CDK::Metadata` is a no-op metadata resource that the AWS CDK adds to every synthesized template unless analytics are disabled; CloudFormation accepts it, creates the changeset, and the stack reaches CREATE_COMPLETE with that logical resource present (its physical id is the analytics string). It is not a user-registered type and needs no registration.
Emulator: fails changeset creation with `Template format error: Unrecognized resource types: [AWS::CDK::Metadata]`, so `cdk deploy` exits 1 immediately after publishing the template and no stack resource is ever created.
Why: this is the default output of the CDK, so no CDK application can be deployed against the emulator at all — not the stack under test, not its Lambda, table or grants. A user must edit synthesized output to work around it, which defeats the point of replaying a default `cdk deploy`.
Fix: the template resource-type registry/validation in the cloudformation crate — treat AWS::CDK::Metadata as a recognised no-op resource (accepted, created, reported in the stack's resources, and deleted with the stack) rather than an unknown type.
Traces: cloudformation__dynamodb__lambda__logs__s3.cdk-default-deploy-function-writes-granted-table-and-carries-cdk-metadata

One cause must be closed before submitting.

## Evidence
From the simparity-behavioral-aws-v1 replay against this checkout.

cdk-default-deploy-function-writes-granted-table-and-carries-cdk-metadata:
  AssertionError: `cdk deploy` exited 1; output tail '... <stack>: start: Publishing <stack> Template ...\n<stack>: success: Published <stack> Template ...\n<stack>: creating CloudFormation changeset...\nTemplate format error: Unrecognized resource types: [AWS::CDK::Metadata]\n'

The stack name and the asset hash in that output are per-run values; the refusal text and the named type are the divergence. AWS creates the changeset and the stack reaches CREATE_COMPLETE with the AWS::CDK::Metadata logical resource.

## Steps to Reproduce
Boot the emulator from this checkout and point the CDK/CLI at it.

1. Synthesize any default CDK app (no `--no-version-reporting`), so the template contains a resource of type AWS::CDK::Metadata.
2. cdk deploy
-> exits 1 with 'Template format error: Unrecognized resource types: [AWS::CDK::Metadata]'.
Equivalently: aws cloudformation create-change-set --stack-name t --change-set-name c --template-body '{"Resources":{"CDKMetadata":{"Type":"AWS::CDK::Metadata","Properties":{"Analytics":"v2:deflate64:abc"}}}}' --capabilities CAPABILITY_IAM -> same template format error; AWS accepts it.

Then re-replay: cloudformation__dynamodb__lambda__logs__s3.cdk-default-deploy-function-writes-granted-table-and-carries-cdk-metadata
