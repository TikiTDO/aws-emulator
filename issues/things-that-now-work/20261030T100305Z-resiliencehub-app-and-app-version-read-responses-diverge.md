Issue-Source: trace-sweep-divergence
Issue-Trace: resiliencehub-app-version-publish
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resiliencehub-app-version-publish resiliencehub-app-lifecycle
Issue-Shape-Id: com.amazonaws.resiliencehub#AwsResilienceHub
Issue-Key: resiliencehub|DescribeAppVersionTemplate|2xx->2xx|$.appTemplateBody
Issue-Key: resiliencehub|UpdateApp|2xx->2xx|$.app.tags.resourceArn
Issue-Key: resiliencehub|DescribeAppVersionTemplate|4xx->4xx|$.message
Issue-Commit: 7d4a82fc68d671fcbf7a8009b036d72ed0c4ec91

# resiliencehub app and app-version read responses diverge from AWS: template body, UpdateApp tags, version-not-found wording

> _Note: this issue was identified against the code base at commit `7d4a82fc68d671fcbf7a8009b036d72ed0c4ec91`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAppVersionTemplate

## Description
Three net-new causes on resiliencehub's read paths for an application and its app versions. Every step below reached the handler and returned the right HTTP status; what diverges is the RESPONSE BODY (Causes 1 and 2) or the refusal MESSAGE (Cause 3). None of these is covered by an open issue: the open issue 'resiliencehub refusal messages diverge from AWS wording across app, resource and policy errors' covers CreateApp/DescribeApp/DescribeResiliencyPolicy/DeleteResiliencyPolicy messages but NOT the DescribeAppVersionTemplate app-version 404 of Cause 3; the open issues 'resiliencehub PublishAppVersion labels the published version by sequence number instead of the release alias' and 'resiliencehub: PublishAppVersion mints numbered versions instead of the "release" alias' are about the appVersion LABEL of a published version, not about the serialization of appTemplateBody in Cause 2; 'resiliencehub UpdateResiliencyPolicy does not persist the updated policy fields' is about policy state, not about App.tags in Cause 1. A fixer must close all THREE causes before submitting.

