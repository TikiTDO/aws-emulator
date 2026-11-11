Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-domain-routing-rule-bogus-child-and-bounds
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-domain-routing-rule-bogus-child-and-bounds
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2|GetDomainName|2xx->2xx|$.domainNameConfigurations[*].apiGatewayDomainName
Issue-Related: 20261029T220031Z-apigatewayv2-responses-omit-defaulted-and-derived-members.md
Issue-Commit: c4fcf6952896a9081c9de16adffed799f1736bf9

# apigatewayv2 GetDomainName returns an account-qualified domain ARN and an unserved domain host

> _Note: this issue was identified against the code base at commit `c4fcf6952896a9081c9de16adffed799f1736bf9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDomainName

## Description
One net-new cause, filed under the apigatewayv2 service token (the same representation defect is filed separately for the apigateway token; the two are distinct services and distinct replay keys). The open issue 20261109T132410Z-acm-apigatewayv2-an-available-regional-custom-domain-publishes.md describes the unserved d-* host behaviorally and carries none of this step's replay keys.

Cause 1 of 1: GetDomainName's representation of a custom domain.
AWS: Smithy: GetDomainName output members DomainNameArn (target Arn, jsonName 'domainNameArn') and DomainNameConfigurations (DomainNameConfiguration list, member apiGatewayDomainName, 'A domain name for the API'); http GET /v2/domainnames/{DomainName} code 200. An API Gateway domain-name ARN carries an EMPTY account-id section: the template is 'arn:aws:apigateway:<region>::/domainnames/<domainName>', the variable parts being the request's region and the domain name. apiGatewayDomainName is the target host a caller CNAMEs its domain to, and must therefore be an address that serves the API.
Emulator: answers 200 but returns 'arn:aws:apigateway:<region>:<accountId>:/domainnames/<domainName>' (recorded with the 000000000000 account section where AWS has none), and apiGatewayDomainName as an execute-api host the emulator does not answer on (verdict host_not_own, recorded 'd-22ftwv00cl.execute-api.us-west-2.amazonaws.com').
Why: ARN-keyed IAM conditions, tag-API calls and CFN/CDK comparisons against the ARN AWS produces never match an account-qualified one; and a client that follows apiGatewayDomainName to reach its custom domain reaches nothing the emulator serves, so the custom-domain data path cannot be exercised at all.
Fix: the apigatewayv2 DomainName serializer/state — the apigateway ARN formatter (no account section) and the apiGatewayDomainName derivation (publish the address the emulator's execute-api front door serves).
Traces: apigatewayv2-domain-routing-rule-bogus-child-and-bounds#task#3.

## Evidence
Replay of simparity-wire-aws-v1, cluster with empty stub_path and empty owned_by:

apigatewayv2|GetDomainName|status=200->200, fixture apigatewayv2-domain-routing-rule-bogus-child-and-bounds step task#3 —
  $.domainNameArn: expected 'arn:aws:apigateway:us-west-2::/domainnames/sim2real-probe-agwv2-err-rr-1158fd51.example.com', got 'arn:aws:apigateway:us-west-2:000000000000:/domainnames/sim2real-probe-agwv2-err-rr-1158fd51.example.com'
  $.domainNameConfigurations[0].apiGatewayDomainName: host_not_own (got 'd-22ftwv00cl.execute-api.us-west-2.amazonaws.com')

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-wire-aws-v1.
2. Create a custom domain name, then GET /v2/domainnames/{domainName}.
3. Compare domainNameArn — the account-id section must be empty ('arn:aws:apigateway:<region>::/domainnames/<domainName>') — and check whether domainNameConfigurations[0].apiGatewayDomainName is an address this emulator serves. Replay apigatewayv2-domain-routing-rule-bogus-child-and-bounds#task#3.
