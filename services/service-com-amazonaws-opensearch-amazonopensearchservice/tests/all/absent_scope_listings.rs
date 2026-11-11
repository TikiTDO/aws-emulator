// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the reads that report a SET the service keeps ABOUT a named
//! scope — `ListPackagesForDomain`, `ListDomainsForPackage`, `ListVpcEndpointAccess`
//! and `ListInsights` — and for the configuration-setting list `GetApplication`
//! reports.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. This suite adds no second copy of anything
//! the sibling suites already have: the harness comes from [`super::domain_lifecycle`],
//! the table of names no domain is held under and the `DeleteDomain` that makes one
//! from [`super::package_listing_for_missing_domain`], the package helpers from
//! [`super::package_associations`], the listing and associate request helpers from
//! [`super::package_listing_page_bounds`], the authorization paths from
//! [`super::vpc_endpoint_access`], the insight listing from [`super::insights`], the
//! insight seeding from [`super::insight_details`] and the application paths and the
//! bounded `appConfigs` generator from [`super::application_updates`].
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * a read that reports a set the service keeps about a named scope treats that name
//!   as a SELECTOR over the set, so it answers 200 with the set's member present and
//!   empty whenever nothing in it matches — including when the account holds no
//!   resource under that name — over a table crossing each of the four reads with
//!   every shape of unresolvable scope (never created, at either published bound of the
//!   name, created and then deleted, and for a package both forms its id pattern
//!   admits) and with the filters a request may carry;
//! * the same reads report what the scope DOES hold, so an empty answer is the set and
//!   not the read answering nothing at all;
//! * refusing an unresolvable scope belongs to the calls that CHANGE it: over the same
//!   table, associating a package, authorizing endpoint access and rating an insight are
//!   each refused while the matching read is served, and the served read creates
//!   nothing;
//! * `GetApplication` reports `appConfigs` as the list the application holds — `[]`
//!   where it holds none — over a table spanning the member's published bounds, because
//!   `AppConfigs` is `@length(min: 0, max: 200)`.

use serde_json::{json, Value};

use super::application_updates::{
    app_configs, create_application, get_application, update_application,
};
use super::domain_lifecycle::{
    create_baseline, error_code, string_member, Answer, Emulator, PREFIX,
    RECORDED_NOT_FOUND_STATUS, SETTLE_WAIT,
};
use super::insight_details::{guid, raise_on_domain, record};
use super::insights::{insights, list_domain as list_insights_for};
use super::package_associations::{
    available_packages, create_package, PACKAGE_TYPES, SERVICE_PACKAGE_ID, UNKNOWN_PACKAGE_ID,
};
use super::package_listing_for_missing_domain::missing_names;
use super::package_listing_page_bounds::{
    associate_one, details, list_for_domain, list_for_package, page_query, PUBLISHED_MAX_RESULTS,
};
use super::vpc_endpoint_access::{authorize, list as list_vpc_access, principal_list};

/// The page sizes every row that takes one is asked at: none at all (the modelled
/// default), the smallest page a request can ask for, one in between, and the published
/// maximum — so the rule is asserted about the SET rather than about one page size.
const PAGE_SIZES: [i32; 4] = [0, 1, 7, PUBLISHED_MAX_RESULTS];

/// `ListInsights` bodies a row sends for one entity, so the rule is asserted over the
/// filters a request may carry and not only over the bare listing: no filter at all,
/// each sort order `InsightSortOrder` declares, a bounded page, and a domain-scoped,
/// sorted, time-ranged query.
fn insight_queries() -> Vec<(&'static str, Value)> {
    vec![
        ("no filter", json!({})),
        ("ascending", json!({ "SortOrder": "ASC" })),
        ("descending", json!({ "SortOrder": "DESC" })),
        ("a bounded page", json!({ "MaxResults": 1 })),
        (
            "sorted over a time range",
            json!({
                "SortOrder": "DESC",
                "MaxResults": 20,
                "TimeRange": { "From": 1_600_000_000_000_i64, "To": 1_900_000_000_000_i64 },
            }),
        ),
    ]
}

