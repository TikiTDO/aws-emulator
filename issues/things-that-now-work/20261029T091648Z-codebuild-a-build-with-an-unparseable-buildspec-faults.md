Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild.codebuild-invalid-buildspec-yaml-rejected-before-the-build-phase-runs
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild.codebuild-invalid-buildspec-yaml-rejected-before-the-build-phase-runs|run#13|AssertionError|assert bad["buildStatus"] == "FAILED", (
Issue-Commit: 574d6c6b58670e36287ce665233caf9a4430399b

# CodeBuild: a build with an unparseable buildspec FAULTs in PROVISIONING instead of FAILING in DOWNLOAD_SOURCE with the YAML parser diagnostic (1 cause)

> _Note: this issue was identified against the code base at commit `574d6c6b58670e36287ce665233caf9a4430399b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetBuilds

## Description
Cause 1 of 1: StartBuild with a buildspecOverride that is not parseable YAML (tab characters used as indentation) must be accepted by the control plane and then rejected by the build itself during source/buildspec processing. The emulator never gets that far: the build terminates in PROVISIONING with the generic FAULT status, so the buildspec is never parsed at all and the parse refusal is unobservable.

AWS: Smithy: BatchGetBuilds -> Build.buildStatus and Build.phases[].phaseStatus both target com.amazonaws.codebuild#StatusType, whose values are SUCCEEDED | FAILED | FAULT | TIMED_OUT | IN_PROGRESS | STOPPED; the model documents FAILED as "The build phase failed" and FAULT as "The build phase faulted" (an infrastructure fault), i.e. they are distinct outcomes and not interchangeable. Build.phases[].phaseType targets BuildPhaseType = SUBMITTED | QUEUED | PROVISIONING | DOWNLOAD_SOURCE | INSTALL | PRE_BUILD | BUILD | POST_BUILD | UPLOAD_ARTIFACTS | FINALIZING | COMPLETED, and BuildPhase.contexts (PhaseContexts of PhaseContext{statusCode,message}) is documented as "Additional information about a build phase, especially to help troubleshoot a failed build." The per-phase placement of a buildspec parse refusal is recorded behavior only: the recording shows the build reaching DOWNLOAD_SOURCE, that phase alone carrying phaseStatus FAILED, the overall Build.buildStatus being FAILED (not FAULT), no BUILD phase element present at all, Build.exportedEnvironmentVariables absent/empty, and the failing phase's contexts[].message carrying the YAML parser's own diagnostic rather than a CodeBuild classification wrapper. The fixed text of that message is the libyaml-style parser text "found character that cannot start any token"; its variable part is the source position the parser reports for the offending character (here "at line 2", derived from where the tab appears in the submitted buildspec) — the line number is a value taken from the request's buildspec, not part of the template. Note that AWS does NOT wrap this in a YAML_FILE_ERROR statusCode for an unparseable document; weaker malformations (a scalar under `commands:`, or `commands: []`) are accepted by AWS and must not be turned into this failure.

Emulator: BatchGetBuilds reports the build as buildComplete with buildStatus 'FAULT' and phases = [('SUBMITTED','SUCCEEDED'), ('QUEUED','SUCCEEDED'), ('PROVISIONING','FAULT'), ('COMPLETED', None)]. There is no DOWNLOAD_SOURCE phase, so there is no phase whose phaseStatus is FAILED, no contexts entry carrying a parser diagnostic, and the build is indistinguishable from an infrastructure fault. The positive-control build on the same project (well-formed buildspec, expected SUCCEEDED with an exported SIM2REAL_RESULT) is never reached by the check because the malformed-buildspec assertion fails first, but it terminates through the same PROVISIONING/FAULT path.

Why: a client cannot tell a user error from an emulator/infrastructure error. Code that branches on buildStatus — retrying FAULT as transient capacity loss while surfacing FAILED to the developer — retries forever on a buildspec typo that can never succeed. CI tooling and buildspec linters that read the failing phase's phaseType and contexts[].message to point at the offending line get nothing to report, and anything asserting that an unparseable buildspec never enters BUILD cannot distinguish "BUILD was skipped" from "the build never left PROVISIONING".

Fix: the build execution/state-machine component for CodeBuild builds — the code that advances a started build through the BuildPhaseType sequence and computes the terminal Build.buildStatus, plus the buildspec parsing step that must run in DOWNLOAD_SOURCE and attach a PhaseContext to the phase it fails. The build must progress past PROVISIONING, parse the effective buildspec (buildspecOverride when supplied, otherwise the project's source.buildspec), and on a parse error set that phase's phaseStatus to FAILED with the parser message in contexts, set buildStatus to FAILED, emit no BUILD phase and no exportedEnvironmentVariables. PROVISIONING/FAULT must be reserved for actual provisioning faults.

Note: the open issue 20261022T172625Z-codebuild-builds-never-execute-every-build-faults-at.md describes the same PROVISIONING FAULT family and lists this check among its trace ids, but the suite did not report this check as owned by it (its recorded replay key for this check does not match the frame this replay failed on), so it is filed here as a surviving cause rather than skipped. Closing that issue should be verified against this check explicitly.

## Evidence
Check: codebuild.codebuild-invalid-buildspec-yaml-rejected-before-the-build-phase-runs [mismatch]

Request shape: CreateProject(source={type: NO_SOURCE, buildspec: <well-formed 0.2 spec>}, artifacts={type: NO_ARTIFACTS}, cache={type: NO_CACHE}, environment={type: LINUX_CONTAINER, image: aws/codebuild/standard:7.0, computeType: BUILD_GENERAL1_SMALL}, logsConfig all DISABLED, timeoutInMinutes=5), then StartBuild(projectName=<project>, buildspecOverride="version: 0.2\nphases:\n\tbuild:\n\t\tcommands:\n\t\t\t- echo tab-indented-and-unparseable\n"), then BatchGetBuilds(ids=[<build id>]) polled to buildComplete.

Expected (AWS): builds[0].buildStatus == "FAILED"; exactly one phase with phaseStatus == "FAILED" and that phase's phaseType == "DOWNLOAD_SOURCE"; that phase's contexts[].message contains the parser text "cannot start any token" (AWS form: "found character that cannot start any token at line <N>", <N> from the submitted buildspec); "BUILD" absent from the phaseType list; exportedEnvironmentVariables in (None, []).

Actual (emulator): builds[0].buildStatus == "FAULT"; phases == [("SUBMITTED","SUCCEEDED"), ("QUEUED","SUCCEEDED"), ("PROVISIONING","FAULT"), ("COMPLETED", None)]. No DOWNLOAD_SOURCE phase, no FAILED phase, no PhaseContext message.

Assertion: AssertionError: a malformed buildspec must fail the build, got 'FAULT'; phases=[('SUBMITTED', 'SUCCEEDED'), ('QUEUED', 'SUCCEEDED'), ('PROVISIONING', 'FAULT'), ('COMPLETED', None)]
(check_codebuild_codebuild_invalid_buildspec_yaml_rejected_before_the_build_phase_runs.py line 204)

No error code or HTTP status diverges here: every API call returned 200 and the divergence is in the modeled StatusType/BuildPhaseType values BatchGetBuilds reports.

## Steps to Reproduce
1. CreateProject with source={"type":"NO_SOURCE","buildspec": a well-formed version 0.2 buildspec}, artifacts={"type":"NO_ARTIFACTS"}, environment={"type":"LINUX_CONTAINER","image":"aws/codebuild/standard:7.0","computeType":"BUILD_GENERAL1_SMALL"}, a CodeBuild-assumable serviceRole, timeoutInMinutes=5.
2. StartBuild(projectName=<that project>, buildspecOverride="version: 0.2\nphases:\n\tbuild:\n\t\tcommands:\n\t\t\t- echo tab-indented-and-unparseable\n") — tabs are illegal as YAML indentation, so the document is unparseable.
3. Poll BatchGetBuilds(ids=[<build id>]) until buildComplete is true.
4. Observe buildStatus == "FAULT" and phases ending at PROVISIONING/FAULT, instead of buildStatus == "FAILED" with DOWNLOAD_SOURCE as the sole FAILED phase carrying the YAML parser diagnostic in contexts[].message, no BUILD phase and no exportedEnvironmentVariables.