=== Cause 1 of 3: UpdateApp's App response omits the application's tags ===
AWS: UpdateApp returns the full App structure for the updated application (Smithy: UpdateApp output member `app` (App) @required; App$tags -> TagMap, @sensitive, length 1..50, keys TagKey length 1..128, values TagValue length 0..256). App$tags is documented as 'Tags assigned to the resource. A tag is a label that you assign to an Amazon Web Services resource. Each tag consists of a key/value pair.' — i.e. the tag set currently assigned to the application, whatever was assigned at CreateApp time or afterwards. The recording shows the updated app answering with its tag map populated (a tag whose key is `resourceArn` carrying the application's own ARN as its value — the key and value are the per-run tag the fixture assigned, the requirement is that the assigned tag set comes back, not those literal strings).
Emulator: the `tags` member is absent from `$.app` entirely — the replay reports `$.app.tags.resourceArn: missing (expected '<app arn>', got None)` on an otherwise correct HTTP 200 App body.
Why: tags are how clients attribute cost, drive tag-based IAM/ABAC and discover resources. A client that tags an application and then calls UpdateApp (the read-after-write every SDK waiter and most IaC providers perform) sees an untagged application, concludes its tags were dropped and re-applies or diffs forever; tag-filtering code reading App.tags finds nothing.
Fix: the App projection written by the UpdateApp handler plus the tag state it reads — the handler returns the stored App record verbatim (`Ok(UpdateAppOutput { app: app.clone() })`), so the stored record is what is missing the post-create tag set. Note TagResource is still an unimplemented scaffold, so there is currently no path by which a tag assigned after CreateApp reaches the App record; Hypothesis: that is why the member comes back absent rather than empty. Whatever the owning component, UpdateApp must answer the tag set the application carries.
Traces: resiliencehub-app-lifecycle#task#5.

=== Cause 2 of 3: DescribeAppVersionTemplate renders an appTemplateBody missing four documented pieces of structure and puts the app version label in `version` ===
AWS: DescribeAppVersionTemplate returns `appTemplateBody` (Smithy: AppTemplateBody, @required, length 0..409600), documented as 'A JSON string that provides information about your application structure' with the structure `resources` / `appComponents` / `excludedResources` / `version`. The recorded body shows, for a published version:
  - each `resources[].logicalResourceId` carries ALL of the documented source members, with the ones that do not apply present as JSON `null` (`identifier`, `logicalStackName`: null, `resourceGroupName`: null) — not omitted;
  - each `appComponents[]` entry carries an `id` alongside `name`, `type` and `resourceNames`;
  - the implicit AppCommon component is part of every template: an `appComponents[]` entry of type `AWS::ResilienceHub::AppCommonAppComponent` named/ided `appcommon` with `resourceNames: []`;
  - `excludedResources` is always present, as `{"logicalResourceIds": []}` when nothing is excluded;
  - `version` is the TEMPLATE SCHEMA version as a JSON NUMBER (`2.0`), not a string and not the app version label.
Emulator: renders only `{"resources":[...],"appComponents":[...],"version":...}` where `logicalResourceId` holds just `identifier`, `appComponents[]` entries have no `id`, the `appcommon` component is never synthesized, `excludedResources` is absent, and `version` is set to the requested app version label (the string `"release"` in the recording) instead of the numeric template schema version.
Why: appTemplateBody is a JSON document clients round-trip — read it with DescribeAppVersionTemplate, edit it, write it back with PutDraftAppVersionTemplate. A parser that expects `excludedResources` or reads `appComponents[].id` throws or silently loses the component identity; a client that treats `version` as the template schema version reads an app label and either rejects the template or writes back a body whose schema version it cannot interpret; and a round-trip through this emulator silently deletes the appcommon component and the exclusion list from the customer's template.
Fix: the appTemplateBody serializer in the DescribeAppVersionTemplate handler (its `app_template_body` / `logical_resource_id` renderers) — it must emit the unset logicalResourceId source members as null, emit each component's id, synthesize the appcommon component, always emit `excludedResources.logicalResourceIds`, and emit the template schema version number for `version` rather than the app version label. The same renderer must stay consistent with what PutDraftAppVersionTemplate accepts.
Traces: resiliencehub-app-version-publish#task#11.

=== Cause 3 of 3: DescribeAppVersionTemplate's app-version ResourceNotFoundException message is paraphrased ===
AWS: reading a template at an appVersion the application does not have is refused with ResourceNotFoundException / HTTP 404 (Smithy: DescribeAppVersionTemplate errors include ResourceNotFoundException, httpError 404, with a `message` member). Recorded message: `App version <appVersion> not found for appArn <appArn>` — fixed text `App version `, ` not found for appArn `, with the request's appVersion and appArn as the variable parts, no trailing period. Note it is the APP-VERSION variant of the same family as the app-level `App not found for appArn <appArn>` text.
Emulator: `application <appArn> does not exist has no version <appVersion>` style paraphrase — concretely `application <appArn> has no version <appVersion>`: different fixed text, reversed variable order, and it never uses the words `App version` or `not found`.
Why: a client probing whether a version exists (the standard way to tell a draft from a published version before publishing) distinguishes 'app missing' from 'version missing' by this text, since both are ResourceNotFoundException/404 on the same operation; a paraphrase collapses the two cases and breaks message-based classification and log scraping keyed on `App version ... not found for appArn`.
Fix: the app-version not-found error construction in the DescribeAppVersionTemplate handler (its `app_version_not_found` helper), which should share the service's app/app-version not-found message templates.
Traces: resiliencehub-app-version-publish#task#12.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200).

Cause 1 — UpdateApp, HTTP 200 expected and returned, resiliencehub-app-lifecycle#task#5:
  $.app.tags.resourceArn: missing (expected '<application's ARN>', got None)
  The whole `tags` map is absent from the returned App; every other App member matched.

Cause 2 — DescribeAppVersionTemplate, HTTP 200 expected and returned, resiliencehub-app-version-publish#task#11:
  $.appTemplateBody expected:
    {"resources":[{"logicalResourceId":{"identifier":"<resId>","logicalStackName":null,"resourceGroupName":null},"type":"AWS::S3::Bucket","name":"<resName>"}],"appComponents":[{"id":"<compId>","name":"<compName>","type":"AWS::ResilienceHub::StorageAppComponent","resourceNames":["<resName>"]},{"id":"appcommon","name":"appcommon","type":"AWS::ResilienceHub::AppCommonAppComponent","resourceNames":[]}],"excludedResources":{"logicalResourceIds":[]},"version":2.0}
  $.appTemplateBody actual:
    {"resources":[{"logicalResourceId":{"identifier":"<resId>"},"type":"AWS::S3::Bucket","name":"<resName>"}],"appComponents":[{"name":"<compName>","type":"AWS::ResilienceHub::StorageAppComponent","resourceNames":["<resName>"]}],"version":"release"}
  Diffs, independent of the per-run resource/component names: logicalStackName and resourceGroupName nulls dropped; appComponents[].id dropped; the appcommon AppCommonAppComponent entry never emitted; excludedResources.logicalResourceIds absent; version is the app version label string instead of the numeric template schema version 2.0.

Cause 3 — DescribeAppVersionTemplate, HTTP 404 expected and returned, resiliencehub-app-version-publish#task#12:
  $.message expected 'App version <appVersion> not found for appArn <appArn>'
  $.message actual   'application <appArn> has no version <appVersion>'
  Fixed text of the requirement: 'App version ' + ' not found for appArn '; variable parts: the request's appVersion and appArn. The ARN, app id and the version token 99 in the recording are per-run values filling that template.

Smithy citations (services/service-com-amazonaws-resiliencehub-awsresiliencehub/smithy-model/com-amazonaws-resiliencehub-awsresiliencehub.json): UpdateApp -> UpdateAppResponse$app (App, @required); App$tags -> TagMap (@sensitive, length 1..50); DescribeAppVersionTemplate -> DescribeAppVersionTemplateResponse$appTemplateBody (AppTemplateBody, @required, length 0..409600) whose documentation enumerates resources/logicalResourceId{identifier,logicalStackName,resourceGroupName,terraformSourceName,eksSourceName}, appComponents{name,type,resourceNames,additionalInfo}, excludedResources{logicalResourceIds} and version; DescribeAppVersionTemplate errors include ResourceNotFoundException (httpError 404) with a message member.
Code sites for the fixer: src/ops/update_app.rs (handler returns the stored App verbatim), src/ops/describe_app_version_template.rs (`app_template_body`, `logical_resource_id`, `app_version_not_found`), src/ops/tag_resource.rs (still a not_implemented scaffold).

## Steps to Reproduce
1. Cause 1: CreateApp(name=<name>), assign a tag to the resulting appArn, then UpdateApp(appArn=<arn>, description=<new description>) -> HTTP 200 whose `app` object has NO `tags` member at all, instead of the App carrying the tag set assigned to the application. Replay resiliencehub-app-lifecycle#task#5.
2. Cause 2: CreateApp, CreateAppVersionAppComponent(type=AWS::ResilienceHub::StorageAppComponent), CreateAppVersionResource(type=AWS::S3::Bucket) on the draft, ResolveAppVersionResources, PublishAppVersion, then DescribeAppVersionTemplate(appArn=<arn>, appVersion=<published version>) -> HTTP 200 whose appTemplateBody omits the logicalResourceId source nulls, omits appComponents[].id, omits the appcommon AppCommonAppComponent, omits excludedResources, and sets "version" to the app version label instead of the numeric template schema version 2.0. Replay resiliencehub-app-version-publish#task#11.
3. Cause 3: DescribeAppVersionTemplate(appArn=<arn of an existing app>, appVersion="99") -> HTTP 404 ResourceNotFoundException with message 'application <arn> has no version 99' instead of 'App version 99 not found for appArn <arn>'. Replay resiliencehub-app-version-publish#task#12.
4. Full check: replay_wire_corpus() and re-inspect resiliencehub-app-lifecycle#task#5, resiliencehub-app-version-publish#task#11 and resiliencehub-app-version-publish#task#12.
