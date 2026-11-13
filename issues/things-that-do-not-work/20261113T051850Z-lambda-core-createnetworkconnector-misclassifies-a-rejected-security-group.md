Issue-Source: trace-sweep-divergence
Issue-Trace: lambdacore-create-validation-rejections
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambdacore-create-validation-rejections
Issue-Shape-Id: com.amazonaws.lambdacore#LambdaCoreApiService
Issue-Key: lambda-core|CreateNetworkConnector|4xx->4xx|$.message
Issue-Commit: 65a54ca45d4af5e17b89290ffb091f639a3756dd

# lambda-core CreateNetworkConnector misclassifies a rejected security-group id as InvalidGroup.NotFound instead of surfacing EC2's InvalidGroupId.Malformed

> _Note: this issue was identified against the code base at commit `65a54ca45d4af5e17b89290ffb091f639a3756dd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateNetworkConnector

## Description
One net-new root cause: when CreateNetworkConnector cannot resolve a security group named in its VpcEgressConfiguration, the emulator always reports the EC2 failure as a lookup miss, so a security-group id that EC2 itself rejects as malformed is reported with the wrong wrapped EC2 error code and the wrong message text. The HTTP status and the Lambda-level error class are correct; only the wrapped EC2 code/message diverge.

Cause 1 of 1 - the wrapped DescribeSecurityGroups failure is hard-wired to InvalidGroup.NotFound.
AWS: Smithy CreateNetworkConnector (POST /2026-04-04/network-connectors, success code 202) takes Configuration -> NetworkConnectorConfiguration, a union whose VpcEgressConfiguration carries SecurityGroupIds -> NetworkConnectorSecurityGroupIds (list, length 0..16) of NetworkConnectorSecurityGroupId (string, length 0..1024, pattern ^sg-[0-9a-zA-Z]*$); the modeled client error for a bad parameter value is InvalidParameterValueException (errorKind client, httpError 400, members Type and message), documented as 'One of the parameters in the request is not valid. Check the error message for details about which parameter failed validation.' The model's own pattern is deliberately loose, so the id is validated downstream by EC2 and Lambda surfaces EC2's verdict verbatim. Recorded behavior: the connector creation calls EC2 DescribeSecurityGroups to resolve the referenced groups and, when EC2 refuses, answers 400 InvalidParameterValueException with a message built from a fixed wrapper around EC2's own code and message: the fixed text is 'Error occurred while DescribeSecurityGroups. EC2 Error Code: <ec2-error-code>. EC2 Error Message: <ec2-error-message>', where <ec2-error-code> and <ec2-error-message> are EC2's verbatim values for that failure class. For a security-group id EC2 refuses as syntactically invalid, the recorded values are code InvalidGroupId.Malformed and message Invalid id: "<the security-group id taken verbatim from the request>" (the double quotes around the id are part of EC2's fixed text). So AWS distinguishes at least two failure classes behind the same wrapper: the id is rejected by EC2's id parser (InvalidGroupId.Malformed / Invalid id: "...") versus the id parses but names no group in the account (InvalidGroup.NotFound / The security group '<id>' does not exist).
Emulator: answers 400 InvalidParameterValueException with the correct wrapper text but the not-found variant of the inner pair for a request whose security-group id AWS has EC2 reject as malformed: it returned 'Error occurred while DescribeSecurityGroups. EC2 Error Code: InvalidGroup.NotFound. EC2 Error Message: The security group '<id>' does not exist' where AWS returned 'Error occurred while DescribeSecurityGroups. EC2 Error Code: InvalidGroupId.Malformed. EC2 Error Message: Invalid id: "<id>"'. Hypothesis: the emulator has exactly one failure branch for this validation - a membership test against stored security groups - and no branch that reproduces EC2's rejection of the id itself, so every unresolvable security-group id collapses onto the not-found pair.
Why: the inner EC2 code is the only machine-readable discriminator a client has here, because both classes arrive as the same 400 InvalidParameterValueException. Tooling that parses it to decide what to do next - retry after creating or waiting for the group, versus fail fast and ask the user to fix a typo'd or truncated id - takes the wrong branch: a malformed id is reported as a transient 'not yet there' condition, so provisioners retry or poll forever instead of surfacing a permanent input error, and the user-facing message names a nonexistent group rather than an invalid one.
Fix: in CreateNetworkConnector's validation of Configuration.VpcEgressConfiguration.SecurityGroupIds (the step that stands in for the DescribeSecurityGroups call), classify the failure before the existence lookup: an id EC2's parser refuses must produce the InvalidGroupId.Malformed / Invalid id: "<id>" pair, and only a well-formed id that resolves to no stored group may produce InvalidGroup.NotFound / The security group '<id>' does not exist. The shared wrapper text and the 400 InvalidParameterValueException envelope are already correct and must not change.
Traces: lambdacore-create-validation-rejections#task#4.

## Evidence
Replay of corpus simparity-wire-aws-v1 (emulator_up=true, bootstrap_signal=false, score_pct=80.7).

Cause 1 - fixture lambdacore-create-validation-rejections, step task#4, CreateNetworkConnector: status 400 expected and 400 actual (match); $.message value_mismatch -
  expected: 'Error occurred while DescribeSecurityGroups. EC2 Error Code: InvalidGroupId.Malformed. EC2 Error Message: Invalid id: "sg-0123456789abcdef0"'
  actual:   'Error occurred while DescribeSecurityGroups. EC2 Error Code: InvalidGroup.NotFound. EC2 Error Message: The security group 'sg-0123456789abcdef0' does not exist'
divergence_key service=lambda-core|CreateNetworkConnector|status=400->400|keys=$.message; affected_count=1; stub_path empty; owned_by empty (no open issue carries this replay key).

Message anatomy: fixed text = 'Error occurred while DescribeSecurityGroups. EC2 Error Code: ', '. EC2 Error Message: ', and EC2's own fixed texts 'Invalid id: "<id>"' / 'The security group '<id>' does not exist'. Variable parts = the EC2 error code for the failure class and the security-group id, which is echoed verbatim from the request's Configuration.VpcEgressConfiguration.SecurityGroupIds; the particular id in this recording is request data, not part of the requirement.

Model references (services/service-com-amazonaws-lambdacore-lambdacoreapiservice/smithy-model/com-amazonaws-lambdacore-lambdacoreapiservice.json): CreateNetworkConnector POST /2026-04-04/network-connectors code 202, errors InvalidParameterValueException(400), NetworkConnectorLimitExceededException(400), ResourceConflictException(409), TooManyRequestsException(429), ServiceException(500); NetworkConnectorConfiguration union member VpcEgressConfiguration -> NetworkConnectorVpcEgressConfiguration with SubnetIds/SecurityGroupIds/NetworkProtocol/AssociatedComputeResourceTypes; NetworkConnectorSecurityGroupId pattern ^sg-[0-9a-zA-Z]*$.

Related but NOT covering this: open issue 20261023T022015Z-lambda-core-admits-requests-aws-refuses-and-misclassifies.md covers mis-classified refusals on other connector operations (it owns the UpdateNetworkConnector 400->404 cluster in this same replay) and does not carry this fixture/step key or this operation.

## Steps to Reproduce
1. git_clone, then replay_wire_corpus() to replay simparity-wire-aws-v1 against the emulator built from the checkout.
2. Fixture lambdacore-create-validation-rejections, step task#4: POST /2026-04-04/network-connectors with Configuration.VpcEgressConfiguration.SecurityGroupIds containing a security-group id that EC2's id parser refuses.
3. Observe the 400 InvalidParameterValueException body's $.message: the emulator wraps EC2 code InvalidGroup.NotFound with 'The security group '<id>' does not exist', where AWS wraps InvalidGroupId.Malformed with 'Invalid id: "<id>"'.
4. For contrast, request a connector with a well-formed security-group id that names no existing group: that case must keep the InvalidGroup.NotFound pair, so the two classes are distinguishable.
