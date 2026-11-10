Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticbeanstalk.create-environment-invalid-solution-stack-must-fail elasticbeanstalk.update-environment-tier-change-rejected elasticbeanstalk.create-platform-version-malformed-source-bundle-must-fail elasticbeanstalk.describe-configuration-options-catalog-constraints-typed elasticbeanstalk.describe-configuration-options-for-solution-stack
Issue-Shape-Id: com.amazonaws.elasticbeanstalk#AWSElasticBeanstalkService
Issue-Key: elasticbeanstalk.create-environment-invalid-solution-stack-must-fail|_error_code#12|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Key: elasticbeanstalk.update-environment-tier-change-rejected|run#20|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: elasticbeanstalk.create-platform-version-malformed-source-bundle-must-fail|run#17|AssertionError|assert status == "Failed", (
Issue-Key: elasticbeanstalk.describe-configuration-options-catalog-constraints-typed|run#18|AssertionError|assert env_type["UserDefined"] is False, (
Issue-Key: elasticbeanstalk.describe-configuration-options-for-solution-stack|run#8|AssertionError|assert len(instance_type) == 1, "InstanceType option must be described for the stack"
Issue-Commit: 9ba218dfac78ec8aaaf32d0b8457457c6bd04b91

# Elastic Beanstalk accepts inputs AWS refuses and publishes an incomplete/mislabelled configuration-option catalog (5 causes)

> _Note: this issue was identified against the code base at commit `9ba218dfac78ec8aaaf32d0b8457457c6bd04b91`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEnvironment

## Description
FIVE distinct root causes in the elasticbeanstalk emulator, all in the same service, all grounded in failing behavioral checks of this suite. Causes 1-3 are overly-permissive paths (the emulator accepts or succeeds where AWS refuses or fails the build); causes 4-5 are the configuration-option catalog DescribeConfigurationOptions publishes. The open issue 20261024T015947Z-elasticbeanstalk-elastic-beanstalk-custom-platform-versions-are-not-registered.md covers ADJACENT platform-catalog ground but does not carry these checks (the suite did not mark them OWNED). FIVE causes must be closed before submitting.

Cause 1 of 5: CreateEnvironment accepts a SolutionStackName that does not exist.
AWS: Smithy: CreateEnvironment(ApplicationName required, SolutionStackName, PlatformArn, ...) -> EnvironmentDescription, errors InsufficientPrivilegesException (403) / TooManyEnvironmentsException (400); the solution-stack existence rule is service-side validation. Recorded behavior: a SolutionStackName that is not in ListAvailableSolutionStacks is refused synchronously with InvalidParameterValue / HTTP 400 and the message template "No Solution Stack named '<SolutionStackName>' found." (fixed text around the requested stack name), and NO environment is created.
Emulator: HTTP 200 with a full EnvironmentDescription — EnvironmentId minted, Status 'Launching', the bogus SolutionStackName echoed back — so a nonexistent platform silently becomes a live environment. A positive control in the same check shows CreateConfigurationTemplate accepts a real stack, so the emulator is not refusing everything; it validates nothing.
Why: the single most common platform typo is accepted; a client is told its environment is launching on a platform that cannot exist, and the error path it must handle on real AWS is never exercised.
Fix: SolutionStackName/PlatformArn validation against the published solution-stack catalog in the CreateEnvironment handler (services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/src/ops/create_environment.rs).
Traces: elasticbeanstalk.create-environment-invalid-solution-stack-must-fail.

Cause 2 of 5: UpdateEnvironment does not reject an environment-tier change, and a prior update leaves the environment stuck Updating so the tier call is refused for the wrong reason.
AWS: Smithy: UpdateEnvironment.Tier targets EnvironmentTier; recorded behavior: changing an existing environment's tier is refused with InvalidParameterValue / HTTP 400 and the message template "Unsupported environment tier <Tier.Name> version <Tier.Version> for solution stack '<SolutionStackName>'" — fixed text with the requested tier name, tier version and the environment's solution stack substituted.
Emulator: the tier change is never evaluated: the call is refused with 'Environment <name> is not in a state from which it can be updated. Current status: Updating.', i.e. the environment the check updated moments earlier has not returned to Ready, so the tier rule is masked by a state guard.
Why: a client that changes a tier by mistake gets no tier diagnosis at all; worse, the environment appears permanently busy after one UpdateEnvironment, so every subsequent update is refused.
Fix: tier-change validation in the UpdateEnvironment handler plus the Updating -> Ready transition it owes the clock (src/ops/update_environment.rs and the service's state module's clock-owed status transitions).
Traces: elasticbeanstalk.update-environment-tier-change-rejected.

Cause 3 of 5: CreatePlatformVersion reports a successful build for a definition bundle with no manifest.
AWS: Smithy: CreatePlatformVersion(PlatformName, PlatformVersion, PlatformDefinitionBundle required) -> CreatePlatformVersionResult{PlatformSummary, Builder}, errors ElasticBeanstalkServiceException (400), InsufficientPrivilegesException (403), TooManyPlatformsException (400); recorded behavior: the build of a bundle that carries no platform manifest FAILS — the platform version leaves PlatformStatus 'Creating' for a terminal failure status, never 'Ready'.
Emulator: the platform version reaches PlatformStatus 'Ready' for a bundle with no manifest, so a broken custom platform is published as usable.
Why: a client cannot detect a bad platform bundle at all; it then deploys environments onto a platform that could never have built on AWS.
Fix: source-bundle validation and the terminal failure branch of the platform-build state transition in the CreatePlatformVersion handler (src/ops/create_platform_version.rs plus the platform state it writes).
Traces: elasticbeanstalk.create-platform-version-malformed-source-bundle-must-fail.

