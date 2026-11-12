// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeServiceUpdates`.
//!
//! Every assertion goes over the wire, through the same router the production
//! emulator mounts, and nothing reaches into the service's state struct — so a
//! response that is not computed from the published catalog and the clock
//! cannot pass. The request builder and the XML reader are the shared
//! [`crate::harness`] ones, so this target keeps one harness rather than a
//! second copy that could drift.
//!
//! The tests are tables: each row is one input along the axis the rule under
//! test generalises over (the status filter and its combinations, the name
//! looked up, the page size, the refused value), so no single recorded request
//! is what makes them pass.

use std::time::{SystemTime, UNIX_EPOCH};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{params, Emulator, Reply};

/// One `ServiceUpdate` element of a response, as the wire reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WireUpdate {
    name: String,
    engine: String,
    engine_version: String,
    severity: String,
    status: String,
    update_type: String,
    description: String,
    release_date: String,
    recommended_apply_by_date: String,
    end_date: String,
    auto_update: String,
    estimated_update_time: String,
}

/// Seconds since the epoch of a timestamp as the awsQuery serializer rendered
/// it. Both renderings a timestamp member can take are accepted, so the test
/// pins the instant rather than the protocol's spelling of it.
pub(crate) fn epoch_secs(case: &str, member: &str, text: &str) -> i64 {
    DateTime::from_str(text, Format::DateTime)
        .or_else(|_| DateTime::from_str(text, Format::EpochSeconds))
        .unwrap_or_else(|err| panic!("case {case}: {member} {text:?} is not a timestamp: {err}"))
        .secs()
}

/// Now, in seconds since the epoch — the clock the reported statuses are read
/// against.
pub(crate) fn now_secs() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_secs(),
    )
    .expect("the epoch seconds of now fit in an i64")
}

/// Every `ServiceUpdate` a reply reports, in document order.
///
/// Each member is read as a column and the columns are zipped, which is also
/// the assertion that every reported update carries every member of the shape.
fn wire_updates(case: &str, reply: &Reply) -> Vec<WireUpdate> {
    let names = reply.all("ServiceUpdateName");
    let count = names.len();
    let column = |member: &str| -> Vec<String> {
        let values = reply.all(member);
        assert_eq!(
            values.len(),
            count,
            "case {case}: {member} must be reported once per update ({count}), got {}: {}",
            values.len(),
            reply.body
        );
        values.into_iter().map(str::to_string).collect()
    };
    let engines = column("Engine");
    let engine_versions = column("EngineVersion");
    let severities = column("ServiceUpdateSeverity");
    let statuses = column("ServiceUpdateStatus");
    let types = column("ServiceUpdateType");
    let descriptions = column("ServiceUpdateDescription");
    let release_dates = column("ServiceUpdateReleaseDate");
    let apply_by_dates = column("ServiceUpdateRecommendedApplyByDate");
    let end_dates = column("ServiceUpdateEndDate");
    let auto_updates = column("AutoUpdateAfterRecommendedApplyByDate");
    let estimated_times = column("EstimatedUpdateTime");
    (0..count)
        .map(|index| WireUpdate {
            name: names[index].to_string(),
            engine: engines[index].clone(),
            engine_version: engine_versions[index].clone(),
            severity: severities[index].clone(),
            status: statuses[index].clone(),
            update_type: types[index].clone(),
            description: descriptions[index].clone(),
            release_date: release_dates[index].clone(),
            recommended_apply_by_date: apply_by_dates[index].clone(),
            end_date: end_dates[index].clone(),
            auto_update: auto_updates[index].clone(),
            estimated_update_time: estimated_times[index].clone(),
        })
        .collect()
}

/// The form pairs that carry `statuses` in the `ServiceUpdateStatus` filter.
fn status_filter(statuses: &[&str]) -> Vec<(String, String)> {
    statuses
        .iter()
        .enumerate()
        .map(|(index, status)| {
            (
                format!("ServiceUpdateStatus.member.{}", index + 1),
                (*status).to_string(),
            )
        })
        .collect()
}

