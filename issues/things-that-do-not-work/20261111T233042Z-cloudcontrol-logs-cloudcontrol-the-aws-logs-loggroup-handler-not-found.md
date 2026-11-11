Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol__logs.native-log-group-delete-leaves-cloudcontrol-not-found
Issue-Key: cloudcontrol__logs.native-log-group-delete-leaves-cloudcontrol-not-found|run#20|AssertionError|assert cc_error == ("ResourceNotFoundException", expected_message), (
Issue-Commit: 7be3c4c4f98e3bb4fe12b293a9b02ae8c9c46178

# cloudcontrol: the AWS::Logs::LogGroup handler not-found message renders the identifier as the bare log group name instead of the JSON primary-identifier object

> _Note: this issue was identified against the code base at commit `7be3c4c4f98e3bb4fe12b293a9b02ae8c9c46178`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResource

## Description
Cause 1 of 1: for AWS::Logs::LogGroup, the identifier embedded in Cloud Control's handler-wrapped ResourceNotFoundException message is rendered as the bare log group name, where AWS renders it as the compound primary-identifier JSON object. Failing check: cloudcontrol__logs.native-log-group-delete-leaves-cloudcontrol-not-found.

AWS: Smithy (services/service-com-amazonaws-cloudcontrol-cloudapiservice/smithy-model/com-amazonaws-cloudcontrol-cloudapiservice.json) models ResourceNotFoundException on GetResource (doc: "A resource with the specified identifier can't be found.", httpError 404 in the model) and documents GetResource$Identifier as "the primary identifier, or any secondary identifier... Primary identifiers can be specified as a string or JSON". The model does not state how the identifier is rendered back inside the handler diagnostic, so for that the basis is recorded behavior only. The recorded message template is the fixed text "<TypeName> Handler returned status FAILED: Resource of type '<TypeName>' with identifier '<identifier>' was not found. (HandlerErrorCode: NotFound, RequestToken: <token>)". Its variable parts are the TypeName and the identifier taken from the request and the server-minted RequestToken. For AWS::Logs::LogGroup the recorded <identifier> is the resource type's primary-identifier JSON object, i.e. {"/properties/LogGroupName":"<the log group name the caller passed>"} serialized with no spaces — NOT the bare name. The error code is ResourceNotFoundException. This per-type rendering is already on record in this repo: open issue 20261110T221719Z-cloudcontrol-the-aws-sqs-queue-handler-not-found.md (and its sibling 20261110T221719Z-cloudcontrol-sqs-the-aws-sqs-queue-handler-not.md) states that AWS uses the bare string for AWS::SQS::Queue and the '{"/properties/LogGroupName":"<name>"}' JSON form for AWS::Logs::LogGroup in this same message slot, and warns that flipping the rendering globally breaks the other type. This finding is the Logs half now diverging in the opposite direction, so it is a distinct cause from those SQS issues.

