Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol__iam__stepfunctions.cloudcontrol-state-machine-definition-replace-patch-rejected
Issue-Key: sfn|DescribeStateMachine|InvalidArn
Issue-Commit: 83a487fe0db4ce5c414ce564b50d725b494461fa

# cloudcontrol CreateResource(AWS::StepFunctions::StateMachine) reports the StateMachineName as ProgressEvent.Identifier, so the identifier is unusable as the state machine ARN (third check, not covered by the open identifier issue's replay keys)

> _Note: this issue was identified against the code base at commit `83a487fe0db4ce5c414ce564b50d725b494461fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateResource

## Description
One net-new root cause in the cloudcontrol service.

Cause 1 of 1: CreateResource for a resource type whose primary identifier is a readOnly ARN answers ProgressEvent.Identifier with the NAME property carried in DesiredState instead of the type's primary identifier, so for AWS::StepFunctions::StateMachine the caller is handed the bare StateMachineName where the state machine ARN is required.
Failing check: cloudcontrol__iam__stepfunctions.cloudcontrol-state-machine-definition-replace-patch-rejected

AWS: Smithy (services/service-com-amazonaws-cloudcontrol-cloudapiservice/smithy-model/com-amazonaws-cloudcontrol-cloudapiservice.json) — CreateResource takes TypeName (required, pattern ^[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}$) and DesiredState (required) and answers a ProgressEvent. ProgressEvent$Identifier (length 1..1024, pattern ^.+$) is documented as "The primary identifier for the resource.", and it is the same value GetResource$Identifier / UpdateResource$Identifier / DeleteResource$Identifier take. WHICH property is the primary identifier is stated by the resource type's registry schema, not by Smithy: for AWS::StepFunctions::StateMachine the primaryIdentifier is /properties/Arn (readOnly), so the identifier is the state machine ARN of the form arn:<partition>:states:<region>:<account-id>:stateMachine:<StateMachineName> — fixed text "arn:", ":states:", ":stateMachine:"; partition, region, account id and StateMachineName are taken from the request and the emulator's state. That value is exactly what Step Functions' DescribeStateMachine/UpdateStateMachine/StartExecution accept as stateMachineArn, and what Cloud Control's GetResource must report as the Arn property.

Emulator: the ProgressEvent for the create reaches OperationStatus SUCCESS but carries Identifier = the bare StateMachineName from DesiredState (no ARN parts at all). The check then hands that identifier straight to Step Functions, and the emulator correctly refuses the malformed ARN — the recorded failure is botocore.errorfactory.InvalidArn: An error occurred (InvalidArn) when calling the DescribeStateMachine operation: Invalid Arn: 'Invalid ARN prefix: <state-machine-name>' (here the name was the per-run generated sim2real-xsvc-ccsm-36f2fa8725ae; the name itself is a per-run value, the requirement is that Identifier have the ARN shape above). So the InvalidArn refusal is not itself the defect: the defect is the identifier cloudcontrol reported. Because the check dies there, everything it was written to exercise downstream is never reached and stays unverified by this run: the ValidationException refusal of a `replace` patch of the write-only /Definition with the message "Invalid patch update: writeOnlyProperties [/Definition] can only be updated using 'add' operation", that the refused patch leaves the native definition and revisionId untouched, and that the same change sent as `add` lands and is visible in DescribeStateMachine.

Why: Cloud Control is the generic control plane a client uses WITHOUT knowing the owning service's API, and ProgressEvent.Identifier is the only handle it is given. A client that stores that value as the resource's primary identifier records something that cannot be re-resolved, and a client that passes it to Step Functions (stateMachineArn on DescribeStateMachine/StartExecution/UpdateStateMachine) or back into Cloud Control GetResource/UpdateResource/DeleteResource is refused — even though Cloud Control reported SUCCESS. Generic IaC and resource-management tooling that round-trips primaryIdentifier breaks on every ARN-identified type.

Fix: the cloudcontrol crate's identifier derivation (the CreateResource handler's identifier resolution and the shared state helper it calls) must resolve the type's primary identifier from the resource type's schema instead of preferring a "<ResourceSegment>Name" property of DesiredState, and for a type whose primary identifier is a readOnly ARN it must synthesize that ARN (partition/region/account from the request context, remaining parts from the desired state), report it as ProgressEvent.Identifier, and record it under the identifier property (Arn) in the stored resource model so GetResource/ListResources/UpdateResource/DeleteResource resolve the same value.

RELATION TO AN OPEN ISSUE: open issue 20261102T043935Z-cloudcontrol-createresource-reports-the-resource-name-as-progressevent.md describes this same identifier-derivation cause (its Cause 2 of 2 is AWS::StepFunctions::StateMachine), but its Issue-Trace-Ids/Issue-Key cover only cloudcontrol__iam__stepfunctions.cloudcontrol-iam-managed-policy-lifecycle-identifier-is-arn and cloudcontrol__iam__stepfunctions.cloudcontrol-state-machine-definition-object-round-trip. The check filed here is a THIRD check blocked by the same cause and carries no replay key there, so it is filed so it is not closed unrun; a fixer closing that issue should re-replay this check too, and once the identifier is an ARN this check will go on to exercise the write-only /Definition patch behavior listed above, which no run has verified yet.

Traces: cloudcontrol__iam__stepfunctions.cloudcontrol-state-machine-definition-replace-patch-rejected (verdict: error). Related code, as cited by the open issue above: services/service-com-amazonaws-cloudcontrol-cloudapiservice/src/ops/create_resource.rs (DesiredState parse + identifier derivation, and the identifier recorded into the ResourceEffect/ProgressEvent) and services/service-com-amazonaws-cloudcontrol-cloudapiservice/src/state.rs (identifier_property_candidates / derive_identifier).

## Evidence
Check cloudcontrol__iam__stepfunctions.cloudcontrol-state-machine-definition-replace-patch-rejected — verdict: error.

Flow: create an IAM role trusted by states.amazonaws.com; CreateResource(TypeName="AWS::StepFunctions::StateMachine", DesiredState={"StateMachineName": <name>, "RoleArn": <role-arn>, "Definition": {"StartAt": "P", "States": {"P": {"Type": "Pass", "Result": {"version": 1}, "End": True}}}}); poll the ProgressEvent to terminal; take identifier = ProgressEvent.Identifier; call sfn.DescribeStateMachine(stateMachineArn=identifier).

Expected (AWS): the create reaches OperationStatus "SUCCESS" with ProgressEvent.Identifier == arn:aws:states:<region>:<account-id>:stateMachine:<name> (the type's primaryIdentifier /properties/Arn), and DescribeStateMachine(stateMachineArn=<identifier>) answers with that name and the definition that was sent. The check then expects UpdateResource with PatchDocument [{"op": "replace", "path": "/Definition", "value": {...}}] to be refused with Error.Code "ValidationException" and Error.Message exactly "Invalid patch update: writeOnlyProperties [/Definition] can only be updated using 'add' operation" (all fixed text except the bracketed property pointer /Definition, which names the write-only property from the patch), the native definition and revisionId to be unchanged by the refused patch, and the same change sent as {"op": "add", "path": "/Definition", ...} to reach SUCCESS and be visible in DescribeStateMachine.

Actual (emulator):
  File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.errorfactory.InvalidArn: An error occurred (InvalidArn) when calling the DescribeStateMachine operation: Invalid Arn: 'Invalid ARN prefix: sim2real-xsvc-ccsm-36f2fa8725ae'

The value DescribeStateMachine rejected is precisely the ProgressEvent.Identifier cloudcontrol returned: it is the bare StateMachineName from DesiredState (sim2real-xsvc-ccsm-36f2fa8725ae is this run's generated name; the requirement is the ARN shape, not that name). The create itself was accepted, so only the identifier is wrong. The ValidationException/writeOnlyProperties assertions and the add-patch positive control are never reached.

## Steps to Reproduce
1. Boot the emulator and point botocore at it. cc = client("cloudcontrol"); sfn = client("stepfunctions"); iam = client("iam").
2. iam.create_role(RoleName="<unique-role-name>", AssumeRolePolicyDocument=json.dumps({"Version": "2012-10-17", "Statement": [{"Effect": "Allow", "Principal": {"Service": "states.amazonaws.com"}, "Action": "sts:AssumeRole"}]})) and take Role.Arn as role_arn.
3. name = "<unique-name>"; desired = json.dumps({"StateMachineName": name, "RoleArn": role_arn, "Definition": {"StartAt": "P", "States": {"P": {"Type": "Pass", "Result": {"version": 1}, "End": True}}}}).
4. ev = cc.create_resource(TypeName="AWS::StepFunctions::StateMachine", DesiredState=desired)["ProgressEvent"]; poll cc.get_resource_request_status(RequestToken=ev["RequestToken"])["ProgressEvent"] until OperationStatus is terminal (it reaches "SUCCESS").
5. Read identifier = ProgressEvent["Identifier"].
   Observed: the bare StateMachineName (no "arn:" prefix, no colons).
   Expected: arn:aws:states:<region>:<account-id>:stateMachine:<name>.
6. sfn.describe_state_machine(stateMachineArn=identifier).
   Observed: InvalidArn — "Invalid Arn: 'Invalid ARN prefix: <name>'" (correct refusal of the malformed value handed out in step 5).
   Expected: the state machine description, with definition equal to the Definition sent in step 3.
7. (Blocked by steps 5-6, and unverified by this run) cc.update_resource(TypeName="AWS::StepFunctions::StateMachine", Identifier=identifier, PatchDocument=json.dumps([{"op": "replace", "path": "/Definition", "value": {"StartAt": "Q", "States": {"Q": {"Type": "Pass", "Result": {"version": 2}, "End": True}}}}])) must raise ValidationException with Message "Invalid patch update: writeOnlyProperties [/Definition] can only be updated using 'add' operation", must leave DescribeStateMachine's definition and revisionId unchanged, and the same change with "op": "add" must reach SUCCESS and be visible in DescribeStateMachine.
