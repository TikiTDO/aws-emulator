Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm__cloudformation__ec2__elbv2.cfn-elbv2-listenercertificate-resource-lifecycle acm__cloudformation__ec2__elbv2.cfn-listenercertificate-nonexistent-certificate-rejected
Issue-Key: acm__cloudformation__ec2__elbv2.cfn-elbv2-listenercertificate-resource-lifecycle|run#18|AssertionError|assert certs == {cert_a: True, cert_b: False}, (
Issue-Key: acm__cloudformation__ec2__elbv2.cfn-listenercertificate-nonexistent-certificate-rejected|run#15|AssertionError|assert certs == {cert_a: True, cert_b: False}, f"control listener certificates {certs!r}"
Issue-Commit: d9b54befa158e14a4810d0b0face0e7b12d06851

# acm__cloudformation__ec2__elbv2: AWS::ElasticLoadBalancingV2::ListenerCertificate reaches CREATE_COMPLETE without attaching the certificate — DescribeListenerCertificates reports only the listener's default certificate

> _Note: this issue was identified against the code base at commit `d9b54befa158e14a4810d0b0face0e7b12d06851`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddListenerCertificates

## Description
One net-new root cause (Cause 1 of 1), grounded in two failing behavioral checks that both fail on the same assertion.

Cause 1 of 1: a CloudFormation stack containing an AWS::ElasticLoadBalancingV2::ListenerCertificate resource reaches CREATE_COMPLETE, but the extra certificate is never attached to the HTTPS listener: ELBv2 DescribeListenerCertificates returns only the listener's default certificate. Failing checks: acm__cloudformation__ec2__elbv2.cfn-elbv2-listenercertificate-resource-lifecycle and acm__cloudformation__ec2__elbv2.cfn-listenercertificate-nonexistent-certificate-rejected.

AWS: Smithy (services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/smithy-model/...json) AddListenerCertificates{ListenerArn required, Certificates required} -> Certificates, errors CertificateNotFoundException (awsQueryError CertificateNotFound, HTTP 400), ListenerNotFoundException, TooManyCertificatesException; DescribeListenerCertificates{ListenerArn required} -> Certificates (paginated by Marker/NextMarker), error ListenerNotFoundException. Recorded AWS behavior: CloudFormation's AWS::ElasticLoadBalancingV2::ListenerCertificate resource provisions by calling AddListenerCertificates for each Certificates[].CertificateArn on the referenced listener; after the stack reaches CREATE_COMPLETE, DescribeListenerCertificates on that listener reports exactly two entries — the listener's own default certificate with IsDefault true and the ListenerCertificate-attached ARN with IsDefault false. Dropping the resource in a stack update removes exactly that one certificate, leaving the default; and an update pointing the resource at a well-formed ARN ACM never issued fails the resource with ELBv2's CertificateNotFound (message template "Certificate '<arn>' not found (Service: AmazonElasticLoadBalancing; Status Code: 400; Error Code: CertificateNotFound; Request ID: <id>; Proxy: null)" — the fixed text is everything but the ARN from the request and the per-run request id) and rolls the stack back to UPDATE_ROLLBACK_COMPLETE with the real certificate still attached.

Emulator: the stack reaches CREATE_COMPLETE and the listener itself reads back correctly (Port 443, Protocol HTTPS, SslPolicy ELBSecurityPolicy-2016-08), but DescribeListenerCertificates on it returns a single entry — the default certificate with IsDefault true — so the ListenerCertificate resource provisioned nothing observable. Replay: "listener certificates {'arn:aws:acm:<region>:<account>:certificate/<a>': True}: expected '<a>' default and '<b>' attached" and "control listener certificates {'arn:aws:acm:<region>:<account>:certificate/<a>': True}" (the ARNs are per-run values). Hypothesis: the CloudFormation resource handler for AWS::ElasticLoadBalancingV2::ListenerCertificate is a no-op that reports success without calling into ELBv2 state, or the ELBv2 listener state keeps only the default certificate so non-default attachments are dropped on write or omitted from the DescribeListenerCertificates projection.

Why: a client gets a stack CloudFormation says is complete while the listener is missing a certificate it declared, so TLS for the second domain silently falls back to the default certificate. Because provisioning is a no-op, the removal path (an update that drops the resource) and the negative path (an update naming a never-issued ARN, which must fail the resource with CertificateNotFound and roll back) are unreachable too — no part of the resource's lifecycle can be trusted.

Fix: the CloudFormation resource handler for AWS::ElasticLoadBalancingV2::ListenerCertificate (create/update/delete) must drive ELBv2 AddListenerCertificates / RemoveListenerCertificates against the referenced listener and surface ELBv2's CertificateNotFoundException verbatim as the resource's failure reason; and the ELBv2 listener state plus the DescribeListenerCertificates projection must record and return non-default certificates with IsDefault false alongside the default one.

