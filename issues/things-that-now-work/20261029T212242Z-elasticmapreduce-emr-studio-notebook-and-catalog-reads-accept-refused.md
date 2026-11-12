Issue-Source: trace-sweep-divergence
Issue-Trace: elasticmapreduce-notebook-execution-error-paths
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticmapreduce-notebook-execution-error-paths elasticmapreduce-release-label-and-instance-type-catalog elasticmapreduce-run-job-flow-validation-errors emr-account-level-read-only-config emr-describe-job-flows-deprecated-error emr-describe-job-flows-filters emr-create-studio-invalid-network-error emr-create-studio-sso-missing-identity-center-error emr-studio-create-invalid-vpc-error emr-studio-create-mismatched-subnet-error emr-studio-lifecycle-minimal emr-studio-session-mapping-create-then-get-roundtrip
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: elasticmapreduce|DescribeNotebookExecution|4xx->4xx|$.Message
Issue-Key: elasticmapreduce|StopNotebookExecution|4xx->4xx|$.Message
Issue-Key: elasticmapreduce|DescribeReleaseLabel|2xx->2xx|$.Applications[Name="HCatalog"]
Issue-Key: elasticmapreduce|DescribeReleaseLabel|2xx->2xx|$.Applications[Name="JupyterHub"].Version
Issue-Key: elasticmapreduce|DescribeReleaseLabel|2xx->2xx|$.Applications[Name="MXNet"]
Issue-Key: elasticmapreduce|DescribeReleaseLabel|2xx->2xx|$.Applications[Name="Phoenix"].Version
Issue-Key: elasticmapreduce|DescribeReleaseLabel|2xx->2xx|$.Applications[Name="Sqoop"]
Issue-Key: elasticmapreduce|DescribeReleaseLabel|2xx->2xx|$.Applications[Name="TensorFlow"]
Issue-Key: elasticmapreduce|DescribeReleaseLabel|2xx->2xx|$.AvailableOSReleases[*]
Issue-Key: elasticmapreduce|RunJobFlow|4xx->2xx|$.__type
Issue-Key: elasticmapreduce|RunJobFlow|4xx->2xx|$.message
Issue-Key: elasticmapreduce|RunJobFlow|4xx->2xx|status
Issue-Key: elasticmapreduce|DescribeJobFlows|4xx->4xx|$.message
Issue-Key: elasticmapreduce|GetBlockPublicAccessConfiguration|2xx->2xx|$.BlockPublicAccessConfigurationMetadata.CreatedByArn
Issue-Key: elasticmapreduce|CreateStudio|4xx->4xx|$.Message
Issue-Key: elasticmapreduce|CreateStudio|4xx->4xx|$.__type
Issue-Key: elasticmapreduce|CreateStudio|4xx->4xx|$.message
Issue-Key: elasticmapreduce|CreateStudio|4xx->2xx|$.Message
Issue-Key: elasticmapreduce|CreateStudio|4xx->2xx|$.__type
Issue-Key: elasticmapreduce|CreateStudio|4xx->2xx|status
Issue-Key: elasticmapreduce|GetStudioSessionMapping|2xx->2xx|$.SessionMapping.IdentityName
Issue-Commit: 14bf84ae7f57c35d094a91bae03d265103651cef

# EMR Studio, notebook and catalog reads accept refused requests and serve stale or omitted members

