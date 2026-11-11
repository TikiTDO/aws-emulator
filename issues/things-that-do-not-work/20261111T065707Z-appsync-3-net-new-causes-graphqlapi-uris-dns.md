Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.api-key-expiry-ttl-enforced-denies-access appsync.delete-api-key-revokes-data-plane-access appsync.e2e-api-cache-flush-then-fresh-fetch appsync.graphql-append-error-partial-data-and-nonnull-propagation appsync.graphql-pipeline-stash-prev-result-and-before-return appsync.graphql-query-execution-vtl-local-resolver appsync.graphql-query-list-argument-foreach-sum appsync.graphql-query-none-datasource-resolver appsync.pipeline-resolver-function-chain-stash-propagation appsync.pipeline-resolver-update-reorders-execution appsync.api-association-get-not-found-when-unassociated appsync.associate-api-domain-mismatched-certificate-rejected
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync.api-key-expiry-ttl-enforced-denies-access|_http_post_json#20|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync.delete-api-key-revokes-data-plane-access|_graphql_post#13|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync.e2e-api-cache-flush-then-fresh-fetch|_graphql_request#20|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync.graphql-append-error-partial-data-and-nonnull-propagation|_post_json#12|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync.graphql-pipeline-stash-prev-result-and-before-return|_post_json#15|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync.graphql-query-execution-vtl-local-resolver|_graphql_post#9|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(endpoint)
Issue-Key: appsync.graphql-query-list-argument-foreach-sum|_graphql_post#12|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync.graphql-query-none-datasource-resolver|_graphql_post#13|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync.pipeline-resolver-function-chain-stash-propagation|_graphql_post#13|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync.pipeline-resolver-update-reorders-execution|_graphql_post#13|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync.api-association-get-not-found-when-unassociated|run#9|AssertionError|assert message == "Domain name is not associated.", (
Issue-Key: appsync.associate-api-domain-mismatched-certificate-rejected|run#13|AssertionError|assert code == "BadRequestException", (
Issue-Commit: f456950adb04cb9e7696776dd0db02907fd8875a

# appsync: 3 net-new causes — GraphqlApi uris/dns published on real-AWS hostnames, GetApiAssociation NotFoundException message, CreateDomainName accepts a certificate that does not cover the domain

