Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__elasticbeanstalk.deploy-elasticbeanstalk-environment-invalid-solution-stack-rejected-and-rolled-back
Issue-Key: cloudformation__elasticbeanstalk.deploy-elasticbeanstalk-environment-invalid-solution-stack-rejected-and-rolled-back|run#13|AssertionError|assert got == _REJECTED, f"handler reason {got!r}, expected {_REJECTED!r}"
Issue-Commit: dff6e291e5e6489707888b0049fa49b367909ca3

# cloudformation__elasticbeanstalk: the CREATE_FAILED reason for an AWS::ElasticBeanstalk::Environment naming an unknown SolutionStackName omits AWS's service clause — no (Service: ElasticBeanstalk, Status Code: 400, Request ID: ...) (SDK Attempt Count: ...)

> _Note: this issue was identified against the code base at commit `dff6e291e5e6489707888b0049fa49b367909ca3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackEvents

## Description
ONE net-new root cause under the cloudformation__elasticbeanstalk suite heading, grounded in the single failing check of this heading that the replay marked neither ALREADY FILED nor OWNED. ONE cause must be closed before submitting.

Relationship to already-filed ground: the open issue 20261109T144630Z-cloudformation-elasticbeanstalk-stack-outputs-omit-outputvalue-a-stack.md names this same check in its Cause 3, but it carries that check under the replay key `run#11|AssertionError|assert status == "ROLLBACK_COMPLETE"` — i.e. the stack reaching CREATE_COMPLETE instead of rolling back. In THIS replay the stack DOES end ROLLBACK_COMPLETE and the Environment resource DOES go CREATE_FAILED with HandlerErrorCode InvalidRequest; the check now fails one assertion later, on the TEXT of the handler's message. That is a different cause (the refusal text, not the missing refusal) and it is not carried by any open issue's replay key, which is why the replay did not mark it OWNED. It is also not a reopening of a fixed issue: the operation is not refused, it answers and answers with the wrong message text.

Cause 1 of 1: a CloudFormation resource-handler failure relaying an Elastic Beanstalk refusal drops the AWS SDK service clause from the message.
AWS: Smithy (cloudformation): StackEvent.ResourceStatusReason — "Success/failure message associated with the resource." Recorded behavior: when a registry handler's downstream AWS call is refused, CloudFormation records the reason as `Resource handler returned message: "<downstream message> (Service: <ServiceName>, Status Code: <http status>, Request ID: <request id>) (SDK Attempt Count: <n>)" (RequestToken: <token>, HandlerErrorCode: <code>)`. For an AWS::ElasticBeanstalk::Environment whose SolutionStackName does not exist, the downstream message is Elastic Beanstalk's own `No Solution Stack named '<SolutionStackName>' found.` at HTTP 400, and the HandlerErrorCode is `InvalidRequest`. Fixed text of the requirement: the literal `Resource handler returned message: "` wrapper, the literal `No Solution Stack named '` ... `' found.`, and the literal SDK clause labels `(Service: `, `, Status Code: `, `, Request ID: `, `) (SDK Attempt Count: `, plus `(RequestToken: `, `, HandlerErrorCode: `. Variable parts, taken from the request/state and never from one recording: the SolutionStackName from the template, the service name (`ElasticBeanstalk`) and status code (`400`) of the refusing call, and the per-run request id, SDK attempt count and request token (the check masks exactly those three, so their values are not the requirement — their presence and labels are).
Emulator: the Environment resource reaches CREATE_FAILED with the right HandlerErrorCode (`InvalidRequest`) and the right downstream sentence, but the message stops there: the masked reason reads `No Solution Stack named '<SolutionStackName>' found.` with no `(Service: ElasticBeanstalk, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)` clause appended. Hypothesis: the resource provisioner formats the reason from the downstream error's message string alone instead of from the SDK service-exception rendering (service name + HTTP status + request id + attempt count).
Why: a client that classifies a failed deployment by parsing the resource event's reason — the documented way to learn WHY a stack rolled back — cannot tell which service refused, with what HTTP status, or which request to correlate in logs. Deployment gates, CI log scrapers and support tooling that match AWS's `(Service: ..., Status Code: ..., Request ID: ...)` clause see no match and report an unclassified failure, even though CloudFormation itself did the right thing. The same wrapper is used by every registry handler, so the defect is not specific to Elastic Beanstalk.
Fix: the reason-formatting path of the CloudFormation resource-provisioning/rollback engine — when a resource handler's downstream service call is refused, render the downstream error as the SDK does (message + `(Service: <sdk service name>, Status Code: <status>, Request ID: <id>) (SDK Attempt Count: <n>)`) before wrapping it in `Resource handler returned message: "..." (RequestToken: ..., HandlerErrorCode: ...)`. Belongs in the CloudFormation stack/resource provisioning module that writes StackEvent.ResourceStatusReason, not in the Elastic Beanstalk handler (which already produces the correct sentence and code).
Traces: cloudformation__elasticbeanstalk.deploy-elasticbeanstalk-environment-invalid-solution-stack-rejected-and-rolled-back.

