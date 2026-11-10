Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2__s3.log-edge-producers-alb-https-access-log-records-negotiated-tls-and-sni
Issue-Key: elbv2__s3.log-edge-producers-alb-https-access-log-records-negotiated-tls-and-sni|_poll_until#29|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: 41bd6c476a5fe40a474aadc7e47c434277d677d7

# elbv2__s3: an ALB's HTTPS listener refuses every connection at the load balancer's DNS name, so no TLS is terminated and no access-log record is ever written

> _Note: this issue was identified against the code base at commit `41bd6c476a5fe40a474aadc7e47c434277d677d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateListener

## Description
Cause 1 of 1: the HTTPS listener data plane does not exist — the load balancer's announced address refuses TCP connections on 443 for the whole provisioning window.
AWS: recorded behavior. After CreateLoadBalancer (internet-facing, two subnets, a security group allowing 443) and CreateListener with Protocol=HTTPS, Port=443, an SslPolicy and `Certificates=[{CertificateArn: <imported ACM cert>}]` and a `fixed-response` default action, the load balancer's `DNSName` accepts TLS on port 443 within the provisioning window and answers that fixed response — its declared status code and message body — for any path. The listener presents the certificate that was imported (the client sees exactly those DER bytes), negotiates the protocol/cipher the SslPolicy permits, and honours SNI independently of the Host header. Each such request then appears in the delivered access log with the negotiated protocol and cipher, the SNI sent, the chosen certificate ARN, the request line `GET https://<dns>:443<path> HTTP/1.1`, the ELB status code, and `-` in every target field because a fixed-response action involves no target.
Emulator: every connection attempt to the load balancer's address on port 443 is refused for the full 420 s window — `ConnectionRefusedError(111, 'Connection refused')`, status None. Nothing is ever served, so TLS is never negotiated, no certificate is presented, and no access-log record can exist.
Why: an HTTPS-fronted workload is unusable and undiagnosable. A client gets a transport-level connection refusal, which is indistinguishable from addressing the wrong host or from a security-group denial, instead of the listener's response; and because no request is ever served, every downstream signal (access logs, TLS/SNI fields, request metrics) is permanently empty rather than wrong.
Fix: the ELBv2 listener data plane — bind and serve the address the load balancer announces on the listener's port, terminate TLS with the listener's certificate and SSL policy, execute the listener's default action, and then emit the access-log record for the served request. Adjacent open issues 20261109T153103Z-elbv2-s3-2-net-new-causes-modifyloadbalancerattributes-enables.md and 20261104T055008Z-elbv2-s3-an-https-alb-writes-no-access.md describe this area (unserved DNSName, missing access-log records) but do not carry this check's replay key.
Traces: elbv2__s3.log-edge-producers-alb-https-access-log-records-negotiated-tls-and-sni.

## Evidence
elbv2__s3.log-edge-producers-alb-https-access-log-records-negotiated-tls-and-sni:
"File check_elbv2__s3_log_edge_producers_alb_https_access_log_records_negotiated_tls_and_sni.py, line 388, in run / _poll_until(lambda: _get(f\"/{marker}/warm\", dns), lambda r: r.get(\"status\") == int(_STATUS), ...) / AssertionError: the HTTPS listener serving not met within 420s. Last result: {'status': None, 'error': \"ConnectionRefusedError(111, 'Connection refused')\"}"
Expected: the HTTPS listener answers the fixed-response status code declared on the default action (the check uses a 4xx fixed response and its own body), then the access log record carries the negotiated TLS protocol/cipher, the SNI, the chosen certificate ARN and `-` for every target field.

## Steps to Reproduce
1. Create an internet-facing ALB in two subnets with a security group permitting inbound TCP 443.
2. Import a self-signed certificate into ACM and CreateListener(Protocol=HTTPS, Port=443, SslPolicy=<a TLS1.3-capable policy>, Certificates=[that cert], DefaultActions=[{Type: fixed-response, FixedResponseConfig: {StatusCode, ContentType, MessageBody}}]).
3. Wait for the load balancer State.Code to become `active`, then open a TLS connection to its DNSName on port 443 and send `GET /warm HTTP/1.1`.
   Expect: the TLS handshake completes presenting the imported certificate and the fixed response is returned.
   Observe: ConnectionRefusedError(111) on every attempt for 420 s.
4. Re-replay the check id in trace_ids to confirm.
