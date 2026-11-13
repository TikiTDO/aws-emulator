Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol.cancel-in-flight-request-reaches-cancel-complete cloudcontrol.e2e-cancel-request-mid-flight-resource-absent
Issue-Shape-Id: com.amazonaws.cloudcontrol#CloudApiService
Issue-Key: cloudcontrol.cancel-in-flight-request-reaches-cancel-complete|run#11|AssertionError|assert final.get("StatusMessage") == "User Initialized the Cancellation", (
Issue-Key: sns|GetTopicAttributes|InvalidParameter
Issue-Commit: ba88c028fef36e1bc767f96ee2f820f05186c145

# cloudcontrol: 2 net-new causes — a cancelled request's ProgressEvent carries no StatusMessage "User Initialized the Cancellation", and CreateResource of AWS::SNS::Topic reports the TopicName as Identifier instead of the topic ARN

> _Note: this issue was identified against the code base at commit `ba88c028fef36e1bc767f96ee2f820f05186c145`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelResourceRequest

## Description
Two net-new root causes, both in the cloudcontrol service. Each must be closed before submitting; the failing check ids are listed per cause.

Cause 1 of 2: a request cancelled while in flight reaches CANCEL_COMPLETE with NO StatusMessage, where AWS reports the user-cancellation message.
Failing check: cloudcontrol.cancel-in-flight-request-reaches-cancel-complete
AWS: Smithy (services/service-com-amazonaws-cloudcontrol-cloudapiservice/smithy-model/com-amazonaws-cloudcontrol-cloudapiservice.json) models CancelResourceRequest(RequestToken required, @length 1-128, pattern ^[-A-Za-z0-9+/=]+$) -> ProgressEvent, errors RequestTokenNotFoundException (404) and ConcurrentModificationException (500); ProgressEvent$OperationStatus enumerates PENDING / IN_PROGRESS / SUCCESS / FAILED / CANCEL_IN_PROGRESS / CANCEL_COMPLETE, and ProgressEvent$StatusMessage (@length 0-2048) is documented as "Any message explaining the current status." The model does not state the text, so this is recorded behavior only: a create request that is cancelled while IN_PROGRESS settles at OperationStatus CANCEL_COMPLETE and the SAME ProgressEvent carries StatusMessage "User Initialized the Cancellation" — that whole sentence is FIXED text for the user-cancellation class, with no variable part (no token, type name, identifier or timestamp is interpolated into it). A re-cancel of the now-CANCEL_COMPLETE request is accepted and echoes the terminal event.
Emulator: the cancel is accepted and the request does settle at CANCEL_COMPLETE (so the request state machine now transitions), but ProgressEvent.StatusMessage is absent/None on both the CANCEL_IN_PROGRESS event the cancel answers with and the CANCEL_COMPLETE event GetResourceRequestStatus later reports. The replay fails with: AssertionError: expected the cancelled request's StatusMessage 'User Initialized the Cancellation', got None.
Why: StatusMessage is the only field that says WHY a request ended the way it did. A client that distinguishes "a user/operator cancelled this" from "the cancellation path ran for some other reason" (audit trails, IaC drivers that surface the reason to the user, retry logic that must not retry a user-initiated abort) gets nothing to read, and any code asserting on or logging the explanation of a cancelled request sees an empty field against the emulator where AWS always supplies one.
Fix: the request state machine in the cloudcontrol crate's state module, where a cancel is recorded and where the cancellation falls due, must set the record's status_message to the user-cancellation sentence so the shared ProgressEvent renderer reports it for CANCEL_IN_PROGRESS and CANCEL_COMPLETE alike (today only the FAILED branch ever populates status_message).
Traces: cloudcontrol.cancel-in-flight-request-reaches-cancel-complete. Code citations: services/service-com-amazonaws-cloudcontrol-cloudapiservice/src/state.rs lines 792-805 (cancel_request: sets operation_status to CANCEL_IN_PROGRESS and drops the owed work, never touches status_message), lines 522-542 (materialise_due: the `None => record.operation_status = STATUS_CANCEL_COMPLETE` branch sets only the status, while the Err branch beside it is the only place status_message is ever written), lines 2076-2094 (progress_event copies record.status_message straight through); services/service-com-amazonaws-cloudcontrol-cloudapiservice/src/ops/cancel_resource_request.rs lines 164-194.

