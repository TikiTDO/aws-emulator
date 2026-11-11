Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol.list-resource-requests-and-get-status-agree-on-one-request cloudcontrol.list-resource-requests-honors-status-filter cloudcontrol.list-resource-requests-time-window-filter
Issue-Shape-Id: com.amazonaws.cloudcontrol#CloudApiService
Issue-Key: cloudcontrol|DescribeTable|ResourceNotFoundException
Issue-Key: cloudcontrol.list-resource-requests-honors-status-filter|run#18|AssertionError|assert not_found_message == expected, f"expected message {expected!r}, got {not_found_message!r}"
Issue-Key: cloudcontrol.list-resource-requests-time-window-filter|run#23|AssertionError|assert not_found_message == expected, f"expected message {expected!r}, got {not_found_message!r}"
Issue-Commit: f3853c2ce3831f59f19a9d4fb097e3f9d5244398

# cloudcontrol: 3 net-new causes — CreateResource reports SUCCESS for an AWS::DynamoDB::Table that DynamoDB never gets, and the SNS/SQS handler not-found messages are mis-rendered

> _Note: this issue was identified against the code base at commit `f3853c2ce3831f59f19a9d4fb097e3f9d5244398`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateResource

## Description
Three net-new root causes on the cloudcontrol request/read surface. Each is grounded in a failing behavioral check that no open issue carries the replay key for (the suite marked none of these three OWNED or ALREADY FILED); for causes 2 and 3 an open issue describes the same defect for OTHER checks and is named below so the fixer can close them together.

Cause 1 of 3: a CloudControl CreateResource of TypeName=AWS::DynamoDB::Table runs to a terminal OperationStatus=SUCCESS, and both request views (GetResourceRequestStatus and ListResourceRequests) agree on that settled record, but no DynamoDB table of that name exists — the native DescribeTable refuses it.
Failing check: cloudcontrol.list-resource-requests-and-get-status-agree-on-one-request
AWS:      Smithy (services/service-com-amazonaws-cloudcontrol-cloudapiservice/smithy-model/com-amazonaws-cloudcontrol-cloudapiservice.json): CreateResource takes required TypeName (pattern ^[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}$) and required DesiredState and answers a ProgressEvent; GetResourceRequestStatus answers the ProgressEvent for a RequestToken; ProgressEvent.OperationStatus is the enum PENDING | IN_PROGRESS | SUCCESS | FAILED | CANCEL_IN_PROGRESS | CANCEL_COMPLETE and the model documents SUCCESS as "The resource operation has successfully completed." Cloud Control is a facade over the owning type's handler, so a CREATE of AWS::DynamoDB::Table that terminates SUCCESS means DynamoDB holds that table and DescribeTable answers 200 with Table.TableStatus == "ACTIVE" for the requested TableName. The model carries NotStabilizedException (client, httpError 400) and HandlerFailureException (server, httpError 502) precisely so a facade that cannot stand the resource up reports FAILED or refuses instead of reporting SUCCESS.
Emulator: the request is accepted non-terminal, materialises to SUCCESS with Identifier equal to the requested TableName, and is reported identically by ListResourceRequests — but the effect never reaches the DynamoDB service state, so the immediately following DynamoDB DescribeTable of that name is refused ResourceNotFoundException with message "Requested resource not found: Table: <TableName> not found" (fixed text "Requested resource not found: Table: " + the TableName from the request + " not found"). DynamoDB is behaving correctly for a table that was never created; the defect is the SUCCESS verdict.
Why:      Cloud Control is the generic control plane a client uses without knowing the owning service's API. A client that creates a table there, polls to SUCCESS (the documented stabilization barrier) and then issues DescribeTable/PutItem gets a not-found from a resource the control plane said was ready, and no modeled signal (FAILED, NotStabilizedException) ever fires, so "created" and "not created" are indistinguishable.
Fix:      the cloudcontrol crate's resource-effect application — the code that materialises a request record's create/delete effect must apply it to the state of the service owning TypeName (DynamoDB for AWS::DynamoDB::Table) before the record may reach SUCCESS, and must terminate the request FAILED (or refuse with the modeled NotStabilizedException/HandlerFailureException) when it cannot. Open issue 20261102T041105Z-cloudcontrol-dynamodb-cloudcontrol-createresource-deleteresource-reach-a-terminal-success-without.md describes this same split-store defect for a DIFFERENT check (cloudcontrol__dynamodb.cloudcontrol-success-means-the-native-table-is-already-active-and-writable) and does not carry this check's replay key; fixing the effect application should close both.
Traces:   cloudcontrol.list-resource-requests-and-get-status-agree-on-one-request — it creates the table through Cloud Control, grades the two request views against each other (those assertions pass), and then fails on the native DescribeTable.

