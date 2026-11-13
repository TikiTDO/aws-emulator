// SPDX-License-Identifier: CC0-1.0

//! The ACCOUNT SETTINGS surface, driven end to end over the shared wire harness in
//! [`crate::ecs_wire`]: `PutAccountSetting`/`PutAccountSettingDefault` write, and
//! `ListAccountSettings` and a cluster read answer from what was written.
//!
//! The rules under test:
//!
//! * `ListAccountSettings` with `effectiveSettings: true` reports, for EVERY setting
//!   name the model defines, the value in effect for the principal: that principal's own
//!   value when it has one, otherwise the account-wide default, otherwise the setting's
//!   documented default ("If `true`, the account settings for the root user or the
//!   default setting for the `principalArn` are returned").
//! * With `effectiveSettings: false` only the names that principal has WRITTEN are
//!   reported ("If `false`, the account settings for the `principalArn` are returned if
//!   they're set. Otherwise, no account settings are returned").
//! * `PutAccountSettingDefault` "modifies an account setting for all users on an account
//!   for whom no individual account setting has been specified", so it is the root
//!   user's setting and an individual `PutAccountSetting` shadows it for that principal
//!   alone.
//! * `Setting$type` is a property of the NAME: `aws_managed` for the one setting Amazon
//!   Web Services manages, `user` for the rest — and an `aws_managed` setting is
//!   read-only, so a write of it is refused.
//! * `DeleteAccountSetting` "disables an account setting for a specified user, role, or
//!   the root user for an account": the principal's stored value is removed, so the
//!   value in effect for it falls back to the account-wide default, and the response
//!   reports the setting that stopped applying. A principal that holds no setting of that
//!   name has nothing to disable, so the call is refused rather than answered with what is
//!   already in effect.
//! * A `principalArn` that names a principal OTHER than the caller's own is reached by the
//!   root user alone, whichever of these operations names it: every other caller reads and
//!   writes its own settings, and the account-wide default is written through
//!   `PutAccountSettingDefault`, which names no principal.
//! * `ClusterSetting$value`: "If a cluster value is specified, it will override the
//!   `containerInsights` value set with PutAccountSetting or PutAccountSettingDefault" —
//!   so a cluster that set nothing reports the account setting in effect.
//! * `ListAccountSettings$maxResults`: "This value can be between 1 and 10. If this
//!   parameter isn't used, then ListAccountSettings returns up to 10 results and a
//!   nextToken value if applicable."
//! * `ListAccountSettings$value`: "You must also specify an account setting name to use
//!   this parameter."

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{list, ok, refusal, refused, service, string, ACCOUNT};

/// Every Amazon ECS account setting, from
/// `smithy_query(query="shape", shape="SettingName")` -> values, with the value it has
/// for an account that never wrote it and its `Setting$type`
/// (`smithy_query(query="shape", shape="Setting")` -> `type`; the `aws_managed` one is
/// "the `guardDutyActivate` account setting ... the only one Amazon Web Services
/// manages").
///
/// The defaults are the ones the model's own `PutAccountSetting$name`/`$value` docs
/// state: the ARN-and-id-format settings are on (the format every ARN this emulator
/// emits and the resource tagging it supports require it), `defaultLogDriverMode` is
/// `non-blocking` ("On June 25, 2025, Amazon ECS changed the default log driver mode
/// from `blocking` to `non-blocking`"), `fargateTaskRetirementWaitPeriod` is the 7
/// calendar days Fargate task maintenance waits, and every other opt-in feature is off
/// until a write turns it on.
const ACCOUNT_SETTINGS: [(&str, &str, &str); 11] = [
    ("awsvpcTrunking", "disabled", "user"),
    ("containerInsights", "disabled", "user"),
    ("containerInstanceLongArnFormat", "enabled", "user"),
    ("defaultLogDriverMode", "non-blocking", "user"),
    ("fargateEventWindows", "disabled", "user"),
    ("fargateFIPSMode", "disabled", "user"),
    ("fargateTaskRetirementWaitPeriod", "7", "user"),
    ("guardDutyActivate", "disabled", "aws_managed"),
    ("serviceLongArnFormat", "enabled", "user"),
    ("tagResourceAuthorization", "disabled", "user"),
    ("taskLongArnFormat", "enabled", "user"),
];

/// The largest page `ListAccountSettings$maxResults` allows, and the page size it uses
/// when a request gives none.
const MAX_PAGE: usize = 10;

/// The principal every unsigned-identity request is listed for: the one IAM user this
/// emulator authenticates every caller as.
fn authenticated_principal() -> String {
    format!("arn:aws:iam::{ACCOUNT}:user/emulator")
}

/// The root user — the principal whose settings ARE the account-wide defaults.
fn root_principal() -> String {
    format!("arn:aws:iam::{ACCOUNT}:root")
}

/// The sentence an account-setting call that names a principal other than the caller's own
/// is refused with: whose settings a caller may reach is the account's own boundary, and
/// only the root user reaches another principal's.
const ROOT_ONLY_MESSAGE: &str =
    "Only the root user can view or modify the account settings for another user. You do \
     not need to specify a principalArn value to change your own account settings.";