Cause 4 of 5: DescribeConfigurationOptions marks platform-published options as user-defined.
AWS: Smithy: ConfigurationOptionDescription.UserDefined (UserDefinedOption, boolean) — "true: This configuration option was defined by the user ... false: This configuration was not defined by the user", with the constraint "You can remove only UserDefined options from a configuration". Recorded behavior: an option the platform itself publishes, e.g. Namespace 'aws:elasticbeanstalk:environment' / OptionName 'EnvironmentType', is reported with UserDefined=false.
Emulator: that option is reported with UserDefined=true.
Why: UserDefined is the flag a client reads to decide what it may pass in OptionsToRemove; marking platform options user-defined invites OptionsToRemove calls AWS rejects, and tooling that renders "your settings" vs "platform defaults" shows the wrong set.
Fix: the UserDefined derivation in the configuration-option catalog served by DescribeConfigurationOptions (src/ops/describe_configuration_options.rs and the option catalog in the service's state module).
Traces: elasticbeanstalk.describe-configuration-options-catalog-constraints-typed.

Cause 5 of 5: the configuration-option catalog for a solution stack omits the launch-configuration InstanceType option.
AWS: Smithy: DescribeConfigurationOptions(SolutionStackName | PlatformArn | ApplicationName+TemplateName | EnvironmentName) -> ConfigurationOptionsDescription{SolutionStackName, PlatformArn, Options}; recorded behavior: for a current solution stack the published Options include exactly one description for Namespace 'aws:autoscaling:launchconfiguration' / OptionName 'InstanceType' (with its ValueType and ChangeSeverity from the modelled closed sets).
Emulator: no InstanceType option description is present for the stack (the check finds zero matches where it requires one).
Why: console-like and IaC clients enumerate this catalog to validate and render option settings; an option they can legally set on an environment is missing from the catalog, so they reject valid configuration or cannot offer instance sizing at all.
Fix: the per-solution-stack option catalog DescribeConfigurationOptions serves (src/ops/describe_configuration_options.rs and the catalog data in the service's state module).
Traces: elasticbeanstalk.describe-configuration-options-for-solution-stack.

## Evidence
Cause 1: expected InvalidParameterValue/400 with "No Solution Stack named '64bit Definitely/Not A Real Solution Stack v0.0.0' found."; got HTTP 200 and {'EnvironmentName': 'sim2real-behav-eb-env-bad-de85619535a0', 'EnvironmentId': 'e-8446c2b1a5254', 'SolutionStackName': '64bit Definitely/Not A Real Solution Stack v0.0.0', 'Status': 'Launching', ...} (AssertionError: "expected a ClientError, but the call succeeded"). The ids/names are this run's generated values.
Cause 2: expected "Unsupported environment tier Worker version 1.0 for solution stack '64bit Amazon Linux 2023 v6.7.0 running Node.js 24'"; got 'Environment sim2real-tier-23c80060402f is not in a state from which it can be updated. Current status: Updating.'
Cause 3: AssertionError "platform version reached status 'Ready' for a bundle with no manifest; expected the build to fail, not report success".
Cause 4: AssertionError "EnvironmentType is published by the platform, so the reference catalog reports UserDefined=false, got True".
Cause 5: AssertionError "InstanceType option must be described for the stack" (len(instance_type) == 0 in the DescribeConfigurationOptions(SolutionStackName=...) response).

## Steps to Reproduce
1. Boot the emulator and point an AWS client at it (region us-west-2).
2. Cause 1: eb.create_application; eb.create_environment(ApplicationName, EnvironmentName, SolutionStackName='64bit Definitely/Not A Real Solution Stack v0.0.0') -> expect InvalidParameterValue/400, observe HTTP 200 with a Launching environment.
3. Cause 2: create an environment, wait Ready, eb.update_environment(OptionSettings=[...]), wait for it to settle Ready, then eb.update_environment(EnvironmentName, Tier={'Name':'Worker','Type':'SQS/HTTP','Version':'1.0'}) -> expect the unsupported-tier refusal.
4. Cause 3: eb.create_storage_location + s3.put_object of a zip with no platform manifest; eb.create_platform_version(PlatformName, PlatformVersion='1.0.0', PlatformDefinitionBundle=...) and poll eb.describe_platform_version -> observe PlatformStatus 'Ready'.
5. Cause 4/5: eb.describe_configuration_options(SolutionStackName=<a stack from eb.list_available_solution_stacks>) -> inspect the entry for aws:elasticbeanstalk:environment/EnvironmentType (UserDefined) and look for aws:autoscaling:launchconfiguration/InstanceType.
6. Re-replay the checks listed in trace_ids.
