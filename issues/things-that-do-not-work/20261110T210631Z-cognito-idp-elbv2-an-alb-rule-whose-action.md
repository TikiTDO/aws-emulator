Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__elbv2.alb-authenticate-cognito-redirects-an-unauthenticated-request-to-the-hosted-ui
Issue-Key: cognito-idp__elbv2.alb-authenticate-cognito-redirects-an-unauthenticated-request-to-the-hosted-ui|run#51|AssertionError|assert redirected["status"] == 302, (
Issue-Commit: 7753983c6e495ec3612695b1336bf4936fc7b7fd

# cognito-idp__elbv2: an ALB rule whose action is authenticate-cognito answers HTTP 500 Internal Server Error instead of the hosted-UI 302 (the fixed-response-only rule on the same listener serves 200)

> _Note: this issue was identified against the code base at commit `7753983c6e495ec3612695b1336bf4936fc7b7fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateListener

## Description
One net-new root cause.

Cause 1 of 1: evaluating an `authenticate-cognito` action on the ALB data plane fails with a load-balancer 500 instead of producing the IdP redirect. Failing check: cognito-idp__elbv2.alb-authenticate-cognito-redirects-an-unauthenticated-request-to-the-hosted-ui

AWS: Smithy (elasticloadbalancingv2) models AuthenticateCognitoActionConfig with required UserPoolArn, UserPoolClientId, UserPoolDomain plus Scope, SessionCookieName, SessionTimeout and OnUnauthenticatedRequest; the modeled documentation of AuthenticateCognitoActionConfig.OnUnauthenticatedRequest states the three behaviours verbatim: "deny - Return an HTTP 401 Unauthorized error", "allow - Allow the request to be forwarded to the target", "authenticate - Redirect the request to the IdP authorization endpoint. This is the default value." The wire shape of that redirect is not in the model — recorded behavior and the ELB documentation the check cites (listener-authenticate-users) only: an unauthenticated request matching a rule whose authenticate-cognito action has OnUnauthenticatedRequest=authenticate is answered HTTP 302 with a Location of https://<UserPoolDomain hosted UI host>/oauth2/authorize carrying client_id=<UserPoolClientId>, response_type=code, redirect_uri built from the *incoming request's* Host header (not the load balancer's own DNS name), scope=<the action's Scope> and a non-empty state, and with a Set-Cookie whose name begins with AWSALBAuthNonce. A request carrying an unverifiable AWSALBAuthSessionCookie-0 is unauthenticated too and gets the same 302, never the protected body. A rule with OnUnauthenticatedRequest=deny answers HTTP 401 and sets no AWSALBAuthNonce cookie. The pool domain prefix, client id, Host, state value and nonce body are per-run values taken from the request and the emulator's own state; the statuses, the authorize path and the parameter set are the requirement.

Emulator: the ALB data path itself now works — the sibling rule on the same HTTPS listener that carries only the fixed-response action serves HTTP 200 with the expected body, so TLS terminates and rules are matched. But the request that matches a rule carrying the authenticate-cognito action is answered `500 'Internal Server Error'` with no Location header: "an unauthenticated request to an authenticate-cognito rule must be redirected with a 302, got 500 'Internal Server Error'". Because the very first authenticate-cognito request 500s, the forged-AWSALBAuthSessionCookie-0 case and the OnUnauthenticatedRequest=deny 401 case are never reached either. All control-plane steps pass, including DescribeRules echoing AuthenticateCognitoConfig.UserPoolArn / UserPoolClientId / UserPoolDomain and OnUnauthenticatedRequest=deny.

Why: a client that puts Cognito sign-in at the edge gets a 5xx for every unauthenticated browser hit instead of being bounced to the user pool's hosted UI, so no authorization-code flow can ever start; the deny path cannot be observed either. A 500 is also not a refusal a client can act on — it reads as an emulator fault rather than the documented redirect, and retry logic in front of the ALB will spin.

Fix: the ELBv2 listener/rule data-plane action evaluator — the component that executes an action chain in Order must implement the authenticate-cognito action (resolve the pool domain to its hosted-UI host, mint the state and AWSALBAuthNonce cookie, emit the 302 for OnUnauthenticatedRequest=authenticate, the 401 for deny, pass-through for allow, and treat an unverifiable session cookie as unauthenticated) ahead of the ordered fixed-response/forward action, rather than raising and collapsing to a generic 500.

Related but distinct open issues, neither of which covers this: 20261104T092533Z-cognito-idp-elbv2-an-alb-https-listener-carrying.md (the listener served nothing at all — a timeout, not a 500) and 20261109T124858Z-cognito-idp-elbv2-an-alb-s-published-dnsname.md (the published DNSName was an unserved elb.amazonaws.com host). Both of those blocked the request from ever being answered; the request is now answered, and answered wrongly.

Traces: cognito-idp__elbv2.alb-authenticate-cognito-redirects-an-unauthenticated-request-to-the-hosted-ui.

## Evidence
Expected (AWS): GET / over HTTPS to the ALB (Host = the certificate's host, no session cookie) -> HTTP 302, Location = https://<domain-prefix hosted UI>/oauth2/authorize?client_id=<created app client id>&response_type=code&redirect_uri=https://<request Host>/oauth2/idpresponse&scope=openid%20email&state=<non-empty>, plus Set-Cookie: AWSALBAuthNonce...; the same request with Cookie: AWSALBAuthSessionCookie-0=<forged> -> still HTTP 302 and never the protected body; GET /deny (OnUnauthenticatedRequest=deny) -> HTTP 401 with no AWSALBAuthNonce cookie. (Fixed text/structure = the status, the /oauth2/authorize path and the parameter names; client_id, domain prefix, Host, state and nonce are per-run values.)

Actual (emulator): the positive control passed — GET /open (fixed-response only) returned 200 with the per-run nonce body — and then:
  File "check_cognito_idp__elbv2_alb_authenticate_cognito_redirects_an_unauthenticated_request_to_the_hosted_ui.py", line 212, in run
    assert redirected["status"] == 302, (
  AssertionError: an unauthenticated request to an authenticate-cognito rule must be redirected with a 302, got 500 'Internal Server Error'
No Location header and no AWSALBAuthNonce cookie are produced. DescribeRules on the deny rule correctly reported the authenticate-cognito action with the pool ARN, client id, domain prefix and OnUnauthenticatedRequest=deny before any HTTP was sent.

## Steps to Reproduce
1. acm.ImportCertificate with a self-signed root + leaf for host <h>.example.com; wait for Status=ISSUED.
2. Create a VPC with two public subnets and a security group; elbv2.CreateLoadBalancer (Type=application, Scheme=internet-facing); note DNSName.
3. cognito-idp.CreateUserPool; CreateUserPoolDomain(<prefix>) and wait for Status=ACTIVE; CreateUserPoolClient with GenerateSecret=true, AllowedOAuthFlowsUserPoolClient=true, AllowedOAuthFlows=[code], AllowedOAuthScopes=[openid,email], CallbackURLs=[https://<h>.example.com/oauth2/idpresponse], SupportedIdentityProviders=[COGNITO].
4. elbv2.CreateListener Protocol=HTTPS Port=443 with the imported certificate and DefaultActions = [authenticate-cognito(UserPoolArn, UserPoolClientId, UserPoolDomain=<prefix>, Scope='openid email', OnUnauthenticatedRequest=authenticate) Order=1, fixed-response 200 <nonce> Order=2].
5. elbv2.CreateRule priority 1, path-pattern /deny*, actions = [the same authenticate-cognito action but OnUnauthenticatedRequest=deny, fixed-response 200 <nonce> Order=2]; CreateRule priority 2, path-pattern /open*, actions = [fixed-response 200 <nonce>].
6. Wait for the load balancer State.Code=active. Over TLS to the load balancer endpoint on 443 with SNI/Host = <h>.example.com (trusting the minted root), send 'GET /open' -> observed 200 with the nonce (correct).
7. Send 'GET /' with no cookie. Expected HTTP 302 to <prefix> hosted UI /oauth2/authorize with client_id/response_type=code/redirect_uri/scope/state and an AWSALBAuthNonce cookie. Observed: HTTP 500 'Internal Server Error', no Location.
8. (Unreached because of step 7) 'GET /' with Cookie: AWSALBAuthSessionCookie-0=<forged> -> expect 302, never the nonce body; 'GET /deny' -> expect 401 with no AWSALBAuthNonce cookie.
