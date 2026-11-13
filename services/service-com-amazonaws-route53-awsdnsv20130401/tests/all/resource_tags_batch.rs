// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `ListTagsForResources`, the batch tag read.
//!
//! The rule under test: a batch tag read answers one `ResourceTagSet` per
//! `ResourceId` the request names, in the order the request named them, each
//! carrying that resource's WHOLE tag set as the single-resource read reports
//! it; an id that names no resource of the request's `ResourceType` refuses the
//! whole request with that type's not-found error. So the tables below differ
//! along what that rule generalises over: how many ids a batch names, which
//! ORDER it names them in, how many tags each named resource carries, which of
//! the two taggable resource types the batch addresses, which spelling an id
//! arrives in, and where in the batch a missing id sits — never merely along
//! which zone is read.
//!
//! Every test drives the service through the real router, using the SAME
//! harness the other tag suite uses, so what is asserted is the HTTP/restXml
//! behaviour a Route 53 client sees.

use smithy_server_runtime::MultiServiceRouter;

use crate::health_checks::{create_health_check, health_check_id};
use crate::hosted_zone_records::{call, create_zone_ok, escape_xml, router, Reply};
use crate::resource_tags::{change_tags, element_text, list_tags, tag_pairs_in};

// ---------------------------------------------------------------------------
// Request builders and response readers
// ---------------------------------------------------------------------------

/// A `ListTagsForResources` body naming each id, in the order given.
///
/// `TagResourceIdList`'s member carries `xmlName: ResourceId`, so the ids are
/// `<ResourceId>` elements inside the `<ResourceIds>` container
/// (smithy_query(query="shape",shape="TagResourceIdList")).
///
/// `pub(crate)` so a sibling suite sends a batch through the SAME request rather
/// than growing a second copy of it.
pub(crate) fn list_tags_batch_body(resource_ids: &[&str]) -> String {
    let ids: String = resource_ids
        .iter()
        .map(|id| format!("<ResourceId>{}</ResourceId>", escape_xml(id)))
        .collect();
    format!(
        "<ListTagsForResourcesRequest xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
         <ResourceIds>{ids}</ResourceIds></ListTagsForResourcesRequest>"
    )
}

/// Read the tags of a batch of resources of one type.
///
/// `pub(crate)` for the same reason [`list_tags_batch_body`] is.
pub(crate) async fn list_tags_batch(
    router: &MultiServiceRouter,
    resource_type: &str,
    resource_ids: &[&str],
) -> Reply {
    call(
        router,
        "POST",
        &format!("/2013-04-01/tags/{resource_type}"),
        &list_tags_batch_body(resource_ids),
    )
    .await
}

/// One `ResourceTagSet` of a batch reply.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TagSet {
    resource_type: String,
    resource_id: String,
    tags: Vec<(String, String)>,
}

/// Every `<ResourceTagSet>` of a reply, in document order.
///
/// Parsed per BLOCK rather than through the harness' flattener, because what is
/// under test is which tags belong to which tag set, which a flat list of
/// `(path, text)` pairs no longer says.
fn tag_sets(reply: &Reply) -> Vec<TagSet> {
    const OPEN: &str = "<ResourceTagSet>";
    const CLOSE: &str = "</ResourceTagSet>";

    let mut sets = Vec::new();
    let mut rest = reply.body.as_str();
    while let Some(start) = rest.find(OPEN) {
        let block_start = start + OPEN.len();
        let end = rest[block_start..]
            .find(CLOSE)
            .map(|offset| block_start + offset)
            .unwrap_or_else(|| {
                panic!("unterminated <ResourceTagSet> element in {}", reply.body)
            });
        let block = &rest[block_start..end];
        sets.push(TagSet {
            resource_type: element_text(block, "ResourceType").unwrap_or_else(|| {
                panic!(
                    "a <ResourceTagSet> must name its ResourceType; body {}",
                    reply.body
                )
            }),
            resource_id: element_text(block, "ResourceId").unwrap_or_else(|| {
                panic!(
                    "a <ResourceTagSet> must name its ResourceId; body {}",
                    reply.body
                )
            }),
            tags: tag_pairs_in(block),
        });
        rest = &rest[end + CLOSE.len()..];
    }
    sets
}

