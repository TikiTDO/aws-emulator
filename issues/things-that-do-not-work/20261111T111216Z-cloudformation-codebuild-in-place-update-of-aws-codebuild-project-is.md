Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__codebuild.cfn-codebuild-project-in-place-update-builds-retained-replacement-then-not-found
Issue-Key: cloudformation__codebuild.cfn-codebuild-project-in-place-update-builds-retained-replacement-then-not-found|_update#19|AssertionError|assert stack["StackStatus"] == "UPDATE_COMPLETE", (
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# In-place update of AWS::CodeBuild::Project is refused with an emulator-authored NotImplemented reason

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Cause 1 of 1: changing a mutable property of a CodeBuild project through CloudFormation is refused instead of applied.

AWS: recorded behavior only. An UpdateStack that changes mutable AWS::CodeBuild::Project properties (buildspec, description, environment image/compute) is an in-place update applied through CodeBuild UpdateProject; the project name (the physical id) is unchanged, the resource goes UPDATE_IN_PROGRESS -> UPDATE_COMPLETE and the stack reaches UPDATE_COMPLETE. Only a Name change forces replacement.

Emulator: the update fails the resource and the stack rolls back: status 'UPDATE_ROLLBACK_COMPLETE', StatusReason 'The following resource(s) failed to update: [Project]. ', resource reason 'NotImplemented: this emulator does not change the properties of an existing AWS::CodeBuild::Project in place; the resource was left as it is.' (fixed text 'NotImplemented: this emulator does not change the properties of an existing ' ... ' in place; the resource was left as it is.', variable part = resource type). AWS has no NotImplemented error for a supported property change.

Why: a client cannot edit a build project it manages with CloudFormation — every buildspec or environment change rolls the stack back, and the retained-replacement behaviour that follows is unreachable.

Fix: the AWS::CodeBuild::Project resource handler's update path in the CloudFormation provisioner — classify mutable-property changes as in-place and call UpdateProject.

Traces: the failing check in trace_ids.

## Evidence
Expected: the update ends UPDATE_COMPLETE with the project retained.
Actual: AssertionError: update ended 'UPDATE_ROLLBACK_COMPLETE': 'The following resource(s) failed to update: [Project]. '; [('Project', 'NotImplemented: this emulator does not change the properties of an existing AWS::CodeBuild::Project in place; the resource was left as it is.')]

## Steps to Reproduce
1. CreateStack a template with an AWS::CodeBuild::Project; wait for CREATE_COMPLETE.
2. UpdateStack changing only the buildspec/description.
3. DescribeStackEvents: Project is UPDATE_FAILED with the NotImplemented reason; the stack is UPDATE_ROLLBACK_COMPLETE.