/// Every shape of `PackageID` this account holds no package under, each satisfying the
/// member's pattern `^([FG][0-9]+)$|^(pkg-[a-f0-9]+)$`
/// (`smithy_query(query="operation",operation="ListDomainsForPackage")`): both forms the
/// pattern admits, at more than one length, plus an id whose package the account created
/// and then deleted.
async fn unresolvable_package_ids(emulator: &Emulator) -> Vec<(String, String)> {
    let gone = create_package(emulator, "absent-scope-gone", PACKAGE_TYPES[0]).await;
    // A package can be deleted only once its content is in place, and the removal the
    // delete owes falls due a span later — so the id stops resolving at all rather than
    // naming a record on its way out.
    tokio::time::sleep(SETTLE_WAIT).await;
    let deleted = emulator
        .call("DELETE", &format!("{PREFIX}/packages/{gone}"), None)
        .await;
    assert_eq!(
        deleted.status, 200,
        "case DeletePackage({gone}): expected 200, got {} with {:?}",
        deleted.status, deleted.body
    );
    tokio::time::sleep(SETTLE_WAIT).await;

    vec![
        (
            "an id no create of this account minted".to_owned(),
            UNKNOWN_PACKAGE_ID.to_owned(),
        ),
        (
            "a shorter id of the same form".to_owned(),
            "pkg-abcdef0123456789".to_owned(),
        ),
        (
            "the service-published form".to_owned(),
            SERVICE_PACKAGE_ID.to_owned(),
        ),
        (
            "the shortest service-published id".to_owned(),
            "G7".to_owned(),
        ),
        ("an id whose package was deleted".to_owned(), gone),
    ]
}

/// Assert that `answer` is a served listing whose `member` is present and empty, and
/// that it hands out no continuation token past the end of an empty set.
fn empty_page(answer: &Answer, member: &str, case: &str) {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let reported = answer.body[member].as_array().unwrap_or_else(|| {
        panic!(
            "case {case}: expected {member} to be present as a list, got {:?}",
            answer.body
        )
    });
    assert!(
        reported.is_empty(),
        "case {case}: expected an empty {member}, got {reported:?}"
    );
    assert_eq!(
        answer.body["NextToken"].as_str(),
        None,
        "case {case}: expected no token past the end of an empty set, got {:?}",
        answer.body
    );
}

#[tokio::test]
async fn every_set_scoped_read_answers_the_empty_set_for_a_scope_nothing_is_held_under() {
    let emulator = Emulator::new();
    let names = missing_names(&emulator).await;

    for (row, name) in &names {
        // The two reads scoped by a domain name that take a page size.
        for page_size in PAGE_SIZES {
            let case = format!("ListPackagesForDomain({name:?}, {row}, maxResults={page_size})");
            let answer = list_for_domain(&emulator, name, &page_query(page_size, None)).await;
            empty_page(&answer, "DomainPackageDetailsList", &case);
        }

        // One answer reports a domain's whole authorization set, so this read takes no
        // page size at all.
        let case = format!("ListVpcEndpointAccess({name:?}, {row})");
        let answer = list_vpc_access(&emulator, name, None).await;
        empty_page(&answer, "AuthorizedPrincipalList", &case);

        // And the insight listing, over every filter a request may carry.
        for (filter, query) in insight_queries() {
            let case = format!("ListInsights(DomainName {name:?}, {row}, {filter})");
            let answer = list_insights_for(&emulator, name, query).await;
            empty_page(&answer, "Insights", &case);
        }
    }

    // The same rule the other way round the link: a package id the account holds no
    // package under selects no domain.
    for (row, package_id) in unresolvable_package_ids(&emulator).await {
        for page_size in PAGE_SIZES {
            let case =
                format!("ListDomainsForPackage({package_id}, {row}, maxResults={page_size})");
            let answer = list_for_package(&emulator, &package_id, &page_query(page_size, None))
                .await;
            empty_page(&answer, "DomainPackageDetailsList", &case);
        }
    }
}