> _Note: this issue was identified against the code base at commit `14bf84ae7f57c35d094a91bae03d265103651cef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStudio

## Description
This issue bundles 10 net-new root causes in the EMR (elasticmapreduce) crate on the Studio, notebook-execution, catalog and account-configuration surfaces. They share ONE fault: these handlers do not model AWS's response contract — two of them ACCEPT (HTTP 200) requests AWS refuses with HTTP 400, three serve response members that are stale, fabricated or omitted, and the rest word their refusal themselves. The two accept-where-AWS-refuses causes are the severe ones: a 200 where AWS returns 400 inverts the client's control flow entirely.

For each cause the fixed text / contract is the requirement; the concrete ids, labels and version strings in the recordings are per-run or per-catalog values, not part of the requirement.

--- Cause 1 of 10: DescribeNotebookExecution's not-found message is reworded and drops AWS's service-detail suffix ---
AWS: Smithy: DescribeNotebookExecution models `InvalidRequestException` (client, 400). Recorded behavior only for the text: an unknown NotebookExecutionId is refused 400 with `Execution does not exist (Service: AWSEditors; Status Code: 400; Error Code: InvalidRequestException; Request ID: <requestId>; Proxy: null)`. Fixed text: `Execution does not exist (Service: AWSEditors; Status Code: 400; Error Code: InvalidRequestException; Request ID: ` / `; Proxy: null)`. The ONLY variable part is the per-request request id (which differs every run and is not itself a requirement); note AWS does NOT echo the execution id.
Emulator: `Notebook execution id '<NotebookExecutionId>' is not valid or does not exist.` — nothing of AWS's sentence survives: no `Execution does not exist` phrase, no `Service: AWSEditors` attribution, and it echoes an id AWS omits.
Why: callers matching the sentence, or reading the embedded `Service:`/`Error Code:` labels to attribute the failure to the AWSEditors backend, get nothing usable.
Fix: the not-found branch of the DescribeNotebookExecution handler in the EMR crate's `src/ops/`.
Traces: elasticmapreduce-notebook-execution-error-paths#task#3.

--- Cause 2 of 10: StopNotebookExecution's not-found message is reworded the same way ---
AWS: Smithy: StopNotebookExecution models `InvalidRequestException` (client, 400). Recorded behavior only: 400 with the same `Execution does not exist (Service: AWSEditors; Status Code: 400; Error Code: InvalidRequestException; Request ID: <requestId>; Proxy: null)` template as Cause 1 — confirming the template is shared across the notebook-execution operations, so the fix is one helper.
Emulator: `Notebook execution id '<id>' is not valid or does not exist.`
Why / Fix: as Cause 1, reached through the StopNotebookExecution handler; fixing the shared helper closes both, but BOTH steps must be re-replayed.
Traces: elasticmapreduce-notebook-execution-error-paths#task#4.

--- Cause 3 of 10: DescribeReleaseLabel serves a stale, truncated application and OS-release catalog ---
AWS: Smithy: DescribeReleaseLabel output = {ReleaseLabel, Applications: SimplifiedApplicationList of SimplifiedApplication{Name, Version}, AvailableOSReleases: OSReleaseList of OSRelease{Label}, NextToken}. Recorded behavior for the content: for a given release label AWS returns the COMPLETE application set for that label with each application's real bundled version, and the complete list of Amazon Linux OS release labels available to it (the recording carries 50+ entries). A client enumerates these to decide what it may request in RunJobFlow.
Emulator: returns a truncated, frozen snapshot — whole applications are absent from `Applications` (HCatalog, MXNet, Sqoop, TensorFlow), present ones carry older versions (JupyterHub, Phoenix), and `AvailableOSReleases` holds two old labels where AWS returns the full ordered list. Hypothesis: the catalog is a hard-coded table captured once and never widened, rather than being derived per release label.
Why: a client that enumerates applications or OS releases to validate a cluster definition concludes an application or OS release is unavailable and either refuses a valid configuration or picks an obsolete one.
Fix: the release-label catalog data the DescribeReleaseLabel handler reads in the EMR crate (its static catalog/state table), which must cover the full application and OS-release set per label.
Traces: elasticmapreduce-release-label-and-instance-type-catalog#task#2.

--- Cause 4 of 10: RunJobFlow ACCEPTS an unknown ReleaseLabel that AWS refuses (200 instead of 400) ---
AWS: Smithy: RunJobFlow.ReleaseLabel is XmlStringMaxLen256 with no enum, so recorded behavior only: a release label that is not in EMR's catalog is refused with HTTP 400, `__type` = `ValidationException` and the message `The supplied release label is invalid: <ReleaseLabel>.` — fixed text `The supplied release label is invalid: ` plus a trailing period; variable part: the ReleaseLabel from the request. (Same wording ListSupportedInstanceTypes uses, and deliberately NOT DescribeReleaseLabel's constant.)
Emulator: returns HTTP 200 with a JobFlowId — it never validates ReleaseLabel against the catalog, so a cluster is created on a release that does not exist, with no `__type` and no `message` in the response at all.
Why: this inverts the client's control flow. Code that relies on RunJobFlow to reject a typo'd or unsupported release label proceeds as if the cluster is valid, and every later call against that JobFlowId behaves as though an impossible configuration were real; the error path is never exercised.
Fix: add the ReleaseLabel catalog check to the RunJobFlow handler's input validation, ahead of cluster creation, raising `ValidationException`/400.
Traces: elasticmapreduce-run-job-flow-validation-errors#task#7 (cluster also covers task#8 and task#9 of that fixture).

--- Cause 5 of 10: DescribeJobFlows' deprecation message is emulator-authored ---
AWS: Smithy: DescribeJobFlows is the deprecated job-flow read. Recorded behavior only for the text: AWS refuses it with HTTP 400 and the CONSTANT message `DescribeJobFlows API is deprecated. Please refer to http://docs.aws.amazon.com/ElasticMapReduce/latest/API/API_DescribeJobFlows.html for further information.` — no variable part; the documentation URL is part of the fixed text.
Emulator: `The DescribeJobFlows API is deprecated and no longer serves job flows. Use ListClusters, DescribeCluster, ListSteps, ListInstanceGroups and ListBootstrapActions instead.` — a reworded sentence that names replacement operations AWS does not name and omits the URL AWS does include.
Why: callers and operators matching AWS's deprecation sentence (or extracting the doc URL from it) do not recognise the refusal.
Fix: the deprecation refusal in the DescribeJobFlows handler.
Traces: emr-account-level-read-only-config#task#1 (cluster also covers emr-describe-job-flows-deprecated-error#task#1, emr-describe-job-flows-filters#task#1 and #task#2 — every DescribeJobFlows call in the corpus takes the same wrong text).

