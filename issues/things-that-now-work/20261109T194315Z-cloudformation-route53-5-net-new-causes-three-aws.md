Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__route53.deploy-route53-cidrcollection-delegationset-trafficpolicy-round-trip-and-delete cloudformation__route53.deploy-route53-trafficpolicy-invalid-document-rejected cloudformation__route53.deploy-route53-hostedzone-round-trip-and-delete cloudformation__route53.deploy-route53-recordset-and-recordsetgroup-round-trip-and-delete cloudformation__route53.cfn-route53-recordset-hostedzone-id-and-name-both-rejected cloudformation__route53.deploy-route53-recordset-nonexistent-hostedzone-rejected
Issue-Key: cloudformation__route53|CreateStack|ValidationError
Issue-Key: cloudformation__route53.deploy-route53-hostedzone-round-trip-and-delete|stack_outputs#8|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: cloudformation__route53.deploy-route53-recordset-and-recordsetgroup-round-trip-and-delete|run#12|AssertionError|assert outputs["RecRef"] == txt_name.rstrip("."), f"RecordSet Ref {outputs['RecRef']!r}"
Issue-Key: cloudformation__route53.cfn-route53-recordset-hostedzone-id-and-name-both-rejected|run#20|AssertionError|assert status == "ROLLBACK_COMPLETE", f"both members: expected ROLLBACK_COMPLETE, got {status!r}"
Issue-Key: cloudformation__route53.deploy-route53-recordset-nonexistent-hostedzone-rejected|run#15|AssertionError|assert got == _REJECTED, f"handler reason {got!r}, expected {_REJECTED!r}"
Issue-Commit: 7e082a9dfef7650059a714e1b731386462df20dc

# cloudformation__route53: 5 net-new causes — three AWS::Route53::* resource types unregistered, stack Outputs carry no OutputValue, RecordSet Ref keeps the trailing dot, the HostedZoneId/HostedZoneName oneOf check is not enforced, and the NoSuchHostedZone handler reason/code diverge