#[tokio::test]
async fn the_same_reads_report_what_a_scope_that_exists_holds() {
    let emulator = Emulator::new();
    let ids = available_packages(&emulator).await;

    // A domain holding one of each kind of set, and a sibling domain holding none, so
    // an empty answer above is the SET being empty and not the read answering nothing.
    let holder = "absent-scope-holder";
    let bare = "absent-scope-bare";
    create_baseline(&emulator, holder).await;
    create_baseline(&emulator, bare).await;

    let linked = associate_one(&emulator, holder, &ids[0]).await;
    assert_eq!(
        linked.status, 200,
        "case AssociatePackage({holder:?}): expected 200, got {} with {:?}",
        linked.status, linked.body
    );
    let granted = authorize(&emulator, holder, json!({ "Account": "000000000001" })).await;
    assert_eq!(
        granted.status, 200,
        "case AuthorizeVpcEndpointAccess({holder:?}): expected 200, got {} with {:?}",
        granted.status, granted.body
    );
    raise_on_domain(
        &emulator,
        holder,
        record(&guid("000000000001"), std::time::SystemTime::now(), Vec::new()),
    );

    // Each read reports the one record the scope holds.
    let case = format!("ListPackagesForDomain({holder:?})");
    let reported = details(&list_for_domain(&emulator, holder, "").await, &case);
    assert_eq!(
        reported.len(),
        1,
        "case {case}: expected the one association made, got {reported:?}"
    );
    let case = format!("ListDomainsForPackage({})", ids[0]);
    let reported = details(&list_for_package(&emulator, &ids[0], "").await, &case);
    assert_eq!(
        reported.len(),
        1,
        "case {case}: expected the one association made, got {reported:?}"
    );
    let case = format!("ListVpcEndpointAccess({holder:?})");
    let answer = list_vpc_access(&emulator, holder, None).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        principal_list(&answer, &case).len(),
        1,
        "case {case}: expected the one grant made, got {:?}",
        answer.body
    );
    let case = format!("ListInsights(DomainName {holder:?})");
    let answer = list_insights_for(&emulator, holder, json!({})).await;
    assert_eq!(
        insights(&answer, &case).len(),
        1,
        "case {case}: expected the one insight raised, got {:?}",
        answer.body
    );

    // A domain that EXISTS holding nothing answers exactly as a name no domain is held
    // under does, which is what makes the rule about the set rather than about the name
    // resolving.
    empty_page(
        &list_for_domain(&emulator, bare, "").await,
        "DomainPackageDetailsList",
        &format!("ListPackagesForDomain({bare:?})"),
    );
    empty_page(
        &list_vpc_access(&emulator, bare, None).await,
        "AuthorizedPrincipalList",
        &format!("ListVpcEndpointAccess({bare:?})"),
    );
    empty_page(
        &list_insights_for(&emulator, bare, json!({})).await,
        "Insights",
        &format!("ListInsights(DomainName {bare:?})"),
    );
    empty_page(
        &list_for_package(&emulator, &ids[1], "").await,
        "DomainPackageDetailsList",
        &format!("ListDomainsForPackage({}, on no domain)", ids[1]),
    );
}

