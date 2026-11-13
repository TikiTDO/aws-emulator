Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy.appspec-naming-nonexistent-alias-rejected-and-alias-not-created codedeploy.deployment-rollback-restores-alias-and-records-both-deployments codedeploy.deployment-service-role-without-lambda-permission-denied-and-alias-unmoved codedeploy.ignore-poll-alarm-failure-decides-whether-an-unpollable-alarm-stops-the-deployment
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy.appspec-naming-nonexistent-alias-rejected-and-alias-not-created|run#24|AssertionError|assert err.get("message") == expected_err_message, (
Issue-Key: codedeploy.deployment-rollback-restores-alias-and-records-both-deployments|run#27|AssertionError|assert reread.get("revision", {}).get("appSpecContent", {}).get("sha256") != \
Issue-Key: codedeploy.deployment-service-role-without-lambda-permission-denied-and-alias-unmoved|run#17|AssertionError|assert denied.get("status") == "Failed", (
Issue-Key: codedeploy.ignore-poll-alarm-failure-decides-whether-an-unpollable-alarm-stops-the-deployment|_await_probe_refused#24|Timeout|time.sleep(2)
Issue-Commit: 43c7d03c349635b4ef0fa0dfa9d9ce4b91e8a4c8

# CodeDeploy deployment outcomes diverge: IAM_ROLE_PERMISSIONS never raised for an under-privileged service role, errorInformation message for INVALID_LAMBDA_FUNCTION is emulator-authored, and appSpecContent.sha256 is omitted (3 causes)