Cause 2 of 2: CreateResource(TypeName="AWS::SNS::Topic") answers ProgressEvent.Identifier = the TopicName from DesiredState, not the topic ARN, so the identifier Cloud Control hands back cannot address the topic.
Failing check: cloudcontrol.e2e-cancel-request-mid-flight-resource-absent
AWS: Smithy as above — ProgressEvent$Identifier (@length 1-1024, pattern ^.+$) is documented as "The primary identifier for the resource.", and it is the same value GetResource$Identifier / UpdateResource$Identifier / DeleteResource$Identifier take. Which property IS the primary identifier is stated by the resource type's registry schema, not by Smithy: for AWS::SNS::Topic the primaryIdentifier is /properties/TopicArn (readOnly), so the identifier is the SNS topic ARN of the form arn:<partition>:sns:<region>:<account-id>:<TopicName> — fixed text "arn:" and ":sns:", with partition, region, account id from the request context and TopicName from DesiredState. TopicName is create-only. Because a Cloud Control AWS::SNS::Topic IS the SNS topic, that identifier is exactly what sns:GetTopicAttributes/SetTopicAttributes/DeleteTopic accept as TopicArn.
Emulator: the identifier reported for a created AWS::SNS::Topic is the bare TopicName. Handing it to the native SNS API is refused: botocore.errorfactory.InvalidParameterException: An error occurred (InvalidParameter) when calling the GetTopicAttributes operation: Invalid parameter: TopicArn Reason: An ARN must have at least 6 elements, not 1 — i.e. the value contained no ':' at all, so it was a name and not an ARN. The create itself reaches SUCCESS and Cloud Control's own GetResource/ListResources echo that same name, so the inconsistency is only visible against the owning service.
Why: Cloud Control is the generic control plane a client uses without knowing the owning service's API, and ProgressEvent.Identifier is the ONLY handle it is given. A client that creates a topic through Cloud Control and then publishes to it, sets its attributes or deletes it through SNS (all of which take a TopicArn) is refused InvalidParameter even though Cloud Control reported SUCCESS; a client that persists the value as the resource's primaryIdentifier records something it cannot re-resolve, and generic IaC tooling that round-trips primaryIdentifier breaks for every ARN-identified type.
Fix: the cloudcontrol crate's identifier derivation must resolve the type's primary identifier from the resource type's schema instead of guessing "<ResourceSegment>Name" first, and for a type whose primary identifier is a readOnly ARN it must synthesize that ARN (partition/region/account from the request context, remaining parts from the desired state), report it as ProgressEvent.Identifier, and record it under the identifier property (TopicArn) so GetResource/ListResources/UpdateResource/DeleteResource all resolve the same value.
Traces: cloudcontrol.e2e-cancel-request-mid-flight-resource-absent. Code citations: services/service-com-amazonaws-cloudcontrol-cloudapiservice/src/state.rs lines 1036-1046 (identifier_property_candidates: "<Resource>Name", "<Resource>Id", "<Resource>Arn", "Name", "Id", "Arn" — most specific first, so TopicName wins over the ARN) and lines 1098-1115 (derive_identifier).

