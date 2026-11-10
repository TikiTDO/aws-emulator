Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__events__lambda__sqs.cfn-lambda-permission-retained-replacement-keeps-both-grants-then-not-found
Issue-Key: cloudformation__events__lambda__sqs.cfn-lambda-permission-retained-replacement-keeps-both-grants-then-not-found|run#30|AssertionError|assert second_sid != first_sid, f"the replacement must be a new statement, still {first_sid!r}"
Issue-Commit: cd6d73564e5f4cf53934bca1327f08de31f2ee5f

# cloudformation__events__lambda__sqs: a replaced AWS::Lambda::Permission keeps the old statement id, so the replacement grant is indistinguishable from the retained one

> _Note: this issue was identified against the code base at commit `cd6d73564e5f4cf53934bca1327f08de31f2ee5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
One net-new root cause of service `cloudformation__events__lambda__sqs`, grounded in one failing behavioral check.

Cause 1 of 1 — a stack update that must REPLACE an AWS::Lambda::Permission reuses the replaced resource's physical id (the policy statement id) instead of minting a new one.
AWS: recorded behavior only (resource-provisioner behavior, not modelled in Smithy). Every property of AWS::Lambda::Permission is create-only, so changing one replaces the resource: CloudFormation creates a NEW permission whose physical id is a freshly generated statement id of the form `<stack-name>-<logical-id>-<random-suffix>`, and under `UpdateReplacePolicy: Retain` the old statement is left on the function. `GetPolicy` on the function therefore shows TWO statements with DIFFERENT `Sid` values — the retained one and the replacement one.
Emulator: after the replacing update the function's policy carries the replacement under the SAME statement id as the original (`...-Perm-1456C6827C12` both times), so the new grant and the retained grant cannot be told apart and the "both grants exist" state is unreachable.
Why: a client that reads the function policy to audit which grant a stack owns (or that calls RemovePermission with the statement id CloudFormation published) addresses the wrong statement — removing the replacement also removes the retained grant, and the retained grant is invisible.
Fix: the AWS::Lambda::Permission resource provisioner's replacement path in the cloudformation crate — the create half of a replacement must generate a new statement id rather than reusing the replaced resource's physical id.
Traces: cloudformation__events__lambda__sqs.cfn-lambda-permission-retained-replacement-keeps-both-grants-then-not-found

## Evidence
Check cloudformation__events__lambda__sqs.cfn-lambda-permission-retained-replacement-keeps-both-grants-then-not-found
assert second_sid != first_sid, "the replacement must be a new statement, still ..."
AssertionError: the replacement must be a new statement, still 'sim2real-cfnperm-stack-3a16c060b1fa-Perm-1456C6827C12'
(The statement id value itself is per-run; the requirement is that the replacement's id differs from the replaced one.)

## Steps to Reproduce
1. Boot the emulator and replay cloudformation__events__lambda__sqs.cfn-lambda-permission-retained-replacement-keeps-both-grants-then-not-found.
2. Create a stack with an AWS::Lambda::Permission (UpdateReplacePolicy: Retain) on a function, and note the physical id CloudFormation reports for it.
3. UpdateStack changing a create-only property of that permission (e.g. SourceArn), forcing a replacement.
4. Read the replaced resource's physical id and lambda GetPolicy: expect a new, different statement id for the replacement and the old statement still present.