Cause 2 of 3: the AWS::SNS::Topic handler not-found message that GetResource relays omits the "(SDK Attempt Count: <n>)" segment.
Failing check: cloudcontrol.list-resource-requests-honors-status-filter
AWS:      Smithy: GetResource's errors include ResourceNotFoundException (errorKind client, httpError 404). Recorded behavior only for the text — the model has no message template. Error.Message is the SNS handler's relayed refusal: "AWS::SNS::Topic Handler returned status FAILED: Topic does not exist (Service: Sns, Status Code: 404, Request ID: <request id>) (SDK Attempt Count: <n>) (HandlerErrorCode: NotFound, RequestToken: <request token>)". Fixed text: "AWS::SNS::Topic Handler returned status FAILED: Topic does not exist (Service: Sns, Status Code: 404, Request ID: ", ") (SDK Attempt Count: ", ") (HandlerErrorCode: NotFound, RequestToken: ", ")". Variable parts, taken from the call and state: the downstream SNS request id, the SDK attempt count integer, and the Cloud Control RequestToken; those vary per call and are not requirements.
Emulator: serves the right code (ResourceNotFoundException) and every other segment but drops "(SDK Attempt Count: <n>) " entirely, so the Request ID group is followed directly by the HandlerErrorCode group.
Why:      the relayed handler message is the only place a synchronous Cloud Control refusal exposes the downstream service, status code, request id and attempt count of a handler failure; a client parsing or regex-matching AWS's handler-message shape fails on an otherwise correct message.
Fix:      the handler-refusal message builder on the cloudcontrol GetResource path must emit the "(SDK Attempt Count: <n>)" group between the Request ID group and the HandlerErrorCode group for AWS::SNS::Topic. Open issue 20261109T222806Z-cloudcontrol-the-aws-sns-topic-handler-not-found.md states the same cause for cloudcontrol.list-resources-filters-by-type-name and cloudcontrol.timestamp-property-precision-preserved only; it does not carry this check's replay key.
Traces:   cloudcontrol.list-resource-requests-honors-status-filter — creates a topic, grades the CREATE/SUCCESS and DELETE/FAILED list filters (those pass), deletes it, then grades the GetResource not-found message.

Cause 3 of 3: the AWS::SQS::Queue handler not-found message renders the identifier as the primary-identifier JSON object instead of the bare identifier the caller passed.
Failing check: cloudcontrol.list-resource-requests-time-window-filter
AWS:      Smithy: GetResource takes required Identifier (length 1..1024, pattern ^.+$) and models ResourceNotFoundException (client, 404, doc: "A resource with the specified identifier can't be found."). Recorded behavior only for the text: "<TypeName> Handler returned status FAILED: Resource of type '<TypeName>' with identifier '<identifier>' was not found. (HandlerErrorCode: NotFound, RequestToken: <token>)". Fixed text: " Handler returned status FAILED: Resource of type '", "' with identifier '", "' was not found. (HandlerErrorCode: NotFound, RequestToken: ", ")". Variable parts: TypeName and Identifier from the request plus the server-minted RequestToken. For AWS::SQS::Queue the recorded identifier is the queue URL exactly as the caller passed it — a bare string, no JSON wrapper.
Emulator: substitutes the resource-model object, i.e. '{"/properties/QueueUrl":"<queue url>"}' where AWS has '<queue url>'; prefix, inner sentence, HandlerErrorCode and RequestToken all match.
Why:      a client that correlates the refusal back to the identifier it sent (ignore-if-not-found logic, drift reconciliation, log scraping keyed on the queue URL) never matches, and Cloud Control's internal /properties keying leaks into a user-visible message.
Fix:      the per-type identifier rendering in the shared not-found refusal composer behind GetResource/UpdateResource/DeleteResource in the cloudcontrol crate. The rendering is PER RESOURCE TYPE and must not be flipped globally: open issue 20261110T221719Z-cloudcontrol-the-aws-sqs-queue-handler-not-found.md records the same defect for ten other checks and notes that AWS DOES use the JSON /properties form for AWS::Logs::LogGroup in this same slot. That issue does not carry this check's replay key.
Traces:   cloudcontrol.list-resource-requests-time-window-filter — creates an SQS queue, grades the SUCCESS/FAILED list filters (those pass), deletes it, then grades the GetResource not-found message.

## Evidence
All quotes are from the replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout. Generated names, request ids and tokens fill the templates and are not part of any requirement.

Cause 1 — cloudcontrol.list-resource-requests-and-get-status-agree-on-one-request (verdict: error). The Cloud Control create of AWS::DynamoDB::Table reached terminal SUCCESS, both views agreed on the record and the identifier equalled the requested TableName; the body then failed on the native read:
  File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeTable operation: Requested resource not found: Table: sim2real-behav-ccviews-53b4ce9ee40f not found