> _Note: this issue was identified against the code base at commit `f456950adb04cb9e7696776dd0db02907fd8875a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGraphqlApi

## Description
Three net-new root causes in the appsync emulator, each grounded in failing behavioral checks of this suite and each carrying its own failing check ids. All three must be closed before this issue is submitted.

=== Cause 1 of 3: CreateGraphqlApi/GetGraphqlApi publish GraphqlApi$uris and GraphqlApi$dns on real-AWS hostnames, so the emulator's own GraphQL data plane is unaddressable ===
AWS: Smithy: CreateGraphqlApi (POST /v1/apis, code 200) returns GraphqlApi, whose members `uris` and `dns` are both MapOfStringToString (doc: "The URIs." / "The DNS records for the API."), keyed GRAPHQL and REALTIME. On AWS those URIs are the only published address of the API's data plane: the caller reads uris.GRAPHQL and POSTs its GraphQL document there, and opens the realtime WebSocket against uris.REALTIME. The requirement is structural — the published address must be an address the serving endpoint actually answers on.
Emulator: the GraphqlApi projection emits uris.GRAPHQL = https://<26-char token>.appsync-api.<region>.amazonaws.com/graphql (and the matching wss://<token>.appsync-realtime-api.<region>.amazonaws.com/graphql), i.e. a hostname the emulator does not serve. Every check that follows the published URI is refused before any request is sent: simparity.published.PublishedUrlRejected: "endpoint_url was given https://<token>.appsync-api.us-west-2.amazonaws.com/graphql (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener". (Hosts/ports vary per run and are not part of the requirement; the defect is that the announced host is not one the emulator is reached on.)
Why: a client that does exactly what AWS documents — create an API, read uris.GRAPHQL, post a query — is sent off the emulator entirely. API-key TTL/revocation enforcement on the data plane, VTL and JS resolver execution, $util.foreach/list arguments, NONE-data-source resolvers, pipeline stash/prev.result threading, pipeline function re-ordering, api-cache flush effects and non-null error propagation are all unobservable, so the emulator appears to have no GraphQL data plane.
Fix: the GraphqlApi projection (uris/dns construction) shared by the CreateGraphqlApi / UpdateGraphqlApi / GetGraphqlApi / ListGraphqlApis handlers, plus the data-plane listener those URIs must name. NOTE: open issue 20261110T215409Z-appsync-10-net-new-causes-graphql-realtime-endpoints.md (Cause 1) describes this same published-URI defect for a different set of checks; the suite did not mark the ten checks below as owned by it, so they are filed here and may be closed by the same fix.
Traces: appsync.api-key-expiry-ttl-enforced-denies-access, appsync.delete-api-key-revokes-data-plane-access, appsync.e2e-api-cache-flush-then-fresh-fetch, appsync.graphql-append-error-partial-data-and-nonnull-propagation, appsync.graphql-pipeline-stash-prev-result-and-before-return, appsync.graphql-query-execution-vtl-local-resolver, appsync.graphql-query-list-argument-foreach-sum, appsync.graphql-query-none-datasource-resolver, appsync.pipeline-resolver-function-chain-stash-propagation, appsync.pipeline-resolver-update-reorders-execution.

=== Cause 2 of 3: GetApiAssociation for a domain with no association returns the wrong NotFoundException message ===
AWS: Smithy: GetApiAssociation (GET /v1/domainnames/{domainName}/apiassociation) models NotFoundException (client, HTTP 404) and returns ApiAssociation{domainName, apiId, associationStatus, deploymentDetail}. Recorded behavior: a DomainName that exists but was never linked to an Api is refused with NotFoundException/404 and the message "Domain name is not associated." — that text is entirely fixed; it interpolates no request value.
Emulator: the code is right (NotFoundException) but the message is "API association not found." — a different fixed string.
Why: the message is part of the error contract. A client (or operator log/alert) that distinguishes "this domain has no association" from other 404s by AWS's wording sees an unrecognised string, and test suites or tooling that assert on the documented message fail against the emulator while passing against AWS.
Fix: the GetApiAssociation handler's not-found error path — emit the NotFoundException message AWS words for an unassociated domain instead of the generic "not found" text. NOTE: open issue 20261029T174359Z-appsync-control-plane-error-semantics-not-found-bad.md covers not-found/bad-request error semantics for other checks on this plane and may close this one too.
Traces: appsync.api-association-get-not-found-when-unassociated (AssertionError: expected the exact NotFoundException message AWS returns, got 'API association not found.').

=== Cause 3 of 3: CreateDomainName accepts a certificate whose subject/SANs do not cover the requested domain ===
AWS: Smithy: CreateDomainName (POST /v1/domainnames) takes required domainName (DomainName, length 1-253, pattern ^(\\*[\\w\\d-]*\\.)?([\\w\\d-]+\\.)+[\\w\\d-]+$) and required certificateArn (an acm/iam certificate ARN), and models BadRequestException (client, HTTP 400). Recorded behavior: AppSync resolves the certificate and refuses the pair when the certificate does not cover the requested domain — BadRequestException/400 — and creates NO DomainName resource, so the subsequent GetDomainName answers ('NotFoundException', 'Domain name not found'). The domain label and certificate ARN in any one run are request values, not part of the rule; the rule is "certificate must cover domainName".
Emulator: the mismatched pair is accepted — CreateDomainName returns HTTP 200 with a domainNameConfig (no error at all, the check observed code None), and the phantom DomainName is then readable, so AssociateApi can reach a domain no certificate covers.
Why: a client that mis-wires a certificate gets a silent success and a domain that could never serve TLS on AWS; the validation error AWS uses to tell it the certificate is wrong never arrives, and its error-handling path is never exercised.
Fix: the CreateDomainName handler's certificate validation (resolve certificateArn and check the requested domainName against the certificate's subject/SANs) before the state write, so the refusal happens with no resource created. NOTE: open issue 20261027T212657Z-appsync-custom-domain-names-createdomainname-performs-no-certificate.md describes missing certificate validation on CreateDomainName for other checks; this check was not marked owned by it and may be closed by the same fix.
Traces: appsync.associate-api-domain-mismatched-certificate-rejected (AssertionError: expected BadRequestException registering a domain the certificate does not cover, got None).

## Evidence
Cause 1 — expected: uris.GRAPHQL names a host:port the emulator serves, e.g. http://127.0.0.1:<port>/<path>, and a POST of a GraphQL document to it reaches the API. Actual: uris.GRAPHQL = https://<token>.appsync-api.us-west-2.amazonaws.com/graphql; the egress is refused with simparity.published.PublishedUrlRejected "...its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener" in all ten data-plane checks listed.

Cause 2 — expected: GetApiAssociation on an unassociated domain -> HTTP 404 NotFoundException, Message "Domain name is not associated.". Actual: HTTP 404 NotFoundException, Message "API association not found." (AssertionError: expected the exact NotFoundException message AWS returns, got 'API association not found.').

Cause 3 — expected: CreateDomainName(domainName=<domain the cert does not cover>, certificateArn=<cert for a different domain>) -> HTTP 400 BadRequestException and no resource, so GetDomainName -> ('NotFoundException','Domain name not found'). Actual: CreateDomainName returns HTTP 200 with domainNameConfig (no ClientError raised; error code observed as None) and the domain is subsequently readable.

## Steps to Reproduce
Cause 1: appsync.create_graphql_api(name=..., authenticationType='API_KEY'); read graphqlApi['uris']['GRAPHQL'] (and ['REALTIME']); POST {"query":"{ ... }"} with header x-api-key to that URI. The URI points at *.appsync-api.<region>.amazonaws.com instead of the emulator's own host:port, so the request never reaches the emulator. Same for dns.GRAPHQL/dns.REALTIME and for GetGraphqlApi/ListGraphqlApis projections.

Cause 2: appsync.create_domain_name(domainName=<d>, certificateArn=<acm cert covering d>); then appsync.get_api_association(domainName=<d>) without ever calling AssociateApi. Inspect the ClientError: code is NotFoundException but Message is "API association not found." instead of "Domain name is not associated.".

Cause 3: issue one ACM certificate for domain A; appsync.create_domain_name(domainName=A, certificateArn=cert) (succeeds, positive control); then appsync.create_domain_name(domainName=B, certificateArn=cert) where B is not covered by the certificate. The second call returns 200 instead of BadRequestException, and appsync.get_domain_name(domainName=B) then succeeds instead of answering NotFoundException "Domain name not found".