/// One `DescribeServiceUpdates` call that must succeed, with its reported
/// updates.
async fn describe(
    emulator: &Emulator,
    case: &str,
    pairs: &[(String, String)],
) -> (Reply, Vec<WireUpdate>) {
    let reply = emulator.call("DescribeServiceUpdates", pairs).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: DescribeServiceUpdates must answer 200, got {}: {}",
        reply.status, reply.body
    );
    let updates = wire_updates(case, &reply);
    (reply, updates)
}

/// The whole published catalog, in the order the service reports it.
async fn catalog(emulator: &Emulator, case: &str) -> Vec<WireUpdate> {
    describe(emulator, case, &[]).await.1
}

/// The status a published update must report, given when it stops being
/// available: `ServiceUpdateEndDate` is "the date after which the service
/// update is no longer available".
fn expected_status(end_secs: i64, now: i64) -> &'static str {
    if now >= end_secs {
        state::SERVICE_UPDATE_EXPIRED
    } else {
        state::SERVICE_UPDATE_AVAILABLE
    }
}

#[tokio::test]
async fn the_published_catalog_is_reported_with_every_member_of_the_shape() {
    let emulator = Emulator::new();
    let case = "unfiltered";
    let updates = catalog(&emulator, case).await;
    assert!(
        !updates.is_empty(),
        "case {case}: the service publishes a catalog, so an unfiltered describe reports updates"
    );

    let now = now_secs();
    for update in &updates {
        let name = &update.name;
        // A published update's name is part of what the service publishes: it
        // names the service and the engine the update applies to, and carries
        // nothing taken from the clock, so the same name is resolvable on every
        // store this build brings up (pinned against a differently anchored
        // catalog by the `catalog_publication` suite).
        assert!(
            name.starts_with("elasticache-"),
            "case {case}: {name} must be named for the service"
        );
        assert!(
            name.contains(&update.engine),
            "case {case}: {name} must name the engine it applies to, got {:?}",
            update.engine
        );
        assert!(
            name.split('-')
                .all(|part| !(part.len() == 8 && part.chars().all(|c| c.is_ascii_digit()))),
            "case {case}: {name} must not be named after a date the clock supplied"
        );

        // `Engine` is "the Elasticache engine to which the update applies" and
        // `EngineVersion` "the Elasticache engine version". A security update
        // applies to one released version; an end-of-life update applies to a
        // whole version LINE, reported as the engine and that line.
        assert!(
            state::ENGINES.contains(&update.engine.as_str()),
            "case {case}: {name} must apply to one of {:?}, got {:?}",
            state::ENGINES,
            update.engine
        );
        if update.update_type == state::SERVICE_UPDATE_TYPE {
            assert_eq!(
                update.engine_version,
                state::default_engine_version(&update.engine),
                "case {case}: {name} must apply to the version this service publishes for {}, \
                 got {:?}",
                update.engine,
                update.engine_version
            );
        } else {
            let line = update
                .engine_version
                .strip_prefix(&format!("{} ", update.engine))
                .unwrap_or_else(|| {
                    panic!(
                        "case {case}: {name} must report the engine and the version line it \
                         applies to, got {:?}",
                        update.engine_version
                    )
                });
            assert!(
                state::parameter_group_family(&update.engine, line).is_some(),
                "case {case}: {name} must apply to a version line this service publishes, got \
                 {line:?}"
            );
        }

        // `ServiceUpdateType` is published in two spellings — the modelled
        // `security-update` and the end-of-life update beside it — and
        // `ServiceUpdateSeverity` carries one of four values.
        assert!(
            [
                state::SERVICE_UPDATE_TYPE,
                state::SERVICE_UPDATE_TYPE_ENGINE_MAJOR_VERSION,
            ]
            .contains(&update.update_type.as_str()),
            "case {case}: {name} must be a {} or {} update, got {:?}",
            state::SERVICE_UPDATE_TYPE,
            state::SERVICE_UPDATE_TYPE_ENGINE_MAJOR_VERSION,
            update.update_type
        );
        assert!(
            ["critical", "important", "medium", "low"].contains(&update.severity.as_str()),
            "case {case}: {name} must carry a modelled ServiceUpdateSeverity, got {:?}",
            update.severity
        );
        assert!(
            !update.description.is_empty(),
            "case {case}: {name} must provide details of the update"
        );
        assert!(
            !update.estimated_update_time.is_empty(),
            "case {case}: {name} must report how long the update takes"
        );
        assert!(
            ["true", "false"].contains(&update.auto_update.as_str()),
            "case {case}: {name} must report AutoUpdateAfterRecommendedApplyByDate as a boolean, \
             got {:?}",
            update.auto_update
        );

        // The dates: released first, recommended apply-by inside the window,
        // and the end date last.
        let release = epoch_secs(case, "ServiceUpdateReleaseDate", &update.release_date);
        let apply_by = epoch_secs(
            case,
            "ServiceUpdateRecommendedApplyByDate",
            &update.recommended_apply_by_date,
        );
        let end = epoch_secs(case, "ServiceUpdateEndDate", &update.end_date);
        assert!(
            release < apply_by && apply_by < end,
            "case {case}: {name} must be released before its apply-by date and end date, got \
             {release} / {apply_by} / {end}"
        );
        assert_eq!(
            update.status,
            expected_status(end, now),
            "case {case}: {name} ends at {end} and now is {now}, so its status must be {}, got \
             {:?}",
            expected_status(end, now),
            update.status
        );
    }

    // Every engine the service publishes versions for has an update, and the
    // catalog spans both sides of the end-date rule.
    for engine in state::ENGINES {
        assert!(
            updates.iter().any(|update| update.engine == engine),
            "case {case}: the catalog must publish an update for {engine}, got {:?}",
            updates.iter().map(|u| &u.engine).collect::<Vec<_>>()
        );
    }
    for status in [
        state::SERVICE_UPDATE_AVAILABLE,
        state::SERVICE_UPDATE_EXPIRED,
    ] {
        assert!(
            updates.iter().any(|update| update.status == status),
            "case {case}: the catalog must hold an update the clock reports as {status}, got {:?}",
            updates.iter().map(|u| &u.status).collect::<Vec<_>>()
        );
    }

    // Names are unique: `ServiceUpdateName` is "the unique ID of the service
    // update".
    let mut names: Vec<&str> = updates.iter().map(|update| update.name.as_str()).collect();
    names.sort_unstable();
    let total = names.len();
    names.dedup();
    assert_eq!(
        names.len(),
        total,
        "case {case}: every ServiceUpdateName must be unique, got {names:?}"
    );
}

