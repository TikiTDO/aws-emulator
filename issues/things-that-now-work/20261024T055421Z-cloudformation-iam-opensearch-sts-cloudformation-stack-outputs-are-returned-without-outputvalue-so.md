Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__opensearch__sts.deploy-opensearch-1-changeset-swaps-allowed-and-denied-roles-in-place
Issue-Key: cloudformation__iam__opensearch__sts.deploy-opensearch-1-changeset-swaps-allowed-and-denied-roles-in-place|_stack_outputs#5|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# CloudFormation stack Outputs are returned without OutputValue, so !GetAtt/!Ref outputs cannot be read back

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
One root cause.

Cause 1 of 1: DescribeStacks returns Outputs entries that carry OutputKey but omit OutputValue.
AWS: once a stack reaches CREATE_COMPLETE/UPDATE_COMPLETE, DescribeStacks returns, for every template Output, an Output member with both OutputKey and the resolved OutputValue. Smithy: cloudformation#Output has members OutputKey, OutputValue, Description, ExportName; OutputValue is the resolved string of the template's `Value` expression (here !GetAtt Domain.Arn and !Ref on IAM role ARNs). A completed stack whose template declares Outputs therefore always yields OutputValue for each of them.
Emulator: the Outputs list is present but each entry lacks the OutputValue key, so `{o['OutputKey']: o['OutputValue'] for o in stack.get('Outputs', [])}` raises KeyError: 'OutputValue'. The stack itself reported completion, so the divergence is purely in how outputs are resolved/serialized, not in stack status.
Why: the output map is the standard hand-off between a deployed stack and the code that uses it (role ARNs, domain ARNs, endpoints). A client reading stack outputs crashes with a missing key, or silently gets no values, and every downstream step that needs a deployed resource identifier is unreachable — the check fails at leg 0 with the whole 11-leg workflow ungraded.
Fix: the CloudFormation output resolution/serialization path — resolve each template Output's Value (intrinsics such as !GetAtt, !Ref, !Sub) at stack completion, store it, and emit OutputValue in the DescribeStacks Output serializer.
Traces: cloudformation__iam__opensearch__sts.deploy-opensearch-1-changeset-swaps-allowed-and-denied-roles-in-place.

## Evidence
Expected: DescribeStacks -> Stacks[0].Outputs == [{'OutputKey': 'DomainArn', 'OutputValue': 'arn:aws:es:<region>:<account>:domain/<name>'}, ...] (key and resolved value both present).
Actual: KeyError: 'OutputValue' raised at
  File "check_cloudformation__iam__opensearch__sts_deploy_opensearch_1_changeset_swaps_allowed_and_denied_roles_in_place.py", line 363, in _stack_outputs
  return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}

## Steps to Reproduce
1. Boot the emulator.
2. cfn.create_stack with a template declaring Resources (an AWS::OpenSearchService::Domain and IAM roles) and Outputs using !GetAtt/!Ref.
3. Wait for stack_create_complete, then cfn.describe_stacks(StackName=...).
4. Inspect Stacks[0]['Outputs']: entries have OutputKey but no OutputValue.
5. Replay cloudformation__iam__opensearch__sts.deploy-opensearch-1-changeset-swaps-allowed-and-denied-roles-in-place.
