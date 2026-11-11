// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the OpenSearch Service direct-query data source registry:
//! `AddDataSource`, `GetDataSource`, `ListDataSources` and `DeleteDataSource`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`]; this suite adds no second copy of it, only the
//! data-source paths it drives.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * the listing reports EXACTLY the data sources stored on the named domain, over a
//!   table of five creates differing in name length and character class, in whether a
//!   description was supplied and how long it is, and in whether the union's member
//!   carried a role ARN — each row also read back one at a time through
//!   `GetDataSource`, and each domain's listing holding only its own records;
//! * every one of the four operations refuses a domain the store does not hold with
//!   the modelled `ResourceNotFoundException`, over a table of never-created and
//!   deleted domain names, while a name that resolves to no data source OF an
//!   existing domain is refused with the modelled `ValidationException`;
//! * `DataSourceName` is accepted exactly where its modelled constraint allows it —
//!   each published bound and one past it, plus one row per character class its
//!   pattern refuses;
//! * a create on a name the domain already holds is refused and leaves the stored
//!   record exactly as it was;
//! * a delete releases exactly the record it names and leaves the domain's other
//!   records in place, over every row of the create table;
//! * the per-domain store is bounded: creates are accepted up to the quota and the
//!   one past it is refused with the modelled `LimitExceededException`, per domain
//!   and with the stored set left at the quota.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    DATA_SOURCE_QUOTA, DATA_SOURCE_STATUSES, DATA_SOURCE_STATUS_ACTIVE,
};

use super::domain_lifecycle::{error_code, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS};

/// `AddDataSource`: `POST /opensearch/domain/{DomainName}/dataSource`.
pub(crate) async fn add(emulator: &Emulator, domain: &str, body: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/domain/{domain}/dataSource"),
            Some(body),
        )
        .await
}

/// `GetDataSource`: `GET /opensearch/domain/{DomainName}/dataSource/{Name}`.
pub(crate) async fn get(emulator: &Emulator, domain: &str, name: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/domain/{domain}/dataSource/{name}"),
            None,
        )
        .await
}

/// `ListDataSources`: `GET /opensearch/domain/{DomainName}/dataSource`.
pub(crate) async fn list(emulator: &Emulator, domain: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/domain/{domain}/dataSource"),
            None,
        )
        .await
}

/// `DeleteDataSource`: `DELETE /opensearch/domain/{DomainName}/dataSource/{Name}`.
async fn delete(emulator: &Emulator, domain: &str, name: &str) -> Answer {
    emulator
        .call(
            "DELETE",
            &format!("{PREFIX}/opensearch/domain/{domain}/dataSource/{name}"),
            None,
        )
        .await
}

/// `DeleteDomain`, so a test can reach a domain name the store no longer holds.
async fn delete_domain(emulator: &Emulator, name: &str) -> Answer {
    emulator
        .call("DELETE", &format!("{PREFIX}/opensearch/domain/{name}"), None)
        .await
}

/// The `DataSources` list of a `ListDataSources` answer, with the case named on
/// failure.
pub(crate) fn data_sources(answer: &Answer, case: &str) -> Vec<Value> {
    answer.body["DataSources"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a DataSources array, got {:?}",
                answer.body
            )
        })
        .clone()
}

/// One create to make: the data source's `Name`, the `Description` to send (absent
/// where `None`), and the `RoleArn` the `S3GlueDataCatalog` member carries (absent
/// where `None`).
struct Row {
    name: String,
    description: Option<String>,
    role_arn: Option<String>,
}

/// The create table: one row per way an accepted create differs from another —
/// the name at its shortest and longest and spanning its character classes, no
/// description / an empty one / a long one, and the union's member with and without
/// a role ARN.
///
/// `DataSourceName` is `@length(min: 3, max: 80)`, `DataSourceDescription` is
/// `@length(min: 0, max: 1000)`, and `S3GlueDataCatalog$RoleArn` is optional — all
/// from this service's Smithy model
/// (`smithy_query(query="operation",operation="AddDataSource")`).
fn create_rows() -> Vec<Row> {
    vec![
        // The shortest name the constraint allows, and no description at all.
        Row {
            name: "abc".to_string(),
            description: None,
            role_arn: Some("arn:aws:iam::123456789012:role/s3-glue-one".to_string()),
        },
        Row {
            name: "logs_s3_source".to_string(),
            description: Some("primary S3 Glue catalog".to_string()),
            role_arn: Some("arn:aws:iam::123456789012:role/s3-glue-two".to_string()),
        },
        // A description at its minimum length: zero characters.
        Row {
            name: "a_9".to_string(),
            description: Some(String::new()),
            role_arn: Some("arn:aws:iam::123456789012:role/s3-glue-three".to_string()),
        },
        // Both strings at their maximum length.
        Row {
            name: "z".repeat(80),
            description: Some("d".repeat(1000)),
            role_arn: Some("arn:aws:iam::123456789012:role/s3-glue-four".to_string()),
        },
        // The union member is required; the role ARN inside it is not.
        Row {
            name: "no_role_source".to_string(),
            description: Some("catalog without a role".to_string()),
            role_arn: None,
        },
    ]
}