--- Cause 6 of 10: GetBlockPublicAccessConfiguration fabricates a CreatedByArn for a configuration no principal has written ---
AWS: Smithy: GetBlockPublicAccessConfiguration output carries `BlockPublicAccessConfigurationMetadata` = {CreationDateTime required, CreatedByArn required (ArnType @length 20..2048) "The Amazon Resource Name that created or last modified the configuration."}. Recorded behavior: for an account whose block-public-access configuration is still the service default — never written by `PutBlockPublicAccessConfiguration` — AWS returns `CreatedByArn` as the EMPTY STRING, because no principal created or last modified it. Hypothesis: AWS keeps the member present (it is modelled required) but leaves it empty rather than attributing the default to anyone.
Emulator: returns `arn:aws:iam::000000000000:root` — an ARN it synthesised for a configuration nobody wrote.
Why: a client reading `CreatedByArn` to decide whether block-public-access has ever been configured (empty = still default, non-empty = explicitly set) concludes it was explicitly set by the account root, and audit/compliance logic that attributes the setting to a principal attributes it to a principal that never acted.
Fix: the metadata the GetBlockPublicAccessConfiguration handler builds in the EMR crate — leave `CreatedByArn` empty until a Put has written the configuration, and set it to the writing principal thereafter.
Traces: emr-account-level-read-only-config#task#2.

