Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.delete-trust-store-in-use-by-listener-fails
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elbv2.delete-trust-store-in-use-by-listener-fails|_wait_association#10|Timeout|time.sleep(interval_s)
Issue-Commit: 4839448a699fa31c23507e6b5ab2cdd919186720

# elbv2: DescribeTrustStoreAssociations never reports the load balancer ARN, so the mTLS in-use trust-store check never reaches its DeleteTrustStore assertion (60 s timeout)

> _Note: this issue was identified against the code base at commit `4839448a699fa31c23507e6b5ab2cdd919186720`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTrustStoreAssociations

## Description
One net-new-to-the-report root cause (the open issue 20261111T160347Z-elbv2-trust-store-plane-4-net-new-causes.md describes the same defect as its Cause 1 and lists this check id in its Issue-Trace-Ids, but its stored replay key for this check no longer matches, so the suite did not mark the check OWNED; a fixer closing that issue's Cause 1 should close this one with it rather than treating it as a separate defect).

Cause 1 of 1: DescribeTrustStoreAssociations reports the ASSOCIATED LISTENER's ARN where AWS reports the ARN of the LOAD BALANCER that owns the listener.
Failing check: elbv2.delete-trust-store-in-use-by-listener-fails.

AWS: Smithy (com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10) DescribeTrustStoreAssociations takes required TrustStoreArn plus Marker / PageSize (@range 1..400) and answers DescribeTrustStoreAssociationsOutput{TrustStoreAssociations: list<TrustStoreAssociation>, NextMarker}; TrustStoreAssociation carries only ResourceArn, and its sole modeled error is TrustStoreNotFoundException (client, awsQueryError code TrustStoreNotFound, httpError 400). The model does not say WHICH resource the ARN names, so for that detail this is recorded behavior only: when an HTTPS listener enables MutualAuthentication {Mode: verify, TrustStoreArn: <ts>}, AWS reports the association against the load balancer that owns the listener — ResourceArn's resource segment is loadbalancer/app/<lb-name>/<lb-id>, never listener/app/<lb-name>/<lb-id>/<listener-id>. The check encodes exactly that: after CreateListener it polls DescribeTrustStoreAssociations(TrustStoreArn=<ts>) for the ARN returned by CreateLoadBalancer, and only then asserts the DeleteTrustStore refusal (Smithy DeleteTrustStore: required TrustStoreArn, modeled TrustStoreInUseException -> code TrustStoreInUse, HTTP 400, message template fixed text "Trust store with ARN '" + "' is currently associated" with the request's TrustStoreArn as its only variable part) and the post-delete DescribeTrustStores refusal TrustStoreNotFound with message "Trust store ARN '<TrustStoreArn>' not found".

Emulator: the association set is derived from the listeners that carry MutualAuthenticationAttributes$TrustStoreArn and the LISTENER ARN is emitted as TrustStoreAssociation.ResourceArn, so the load balancer ARN never appears. The check's bounded poll (_wait_association, "resource <arn> never appeared in trust store associations") therefore spins until the harness kills the check: the only replay output is "Check timed out after 60.0s". Because the poll sits before the refusal assertions, the emulator's DeleteTrustStore / DescribeTrustStores behaviour for this flow is never exercised at all by this check. The ARNs involved are per-run generated values; the requirement is the ARN's resource-type segment (loadbalancer/app/... rather than listener/app/...).

Why: a client calls DescribeTrustStoreAssociations to answer "what still uses this trust store" before deleting or re-sharing it, and to join a trust store back to the load balancers it protects. Against the emulator every such join fails — a caller matching or filtering on load-balancer ARNs sees an EMPTY association set for a trust store that is in use, and a caller that forwards the returned ARN to DescribeLoadBalancers gets not-found. Deletion-safety logic and cross-account trust-store sharing built on this read are unusable.

Fix: the DescribeTrustStoreAssociations handler and the state derivation it reads — services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/ops/describe_trust_store_associations.rs together with crate::state's trust_store_associations derivation, which must map a listener that enables mutual TLS against the trust store to its OWNING load balancer's ARN before paginating. DeleteSharedTrustStoreAssociation and DeleteTrustStore read the same derivation, so the correction must leave their in-use decision unchanged.