impl Row {
    /// This row as an `AddDataSource` body.
    fn body(&self) -> Value {
        let mut catalog = json!({});
        if let Some(role_arn) = &self.role_arn {
            catalog["RoleArn"] = json!(role_arn);
        }
        let mut body = json!({
            "Name": self.name,
            "DataSourceType": { "S3GlueDataCatalog": catalog },
        });
        if let Some(description) = &self.description {
            body["Description"] = json!(description);
        }
        body
    }

    /// The `DataSourceDetails` this row must be reported as once stored.
    fn expected_details(&self) -> Value {
        let mut catalog = json!({});
        if let Some(role_arn) = &self.role_arn {
            catalog["RoleArn"] = json!(role_arn);
        }
        let mut details = json!({
            "Name": self.name,
            "DataSourceType": { "S3GlueDataCatalog": catalog },
            // `DataSourceStatus` declares exactly ACTIVE and DISABLED; a create
            // yields a queryable data source, so ACTIVE.
            "Status": DATA_SOURCE_STATUS_ACTIVE,
        });
        if let Some(description) = &self.description {
            details["Description"] = json!(description);
        }
        details
    }
}

/// `CreateDomain` for `name`, asserting it was accepted.
pub(crate) async fn created_domain(emulator: &Emulator, name: &str) {
    let answer = emulator.create(json!({ "DomainName": name })).await;
    assert_eq!(
        answer.status, 200,
        "case CreateDomain({name:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
}

#[tokio::test]
async fn every_created_data_source_is_reported_by_the_listing_and_read_back() {
    let emulator = Emulator::new();
    created_domain(&emulator, "catalogs").await;

    // A domain nobody added a data source to holds none: the listing is the EMPTY
    // list, not an absent member and not somebody else's records.
    let empty = list(&emulator, "catalogs").await;
    assert_eq!(
        empty.status, 200,
        "case ListDataSources(empty): expected 200, got {} with {:?}",
        empty.status, empty.body
    );
    assert!(
        data_sources(&empty, "empty").is_empty(),
        "case ListDataSources(empty): expected no data sources, got {:?}",
        empty.body
    );

    let rows = create_rows();
    for row in &rows {
        let case = format!("AddDataSource({:?})", row.name);
        let answer = add(&emulator, "catalogs", row.body()).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        // The message is "a message associated with creation of the data source";
        // the service's own wording is not published, so what is pinned is the rule
        // the handler follows — it names the data source and its domain.
        let message = answer.body["Message"].as_str().unwrap_or_default();
        assert!(
            message.contains(&row.name) && message.contains("catalogs"),
            "case {case}: expected a Message naming the data source and domain, got {:?}",
            answer.body
        );
    }

    // The listing reports exactly the stored set — one entry per create, each
    // member-for-member what was persisted, and nothing else.
    let listed = list(&emulator, "catalogs").await;
    let entries = data_sources(&listed, "listing");
    assert_eq!(
        entries.len(),
        rows.len(),
        "case ListDataSources: expected {} data sources, got {:?}",
        rows.len(),
        listed.body
    );
    for row in &rows {
        let case = format!("ListDataSources({:?})", row.name);
        let entry = entries
            .iter()
            .find(|entry| entry["Name"] == json!(row.name))
            .unwrap_or_else(|| panic!("case {case}: the created name is missing from {entries:?}"));
        assert_eq!(
            *entry,
            row.expected_details(),
            "case {case}: expected {:?}, got {entry:?}",
            row.expected_details()
        );
        assert!(
            DATA_SOURCE_STATUSES.contains(&entry["Status"].as_str().unwrap_or_default()),
            "case {case}: Status must be one of {DATA_SOURCE_STATUSES:?}, got {entry:?}"
        );
    }

    // The order is the stored order — by name — and it is stable, because a read
    // changes nothing about what the next caller sees.
    let mut expected_names: Vec<String> = rows.iter().map(|row| row.name.clone()).collect();
    expected_names.sort();
    let got_names: Vec<String> = entries
        .iter()
        .map(|entry| entry["Name"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        got_names, expected_names,
        "case ListDataSources(order): expected {expected_names:?}, got {got_names:?}"
    );
    let again = list(&emulator, "catalogs").await;
    assert_eq!(
        again.body["DataSources"], listed.body["DataSources"],
        "case ListDataSources(twice): a read must not change the records: {:?} then {:?}",
        listed.body, again.body
    );

    // Each record also reads back one at a time: `GetDataSource` and the listing are
    // two projections of ONE stored record, so they agree member for member.
    for row in &rows {
        let case = format!("GetDataSource({:?})", row.name);
        let read = get(&emulator, "catalogs", &row.name).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body,
            row.expected_details(),
            "case {case}: expected {:?}, got {:?}",
            row.expected_details(),
            read.body
        );
    }

    // A data source belongs to its own domain: a second domain's listing holds only
    // what was added to it.
    created_domain(&emulator, "other-catalogs").await;
    let other = Row {
        name: "other_source".to_string(),
        description: Some("on the second domain".to_string()),
        role_arn: Some("arn:aws:iam::123456789012:role/s3-glue-other".to_string()),
    };
    let added = add(&emulator, "other-catalogs", other.body()).await;
    assert_eq!(
        added.status, 200,
        "case AddDataSource(other-catalogs): expected 200, got {} with {:?}",
        added.status, added.body
    );
    let other_entries = data_sources(&list(&emulator, "other-catalogs").await, "other domain");
    assert_eq!(
        other_entries,
        vec![other.expected_details()],
        "case ListDataSources(other-catalogs): expected only its own record, got {other_entries:?}"
    );
    let first_entries = data_sources(&list(&emulator, "catalogs").await, "first domain again");
    assert_eq!(
        first_entries.len(),
        rows.len(),
        "case ListDataSources(catalogs): another domain's create must not change it, got \
         {first_entries:?}"
    );
    let missing_on_other = get(&emulator, "catalogs", &other.name).await;
    assert_eq!(
        missing_on_other.status, 400,
        "case GetDataSource(catalogs, other_source): a sibling domain's data source is not \
         visible here, got {} with {:?}",
        missing_on_other.status, missing_on_other.body
    );
    assert_eq!(
        error_code(&missing_on_other),
        "ValidationException",
        "case GetDataSource(catalogs, other_source): expected ValidationException, got {:?}",
        missing_on_other.body
    );
}

#[tokio::test]
async fn a_data_source_call_on_a_domain_the_store_does_not_hold_is_refused_as_not_found() {
    let emulator = Emulator::new();

    // A domain that existed and was deleted is as absent as one that never existed:
    // both rows below satisfy `DomainName`'s own constraint, so the refusal comes
    // from the store, not from the boundary validator.
    created_domain(&emulator, "gone-domain").await;
    let deleted = delete_domain(&emulator, "gone-domain").await;
    assert_eq!(
        deleted.status, 200,
        "case DeleteDomain(gone-domain): expected 200, got {} with {:?}",
        deleted.status, deleted.body
    );

    let valid_body = create_rows()
        .first()
        .expect("the create table has rows")
        .body();
    for domain in ["never-created", "gone-domain"] {
        for (operation, answer) in [
            ("ListDataSources", list(&emulator, domain).await),
            ("GetDataSource", get(&emulator, domain, "abc").await),
            ("DeleteDataSource", delete(&emulator, domain, "abc").await),
            (
                "AddDataSource",
                add(&emulator, domain, valid_body.clone()).await,
            ),
        ] {
            let case = format!("{operation}({domain:?})");
            assert_eq!(
                answer.status, RECORDED_NOT_FOUND_STATUS,
                "case {case}: expected {RECORDED_NOT_FOUND_STATUS} for a domain the store does \
                 not hold, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                error_code(&answer),
                "ResourceNotFoundException",
                "case {case}: expected ResourceNotFoundException, got {:?}",
                answer.body
            );
        }
    }

    // On a domain that DOES exist, a name no data source of it carries is a bad
    // REQUEST, not a missing domain: the service refuses it with `ValidationException`
    // naming the name it could not resolve. That is how a caller tells the two apart.
    created_domain(&emulator, "live-domain").await;
    for name in ["nothing_here", "another_absent_name"] {
        for (operation, absent) in [
            ("GetDataSource", get(&emulator, "live-domain", name).await),
            (
                "DeleteDataSource",
                delete(&emulator, "live-domain", name).await,
            ),
        ] {
            let case = format!("{operation}(live-domain, {name:?})");
            assert_eq!(
                absent.status, 400,
                "case {case}: expected 400, got {} with {:?}",
                absent.status, absent.body
            );
            assert_eq!(
                error_code(&absent),
                "ValidationException",
                "case {case}: expected ValidationException, got {:?}",
                absent.body
            );
            assert!(
                absent.body["message"]
                    .as_str()
                    .or_else(|| absent.body["Message"].as_str())
                    .is_some_and(|message| message.contains(name)),
                "case {case}: the refusal must name the unresolved name, got {:?}",
                absent.body
            );
        }
    }
}

#[tokio::test]
async fn a_data_source_name_is_accepted_exactly_where_the_modelled_constraint_allows_it() {
    let emulator = Emulator::new();
    created_domain(&emulator, "name-rules").await;

    // `DataSourceName` is `@length(min: 3, max: 80)` with
    // `@pattern("^[a-z][a-z0-9_]+$")`: each published bound and one past it, plus one
    // row per character class the pattern refuses.
    let eighty = "y".repeat(80);
    let eighty_one = "y".repeat(81);
    let cases: Vec<(&str, bool)> = vec![
        ("abc", true),
        ("a_0", true),
        ("abc_123_source", true),
        (eighty.as_str(), true),
        // One past each published length bound.
        ("ab", false),
        (eighty_one.as_str(), false),
        // The pattern refuses an upper-case letter, a leading digit, a hyphen and a
        // dot.
        ("Abc", false),
        ("0abc", false),
        ("a-bc", false),
        ("a.bc", false),
    ];

    for (name, accepted) in cases {
        let case = format!("AddDataSource({name:?})");
        let row = Row {
            name: name.to_string(),
            description: Some("constraint row".to_string()),
            role_arn: Some("arn:aws:iam::123456789012:role/s3-glue-names".to_string()),
        };
        let answer = add(&emulator, "name-rules", row.body()).await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200 inside the constraint, got {} with {:?}",
                answer.status, answer.body
            );
            // An accepted name is persisted: the listing now carries it.
            let entries = data_sources(&list(&emulator, "name-rules").await, &case);
            assert!(
                entries.iter().any(|entry| entry["Name"] == json!(name)),
                "case {case}: an accepted name must be persisted, got {entries:?}"
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case}: expected 400 outside the constraint, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case}: expected ValidationException, got {:?}",
                answer.body
            );
            let entries = data_sources(&list(&emulator, "name-rules").await, &case);
            assert!(
                !entries.iter().any(|entry| entry["Name"] == json!(name)),
                "case {case}: a refused create must persist nothing, got {entries:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_delete_releases_exactly_the_record_it_names() {
    let emulator = Emulator::new();
    created_domain(&emulator, "released").await;

    let rows = create_rows();
    for row in &rows {
        let added = add(&emulator, "released", row.body()).await;
        assert_eq!(
            added.status, 200,
            "case AddDataSource({:?}): expected 200, got {} with {:?}",
            row.name, added.status, added.body
        );
    }

    // Deleting each row in turn: the named record goes, every record NOT named stays
    // exactly as it was, and the read paths agree with the listing after each step.
    let mut remaining: Vec<&Row> = rows.iter().collect();
    while let Some(target) = remaining.pop() {
        let case = format!("DeleteDataSource({:?})", target.name);
        let deleted = delete(&emulator, "released", &target.name).await;
        assert_eq!(
            deleted.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            deleted.status, deleted.body
        );

        // Gone from both read paths.
        let read = get(&emulator, "released", &target.name).await;
        assert_eq!(
            read.status, 400,
            "case {case}: a deleted data source must not read back, got {} with {:?}",
            read.status, read.body
        );
        let entries = data_sources(&list(&emulator, "released").await, &case);
        let mut expected: Vec<Value> = remaining.iter().map(|row| row.expected_details()).collect();
        expected.sort_by_key(|entry| entry["Name"].as_str().unwrap_or_default().to_string());
        assert_eq!(
            entries, expected,
            "case {case}: expected the remaining records {expected:?}, got {entries:?}"
        );

        // A second delete of the same name resolves to nothing, so it is refused
        // rather than reported as another success.
        let again = delete(&emulator, "released", &target.name).await;
        assert_eq!(
            again.status, 400,
            "case {case} twice: expected 400 the second time, got {} with {:?}",
            again.status, again.body
        );
    }

    // The domain itself is untouched by the deletes: it still exists and now holds
    // no data source at all.
    let empty = data_sources(&list(&emulator, "released").await, "all released");
    assert!(
        empty.is_empty(),
        "case ListDataSources(released): expected no records left, got {empty:?}"
    );
}

