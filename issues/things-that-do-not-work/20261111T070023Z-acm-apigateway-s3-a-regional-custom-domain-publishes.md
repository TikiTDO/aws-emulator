Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm__apigateway__s3.regional-custom-domain-mutual-tls-enforces-trusted-client-certificates
Issue-Key: acm__apigateway__s3.regional-custom-domain-mutual-tls-enforces-trusted-client-certificates|_request#44|PublishedUrlRejected|with socket.create_connection(ctx.endpoint_host(gateway_host, 443), timeout=15) as raw:
Issue-Commit: d9b54befa158e14a4810d0b0face0e7b12d06851

# acm__apigateway__s3: a REGIONAL custom domain publishes regionalDomainName as a real-AWS d-*.execute-api.<region>.amazonaws.com host the emulator does not serve, so the mutual-TLS scenario is unreachable

> _Note: this issue was identified against the code base at commit `d9b54befa158e14a4810d0b0face0e7b12d06851`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomainName

## Description
One net-new root cause (Cause 1 of 1), grounded in one failing behavioral check. The check now gets past its ACM prerequisite (a DNS-validated certificate reaching ISSUED) and fails at the next step, so this is a different cause from the one the open filing for this check id was keyed to.

Cause 1 of 1: API Gateway (REST, v1) publishes a REGIONAL custom domain's regionalDomainName as an AWS-owned execute-api hostname instead of an address the emulator answers on, so no client can ever reach the custom-domain front door. Failing check: acm__apigateway__s3.regional-custom-domain-mutual-tls-enforces-trusted-client-certificates.

AWS: Smithy (services/service-com-amazonaws-apigateway-backplanecontrolservice/smithy-model/com-amazonaws-apigateway-backplanecontrolservice.json) DomainName.regionalDomainName — "The domain name associated with the regional endpoint for this custom domain name. You set up this association by adding a DNS record that points the custom domain name to this regional domain name. The regional domain name is returned by API Gateway when you create a regional endpoint." Recorded AWS behavior: once GetDomainName reports domainNameStatus AVAILABLE, a TCP+TLS connection to regionalDomainName:443 with SNI/Host = the custom domain completes under the configured securityPolicy, presents the ACM certificate named by regionalCertificateArn, and — with mutualTlsAuthentication over the S3 truststore — serves 200 for a client certificate the truststore trusts, refuses a connection presenting NO client certificate at the TLS layer (reset, close with no response, or a certificate-required alert), and refuses an untrusted-CA client certificate with HTTP 403, body {"message":"Forbidden"} and header x-amzn-ErrorType: ForbiddenException. The published address is therefore required to be reachable by the caller.

Emulator: the custom domain reports AVAILABLE and publishes regionalDomainName of the form d-<id>.execute-api.<region>.amazonaws.com (replay: 'd-3pzlwequrz.execute-api.us-west-2.amazonaws.com'; the d-<id> label is a per-run value, the AWS-owned domain is the defect). That host is none of the emulator's own hosts, so the harness refuses the address before any socket is opened: simparity.published.PublishedUrlRejected "endpoint_host was given d-<id>.execute-api.<region>.amazonaws.com:443 (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs an own host[:port]: the endpoint host, a documented own host or, for a loopback endpoint, a loopback address".

Why: the only address API Gateway hands a customer for a REGIONAL custom domain is unusable — it either escapes to real AWS or does not resolve. Certificate presentation under the custom domain, the securityPolicy that governs the handshake, and the whole mutualTlsAuthentication contract (trusted client cert -> 200, no client cert -> TLS-layer refusal with no HTTP response, untrusted CA -> 403 {"message":"Forbidden"} with x-amzn-ErrorType: ForbiddenException) are unobservable, while the control plane reports a fully provisioned AVAILABLE domain.

Fix: the apigateway custom-domain handler/state that mints and serializes DomainName.regionalDomainName (CreateDomainName, GetDomainName, UpdateDomainName) must publish the emulator's own reachable host[:port]; the custom-domain TLS front door must then terminate on that address with the ACM certificate named by regionalCertificateArn and enforce the configured mutualTlsAuthentication truststore.