> _Note: this issue was identified against the code base at commit `7e082a9dfef7650059a714e1b731386462df20dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Five distinct root causes in CloudFormation's Route 53 resource coverage, all grounded in failing behavioral checks of the cloudformation__route53 group. Each must be closed before submitting.

Cause 1 of 5: AWS::Route53::CidrCollection, AWS::Route53::DelegationSet and AWS::Route53::TrafficPolicy are not registered resource types, so CreateStack refuses the template outright.
AWS: these three types are documented CloudFormation resource types (aws-resource-route53-cidrcollection, aws-resource-route53-delegationset, aws-resource-route53-trafficpolicy); a template declaring them is accepted by CreateStack and provisioned against Route 53. Smithy: CreateStack models ValidationError only for genuinely invalid templates; recorded behavior shows CreateStack returning a StackId for these templates.
Emulator: CreateStack answers 400 ValidationError with Message "Template format error: Unrecognized resource types: [AWS::Route53::CidrCollection, AWS::Route53::DelegationSet, AWS::Route53::TrafficPolicy]" (and "[AWS::Route53::TrafficPolicy]" for the traffic-policy-only template). Fixed text: "Template format error: Unrecognized resource types: "; variable part: the bracketed list of the template's unrecognized Type strings. Nothing is created, so no round trip, no delete and no invalid-document rejection can be observed.
Why: every stack (and every CDK app) that declares a CIDR collection, a reusable delegation set or a traffic policy fails at CreateStack, so these Route 53 resources are unreachable through infrastructure-as-code.
Fix: the template processor's resource-type registry plus the three resource handlers (create/read/update/delete mapped onto Route 53 CreateCidrCollection/ChangeCidrCollection, CreateReusableDelegationSet and CreateTrafficPolicy).
Traces: cloudformation__route53.deploy-route53-cidrcollection-delegationset-trafficpolicy-round-trip-and-delete, cloudformation__route53.deploy-route53-trafficpolicy-invalid-document-rejected.

Cause 2 of 5: DescribeStacks returns Outputs rows with an OutputKey but no OutputValue.
AWS: Smithy: cloudformation#Output has members OutputKey and OutputValue ("The value associated with the output."); for a CREATE_COMPLETE stack AWS populates OutputValue with the resolved intrinsic — here {"Ref": "Zone"} (the hosted-zone id) and {"Fn::Join": [",", {"Fn::GetAtt": ["Zone", "NameServers"]}]} (the zone's delegation-set name servers, comma-joined).
Emulator: the stack reaches CREATE_COMPLETE but at least one DescribeStacks Outputs entry omits OutputValue entirely — the check's {o["OutputKey"]: o["OutputValue"]} projection raises KeyError: 'OutputValue'. So Ref/Fn::GetAtt on AWS::Route53::HostedZone (NameServers in particular) is not resolved into the output.
Why: any client reading stack outputs to learn the created zone id or its name servers gets a KeyError / missing field instead of a value; cross-stack wiring and CDK outputs break on a stack the emulator itself reports as successfully created.
Fix: the stack output-resolution step of the create/update engine (intrinsic resolution for Ref and Fn::GetAtt against the AWS::Route53::HostedZone handler's attributes) and the DescribeStacks serializer that emits Output.
Traces: cloudformation__route53.deploy-route53-hostedzone-round-trip-and-delete.

Cause 3 of 5: AWS::Route53::RecordSet Ref hands back the record name with its trailing dot.
AWS: the AWS::Route53::RecordSet return value is the record's name, without the trailing dot — for a record declared as "txt.<zone>.test." the Ref (and the physical resource id) is "txt.<zone>.test". Recorded behavior confirms the dot is stripped.
Emulator: the stack Output fed by {"Ref": "Rec"} comes back as the declared FQDN including the trailing dot (observed: 'txt.sim2real-r53rs-<run>.test.'). Fixed shape: the declared Name; the divergence is the retained terminal '.'.
Why: a client that uses the Ref as a DNS name or joins it into a URL, or that compares it to the name it declared minus the dot, gets a value AWS never returns; the names also differ between the Ref and the record-set read back from Route 53.
Fix: the AWS::Route53::RecordSet resource handler's physical-resource-id / Ref computation.
Traces: cloudformation__route53.deploy-route53-recordset-and-recordsetgroup-round-trip-and-delete.

Cause 4 of 5: the AWS::Route53::RecordSet oneOf pre-provisioning schema check (HostedZoneId xor HostedZoneName) is not enforced.
AWS: the resource schema makes HostedZoneId and HostedZoneName mutually exclusive, and CloudFormation rejects a resource declaring both before provisioning: the stack ends ROLLBACK_COMPLETE, DescribeEvents records exactly one VALIDATION_ERROR operation event for the resource with ValidationStatusReason "#: #: 2 subschemas matched instead of one" and ValidationPath "/Resources/<LogicalId>/Properties", ListStackResources lists no resources, and no record set is created. Fixed text of the reason is "#: #: 2 subschemas matched instead of one"; the variable part of the path is the resource's logical id.
Emulator: the same stack reaches CREATE_COMPLETE ("both members: expected ROLLBACK_COMPLETE, got 'CREATE_COMPLETE'") — both members are accepted, no VALIDATION_ERROR event is recorded, and the record is provisioned.
Why: a template AWS refuses deploys here, so a client gets a record created from an ambiguous declaration and no validation feedback; the mutual-exclusion contract of the resource is silently absent.
Fix: the template processor's resource-schema (oneOf) validation pass and the VALIDATION_ERROR operation-event emission it feeds, for the AWS::Route53::RecordSet schema.
Traces: cloudformation__route53.cfn-route53-recordset-hostedzone-id-and-name-both-rejected.

Cause 5 of 5: the CREATE_FAILED handler reason for a RecordSet naming a hosted zone that does not exist has the wrong HandlerErrorCode and drops the AWS SDK exception suffix.
AWS: the record-set handler surfaces Route 53's own refusal, so StackEvent.ResourceStatusReason (Smithy: cloudformation#StackEvent.ResourceStatusReason, "Success/failure message associated with the resource") carries the message "No hosted zone found with ID: <HostedZoneId> (Service: Route53, Status Code: 404, Request ID: <id>) (SDK Attempt Count: <n>)" with HandlerErrorCode "InvalidRequest". Fixed text: "No hosted zone found with ID: " plus the SDK clause template "(Service: Route53, Status Code: 404, Request ID: <id>) (SDK Attempt Count: <n>)"; variable parts are the HostedZoneId taken from the request and the per-run request id and attempt count (not part of the requirement).
Emulator: the reason is "No hosted zone found with ID: <HostedZoneId>" with HandlerErrorCode "NotFound" — the whole "(Service: Route53, Status Code: 404, Request ID: ...) (SDK Attempt Count: ...)" suffix is missing and the handler error code is NotFound instead of InvalidRequest.
Why: clients and CDK/CLI tooling that classify a rollback by HandlerErrorCode see NotFound where AWS reports InvalidRequest, and a reason parser that expects the SDK exception clause (service name, status code) finds none, so the originating service and HTTP status of the failure are unrecoverable from the event.
Fix: the AWS::Route53::RecordSet resource handler's error mapping — the handler-error-code classification and the SDK-exception-suffix formatting applied when a downstream Route 53 call is refused.
Traces: cloudformation__route53.deploy-route53-recordset-nonexistent-hostedzone-rejected.

## Evidence
Cause 1 — expected: CreateStack accepts the template and returns a StackId. Actual: botocore ClientError "An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Route53::CidrCollection, AWS::Route53::DelegationSet, AWS::Route53::TrafficPolicy]"; and for the traffic-policy template "... Unrecognized resource types: [AWS::Route53::TrafficPolicy]".

Cause 2 — expected: DescribeStacks Stacks[0].Outputs == [{OutputKey: 'ZoneId', OutputValue: <zone id>}, {OutputKey: 'NameServers', OutputValue: '<ns1>,<ns2>,...'}]. Actual: an Outputs entry has no OutputValue member — "KeyError: 'OutputValue'" when projecting {o['OutputKey']: o['OutputValue']}, on a stack the emulator reports CREATE_COMPLETE.

Cause 3 — expected: Output 'RecRef' (={"Ref": "Rec"}) == the declared record name with the trailing dot stripped, e.g. 'txt.<zone>.test'. Actual: 'txt.sim2real-r53rs-aa5fba741cbb.test.' (trailing dot retained). AssertionError: RecordSet Ref 'txt.sim2real-r53rs-aa5fba741cbb.test.'

Cause 4 — expected: stack status ROLLBACK_COMPLETE; DescribeEvents VALIDATION_ERROR for 'R' == ('R', '#: #: 2 subschemas matched instead of one', '/Resources/R/Properties'); ListStackResources empty; the record absent from the zone. Actual: AssertionError: both members: expected ROLLBACK_COMPLETE, got 'CREATE_COMPLETE'.

Cause 5 — expected handler reason (message, HandlerErrorCode) == ('No hosted zone found with ID: Z0000000000000NOTREAL (Service: Route53, Status Code: 404, Request ID: <id>) (SDK Attempt Count: <n>)', 'InvalidRequest'). Actual: ('No hosted zone found with ID: Z0000000000000NOTREAL', 'NotFound') — the suite's masking of the account digits renders the id as 'Z<account>0NOTREAL'; the divergence is the absent SDK clause and the error code, not the masked id.

## Steps to Reproduce
Cause 1: cloudformation.create_stack with a template whose Resources declare AWS::Route53::CidrCollection, AWS::Route53::DelegationSet and/or AWS::Route53::TrafficPolicy. Observe the 400 ValidationError 'Unrecognized resource types' instead of a StackId.

Cause 2: create_stack with {Resources: {Zone: {Type: AWS::Route53::HostedZone, Properties: {Name: '<name>.test.', HostedZoneConfig: {Comment: 'c'}}}}, Outputs: {ZoneId: {Value: {Ref: Zone}}, NameServers: {Value: {'Fn::Join': [',', {'Fn::GetAtt': ['Zone','NameServers']}]}}}}; wait for CREATE_COMPLETE; call describe_stacks and read Stacks[0].Outputs — an entry has no OutputValue.

Cause 3: create_stack with a HostedZone plus a RecordSet 'Rec' (Name 'txt.<base>.test.', Type TXT, TTL 300) and Output RecRef = {Ref: Rec}; wait CREATE_COMPLETE; read the output — it keeps the trailing dot.

Cause 4: create_stack with a single AWS::Route53::RecordSet declaring BOTH HostedZoneId and HostedZoneName (plus Name/Type/TTL/ResourceRecords); poll describe_stacks until terminal — it reaches CREATE_COMPLETE instead of ROLLBACK_COMPLETE, and describe_events records no VALIDATION_ERROR for the resource.

Cause 5: create_stack with a HostedZone and a RecordSet whose HostedZoneId is a syntactically valid but nonexistent id (e.g. Z0000000000000NOTREAL); wait for ROLLBACK_COMPLETE; read the CREATE_FAILED StackEvent for the record's logical id and inspect ResourceStatusReason — the HandlerErrorCode is NotFound and the '(Service: Route53, Status Code: 404, Request ID: ...) (SDK Attempt Count: ...)' clause is absent.