#[tokio::test]
async fn the_status_filter_reports_exactly_the_updates_in_those_statuses() {
    let emulator = Emulator::new();
    let published = catalog(&emulator, "unfiltered").await;

    // The axis: which of `ServiceUpdateStatus`'s three values the filter names,
    // how many it names (the member is modelled `@length(max: 3)`) and in which
    // order.
    let rows: [&[&str]; 7] = [
        &[],
        &[state::SERVICE_UPDATE_AVAILABLE],
        &[state::SERVICE_UPDATE_EXPIRED],
        &[state::SERVICE_UPDATE_CANCELLED],
        &[state::SERVICE_UPDATE_AVAILABLE, state::SERVICE_UPDATE_EXPIRED],
        &[state::SERVICE_UPDATE_EXPIRED, state::SERVICE_UPDATE_AVAILABLE],
        &[
            state::SERVICE_UPDATE_AVAILABLE,
            state::SERVICE_UPDATE_CANCELLED,
            state::SERVICE_UPDATE_EXPIRED,
        ],
    ];
    for row in rows {
        let case = format!("status filter {row:?}");
        let expected: Vec<&WireUpdate> = published
            .iter()
            .filter(|update| row.is_empty() || row.contains(&update.status.as_str()))
            .collect();
        let (_, reported) = describe(&emulator, &case, &status_filter(row)).await;
        assert_eq!(
            reported.len(),
            expected.len(),
            "case {case}: must report {} of the {} published updates, got {:?}",
            expected.len(),
            published.len(),
            reported.iter().map(|u| &u.name).collect::<Vec<_>>()
        );
        for (got, want) in reported.iter().zip(&expected) {
            assert_eq!(
                &got, want,
                "case {case}: a filtered update must be reported exactly as the catalog has it"
            );
        }
    }
}

