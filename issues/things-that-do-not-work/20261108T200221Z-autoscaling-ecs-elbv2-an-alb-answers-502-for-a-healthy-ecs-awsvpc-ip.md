Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ecs__elbv2.alb-listener-forwards-request-to-ecs-task-ip
Issue-Key: autoscaling__ecs__elbv2.alb-listener-forwards-request-to-ecs-task-ip|_poll_until#4|AssertionError|raise AssertionError(f"timed out waiting for {desc}; last observed: {last!r}")
Issue-Commit: 99cd58a9732d3abbd4ae376bbbd8ca320e363114

# autoscaling__ecs__elbv2: an ALB answers 502 for an ECS awsvpc ip target, because nothing in the emulator runs the container behind it (cause 1 of the two-cause report)

> _Split out of
> `issues/things-that-now-work/20261108T200221Z-autoscaling-ecs-elbv2-2-net-new-causes-an-b49c9b.md`,
> whose cause 2 (a health check pointed at a port nothing listens on reporting `healthy`) is
> fixed. This cause is NOT fixed; it keeps the original report's numbering, trace id and
> replay key so it stays filed and replayable. It was identified against the code base at
> commit `99cd58a9732d3abbd4ae376bbbd8ca320e363114`, so trace it back to the current code
> base._

> _WHAT THE CAUSE-2 FIX ESTABLISHED ABOUT THIS ONE, so a later fixer starts from here:_
>
> * _The 502 is the data plane's honest answer, not a wrong status: a `forward` to a
>   non-`lambda` target group picks a target that is a REGISTRATION — an address or an
>   instance id — with no server of this emulator's behind it, so the connection to it cannot
>   be made
>   (`services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/service/data_plane.rs`,
>   `TARGET_UNREACHABLE_STATUS`, reached from `fn forward`). Serving the recorded 200 means
>   the container image the ECS task runs (`public.ecr.aws/nginx/nginx:latest` in the
>   recording) actually executing. This emulator has no container runtime and makes no
>   outbound connection, so nothing can fetch or run that image; the engines the project
>   provides run caller-supplied JavaScript, Python and a native `.so` shipped in a caller's
>   package, none of which is a Linux container image. Synthesising a 200 with an invented
>   body, or swapping the 502 for another status, would be an outcome no engine produced:
>   this is a PLATFORM BLOCKER (a runtime the platform does not provide), and it is what has
>   to be lifted before the rest of this report can be served._
> * _Cause 2's fix changed the health side only, and it no longer contradicts the data plane
>   in the recorded shape: a target whose configured health-check port nothing listens on is
>   now reported `unhealthy`, and the data plane sends it no request. For THIS check the task
>   does serve its traffic port, so the target is `healthy` and the forward is still
>   answered 502._
> * _A SECOND, independent gap sits in front of the 502 and is what the replay now reports
>   first: `LoadBalancer$DNSName` is the AWS-form `<name>-<id>.<region>.elb.amazonaws.com`
>   the recorded control plane returns, which resolves to no address of this emulator, so the
>   check's `GET http://<lb-dns>/` is refused before any request arrives
>   (`PublishedUrlRejected: ... its host is not one of the emulator's own hosts; a customer of
>   the emulator needs http(s)://127.0.0.1:<port>/<resource path>`). The data plane does
>   RECOGNISE that name when a client points at the emulator with it as the `Host` header, and
>   it serves three further addressings beside it
>   (`.../src/service/data_plane.rs`, `claims_parts`). Changing the published value would
>   contradict the recorded `CreateLoadBalancer`/`DescribeLoadBalancers` response, so whatever
>   resolves this has to keep the AWS form in the control plane and make the address reachable;
>   it is a separate concern from the 502 and does not on its own make this check pass._

## Operation
DescribeTargetHealth

## Description
Cause 1 of 2: an ALB listener whose default action forwards to a target group holding a RUNNING ECS awsvpc task's private IP — a target the emulator itself reports as `healthy` — answers HTTP 502 instead of the container's response.
Failing check ids: autoscaling__ecs__elbv2.alb-listener-forwards-request-to-ecs-task-ip.
AWS: recorded behavior only — the ELBv2 Smithy model describes the control plane (CreateListener `forward` action with a TargetGroupArn, `TargetHealth.State`) and says nothing about the data plane. The recording shows that once DescribeTargetHealth reports the registered ip target `healthy`, a GET to the load balancer's DNSName on the listener port is proxied to that address and port and the ALB returns the target's own status line and body (HTTP 200 for an nginx task serving `/`). A 502 is what AWS returns only when the target cannot be connected to or returns a malformed response, and in that case the target is not simultaneously reported `healthy`.
Emulator: the target-health poll succeeds (`['healthy']`), but every GET to the ALB's DNS name over a 120s window answers 502. Replay: `AssertionError: timed out waiting for ALB to forward request to the task and return 200; last observed: 502`.
Why: a client that reads target health as proof the path is wired sees a fully healthy ALB that cannot serve a single request. Every end-to-end assertion that an ECS service is reachable through its load balancer fails while the control plane claims success, so a deployment gate, smoke test or canary built on 'healthy targets + HTTP 200' can never pass, and the emulator's own status signal contradicts its data plane.
Fix: the ELBv2 data-plane listener/forward path — the component that accepts a connection on the load balancer endpoint, selects a target from the forward action's target group and proxies to the ECS awsvpc task's address/port (and the ECS side that must make that address actually serveable). The 502 must be reserved for a target that genuinely cannot answer, and must not coexist with State='healthy'.
Traces: autoscaling__ecs__elbv2.alb-listener-forwards-request-to-ecs-task-ip.

## Evidence
Failing check: [mismatch] autoscaling__ecs__elbv2.alb-listener-forwards-request-to-ecs-task-ip
  File "check_autoscaling__ecs__elbv2_alb_listener_forwards_request_to_ecs_task_ip.py", line 32, in run
    status = _poll_until(...)
  AssertionError: timed out waiting for ALB to forward request to the task and return 200; last observed: 502
Expected (AWS): after DescribeTargetHealth reports the ECS task's private IP as State='healthy', GET http://<lb-dns>/ returns 200 with the container's body.
Actual (emulator): 502 on every attempt for 120s, while target health reads ['healthy']; and, where the client cannot resolve the published DNS name at all, the GET is refused before it is made (see the note above).

## Steps to Reproduce
Replay the behavioral suite and re-run exactly the check in trace_ids, or by hand:
1. In the shared-network VPC, create a security group that admits itself (so the ALB's health checks and traffic reach the tasks), and pick two subnets in different AZs.
2. elbv2 CreateLoadBalancer (Type='application') on those subnets with that security group; wait for State.Code='active'.
3. elbv2 CreateTargetGroup TargetType='ip', Protocol='HTTP', Port=80, VpcId=<vpc>, HealthCheckProtocol='HTTP', HealthCheckPath='/'; CreateListener Protocol='HTTP' Port=80 with DefaultActions=[{Type:'forward', TargetGroupArn:<tg>}].
4. ecs RegisterTaskDefinition (awsvpc, Fargate, an image that serves HTTP on container port 80) and CreateService with desiredCount=1 and loadBalancers=[{targetGroupArn:<tg>, containerName:<c>, containerPort:80}]; wait runningCount=1.
5. Poll elbv2 DescribeTargetHealth until the task IP is State='healthy' (this passes today).
6. GET http://<lb-dns>/. EXPECT 200 with the container's body. OBSERVED: 502 for the full 120s window.