/// The ids a batch reply reports, in the order it reported them.
fn reported_ids(reply: &Reply) -> Vec<String> {
    tag_sets(reply)
        .into_iter()
        .map(|set| set.resource_id)
        .collect()
}

/// The tag set of `count` tags a resource is given below, in the tag-key order a
/// tag read reports.
///
/// Keys `k0`..`k9`, so a count up to the ten tags a resource may carry keeps
/// lexicographic key order the same as the order they were added in.
fn expected_tags(count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|index| (format!("k{index}"), index.to_string()))
        .collect()
}

/// Tag `resource_id` with [`expected_tags`]`(count)` in one batch.
async fn tag_with(
    router: &MultiServiceRouter,
    resource_type: &str,
    resource_id: &str,
    count: usize,
) {
    if count == 0 {
        return;
    }
    let owned = expected_tags(count);
    let add: Vec<(&str, &str)> = owned
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let changed = change_tags(router, resource_type, resource_id, &add, &[]).await;
    assert_eq!(
        changed.status, 200,
        "tagging {resource_id} with {count} tag(s): expected 200, got {} with body {}",
        changed.status, changed.body
    );
}

/// `ids` as the borrowed slice the request builders take.
fn borrowed(ids: &[String]) -> Vec<&str> {
    ids.iter().map(String::as_str).collect()
}

// ---------------------------------------------------------------------------
// One tag set per requested id, in request order
// ---------------------------------------------------------------------------

/// How many tags each of the zones a batch is read over carries. The counts
/// span none, one, several and the ten a resource may carry, so a row's
/// expected tag set is not merely a different zone's.
const ZONE_TAG_COUNTS: [usize; 10] = [0, 1, 2, 10, 3, 0, 1, 5, 0, 10];

/// One row of the batch table: which of the created zones the batch names, by
/// index into [`ZONE_TAG_COUNTS`], in the order it names them.
#[derive(Debug, Clone, Copy)]
struct BatchRow {
    case: &'static str,
    indexes: &'static [usize],
}

#[tokio::test]
async fn a_batch_reports_one_tag_set_per_requested_id_in_request_order() {
    let rows = [
        BatchRow {
            // `TagResourceIdList` is `len 1..10`, so one id is the low bound.
            case: "one id, of an untagged resource",
            indexes: &[0],
        },
        BatchRow {
            case: "one id, of a resource carrying the ten tags a resource may carry",
            indexes: &[3],
        },
        BatchRow {
            case: "two ids",
            indexes: &[1, 2],
        },
        BatchRow {
            case: "the same two ids, named the other way round",
            indexes: &[2, 1],
        },
        BatchRow {
            case: "an untagged, a singly tagged and a fully tagged resource",
            indexes: &[0, 1, 3],
        },
        BatchRow {
            case: "the same three ids, named in reverse",
            indexes: &[3, 1, 0],
        },
        BatchRow {
            // The high bound of `TagResourceIdList`.
            case: "ten ids",
            indexes: &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
        },
        BatchRow {
            case: "the same ten ids, named in reverse",
            indexes: &[9, 8, 7, 6, 5, 4, 3, 2, 1, 0],
        },
    ];

    let router = router();
    let mut zones = Vec::new();
    for (index, count) in ZONE_TAG_COUNTS.iter().enumerate() {
        let zone = create_zone_ok(
            &router,
            &format!("batch-{index}.example.com"),
            &format!("tag-batch-{index}"),
        )
        .await;
        tag_with(&router, "hostedzone", &zone, *count).await;
        zones.push(zone);
    }

    for row in rows {
        let requested: Vec<&str> = row.indexes.iter().map(|index| zones[*index].as_str()).collect();
        let reply = list_tags_batch(&router, "hostedzone", &requested).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200 from ListTagsForResources, got {} with body {}",
            row.case, reply.status, reply.body
        );

        let expected: Vec<TagSet> = row
            .indexes
            .iter()
            .map(|index| TagSet {
                resource_type: "hostedzone".to_string(),
                resource_id: zones[*index].clone(),
                tags: expected_tags(ZONE_TAG_COUNTS[*index]),
            })
            .collect();
        assert_eq!(
            tag_sets(&reply),
            expected,
            "case {:?}: the batch must answer one tag set per requested id, in request order, \
             each carrying that resource's whole tag set; body {}",
            row.case,
            reply.body
        );
    }

    // The order a batch answers in is the REQUEST's, not the store's: the store
    // holds zones in id order, so reading the ids in descending order proves the
    // answer is not simply sorted.
    let mut descending = zones.clone();
    descending.sort_by(|left, right| right.cmp(left));
    assert_ne!(
        descending,
        {
            let mut ascending = zones.clone();
            ascending.sort();
            ascending
        },
        "the zone ids must differ so the two orders are distinguishable; ids {zones:?}"
    );
    let reply = list_tags_batch(&router, "hostedzone", &borrowed(&descending)).await;
    assert_eq!(
        reported_ids(&reply),
        descending,
        "a batch named in descending id order must answer in that order; body {}",
        reply.body
    );
}