/// Assert that `operation` refuses `body` because it names a principal the caller may not
/// reach — the one refusal every account-setting operation answers such a request with.
async fn refused_as_another_principal(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    case: &str,
) {
    let (code, message) = refusal(router, operation, body, case).await;
    assert_eq!(
        (code.as_str(), message.as_str()),
        ("InvalidParameterException", ROOT_ONLY_MESSAGE),
        "case {case:?}: expected {operation} to refuse another principal's settings, got \
         {code:?} with {message:?}"
    );
}

/// One `ListAccountSettings` page.
async fn page(router: &MultiServiceRouter, request: Value, case: &str) -> Value {
    ok(router, "ListAccountSettings", request, case).await
}

/// Every setting `ListAccountSettings` reports for `request`, walking `nextToken` to the
/// end so a paginated answer is compared whole.
async fn all_settings(router: &MultiServiceRouter, request: Value, case: &str) -> Vec<Value> {
    let mut collected: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    // One request per page, bounded by the catalog size so a token that never clears
    // fails the test instead of looping forever.
    for _ in 0..=ACCOUNT_SETTINGS.len() {
        let mut body = request.clone();
        if let Some(token) = &token {
            body["nextToken"] = json!(token);
        }
        let answer = page(router, body, case).await;
        collected.extend(list(&answer, "settings", case));
        token = answer["nextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return collected;
        }
    }
    panic!("case {case:?}: ListAccountSettings never stopped paging, got {collected:?}");
}

