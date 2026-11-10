Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.e2e-blue-green-weighted-target-group-shift elbv2.e2e-http-to-https-redirect-traffic-flow elbv2.deregistration-delay-serves-in-flight-requests-while-draining elbv2.elbv2shape-deep-elbv2-3-slow-start-ramps-late-target-but-not-one-call-batch elbv2.target-response-exceeding-idle-timeout-yields-504 elbv2.redirect-action-placeholder-substitution elbv2.elbv2attr-deep-elbv2-1-mtls-passthrough-clientcert-chain elbv2.elbv2attr-deep-elbv2-1-mtls-passthrough-forged-clientcert-header-stripped elbv2.alb-generated-responses-excluded-from-requestcount-and-target-codes elbv2.desync-mitigation-strictest-blocks-ambiguous-framing-before-the-target elbv2.drop-invalid-header-fields-strips-the-header-before-the-target elbv2.forward-weighted-and-sticky-target-groups-data-plane elbv2.instance-target-serves-http-and-unhealthy-stops-routing elbv2.listener-ssl-policy-bounds-the-negotiated-tls-version-and-cipher elbv2.routing-http2-attribute-decides-the-alpn-protocol-the-alb-selects elbv2.xff-header-processing-mode-changes-what-the-target-receives elbv2.zero-weight-target-group-is-never-selected
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elbv2.e2e-blue-green-weighted-target-group-shift|_poll_http_get#23|EndpointRefused|_url, _route = ctx.endpoint_route(f"http://{host}:{port}")
Issue-Key: elbv2.e2e-http-to-https-redirect-traffic-flow|_poll_http_get#21|EndpointRefused|_url, _route = ctx.endpoint_route(f"http://{host}:{port}")
Issue-Key: elbv2.deregistration-delay-serves-in-flight-requests-while-draining|_poll_until#27|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: elbv2.elbv2shape-deep-elbv2-3-slow-start-ramps-late-target-but-not-one-call-batch|_poll_until#22|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: elbv2.target-response-exceeding-idle-timeout-yields-504|_poll_until#24|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: elbv2.redirect-action-placeholder-substitution|run#24|AssertionError|assert parsed.hostname == dns.lower(), f"#{{host}} not substituted with the request host: {location!r}"
Issue-Key: elbv2.elbv2attr-deep-elbv2-1-mtls-passthrough-clientcert-chain|run#31|AssertionError|assert _HDR in hdrs, (
Issue-Key: elbv2.elbv2attr-deep-elbv2-1-mtls-passthrough-forged-clientcert-header-stripped|run#30|AssertionError|assert control is not None and len(control) == 1, (
Issue-Key: elbv2.alb-generated-responses-excluded-from-requestcount-and-target-codes|_poll_until#41|Timeout|time.sleep(interval_s)
Issue-Key: elbv2.desync-mitigation-strictest-blocks-ambiguous-framing-before-the-target|_raw_request#31|Timeout|assert raw, f"the load balancer at {host}:{port} returned zero bytes for a raw HTTP request"
Issue-Key: elbv2.drop-invalid-header-fields-strips-the-header-before-the-target|_poll_until#27|Timeout|time.sleep(interval_s)
Issue-Key: elbv2.forward-weighted-and-sticky-target-groups-data-plane|_http#26|Timeout|except urllib.error.HTTPError as exc:
Issue-Key: elbv2.instance-target-serves-http-and-unhealthy-stops-routing|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Key: elbv2.listener-ssl-policy-bounds-the-negotiated-tls-version-and-cipher|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: elbv2.routing-http2-attribute-decides-the-alpn-protocol-the-alb-selects|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: elbv2.xff-header-processing-mode-changes-what-the-target-receives|_http#21|Timeout|except urllib.error.HTTPError as exc:
Issue-Key: elbv2.zero-weight-target-group-is-never-selected|_poll_until#28|Timeout|time.sleep(interval_s)
Issue-Commit: b35089c281a7e84768d928c8c72034d27eee1ae8

# elbv2: 5 net-new data-plane causes — DNSName is published with an embedded port, the ALB answers 502 for registered targets, #{host} resolves to the control-plane endpoint, mTLS headers are never passed, and no TLS/attribute data-plane behaviour converges