// ---------------------------------------------------------------------------
// Both taggable resource types
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_batch_reads_either_taggable_resource_type() {
    // The rule holds per `ResourceType`: `TagResourceType` names `healthcheck`
    // beside `hostedzone` (smithy_query(query="shape",shape="TagResourceType")),
    // and a batch of health checks answers by the same rule as a batch of zones.
    let counts = [0usize, 2, 10];

    let router = router();
    let mut checks = Vec::new();
    for (index, count) in counts.iter().enumerate() {
        let created = create_health_check(
            &router,
            &format!("tag-batch-check-{index}"),
            &format!(
                "<Type>HTTPS</Type><FullyQualifiedDomainName>check-{index}.example.com\
                 </FullyQualifiedDomainName><Port>443</Port>"
            ),
        )
        .await;
        let id = health_check_id(&created, "a health check of the batch");
        tag_with(&router, "healthcheck", &id, *count).await;
        checks.push(id);
    }

    let reply = list_tags_batch(&router, "healthcheck", &borrowed(&checks)).await;
    assert_eq!(
        reply.status, 200,
        "expected 200 from a health-check batch, got {} with body {}",
        reply.status, reply.body
    );
    let expected: Vec<TagSet> = checks
        .iter()
        .zip(counts)
        .map(|(id, count)| TagSet {
            resource_type: "healthcheck".to_string(),
            resource_id: id.clone(),
            tags: expected_tags(count),
        })
        .collect();
    assert_eq!(
        tag_sets(&reply),
        expected,
        "a health-check batch must report each check's own tag set; body {}",
        reply.body
    );

    // A zone id is not a health-check id: the batch resolves against the type
    // its URI names, so a zone read as a health check is a missing health check.
    let zone = create_zone_ok(&router, "not-a-check.example.com", "tag-batch-wrong-type").await;
    let crossed = list_tags_batch(&router, "healthcheck", &[zone.as_str()]).await;
    assert_eq!(
        (crossed.status, crossed.error_code()),
        (404, "NoSuchHealthCheck"),
        "a zone id read as a health check must be a missing health check; body {}",
        crossed.body
    );
}