Emulator: the wrapper, the error code and the RequestToken are all correct, but the identifier is substituted verbatim as the caller passed it. After CloudWatch Logs' own DeleteLogGroup removes a group that Cloud Control resolved through the native read-through, GetResource(TypeName="AWS::Logs::LogGroup", Identifier=<name>) answers ResourceNotFoundException with "...with identifier '<name>' was not found..." where AWS has "...with identifier '{\"/properties/LogGroupName\":\"<name>\"}' was not found...".

Why: this message is the only place a synchronous Cloud Control refusal exposes the owning handler's verdict and the resource it concerns. A client that parses the diagnostic to recover the resource identity — drift reconciliation, ignore-if-not-found logic, log scraping keyed on the handler diagnostic — sees a shape it does not expect for this type, and tooling written against AWS's JSON /properties form fails to extract the LogGroupName at all. Because the rendering is per resource type, a client cannot normalise it away.

Fix: the per-type identifier rendering in the shared not-found refusal composer, crate::state::resource_not_found_refusal, used by the resource-level handlers of the cloudcontrol crate. The requirement is "the identifier as the owning type's handler renders it": the JSON /properties object for AWS::Logs::LogGroup, the bare string for AWS::SQS::Queue. Do not change it globally in either direction — a global flip re-breaks whichever type was previously right.

Traces: cloudcontrol__logs.native-log-group-delete-leaves-cloudcontrol-not-found. Call sites: services/service-com-amazonaws-cloudcontrol-cloudapiservice/src/ops/get_resource.rs:264-272 (the ResourceNotFoundException construction) and the composer in services/service-com-amazonaws-cloudcontrol-cloudapiservice/src/state.rs.

## Evidence
From the replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout. The generated log group name and the RequestToken fill the template and are not part of the requirement.

cloudcontrol__logs.native-log-group-delete-leaves-cloudcontrol-not-found (mismatch):

AssertionError: GetResource for the natively-deleted log group 'sim2real-behav-ccnat-454733750596' answered ('ResourceNotFoundException', "AWS::Logs::LogGroup Handler returned status FAILED: Resource of type 'AWS::Logs::LogGroup' with identifier 'sim2real-behav-ccnat-454733750596' was not found. (HandlerErrorCode: NotFound, RequestToken: 98d3dd73-824f-42e5-ae1c-5ef6ee64804c)"), expected ('ResourceNotFoundException', 'AWS::Logs::LogGroup Handler returned status FAILED: Resource of type \'AWS::Logs::LogGroup\' with identifier \'{"/properties/LogGroupName":"sim2real-behav-ccnat-454733750596"}\' was not found. (HandlerErrorCode: NotFound, RequestToken: 98d3dd73-824f-42e5-ae1c-5ef6ee64804c)'); a target that still serves it is over-permissive

Fixed text of the message (identical on both sides): "AWS::Logs::LogGroup Handler returned status FAILED: Resource of type '", "' with identifier '", "' was not found. (HandlerErrorCode: NotFound, RequestToken: ", ")". Variable parts: the TypeName and the identifier, both from the request, and the server-minted RequestToken. Only the identifier RENDERING diverges — expected the compound primary-identifier object {"/properties/LogGroupName":"<log group name>"} (JSON, separators "," and ":", no whitespace), observed the bare <log group name>.

The error code (ResourceNotFoundException) and the RequestToken agree, and every earlier assertion in the check passes: Cloud Control GetResource does resolve the natively created log group with Logs' own LogGroupName, RetentionInDays and Arn, and ListResources does enumerate it. The divergence is confined to the identifier rendering in the post-delete refusal message.

## Steps to Reproduce
Boot the emulator from this checkout and point botocore at it; cc = client("cloudcontrol", endpoint_url=<emulator>), logs = client("logs", endpoint_url=<emulator>).

1. logs.create_log_group(logGroupName="<a fresh name>") and logs.put_retention_policy(logGroupName="<that name>", retentionInDays=1).
2. cc.get_resource(TypeName="AWS::Logs::LogGroup", Identifier="<that name>") — succeeds today and reports Logs' own LogGroupName/RetentionInDays/Arn.
3. logs.delete_log_group(logGroupName="<that name>"), then poll logs.describe_log_groups(logGroupNamePrefix="<that name>") until the group is gone.
4. cc.get_resource(TypeName="AWS::Logs::LogGroup", Identifier="<that name>") and read Error.Code / Error.Message of the raised ClientError.

Required: Error.Code == "ResourceNotFoundException" and Error.Message == "AWS::Logs::LogGroup Handler returned status FAILED: Resource of type 'AWS::Logs::LogGroup' with identifier '{\"/properties/LogGroupName\":\"<that name>\"}' was not found. (HandlerErrorCode: NotFound, RequestToken: <token>)" — the identifier is the compound primary-identifier JSON object.
Observed: the identifier appears as the bare <that name>.

Check the same refusal via cc.update_resource / cc.delete_resource against the gone identifier. Re-run cloudcontrol__logs.native-log-group-delete-leaves-cloudcontrol-not-found to confirm the fix, and re-run the AWS::SQS::Queue checks named in 20261110T221719Z-cloudcontrol-the-aws-sqs-queue-handler-not-found.md to confirm the bare rendering there was not disturbed.