Expected: DescribeTable answers 200 with Table.TableStatus == "ACTIVE" and Table.TableName equal to the name Cloud Control reported SUCCESS for. (The table name shown is a per-run value.)

Cause 2 — cloudcontrol.list-resource-requests-honors-status-filter (verdict: mismatch), on the GetResource of the deleted topic. The check reads the per-call request id and attempt count back out of the served message before comparing, so only fixed segments are graded:
  expected message 'AWS::SNS::Topic Handler returned status FAILED: Topic does not exist (Service: Sns, Status Code: 404, Request ID: <id>) (SDK Attempt Count: <n>) (HandlerErrorCode: NotFound, RequestToken: <token>)'
  got 'AWS::SNS::Topic Handler returned status FAILED: Topic does not exist (Service: Sns, Status Code: 404, Request ID: 186f744e-657d-472c-83aa-6f98cf0ed09c) (HandlerErrorCode: NotFound, RequestToken: aecacc57-f8b7-4fef-ad0a-26f68fa982b8)'
The error code is correct (ResourceNotFoundException, 404); the '(SDK Attempt Count: <n>)' group is simply absent. The ids above are per-run values.

Cause 3 — cloudcontrol.list-resource-requests-time-window-filter (verdict: mismatch), on the GetResource of the deleted queue:
  expected message "AWS::SQS::Queue Handler returned status FAILED: Resource of type 'AWS::SQS::Queue' with identifier 'https://sqs.<region>.amazonaws.com/<account>/<queue-name>' was not found. (HandlerErrorCode: NotFound, RequestToken: <token>)"
  got "AWS::SQS::Queue Handler returned status FAILED: Resource of type 'AWS::SQS::Queue' with identifier '{\"/properties/QueueUrl\":\"https://sqs.<region>.amazonaws.com/<account>/<queue-name>\"}' was not found. (HandlerErrorCode: NotFound, RequestToken: <token>)"
Error code (ResourceNotFoundException) and RequestToken agree on both sides; only the identifier rendering diverges. Host, account, queue name and token are per-run values.

## Steps to Reproduce
Boot the emulator from this checkout and point botocore at it: cc = client("cloudcontrol", endpoint_url=<emulator>), ddb = client("dynamodb", endpoint_url=<emulator>).

Cause 1 (replay cloudcontrol.list-resource-requests-and-get-status-agree-on-one-request):
1. desired = json.dumps({"TableName": "<fresh-name>", "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}], "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}], "BillingMode": "PAY_PER_REQUEST"}); ev = cc.create_resource(TypeName="AWS::DynamoDB::Table", DesiredState=desired)["ProgressEvent"].
2. Poll cc.get_resource_request_status(RequestToken=ev["RequestToken"])["ProgressEvent"] until OperationStatus is terminal; observe SUCCESS.
3. ddb.describe_table(TableName="<fresh-name>").
   Required: 200 with Table.TableStatus == "ACTIVE". Observed: ResourceNotFoundException "Requested resource not found: Table: <fresh-name> not found".

Cause 2 (replay cloudcontrol.list-resource-requests-honors-status-filter):
1. cc.create_resource(TypeName="AWS::SNS::Topic", DesiredState=json.dumps({"TopicName": "<fresh-name>"})), poll to SUCCESS, keep ProgressEvent.Identifier (the topic ARN).
2. cc.delete_resource(TypeName="AWS::SNS::Topic", Identifier=<that ARN>), poll to SUCCESS.
3. cc.get_resource(TypeName="AWS::SNS::Topic", Identifier=<that ARN>) and read Error.Message.
   Required: '... Request ID: <id>) (SDK Attempt Count: <n>) (HandlerErrorCode: NotFound, RequestToken: <token>)'. Observed: the '(SDK Attempt Count: <n>)' group is missing.

Cause 3 (replay cloudcontrol.list-resource-requests-time-window-filter):
1. cc.create_resource(TypeName="AWS::SQS::Queue", DesiredState=json.dumps({"QueueName": "<fresh-name>"})), poll to SUCCESS, keep ProgressEvent.Identifier (the queue URL).
2. cc.delete_resource(TypeName="AWS::SQS::Queue", Identifier=<that URL>), poll to SUCCESS.
3. cc.get_resource(TypeName="AWS::SQS::Queue", Identifier=<that URL>) and read Error.Message.
   Required: "... with identifier '<that URL>' was not found. ..." — the bare identifier as passed. Observed: '{"/properties/QueueUrl":"<that URL>"}'.

Re-replay all three checks in trace_ids to confirm the fix.
