Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 804429c0e5267423598a5b7dbc8d0525cc046b43

# An empty NextToken is refused by every listing but ListOrganizationServiceAccessStatus, which documents the opposite rule as shared

_Filed: 2026-10-28T02:37:10Z_
_Operation: GetSites | Class: validation_

> _Note: this issue was identified against the code base at commit `804429c0e5267423598a5b7dbc8d0525cc046b43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`NextToken` is modelled `length 0..2048` with the permissive pattern, and it arrives in the query string, so `?nextToken=` reaches a handler as `Some("")`. `decode_page_token("")` hex-decodes to the empty string, fails `strip_prefix("networkmanager:")` and answers `None`, so every listing that passes the raw value through (GetSites, GetDevices, GetLinks, DescribeGlobalNetworks, GetConnections, GetCustomerGatewayAssociations, GetConnectPeerAssociations, GetTransitGatewayRegistrations, GetNetworkResources, GetNetworkTelemetry, ListAttachments, ListConnectPeers, ListCoreNetworkPolicyVersions, ListCoreNetworkRoutingInformation, ListCoreNetworkPrefixListAssociations, ListAttachmentRoutingPolicyAssociations, GetCoreNetworkChangeSet, GetCoreNetworkChangeEvents, ...) answers 400 ValidationException "NextToken is not a token this service issued".

One listing filters the empty string out first and serves the first page instead — and its comment asserts that it refuses an unminted token "the same way every other paginated listing of this service refuses it", which is exactly what it does not do for this input. One of the two behaviours is wrong; the crate cannot have both.

## Evidence

The outlier, with the rule it states — src/ops/list_organization_service_access_status.rs:149-165:
```
// A token that is not one this service minted is refused before anything is
// reported, the same way every other paginated listing of this service refuses it.
// An EMPTY `NextToken` is not such a token: the member is modelled `length 0..2048`,
// so an empty string satisfies the schema, and the only page it can name is the
// first one — it is read as "no token" rather than refused.
let after = match input.next_token.as_deref().filter(|token| !token.is_empty()) {
```
A sibling that refuses it — src/ops/get_sites.rs:161-173:
```
let after = match input.next_token.as_deref() {
    Some(token) => Some(crate::state::decode_page_token(token).ok_or_else(|| {
        GetSitesErrorModeled::ValidationException(ValidationException {
            message: "NextToken is not a token this service issued".to_string(), ...
```
and the decoder that makes `""` unparseable — src/state.rs:8000-8003:
```
pub fn decode_page_token(token: &str) -> Option<String> {
    let text = String::from_utf8(hex::decode(token).ok()?).ok()?;
    text.strip_prefix(PAGE_TOKEN_PREFIX).map(str::to_string)
}
```
(`PAGE_TOKEN_PREFIX = "networkmanager:"`, src/state.rs:7986).

## Steps to Reproduce

1. CreateGlobalNetwork
2. GET /global-networks/{id}/sites?nextToken= → 400 ValidationException "NextToken is not a token this service issued"
3. GET /organizations/service-access?nextToken= → 200, first page
4. Both members are the same modelled `NextToken` (length 0..2048) carried in the same way, so the two answers for the same input cannot both be right
