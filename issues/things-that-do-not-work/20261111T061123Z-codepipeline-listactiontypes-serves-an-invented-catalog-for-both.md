Issue-Source: trace-sweep-divergence
Issue-Trace: codepipeline-catalog-list-action-types
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codepipeline-catalog-list-action-types
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline|ListActionTypes|2xx->2xx|$.actionTypes[*].settings.entityUrlTemplate
Issue-Key: codepipeline|ListActionTypes|2xx->2xx|$.actionTypes[*].settings.executionUrlTemplate
Issue-Key: codepipeline|ListActionTypes|2xx->2xx|$.actionTypes[*].settings.revisionUrlTemplate
Issue-Key: codepipeline|ListActionTypes|2xx->2xx|$.actionTypes[*].settings.thirdPartyConfigurationUrl
Issue-Related: 20261107T211738Z-codepipeline-listactiontypes-returns-an-emulator-invented-action-type.md
Issue-Commit: d0a04fabcd83cf2b721ac740047fd92b0e777ff8

# CodePipeline ListActionTypes serves an invented catalog for both the AWS and ThirdParty owner scopes

> _Note: this issue was identified against the code base at commit `d0a04fabcd83cf2b721ac740047fd92b0e777ff8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListActionTypes

## Description
ListActionTypes answers HTTP 200 with an action-type catalog whose membership, ordering and per-entry metadata are the emulator's own invention rather than CodePipeline's, and it does so for every owner scope the operation is asked about. 2 causes, one per owner scope exercised by the corpus.

Cause 1 of 2 - the AWS-owned catalog (fixture codepipeline-catalog-list-action-types, step task#1).
AWS: ListActionTypes returns the service's catalog of available action types, optionally narrowed by actionOwnerFilter. Smithy: ListActionTypes input {actionOwnerFilter: ActionOwner enum, regionFilter: AWSRegionName @length 4..30, nextToken @length 1..2048}, output.actionTypes is @required (ActionTypeList), errors InvalidNextTokenException / ValidationException (both httpError 400). Smithy shapes: ActionType.id (ActionTypeId: category/owner/provider/version), ActionType.inputArtifactDetails and outputArtifactDetails (ArtifactDetails.minimumCount/maximumCount), ActionType.settings (ActionTypeSettings: entityUrlTemplate/executionUrlTemplate/revisionUrlTemplate/thirdPartyConfigurationUrl), ActionType.actionConfigurationProperties (ActionConfigurationProperty: name/required/key/secret/queryable/description). The catalog's CONTENTS are not in the model, so for those: recorded behavior only - the recording returns 28 AWS-owned entries, led by category=Source owner=AWS provider=S3 version=1 whose four configuration properties (S3Bucket, S3ObjectKey, PollForSourceChanges, AllowOverrideForS3ObjectKey) each carry a description and key=true, whose settings carry an entityUrlTemplate into the S3 console for the requested region, and whose outputArtifactDetails is minimumCount=1/maximumCount=1; the second entry is the AWS Lambda Invoke type with described FunctionName/UserParameters properties.
Emulator: it returns a shorter, differently ordered list led by category=Approval owner=AWS provider=Manual version=1 with settings={}, configuration properties (NotificationArn, CustomData, ExternalEntityLink) that carry NO description and key=false, and inputArtifactDetails/outputArtifactDetails both 0/0. The replay reports every expected index $.actionTypes[0]..$.actionTypes[27] as diverging, i.e. no position in the list matches, and the settings.entityUrlTemplate / executionUrlTemplate of most entries are absent entirely.
Why: this operation IS the discoverability surface for the catalog. A client that enumerates it to decide which category/owner/provider triples it may put into a pipeline declaration, to render the required/key/secret flags and descriptions of a provider's configuration keys, or to size artifacts against inputArtifactDetails/outputArtifactDetails, is shown providers that do not exist on AWS and is not shown the ones that do (S3, Lambda, CodeCommit, ...). Declarations built from the emulator's catalog are then rejected by AWS, and vice versa.
Fix: the built-in action-type catalog table the ListActionTypes handler serves from - it must carry CodePipeline's AWS-owned entries in the service's order, each with its settings URL templates, its configuration-property descriptions and flags, and its real artifact-count bounds.
Traces: codepipeline-catalog-list-action-types#task#1.

Cause 2 of 2 - the ThirdParty-owned catalog (fixture codepipeline-catalog-list-action-types, step task#2).
AWS: same operation, shapes and errors as Cause 1, with actionOwnerFilter selecting the ThirdParty slice. Contents are recorded behavior only - the recording returns 5 ThirdParty entries led by category=Test owner=ThirdParty provider=GhostInspector version=1, with described configuration properties ClientId, ClientToken (secret=true), TestId and ExtraParams, settings carrying entityUrlTemplate, executionUrlTemplate and thirdPartyConfigurationUrl, inputArtifactDetails 0..1 and outputArtifactDetails 0..0.
Emulator: it returns a different ThirdParty set led by category=Source owner=ThirdParty provider=GitHub version=1 with its own settings and Owner/Repo/Branch/OAuthToken/PollForSourceChanges properties, none of which carry a description. The replay reports all 5 expected indices $.actionTypes[0]..$.actionTypes[4] as diverging.
Why: a client discovering third-party integrations gets a provider list that does not match the account's, so the provider it offers a user cannot be used and the provider AWS does offer is never offered. The description and secret-flag metadata a console or wizard renders is missing too.
Fix: same component - the ThirdParty slice of that catalog table, plus the actionOwnerFilter selection that picks it.

Related open issues (neither covers these replay keys as filed): 20261107T211738Z-codepipeline-listactiontypes-returns-an-emulator-invented-action-type.md describes the same catalog fault but the replay reports these two clusters as unowned, so they are re-filed here with their verbatim fixture/step binding so a fixer re-replays them; 20261029T202908Z-codepipeline-pipelinedeclaration-responses-omit-modeled-defaults-and-nested.md is about PipelineDeclaration responses (GetPipeline/CreatePipeline/UpdatePipeline), not the action-type catalog.

Note on values: the provider names, property descriptions, URL templates and artifact counts above are the catalog's own fixed content, not per-run values. The only request-derived part is the region inside the S3 entityUrlTemplate.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up=true, /__admin/health 200, data listener accepting). Both clusters: verdict mismatch, expected status 200, actual status 200 - the divergence is purely in the body.

Cause 1 - codepipeline-catalog-list-action-types#task#1, ListActionTypes.
  Diverging keys: $.actionTypes[0] .. $.actionTypes[27] plus $.actionTypes[N].settings.entityUrlTemplate / .executionUrlTemplate for most N (all 28 recorded entries).
  $.actionTypes[0]: missing - expected {'id': {'__type': 'ActionTypeId', 'category': 'Source', 'owner': 'AWS', 'provider': 'S3', 'version': '1'}, 'actionConfigurationProperties': [S3Bucket (description 'The Amazon S3 Bucket', key=true, required=true), S3ObjectKey (description 'The Amazon S3 object key', key=true, required=true), PollForSourceChanges (description 'The option to poll for source repository changes', key=true, required=false), AllowOverrideForS3ObjectKey (description 'Whether S3 Object Key can be overridden during StartPipelineExecution', key=true, required=false)], 'inputArtifactDetails': {'minimumCount': 0, 'maximumCount': 0}, 'outputArtifactDetails': {'minimumCount': 1, 'maximumCount': 1}, 'settings': {'entityUrlTemplate': 'https://console.aws.amazon.com/s3/home?region=us-west-2#'}}
  got {'id': {'__type': 'ActionTypeId', 'category': 'Approval', 'owner': 'AWS', 'provider': 'Manual', 'version': '1'}, 'settings': {}, 'actionConfigurationProperties': [NotificationArn, CustomData, ExternalEntityLink - all required=false, key=false, secret=false, queryable=false and with no description], 'inputArtifactDetails': {'minimumCount': 0, 'maximumCount': 0}, 'outputArtifactDetails': {'minimumCount': 0, 'maximumCount': 0}}
  $.actionTypes[1]: missing - expected the AWS/Invoke/Lambda entry with FunctionName ('The AWS Lambda function to invoke', key=true, required=true) and UserParameters ('Extra parameters for the AWS Lambda function', key=true, required=false).

Cause 2 - codepipeline-catalog-list-action-types#task#2, ListActionTypes.
  Diverging keys: $.actionTypes[0], $.actionTypes[0].settings.revisionUrlTemplate, $.actionTypes[0].settings.thirdPartyConfigurationUrl, $.actionTypes[1], $.actionTypes[1].settings.thirdPartyConfigurationUrl, $.actionTypes[2], $.actionTypes[3], $.actionTypes[4].
  $.actionTypes[0]: missing - expected {'id': {'__type': 'ActionTypeId', 'category': 'Test', 'owner': 'ThirdParty', 'provider': 'GhostInspector', 'version': '1'}, 'actionConfigurationProperties': [ClientId 'The user client id' required=true, ClientToken 'The user client token' required=true secret=true, TestId 'The test id used by the Ghost Inspector tests' required=true, ExtraParams 'A JSON-encoded string of extra parameters to provide to the test or suite at runtime.' required=false], 'inputArtifactDetails': {'minimumCount': 0, 'maximumCount': 1}, 'outputArtifactDetails': {'minimumCount': 0, 'maximumCount': 0}, 'settings': {'entityUrlTemplate': 'https://app.ghostinspector.com/tests/{Config:TestId}', 'executionUrlTemplate': 'https://app.ghostinspector.com/results/{ExternalExecutionId}', 'thirdPartyConfigurationUrl': 'https://app.ghostinspector.com/code-pipeline'}}
  got {'id': {'__type': 'ActionTypeId', 'category': 'Source', 'owner': 'ThirdParty', 'provider': 'GitHub', 'version': '1'}, 'settings': {'thirdPartyConfigurationUrl': 'https://github.com/join', 'entityUrlTemplate': 'https://github.com/{Config:Owner}/{Config:Repo}/tree/{Config:Branch}', 'revisionUrlTemplate': 'https://github.com/{Config:Owner}/{Config:Repo}/commits'}, 'actionConfigurationProperties': [Owner, Repo, Branch, OAuthToken (secret=true), PollForSourceChanges - none with a description]}

Model consulted: services/service-com-amazonaws-codepipeline-codepipeline-20150709/smithy-model/com-amazonaws-codepipeline-codepipeline-20150709.json - ListActionTypes signature and errors, shapes ActionType, ActionTypeId, ActionTypeSettings, ActionConfigurationProperty, ArtifactDetails.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Replay fixture codepipeline-catalog-list-action-types.
3. Step task#1: call ListActionTypes for the AWS-owned scope and compare the actionTypes list with the recorded response - the emulator's list differs at every index (28 recorded entries led by Source/AWS/S3; the emulator leads with Approval/AWS/Manual), and its entries omit actionConfigurationProperties[].description, the settings URL templates and the non-zero input/outputArtifactDetails bounds.
4. Step task#2: call ListActionTypes with actionOwnerFilter selecting ThirdParty - the emulator's list differs at all 5 indices (recorded leads with Test/ThirdParty/GhostInspector; the emulator leads with Source/ThirdParty/GitHub) and again omits every configuration-property description.
5. Both steps answer 200, so the divergence is entirely in catalog membership, ordering and per-entry metadata.