Relation to open issues (neither carries these checks' replay keys, so neither covers them):
- 20261105T230817Z-cloudcontrol-a-mid-flight-cancelresourcerequest-never-reaches-cancel.md lists cloudcontrol.e2e-cancel-request-mid-flight-resource-absent, but for a DIFFERENT, earlier cause (CancelResourceRequest answering 501 NotImplemented and the create having no non-terminal window). That cause no longer reproduces: the cancel is now accepted and the request now reaches a terminal status, and the check now fails further on, at the native SNS read, on the identifier shape described in Cause 2.
- 20261102T043935Z-cloudcontrol-createresource-reports-the-resource-name-as-progressevent.md describes the same code path as Cause 2 for AWS::IAM::ManagedPolicy and AWS::StepFunctions::StateMachine only; Cause 2 is the AWS::SNS::Topic instance of it and must be re-replayed through this check.
- 20261102T095426Z-cloudcontrol-cancelresourcerequest-stub.md asks only for the operation to exist; it states neither the cancellation StatusMessage of Cause 1 nor the identifier requirement of Cause 2.

## Evidence
Cause 1 — check cloudcontrol.cancel-in-flight-request-reaches-cancel-complete (verdict: mismatch).
Flow: CreateResource(TypeName="AWS::CloudFormation::Stack", DesiredState={"StackName":<unique>,"TemplateBody":<a stack whose only resource is an AWS::CloudFormation::WaitCondition with a CreationPolicy ResourceSignal, so the create stays IN_PROGRESS until the check signals it>}) -> poll GetResourceRequestStatus until the event names its stack and is IN_PROGRESS -> CancelResourceRequest(RequestToken=<token>) -> poll to a terminal OperationStatus.
Expected (AWS): the cancel is accepted (200) with a ProgressEvent for that same RequestToken, Operation "CREATE", OperationStatus CANCEL_IN_PROGRESS or CANCEL_COMPLETE; the request then settles at OperationStatus "CANCEL_COMPLETE" with StatusMessage exactly "User Initialized the Cancellation" (fixed text, no interpolated values); a repeat CancelResourceRequest is accepted and echoes OperationStatus CANCEL_COMPLETE.
Actual (emulator):
  File "check_cloudcontrol_cancel_in_flight_request_reaches_cancel_complete.py", line 35, in run
    assert final.get("StatusMessage") == "User Initialized the Cancellation", (
  AssertionError: expected the cancelled request's StatusMessage 'User Initialized the Cancellation', got None
The status transition itself is right (CANCEL_COMPLETE was reached); only StatusMessage is missing. The emulator writes record.status_message exclusively on the FAILED path (state.rs line 534); the CANCEL_COMPLETE path (state.rs line 540) sets the status alone, and progress_event (state.rs line 2088) therefore serializes StatusMessage as absent.

Cause 2 — check cloudcontrol.e2e-cancel-request-mid-flight-resource-absent (verdict: error).
Flow (positive control, after the cancelled-create half of the check passes): control_event = <terminal ProgressEvent of CreateResource(TypeName="AWS::SNS::Topic", DesiredState={"TopicName":<unique>,"Tags":[{"Key":"sim2real","Value":<check>}]})>, which reaches OperationStatus "SUCCESS"; control_id = control_event["Identifier"]; then sns.get_topic_attributes(TopicArn=control_id).
Expected (AWS): control_id is the topic's ARN (the type's primaryIdentifier /properties/TopicArn), shaped arn:<partition>:sns:<region>:<account-id>:<TopicName>, and sns:GetTopicAttributes(TopicArn=control_id) answers 200 with Attributes["TopicArn"] == control_id.
Actual (emulator):
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.errorfactory.InvalidParameterException: An error occurred (InvalidParameter) when calling the GetTopicAttributes operation: Invalid parameter: TopicArn Reason: An ARN must have at least 6 elements, not 1
"not 1" is the diagnostic: the value Cloud Control reported as the primary identifier split into a single element, i.e. it contained no ':' and is the bare TopicName from DesiredState, not an ARN. (The generated topic name itself is a per-run value; the requirement is the ARN SHAPE, not any particular name.) This is the same derivation as state.rs identifier_property_candidates/derive_identifier, whose candidate list starts with "<Resource>Name" — "TopicName" for AWS::SNS::Topic.

## Steps to Reproduce
Cause 1 (CancelResourceRequest StatusMessage):
1. Boot the emulator and point botocore at it; cc = client("cloudcontrol"), cfn = client("cloudformation").
2. template = json.dumps({"Resources": {"Gate": {"Type": "AWS::CloudFormation::WaitCondition", "CreationPolicy": {"ResourceSignal": {"Count": 1, "Timeout": "PT10M"}}}}}) — a stack that stays CREATE_IN_PROGRESS until signalled.
3. ev = cc.create_resource(TypeName="AWS::CloudFormation::Stack", DesiredState=json.dumps({"StackName": "<unique>", "TemplateBody": template}))["ProgressEvent"]; poll cc.get_resource_request_status(RequestToken=ev["RequestToken"])["ProgressEvent"] until it is IN_PROGRESS and names an Identifier.
4. cc.cancel_resource_request(RequestToken=ev["RequestToken"]) — 200 with OperationStatus CANCEL_IN_PROGRESS (or CANCEL_COMPLETE).
5. Poll cc.get_resource_request_status(RequestToken=ev["RequestToken"])["ProgressEvent"] to a terminal status.
   Observed: OperationStatus "CANCEL_COMPLETE" with no StatusMessage key at all.
   Expected: OperationStatus "CANCEL_COMPLETE" and StatusMessage "User Initialized the Cancellation".
6. cc.cancel_resource_request(RequestToken=<same token>) again — must be accepted and echo CANCEL_COMPLETE (and the same StatusMessage).

Cause 2 (AWS::SNS::Topic primary identifier):
1. cc = client("cloudcontrol"), sns = client("sns"), sts = client("sts").
2. name = "<unique>"; ev = cc.create_resource(TypeName="AWS::SNS::Topic", DesiredState=json.dumps({"TopicName": name, "Tags": [{"Key": "sim2real", "Value": "x"}]}))["ProgressEvent"]; poll cc.get_resource_request_status(RequestToken=ev["RequestToken"]) to terminal — it reaches SUCCESS.
3. Read ProgressEvent.Identifier.
   Observed: the bare TopicName (no colons).
   Expected: arn:aws:sns:<region>:<account-id>:<name> — the type's primaryIdentifier /properties/TopicArn.
4. sns.get_topic_attributes(TopicArn=<that identifier>).
   Observed: InvalidParameter — "Invalid parameter: TopicArn Reason: An ARN must have at least 6 elements, not 1".
   Expected: 200 with Attributes["TopicArn"] equal to the identifier Cloud Control reported.
5. cc.get_resource(TypeName="AWS::SNS::Topic", Identifier=arn:aws:sns:<region>:<account-id>:<name>) and cc.list_resources(TypeName="AWS::SNS::Topic") must both resolve/report that ARN as the identifier, so Cloud Control and SNS agree on one handle.
