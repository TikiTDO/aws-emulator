Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-quicksetup-not-found-error-paths
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-quicksetup-not-found-error-paths ssm-quicksetup-validation-error-paths ssm-quicksetup-catalog-list-quick-setup-types
Issue-Shape-Id: com.amazonaws.ssmquicksetup#QuickSetup
Issue-Key: ssm-quicksetup|GetConfiguration|4xx->4xx|$.Message
Issue-Key: ssm-quicksetup|GetConfiguration|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: ssm-quicksetup|GetConfiguration|4xx->4xx|status
Issue-Key: ssm-quicksetup|UpdateServiceSettings|4xx->4xx|$.message
Issue-Key: ssm-quicksetup|ListQuickSetupTypes|2xx->2xx|$.QuickSetupTypeList[*]
Issue-Commit: 78295efc92da5c3ab6dc723b39af1e4944870efd

# ssm-quicksetup payload defects: GetConfiguration rejection class, UpdateServiceSettings validation body, stale type catalog

> _Note: this issue was identified against the code base at commit `78295efc92da5c3ab6dc723b39af1e4944870efd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetConfiguration

## Description
Three ssm-quicksetup responses carry a body the recorded service never returns: a rejection raised by the wrong stage with the wrong class and sentence, a validation response with no message member at all, and a catalog listing with missing entries and stale versions. The open issue 20261022T034519Z-ssm-quicksetup-error-paths-wrong-rejection-class-unvalidated.md covers other ssm-quicksetup error paths and none of these three operations. 3 causes; all must be closed.

Cause 1 of 3: GetConfiguration treats an id it cannot resolve as not-found instead of validating the id's format first.
AWS: GET /getConfiguration/{ConfigurationId} with a configuration id that is not in the service's id form is rejected by input validation with status 400 and x-amzn-errortype: ValidationException (Smithy: GetConfigurationInput.ConfigurationId is @httpLabel @required with pattern ^[a-zA-Z0-9-_/:]{1,100}$; GetConfiguration binds ValidationException httpError 400 and ResourceNotFoundException httpError 404, and ValidationException carries member Message). The recorded body is the modeled $.Message built from the request by interpolating the rejected id into the fixed template 'Configuration id <ConfigurationId> has invalid format.' - the fixed text is 'Configuration id ' + ' has invalid format.' and the only variable part is the id taken from the request path. The recording establishes the ordering: format validation of the id runs BEFORE any store lookup, so a wrongly shaped id never reaches the not-found path.
Emulator: status 404 with x-amzn-errortype: ResourceNotFoundException and $.Message 'no configuration with ID <id> exists' - a locally invented sentence from the not-found path, reached because the handler looks the id up without first checking its form.
Why: 400 is a permanent client-input fault and 404 is a resource-state fault. A client handed a malformed or foreign configuration id is told the configuration does not exist, so it retries/polls or creates instead of fixing the identifier, and anything matching on the service's own validation sentence never sees it.
Fix: GetConfiguration input validation in the ssm-quicksetup service crate - validate ConfigurationId's format and raise ValidationException with the invalid-format template before the configuration lookup in state.
Traces: ssm-quicksetup-not-found-error-paths#task#5

Cause 2 of 3: UpdateServiceSettings rejects an invalid role ARN with an empty body - the validation message is missing entirely.
AWS: PUT /serviceSettings with ExplorerEnablingRoleArn that is not an IAM role ARN answers status 400 with the front-door validation body in the lowercase member $.message, built from the request: '1 validation error detected: Value '<rejected value>' at '<member path>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>'. Fixed text is the '1 validation error detected: Value ... at ... failed to satisfy constraint: Member must satisfy regular expression pattern: ' frame; the variable parts are the rejected value and the member path, both from the request, and the IAM-role-ARN pattern, rendered BARE (no ^ / $ anchors) as arn:aws(-cn|-us-gov|-iso(-[a-z])?|-eusc)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+.* (Smithy: UpdateServiceSettingsInput.ExplorerEnablingRoleArn targets IAMRoleArn, an @arnReference to AWS::IAM::Role; UpdateServiceSettings binds ValidationException httpError 400).
Emulator: the status 400 is right, but no $.message member is emitted at all - the response carries no validation text for the client to read.
Why: the client learns only that something was invalid; it cannot tell which member failed or why, which is exactly what this message exists to convey, and SDK error-message surfacing comes back empty.
Fix: the ValidationException/front-door message renderer used by UpdateServiceSettings' input validation in the ssm-quicksetup service crate - it must populate the lowercase $.message member with the standard validation sentence for the ExplorerEnablingRoleArn member.
Traces: ssm-quicksetup-validation-error-paths#task#4

Cause 3 of 3: ListQuickSetupTypes serves a catalog that omits types and reports stale LatestVersion values.
AWS: GET /listQuickSetupTypes returns 200 with QuickSetupTypeList, a list of QuickSetupTypeOutput {Type, LatestVersion} covering every Quick Setup configuration type the service supports, ordered by Type (recorded behavior only - the model constrains neither the membership nor the version strings). The recording's catalog includes AWSQuickSetupType-CWASetup and AWSQuickSetupType-JITNA and reports a later LatestVersion for every type it shares with the emulator.
Emulator: 200 with a list of the same length but different membership - CWASetup and JITNA are absent (the list is padded at the tail instead), which shifts every entry from index 2 on, and every shared type reports an older LatestVersion than the recording (for example CFGCPacks and DevOpsGuru report 1.1 and 1.0 where the recording has 3.0 and 3.0, PatchPolicy 2.0 where the recording has 2.6). The defect is a hard-coded catalog table that is both incomplete and version-stale, not a per-run value.
Why: this operation is how a client discovers which configuration types it may create and which type version is current. A client that enumerates the catalog cannot create a CWASetup or JITNA manager at all, and one that pins TypeVersion from LatestVersion pins a version AWS no longer treats as latest - so a subsequent CreateConfigurationManager/UpdateConfigurationDefinition is driven by a catalog AWS never served.
Fix: the ListQuickSetupTypes handler's catalog table in the ssm-quicksetup service crate - it must carry every supported type (including CWASetup and JITNA) with the service's current LatestVersion for each, serialized in Type order.
Traces: ssm-quicksetup-catalog-list-quick-setup-types#task#1

## Evidence
Cause 1 - ssm-quicksetup-not-found-error-paths#task#5, GetConfiguration: expected status 400, x-amzn-errortype 'ValidationException', $.Message 'Configuration id <ConfigurationId> has invalid format.' (recorded instance: 'Configuration id c68e30b1-b8b1-44a7-b92a-293f08358ef9 has invalid format.'); actual status 404, x-amzn-errortype 'ResourceNotFoundException', $.Message 'no configuration with ID c68e30b1-b8b1-44a7-b92a-293f08358ef9 exists'. Fixed text: 'Configuration id ' ... ' has invalid format.'; variable part: the id from the request path.
Cause 2 - ssm-quicksetup-validation-error-paths#task#4, UpdateServiceSettings: expected $.message "1 validation error detected: Value 'not-a-role-arn' at 'explorerEnablingRoleArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws(-cn|-us-gov|-iso(-[a-z])?|-eusc)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+.*"; actual: member absent (status 400 on both sides). The rejected value 'not-a-role-arn' and the member path 'explorerEnablingRoleArn' come from the request; the pattern is the modeled IAM-role-ARN pattern rendered without anchors.
Cause 3 - ssm-quicksetup-catalog-list-quick-setup-types#task#1, ListQuickSetupTypes: status 200 on both sides, but all 14 QuickSetupTypeList entries diverge. Expected $.QuickSetupTypeList[0..] = {CFGCPacks,3.0}, {CFGRecording,3.1}, {CWASetup,1.3}, {DHMC,2.1}, {DevOpsGuru,3.0}, {Distributor,4.1}, {JITNA,1.1}, {PatchPolicy,2.6}, {ResourceExplorer,2.2}, {SSM,3.0}, {SSMChangeMgr,2.2}, ...; actual {CFGCPacks,1.1}, {CFGRecording,1.2}, {DHMC,1.1}, {DevOpsGuru,1.0}, {Distributor,1.1}, {PatchPolicy,2.0}, {ResourceExplorer,1.1}, {SSM,1.0}, {SSMChangeMgr,1.1}, {SSMHostMgmt,2.0}, {SSMOpsCenter,1.1}, ... - i.e. the AWSQuickSetupType-CWASetup and AWSQuickSetupType-JITNA entries are missing and every shared type carries an older LatestVersion.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: GET /getConfiguration/{ConfigurationId} with an id that is not in the service's configuration-id form (fixture ssm-quicksetup-not-found-error-paths, step task#5). The emulator answers 404 ResourceNotFoundException with 'no configuration with ID ... exists' instead of 400 ValidationException with 'Configuration id ... has invalid format.'.
3. Cause 2: PUT /serviceSettings with {"ExplorerEnablingRoleArn": "not-a-role-arn"} (fixture ssm-quicksetup-validation-error-paths, step task#4). The 400 response carries no $.message member.
4. Cause 3: GET /listQuickSetupTypes (fixture ssm-quicksetup-catalog-list-quick-setup-types, step task#1). Compare QuickSetupTypeList against the recording: CWASetup and JITNA are missing and the LatestVersion of every shared type is older.