#[tokio::test]
async fn the_reads_are_served_where_the_calls_that_change_the_scope_are_refused() {
    let emulator = Emulator::new();
    let ids = available_packages(&emulator).await;
    let names = missing_names(&emulator).await;

    for (row, name) in &names {
        // Every call that would CHANGE what this name holds is refused, each with the
        // error its own model declares for a resource that is not there.
        let refused = associate_one(&emulator, name, &ids[0]).await;
        assert_eq!(
            (refused.status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case AssociatePackage({name:?}, {row}): expected the name to be refused, got {} \
             with {:?}",
            refused.status,
            refused.body
        );
        let refused = authorize(&emulator, name, json!({ "Account": "000000000002" })).await;
        assert_eq!(
            (refused.status, error_code(&refused).as_str()),
            (RECORDED_NOT_FOUND_STATUS, "ResourceNotFoundException"),
            "case AuthorizeVpcEndpointAccess({name:?}, {row}): expected the name to be refused, \
             got {} with {:?}",
            refused.status,
            refused.body
        );
        let refused = emulator
            .call(
                "POST",
                &format!("{PREFIX}/opensearch/insight-feedback"),
                Some(json!({
                    "Entity": { "Type": "DomainName", "Value": name },
                    "InsightId": guid("000000000009"),
                    "Thumbs": "Up",
                })),
            )
            .await;
        assert_eq!(
            (refused.status, error_code(&refused).as_str()),
            (RECORDED_NOT_FOUND_STATUS, "ResourceNotFoundException"),
            "case InsightFeedback({name:?}, {row}): expected the name to be refused, got {} \
             with {:?}",
            refused.status,
            refused.body
        );

        // The reads of the SAME name are served, and they created nothing: the name
        // still resolves to no domain, so the calls above are still refused.
        empty_page(
            &list_for_domain(&emulator, name, "").await,
            "DomainPackageDetailsList",
            &format!("ListPackagesForDomain({name:?}, {row}, after the refusals)"),
        );
        empty_page(
            &list_vpc_access(&emulator, name, None).await,
            "AuthorizedPrincipalList",
            &format!("ListVpcEndpointAccess({name:?}, {row}, after the refusals)"),
        );
        empty_page(
            &list_insights_for(&emulator, name, json!({})).await,
            "Insights",
            &format!("ListInsights(DomainName {name:?}, {row}, after the refusals)"),
        );

        let described = emulator.describe(name).await;
        assert_eq!(
            described.status, RECORDED_NOT_FOUND_STATUS,
            "case DescribeDomain({name:?}, {row}): expected the reads to have created no \
             domain, got {} with {:?}",
            described.status, described.body
        );
        let still_refused = authorize(&emulator, name, json!({ "Account": "000000000003" })).await;
        assert_eq!(
            still_refused.status, RECORDED_NOT_FOUND_STATUS,
            "case AuthorizeVpcEndpointAccess({name:?}, {row}, after the reads): expected the \
             name to still be refused, got {} with {:?}",
            still_refused.status,
            still_refused.body
        );
    }
}

#[tokio::test]
async fn a_read_reports_an_applications_configuration_settings_as_a_list_even_when_empty() {
    let emulator = Emulator::new();

    // Rows differ along the only thing this rule generalises over: how many settings
    // the application holds. The member a request leaves out entirely, the published
    // lower bound of `AppConfigs` (`@length(min: 0, max: 200)`), one, a handful, and
    // the published upper bound.
    let rows: Vec<(&str, Option<usize>)> = vec![
        ("a create that named no appConfigs at all", None),
        ("the published lower bound", Some(0)),
        ("one setting", Some(1)),
        ("a handful of settings", Some(7)),
        ("the published upper bound", Some(200)),
    ];

    for (index, (case, count)) in rows.into_iter().enumerate() {
        let mut body = json!({ "name": format!("configs-ui-{index}") });
        if let Some(count) = count {
            body["appConfigs"] = app_configs(count);
        }
        let created = create_application(&emulator, body).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected 200 from CreateApplication, got {} with {:?}",
            created.status, created.body
        );
        let id = string_member(&created.body, "id", case);

        // The read reports the whole stored record, so the member is a LIST in every
        // row — the empty one where the application holds no setting.
        let read = get_application(&emulator, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetApplication, got {} with {:?}",
            read.status, read.body
        );
        let reported = read.body["appConfigs"].as_array().unwrap_or_else(|| {
            panic!(
                "case {case}: expected appConfigs to be present as a list, got {:?}",
                read.body
            )
        });
        assert_eq!(
            Value::Array(reported.clone()),
            app_configs(count.unwrap_or_default()),
            "case {case}: expected the settings the create named, got {reported:?}"
        );

        // And the member stays a list through an update that empties it, which is the
        // round trip a client reading it relies on.
        let emptied = update_application(&emulator, &id, json!({ "appConfigs": [] })).await;
        assert_eq!(
            emptied.status, 200,
            "case {case}: expected 200 from UpdateApplication, got {} with {:?}",
            emptied.status, emptied.body
        );
        let read = get_application(&emulator, &id).await;
        assert_eq!(
            read.body["appConfigs"],
            json!([]),
            "case {case}: expected the emptied settings to be reported as [], got {:?}",
            read.body
        );
    }
}