--- Cause 7 of 10: CreateStudio's invalid-VpcId message is reworded and drops AWS's service-detail suffix ---
AWS: Smithy: CreateStudio(Name, AuthMode, VpcId, SubnetIds, ServiceRole, WorkspaceSecurityGroupId, EngineSecurityGroupId, DefaultS3Location all required), errors `InternalServerException` (500) and `InvalidRequestException` (client, 400). Recorded behavior only for the text: a VpcId that does not resolve is refused 400 with `VpcId is not valid. (Service: AWSEditors; Status Code: 400; Error Code: InvalidRequestException; Request ID: <requestId>; Proxy: null)`. Fixed text: `VpcId is not valid. (Service: AWSEditors; Status Code: 400; Error Code: InvalidRequestException; Request ID: ` / `; Proxy: null)` — AWS names the input MEMBER and does NOT echo the id; the only variable part is the per-request request id.
Emulator: `VpcId '<VpcId>' does not name a VPC in this account` — reworded, echoes the id AWS omits, drops the trailing period and the whole AWSEditors service-detail suffix.
Why: callers matching the sentence fail; the AWSEditors attribution that tells an operator which backend refused is lost.
Fix: the VpcId resolution branch of the CreateStudio handler.
Traces: emr-create-studio-invalid-network-error#task#1.

--- Cause 8 of 10: CreateStudio's missing-UserRole refusal has the wrong error type and no message (WRONG ERROR CODE) ---
AWS: Smithy: CreateStudio.AuthMode is the required `AuthMode` enum; `UserRole` is OPTIONAL in the model. Recorded behavior only: when AuthMode is SSO and UserRole is omitted, AWS refuses 400 with `__type` = `ValidationException` and the CONSTANT message `UserRole is required to create a studio with SSO auth-mode.` — no variable part.
Emulator: `__type` = `InvalidRequestException` with NO `message` member at all.
Why: wrong error type plus a missing message — a client branching on `ValidationException` (fix the request) takes the `InvalidRequestException` branch and cannot report which member is missing. The model cannot express this conditional requirement, which is exactly why the handler must encode it.
Fix: the AuthMode/UserRole conditional-requirement check in the CreateStudio handler's input validation, raising `ValidationException`.
Traces: emr-create-studio-sso-missing-identity-center-error#task#1.

--- Cause 9 of 10: CreateStudio ACCEPTS unresolvable VPC/subnet network input that AWS refuses (200 instead of 400) ---
AWS: Smithy: CreateStudio requires VpcId and SubnetIds and models `InvalidRequestException` (client, 400). Recorded behavior: a VpcId that does not exist, and a SubnetIds set whose subnets do not belong to the given VpcId, are each refused with HTTP 400 and `__type` = `InvalidRequestException` (the corpus does not assert the message body on these steps).
Emulator: returns HTTP 200 with a StudioId and Url — it creates the Studio without resolving VpcId or checking subnet/VPC membership, so no `__type` and no `Message` appear.
Why: inverts the client's control flow — a Studio is created on a network that cannot exist, and code that depends on CreateStudio to validate the network wiring proceeds with an unusable Studio instead of surfacing the error. Note this is a DIFFERENT defect from Cause 7: that one is the wording on a path that does refuse; this one is the paths that do not refuse at all (unresolvable VPC in one fixture, mismatched subnet in another), so Cause 7's fix does not close it.
Fix: add VpcId resolution and SubnetIds-belong-to-VpcId validation to the CreateStudio handler, ahead of Studio creation.
Traces: emr-studio-create-invalid-vpc-error#task#3 (cluster also covers emr-studio-create-mismatched-subnet-error#task#7 and emr-studio-lifecycle-minimal#task#8).

