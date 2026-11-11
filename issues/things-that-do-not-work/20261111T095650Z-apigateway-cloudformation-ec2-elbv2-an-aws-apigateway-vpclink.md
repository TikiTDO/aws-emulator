Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation__ec2__elbv2.deploy-apigateway-vpclink-invalid-target-arn-rejected-and-rolled-back
Issue-Key: apigateway__cloudformation__ec2__elbv2.deploy-apigateway-vpclink-invalid-target-arn-rejected-and-rolled-back|run#14|AssertionError|assert status == "ROLLBACK_COMPLETE", f"rejected stack ended {status!r}"
Issue-Commit: 19132400f00c6250ea4e57ba1f7117d500668e37

# apigateway__cloudformation__ec2__elbv2: an AWS::ApiGateway::VpcLink whose TargetArns names a nonexistent NLB reaches CREATE_COMPLETE instead of failing stabilization and rolling the stack back

> _Note: this issue was identified against the code base at commit `19132400f00c6250ea4e57ba1f7117d500668e37`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVpcLink

## Description
Cause 1 of 1: a CloudFormation AWS::ApiGateway::VpcLink whose TargetArns names a load balancer that does not exist is provisioned successfully instead of failing.

AWS: Smithy (apigateway) CreateVpcLink takes targetArns (list of NLB ARNs) and returns a VpcLink whose status is the asynchronous enum PENDING -> AVAILABLE | FAILED | DELETING, with statusMessage carrying the reason; no synchronous error is modeled for an unresolvable target, so the refusal is asynchronous. Recorded behavior: the create is accepted, provisioning then fails because the named NLB does not exist, the link settles FAILED, and CloudFormation's AWS::ApiGateway::VpcLink handler never stabilizes it. The stack emits a CREATE_FAILED event for the VpcLink resource whose ResourceStatusReason is `Resource handler returned message: "Failed to stabilize Vpc Link with id <vpcLinkId> Status Message NLB ARN not found." (RequestToken: <token>, HandlerErrorCode: GeneralServiceException)` — fixed text: `Failed to stabilize Vpc Link with id `, ` Status Message NLB ARN not found.`, HandlerErrorCode `GeneralServiceException`; the variable parts are the physical link id the create returned and the per-run request token. The stack then ends ROLLBACK_COMPLETE, GetVpcLinks lists no link under that name, and GetVpcLink on the id answers NotFoundException with message `Invalid Vpc Link identifier specified`.

Emulator: the stack whose only resource is that VpcLink ends CREATE_COMPLETE (replay assertion: `rejected stack ended 'CREATE_COMPLETE'`). The link is created and settles as if its target existed; nothing validates targetArns against ELBv2 state and the handler reports success.

Why: a client that relies on CloudFormation to reject a mis-wired VPC Link gets a stack it believes is good, holding a VPC Link that can never route. The whole negative-path contract (CREATE_FAILED reason, ROLLBACK_COMPLETE, absence of the link afterwards) is unobservable, and IaC guardrails built on it silently pass.

Fix: apigateway CreateVpcLink's asynchronous provisioning (it must resolve every targetArns entry against ELBv2 load-balancer state and settle the link FAILED with statusMessage `NLB ARN not found.` when one is unresolvable) plus the AWS::ApiGateway::VpcLink resource provider's stabilization step, which must surface that as the CREATE_FAILED handler reason above and roll the stack back.

Related open issues that do NOT cover this check (neither carries its replay key, and both are filed against the plain apigateway surface rather than the CloudFormation path): 20261103T135737Z-apigateway-createvpclink-s-asynchronous-provisioning-settles-a-link.md and 20261106T040418Z-apigateway-2-net-new-causes-createdomainname-accepts-an.md.

Traces: apigateway__cloudformation__ec2__elbv2.deploy-apigateway-vpclink-invalid-target-arn-rejected-and-rolled-back.

## Evidence
Check apigateway__cloudformation__ec2__elbv2.deploy-apigateway-vpclink-invalid-target-arn-rejected-and-rolled-back [mismatch]:
  AssertionError: rejected stack ended 'CREATE_COMPLETE'
  (assert status == "ROLLBACK_COMPLETE", f"rejected stack ended {status!r}")

Expected (AWS): stack status ROLLBACK_COMPLETE; exactly one VpcLink CREATE_FAILED event with handler reason ("Failed to stabilize Vpc Link with id <id> Status Message NLB ARN not found.", "GeneralServiceException"); afterwards GetVpcLinks lists no link with that name and GetVpcLink answers NotFoundException "Invalid Vpc Link identifier specified".
Actual (emulator): stack status CREATE_COMPLETE, no CREATE_FAILED event, link provisioned.
The control half of the same check (a VpcLink targeting a real internal NLB) is expected to reach CREATE_COMPLETE and AVAILABLE — only the invalid-target half diverges.

## Steps to Reproduce
1. Create a CloudFormation stack whose single resource is AWS::ApiGateway::VpcLink with Name=<unique>, TargetArns=["arn:aws:elasticloadbalancing:<region>:<account>:loadbalancer/net/<name-that-does-not-exist>/0123456789abcdef"].
2. Poll DescribeStacks until a terminal status.
3. Observe CREATE_COMPLETE; AWS ends ROLLBACK_COMPLETE with the VpcLink CREATE_FAILED/GeneralServiceException handler reason above.
4. Re-replay apigateway__cloudformation__ec2__elbv2.deploy-apigateway-vpclink-invalid-target-arn-rejected-and-rolled-back to confirm a fix.
