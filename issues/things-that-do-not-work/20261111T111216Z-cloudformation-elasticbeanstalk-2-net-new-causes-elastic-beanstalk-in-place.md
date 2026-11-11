Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__elasticbeanstalk.cfn-elasticbeanstalk-application-in-place-update-retained-replacement-then-not-found cloudformation__elasticbeanstalk.cfn-elasticbeanstalk-environment-in-place-update-keeps-instance-then-terminated cloudformation__elasticbeanstalk.cfn-elasticbeanstalk-environment-retained-replacement-keeps-old-running-then-terminated
Issue-Key: cloudformation__elasticbeanstalk.cfn-elasticbeanstalk-application-in-place-update-retained-replacement-then-not-found|_expect#24|AssertionError|raise AssertionError(
Issue-Key: cloudformation__elasticbeanstalk.cfn-elasticbeanstalk-environment-in-place-update-keeps-instance-then-terminated|_expect#29|AssertionError|raise AssertionError(
Issue-Key: cloudformation__elasticbeanstalk.cfn-elasticbeanstalk-environment-retained-replacement-keeps-old-running-then-terminated|_assert_replaced_and_retained#37|AssertionError|assert (
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# 2 net-new causes: Elastic Beanstalk in-place updates are refused, and a replacing update is not reported as an update

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Two independent root causes in the Elastic Beanstalk CloudFormation resource handlers.

Cause 1 of 2: in-place updates of AWS::ElasticBeanstalk::Application and AWS::ElasticBeanstalk::Environment are refused with an emulator-authored reason.
AWS: changing mutable properties (an application's Description, an environment's OptionSettings/Description/VersionLabel) is an in-place update applied through UpdateApplication / UpdateEnvironment; the physical id (application name / environment name) is unchanged, the resource goes UPDATE_IN_PROGRESS -> UPDATE_COMPLETE, and for an environment the instances it owns keep running.
Emulator: both are UPDATE_FAILED and the stack ends 'UPDATE_ROLLBACK_COMPLETE' with StatusReason 'The following resource(s) failed to update: [<LogicalId>]. '; the reasons are 'NotImplemented: this emulator does not change the properties of an existing AWS::ElasticBeanstalk::Application in place; the resource was left as it is.' and the same sentence for AWS::ElasticBeanstalk::Environment (fixed text 'NotImplemented: this emulator does not change the properties of an existing ' ... ' in place; the resource was left as it is.'; variable part = resource type). AWS has no NotImplemented error for a supported property change.
Why: a Beanstalk application or environment managed by CloudFormation cannot be edited at all — every config or version change rolls the stack back and never reaches the environment.
Fix: the AWS::ElasticBeanstalk::Application and ::Environment resource handlers' update paths in the CloudFormation provisioner.
Failing checks: cfn-elasticbeanstalk-application-in-place-update-retained-replacement-then-not-found, cfn-elasticbeanstalk-environment-in-place-update-keeps-instance-then-terminated.

Cause 2 of 2: a replacing update of AWS::ElasticBeanstalk::Environment is emitted as a bare create-then-delete and the retained old environment is deleted.
AWS: a replacement is reported on the logical resource as UPDATE_IN_PROGRESS (old environment name) -> CREATE_IN_PROGRESS 'Resource creation Initiated' (new name) -> CREATE_COMPLETE -> UPDATE_COMPLETE; with UpdateReplacePolicy Retain the old environment is not deleted and keeps running.
Emulator: the events are only CREATE_IN_PROGRESS / CREATE_IN_PROGRESS('Resource creation Initiated') / CREATE_COMPLETE for the new environment followed by DELETE_IN_PROGRESS / DELETE_COMPLETE for the old one — no UPDATE_* events on the logical resource and the Retain policy is ignored.
Why: a client watching events cannot see that a replacement occurred, and an environment the author asked to retain (with its running instances) is terminated.
Fix: the CloudFormation update engine's replacement event emission and UpdateReplacePolicy Retain handling.
Failing check: cfn-elasticbeanstalk-environment-retained-replacement-keeps-old-running-then-terminated.

## Evidence
Cause 1: AssertionError: stack ended 'UPDATE_ROLLBACK_COMPLETE' ('The following resource(s) failed to update: [Application]. '), expected 'UPDATE_COMPLETE'; failed events: [('Application', 'UPDATE_FAILED', 'NotImplemented: this emulator does not change the properties of an existing AWS::ElasticBeanstalk::Application in place; the resource was left as it is.')] — and the same with [Environment] / AWS::ElasticBeanstalk::Environment.
Cause 2: AssertionError: Environment must announce its replacement, its events were [('CREATE_IN_PROGRESS', None, ''), ('CREATE_IN_PROGRESS', 'Resource creation Initiated', 's2r-ebe-b-...'), ('CREATE_COMPLETE', None, 's2r-ebe-b-...'), ('DELETE_IN_PROGRESS', None, None), ('DELETE_COMPLETE', None, 's2r-ebe-a-...')]

## Steps to Reproduce
Cause 1: CreateStack an AWS::ElasticBeanstalk::Application (and an ::Environment on it), then UpdateStack changing only Description / OptionSettings.
Cause 2: give the Environment UpdateReplacePolicy Retain, UpdateStack with a replacement-forcing change (EnvironmentName), then read DescribeStackEvents.
