Issue-Source: trace-sweep-divergence
Issue-Trace: cloudcontrol-list-resource-requests-in-progress
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudcontrol-list-resource-requests-in-progress
Issue-Shape-Id: com.amazonaws.cloudcontrol#CloudApiService
Issue-Key: cloudcontrolapi|ListResourceRequests|2xx->2xx|$.ResourceRequestStatusSummaries[*]
Issue-Commit: 14c2f807276b097453c7eebbf32725a3995b3836

# cloudcontrolapi ListResourceRequests summaries diverge from AWS: bare resource name as Identifier plus a ResourceModel AWS omits

> _Note: this issue was identified against the code base at commit `14c2f807276b097453c7eebbf32725a3995b3836`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListResourceRequests

## Description
ONE net-new root cause in the cloudcontrolapi (Cloud Control API, sdkId CloudControl) service: the ProgressEvent summaries ListResourceRequests reports do not match the summary AWS reports for the same completed request. A fixer must close this one cause, which has two observable differences in the SAME summary element, before submitting. Model citations are from services/service-com-amazonaws-cloudcontrol-cloudapiservice/smithy-model/com-amazonaws-cloudcontrol-cloudapiservice.json.

Cause 1 of 1: the ResourceRequestStatusSummaries element for a completed CREATE renders the resource NAME as Identifier where the type's primary identifier is an ARN, and carries a ResourceModel member that AWS's list summary does not.

AWS: ListResourceRequests "Returns existing resource operation requests" and its output member ResourceRequestStatusSummaries is a list of ProgressEvent (Smithy: ResourceRequestStatusSummaries member -> ProgressEvent). ProgressEvent.Identifier is documented as "The primary identifier for the resource" (@length 1..1024, pattern ^.+$). For a resource type whose registry schema declares an ARN-valued primary identifier - AWS::SNS::Topic's /properties/TopicArn - the primary identifier is the full ARN, so the Identifier reported in each list summary is of the form arn:aws:sns:<region>:<account-id>:<topic-name>, with region, account id and topic name taken from the request and the emulator's own partition/account configuration. ProgressEvent.ResourceModel (@sensitive, the "JSON string containing the resource model") is modeled on the shape but, per the recorded wire response (recorded behavior only - the model marks no member of this list as suppressed), the summaries AWS returns from ListResourceRequests carry TypeName, Identifier, RequestToken, Operation, OperationStatus and EventTime and NO ResourceModel: the resource model is surfaced by GetResourceRequestStatus / GetResource, not by the request listing.

Emulator: for the SUCCESS create of an AWS::SNS::Topic, the summary element it returns is {"TypeName": "AWS::SNS::Topic", "Identifier": "<bare topic name>", "RequestToken": "<token>", "Operation": "CREATE", "OperationStatus": "SUCCESS", "EventTime": <epoch-seconds>, "ResourceModel": "{\"TopicName\":\"<bare topic name>\"}"} - i.e. the bare topic name instead of the topic ARN in Identifier, plus an extra ResourceModel member. HTTP status matches (200); only the summary element diverges.

Why: a client that enumerates in-flight and recent requests with ListResourceRequests and then addresses a resource by the Identifier it was handed (GetResource/UpdateResource/DeleteResource, or by parsing the ARN for region/account) is given a value AWS never emits, so the round-trip addresses a resource that does not resolve and ARN parsing yields nothing. The spurious ResourceModel additionally publishes a @sensitive payload on a listing that AWS keeps free of it, so a caller that logs or diffs listings leaks resource state and sees a shape no AWS response has.

Fix: the summary projection used by the ListResourceRequests handler - crate::state::progress_event(record), called at services/service-com-amazonaws-cloudcontrol-cloudapiservice/src/ops/list_resource_requests.rs where the page is mapped into resource_request_status_summaries - must (a) render the type's primary identifier, ARN-valued types included, rather than the internally minted name, and (b) project a LIST-shaped summary that omits ResourceModel, distinct from the full ProgressEvent that GetResourceRequestStatus returns. This belongs in the state/projection layer (src/state.rs) plus the listing handler, not in the serializer.