## Evidence
Cause 1 — check cloudformation__elasticbeanstalk.deploy-elasticbeanstalk-environment-invalid-solution-stack-rejected-and-rolled-back, verdict mismatch.

The check creates a stack (Capabilities=['CAPABILITY_IAM']) containing a VPC/subnet/SG, an instance profile, an AWS::ElasticBeanstalk::Application and an AWS::ElasticBeanstalk::Environment whose SolutionStackName is the deliberately invalid 'sim2real-not-a-real-solution-stack'. It waits for a terminal stack status (ROLLBACK_COMPLETE — reached), then reads the single CREATE_FAILED StackEvent row of the 'Environment' logical id and parses its ResourceStatusReason with
  ^Resource handler returned message: "(?P<message>.*)" \(RequestToken: [0-9a-f-]+, HandlerErrorCode: (?P<code>[A-Za-z]+)\)$
masking only Request ID / RequestToken / SDK Attempt Count / account / run-generated names.

Expected (masked): ("No Solution Stack named 'sim2real-not-a-real-solution-stack' found. (Service: ElasticBeanstalk, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)", 'InvalidRequest')
Actual   (masked): ("No Solution Stack named 'sim2real-not-a-real-solution-stack' found.", 'InvalidRequest')

AssertionError at check_cloudformation__elasticbeanstalk_deploy_elasticbeanstalk_environment_invalid_solution_stack_rejected_and_rolled_back.py:286 — `assert got == _REJECTED, f"handler reason {got!r}, expected {_REJECTED!r}"`.

The only difference is the missing SDK service clause: the `Resource handler returned message: "..."` wrapper, the RequestToken, the HandlerErrorCode 'InvalidRequest' and the Elastic Beanstalk sentence are all correct. The solution-stack name shown is the check's fixed invalid input; the request id, attempt count and request token are per-run values the harness masks, so this is not a per-run-value difference.

## Steps to Reproduce
1. Boot the emulator and point cloudformation and elasticbeanstalk clients at it (region us-west-2).
2. cfn.create_stack(StackName=<name>, Capabilities=['CAPABILITY_IAM'], TemplateBody=<a template with a VPC, subnet, security group, instance profile, an AWS::ElasticBeanstalk::Application and an AWS::ElasticBeanstalk::Environment referencing it>) with the Environment's SolutionStackName set to a name Elastic Beanstalk does not list, e.g. 'sim2real-not-a-real-solution-stack'.
3. Poll cfn.describe_stacks(StackName=<stack id>) until terminal — the stack ends ROLLBACK_COMPLETE (correct).
4. cfn.describe_stack_events(StackName=<stack id>) and take the single row with LogicalResourceId 'Environment' and ResourceStatus 'CREATE_FAILED'; read ResourceStatusReason.
5. Observe the reason is `Resource handler returned message: "No Solution Stack named 'sim2real-not-a-real-solution-stack' found." (RequestToken: <token>, HandlerErrorCode: InvalidRequest)` — the inner message carries no `(Service: ElasticBeanstalk, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)` clause, which AWS appends.
6. Re-replay the check listed in trace_ids to confirm the fix.