Related open filings that do NOT cover this replay: 20261110T152935Z-acm-apigateway-s3-the-mutual-tls-regional-custom.md names this same check id but is keyed to a _poll_until Timeout on the ACM certificate never reaching ISSUED; the check no longer fails there, so the suite reports it unowned and this is the next, distinct defect. 20261109T132410Z-acm-apigatewayv2-an-available-regional-custom-domain-publishes.md records the same class of defect for apigatewayv2's ApiGatewayDomainName, and 20261111T014037Z-apigateway-custom-domain-representation-diverges-on-domain-arn.md / 20261111T014037Z-apigateway-publishes-api-and-portal-page-endpoints-at.md are filed under other service headings; none carries this check id.

Traces: acm__apigateway__s3.regional-custom-domain-mutual-tls-enforces-trusted-client-certificates.

## Evidence
Replay of suite simparity-behavioral-aws-v1: acm__apigateway__s3 0/1 (0%), 1 failed.

[mismatch] acm__apigateway__s3.regional-custom-domain-mutual-tls-enforces-trusted-client-certificates:
  File "context.py", line 650, in endpoint_host -> self._published_target(f"{published_host}:{port}", "endpoint_host")
  File "context.py", line 617, in _published_target -> raise PublishedUrlRejected(...)
  simparity.published.PublishedUrlRejected: endpoint_host was given d-3pzlwequrz.execute-api.us-west-2.amazonaws.com:443 (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs an own host[:port]: the endpoint host, a documented own host or, for a loopback endpoint, a loopback address

The check issues a DNS-validated ACM certificate for a name in the account's tagged public hosted zone, uploads a CA bundle to S3 as the truststore, creates a REGIONAL custom domain (endpointConfiguration={'types':['REGIONAL']}, regionalCertificateArn=<cert>, securityPolicy='TLS_1_2', mutualTlsAuthentication={'truststoreUri':'s3://<bucket>/truststore.pem','truststoreVersion':<version>}) with a base-path mapping to a deployed MOCK stage, waits for domainNameStatus AVAILABLE, then takes gateway_host = the published regionalDomainName and performs socket.create_connection(ctx.endpoint_host(gateway_host, 443)) with SNI/Host = the custom domain (see _request() in the check). Expected: the handshake completes and the three mTLS outcomes are observable. Actual: the published host is an AWS execute-api address the emulator does not serve, so no handshake is attempted.

Per-run values (the d-<id> label, region string, bucket/object names, certificate ids) are not requirements; the requirement is that the published regionalDomainName is an address the emulator answers on.

Smithy consulted: apigateway DomainName.regionalDomainName (and regionalCertificateArn, securityPolicy, mutualTlsAuthentication, domainNameStatus).

## Steps to Reproduce
1. Clone the repo, build and boot the emulator, then replay suite simparity-behavioral-aws-v1; acm__apigateway__s3.regional-custom-domain-mutual-tls-enforces-trusted-client-certificates reproduces.
2. Obtain an ISSUED ACM certificate for a name in the account's tagged public hosted zone (request_certificate with ValidationMethod='DNS' and publish the handed-out CNAME).
3. Put a CA bundle object into an S3 bucket to serve as the mTLS truststore.
4. apigateway.create_domain_name(domainName=<domain>, endpointConfiguration={'types':['REGIONAL']}, regionalCertificateArn=<arn>, securityPolicy='TLS_1_2', mutualTlsAuthentication={'truststoreUri':'s3://<bucket>/truststore.pem','truststoreVersion':<version>}) and add a base-path mapping to a deployed MOCK stage.
5. apigateway.get_domain_name(domainName=<domain>) until domainNameStatus == 'AVAILABLE', then read regionalDomainName: the emulator returns d-<id>.execute-api.<region>.amazonaws.com.
6. Open TLS to that host on 443 with SNI = the custom domain. AWS completes the handshake and enforces the truststore (trusted client cert -> 200 with the stage body; no client cert -> TLS-layer refusal with no HTTP response; untrusted-CA client cert -> 403 {"message":"Forbidden"} with x-amzn-ErrorType: ForbiddenException). The emulator published an address it does not serve.
7. Re-replay the check id in trace_ids to confirm the fix.
