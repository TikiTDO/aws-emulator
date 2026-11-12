Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticbeanstalk.rebuild-environment-cycles-through-launching-to-ready elasticbeanstalk.restart-app-server-unknown-env-must-fail elasticbeanstalk.environment-info-log-payload-preserved elasticbeanstalk.deploy-cfn-application-version-behaves elasticbeanstalk.create-environment-duplicate-name-rejected
Issue-Shape-Id: com.amazonaws.elasticbeanstalk#AWSElasticBeanstalkService
Issue-Key: elasticbeanstalk.rebuild-environment-cycles-through-launching-to-ready|_wait_for_env_status#8|Timeout|time.sleep(interval_s)
Issue-Key: elasticbeanstalk.restart-app-server-unknown-env-must-fail|_poll_until_seen#14|Timeout|time.sleep(interval_s)
Issue-Key: elasticbeanstalk.environment-info-log-payload-preserved|_poll_for_marker_in_tail#20|Timeout|time.sleep(15)
Issue-Key: elasticbeanstalk.deploy-cfn-application-version-behaves|_poll_http_body#29|Timeout|time.sleep(interval_s)
Issue-Key: elasticbeanstalk.create-environment-duplicate-name-rejected|_wait_env_status#29|Timeout|time.sleep(interval_s)
Issue-Commit: 9ba218dfac78ec8aaaf32d0b8457457c6bd04b91

# Elastic Beanstalk lifecycle and log-retrieval operations are accepted but produce no observable effect, so the checks never finish (5 causes)

> _Note: this issue was identified against the code base at commit `9ba218dfac78ec8aaaf32d0b8457457c6bd04b91`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RebuildEnvironment

## Description
FIVE distinct root causes in the elasticbeanstalk emulator, all in the same service, all grounded in failing behavioral checks of this suite. Each of these checks is killed at the harness's 60s per-check budget while polling for a server-side effect the emulator never produces, so the replay reports 'Check timed out after 60.0s' rather than a frame. Adjacent open issues do NOT carry these checks (the suite did not mark them OWNED): 20261024T015947Z-elasticbeanstalk-elastic-beanstalk-application-version-environment-state-machines-never.md describes the same RebuildEnvironment and CloudFormation faults for a DIFFERENT set of checks, and 20261023T205650Z-elasticbeanstalk-elastic-beanstalk-control-plane-guards-are-not-enforced.md owns other ones. FIVE causes must be closed before submitting.