Related open filing that does NOT cover these replays: 20261109T132410Z-acm-cloudformation-ec2-elbv2-a-load-balancer-created.md (a stack's load balancer unknown to ELBv2 — these replays now read the listener back successfully and fail one step later, and that issue carries neither check id).

Traces: acm__cloudformation__ec2__elbv2.cfn-elbv2-listenercertificate-resource-lifecycle, acm__cloudformation__ec2__elbv2.cfn-listenercertificate-nonexistent-certificate-rejected.

## Evidence
Replay of suite simparity-behavioral-aws-v1: acm__cloudformation__ec2__elbv2 0/2 (0%), 2 failed.

[mismatch] acm__cloudformation__ec2__elbv2.cfn-elbv2-listenercertificate-resource-lifecycle:
  File "check_...cfn_elbv2_listenercertificate_resource_lifecycle.py", line 283, in run -> assert certs == {cert_a: True, cert_b: False}
  AssertionError: listener certificates {'arn:aws:acm:us-west-2:000000000000:certificate/a6ce3f05-6b07-4783-8a41-1da12a103daa': True}: expected 'arn:aws:acm:us-west-2:000000000000:certificate/a6ce3f05-6b07-4783-8a41-1da12a103daa' default and 'arn:aws:acm:us-west-2:000000000000:certificate/6e087dcb-01e7-459b-be17-f347966f1657' attached

[mismatch] acm__cloudformation__ec2__elbv2.cfn-listenercertificate-nonexistent-certificate-rejected:
  File "check_...cfn_listenercertificate_nonexistent_certificate_rejected.py", line 274, in run -> assert certs == {cert_a: True, cert_b: False}
  AssertionError: control listener certificates {'arn:aws:acm:us-west-2:000000000000:certificate/2720e2ea-9f3d-4ef5-a2e1-a7c77b3f9eb7': True}

Both checks import two ACM leaves (cert_a, cert_b) and create a stack with VPC + two subnets + security group + internal ALB + an HTTPS Listener (Port 443, Protocol HTTPS, SslPolicy 'ELBSecurityPolicy-2016-08', Certificates=[{CertificateArn: Ref CertAArn}]) plus:
  "ListenerCert": {"Type": "AWS::ElasticLoadBalancingV2::ListenerCertificate", "Properties": {"ListenerArn": {"Ref": "Listener"}, "Certificates": [{"CertificateArn": cert_b}]}}
After CREATE_COMPLETE they resolve the listener from the stack's AlbRef/ListenerRef outputs via DescribeListeners and read {CertificateArn: IsDefault} over every page of DescribeListenerCertificates. Expected (AWS): {cert_a: True, cert_b: False}. Actual (emulator): {cert_a: True} — cert_b absent.

Unreached as a consequence: (a) an update dropping the ListenerCertificate leaving exactly {cert_a: True}; (b) after the stack delete, DescribeListenerCertificates on the gone listener refused with ListenerNotFound "One or more listeners not found"; (c) an update swapping the resource's ARN for a well-formed ARN ACM never issued, which must fail the ListenerCert resource with reason "Certificate '<arn>' not found (Service: AmazonElasticLoadBalancing; Status Code: 400; Error Code: CertificateNotFound; Request ID: <id>; Proxy: null)" and settle UPDATE_ROLLBACK_COMPLETE with {cert_a: True, cert_b: False} still live.

Per-run values (certificate ARNs/uuids, stack and resource names, account and region, request ids) are not requirements; the requirements are the attachment itself, the IsDefault flags, and the CertificateNotFound code/message template.

Smithy consulted: elbv2 AddListenerCertificates, RemoveListenerCertificates, DescribeListenerCertificates, Certificate, CertificateNotFoundException (awsQueryError CertificateNotFound), ListenerNotFoundException.

## Steps to Reproduce
1. Clone the repo, build and boot the emulator, then replay suite simparity-behavioral-aws-v1; both acm__cloudformation__ec2__elbv2 checks reproduce.
2. acm.import_certificate two self-signed leaves -> cert_a, cert_b.
3. cloudformation.create_stack with a template holding a VPC, two subnets, a security group, an internal application load balancer, an HTTPS listener (Port 443, SslPolicy 'ELBSecurityPolicy-2016-08', Certificates=[{CertificateArn: cert_a}]) and an AWS::ElasticLoadBalancingV2::ListenerCertificate whose Properties are {ListenerArn: {Ref: Listener}, Certificates: [{CertificateArn: cert_b}]}; wait for CREATE_COMPLETE.
4. elbv2.describe_listeners(LoadBalancerArn=<stack AlbRef>) to get the listener ARN, then page elbv2.describe_listener_certificates(ListenerArn=<that>). Expected (AWS): {cert_a: IsDefault true, cert_b: IsDefault false}. Observed: only {cert_a: true}.
5. cloudformation.update_stack with the same template minus the ListenerCertificate resource; expected: exactly {cert_a: true} remains.
6. cloudformation.update_stack with the ListenerCertificate pointing at <cert_b prefix>/00000000-0000-0000-0000-000000000000 (a well-formed ARN ACM never issued); expected: UPDATE_ROLLBACK_COMPLETE with the ListenerCert resource's UPDATE_FAILED reason "Certificate '<arn>' not found (Service: AmazonElasticLoadBalancing; Status Code: 400; Error Code: CertificateNotFound; Request ID: <id>; Proxy: null)" and {cert_a: true, cert_b: false} still attached.
7. Re-replay both check ids in trace_ids to confirm the fix.