// ---------------------------------------------------------------------------
// The batch agrees with the single-resource read
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_batch_entry_is_what_the_single_resource_read_reports() {
    // The rule: a batch read reports each resource's tag set exactly as
    // `ListTagsForResource` reports it — and, being a read, leaves every tag set
    // as it was, so reading twice answers the same both times.
    let router = router();
    let mut zones = Vec::new();
    for (index, count) in [0usize, 1, 4].iter().enumerate() {
        let zone = create_zone_ok(
            &router,
            &format!("agree-{index}.example.com"),
            &format!("tag-batch-agree-{index}"),
        )
        .await;
        tag_with(&router, "hostedzone", &zone, *count).await;
        zones.push(zone);
    }

    let first = list_tags_batch(&router, "hostedzone", &borrowed(&zones)).await;
    let sets = tag_sets(&first);
    assert_eq!(
        sets.len(),
        zones.len(),
        "the batch must answer every requested id; body {}",
        first.body
    );

    for (zone, set) in zones.iter().zip(&sets) {
        let single = list_tags(&router, "hostedzone", zone).await;
        assert_eq!(
            single.status, 200,
            "zone {zone}: expected 200 from ListTagsForResource, got {} with body {}",
            single.status, single.body
        );
        assert_eq!(
            (set.resource_id.as_str(), set.tags.clone()),
            (zone.as_str(), tag_pairs_in(&single.body)),
            "zone {zone}: the batch entry must be what the single-resource read reports; \
             bodies {} / {}",
            first.body,
            single.body
        );
    }

    let again = list_tags_batch(&router, "hostedzone", &borrowed(&zones)).await;
    assert_eq!(
        tag_sets(&again),
        sets,
        "reading a batch must not change what a later read sees; bodies {} / {}",
        first.body,
        again.body
    );
}