Traces: elbv2.delete-trust-store-in-use-by-listener-fails (suite simparity-behavioral-aws-v1, group elbv2).

## Evidence
Failing check elbv2.delete-trust-store-in-use-by-listener-fails [mismatch]; replay detail: "Error: Check timed out after 60.0s" (no assertion reached).

Check body (abridged): creates a VPC + two subnets + SG, an ACTIVE trust store from a CA bundle in S3, an internal ALB, a target group and an imported ACM certificate, then CreateListener(Protocol=HTTPS, Port=443, MutualAuthentication={Mode: verify, TrustStoreArn: ts_arn}), then _wait_association(ctx, ts_arn, lb_arn) which polls DescribeTrustStoreAssociations(TrustStoreArn=ts_arn) every 5 s for an entry whose ResourceArn == the CreateLoadBalancer ARN, failing with "resource <arn> never appeared in trust store associations".

Expected (AWS): TrustStoreAssociations contains {"ResourceArn": "arn:<partition>:elasticloadbalancing:<region>:<account>:loadbalancer/app/<lb-name>/<lb-id>"} shortly after the mTLS listener is created, so the check proceeds to assert DeleteTrustStore -> HTTP 400 / TrustStoreInUse / "Trust store with ARN '<ts-arn>' is currently associated" (fixed text; <ts-arn> echoed from the request), that DescribeTrustStores still returns the store, and after ModifyListener{MutualAuthentication:{Mode: off}} that DeleteTrustStore succeeds and DescribeTrustStores then answers TrustStoreNotFound / "Trust store ARN '<ts-arn>' not found".

Actual (emulator): the association listing only ever contains the LISTENER ARN (resource segment listener/app/<lb-name>/<lb-id>/<listener-id>), as directly asserted by the sibling checks of the same defect — elbv2.deploy-trust-store-deletion-rejected-while-in-use: "expected association with 'arn:...:loadbalancer/app/<lb-name>/<lb-id>', got {'arn:...:listener/app/<lb-name>/<lb-id>/<listener-id>'}" — so the load balancer ARN never appears and this check burns its whole 60 s budget without reaching any refusal assertion.

Smithy basis: DescribeTrustStoreAssociations input {TrustStoreArn required, Marker, PageSize @range 1..400}, output {TrustStoreAssociations, NextMarker}, errors [TrustStoreNotFoundException -> TrustStoreNotFound, 400]; DeleteTrustStore input {TrustStoreArn required}, errors [TrustStoreInUseException -> TrustStoreInUse, 400; TrustStoreNotFoundException -> TrustStoreNotFound, 400].

Deduplication: 20261111T160347Z-elbv2-trust-store-plane-4-net-new-causes.md (open) documents this as Cause 1 and already names this check id; it was not reported as OWNED for this replay because its stored key for the check no longer matches the observed failure signature.

## Steps to Reproduce
1. Clone the repository and boot the emulator built from it.
2. Replay suite simparity-behavioral-aws-v1 and look at check elbv2.delete-trust-store-in-use-by-listener-fails in the elbv2 group; it ends as "Check timed out after 60.0s".
3. Reproduce by hand: create a VPC with two subnets and a security group; upload a self-signed CA bundle to S3 and CreateTrustStore from it, waiting until DescribeTrustStores reports Status ACTIVE; CreateLoadBalancer (Type=application, Scheme=internal) and note the returned LoadBalancerArn; CreateTargetGroup and import a certificate into ACM.
4. CreateListener on that load balancer with Protocol=HTTPS, Port=443, a default forward action, and MutualAuthentication={Mode: "verify", TrustStoreArn: <ts-arn>}.
5. Call DescribeTrustStoreAssociations(TrustStoreArn=<ts-arn>). Observe TrustStoreAssociations[0].ResourceArn is the LISTENER ARN (listener/app/...); AWS reports the ARN of the load balancer that owns the listener (loadbalancer/app/...). A client polling for the load balancer ARN never terminates.
6. After the fix, re-replay elbv2.delete-trust-store-in-use-by-listener-fails: the association poll must complete and the check must then verify the DeleteTrustStore TrustStoreInUse refusal, the surviving trust store, and the successful delete after MutualAuthentication is turned off.
