Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-domain-name-create-server-defaults
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-domain-name-create-server-defaults
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|CreateDomainName|2xx->2xx|$.domainNameConfigurations[*].apiGatewayDomainName
Issue-Key: apigateway|GetDomainName|2xx->2xx|$.domainNameConfigurations[*].apiGatewayDomainName
Issue-Related: 20261029T220031Z-apigateway-resource-representations-omit-derived-members-and-drop.md
Issue-Commit: c4fcf6952896a9081c9de16adffed799f1736bf9

# apigateway custom-domain representation diverges on domain ARN, hosted zone id, tags and the published domain host

> _Note: this issue was identified against the code base at commit `c4fcf6952896a9081c9de16adffed799f1736bf9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomainName

## Description
Two net-new causes: the custom-domain representation the emulator returns on create and on read. Both steps answer the modeled status (201 / 200); the members inside are wrong. The open issue 20261109T132410Z-acm-apigatewayv2-an-available-regional-custom-domain-publishes.md describes the unserved d-* host behaviorally but carries none of the replay keys below, so these causes are not owned.

Cause 1 of 2: CreateDomainName's response projection.
AWS: Smithy: CreateDomainName output members DomainNameArn (target Arn, jsonName 'domainNameArn'), DomainNameConfigurations (DomainNameConfiguration list, members apiGatewayDomainName and hostedZoneId documented as 'A domain name for the API' and 'The Amazon Route 53 Hosted Zone ID of the endpoint'), Tags (jsonName 'tags'); http POST /v2/domainnames code 201. Three requirements the recording pins down: (a) an API Gateway domain-name ARN has an EMPTY account-id section — the template is 'arn:aws:apigateway:<region>::/domainnames/<domainName>', with region and domain name taken from the request; (b) hostedZoneId is the fixed, AWS-published Route 53 hosted zone id of the regional API Gateway endpoint for that region (recorded: us-west-2 -> Z2OJLYMUO9EFXC) — a per-region constant, not a value minted per domain; (c) with no tags on the request, AWS OMITS the tags member rather than returning an empty map.
Emulator: returns 'arn:aws:apigateway:<region>:<accountId>:/domainnames/<domainName>' (recorded with the 000000000000 account section where AWS has none), a generated hostedZoneId (recorded 'Z87FCC56E081EA') in place of the regional constant, tags as {} where AWS omits it, and domainNameConfigurations[0].apiGatewayDomainName as an execute-api host it does not serve (verdict host_not_own, recorded 'd-22ftwv00cl.execute-api.us-west-2.amazonaws.com').
Why: ARN-matching IAM policies, CFN/CDK ARN comparisons and tag-presence checks all break — an account-qualified ARN never equals the ARN AWS produces for the same domain; a client that creates the Route 53 alias record from hostedZoneId points DNS at a zone that does not exist; code using 'tags' presence to detect an untagged resource sees it tagged; and a client that CNAMEs its custom domain to apiGatewayDomainName reaches an address the emulator does not answer.
Fix: the CreateDomainName handler's response projection and the DomainName state — the ARN formatter (drop the account section for apigateway ARNs), the per-region hosted-zone-id table, the tags serializer (omit when empty), and the published apiGatewayDomainName derivation (an address the emulator serves).
Traces: apigatewayv2-domain-name-create-server-defaults#task#2.

Cause 2 of 2: GetDomainName returns the same wrong representation on read.
AWS: Smithy: GetDomainName output members DomainNameArn, DomainNameConfigurations, http GET /v2/domainnames/{DomainName} code 200 — identical representation to create.
Emulator: the stored domain reads back with the account-qualified ARN, the generated hostedZoneId and the unserved apiGatewayDomainName.
Why: the divergence is persisted, not just a create-time formatting slip, so every later read, wait-loop or CFN drift comparison sees it.
Fix: the DomainName state/serializer shared with Cause 1 (store the domain, derive these members at serialization from the ARN formatter and hosted-zone table).
Traces: apigatewayv2-domain-name-create-server-defaults#task#3.

## Evidence
Replay of simparity-wire-aws-v1, clusters with empty stub_path and empty owned_by:

1) apigateway|CreateDomainName|status=201->201, fixture apigatewayv2-domain-name-create-server-defaults step task#2 —
   $.domainNameArn: expected 'arn:aws:apigateway:us-west-2::/domainnames/agwv2-cp-dn-dbe06e2f.example.com', got 'arn:aws:apigateway:us-west-2:000000000000:/domainnames/agwv2-cp-dn-dbe06e2f.example.com'
   $.domainNameConfigurations[0].hostedZoneId: expected 'Z2OJLYMUO9EFXC', got 'Z87FCC56E081EA'
   $.tags: type_mismatch (expected absent, got {})
   $.domainNameConfigurations[0].apiGatewayDomainName: host_not_own (got 'd-22ftwv00cl.execute-api.us-west-2.amazonaws.com')
2) apigateway|GetDomainName|status=200->200, fixture apigatewayv2-domain-name-create-server-defaults step task#3 — same domainNameArn, hostedZoneId and apiGatewayDomainName divergences on read.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-wire-aws-v1.
2. POST /v2/domainnames with a domainName and one REGIONAL domainNameConfiguration and no tags. Compare domainNameArn (account section must be empty), domainNameConfigurations[0].hostedZoneId (must be the region's published API Gateway regional hosted zone id, Z2OJLYMUO9EFXC for us-west-2), the absence of 'tags', and whether apiGatewayDomainName is an address the emulator serves. Replay apigatewayv2-domain-name-create-server-defaults#task#2.
3. GET /v2/domainnames/{domainName} and compare the same members. Replay apigatewayv2-domain-name-create-server-defaults#task#3.