#[tokio::test]
async fn the_per_domain_store_is_bounded_by_its_quota() {
    let emulator = Emulator::new();
    created_domain(&emulator, "bounded").await;
    created_domain(&emulator, "unbounded-neighbour").await;

    // Every create up to the quota is accepted — the bound is on the COUNT, so the
    // names here differ only in their index.
    for index in 0..DATA_SOURCE_QUOTA {
        let row = Row {
            name: format!("source_{index}"),
            description: None,
            role_arn: Some("arn:aws:iam::123456789012:role/s3-glue-quota".to_string()),
        };
        let answer = add(&emulator, "bounded", row.body()).await;
        assert_eq!(
            answer.status, 200,
            "case AddDataSource(#{index} of {DATA_SOURCE_QUOTA}): expected 200, got {} with {:?}",
            answer.status, answer.body
        );
    }

    // One past the quota is refused with the modelled `LimitExceededException`
    // (HTTP 409), not served and not silently dropped.
    let past = Row {
        name: format!("source_{DATA_SOURCE_QUOTA}"),
        description: None,
        role_arn: Some("arn:aws:iam::123456789012:role/s3-glue-quota".to_string()),
    };
    let refused = add(&emulator, "bounded", past.body()).await;
    assert_eq!(
        refused.status, 409,
        "case AddDataSource(one past the quota): expected 409, got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        error_code(&refused),
        "LimitExceededException",
        "case AddDataSource(one past the quota): expected LimitExceededException, got {:?}",
        refused.body
    );

    // The refusal persisted nothing, so the listing is bounded by the quota too.
    let entries = data_sources(&list(&emulator, "bounded").await, "at the quota");
    assert_eq!(
        entries.len(),
        DATA_SOURCE_QUOTA,
        "case ListDataSources(bounded): expected the quota of {DATA_SOURCE_QUOTA} records, got {}",
        entries.len()
    );

    // The bound is PER DOMAIN: a domain at its quota does not stop another domain
    // from taking a create, and releasing a record frees room on the full one.
    let neighbour = Row {
        name: "source_0".to_string(),
        description: None,
        role_arn: Some("arn:aws:iam::123456789012:role/s3-glue-quota".to_string()),
    };
    let accepted = add(&emulator, "unbounded-neighbour", neighbour.body()).await;
    assert_eq!(
        accepted.status, 200,
        "case AddDataSource(unbounded-neighbour): one domain's quota must not bound another, \
         got {} with {:?}",
        accepted.status, accepted.body
    );
    let released = delete(&emulator, "bounded", "source_0").await;
    assert_eq!(
        released.status, 200,
        "case DeleteDataSource(bounded, source_0): expected 200, got {} with {:?}",
        released.status, released.body
    );
    let after_release = add(&emulator, "bounded", past.body()).await;
    assert_eq!(
        after_release.status, 200,
        "case AddDataSource(after a delete): a released record frees room, got {} with {:?}",
        after_release.status, after_release.body
    );
}