> _Note: this issue was identified against the code base at commit `43c7d03c349635b4ef0fa0dfa9d9ce4b91e8a4c8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDeployment

## Description
Three distinct net-new root causes in the codedeploy emulator's deployment execution/representation. All three are reached only AFTER a Lambda deployment runs, so they are behind (but not covered by) the open issues 20261107T224322Z-codedeploy-lambda-deployments-reach-succeeded-without-shifting-the.md and 20261029T180825Z-codedeploy-deployments-settle-to-succeeded-instantly-no-inprogress.md; on these four checks the alias DID move and the deployment DID reach a terminal state, so the divergences below are the emulator's own and none of them is an unimplemented stub.

=== Cause 1 of 3: the deployment-level errorInformation.message for INVALID_LAMBDA_FUNCTION is emulator-authored prose, not AWS's sentence ===
AWS: Smithy (com.amazonaws.codedeploy) models DeploymentInfo.errorInformation as ErrorInformation{code: ErrorCode, message: ErrorMessage}; ErrorCode includes INVALID_LAMBDA_FUNCTION. The emulator already picks the right code. Recorded behavior: for a Lambda deployment whose well-formed AppSpec names an alias that does not exist on the target function, real CodeDeploy reports errorInformation.message as the fixed sentence "The deployment failed because either the target Lambda function <functionName> does not exist or the specified function version or alias cannot be found" — the only variable part is <functionName>, taken from the AppSpec's Resources entry (the Name/the resource key) of the request. AWS deliberately does NOT disclose which of the three (function, version, alias) was missing, and does not name the alias.
Emulator: returns errorInformation.message = "The alias <aliasName> specified in the AppSpec file does not exist on the Lambda function <functionName>" — a different sentence, naming the alias, with a different fixed text.
Why: operators and tooling match on this message (and on its deliberate ambiguity) when triaging a failed release; a re-authored sentence breaks log/alert matching and leaks a distinction AWS does not make.
Fix: the Lambda-platform deployment executor's error-information authoring — wherever the INVALID_LAMBDA_FUNCTION outcome is constructed for GetDeployment/BatchGetDeployments, the message template must be AWS's, with only the function name interpolated from the request's AppSpec.
Traces: codedeploy.appspec-naming-nonexistent-alias-rejected-and-alias-not-created. (Related but NOT covering: open issue 20261025T150007Z-codedeploy-a-lambda-deployment-whose-appspec-names-a-nonexistent.md is about never reaching Failed/INVALID_LAMBDA_FUNCTION at all; the terminal status and code are now correct and only the message diverges. Open issue 20261029T180825Z-codedeploy-rejection-paths-return-the-right-error-codes.md covers client-error (ClientError) messages on refusals, not this in-band deployment errorInformation.)

=== Cause 2 of 3: GetDeployment's revision.appSpecContent omits the modeled sha256 member ===
AWS: Smithy AppSpecContent has two members, content (RawStringContent) and sha256 (RawStringSha256, "The SHA256 hash value of the revision content"). AWS computes and stores the digest when a deployment is created with an AppSpecContent revision and echoes it on every read of that deployment's revision, so two deployments carrying different AppSpec content report different sha256 values.
Emulator: GetDeployment returns revision = {"revisionType": "AppSpecContent", "appSpecContent": {"content": "..."}} with no sha256 member at all, for every deployment; two deployments whose AppSpec content differs (a forward shift v1->v2 and its reverse v2->v1) are therefore indistinguishable by digest (both read absent/None).
Why: a client that identifies or de-duplicates a revision by its digest — the documented, cheap way to tell two revisions apart without re-parsing the AppSpec — cannot distinguish any two revisions, and a deserializer expecting the modeled member sees it missing.
Fix: the CreateDeployment handler's revision state (compute the SHA256 of the AppSpecContent content at create time and store it) plus the DeploymentInfo/revision serializer shared by GetDeployment, BatchGetDeployments and GetApplicationRevision. Note the fixed issue 20261030T003246Z-codedeploy-successful-responses-omit-modeled-output-members-3.md covered other omitted output members; this member is still absent.
Traces: codedeploy.deployment-rollback-restores-alias-and-records-both-deployments.

=== Cause 3 of 3: a deployment whose service role lacks the required permissions Succeeds instead of Failing with IAM_ROLE_PERMISSIONS ===
AWS: ErrorCode includes IAM_ROLE_PERMISSIONS. Recorded behavior: CreateDeploymentGroup and CreateDeployment both ACCEPT an under-privileged service role (no 4xx); the denial surfaces in-band, as the deployment reaching terminal status Failed with errorInformation.code = IAM_ROLE_PERMISSIONS and errorInformation.message = the fixed sentence "The IAM role <serviceRoleArn> does not give you permission to perform operations in the following AWS service: <serviceName>. Contact your AWS administrator if you need help. If you are an AWS administrator, you can grant permissions to your users or groups by creating IAM policies." — the variable parts are the group's serviceRoleArn (from the deployment group's state) and the AWS service that could not be reached ("AWSLambda" for a role granting no lambda:* actions on a Lambda-platform deployment). The refused deployment must have no effect: the alias stays on its prior version with no AdditionalVersionWeights. This pre-flight authorization check also covers cloudwatch:DescribeAlarms whenever the group's alarmConfiguration.enabled is true, and ignorePollAlarmFailure does NOT excuse it — a group with ignorePollAlarmFailure=true is refused identically to one with false.
Emulator: the service role is accepted and never consulted. A Lambda deployment through a group whose serviceRoleArn grants codedeploy:*, cloudwatch:DescribeAlarms, s3:Get*/List*, sns:Publish and tag:GetResources but NO lambda:* actions reached status 'Succeeded' with errorInformation = None (expected Failed / IAM_ROLE_PERMISSIONS). Likewise, with the role's cloudwatch:DescribeAlarms explicitly Denied, an alarm-enabled group's deployment is never refused: the alarm-flag check loops re-deploying a probe group waiting for the IAM_ROLE_PERMISSIONS refusal that never arrives and is killed by the 180s harness timeout.
Why: this is an overly-permissive divergence — the emulator reports a successful release for a role AWS would deny, so a client validating least-privilege service roles (or testing that a mis-scoped role is caught before production) gets a green deployment from a configuration real CodeDeploy refuses, and the two values of ignorePollAlarmFailure become indistinguishable.
Fix: the CreateDeployment/deployment-executor pre-flight authorization step for the group's serviceRoleArn (evaluate the role's attached policy against the actions the compute platform and the group's alarmConfiguration require), and the terminal-state/errorInformation authoring that must then mark the deployment Failed with IAM_ROLE_PERMISSIONS and AWS's message, while leaving the alias and its routing config untouched.
Traces: codedeploy.deployment-service-role-without-lambda-permission-denied-and-alias-unmoved, codedeploy.ignore-poll-alarm-failure-decides-whether-an-unpollable-alarm-stops-the-deployment.

## Evidence
Cause 1 — codedeploy.appspec-naming-nonexistent-alias-rejected-and-alias-not-created (mismatch):
  AssertionError: errorInformation message 'The alias ghost5c989476 specified in the AppSpec file does not exist on the Lambda function sim2real-behav-cd-fn-98f24385f69f', expected 'The deployment failed because either the target Lambda function sim2real-behav-cd-fn-98f24385f69f does not exist or the specified function version or alias cannot be found'
  (status Failed and errorInformation.code INVALID_LAMBDA_FUNCTION were correct; only the message diverges. The generated function name and the 'ghost...' alias name are per-run values filling the template, not requirements.)

Cause 2 — codedeploy.deployment-rollback-restores-alias-and-records-both-deployments (mismatch):
  AssertionError: the two deployments must record different AppSpec digests; both read {'revisionType': 'AppSpecContent', 'appSpecContent': {'content': '{"version": 0.0, "Resources": [{"sim2real-behav-cd-fn-603bc7e9370d": {"Type": "AWS::Lambda::Function", "Properties": {"Name": "sim2real-behav-cd-fn-603bc7e9370d", "Alias": "live", "CurrentVersion": "1", "TargetVersion": "2"}}}]}'}}
  Expected: appSpecContent.sha256 present on each deployment's revision, and the forward (1->2) and reverse (2->1) revisions' sha256 values differing. Actual: the member is absent on both, so the two digests compare equal.

Cause 3 — codedeploy.deployment-service-role-without-lambda-permission-denied-and-alias-unmoved (mismatch):
  AssertionError: a deployment whose service role grants no Lambda actions ended 'Succeeded', expected 'Failed' (errorInformation=None)
  Expected: status 'Failed', errorInformation.code 'IAM_ROLE_PERMISSIONS', errorInformation.message 'The IAM role <role_bad_arn> does not give you permission to perform operations in the following AWS service: AWSLambda. Contact your AWS administrator if you need help. If you are an AWS administrator, you can grant permissions to your users or groups by creating IAM policies.', and the alias still on its prior version with no AdditionalVersionWeights.
  Preconditions already verified by the check before the failure: GetDeploymentGroup echoed the unprivileged serviceRoleArn, and the positive control through a permissioned role Succeeded and moved the alias — so the service role is the only difference.

Cause 3 — codedeploy.ignore-poll-alarm-failure-decides-whether-an-unpollable-alarm-stops-the-deployment (mismatch):
  Error: Check timed out after 180.0s, inside _await_probe_refused, which re-creates a probe deployment until one ends Failed with errorInformation.code 'IAM_ROLE_PERMISSIONS' after cloudwatch:DescribeAlarms was Denied on the shared service role. The emulator never produces that refusal, so the loop never exits.

## Steps to Reproduce
Cause 1:
1. Create a Lambda function with versions 1,2,3 and an alias on version 1.
2. CreateApplication(computePlatform='Lambda'); CreateDeploymentGroup(deploymentConfigName='CodeDeployDefault.LambdaAllAtOnce').
3. CreateDeployment with an AppSpecContent revision whose Resources entry names an alias that does not exist on the function (CurrentVersion=2, TargetVersion=3).
4. Poll GetDeployment to a terminal state and read deploymentInfo.errorInformation.message. Observed: 'The alias <alias> specified in the AppSpec file does not exist on the Lambda function <fn>'. Expected AWS's sentence naming only the function.

Cause 2:
1. Same application/group. CreateDeployment with an AppSpecContent revision shifting the alias v1->v2; wait for Succeeded.
2. CreateDeployment a second, reverse revision v2->v1; wait for Succeeded.
3. GetDeployment on each and read revision.appSpecContent.sha256. Observed: the member is absent on both. Expected: two different SHA256 digests of the respective content strings.

Cause 3:
1. Create an IAM role CodeDeploy can assume whose policy allows codedeploy:*, cloudwatch:DescribeAlarms, s3:Get*/List*, sns:Publish, tag:GetResources and NO lambda:* action.
2. CreateApplication(computePlatform='Lambda') and CreateDeploymentGroup(..., serviceRoleArn=<that role>, deploymentConfigName='CodeDeployDefault.LambdaAllAtOnce'). Both are accepted, as on AWS.
3. CreateDeployment on that group with an AppSpecContent revision shifting the alias to a new version; poll GetDeployment to a terminal state. Observed: 'Succeeded', errorInformation absent. Expected: 'Failed' with errorInformation.code 'IAM_ROLE_PERMISSIONS' and AWS's message naming the role ARN and 'AWSLambda', with the alias unmoved and no weighted routing.
4. Alarm variant: create a CloudWatch alarm; CreateDeploymentConfig with a TimeBasedCanary routing config; create two groups on one shared role differing only in alarmConfiguration.ignorePollAlarmFailure (false/true), each with the alarm enabled. Rewrite the shared role's policy to Deny cloudwatch:DescribeAlarms/DescribeAlarmHistory, then CreateDeployment on either group. Observed: deployments keep reaching Succeeded, so a client waiting for the refusal hangs. Expected: both groups' deployments reach Failed with IAM_ROLE_PERMISSIONS regardless of ignorePollAlarmFailure, and succeed again once the grant is restored.