// ---------------------------------------------------------------------------
// How an id may be spelled
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_zone_id_is_taken_bare_or_prefixed_and_reported_bare() {
    // The rule: a zone is addressable by the bare id or by the
    // `/hostedzone/`-prefixed `Id` a response handed back, and a tag set names
    // the resource's OWN id rather than the spelling it was addressed by.
    let router = router();
    let zone = create_zone_ok(&router, "spelling.example.com", "tag-batch-spelling").await;
    tag_with(&router, "hostedzone", &zone, 2).await;

    let prefixed = format!("/hostedzone/{zone}");
    let unslashed = format!("hostedzone/{zone}");
    for (case, spelling) in [
        ("the bare id", zone.clone()),
        ("the prefixed Id a response hands back", prefixed),
        ("the prefix without its leading slash", unslashed),
    ] {
        let reply = list_tags_batch(&router, "hostedzone", &[spelling.as_str()]).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            tag_sets(&reply),
            vec![TagSet {
                resource_type: "hostedzone".to_string(),
                resource_id: zone.clone(),
                tags: expected_tags(2),
            }],
            "case {case:?}: the tag set must name the zone's own id; body {}",
            reply.body
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// One row of the missing-id table: where in the batch the unknown id sits.
#[derive(Debug, Clone)]
struct MissingRow {
    case: &'static str,
    resource_type: &'static str,
    /// The ids to name, with `{known}` standing for the existing resource's id.
    ids: Vec<&'static str>,
    code: &'static str,
    /// The id the refusal must name.
    missing: &'static str,
}

#[tokio::test]
async fn an_id_that_names_no_resource_refuses_the_whole_batch() {
    // The rule: a batch resolves every id it names before it answers, and
    // reports an id that names no resource through the not-found error of the
    // resource TYPE the request addresses — the same refusal the
    // single-resource read answers it with, wherever in the batch the id sits —
    // rather than dropping the entry from the answer.
    let rows = [
        MissingRow {
            case: "the only id names no zone",
            resource_type: "hostedzone",
            ids: vec!["ZNOSUCHZONE0001"],
            code: "NoSuchHostedZone",
            missing: "ZNOSUCHZONE0001",
        },
        MissingRow {
            case: "the first of two ids names no zone",
            resource_type: "hostedzone",
            ids: vec!["ZNOSUCHZONE0002", "{known}"],
            code: "NoSuchHostedZone",
            missing: "ZNOSUCHZONE0002",
        },
        MissingRow {
            case: "the last of two ids names no zone",
            resource_type: "hostedzone",
            ids: vec!["{known}", "ZNOSUCHZONE0003"],
            code: "NoSuchHostedZone",
            missing: "ZNOSUCHZONE0003",
        },
        MissingRow {
            case: "an id in the middle of a batch names no zone",
            resource_type: "hostedzone",
            ids: vec!["{known}", "ZNOSUCHZONE0004", "{known}"],
            code: "NoSuchHostedZone",
            missing: "ZNOSUCHZONE0004",
        },
        MissingRow {
            case: "an id names no health check",
            resource_type: "healthcheck",
            ids: vec!["8f3b7a1e-0c2d-4f5a-9b6c-1d2e3f4a5b6c"],
            code: "NoSuchHealthCheck",
            missing: "8f3b7a1e-0c2d-4f5a-9b6c-1d2e3f4a5b6c",
        },
    ];

    let router = router();
    let known = create_zone_ok(&router, "known.example.com", "tag-batch-known").await;
    tag_with(&router, "hostedzone", &known, 1).await;

    for row in &rows {
        let ids: Vec<String> = row
            .ids
            .iter()
            .map(|id| id.replace("{known}", &known))
            .collect();
        let reply = list_tags_batch(&router, row.resource_type, &borrowed(&ids)).await;
        assert_eq!(
            reply.status, 404,
            "case {:?}: expected HTTP 404, got {} with body {}",
            row.case, reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            row.code,
            "case {:?}: expected error code {}, got body {}",
            row.case,
            row.code,
            reply.body
        );
        assert!(
            reply
                .first("Error/Message")
                .is_some_and(|message| message.contains(row.missing)),
            "case {:?}: the refusal must name the id that resolved to nothing; body {}",
            row.case,
            reply.body
        );
        assert!(
            tag_sets(&reply).is_empty(),
            "case {:?}: a refused batch must answer no partial tag set; body {}",
            row.case,
            reply.body
        );
    }

    // None of those refusals changed the tag set of the zone they named.
    let still = list_tags_batch(&router, "hostedzone", &[known.as_str()]).await;
    assert_eq!(
        tag_sets(&still),
        vec![TagSet {
            resource_type: "hostedzone".to_string(),
            resource_id: known.clone(),
            tags: expected_tags(1),
        }],
        "a refused batch must leave the resources it named as they were; body {}",
        still.body
    );
}

/// One row of the batch-size table.
#[derive(Debug, Clone)]
struct SizeRow {
    case: &'static str,
    /// How many of the created zones to name; `0` sends an empty container.
    count: usize,
    status: u16,
}

#[tokio::test]
async fn the_number_of_ids_in_a_batch_is_bounded_by_the_model() {
    // The bound comes from the model, not from this test: `ResourceIds` targets
    // `TagResourceIdList`, which is `len 1..10`
    // (smithy_query(query="shape",shape="TagResourceIdList")). Each bound is
    // exercised AT the bound and one id past it.
    let rows = [
        SizeRow {
            case: "no id at all",
            count: 0,
            status: 400,
        },
        SizeRow {
            case: "one id, the minimum",
            count: 1,
            status: 200,
        },
        SizeRow {
            case: "ten ids, the maximum",
            count: 10,
            status: 200,
        },
        SizeRow {
            case: "eleven ids, one past the maximum",
            count: 11,
            status: 400,
        },
    ];

    let router = router();
    let mut zones = Vec::new();
    for index in 0..11 {
        let zone = create_zone_ok(
            &router,
            &format!("bound-{index}.example.com"),
            &format!("tag-batch-bound-{index}"),
        )
        .await;
        zones.push(zone);
    }

    for row in &rows {
        let ids = borrowed(&zones[..row.count]);
        let reply = list_tags_batch(&router, "hostedzone", &ids).await;
        assert_eq!(
            reply.status, row.status,
            "case {:?}: expected HTTP {}, got {} with body {}",
            row.case, row.status, reply.status, reply.body
        );
        let answered = if row.status == 200 { row.count } else { 0 };
        assert_eq!(
            tag_sets(&reply).len(),
            answered,
            "case {:?}: expected {answered} tag set(s) in the answer; body {}",
            row.case,
            reply.body
        );
    }
}