> _Note: this issue was identified against the code base at commit `b35089c281a7e84768d928c8c72034d27eee1ae8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLoadBalancer

## Description
Five net-new root causes in the elbv2 service (Cause 1 of 5 .. Cause 5 of 5). Related open filing that does NOT carry these replay keys: 20261109T153103Z-elbv2-4-net-new-causes-the-published-dnsname.md records the earlier state in which DNSName was an unserved elb.amazonaws.com host and the harness refused the published address before connect; the published host is now the emulator's own, so these checks reach the data plane and fail differently (a port inside DNSName, 502s, a wrong redirect host, missing mTLS headers), and the suite did not mark any of them OWNED. 20261104T094656Z-elbv2-seven-target-health-routing-and-data-plane-causes-still-open.md, 20261107T080944Z-elbv2-2-net-new-data-plane-root-causes.md and 20261105T093535Z-elbv2-4-net-new-data-plane-causes-desync.md are adjacent but carry none of the keys below.

Cause 1 of 5: LoadBalancer.DNSName is published with a port embedded in it, so it is not a DNS name at all and any client that appends a listener port produces a malformed address.
AWS: Smithy (elasticloadbalancingv2) CreateLoadBalancer / DescribeLoadBalancers -> LoadBalancers[].DNSName, documented as "The public DNS name of the load balancer" (shape DNSName), and LoadBalancer.Scheme documents that an internet-facing load balancer's DNS name resolves to the addresses of its nodes. A DNS name is a host only: it carries no ":<port>" and a client composes "http://<DNSName>:<listener port>/<path>" or dials <DNSName>:<port> on a raw socket. The requirement is that DNSName is a bare host that resolves to an address the listener ports answer on.
Emulator: DNSName is "<host>:<port>" (a host with a port appended), so composing the documented URL yields a second colon and the address is rejected before connect. Replay: simparity.target.EndpointRefused "endpoint 'http://127.0.0.1:46023:80' is not a http/https/ws/wss URL with a host" — the host and port digits are per-run values; the defect is the port inside DNSName.
Why: every client that does the one thing a load balancer exists for — take the name the API handed back and dial a listener port on it — builds an unusable address. A 200 from CreateLoadBalancer that returns a host:port pair where a hostname is modeled is worse than a refusal, because nothing tells the client.
Fix: the ELBv2 load-balancer state/serializer that mints and reports DNSName — it must publish a bare host that answers on the listener ports, with the port expressed only by the listener, not folded into the name.
Traces: elbv2.e2e-blue-green-weighted-target-group-shift, elbv2.e2e-http-to-https-redirect-traffic-flow

Cause 2 of 5: the load balancer answers HTTP 502 for a target it has registered (and reports healthy) instead of forwarding the request to it.
AWS: Smithy (elasticloadbalancingv2) RegisterTargets, CreateListener DefaultActions[] {Type 'forward', TargetGroupArn}, DescribeTargetHealth -> TargetHealth.{State,Reason}. On AWS a registered, healthy target receives the request and its response is returned verbatim; HTTP 502 is the load-balancer-generated status reserved for a target that could not be reached or returned a malformed response (reason class Target.FailedHealthChecks). In particular: with deregistration_delay.timeout_seconds set, a target in 'draining' continues to serve in-flight requests for the delay; a target in slow_start.duration_seconds ramps but still serves 200; and a target that exceeds idle_timeout.timeout_seconds must yield 504, not 502.
Emulator: the first and every response is HTTP 502 with an empty body. Replay: "load balancer serving 'TARGET-A' at http://<host>:<port>/ (observed: []) not met within 300s. Last result: (502, '')"; "http://<host>:<port>/x answering 200 (observed: []) not met within 300s. Last result: 502"; "load balancer serving 'SIM2REAL-TARGET-BODY' at http://<host>:<port>/?s=0 ... Last result: (502, '')". The hosts, ports and body markers are per-run values; the defect is the constant 502.
Why: a client gets a load-balancer-generated 502 for a healthy backend, so it blames its own target; and because no request is ever forwarded, the behaviours layered on forwarding — draining during deregistration delay, slow-start ramping, and the 504 an over-idle-timeout target must produce — are all unobservable and indistinguishable from each other.
Fix: the ELBv2 data-plane forwarding path — action evaluation resolving a target group to a registered target's address, the proxying of request and response, and the status generation that must distinguish a genuine 502 from a 504 (idle timeout) and from a served response.
Traces: elbv2.deregistration-delay-serves-in-flight-requests-while-draining, elbv2.elbv2shape-deep-elbv2-3-slow-start-ramps-late-target-but-not-one-call-batch, elbv2.target-response-exceeding-idle-timeout-yields-504

Cause 3 of 5: a redirect action substitutes #{host} with the ELBv2 control-plane endpoint host instead of the host of the request being redirected.
AWS: Smithy (elasticloadbalancingv2) RedirectActionConfig — Host is documented as "The hostname... The hostname can contain #{host}", and Path as "The absolute path... The path can contain #{host}, #{path}, and #{port}"; the reserved keywords are substituted from the REQUEST. So for a request with Host header H to path P on port R, #{host} resolves to H, #{path} to P and #{port} to R, and the Location header is the configured protocol://host:port/path?query with those substitutions applied.
Emulator: #{host} is replaced with the load balancer's service endpoint host rather than the request host. Replay: AssertionError "#{host} not substituted with the request host: 'https://elasticloadbalancingv2.amazonaws.com:8443/movedredir/deep/thing?orig=redir/deep/thing&keep=a=1&b=2'" — the port and path segments are this run's values; the defect is that the substituted host is the ELBv2 API endpoint name instead of the request's own host.
Why: #{host} exists so that a redirect preserves the caller's hostname (vhosts, custom domains, HTTP->HTTPS on the same name). Substituting an AWS service endpoint sends every redirected client to a host that has nothing to do with the application — a broken redirect loop or an off-site hop — and the rest of the Location (#{path}, #{port}, query) cannot be trusted either.
Fix: the ELBv2 redirect-action serializer in the listener-rule action evaluator — the placeholder substitution must read #{protocol}, #{host}, #{port}, #{path} and #{query} from the inbound request, not from the service's own endpoint configuration.
Traces: elbv2.redirect-action-placeholder-substitution

Cause 4 of 5: an HTTPS listener in mTLS passthrough mode forwards no x-amzn-mtls-clientcert* headers to the target even when the client presented a certificate.
AWS: Smithy (elasticloadbalancingv2) CreateListener/ModifyListener MutualAuthentication {Mode, TrustStoreArn, IgnoreClientCertificateExpiry}. In passthrough mode AWS sends the client certificate chain to the target in HTTP headers — x-amzn-mtls-clientcert (the URL-encoded chain), plus x-amzn-mtls-clientcert-leaf / -subject / -issuer / -validity / -serial-number / -fingerprint-style companions — and strips any same-named header a client tried to forge, so the target can only ever see the load balancer's own values. The presence of x-amzn-mtls-clientcert for a request that presented a certificate, and the stripping of a forged one, are the requirement.
Emulator: the target receives no x-amzn-* header at all. Replay: AssertionError "the target received no 'x-amzn-mtls-clientcert' header although the client presented a certificate; x-amzn* headers it did get: []" and, in the forgery control, "control: a real client certificate produced x-amzn-mtls-clientcert values None; expected one".
Why: passthrough mTLS exists so the application can authenticate the client from the certificate the load balancer validated. With no header, the target cannot identify the caller; worse, since the control also fails, nothing demonstrates that a forged x-amzn-mtls-clientcert is stripped — an application that trusts that header would be trivially spoofable.
Fix: the ELBv2 HTTPS listener's mTLS component — after validating the client certificate against the trust store it must inject the x-amzn-mtls-clientcert family into the request forwarded to the target, and unconditionally strip those header names from the inbound request first.
Traces: elbv2.elbv2attr-deep-elbv2-1-mtls-passthrough-clientcert-chain, elbv2.elbv2attr-deep-elbv2-1-mtls-passthrough-forged-clientcert-header-stripped

Cause 5 of 5: no TLS-negotiation or load-balancer-attribute data-plane behaviour ever converges — nine checks spanning request metrics, header handling, framing, routing selection and TLS negotiation are each killed by their ceiling with nothing graded.
AWS: Smithy (elasticloadbalancingv2) CreateListener SslPolicy (bounds the negotiated TLS version and cipher suite); ModifyLoadBalancerAttributes / LoadBalancerAttribute.Key for routing.http2.enabled (decides the ALPN protocol the load balancer selects), routing.http.drop_invalid_header_fields.enabled (strips invalid header fields before the target), routing.http.desync_mitigation_mode (blocks ambiguously framed requests with the load balancer's own 400) and routing.http.xff_header_processing.mode (append/preserve/remove changes what X-Forwarded-For the target receives); ModifyRule forward action with TargetGroupStickinessConfig and per-group Weight (a zero-weight group is never selected; weighted and sticky selection decide which group answers); and the ALB metrics documentation for RequestCount/HTTPCode_Target_4XX_Count, which are not incremented for a load-balancer-generated response because no target was chosen. Each of these is a per-request, attribute-driven behaviour that must be observable on the wire.
Emulator: each check exhausts its declared ceiling with no graded observation — eight at "Check timed out after 60.0s" and one at "Check timed out after 600.0s" — so no attribute, SSL policy, ALPN selection, stickiness or weight has any demonstrable effect.
Why: these attributes are the knobs a client turns to make a load balancer behave safely (desync mitigation, dropping invalid headers, XFF handling) or correctly (SSL policy floors, HTTP/2 negotiation, weighted and sticky routing, zero-weight drain). If none of them is observable, a client cannot verify any of its hardening or traffic-shaping configuration, and an inert attribute is indistinguishable from a working one.
Fix: the ELBv2 data-plane request pipeline — the TLS terminator (SslPolicy-bounded version/cipher negotiation and ALPN selection from routing.http2.enabled), the header/framing stage (drop_invalid_header_fields, desync_mitigation_mode, xff_header_processing), the target-selection stage (weights, zero-weight exclusion, target-group stickiness) and the metric publisher that must exclude load-balancer-generated responses from RequestCount and the target status-code series.
Traces: elbv2.alb-generated-responses-excluded-from-requestcount-and-target-codes, elbv2.desync-mitigation-strictest-blocks-ambiguous-framing-before-the-target, elbv2.drop-invalid-header-fields-strips-the-header-before-the-target, elbv2.forward-weighted-and-sticky-target-groups-data-plane, elbv2.instance-target-serves-http-and-unhealthy-stops-routing, elbv2.listener-ssl-policy-bounds-the-negotiated-tls-version-and-cipher, elbv2.routing-http2-attribute-decides-the-alpn-protocol-the-alb-selects, elbv2.xff-header-processing-mode-changes-what-the-target-receives, elbv2.zero-weight-target-group-is-never-selected

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the booted emulator: 266/603 passed (44.1%); service heading elbv2 177/303 (58%), 126 failed — 109 of them OWNED or ALREADY FILED; the 17 ids in trace_ids survive.

Cause 1 (DNSName carries a port):
  [error] elbv2.e2e-blue-green-weighted-target-group-shift
    File "context.py", line 598, in _published_target -> validate_endpoint(...)
    File "target.py", line 154, in validate_endpoint -> raise EndpointRefused(...)
    simparity.target.EndpointRefused: endpoint 'http://127.0.0.1:46023:80' is not a http/https/ws/wss URL with a host
  [error] elbv2.e2e-http-to-https-redirect-traffic-flow — same frame, endpoint 'http://127.0.0.1:40115:80'
  Expected: DNSName is a bare host, so "http://<DNSName>:80" is a valid URL that reaches the HTTP:80 listener. Actual: DNSName already contains ":<port>", producing 'http://<host>:<port>:80'.

Cause 2 (constant 502 for registered targets):
  [mismatch] elbv2.deregistration-delay-serves-in-flight-requests-while-draining
    AssertionError: load balancer serving 'TARGET-A' at http://127.0.0.1:39843/ (observed: []) not met within 300s. Last result: (502, '')
  [mismatch] elbv2.elbv2shape-deep-elbv2-3-slow-start-ramps-late-target-but-not-one-call-batch
    AssertionError: http://127.0.0.1:40253/x answering 200 (observed: []) not met within 300s. Last result: 502
  [mismatch] elbv2.target-response-exceeding-idle-timeout-yields-504
    AssertionError: load balancer serving 'SIM2REAL-TARGET-BODY' at http://127.0.0.1:36439/?s=0 (observed: []) not met within 300s. Last result: (502, '')
  Expected: HTTP 200 with the target's body (and HTTP 504, not 502, once the target exceeds idle_timeout.timeout_seconds). Actual: HTTP 502 with an empty body on every attempt for 300 s.

Cause 3 (#{host} substitution):
  [mismatch] elbv2.redirect-action-placeholder-substitution
    File "check_elbv2_redirect_action_placeholder_substitution.py", line 320, in run -> assert parsed.hostname == dns.lower()
    AssertionError: #{host} not substituted with the request host: 'https://elasticloadbalancingv2.amazonaws.com:8443/movedredir/deep/thing?orig=redir/deep/thing&keep=a=1&b=2'
  Expected: Location's host is the request's own host (the load balancer address the client dialled), with #{path}/#{port}/query substituted from the request. Actual: the host is the ELBv2 service endpoint name.

Cause 4 (mTLS passthrough headers):
  [mismatch] elbv2.elbv2attr-deep-elbv2-1-mtls-passthrough-clientcert-chain
    AssertionError: the target received no 'x-amzn-mtls-clientcert' header although the client presented a certificate; x-amzn* headers it did get: []
  [mismatch] elbv2.elbv2attr-deep-elbv2-1-mtls-passthrough-forged-clientcert-header-stripped
    AssertionError: control: a real client certificate produced x-amzn-mtls-clientcert values None; expected one
  Expected: x-amzn-mtls-clientcert (plus the -leaf/-subject/-issuer/-validity/-serial-number companions) present for a presented certificate, and any client-supplied header of those names stripped. Actual: no x-amzn-* header reaches the target at all.

Cause 5 (nothing converges) — each exhausts its own declared ceiling with nothing graded:
  [mismatch] elbv2.alb-generated-responses-excluded-from-requestcount-and-target-codes — Check timed out after 600.0s
  [mismatch] elbv2.desync-mitigation-strictest-blocks-ambiguous-framing-before-the-target — Check timed out after 60.0s
  [mismatch] elbv2.drop-invalid-header-fields-strips-the-header-before-the-target — Check timed out after 60.0s
  [mismatch] elbv2.forward-weighted-and-sticky-target-groups-data-plane — Check timed out after 60.0s
  [mismatch] elbv2.instance-target-serves-http-and-unhealthy-stops-routing — Check timed out after 60.0s
  [mismatch] elbv2.listener-ssl-policy-bounds-the-negotiated-tls-version-and-cipher — Check timed out after 60.0s
  [mismatch] elbv2.routing-http2-attribute-decides-the-alpn-protocol-the-alb-selects — Check timed out after 60.0s
  [mismatch] elbv2.xff-header-processing-mode-changes-what-the-target-receives — Check timed out after 60.0s
  [mismatch] elbv2.zero-weight-target-group-is-never-selected — Check timed out after 60.0s

Smithy consulted (model services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10): LoadBalancer.DNSName ("The public DNS name of the load balancer") and LoadBalancer.Scheme; RedirectActionConfig.{Protocol (pattern ^(HTTPS?|#\{protocol\})$), Host ("The hostname can contain #{host}"), Path ("The path can contain #{host}, #{path}, and #{port}"), Query, StatusCode (required)}; TargetHealth.{State,Reason,Description} with its documented reason codes; LoadBalancerAttribute.Key (access_logs.*, deletion_protection.enabled, load_balancing.cross_zone.enabled, the routing.http.* family); CreateListener.{SslPolicy, MutualAuthentication}; RegisterTargets. ALB metric exclusion of load-balancer-generated responses and the x-amzn-mtls-clientcert header family: recorded behavior plus the ALB documentation the checks cite.

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; all 17 ids in trace_ids reproduce.
2. Cause 1: elbv2.create_load_balancer(Type='application', Scheme='internet-facing', Subnets=[2 subnets], SecurityGroups=[sg]) and read LoadBalancers[0].DNSName. Expected: a bare host. Actual: a '<host>:<port>' string, so composing "http://<DNSName>:80/" yields a URL with two ports and is rejected.
3. Cause 2: create a target group, register a target that serves a known body, create an HTTP listener forwarding to it, wait for healthy, then GET the load balancer. Expected 200 with the body; actual 502 with an empty body. Repeat with deregistration_delay.timeout_seconds set and the target deregistered (expect in-flight 200s while 'draining'), with slow_start.duration_seconds set (expect ramped 200s), and with a target that sleeps past idle_timeout.timeout_seconds (expect 504, not 502).
4. Cause 3: create a rule whose action is {Type:'redirect', RedirectConfig:{Protocol:'HTTPS', Host:'#{host}', Path:'/moved#{path}', Query:'orig=#{path}&keep=#{query}', StatusCode:'HTTP_301'}} and send a request with a known Host header, without following redirects. Expected: Location's host equals the request host. Actual: it is 'elasticloadbalancingv2.amazonaws.com'.
5. Cause 4: create an HTTPS listener with MutualAuthentication={Mode:'passthrough', TrustStoreArn:<ts>}, connect with a client certificate, and echo the request headers from the target. Expected x-amzn-mtls-clientcert (and companions); actual none. Repeat sending a forged x-amzn-mtls-clientcert and confirm it is stripped.
6. Cause 5: for each of SslPolicy, routing.http2.enabled, routing.http.drop_invalid_header_fields.enabled, routing.http.desync_mitigation_mode, routing.http.xff_header_processing.mode, weighted/zero-weight forward actions with TargetGroupStickinessConfig, and a fixed-response action, send the corresponding probe and observe the negotiated TLS version/cipher, the ALPN protocol, the headers the target receives, the 400 for ambiguous framing, which target group answers, and the RequestCount/HTTPCode_Target_4XX_Count series. Expected: each attribute changes the observed behaviour as modeled. Actual: no probe produces a graded observation within the budget.
7. Re-replay the 17 ids in trace_ids to confirm the fix.