#[tokio::test]
async fn a_create_on_a_name_the_domain_already_holds_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    created_domain(&emulator, "dup-rules").await;

    let first = Row {
        name: "dup_source".to_string(),
        description: Some("the stored description".to_string()),
        role_arn: Some("arn:aws:iam::123456789012:role/s3-glue-first".to_string()),
    };
    let second = Row {
        name: "dup_source".to_string(),
        description: Some("a later description".to_string()),
        role_arn: Some("arn:aws:iam::123456789012:role/s3-glue-second".to_string()),
    };

    let created = add(&emulator, "dup-rules", first.body()).await;
    assert_eq!(
        created.status, 200,
        "case AddDataSource(dup_source, first): expected 200, got {} with {:?}",
        created.status, created.body
    );

    let again = add(&emulator, "dup-rules", second.body()).await;
    assert_eq!(
        again.status, 400,
        "case AddDataSource(dup_source, again): expected 400 for a name already used, got {} \
         with {:?}",
        again.status, again.body
    );
    assert_eq!(
        error_code(&again),
        "ValidationException",
        "case AddDataSource(dup_source, again): expected ValidationException, got {:?}",
        again.body
    );

    // The refused create left the stored record exactly as the first one wrote it,
    // and added no second entry.
    let read = get(&emulator, "dup-rules", &first.name).await;
    assert_eq!(
        read.body,
        first.expected_details(),
        "case GetDataSource(dup_source): expected the first record {:?}, got {:?}",
        first.expected_details(),
        read.body
    );
    let entries = data_sources(&list(&emulator, "dup-rules").await, "duplicate");
    assert_eq!(
        entries,
        vec![first.expected_details()],
        "case ListDataSources(dup-rules): expected exactly one record, got {entries:?}"
    );
}
