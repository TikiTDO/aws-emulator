Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__elbv2.xsvc-elbv2-3-alb-security-group-ingress-revoked-client-connections-blocked-by-timeout ec2__elbv2.xsvc-elbv2-3-target-security-group-ingress-revoked-health-times-out-until-reauthorized
Issue-Key: ec2__elbv2.xsvc-elbv2-3-alb-security-group-ingress-revoked-client-connections-blocked-by-timeout|_alb_ips#24|EgressRefused|return sorted({a[4][0] for a in socket.getaddrinfo(dns, 80, socket.AF_INET, socket.SOCK_STREAM)})
Issue-Key: ec2__elbv2.xsvc-elbv2-3-target-security-group-ingress-revoked-health-times-out-until-reauthorized|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Commit: deeb1944d1d0db8b9764b546c3d5c87bdcf95957

# ec2__elbv2: 2 net-new causes — the ALB announces a loopback host:port that is not the endpoint under test, and revoking a target's security-group ingress never affects target health

> _Note: this issue was identified against the code base at commit `deeb1944d1d0db8b9764b546c3d5c87bdcf95957`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLoadBalancers

## Description
Two net-new root causes in the EC2 + ELBv2 security-group/data-path surface.

=== Cause 1 of 2: the address the emulator announces for a load balancer is not the endpoint it is reached on, so every client connection to it is refused ===
AWS: CreateLoadBalancer/DescribeLoadBalancers return LoadBalancer.DNSName, and a client that connects to that name reaches the load balancer itself. The announced address is always an address the service actually serves; a client is expected to take the DNSName from the API response and connect to it.
Emulator: recorded behavior only — the host taken from what the emulator returned is a different address from the endpoint under test, and the connection is refused before any request is made: "simparity.egress.EgressRefused: egress refused: '127.0.0.1:<port>' is not the endpoint under test (127.0.0.1); an emulator run may reach only the emulator. If the host came from a URL the emulator returned, the emulator announces an address other than the endpoint it is reached on." The announced value carried a port distinct from the emulator's own, so the data plane is unreachable.
Why: a client that follows the documented contract — read DNSName, connect to it — cannot reach the load balancer at all, so no data-plane behaviour (here: that revoking the ALB's own security-group ingress makes client connections time out, and that re-authorizing restores them) can be observed. Security-group enforcement at the load balancer is therefore untestable.
Fix: the load-balancer address publication in the elbv2 service — DNSName (and any URL derived from it) must name the host:port the emulator serves, and the data plane must accept connections there. Related open issues record the sibling symptom of an unserved elb.amazonaws.com DNSName (e.g. 20261109T153103Z-elbv2-4-net-new-causes-the-published-dnsname.md, 20261110T220525Z-elbv2-5-net-new-data-plane-causes-dnsname.md); neither carries this check key.
Traces: ec2__elbv2.xsvc-elbv2-3-alb-security-group-ingress-revoked-client-connections-blocked-by-timeout.

=== Cause 2 of 2: revoking the target instance's security-group ingress never makes the target unhealthy, and re-authorizing never restores it ===
AWS: a target group health-checks its registered targets from the load balancer. Revoking the ingress rule that permits the load balancer's security group to reach the target's health-check port makes the probe time out, and after UnhealthyThresholdCount consecutive failures DescribeTargetHealth reports State="unhealthy" with Reason="Target.Timeout" (Smithy: elbv2 TargetHealthReasonEnum includes "Target.Timeout"); re-authorizing the ingress returns the target to "healthy" after HealthyThresholdCount successes.
Emulator: recorded behavior only — the check never reaches a verdict and is killed ("Check timed out after 60.0s"); no transition out of the initial state is ever produced, consistent with no health probe being performed and security-group rules having no effect on target health.
Why: security groups are the primary cause of real "target unhealthy" incidents. A client cannot reproduce or test that failure mode, and any emulator-based validation of security-group wiring between an ALB and its targets passes without exercising anything.
Fix: the target-group health-check scheduler in the elbv2 service must perform a real probe from the load balancer's network identity, and that probe must be subject to the EC2 security-group rules of the target and of the load balancer.
Traces: ec2__elbv2.xsvc-elbv2-3-target-security-group-ingress-revoked-health-times-out-until-reauthorized.

## Evidence
Cause 1 — Error: File "egress.py", line 338 ... simparity.egress.EgressRefused: egress refused: '127.0.0.1:37791' is not the endpoint under test (127.0.0.1); an emulator run may reach only the emulator. If the host came from a URL the emulator returned, the emulator announces an address other than the endpoint it is reached on. (The port in that line is a per-run value; the requirement is that the announced address equal the served endpoint.)

Cause 2 — Error: Check timed out after 60.0s, with no assertion reached. Expected: after RevokeSecurityGroupIngress on the target's health-check port, DescribeTargetHealth settles on State='unhealthy' / Reason='Target.Timeout', and after AuthorizeSecurityGroupIngress it returns to 'healthy'.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite so the emulator is serving.
2. Cause 1: create an internet-facing ALB with a security group, an HTTP:80 listener and a registered target; read DNSName from DescribeLoadBalancers and connect to it — the announced host:port is not the emulator's endpoint and the connection is refused. Then RevokeSecurityGroupIngress on the ALB's SG and observe that the client-connection behaviour cannot be exercised at all.
3. Cause 2: register an HTTP-serving instance into a target group behind an ALB listener, wait for 'healthy', RevokeSecurityGroupIngress for the ALB SG on the target's health-check port and poll DescribeTargetHealth — no transition to 'unhealthy'/'Target.Timeout' ever occurs; re-authorize and observe no transition back either.