Cause 1 of 5: RebuildEnvironment is accepted but drives no environment transition.
AWS: Smithy: RebuildEnvironment(EnvironmentId | EnvironmentName) -> Unit, error InsufficientPrivilegesException (403). Recorded behavior: on a Ready SingleInstance environment the call is accepted AND the environment reports the transitional status 'Updating' before settling Ready again, on an instance set disjoint from the one it had before (DescribeEnvironmentResources.Instances changes).
Emulator: the call returns success and the environment never leaves Ready, so the check's 120s wait for 'Updating' never observes it and the check is killed at the 60s budget. Hypothesis: the handler validates and returns without scheduling the status transition the state module's clock-owed transitions already model for Launching/Terminating/Aborting.
Why: a client that rebuilds and waits for the environment to cycle waits forever, and cannot tell a rebuild happened at all.
Fix: the RebuildEnvironment handler and the environment status transition it must schedule (services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/src/ops/rebuild_environment.rs plus the clock-owed transitions in the service's state module).
Traces: elasticbeanstalk.rebuild-environment-cycles-through-launching-to-ready.

Cause 2 of 5: RestartAppServer is accepted but drives no environment transition and does not advance DateUpdated.
AWS: Smithy: RestartAppServer(EnvironmentId | EnvironmentName) -> Unit, no operation-specific errors. Recorded behavior: against a Ready environment the call puts the environment into 'Updating' and, once it settles Ready again, EnvironmentDescription.DateUpdated is strictly later than before the call.
Emulator: the positive-control leg of this check never observes 'Updating' (its 180s poll is cut short by the 60s harness budget), i.e. the restart is a no-op that only accepts the call. Hypothesis: as with Cause 1, no transition is scheduled and DateUpdated is not touched.
Why: a deployment pipeline that restarts the app server and waits for the environment to cycle hangs; nothing distinguishes an accepted restart from an ignored one.
Fix: the RestartAppServer handler and the Updating -> Ready transition plus DateUpdated bump it must write (src/ops/restart_app_server.rs and the service's state module).
Traces: elasticbeanstalk.restart-app-server-unknown-env-must-fail (the check's negative leg, an unknown EnvironmentName, is about the refusal; it is the positive control that hangs).

Cause 3 of 5: the RetrieveEnvironmentInfo 'tail' bundle never carries the deployed application's own log output.
AWS: Smithy: RequestEnvironmentInfo(InfoType) then RetrieveEnvironmentInfo -> EnvironmentInfoDescription{Message} (the S3 URL of the compiled information; "The files are deleted after 15 minutes"). Recorded behavior: the tail bundle compiled for an environment running the deployed application contains that application's own stdout lines byte-identically, including non-ASCII text (no truncation, no mojibake).
Emulator: the requested marker line never appears in any downloaded tail bundle; the check retries (6 attempts) and is killed at the 60s budget. Hypothesis: the compiled information is synthesized from service-side state only and never includes application output.
Why: log-retrieval clients (and anything diagnosing a deployment from the tail) get a bundle that cannot contain what they are looking for, and an encoding regression in the path would be undetectable.
Fix: the environment-information compile that RequestEnvironmentInfo starts and RetrieveEnvironmentInfo reports (src/ops/request_environment_info.rs, src/ops/retrieve_environment_info.rs and the environment-information state it builds), which must include the deployed application's log lines verbatim.
Traces: elasticbeanstalk.environment-info-log-payload-preserved.

Cause 4 of 5: an AWS::ElasticBeanstalk::ApplicationVersion created through CloudFormation is not deployable to an environment.
AWS: recorded behavior only — a CREATE_COMPLETE stack containing AWS::ElasticBeanstalk::Application / ::ApplicationVersion / ::Environment leaves the version visible to DescribeApplicationVersions and deployable, with the environment reporting that exact VersionLabel and serving the content baked into the bundle.
Emulator: the check never gets past waiting on the CloudFormation-provisioned Elastic Beanstalk resources and is killed at the 60s budget. Hypothesis: the AWS::ElasticBeanstalk::* resource providers do not write Elastic Beanstalk state (the same fault the open state-machines issue records for sibling CFN checks), so nothing the stack claims to have created exists.
Why: every infrastructure-as-code path to Elastic Beanstalk produces a stack that looks successful while no EB resource exists, so every follow-up EB call fails.
Fix: the CloudFormation resource providers for the AWS::ElasticBeanstalk::* types and the Elastic Beanstalk state they must write.
Traces: elasticbeanstalk.deploy-cfn-application-version-behaves.

Cause 5 of 5: the CreateEnvironment duplicate-name lifecycle never completes inside the per-check budget.
AWS: Smithy: CreateEnvironment(ApplicationName required, EnvironmentName) -> EnvironmentDescription. Recorded behavior: a second CreateEnvironment reusing the EnvironmentName of a live (not yet Terminated) environment is refused synchronously with InvalidParameterValue / HTTP 400 and the message template 'Environment <EnvironmentName> already exists.'; once that environment reaches Terminated the identical call succeeds with a NEW EnvironmentId.
Emulator: the check is killed at the 60s harness budget with no assertion frame, so the refusal/restore sequence (create -> Ready -> duplicate refusal -> Terminate -> Terminated -> re-create) is never completed. Hypothesis: one of the three environment-status waits in that sequence never observes its target status; the emulator's status changes are materialised lazily when a later call observes them, so a status that is never materialised reads as a stall. This cause is the weakest-grounded of the five: the only hard evidence is that no assertion fired before the budget — the fixer should re-replay this check first and, if it then fails on an assertion instead, treat that assertion as the real cause.
Why: a client that reuses an environment name after terminating one cannot complete the sequence AWS supports, and the name-reuse guard is unverified.
Fix: the environment status transitions behind CreateEnvironment / TerminateEnvironment and the duplicate-name guard in src/ops/create_environment.rs (plus the clock-owed transitions in the service's state module).
Traces: elasticbeanstalk.create-environment-duplicate-name-rejected.

## Evidence
Cause 1: expected the environment to report 'Updating' after eb.rebuild_environment and then settle Ready on a new instance; got no transition and 'Error: Check timed out after 60.0s' (the check's own wait for Updating is 120s, its settle wait 1200s).
Cause 2: expected RestartAppServer on a Ready environment to put it into 'Updating' and advance DateUpdated; got no transition, 'Error: Check timed out after 60.0s'.
Cause 3: expected the non-ASCII marker line logged by the deployed application to appear byte-identically in the RetrieveEnvironmentInfo tail bundle; it never appeared in any attempt, 'Error: Check timed out after 60.0s'.
Cause 4: expected the CFN-created application version to be deployable and the environment to serve its bundled content; got 'Error: Check timed out after 60.0s' while waiting on the stack's Elastic Beanstalk resources.
Cause 5: expected InvalidParameterValue 'Environment <name> already exists.' for the duplicate name and a new EnvironmentId after termination; got 'Error: Check timed out after 60.0s' with no assertion frame.

## Steps to Reproduce
1. Boot the emulator and point an AWS client at it (region us-west-2).
2. Cause 1: create an application and a SingleInstance environment, wait Ready, eb.rebuild_environment(EnvironmentName) then poll eb.describe_environments -> Status never leaves Ready.
3. Cause 2: same environment, eb.restart_app_server(EnvironmentName) then poll eb.describe_environments -> Status never leaves Ready and DateUpdated does not advance.
4. Cause 3: deploy a version whose app.js logs a non-ASCII marker, wait Ready, eb.request_environment_info(InfoType='tail'), then eb.retrieve_environment_info(InfoType='tail') and download the reported bundle -> the marker is absent.
5. Cause 4: deploy a CloudFormation stack with AWS::ElasticBeanstalk::Application, ::ApplicationVersion and ::Environment, wait CREATE_COMPLETE, then eb.describe_application_versions / eb.describe_environments and fetch the environment CNAME -> the version is not deployable and the environment does not serve the bundle.
6. Cause 5: create an environment, wait Ready, call eb.create_environment again with the same EnvironmentName, then terminate, wait Terminated and re-create with the same name.
7. Re-replay the checks listed in trace_ids.
