Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol.cloudcontrol-delete-resource-removes-the-native-log-group-not-just-the-index
Issue-Shape-Id: com.amazonaws.cloudcontrol#CloudApiService
Issue-Key: cloudcontrol.cloudcontrol-delete-resource-removes-the-native-log-group-not-just-the-index|run#22|AssertionError|assert _handler_not_found(
Issue-Commit: b94f7af2da1ac92db0383ac2b193617da0b34cf0

# cloudcontrol: DeleteResource-then-GetResource on AWS::Logs::LogGroup renders the not-found message identifier as the bare log group name instead of the JSON primary-identifier object

> _Note: this issue was identified against the code base at commit `b94f7af2da1ac92db0383ac2b193617da0b34cf0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResource

## Description
Cause 1 of 1: after a Cloud Control DeleteResource has removed an AWS::Logs::LogGroup, GetResource correctly refuses with ResourceNotFoundException, but the identifier embedded in the handler-wrapped diagnostic is rendered as the bare log group name where AWS renders the resource type's primary-identifier JSON object. Failing check: cloudcontrol.cloudcontrol-delete-resource-removes-the-native-log-group-not-just-the-index.

AWS: Smithy (services/service-com-amazonaws-cloudcontrol-cloudapiservice/smithy-model/com-amazonaws-cloudcontrol-cloudapiservice.json) models ResourceNotFoundException on GetResource as a client error with httpError 404 (doc: "A resource with the specified identifier can't be found."), and models GetResource$Identifier as a required string (@length 1..1024, @pattern ^.+$) that may be given either as a bare string or as the primary-identifier JSON. The model does not state how the identifier is rendered back inside the handler diagnostic, so for the message text the basis is recorded behavior only. The recorded message template is the fixed text "<TypeName> Handler returned status FAILED: Resource of type '<TypeName>' with identifier '<identifier>' was not found. (HandlerErrorCode: NotFound, RequestToken: <token>)"; its variable parts are the TypeName and the identifier (both taken from the request) and the server-minted RequestToken. For AWS::Logs::LogGroup the recorded <identifier> is the type's compound primary-identifier JSON object, {"/properties/LogGroupName":"<the log group name the caller passed>"}, serialized with no whitespace — not the bare name. The error code is ResourceNotFoundException. The rendering is per resource type: this repo already records (open issues 20261110T221719Z-cloudcontrol-the-aws-sqs-queue-handler-not-found.md and 20261110T221719Z-cloudcontrol-sqs-the-aws-sqs-queue-handler-not.md) that AWS uses the BARE string for AWS::SQS::Queue in this same message slot, so a global flip of the rendering re-breaks that type.

Emulator: the envelope, the error code and the RequestToken are all correct, but the identifier is substituted verbatim as the caller passed it. After cc.create_resource(TypeName="AWS::Logs::LogGroup", ...) succeeds, the native group is proven live (CreateLogStream + PutLogEvents return 200), cc.delete_resource reaches SUCCESS, DescribeLogGroups no longer lists the group and native PutLogEvents correctly raises ResourceNotFoundException "The specified log group does not exist." — every earlier assertion of the check passes. Only the last one fails: GetResource answers ResourceNotFoundException with "AWS::Logs::LogGroup Handler returned status FAILED: Resource of type 'AWS::Logs::LogGroup' with identifier '<log group name>' was not found. (HandlerErrorCode: NotFound, RequestToken: <token>)" where AWS has "...with identifier '{\"/properties/LogGroupName\":\"<log group name>\"}' was not found...".

Related but NOT covering this check: open issue 20261111T233042Z-cloudcontrol-logs-cloudcontrol-the-aws-logs-loggroup-handler-not-found.md reports the same rendering defect reached through the NATIVE-delete path (check cloudcontrol__logs.native-log-group-delete-leaves-cloudcontrol-not-found, service heading cloudcontrol__logs) and carries only that check's replay key; the replay report did not mark the present check as owned by it. Filed here so the Cloud Control-delete path is carried by a work item with its own replay key; a fixer closing both should re-replay both check ids.

Why: this diagnostic is the only place a synchronous Cloud Control refusal exposes the owning handler's verdict and the resource it concerns. A client that parses it to recover the resource identity — drift reconciliation, ignore-if-not-found deletion logic, log scraping keyed on the handler diagnostic — sees a shape it does not expect for this type, and tooling written against AWS's JSON /properties form cannot extract the LogGroupName at all. Because the rendering is per resource type, a client cannot normalise it away.

Fix: the per-type identifier rendering in the shared not-found refusal composer used by the cloudcontrol crate's resource-level handlers (the ResourceNotFoundException construction in the GetResource handler and the composer it calls in the crate's state module). The requirement is "the identifier as the owning type's handler renders it": the JSON /properties object for AWS::Logs::LogGroup, the bare string for AWS::SQS::Queue. Do not flip it globally in either direction.

Traces: cloudcontrol.cloudcontrol-delete-resource-removes-the-native-log-group-not-just-the-index. Call sites: services/service-com-amazonaws-cloudcontrol-cloudapiservice/src/ops/get_resource.rs (the ResourceNotFoundException construction) and services/service-com-amazonaws-cloudcontrol-cloudapiservice/src/state.rs (the refusal composer).

## Evidence
From the replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout. The generated log group name and the RequestToken fill the message template and are not part of the requirement.

cloudcontrol.cloudcontrol-delete-resource-removes-the-native-log-group-not-just-the-index (mismatch):

  File "check_cloudcontrol_cloudcontrol_delete_resource_removes_the_native_log_group_not_just_the_index.py", line 117, in run
    assert _handler_not_found(
  AssertionError: AWS::Logs::LogGroup Handler returned status FAILED: Resource of type 'AWS::Logs::LogGroup' with identifier 'sim2real-behav-cclg-aa0659592201' was not found. (HandlerErrorCode: NotFound, RequestToken: 6fe84b94-7db1-4582-82d8-e2f221c48deb)

The check asserts Error.Code == "ResourceNotFoundException" (the emulator agrees) and then that Error.Message starts with the detail "AWS::Logs::LogGroup Handler returned status FAILED: Resource of type 'AWS::Logs::LogGroup' with identifier '{\"/properties/LogGroupName\":\"<name>\"}' was not found." followed by " (HandlerErrorCode: NotFound, RequestToken: " and a closing ")".

Fixed text of the message (identical on both sides): "<TypeName> Handler returned status FAILED: Resource of type '", "' with identifier '", "' was not found. (HandlerErrorCode: NotFound, RequestToken: ", ")". Variable parts: the TypeName and the identifier, both from the request, and the server-minted RequestToken (the check deliberately treats the token as server-chosen). Only the identifier RENDERING diverges — required the compound primary-identifier object {"/properties/LogGroupName":"<log group name>"} (JSON, separators "," and ":", no whitespace), observed the bare <log group name>.

Everything else in this check passes, which localises the defect: CreateResource reaches SUCCESS, the created group accepts a native CreateLogStream and PutLogEvents (HTTP 200), DeleteResource reaches SUCCESS, the group is absent from a paginated DescribeLogGroups, and native PutLogEvents afterwards raises ResourceNotFoundException with exactly "The specified log group does not exist.". The divergence is confined to the identifier rendering in the post-delete GetResource refusal message.

## Steps to Reproduce
Boot the emulator from this checkout and point botocore at it; cc = client("cloudcontrol", endpoint_url=<emulator>), logs = client("logs", endpoint_url=<emulator>).

1. cc.create_resource(TypeName="AWS::Logs::LogGroup", DesiredState=json.dumps({"LogGroupName": "<a fresh name>", "RetentionInDays": 1, "Tags": [{"Key": "sim2real", "Value": "<any>"}]})) and poll cc.get_resource_request_status(RequestToken=...) until OperationStatus == "SUCCESS".
2. Prove the group is live: logs.create_log_stream(logGroupName="<that name>", logStreamName="<a stream>") then logs.put_log_events(...) — returns HTTP 200.
3. cc.delete_resource(TypeName="AWS::Logs::LogGroup", Identifier="<that name>") and poll to OperationStatus == "SUCCESS". Confirm the group is gone from logs.describe_log_groups(logGroupNamePrefix="<that name>") and that native logs.put_log_events now raises ResourceNotFoundException "The specified log group does not exist." (both already correct).
4. cc.get_resource(TypeName="AWS::Logs::LogGroup", Identifier="<that name>") and read Error.Code / Error.Message of the raised ClientError.

Required: Error.Code == "ResourceNotFoundException" and Error.Message == "AWS::Logs::LogGroup Handler returned status FAILED: Resource of type 'AWS::Logs::LogGroup' with identifier '{\"/properties/LogGroupName\":\"<that name>\"}' was not found. (HandlerErrorCode: NotFound, RequestToken: <token>)" — the identifier is the compound primary-identifier JSON object, the RequestToken is server-chosen.
Observed: the identifier appears as the bare <that name>.

Check the same refusal through cc.update_resource and cc.delete_resource against the already-gone identifier. Re-replay cloudcontrol.cloudcontrol-delete-resource-removes-the-native-log-group-not-just-the-index (and, if the composer is touched, cloudcontrol__logs.native-log-group-delete-leaves-cloudcontrol-not-found from 20261111T233042Z plus the AWS::SQS::Queue checks named in 20261110T221719Z-cloudcontrol-the-aws-sqs-queue-handler-not-found.md, to confirm the bare rendering there was not disturbed).
