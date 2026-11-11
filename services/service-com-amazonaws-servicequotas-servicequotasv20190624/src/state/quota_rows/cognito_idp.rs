// SPDX-License-Identifier: CC0-1.0

//! Amazon Cognito User Pools' published default-quota catalog.

use super::*;

/// The `AWS/Usage` display name Amazon Cognito User Pools publishes its call counts
/// under, which is the `Service` dimension of every metered `cognito-idp` quota.
const COGNITO_USER_POOL_USAGE_SERVICE: &str = "Cognito User Pool";

/// The plane suffix of a category rate served for ONE USER POOL: none, because the
/// quota's name states the category alone.
const PER_USER_POOL: &str = "";

/// The plane suffix AWS adds to the name of a category rate it serves for the WHOLE
/// ACCOUNT rather than for one user pool.
const PER_ACCOUNT: &str = " per account";

/// One Amazon Cognito User Pools CATEGORY request-rate quota: the combined calls per
/// second served for every API operation of one category, on the plane AWS serves that
/// category's rate on ([`PER_USER_POOL`] or [`PER_ACCOUNT`], which the quota's own name
/// states).
///
/// Cognito User Pools throttles its API by CATEGORY rather than per operation, so the
/// quota's name, window, usage metric and description are all fixed text around the
/// category's own name, and whether AWS lets the rate be provisioned decides both the
/// `Adjustable` flag and which of the two sentences the row carries. One constructor
/// fills the class so two rows of it cannot drift apart.
///
/// SOURCE: AWS's published Service Quotas data for the `cognito-idp` service code (the
/// "Quotas in Amazon Cognito" chapter of the Amazon Cognito Developer Guide, whose
/// category table is what that data publishes), and the recorded default-quota listing of
/// that code for which `QuotaCode` names which category on which plane, for the
/// one-second window and the `AWS/Usage` `CallCount` metric of the category, and for both
/// sentences.
fn cognito_user_pool_category_rate(
    code: &str,
    category: &str,
    name_plane: &str,
    requests_per_second: f64,
    adjustable: bool,
) -> QuotaRow {
    // The two sentences differ in ONE fact: whether the rate can be provisioned within
    // the quota, which is the same fact `Adjustable` reports.
    let description = if adjustable {
        format!(
            "The maximum combined call rate (requests per second) you can request for all API \
             operations in the {category} category. Provision your rate limit within this quota \
             from the Amazon Cognito console or APIs. For the list of included operations, see \
             https://docs.aws.amazon.com/cognito/latest/developerguide/limits.html#category_operations."
        )
    } else {
        format!(
            "The maximum combined call rate (requests per second) for all API operations in the \
             {category} category. All operations within a category share the same quota. For the \
             list of included operations, see \
             https://docs.aws.amazon.com/cognito/latest/developerguide/limits.html#category_operations."
        )
    };
    let row = quota(
        format!("Rate of {category} requests{name_plane}"),
        requests_per_second,
    )
    .code(code)
    .period(1, "SECOND")
    .metric(call_count_metric(COGNITO_USER_POOL_USAGE_SERVICE, category))
    .describe(&description);
    if adjustable {
        row
    } else {
        row.fixed()
    }
}

/// Amazon Cognito User Pools' published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `cognito-idp` service code, and
/// the recorded default-quota listing of that code for which `QuotaCode` names which
/// category's rate, on which plane, at how many requests per second and whether that rate
/// can be provisioned ([`cognito_user_pool_category_rate`]). The remaining codes that
/// listing reports name categories no source available here states, and a code identifies
/// ONE quota, so those rows are left to the default-quota-set gap rather than published
/// under a binding this catalog would be guessing at (see
/// [`crate::state::descriptive_quota_code`]).
pub(in crate::state) fn cognito_idp_quotas() -> Vec<QuotaRow> {
    vec![
        cognito_user_pool_category_rate("L-5987B8A0", "UserCreation", PER_USER_POOL, 50.0, true),
        cognito_user_pool_category_rate(
            "L-7D6E8ED3",
            "UserAccountRecovery",
            PER_USER_POOL,
            30.0,
            false,
        ),
        cognito_user_pool_category_rate(
            "L-B7575496",
            "UserPoolResourceUpdate",
            PER_ACCOUNT,
            15.0,
            false,
        ),
        cognito_user_pool_category_rate("L-D6BD5178", "UserRead", PER_USER_POOL, 120.0, true),
    ]
}
