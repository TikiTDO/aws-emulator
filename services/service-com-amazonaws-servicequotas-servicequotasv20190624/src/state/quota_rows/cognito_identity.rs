// SPDX-License-Identifier: CC0-1.0

//! Amazon Cognito Federated Identities' published default-quota catalog.

use super::*;

/// One Amazon Cognito Federated Identities API REQUEST-RATE quota: the requests per
/// second that identity-pool operation is served at.
///
/// Every member of such a row but the operation's own name and the value is the same
/// across the class — the `QuotaName` is `Rate of <Api> requests` and the `Description`
/// is the sentence the service publishes for the class with the operation named in it —
/// so one constructor fills them and two rows of the class cannot drift apart.
///
/// SOURCE: AWS's published Service Quotas data for the `cognito-identity` service code,
/// and the recorded default-quota listing of that code for which `QuotaCode` names which
/// operation's rate and for the wording. The rate's window is stated in the quota's own
/// name and description, and the rows of this service carry NO `Period` or `UsageMetric`
/// block, which is why none is set here.
fn cognito_identity_request_rate(code: &str, api_name: &str, requests_per_second: f64) -> QuotaRow {
    quota(format!("Rate of {api_name} requests"), requests_per_second)
        .code(code)
        .describe(&format!(
            "The maximum number of {api_name} requests that you can make per second."
        ))
}

/// Amazon Cognito Federated Identities' published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `cognito-identity` service code,
/// and the recorded default-quota listing of that code for which `QuotaCode` names which
/// operation's rate and at how many requests per second it is served
/// ([`cognito_identity_request_rate`]): this service publishes a request-rate quota per
/// identity-pool API operation. The remaining codes that listing reports name operations
/// no source available here states, and a code identifies ONE quota, so those rows are
/// left to the default-quota-set gap rather than published under a binding this catalog
/// would be guessing at (see [`crate::state::descriptive_quota_code`]).
pub(in crate::state) fn cognito_identity_quotas() -> Vec<QuotaRow> {
    vec![
        cognito_identity_request_rate("L-0BF99E8D", "GetOpenIdTokenForDeveloperIdentity", 50.0),
        cognito_identity_request_rate("L-0D31D6D2", "UntagResource", 5.0),
        cognito_identity_request_rate("L-50C9AAB8", "ListTagsForResource", 10.0),
        cognito_identity_request_rate("L-5AE9F13D", "GetCredentialsForIdentity", 200.0),
        cognito_identity_request_rate("L-A38ADA84", "GetId", 25.0),
        cognito_identity_request_rate("L-DAF3EAFF", "TagResource", 5.0),
        cognito_identity_request_rate("L-DBF99350", "ListIdentities", 5.0),
        cognito_identity_request_rate("L-E97D135E", "GetOpenIdToken", 200.0),
    ]
}
