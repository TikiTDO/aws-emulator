Issue-Source: trace-sweep-divergence
Issue-Trace: elbv2err-create-load-balancer-input-rejections
Issue-Step: teardown#14
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elbv2err-create-load-balancer-input-rejections
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: ec2|DeleteSubnet|0->None|$
Issue-Key: ec2|DeleteSubnet|0->None|status
Issue-Commit: 710d5b8ba640b771c09c6bd4c34605ea9039ecfb

# EC2 never releases ELB-managed ENIs, so subnet teardown never converges and the final DeleteSubnet gets no response

> _Note: this issue was identified against the code base at commit `710d5b8ba640b771c09c6bd4c34605ea9039ecfb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteSubnet

## Description
One cause: the EC2 side of load-balancer teardown never reaches the terminal state, so the fixture's subnet-delete loop cannot finish and its last DeleteSubnet step never receives a response at all.

Cause 1 of 1: DeleteSubnet can never succeed because the requester-managed load-balancer ENIs are never released.
AWS: DeleteSubnet takes a single required SubnetId and, once nothing occupies the subnet, answers HTTP 200 with a DeleteSubnetResponse whose `return` element is `true` (Smithy: DeleteSubnet input SubnetId @required, output Unit, no modeled errors - EC2 surfaces faults as ec2-query Error elements, not modeled shapes). While an occupant remains, AWS answers HTTP 400 with Code `DependencyViolation` and a message of the fixed form "The subnet '<subnet-id>' has dependencies and cannot be deleted", whose only variable part is the subnet id taken from the request. Recorded behavior only for the timing: after the load balancer is deleted, AWS releases its requester-managed interfaces (DescribeNetworkInterfaces items with interfaceType `load_balancer`, requesterManaged `true`, requesterId `amazon-elb`, description "ELB app/<lb-name>/<lb-id>"), so a bounded drain poll on DescribeNetworkInterfaces converges to an empty networkInterfaceSet and the subsequent DeleteSubnet returns 200. The recording for this fixture shows exactly that: the ENI drain poll completes, then both subnet deletes answer 200.
Emulator: the ELB-managed interfaces survive the load balancer's deletion. The ENI drain poll at teardown#12 is still being served the two `load_balancer`/`amazon-elb` interfaces when it is capped at 180s (expected an empty result, got a networkInterfaceSet with both eni items), the DeleteSubnet at teardown#13 is therefore still refused with HTTP 400, and by teardown#14 the fixture's 960s budget is spent, so that step produces no HTTP status and no body at all (replay reports `status: expected <none>, got None` / `$: fixture budget 960s spent`).
Why: a client that deletes a load balancer and then tears down its subnets is wedged forever rather than for a bounded interval - the dependency never clears, so retry-until-deleted teardown loops (CloudFormation/Terraform-style) never terminate and every run leaks subnets, ENIs and the VPC above them. Because the terminal steps never execute, nothing after the first refused delete in a teardown is exercised either.
Fix: the load-balancer deletion path's ENI release in EC2 state (the requester-managed `load_balancer` interfaces must be removed when the load balancer is deleted) and DeleteSubnet's dependency bookkeeping, which must stop counting released interfaces as occupants.
Traces: elbv2err-create-load-balancer-input-rejections#teardown#14 (the step with no response). The two upstream steps of the same teardown are already owned by other open issues - the ENI projection/drain divergence at teardown#12 by 20261030T063325Z-ec2-instance-eni-and-volume-projections-omit-modeled.md and 20261103T123451Z-ec2-reads-resolve-resources-outside-the-request-s.md, and the 400 on teardown#13 by 20261103T123451Z-ec2-refuses-valid-requests-with-400-or-501.md (its Cause 4 covers DeleteSubnet refusing a dependency-free subnet). Neither of those issues carries the replay key for teardown#14, and none of them states the non-convergence of the ELB ENI release that makes the teardown unbounded, which is what this issue is about.

## Evidence
Replay verdict (expected -> actual), fixture elbv2err-create-load-balancer-input-rejections:
- teardown#14 DeleteSubnet: expected a graded response, got NONE - `status: expected , got None` and `$: fixture budget 960s spent`. The step never produced an HTTP status because the fixture's 960s budget was consumed upstream.
- Upstream, same teardown (already owned, cited only as the mechanism): teardown#12 DescribeNetworkInterfaces poll `capped at 180s` - expected the ENI set to have drained (expected None), got a networkInterfaceSet still carrying both ELB interfaces (eni-0000000000000009b / eni-0000000000000009c, interfaceType `load_balancer`, requesterId `amazon-elb`, description "ELB app/<lb-name>/<lb-id>"); teardown#13 DeleteSubnet status 200 -> 400.
The recording's expectation chain is: ENI set drains after the load balancer is deleted -> DeleteSubnet 200 -> DeleteSubnet 200. The emulator's chain is: ENI set never drains -> 400 -> no response.

## Steps to Reproduce
1. Replay the wire corpus simparity-wire-aws-v1 against the emulator.
2. Take fixture elbv2err-create-load-balancer-input-rejections and let it run through its teardown: create the ALB, delete it, then poll DescribeNetworkInterfaces for the subnets and delete both subnets.
3. Observe that the DescribeNetworkInterfaces poll keeps returning the ELB-owned interfaces (interfaceType `load_balancer`, requesterId `amazon-elb`) after the load balancer is gone, so it hits the 180s poll cap.
4. Observe teardown#13 DeleteSubnet answering HTTP 400 instead of 200, and teardown#14 DeleteSubnet never completing at all - the fixture's 960s budget is spent and the step reports no status.
