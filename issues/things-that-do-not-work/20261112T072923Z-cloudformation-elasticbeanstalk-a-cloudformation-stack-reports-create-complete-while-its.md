Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__elasticbeanstalk.deploy-elasticbeanstalk-environment-round-trip-and-delete
Issue-Key: cloudformation__elasticbeanstalk.deploy-elasticbeanstalk-environment-round-trip-and-delete|run#21|AssertionError|assert got == ("Ready", solution_stack, out["EndpointURL"]), f"environment reads {got!r}"
Issue-Commit: bbe224ee8535eddb09d8153d70c72e25d751373e

# A CloudFormation stack reports CREATE_COMPLETE while its AWS::ElasticBeanstalk::Environment is still Launching (1 cause)

> _Note: this issue was identified against the code base at commit `bbe224ee8535eddb09d8153d70c72e25d751373e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
ONE root cause, grounded in a failing behavioral check of this suite that the replay marked neither ALREADY FILED nor OWNED. ONE cause must be closed before submitting. Adjacent OPEN issues cover neighbouring ground for this pairing but do not carry this check: 20261111T111216Z-cloudformation-elasticbeanstalk-2-net-new-causes-elastic-beanstalk-in-place.md (in-place update / retained replacement of Beanstalk resources) and 20261109T144630Z-cloudformation-elasticbeanstalk-stack-outputs-omit-outputvalue-a-stack.md (stack Outputs serialization). The same cause is also grounded from the elasticbeanstalk side in the issue filed for that service; this entry is the cloudformation__elasticbeanstalk grounding.

Cause 1 of 1: the CloudFormation provisioner completes AWS::ElasticBeanstalk::Environment before the environment has launched, so the stack reports CREATE_COMPLETE while Elastic Beanstalk still reports the environment Status='Launching'.
AWS: Smithy (elasticbeanstalk): EnvironmentDescription.Status documents 'Launching: Environment is in the process of initial deployment' and 'Ready: Environment is available to have an action performed on it, such as update or terminate'. Recorded behavior: CloudFormation marks AWS::ElasticBeanstalk::Environment CREATE_COMPLETE only after the environment has finished its initial deployment, so at the instant the stack reaches CREATE_COMPLETE an independent eb.describe_environments of the stack's environment reports Status='Ready', with its SolutionStackName and the EndpointURL the stack also publishes as an Output.
Emulator: the stack reaches CREATE_COMPLETE, and the environment it created reads back Status='Launching' (its SolutionStackName and EndpointURL are already populated) — the environment never settles to Ready afterwards either. Hypothesis: the Beanstalk environment resource provisioner reports the resource complete right after CreateEnvironment returns, without driving or waiting on the environment's Launching->Ready transition.
Why: CREATE_COMPLETE is the only signal a stack consumer has that the resources are usable. A client that proceeds on it — deploying a version into the environment, updating its settings, terminating it, or waiting for the Terminated tombstone after delete_stack — acts on an environment the service itself reports mid-launch, so those calls are refused by state guards or never settle, and a stack whose status says success is not usable.
Fix: the CloudFormation resource provisioner for AWS::ElasticBeanstalk::Environment — the resource must not be signalled CREATE_COMPLETE until DescribeEnvironments reports the environment Ready, and the environment state machine must advance for stack-created environments exactly as for API-created ones (the CloudFormation Beanstalk resource handler plus the elasticbeanstalk environment state behind CreateEnvironment, services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/src/ops/create_environment.rs and the service's environment state module).
Traces: cloudformation__elasticbeanstalk.deploy-elasticbeanstalk-environment-round-trip-and-delete.

## Evidence
Stack with AWS::ElasticBeanstalk::Application + AWS::ElasticBeanstalk::Environment reached CREATE_COMPLETE and its Refs read back correctly; the independent read then diverged: expected (Status, SolutionStackName, EndpointURL) == ('Ready', '<the stack's solution stack>', <the stack's EndpointURL output>); got AssertionError "environment reads ('Launching', '64bit Amazon Linux 2023 v4.7.0 running Python 3.14', '127.0.0.1:39253/_aws/elasticbeanstalk/us-west-2/s2r-ebe-6328c218e13e-8d5d6a5f2f')" (check_cloudformation__elasticbeanstalk_deploy_elasticbeanstalk_environment_round_trip_and_delete.py:288). The only diverging element is the Status; the solution stack string, the generated environment name and the 127.0.0.1:<port> address are per-run values.

## Steps to Reproduce
1. Boot the emulator and point AWS clients at it (region us-west-2).
2. cfn.create_stack with AWS::ElasticBeanstalk::Application and AWS::ElasticBeanstalk::Environment (SolutionStackName from eb.list_available_solution_stacks, SingleInstance option settings, Capabilities=['CAPABILITY_IAM']) and Outputs for ApplicationName/EnvironmentName/EndpointURL.
3. Poll cfn.describe_stacks until StackStatus == 'CREATE_COMPLETE'.
4. eb.describe_environments(ApplicationName=<app>) -> the one environment reports Status='Launching' instead of 'Ready' (and keeps reporting it).
5. Re-replay the check listed in trace_ids.