--- Cause 10 of 10: GetStudioSessionMapping omits SessionMapping.IdentityName ---
AWS: Smithy: GetStudioSessionMapping output = {SessionMapping: SessionMappingDetail}, and `SessionMappingDetail.IdentityName` (XmlStringMaxLen256) is documented "The name of the user or group." Recorded behavior: after CreateStudioSessionMapping is called with an IdentityName, a GetStudioSessionMapping read returns that `IdentityName` on the detail.
Emulator: returns 200 with a `SessionMapping` whose `IdentityName` member is absent, so the name supplied at create time is not persisted onto (or not serialised from) the stored mapping.
Why: a create-then-read round trip loses the identity's name. A client that lists or reconciles Studio session mappings by `IdentityName` — the only human-meaningful key, since `IdentityId` is a GUID — cannot match the mapping it just created.
Fix: the Studio session-mapping state and the GetStudioSessionMapping serializer in the EMR crate — persist `IdentityName` from CreateStudioSessionMapping and emit it on the detail.
Traces: emr-studio-session-mapping-create-then-get-roundtrip#task#15.

Note: the open issue "EMR accepts two requests AWS refuses (unknown ReleaseLabel, unresolvable Studio network) and omits two always-present response members" (20261029T175551Z-emr-accepts-two-requests-aws-refuses-unknown-releaselabel.md) describes an adjacent set from the behavioral suite and overlaps conceptually with Causes 4 and 9, but it carries no wire replay key for any of these fixture steps, and it does not cover Causes 1, 2, 3, 5, 6, 7, 8 or 10.

## Evidence
Replay of wire corpus simparity-wire-aws-v1 against the emulator (service token `elasticmapreduce`; 124/202 steps passed).

Cause 1 — elasticmapreduce-notebook-execution-error-paths#task#3, DescribeNotebookExecution, status 400 expected and actual: `$.Message: value_mismatch (expected 'Execution does not exist (Service: AWSEditors; Status Code: 400; Error Code: InvalidRequestException; Request ID: 5eb7e71a-56af-4e49-ac98-ea641c4c91f3; Proxy: null)', got "Notebook execution id 'ex-IZY8V1S8L2GRPARBQIGH1P5W1H2XX' is not valid or does not exist.")`. (Request ID is per-run.)
Cause 2 — elasticmapreduce-notebook-execution-error-paths#task#4, StopNotebookExecution: same expected template, got the same emulator sentence.
Cause 3 — elasticmapreduce-release-label-and-instance-type-catalog#task#2, DescribeReleaseLabel, status 200 expected and actual: `$.Applications[Name="HCatalog"]: missing`, `$.Applications[Name="MXNet"]: missing`, `$.Applications[Name="Sqoop"]: missing`, `$.Applications[Name="TensorFlow"]: missing`; `$.Applications[Name="JupyterHub"].Version: value_mismatch (expected '1.5.0', got '1.4.1')`; `$.Applications[Name="Phoenix"].Version: value_mismatch (expected '5.1.3', got '5.1.2')`; `$.AvailableOSReleases[0]..[50]` diverge — the emulator serves 2 labels where the recording carries 51.
Cause 4 — elasticmapreduce-run-job-flow-validation-errors#task#7, RunJobFlow: `status: expected 400, got 200`; `$.__type: missing (expected 'ValidationException', got None)`; `$.message: missing (expected 'The supplied release label is invalid: emr-0.0.0.', got None)`; affected_count 3 (task#7, task#8, task#9).
Cause 5 — emr-account-level-read-only-config#task#1, DescribeJobFlows, status 400 expected and actual: `$.message: value_mismatch (expected 'DescribeJobFlows API is deprecated. Please refer to http://docs.aws.amazon.com/ElasticMapReduce/latest/API/API_DescribeJobFlows.html for further information.', got 'The DescribeJobFlows API is deprecated and no longer serves job flows. Use ListClusters, DescribeCluster, ListSteps, ListInstanceGroups and ListBootstrapActions instead.')`; affected_count 4.
Cause 6 — emr-account-level-read-only-config#task#2, GetBlockPublicAccessConfiguration, status 200 expected and actual: `$.BlockPublicAccessConfigurationMetadata.CreatedByArn: value_mismatch (expected '', got 'arn:aws:iam::000000000000:root')`.
Cause 7 — emr-create-studio-invalid-network-error#task#1, CreateStudio, status 400 expected and actual: `$.Message: value_mismatch (expected 'VpcId is not valid. (Service: AWSEditors; Status Code: 400; Error Code: InvalidRequestException; Request ID: 94435719-b19f-4e89-893b-c228449be9e7; Proxy: null)', got "VpcId 'vpc-00000000deadbeef' does not name a VPC in this account")`.
Cause 8 — emr-create-studio-sso-missing-identity-center-error#task#1, CreateStudio: `$.__type: value_mismatch (expected 'ValidationException', got 'InvalidRequestException')`; `$.message: missing (expected 'UserRole is required to create a studio with SSO auth-mode.', got None)`.
Cause 9 — emr-studio-create-invalid-vpc-error#task#3, CreateStudio: `status: expected 400, got 200`; `$.__type: missing (expected 'InvalidRequestException', got None)`; `$.Message: missing (expected '<shaped-not-asserted>', got None)`; affected_count 3 (also emr-studio-create-mismatched-subnet-error#task#7, emr-studio-lifecycle-minimal#task#8).
Cause 10 — emr-studio-session-mapping-create-then-get-roundtrip#task#15, GetStudioSessionMapping, status 200 expected and actual: `$.SessionMapping.IdentityName: missing (expected 'sim2real-emr-grp-dc805b99', got None)` — the name is a per-run value; the requirement is that the created IdentityName round-trips.