#[tokio::test]
async fn one_update_is_read_back_by_its_name() {
    let emulator = Emulator::new();
    let published = catalog(&emulator, "unfiltered").await;

    for update in &published {
        let case = format!("lookup {}", update.name);
        // Looked up by name, the answer is that one update and nothing else.
        let (reply, reported) = describe(
            &emulator,
            &case,
            &params(&[("ServiceUpdateName", &update.name)]),
        )
        .await;
        assert_eq!(
            reported,
            vec![update.clone()],
            "case {case}: a single-record lookup must report exactly the catalog's update, got {:?}",
            reported
        );
        assert!(
            reply.marker().is_none(),
            "case {case}: a single-record lookup paginates nothing, got marker {:?}",
            reply.marker()
        );

        // The name and the status filter combine: the update's own status keeps
        // it, another status leaves the page empty.
        let mut pairs = params(&[("ServiceUpdateName", &update.name)]);
        pairs.extend(status_filter(&[update.status.as_str()]));
        let (_, kept) = describe(&emulator, &case, &pairs).await;
        assert_eq!(
            kept.len(),
            1,
            "case {case}: filtering on the update's own status must keep it, got {kept:?}"
        );

        let other = if update.status == state::SERVICE_UPDATE_AVAILABLE {
            state::SERVICE_UPDATE_EXPIRED
        } else {
            state::SERVICE_UPDATE_AVAILABLE
        };
        let mut pairs = params(&[("ServiceUpdateName", &update.name)]);
        pairs.extend(status_filter(&[other]));
        let (_, dropped) = describe(&emulator, &case, &pairs).await;
        assert!(
            dropped.is_empty(),
            "case {case}: the update is {}, so filtering on {other} must report none, got {:?}",
            update.status,
            dropped
        );
    }
}

#[tokio::test]
async fn a_name_no_published_update_carries_is_refused_as_not_found() {
    let emulator = Emulator::new();
    let published = catalog(&emulator, "unfiltered").await;
    let real = published
        .first()
        .expect("the catalog publishes at least one update")
        .name
        .clone();

    // The axis: how the name misses — a name of another form, one of the right
    // form released on a date nothing was, the right name in the wrong case,
    // the right name with whitespace, and the empty name.
    let rows = [
        "no-such-update".to_string(),
        "elasticache-19700101-001".to_string(),
        real.to_uppercase(),
        format!("{real} "),
        String::new(),
    ];
    for name in rows {
        let case = format!("missing name {name:?}");
        let reply = emulator
            .call("DescribeServiceUpdates", &params(&[("ServiceUpdateName", &name)]))
            .await;
        assert_eq!(
            reply.status, 404,
            "case {case}: must be refused with HTTP 404, got {}: {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ServiceUpdateNotFoundFault",
            "case {case}: must be refused with ServiceUpdateNotFoundFault, got {:?}",
            reply.error_code()
        );
    }
}

/// Walk every page of a describe, asserting each page holds at most
/// `max_records` updates and that the marker is opaque, and return the updates
/// in the order the pages reported them.
async fn walk_pages(
    emulator: &Emulator,
    case: &str,
    max_records: Option<i32>,
    statuses: &[&str],
) -> Vec<WireUpdate> {
    let mut collected: Vec<WireUpdate> = Vec::new();
    let mut marker: Option<String> = None;
    for page_number in 1..=32 {
        let mut pairs = status_filter(statuses);
        if let Some(size) = max_records {
            pairs.push(("MaxRecords".to_string(), size.to_string()));
        }
        if let Some(token) = &marker {
            pairs.push(("Marker".to_string(), token.clone()));
        }
        let (reply, page) = describe(emulator, case, &pairs).await;
        if let Some(size) = max_records {
            assert!(
                page.len() <= usize::try_from(size).expect("a positive page size"),
                "case {case}: page {page_number} must hold at most {size} updates, got {}",
                page.len()
            );
        }
        collected.extend(page);
        match reply.marker() {
            Some(token) => {
                assert!(
                    !token.is_empty() && token.chars().all(|c| c.is_ascii_hexdigit()),
                    "case {case}: the Marker must be an opaque token, got {token:?}"
                );
                assert!(
                    !collected
                        .iter()
                        .any(|update| token.contains(update.name.as_str())),
                    "case {case}: the Marker must not spell out a record's name, got {token:?}"
                );
                marker = Some(token.to_string());
            }
            None => return collected,
        }
    }
    panic!("case {case}: pagination did not terminate");
}

