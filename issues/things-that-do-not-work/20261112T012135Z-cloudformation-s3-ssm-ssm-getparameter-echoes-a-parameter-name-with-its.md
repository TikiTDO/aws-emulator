Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__s3__ssm.cfn-nested-stack-in-place-parameter-update-reaches-child-resource-then-not-found
Issue-Key: cloudformation__s3__ssm.cfn-nested-stack-in-place-parameter-update-reaches-child-resource-then-not-found|_assert_parameter#25|AssertionError|assert got == (name, "String", value, version), (
Issue-Commit: 69f62a7bc4bf8c4071f2682882e33fcb818ce341

# SSM GetParameter echoes a parameter Name with its leading '/' stripped, so a CFN-created hierarchical AWS::SSM::Parameter does not round-trip

> _Note: this issue was identified against the code base at commit `69f62a7bc4bf8c4071f2682882e33fcb818ce341`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetParameter

## Description
One net-new root cause, reached through the nested-stack parameter check (the parameter is created by CloudFormation from an AWS::SSM::Parameter resource and then read back with ssm:GetParameter).

NOTE ON RELATED FILINGS: the open issue 20261111T143427Z-ssm-8-net-new-causes-parameter-name-loses.md describes the same emulator behaviour as its 'Cause 1 of 8', but it is grounded in the ssm-only checks ssm.get-parameters-batch-orders-alphabetically-and-buckets-unresolvable-selectors and ssm.parameter-version-selector-and-label-move and does not carry this check's replay key, so this check is not covered by it. The replay suite did not mark this failure as owned or already filed. A fixer closing that issue should re-replay the check listed below as well.

Cause 1 of 1: GetParameter returns Parameter.Name without the leading '/' of a hierarchical parameter name.
AWS: Smithy: com.amazonaws.ssm#Parameter member Name targets PSParameterName (@length 1..2048), documented as "The name of the parameter". Parameter Store echoes the name as stored: a parameter whose name begins with '/' (the form required for any hierarchical name, and the form CloudFormation stores for an AWS::SSM::Parameter whose Name property starts with '/') is returned by GetParameter byte-identical to the name that created it — '/<path>/<leaf>' in, '/<path>/<leaf>' out. The other members of the same response (Type, Value, Version) are the stored values. Recorded behavior confirms the same name the caller sent in GetParameter.Name comes back in Parameter.Name.
Emulator: GetParameter succeeds (HTTP 200) with the right Type, Value and Version, but the echoed Parameter.Name has the leading '/' removed: a GetParameter for '/<name>' answers Name '<name>'. CloudFormation itself is consistent with AWS here — the stack output and DescribeStackResources PhysicalResourceId both carry the slash-prefixed name — so the divergence is in SSM's own projection of the parameter, not in the stack.
Why: a client that keys a cache or a map by the name it asked for sees a different key come back, and a client that feeds the echoed Name into a follow-up call (DeleteParameter, GetParameterHistory, AddTagsToResource, PutParameter) sends a name that is not the stored one — SSM requires the leading '/' for a hierarchical name, so the round trip breaks silently. Infrastructure code that compares an SSM read-back against the name it declared in its template reports a spurious difference.
Fix: the GetParameter response serializer / the Parameter projection in the SSM parameter state must echo the stored name verbatim, including its leading '/', instead of normalising or trimming it.
Traces: cloudformation__s3__ssm.cfn-nested-stack-in-place-parameter-update-reaches-child-resource-then-not-found

## Evidence
Check: cloudformation__s3__ssm.cfn-nested-stack-in-place-parameter-update-reaches-child-resource-then-not-found [mismatch].

The check creates a parent stack whose AWS::CloudFormation::Stack child declares an AWS::SSM::Parameter with Name = '/<generated-name>', Type = 'String', Value = 'hello'. The parent output, the child stack output and DescribeStackResources all report the slash-prefixed name (those assertions passed). It then calls ssm.get_parameter(Name='/<generated-name>').

Expected (Name, Type, Value, Version): ('/<generated-name>', 'String', 'hello', 1)
Actual   (Name, Type, Value, Version): ('<generated-name>', 'String', 'hello', 1)

Replay output:
  AssertionError: GetParameter /sim2real-cfnnest-param-78f28b55e40e: ('sim2real-cfnnest-param-78f28b55e40e', 'String', 'hello', 1), expected ('/sim2real-cfnnest-param-78f28b55e40e', 'String', 'hello', 1)

Only the Name member diverges; the status is 200 and no error is involved. 'sim2real-cfnnest-param-78f28b55e40e' is a per-run generated name, not part of the requirement: the requirement is that whatever name was stored (here a name beginning with '/') is echoed unchanged.

## Steps to Reproduce
1. ssm.put_parameter(Name='/sim2real-leading-slash', Type='String', Value='hello') — or create the same parameter from CloudFormation with an AWS::SSM::Parameter whose Name property is '/sim2real-leading-slash'.
2. ssm.get_parameter(Name='/sim2real-leading-slash').
3. Read response['Parameter']['Name'].
   Expect '/sim2real-leading-slash' (the stored name, leading '/' included), with Type 'String', Value 'hello', Version 1.
   Observe 'sim2real-leading-slash' — the leading '/' is stripped, while Type, Value and Version are correct.
4. Feed the echoed name straight back: ssm.get_parameter(Name=response['Parameter']['Name']) / ssm.delete_parameter(Name=...) and observe that the echoed name is not the name under which the parameter was stored.

Full scenario replay: run the behavioral check cloudformation__s3__ssm.cfn-nested-stack-in-place-parameter-update-reaches-child-resource-then-not-found.