Smithy grounding: BlockPublicAccessConfigurationMetadata.CreatedByArn is a REQUIRED ArnType member; SessionMappingDetail.IdentityName is XmlStringMaxLen256 "The name of the user or group"; DescribeReleaseLabel output carries Applications (SimplifiedApplication{Name,Version}) and AvailableOSReleases (OSRelease{Label}); CreateStudio models InvalidRequestException client/400 and makes UserRole optional, so the SSO conditional is recorded behavior only.

## Steps to Reproduce
Boot the emulator and replay these steps from wire trace set simparity-wire-aws-v1 (service token `elasticmapreduce`):
1. Fixture `elasticmapreduce-notebook-execution-error-paths`, steps task#3 (DescribeNotebookExecution) and task#4 (StopNotebookExecution) on an unknown `ex-` execution id; compare Error.Message.
2. Fixture `elasticmapreduce-release-label-and-instance-type-catalog`, step task#2 — DescribeReleaseLabel for a current release label; compare the Applications set/versions and the AvailableOSReleases list against the recording.
3. Fixture `elasticmapreduce-run-job-flow-validation-errors`, steps task#7, task#8, task#9 — RunJobFlow with a ReleaseLabel not in EMR's catalog; expect HTTP 400 + ValidationException, observe 200 + JobFlowId.
4. Fixture `emr-account-level-read-only-config`, step task#1 — DescribeJobFlows; compare the deprecation message. Step task#2 — GetBlockPublicAccessConfiguration on an account that has never called Put; assert CreatedByArn is empty.
5. Fixture `emr-create-studio-invalid-network-error`, step task#1 — CreateStudio with a VpcId that does not exist; compare Error.Message.
6. Fixture `emr-create-studio-sso-missing-identity-center-error`, step task#1 — CreateStudio with AuthMode=SSO and no UserRole; assert `__type == 'ValidationException'` and the message.
7. Fixtures `emr-studio-create-invalid-vpc-error` step task#3, `emr-studio-create-mismatched-subnet-error` step task#7 and `emr-studio-lifecycle-minimal` step task#8 — CreateStudio with an unresolvable VpcId / subnets outside the VpcId; expect HTTP 400 + InvalidRequestException, observe 200.
8. Fixture `emr-studio-session-mapping-create-then-get-roundtrip`, step task#15 — CreateStudioSessionMapping with an IdentityName, then GetStudioSessionMapping; assert SessionMapping.IdentityName is returned.