#[tokio::test]
async fn pagination_covers_the_catalog_at_every_page_size() {
    let emulator = Emulator::new();
    let published = catalog(&emulator, "unfiltered").await;
    let total = i32::try_from(published.len()).expect("a small catalog");

    // The axis: the page size, from one record per page to more than the
    // catalog holds, plus the request that names no MaxRecords at all.
    let mut rows: Vec<Option<i32>> = (1..=total + 1).map(Some).collect();
    rows.push(None);
    for max_records in rows {
        let case = format!("MaxRecords {max_records:?}");
        let walked = walk_pages(&emulator, &case, max_records, &[]).await;
        assert_eq!(
            walked, published,
            "case {case}: walking the pages must report the whole catalog, in order"
        );
    }

    // Pagination and the status filter compose: a filtered walk reports the
    // filtered catalog.
    for status in [
        state::SERVICE_UPDATE_AVAILABLE,
        state::SERVICE_UPDATE_EXPIRED,
    ] {
        let case = format!("MaxRecords 1 filtered on {status}");
        let expected: Vec<WireUpdate> = published
            .iter()
            .filter(|update| update.status == status)
            .cloned()
            .collect();
        let walked = walk_pages(&emulator, &case, Some(1), &[status]).await;
        assert_eq!(
            walked, expected,
            "case {case}: a filtered walk must report every {status} update, in order"
        );
    }
}

#[tokio::test]
async fn a_value_the_describe_cannot_serve_is_refused_with_invalid_parameter_value() {
    let emulator = Emulator::new();

    // The axis: which parameter carries a value the service cannot serve — a
    // page size that is no page, and a marker this service never minted.
    let rows: [(&str, &[(&str, &str)]); 4] = [
        ("MaxRecords 0", &[("MaxRecords", "0")]),
        ("MaxRecords -5", &[("MaxRecords", "-5")]),
        ("marker not minted here", &[("Marker", "not-a-marker")]),
        ("marker of an odd length", &[("Marker", "zzzz")]),
    ];
    for (case, pairs) in rows {
        let reply = emulator
            .call("DescribeServiceUpdates", &params(pairs))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case}: must be refused with HTTP 400, got {}: {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterValue",
            "case {case}: must be refused with InvalidParameterValue, got {:?}: {}",
            reply.error_code(),
            reply.body
        );
    }
}

#[tokio::test]
async fn a_status_outside_the_modelled_enum_is_refused_at_the_boundary() {
    let emulator = Emulator::new();

    // `ServiceUpdateStatus`'s member is a modelled enum, so a filter naming a
    // value outside `available | cancelled | expired` — including a real value
    // in the wrong case — never reaches the handler: the boundary validator
    // refuses it. The axis is how the value misses the enum.
    let rows = [
        ("status outside the enum", "pending"),
        ("status in the wrong case", "AVAILABLE"),
        ("empty status", ""),
    ];
    for (case, status) in rows {
        let reply = emulator
            .call(
                "DescribeServiceUpdates",
                &params(&[("ServiceUpdateStatus.member.1", status)]),
            )
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case}: must be refused with HTTP 400, got {}: {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ValidationError",
            "case {case}: must be refused as a validation error, got {:?}: {}",
            reply.error_code(),
            reply.body
        );
    }
}

#[tokio::test]
async fn describing_the_catalog_twice_reports_the_same_updates() {
    let emulator = Emulator::new();
    // A describe is a read: the second call sees exactly what the first did,
    // dates and statuses included.
    let first = catalog(&emulator, "first read").await;
    let second = catalog(&emulator, "second read").await;
    assert_eq!(
        first, second,
        "case repeated read: a describe must not change what a later caller sees"
    );
}