/// The `name` -> `value` pairs of a reported settings list.
fn values(settings: &[Value]) -> Vec<(String, String)> {
    settings
        .iter()
        .map(|setting| {
            (
                setting["name"].as_str().unwrap_or_default().to_string(),
                setting["value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The one setting `ListAccountSettings` reports for `name`, effective or not.
async fn effective(router: &MultiServiceRouter, name: &str, case: &str) -> Value {
    let settings = all_settings(
        router,
        json!({ "name": name, "effectiveSettings": true }),
        case,
    )
    .await;
    assert_eq!(
        settings.len(),
        1,
        "case {case:?}: one name has exactly one effective setting, got {settings:?}"
    );
    settings[0].clone()
}

#[tokio::test]
async fn effective_settings_report_every_modelled_name_with_its_documented_default() {
    let router = service();
    let case = "untouched account";

    let reported = all_settings(&router, json!({ "effectiveSettings": true }), case).await;

    let expected: Vec<(String, String)> = ACCOUNT_SETTINGS
        .iter()
        .map(|(name, value, _)| ((*name).to_string(), (*value).to_string()))
        .collect();
    assert_eq!(
        values(&reported),
        expected,
        "case {case:?}: every account setting the model defines must be reported with \
         its documented default, got {reported:?}"
    );
    for (name, _, setting_type) in ACCOUNT_SETTINGS {
        let setting = reported
            .iter()
            .find(|setting| setting["name"] == json!(name))
            .unwrap_or_else(|| panic!("case {case:?}: {name} must be reported"));
        assert_eq!(
            setting["type"],
            json!(setting_type),
            "case {case:?}: {name} is managed by {setting_type}, got {setting}"
        );
        assert_eq!(
            setting["principalArn"],
            json!(root_principal()),
            "case {case:?}: a value nobody overrode comes from the account, so {name} \
             must name the root user, got {setting}"
        );
    }
}

#[tokio::test]
async fn a_read_of_the_account_settings_changes_nothing_a_later_read_sees() {
    let router = service();
    let case = "repeated read";

    let first = all_settings(&router, json!({ "effectiveSettings": true }), case).await;
    let second = all_settings(&router, json!({ "effectiveSettings": true }), case).await;
    assert_eq!(
        first, second,
        "case {case:?}: ListAccountSettings is a read, so two calls in a row must answer \
         the same thing, got {first:?} then {second:?}"
    );

    let unset = all_settings(&router, json!({ "effectiveSettings": false }), case).await;
    assert!(
        unset.is_empty(),
        "case {case:?}: no principal wrote a setting, so a non-effective listing must \
         report none, got {unset:?}"
    );
}

#[tokio::test]
async fn effective_settings_paginate_at_ten_results() {
    let router = service();

    // The rows differ along the page size: absent (the documented default of 10), each
    // published bound (1 and 10), and one inside the range.
    let cases: [(&str, Value, usize); 4] = [
        ("no maxResults", json!({}), MAX_PAGE),
        ("maxResults 1", json!({ "maxResults": 1 }), 1),
        ("maxResults 4", json!({ "maxResults": 4 }), 4),
        ("maxResults 10", json!({ "maxResults": 10 }), MAX_PAGE),
    ];

    for (case, extra, expected_page) in cases {
        let mut request = json!({ "effectiveSettings": true });
        for (member, value) in extra.as_object().expect("an object of extra members") {
            request[member] = value.clone();
        }
        let first = page(&router, request.clone(), case).await;
        let first_page = list(&first, "settings", case);
        assert_eq!(
            first_page.len(),
            expected_page,
            "case {case:?}: expected a page of {expected_page} settings, got {first_page:?}"
        );
        let token = string(&first, "nextToken", case);
        assert_eq!(
            token,
            ACCOUNT_SETTINGS[expected_page].0,
            "case {case:?}: the token must name the setting the next page starts at, got \
             {token:?}"
        );

        let walked = all_settings(&router, request, case).await;
        let expected: Vec<String> = ACCOUNT_SETTINGS
            .iter()
            .map(|(name, _, _)| (*name).to_string())
            .collect();
        let names: Vec<String> = values(&walked).into_iter().map(|(name, _)| name).collect();
        assert_eq!(
            names, expected,
            "case {case:?}: walking every page must report each name exactly once, in \
             order, got {names:?}"
        );
    }
}

#[tokio::test]
async fn a_written_setting_is_what_a_later_list_reports() {
    let router = service();

    // One row per VALUE FORM the model documents: the enabled/disabled toggle, the
    // on/off spelling of the same toggle, the three-valued `containerInsights`, the
    // two-worded `defaultLogDriverMode`, and the numeric Fargate wait period.
    let cases: [(&str, &str, &str); 7] = [
        ("trunking on", "awsvpcTrunking", "enabled"),
        ("tag authorization on", "tagResourceAuthorization", "on"),
        ("tag authorization off", "tagResourceAuthorization", "off"),
        ("insights enhanced", "containerInsights", "enhanced"),
        ("log driver blocking", "defaultLogDriverMode", "blocking"),
        ("retire immediately", "fargateTaskRetirementWaitPeriod", "0"),
        ("retire in a fortnight", "fargateTaskRetirementWaitPeriod", "14"),
    ];

    for (case, name, value) in cases {
        let written = ok(
            &router,
            "PutAccountSetting",
            json!({ "name": name, "value": value }),
            case,
        )
        .await;
        let setting = &written["setting"];
        assert_eq!(
            (setting["name"].clone(), setting["value"].clone()),
            (json!(name), json!(value)),
            "case {case:?}: the write must report the setting it stored, got {setting}"
        );
        assert_eq!(
            setting["principalArn"],
            json!(authenticated_principal()),
            "case {case:?}: a write that names no principal is the authenticated user's, \
             got {setting}"
        );

        // The round trip: the stored value is what both listings answer with.
        let explicit = all_settings(
            &router,
            json!({ "name": name, "effectiveSettings": false }),
            case,
        )
        .await;
        assert_eq!(
            values(&explicit),
            vec![(name.to_string(), value.to_string())],
            "case {case:?}: a non-effective listing must report the setting this \
             principal wrote, got {explicit:?}"
        );
        let in_effect = effective(&router, name, case).await;
        assert_eq!(
            in_effect["value"],
            json!(value),
            "case {case:?}: the written value is the one in effect, got {in_effect}"
        );
        assert_eq!(
            in_effect["principalArn"],
            json!(authenticated_principal()),
            "case {case:?}: the value in effect comes from this principal, got {in_effect}"
        );

        // The filter the same listing offers: a value that is not the stored one matches
        // nothing, the stored one matches this setting.
        let filtered = all_settings(
            &router,
            json!({ "name": name, "effectiveSettings": true, "value": value }),
            case,
        )
        .await;
        assert_eq!(
            values(&filtered),
            vec![(name.to_string(), value.to_string())],
            "case {case:?}: filtering on the stored value must match it, got {filtered:?}"
        );
    }

    // Every write landed in the same store, so the non-effective listing reports them
    // all — one entry per NAME written, with the last value written for it.
    let all = all_settings(&router, json!({ "effectiveSettings": false }), "every write").await;
    assert_eq!(
        values(&all),
        vec![
            ("awsvpcTrunking".to_string(), "enabled".to_string()),
            ("containerInsights".to_string(), "enhanced".to_string()),
            ("defaultLogDriverMode".to_string(), "blocking".to_string()),
            (
                "fargateTaskRetirementWaitPeriod".to_string(),
                "14".to_string()
            ),
            ("tagResourceAuthorization".to_string(), "off".to_string()),
        ],
        "case \"every write\": a non-effective listing must report exactly the names \
         this principal wrote, got {all:?}"
    );
}

#[tokio::test]
async fn an_account_default_is_the_root_users_setting_and_a_principals_own_shadows_it() {
    let router = service();

    // The rows differ along the setting and the two values: what the account default is
    // set to, and what the one principal overrides it with.
    let cases: [(&str, &str, &str, &str); 3] = [
        ("insights", "containerInsights", "enabled", "enhanced"),
        ("trunking", "awsvpcTrunking", "enabled", "disabled"),
        (
            "retirement",
            "fargateTaskRetirementWaitPeriod",
            "14",
            "0",
        ),
    ];

    for (case, name, default_value, principal_value) in cases {
        let written = ok(
            &router,
            "PutAccountSettingDefault",
            json!({ "name": name, "value": default_value }),
            case,
        )
        .await;
        let setting = &written["setting"];
        assert_eq!(
            (setting["name"].clone(), setting["value"].clone()),
            (json!(name), json!(default_value)),
            "case {case:?}: the write must report the default it stored, got {setting}"
        );
        assert_eq!(
            setting["principalArn"],
            json!(root_principal()),
            "case {case:?}: an account-wide default is the root user's setting, got \
             {setting}"
        );

        // The default is what every principal with no setting of its own gets.
        let in_effect = effective(&router, name, case).await;
        assert_eq!(
            (
                in_effect["value"].clone(),
                in_effect["principalArn"].clone()
            ),
            (json!(default_value), json!(root_principal())),
            "case {case:?}: the account default is in effect for the authenticated user, \
             got {in_effect}"
        );
        // ... and it is the ROOT user's own setting, which only the root user may read:
        // this caller is a user, so a listing that names the root user's principal is
        // refused rather than answered.
        refused_as_another_principal(
            &router,
            "ListAccountSettings",
            json!({ "name": name, "principalArn": root_principal(), "effectiveSettings": false }),
            case,
        )
        .await;
        // The authenticated user wrote nothing, so a non-effective listing for it
        // reports nothing of this name.
        let own_explicit = all_settings(
            &router,
            json!({ "name": name, "effectiveSettings": false }),
            case,
        )
        .await;
        assert!(
            own_explicit.is_empty(),
            "case {case:?}: a principal that wrote nothing has no setting of its own, \
             got {own_explicit:?}"
        );

        // An individual write shadows the default for that principal alone.
        ok(
            &router,
            "PutAccountSetting",
            json!({ "name": name, "value": principal_value }),
            case,
        )
        .await;
        let shadowed = effective(&router, name, case).await;
        assert_eq!(
            (shadowed["value"].clone(), shadowed["principalArn"].clone()),
            (
                json!(principal_value),
                json!(authenticated_principal())
            ),
            "case {case:?}: a principal's own setting overrides the account default, got \
             {shadowed}"
        );
        // ... and taking that write away leaves the account default where it was, which is
        // what the principal falls back to.
        ok(
            &router,
            "DeleteAccountSetting",
            json!({ "name": name }),
            case,
        )
        .await;
        let fell_back = effective(&router, name, case).await;
        assert_eq!(
            (
                fell_back["value"].clone(),
                fell_back["principalArn"].clone()
            ),
            (json!(default_value), json!(root_principal())),
            "case {case:?}: one principal's write must leave the account default where it \
             was, got {fell_back}"
        );
    }
}

#[tokio::test]
async fn a_cluster_that_sets_nothing_inherits_the_account_setting() {
    // The rows differ along where `containerInsights` comes from and what it is: the
    // untouched default, each value an account default can hold, and a cluster that
    // overrides the account with one of its own.
    let cases: [(&str, Option<&str>, Option<&str>, &str); 5] = [
        ("nothing set", None, None, "disabled"),
        ("account enabled", Some("enabled"), None, "enabled"),
        ("account enhanced", Some("enhanced"), None, "enhanced"),
        ("cluster overrides", Some("enabled"), Some("disabled"), "disabled"),
        ("cluster enhances", Some("disabled"), Some("enhanced"), "enhanced"),
    ];

    for (index, (case, account_value, cluster_value, expected)) in cases.iter().enumerate() {
        // A fresh engine per row, so one row's account setting cannot leak into another.
        let router = service();
        if let Some(value) = account_value {
            ok(
                &router,
                "PutAccountSettingDefault",
                json!({ "name": "containerInsights", "value": value }),
                case,
            )
            .await;
        }
        let cluster_name = format!("inherit-{index}");
        let mut request = json!({ "clusterName": cluster_name });
        if let Some(value) = cluster_value {
            request["settings"] = json!([{ "name": "containerInsights", "value": value }]);
        }
        let created = ok(&router, "CreateCluster", request, case).await;
        assert_eq!(
            list(&created["cluster"], "settings", case),
            vec![json!({ "name": "containerInsights", "value": expected })],
            "case {case:?}: a created cluster must report the setting in effect for it, \
             got {created}"
        );

        // The same answer on a later READ of the stored cluster.
        let described = ok(
            &router,
            "DescribeClusters",
            json!({ "clusters": [cluster_name], "include": ["SETTINGS"] }),
            case,
        )
        .await;
        let clusters = list(&described, "clusters", case);
        assert_eq!(
            list(&clusters[0], "settings", case),
            vec![json!({ "name": "containerInsights", "value": expected })],
            "case {case:?}: a described cluster must report the setting in effect for it, \
             got {described}"
        );
    }
}

#[tokio::test]
async fn an_account_setting_amazon_manages_is_read_only() {
    let router = service();

    // `guardDutyActivate` is the one `aws_managed` setting, and both writes refuse it.
    for operation in ["PutAccountSetting", "PutAccountSettingDefault"] {
        let case = format!("{operation} on guardDutyActivate");
        refused(
            &router,
            operation,
            json!({ "name": "guardDutyActivate", "value": "enabled" }),
            "InvalidParameterException",
            &case,
        )
        .await;
    }

    // The refusal stored nothing: the setting is still the value Amazon Web Services
    // reports for it.
    let setting = effective(&router, "guardDutyActivate", "after the refusals").await;
    assert_eq!(
        (setting["value"].clone(), setting["type"].clone()),
        (json!("disabled"), json!("aws_managed")),
        "case \"after the refusals\": a read-only setting must keep its value, got \
         {setting}"
    );
}

#[tokio::test]
async fn a_request_outside_the_documented_parameters_is_refused() {
    let router = service();

    // The rows span the documented constraints: the values a NAME accepts (a value from
    // another setting's domain, a wait period outside 0/7/14), the principal a setting
    // may belong to, the `maxResults` range (one past each published bound) and the
    // `value` filter's dependency on `name`.
    let cases: [(&str, &str, Value); 10] = [
        (
            "a log driver mode is not an insights value",
            "PutAccountSetting",
            json!({ "name": "containerInsights", "value": "non-blocking" }),
        ),
        (
            "an insights value is not a log driver mode",
            "PutAccountSettingDefault",
            json!({ "name": "defaultLogDriverMode", "value": "enabled" }),
        ),
        (
            "an undocumented wait period",
            "PutAccountSettingDefault",
            json!({ "name": "fargateTaskRetirementWaitPeriod", "value": "3" }),
        ),
        (
            "a principal that is not an ARN",
            "PutAccountSetting",
            json!({ "name": "awsvpcTrunking", "value": "enabled", "principalArn": "emulator" }),
        ),
        (
            "listing for a principal that is not an ARN",
            "ListAccountSettings",
            json!({ "principalArn": "emulator", "effectiveSettings": true }),
        ),
        (
            "maxResults below the range",
            "ListAccountSettings",
            json!({ "effectiveSettings": true, "maxResults": -1 }),
        ),
        (
            "maxResults above the range",
            "ListAccountSettings",
            json!({ "effectiveSettings": true, "maxResults": 11 }),
        ),
        (
            "a value filter with no name",
            "ListAccountSettings",
            json!({ "effectiveSettings": true, "value": "disabled" }),
        ),
        (
            "a token this operation never minted",
            "ListAccountSettings",
            json!({ "effectiveSettings": true, "nextToken": "zzz" }),
        ),
        (
            "a token that is an empty string",
            "ListAccountSettings",
            json!({ "effectiveSettings": true, "nextToken": "" }),
        ),
    ];

    for (case, operation, request) in cases {
        refused(
            &router,
            operation,
            request,
            "InvalidParameterException",
            case,
        )
        .await;
    }

    // None of the refusals wrote anything: every setting is still its default.
    let reported = all_settings(
        &router,
        json!({ "effectiveSettings": true }),
        "after the refusals",
    )
    .await;
    let expected: Vec<(String, String)> = ACCOUNT_SETTINGS
        .iter()
        .map(|(name, value, _)| ((*name).to_string(), (*value).to_string()))
        .collect();
    assert_eq!(
        values(&reported),
        expected,
        "case \"after the refusals\": a refused write must store nothing, got {reported:?}"
    );
}

#[tokio::test]
async fn a_principal_an_account_setting_may_belong_to_is_an_iam_arn() {
    // `PutAccountSetting$principalArn`: "The ARN of the principal, which can be a user,
    // role, or the root user" — an IAM ARN, `arn:<partition>:iam::<account>:<resource>`.
    // The rows differ along the FORM of the value: the caller's OWN principal, which is the
    // one every caller but the root user may write; the other principal kinds AWS mints, a
    // partition other than `aws`, a resource with a path and a principal of another account,
    // each a well-formed ARN whose settings only the root user reaches; and the fields that
    // make a value no principal ARN at all, plus a key longer than the emulator stores.
    let own = authenticated_principal();
    let another_principal: [(&str, String); 4] = [
        ("the root user", root_principal()),
        ("a role", format!("arn:aws:iam::{ACCOUNT}:role/deployer")),
        (
            "a user under a path",
            format!("arn:aws-us-gov:iam::{ACCOUNT}:user/team/lead"),
        ),
        (
            "a user of another account",
            "arn:aws:iam::111122223333:user/outsider".to_string(),
        ),
    ];
    let refused_arns: [(&str, String); 6] = [
        ("not an ARN at all", "emulator".to_string()),
        ("empty fields", "arn::iam:".to_string()),
        ("no resource", format!("arn:aws:iam::{ACCOUNT}:")),
        (
            "a region on a global service",
            format!("arn:aws:iam:us-west-2:{ACCOUNT}:user/emulator"),
        ),
        (
            "another service entirely",
            format!("arn:aws:ecs::{ACCOUNT}:cluster/default"),
        ),
        (
            "a key longer than the store holds",
            format!("arn:aws:iam::{ACCOUNT}:user/{}", "p".repeat(2048)),
        ),
    ];

    {
        let case = "the caller's own principal";
        let router = service();
        let written = ok(
            &router,
            "PutAccountSetting",
            json!({
                "name": "awsvpcTrunking",
                "value": "enabled",
                "principalArn": own,
            }),
            case,
        )
        .await;
        assert_eq!(
            written["setting"]["principalArn"],
            json!(own),
            "case {case:?}: the write must report the principal it stored the setting \
             for, got {written}"
        );
        let stored = all_settings(
            &router,
            json!({
                "name": "awsvpcTrunking",
                "principalArn": own,
                "effectiveSettings": false,
            }),
            case,
        )
        .await;
        assert_eq!(
            values(&stored),
            vec![("awsvpcTrunking".to_string(), "enabled".to_string())],
            "case {case:?}: a later listing for that principal must report the stored \
             setting, got {stored:?}"
        );
    }

    let router = service();
    for (case, principal_arn) in another_principal {
        refused_as_another_principal(
            &router,
            "PutAccountSetting",
            json!({
                "name": "awsvpcTrunking",
                "value": "enabled",
                "principalArn": principal_arn,
            }),
            case,
        )
        .await;
        refused_as_another_principal(
            &router,
            "ListAccountSettings",
            json!({ "principalArn": principal_arn, "effectiveSettings": true }),
            case,
        )
        .await;
        refused_as_another_principal(
            &router,
            "DeleteAccountSetting",
            json!({ "name": "awsvpcTrunking", "principalArn": principal_arn }),
            case,
        )
        .await;
    }

    for (case, principal_arn) in refused_arns {
        refused(
            &router,
            "PutAccountSetting",
            json!({
                "name": "awsvpcTrunking",
                "value": "enabled",
                "principalArn": principal_arn,
            }),
            "InvalidParameterException",
            case,
        )
        .await;
        refused(
            &router,
            "ListAccountSettings",
            json!({ "principalArn": principal_arn, "effectiveSettings": true }),
            "InvalidParameterException",
            case,
        )
        .await;
    }
}

#[tokio::test]
async fn the_store_holds_the_callers_own_settings_and_the_account_default() {
    let router = service();
    // Whose settings a caller may write is the account's boundary: its OWN principal, and
    // the account-wide default the root user holds (written through
    // `PutAccountSettingDefault`, which names no principal). So the store grows with those
    // two and with nothing else, whatever principal a request names — which is also what
    // keeps a caller from growing the in-memory store without limit
    // (`state::MAX_ACCOUNT_SETTING_PRINCIPALS`).
    let case = "the caller's own principal and the account default";
    for (operation, body) in [
        (
            "PutAccountSetting",
            json!({ "name": "awsvpcTrunking", "value": "enabled" }),
        ),
        (
            "PutAccountSetting",
            json!({ "name": "containerInsights", "value": "enabled" }),
        ),
        (
            "PutAccountSettingDefault",
            json!({ "name": "awsvpcTrunking", "value": "disabled" }),
        ),
    ] {
        ok(&router, operation, body, case).await;
    }

    let own = all_settings(&router, json!({ "effectiveSettings": false }), case).await;
    assert_eq!(
        values(&own),
        vec![
            ("awsvpcTrunking".to_string(), "enabled".to_string()),
            ("containerInsights".to_string(), "enabled".to_string()),
        ],
        "case {case:?}: the caller holds the settings it wrote for itself, got {own:?}"
    );

    // Any other principal is refused, so nothing is stored for it — one row per principal
    // kind a request can name.
    for (row, principal_arn) in [
        ("the root user", root_principal()),
        ("a role", format!("arn:aws:iam::{ACCOUNT}:role/deployer")),
        ("another user", format!("arn:aws:iam::{ACCOUNT}:user/other")),
    ] {
        refused_as_another_principal(
            &router,
            "PutAccountSetting",
            json!({
                "name": "awsvpcTrunking",
                "value": "enabled",
                "principalArn": principal_arn,
            }),
            row,
        )
        .await;
    }

    // The refusals stored nothing and left what the caller wrote alone.
    let after = all_settings(&router, json!({ "effectiveSettings": false }), case).await;
    assert_eq!(
        values(&after),
        values(&own),
        "case {case:?}: a refused write must store nothing, got {after:?}"
    );
}

/// Disable one account setting and return the `Setting` the call reports.
async fn disable(router: &MultiServiceRouter, request: Value, case: &str) -> Value {
    ok(router, "DeleteAccountSetting", request, case)
        .await["setting"]
        .clone()
}

/// The documented default of `name`, from this suite's catalog.
fn documented_default(name: &str) -> String {
    ACCOUNT_SETTINGS
        .iter()
        .find(|(setting, _, _)| *setting == name)
        .map(|(_, value, _)| (*value).to_string())
        .unwrap_or_else(|| panic!("{name} is one of this suite's account settings"))
}

#[tokio::test]
async fn a_disabled_setting_stops_applying_and_the_account_default_takes_over() {
    // The rows differ along the setting and the two value FORMS it is written with: the
    // enabled/disabled toggle, its on/off spelling, the three-valued `containerInsights`,
    // the two-worded `defaultLogDriverMode` and the numeric Fargate wait period.
    let cases: [(&str, &str, &str, &str); 5] = [
        ("trunking", "awsvpcTrunking", "enabled", "disabled"),
        ("insights", "containerInsights", "enabled", "enhanced"),
        ("tag authorization", "tagResourceAuthorization", "on", "off"),
        (
            "log driver",
            "defaultLogDriverMode",
            "blocking",
            "non-blocking",
        ),
        (
            "retirement",
            "fargateTaskRetirementWaitPeriod",
            "14",
            "0",
        ),
    ];

    for (case, name, account_value, principal_value) in cases {
        // A fresh engine per row, so one row's writes cannot leak into another.
        let router = service();
        ok(
            &router,
            "PutAccountSettingDefault",
            json!({ "name": name, "value": account_value }),
            case,
        )
        .await;
        ok(
            &router,
            "PutAccountSetting",
            json!({ "name": name, "value": principal_value }),
            case,
        )
        .await;

        // The disable reports the setting that stopped applying to this principal.
        let disabled = disable(&router, json!({ "name": name }), case).await;
        assert_eq!(
            (
                disabled["name"].clone(),
                disabled["value"].clone(),
                disabled["principalArn"].clone()
            ),
            (
                json!(name),
                json!(principal_value),
                json!(authenticated_principal())
            ),
            "case {case:?}: the disable must report the setting it cleared for this \
             principal, got {disabled}"
        );

        // ... and the store no longer holds it, so the account default applies again.
        let own = all_settings(
            &router,
            json!({ "name": name, "effectiveSettings": false }),
            case,
        )
        .await;
        assert!(
            own.is_empty(),
            "case {case:?}: a disabled setting is no longer this principal's own, got \
             {own:?}"
        );
        let in_effect = effective(&router, name, case).await;
        assert_eq!(
            (
                in_effect["value"].clone(),
                in_effect["principalArn"].clone()
            ),
            (json!(account_value), json!(root_principal())),
            "case {case:?}: after the disable the account default is in effect, got \
             {in_effect}"
        );

        // Disabling the ROOT user's setting is disabling the account-wide default, which
        // is the root user's own call: this caller is a user, so it is refused and the
        // default stays in effect.
        refused_as_another_principal(
            &router,
            "DeleteAccountSetting",
            json!({ "name": name, "principalArn": root_principal() }),
            case,
        )
        .await;
        let after_root = effective(&router, name, case).await;
        assert_eq!(
            (
                after_root["value"].clone(),
                after_root["principalArn"].clone()
            ),
            (json!(account_value), json!(root_principal())),
            "case {case:?}: a refused disable must leave the account default in effect, \
             got {after_root}"
        );
    }
}

/// The sentence a disable of a setting its principal does not hold is refused with.
const SETTING_NOT_FOUND_MESSAGE: &str = "Referenced setting doesn't exist";

/// One row of the table below: how the principal came to hold NO setting of the name the
/// disable names, and the disable it then makes.
struct AbsentSettingCase {
    case: &'static str,
    name: &'static str,
    /// The calls that set the row up, each an operation and the body it is sent.
    setup: Vec<(&'static str, Value)>,
    /// The `DeleteAccountSetting` the row makes.
    request: Value,
}

#[tokio::test]
async fn disabling_a_setting_the_principal_does_not_hold_is_refused_and_changes_nothing() {
    // One row per way a principal can hold NO setting of the name a disable names: nobody
    // ever wrote it, this principal wrote ANOTHER name, only the ACCOUNT DEFAULT holds it
    // (so it belongs to the root user alone), and this principal wrote it and already
    // disabled it (the re-delete a teardown or a converge loop makes). The rows also span
    // the value forms a setting takes: off, on, a word pair and a number.
    let rows = [
        AbsentSettingCase {
            case: "nobody wrote it",
            name: "fargateFIPSMode",
            setup: Vec::new(),
            request: json!({ "name": "fargateFIPSMode" }),
        },
        AbsentSettingCase {
            case: "this principal wrote another name",
            name: "serviceLongArnFormat",
            setup: vec![(
                "PutAccountSetting",
                json!({
                    "name": "taskLongArnFormat",
                    "value": "disabled",
                }),
            )],
            request: json!({ "name": "serviceLongArnFormat" }),
        },
        AbsentSettingCase {
            case: "only the account default holds it",
            name: "defaultLogDriverMode",
            setup: vec![(
                "PutAccountSettingDefault",
                json!({ "name": "defaultLogDriverMode", "value": "blocking" }),
            )],
            request: json!({ "name": "defaultLogDriverMode" }),
        },
        AbsentSettingCase {
            case: "already disabled",
            name: "fargateTaskRetirementWaitPeriod",
            setup: vec![
                (
                    "PutAccountSetting",
                    json!({ "name": "fargateTaskRetirementWaitPeriod", "value": "14" }),
                ),
                (
                    "DeleteAccountSetting",
                    json!({ "name": "fargateTaskRetirementWaitPeriod" }),
                ),
            ],
            request: json!({ "name": "fargateTaskRetirementWaitPeriod" }),
        },
    ];

    for AbsentSettingCase {
        case,
        name,
        setup,
        request,
    } in rows
    {
        // A fresh engine per row, so one row's writes cannot leak into another.
        let router = service();
        for (operation, body) in setup {
            ok(&router, operation, body, case).await;
        }
        let before = effective(&router, name, case).await;

        let (code, message) = refusal(&router, "DeleteAccountSetting", request, case).await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected an InvalidParameterException, got {code:?} with \
             {message:?}"
        );
        assert_eq!(
            message, SETTING_NOT_FOUND_MESSAGE,
            "case {case:?}: the refusal must state the condition and name nothing the \
             request carried, got {message:?}"
        );

        // The refusal stored nothing and removed nothing: the setting in effect for the
        // principal is the one that was in effect before the call, and the principal
        // still holds no setting of its own.
        let after = effective(&router, name, case).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused disable must change nothing, got {after} after \
             {before}"
        );
        let own = all_settings(
            &router,
            json!({ "name": name, "effectiveSettings": false }),
            case,
        )
        .await;
        assert!(
            own.is_empty(),
            "case {case:?}: a refused disable must store nothing, got {own:?}"
        );
    }
}

#[tokio::test]
async fn a_disable_touches_one_principal_and_one_name_only() {
    let router = service();
    let case = "one principal, one name";

    for name in ["awsvpcTrunking", "containerInsights"] {
        ok(
            &router,
            "PutAccountSetting",
            json!({ "name": name, "value": "enabled" }),
            case,
        )
        .await;
    }
    // The ACCOUNT DEFAULT of the same name belongs to the root user, so it is the other
    // principal the disable must leave alone.
    ok(
        &router,
        "PutAccountSettingDefault",
        json!({ "name": "awsvpcTrunking", "value": "disabled" }),
        case,
    )
    .await;

    disable(&router, json!({ "name": "awsvpcTrunking" }), case).await;

    let own = all_settings(&router, json!({ "effectiveSettings": false }), case).await;
    assert_eq!(
        values(&own),
        vec![("containerInsights".to_string(), "enabled".to_string())],
        "case {case:?}: the disable must leave this principal's other names alone, got \
         {own:?}"
    );
    let in_effect = effective(&router, "awsvpcTrunking", case).await;
    assert_eq!(
        (
            in_effect["value"].clone(),
            in_effect["principalArn"].clone()
        ),
        (json!("disabled"), json!(root_principal())),
        "case {case:?}: the disable must leave the account default of the same name alone, \
         got {in_effect}"
    );
}

#[tokio::test]
async fn a_disable_outside_the_documented_parameters_is_refused() {
    let router = service();

    // `guardDutyActivate` is the one `aws_managed` setting, so it is read-only: a
    // disable of it is refused just as a write of it is.
    refused(
        &router,
        "DeleteAccountSetting",
        json!({ "name": "guardDutyActivate" }),
        "InvalidParameterException",
        "guardDutyActivate is read-only",
    )
    .await;

    // The principal a setting may belong to is an IAM ARN; the rows differ along the
    // field that makes the value something else.
    let refused_arns: [(&str, String); 4] = [
        ("not an ARN at all", "emulator".to_string()),
        ("no resource", format!("arn:aws:iam::{ACCOUNT}:")),
        (
            "a region on a global service",
            format!("arn:aws:iam:us-west-2:{ACCOUNT}:user/emulator"),
        ),
        (
            "a key longer than the store holds",
            format!("arn:aws:iam::{ACCOUNT}:user/{}", "p".repeat(2048)),
        ),
    ];
    for (case, principal_arn) in refused_arns {
        refused(
            &router,
            "DeleteAccountSetting",
            json!({ "name": "awsvpcTrunking", "principalArn": principal_arn }),
            "InvalidParameterException",
            case,
        )
        .await;
    }
    // None of the refusals changed anything: `guardDutyActivate` is still what Amazon
    // Web Services reports for it.
    let setting = effective(&router, "guardDutyActivate", "after the refusals").await;
    assert_eq!(
        (setting["value"].clone(), setting["type"].clone()),
        (json!(documented_default("guardDutyActivate")), json!("aws_managed")),
        "case \"after the refusals\": a read-only setting must keep its value, got \
         {setting}"
    );
}

#[tokio::test]
async fn a_refused_principal_is_named_in_the_refusal_with_the_value_it_was_given() {
    let router = service();

    // The refusal is built from the REQUEST: it names the member that is wrong and the
    // value that member carried. The rows differ along that value's form, and the same
    // sentence is expected from every account-setting operation that takes a principal.
    let forms: [(&str, String); 4] = [
        ("not an ARN at all", "not-an-arn".to_string()),
        ("an empty string", String::new()),
        (
            "another service entirely",
            format!("arn:aws:ecs::{ACCOUNT}:cluster/default"),
        ),
        (
            "a region on a global service",
            format!("arn:aws:iam:us-west-2:{ACCOUNT}:user/emulator"),
        ),
    ];

    for (form, principal_arn) in forms {
        for operation in [
            "DeleteAccountSetting",
            "PutAccountSetting",
            "ListAccountSettings",
        ] {
            let case = format!("{operation} with {form}");
            let mut request = json!({ "principalArn": principal_arn });
            if operation != "ListAccountSettings" {
                request["name"] = json!("awsvpcTrunking");
            }
            if operation == "PutAccountSetting" {
                request["value"] = json!("enabled");
            }
            let (code, message) = refusal(&router, operation, request, &case).await;
            assert_eq!(
                code, "InvalidParameterException",
                "case {case:?}: expected an InvalidParameterException, got {code:?} with \
                 {message:?}"
            );
            assert_eq!(
                message,
                format!("Invalid PrincipalArn: {principal_arn}"),
                "case {case:?}: the refusal must name the member and the value it was \
                 given, got {message:?}"
            );
        }
    }
}

/// The catalog the handlers resolve values over must cover EXACTLY the `SettingName`
/// enum: a name the model gains and the catalog misses would be reported with a value
/// nobody stored, and a name the catalog holds and the model drops would be reported for
/// a setting the service does not have. Pinned against the model's own generated value
/// list, so a model update fails here rather than silently.
#[test]
fn the_catalog_covers_exactly_the_modelled_setting_names() {
    let case = "SettingName";
    let mut modelled: Vec<&str> =
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::ops::types::SETTING_NAME_VALUES
            .to_vec();
    modelled.sort_unstable();
    let mut catalogued: Vec<&str> =
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::ACCOUNT_SETTINGS
            .iter()
            .map(|setting| setting.name)
            .collect();
    catalogued.sort_unstable();
    assert_eq!(
        catalogued, modelled,
        "case {case:?}: the account-settings catalog must cover exactly the modelled \
         setting names, got {catalogued:?} against {modelled:?}"
    );

    // ... and the table this suite asserts over is the same set, so the wire
    // expectations above cannot drift from the catalog either.
    let mut tabled: Vec<&str> = ACCOUNT_SETTINGS.iter().map(|(name, _, _)| *name).collect();
    tabled.sort_unstable();
    assert_eq!(
        tabled, modelled,
        "case {case:?}: this suite's table must cover exactly the modelled setting \
         names, got {tabled:?} against {modelled:?}"
    );
}