Why it is not already covered: open issue 20261103T024056Z-cloudcontrolapi-resource-identifiers-and-deleteresource-acceptance-diverge-from.md reports the ARN-vs-name identifier defect as GetResourceRequestStatus reports it, and 20261102T043935Z-cloudcontrol-createresource-reports-the-resource-name-as-progressevent.md reports it on the CreateResource response; the replay reports NO owning issue for this step, which is the ListResourceRequests summary, and neither open issue mentions the extra ResourceModel member on list summaries. A generalised identifier derivation would close the Identifier half of this cause together with those, but the ResourceModel half is specific to this listing.

Traces: cloudcontrol-list-resource-requests-in-progress#task#3 (1 affected step).

## Evidence
All quotes are from the replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, score_pct=62.8). Generated topic names, request tokens and timestamps fill the templates and are not part of the requirement.

Fixture cloudcontrol-list-resource-requests-in-progress, step task#3, operation ListResourceRequests, both sides HTTP 200:

  "$.ResourceRequestStatusSummaries[0]: missing (expected {'EventTime': '<shaped-not-asserted>', 'Identifier': 'arn:aws:sns:us-west-2:000000000000:sim2real-wire-cloudcontrol-topic-6973222e', 'Operation': 'CREATE', 'OperationStatus': 'SUCCESS', 'RequestToken': 'e021b1bf-cb28-480a-9a18-139a2ccd2b5e', 'TypeName': 'AWS::SNS::Topic'}, got {'TypeName': 'AWS::SNS::Topic', 'Identifier': 'sim2real-wire-cloudcontrol-topic-6973222e', 'RequestToken': 'e021b1bf-cb28-480a-9a18-139a2ccd2b5e', 'Operation': 'CREATE', 'OperationStatus': 'SUCCESS', 'EventTime': 1794425727.1933494, 'ResourceModel': '{\"TopicName\":\"sim2real-wire-cloudcontrol-topic-6973222e\"}'})"

What is required, and what is this run's value:
  - REQUIRED (fixed structure): the summary carries TypeName, Identifier, RequestToken, Operation, OperationStatus, EventTime and NO ResourceModel member.
  - REQUIRED (template): Identifier is the type's primary identifier - for AWS::SNS::Topic the ARN form arn:aws:sns:<region>:<account-id>:<topic-name>. The literal 'us-west-2', '000000000000' and 'sim2real-wire-cloudcontrol-topic-6973222e' above are this run's region, account and requested TopicName; the requirement is the ARN FORM built from the request and the emulator's configured partition/account, not those values.
  - NOT a requirement: EventTime is an explicit shape-only assertion ('<shaped-not-asserted>'); the RequestToken value matched and is per-run.

No other key of the element diverged: TypeName, RequestToken, Operation and OperationStatus are already correct, which isolates the defect to the Identifier derivation and the extra ResourceModel.

## Steps to Reproduce
Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1 (fixture cloudcontrol-list-resource-requests-in-progress, step task#3), or drive it with botocore:

  cc = boto3.client("cloudcontrol", endpoint_url=<emulator>)
  r = cc.create_resource(TypeName="AWS::SNS::Topic", DesiredState=json.dumps({"TopicName": "<name>"}))
  # wait for the request to reach OperationStatus SUCCESS
  cc.list_resource_requests(ResourceRequestStatusFilter={"Operations": ["CREATE"], "OperationStatuses": ["SUCCESS"]})

Expected: HTTP 200 and a ResourceRequestStatusSummaries element whose Identifier is the topic ARN arn:aws:sns:<region>:<account-id>:<name>, with members TypeName, Identifier, RequestToken, Operation, OperationStatus, EventTime and no ResourceModel.
Actual: HTTP 200 with Identifier set to the bare topic name "<name>" and an additional ResourceModel member carrying the client-supplied model JSON.
