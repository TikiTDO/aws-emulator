// SPDX-License-Identifier: CC0-1.0

//! The ORDER a domain's search answers its hits in, and the FRAGMENTS each hit carries —
//! driven over the real wire path: a signed request to the endpoint a domain publishes,
//! through the multi-service router, into the mounted service.
//!
//! Two rules, each asserted over a table of inputs rather than one call:
//!
//! > a search applies every section of its body the index's own mappings can serve: the
//! > `sort` orders the selected documents by the per-document values of the fields its keys
//! > name — a multi-field reading the values of the field above it — applies `from`/`size` to
//! > THAT order, and answers each hit's own sort values beside it, while a key naming a field
//! > the index holds no per-document values for is refused rather than ignored;
//!
//! > a `highlight` answers, on each hit, the field's own value with the terms the query asked
//! > about wrapped in the highlighter's tags, as that field's analysis chain reports them —
//! > exactly the matched token on an analyzed field and the whole value on a field held
//! > whole — and a requested field carrying no asked-for term contributes no key at all.
//!
//! The harness is [`super::domain_lifecycle`]'s and the data-plane request helper is
//! [`super::domain_rest_data_plane`]'s; this suite adds no second copy of either.

use serde_json::{json, Value};

use super::domain_lifecycle::{Answer, Emulator};
use super::domain_rest_data_plane::rest_prefixed;

/// The domain whose published endpoint every request below is addressed to.
const DOMAIN: &str = "search-order";

/// The index the ordering table is read over: a `text` field with a `keyword` sub-field, a
/// second one with its own `keyword` sibling, a plain `keyword`, a number, and a `text` field
/// whose mapping switches `fielddata` on.
const INDEX: &str = "library";

/// A second index, holding one long value of several sentences, which is what makes the
/// FRAGMENTS a highlight breaks a value into observable.
const LONGFORM: &str = "longform";

/// The long value that index holds: four sentences, two of them carrying the term the
/// highlight rows ask about.
const LONG_BODY: &str =
    "Alpha runs fast. The vorpal blade is sharp. Nothing here. A second vorpal appears.";

/// `CreateDomain`, then the two indexes and their documents.
async fn given_indexed(emulator: &Emulator) {
    let created = emulator
        .create(json!({
            "DomainName": DOMAIN,
            "ClusterConfig": { "InstanceType": "t3.small.search", "InstanceCount": 1 },
            "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 10 },
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "case CreateDomain({DOMAIN:?}): expected 200, got {} with {:?}",
        created.status, created.body
    );
    let index = put(
        emulator,
        &format!("/{INDEX}"),
        json!({
            "settings": { "number_of_shards": 1, "number_of_replicas": 0 },
            "mappings": { "properties": {
                "title": { "type": "text", "fields": { "raw": { "type": "keyword" } } },
                "body": { "type": "text", "fields": { "keyword": { "type": "keyword" } } },
                "shelf": { "type": "keyword" },
                "rank": { "type": "integer" },
                "notes": { "type": "text", "fielddata": true },
            } }
        }),
    )
    .await;
    assert_eq!(
        index.status, 200,
        "case PUT /{INDEX}: expected 200, got {} with {:?}",
        index.status, index.body
    );
    // Written in an order that is NOT the order any key below puts them in, so an answer in
    // insertion order is distinguishable from an ordered one.
    let documents: &[(&str, Value)] = &[
        (
            "d1",
            json!({
                "title": "gamma release",
                "body": "the quick brown vorpal jumps over the lazy dog",
                "shelf": "east",
                "rank": 3,
                "notes": "gamma notes",
            }),
        ),
        (
            "d2",
            json!({
                "title": "alpha release",
                "body": "a vorpal blade went snicker snack",
                "shelf": "west",
                "rank": 1,
                "notes": "alpha notes",
            }),
        ),
        (
            "d3",
            json!({
                "title": "beta release",
                "body": "nothing to see here",
                "shelf": "east",
                "notes": "beta notes",
            }),
        ),
    ];
    for (id, source) in documents {
        let written = put(emulator, &format!("/{INDEX}/_doc/{id}"), source.clone()).await;
        assert!(
            written.status == 200 || written.status == 201,
            "case write {INDEX}/{id}: expected 200 or 201, got {} with {:?}",
            written.status,
            written.body
        );
    }
    let longform = put(
        emulator,
        &format!("/{LONGFORM}"),
        json!({
            "settings": { "number_of_shards": 1, "number_of_replicas": 0 },
            "mappings": { "properties": { "body": { "type": "text" } } }
        }),
    )
    .await;
    assert_eq!(
        longform.status, 200,
        "case PUT /{LONGFORM}: expected 200, got {} with {:?}",
        longform.status, longform.body
    );
    let written = put(
        emulator,
        &format!("/{LONGFORM}/_doc/long"),
        json!({ "body": LONG_BODY }),
    )
    .await;
    assert!(
        written.status == 200 || written.status == 201,
        "case write {LONGFORM}/long: expected 200 or 201, got {} with {:?}",
        written.status,
        written.body
    );
}

/// One `PUT` to the domain's own data plane.
async fn put(emulator: &Emulator, path: &str, body: Value) -> Answer {
    rest_prefixed(emulator, DOMAIN, "PUT", path, Some(body)).await
}

/// One `POST /{index}/_search`.
async fn search(emulator: &Emulator, path: &str, body: Option<Value>) -> Answer {
    rest_prefixed(emulator, DOMAIN, "POST", path, body).await
}

/// The ids the hits of an answer carry, in the order the answer lists them.
fn hit_ids(answer: &Answer) -> Vec<String> {
    answer.body["hits"]["hits"]
        .as_array()
        .map(|hits| {
            hits.iter()
                .map(|hit| hit["_id"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The hit an answer carries for `id`, where it carries one.
fn hit_of(answer: &Answer, id: &str) -> Option<Value> {
    answer.body["hits"]["hits"]
        .as_array()?
        .iter()
        .find(|hit| hit["_id"].as_str() == Some(id))
        .cloned()
}

/// One ordering to ask for, and what it must answer.
struct OrderRow {
    case: &'static str,
    /// The path the request is sent to — the index's own search path, unless the row names
    /// its order in the query string.
    path: String,
    /// The search body, or `None` for one that carries none.
    body: Option<Value>,
    /// The hit ids, in the order the answer must list them.
    ids: Vec<&'static str>,
    /// The `sort` values the FIRST hit must carry, or `None` for a request that named no
    /// order and whose hits therefore carry none.
    first_sort: Option<Value>,
}

/// The ordering table: one row per way one sort differs from another — the direction, the
/// form the key is written in, the kind of field it names, the number of keys, a document
/// missing the key's value, the page taken out of the order, and no order at all.
fn order_rows() -> Vec<OrderRow> {
    vec![
        OrderRow {
            case: "ascending on a keyword sub-field",
            path: format!("/{INDEX}/_search"),
            body: Some(json!({ "sort": [{ "title.raw": "asc" }] })),
            ids: vec!["d2", "d3", "d1"],
            first_sort: Some(json!(["alpha release"])),
        },
        OrderRow {
            case: "descending on a keyword sub-field",
            path: format!("/{INDEX}/_search"),
            body: Some(json!({ "sort": [{ "title.raw": "desc" }] })),
            ids: vec!["d1", "d3", "d2"],
            first_sort: Some(json!(["gamma release"])),
        },
        OrderRow {
            case: "a key written as a bare field name",
            path: format!("/{INDEX}/_search"),
            body: Some(json!({ "sort": ["title.raw"] })),
            ids: vec!["d2", "d3", "d1"],
            first_sort: Some(json!(["alpha release"])),
        },
        OrderRow {
            case: "a key carrying its direction under `order`",
            path: format!("/{INDEX}/_search"),
            body: Some(json!({ "sort": [{ "title.raw": { "order": "desc" } }] })),
            ids: vec!["d1", "d3", "d2"],
            first_sort: Some(json!(["gamma release"])),
        },
        OrderRow {
            case: "two keys, the second breaking the first's tie",
            path: format!("/{INDEX}/_search"),
            body: Some(json!({ "sort": [{ "shelf": "asc" }, { "title.raw": "asc" }] })),
            ids: vec!["d3", "d1", "d2"],
            first_sort: Some(json!(["east", "beta release"])),
        },
        OrderRow {
            case: "a number ascending, with the document missing it last",
            path: format!("/{INDEX}/_search"),
            body: Some(json!({ "sort": [{ "rank": "asc" }] })),
            ids: vec!["d2", "d1", "d3"],
            first_sort: Some(json!([1])),
        },
        OrderRow {
            case: "a number descending, with the document missing it still last",
            path: format!("/{INDEX}/_search"),
            body: Some(json!({ "sort": [{ "rank": "desc" }] })),
            ids: vec!["d1", "d2", "d3"],
            first_sort: Some(json!([3])),
        },
        OrderRow {
            case: "a text field whose mapping switches fielddata on",
            path: format!("/{INDEX}/_search"),
            body: Some(json!({ "sort": [{ "notes": "asc" }] })),
            ids: vec!["d2", "d3", "d1"],
            first_sort: Some(json!(["alpha notes"])),
        },
        OrderRow {
            case: "from and size taken out of the order",
            path: format!("/{INDEX}/_search"),
            body: Some(json!({ "sort": [{ "title.raw": "asc" }], "from": 1, "size": 1 })),
            ids: vec!["d3"],
            first_sort: Some(json!(["beta release"])),
        },
        OrderRow {
            case: "the order named in the query string",
            path: format!("/{INDEX}/_search?sort=title.raw:desc"),
            body: None,
            ids: vec!["d1", "d3", "d2"],
            first_sort: Some(json!(["gamma release"])),
        },
        OrderRow {
            case: "an order applied to the documents a query selected",
            path: format!("/{INDEX}/_search"),
            body: Some(json!({
                "query": { "term": { "shelf": "east" } },
                "sort": [{ "title.raw": "asc" }]
            })),
            ids: vec!["d3", "d1"],
            first_sort: Some(json!(["beta release"])),
        },
        OrderRow {
            case: "no order at all, which keeps the scan's own sequence",
            path: format!("/{INDEX}/_search"),
            body: Some(json!({})),
            ids: vec!["d1", "d2", "d3"],
            first_sort: None,
        },
    ]
}

/// RULE: a search orders its hits by the per-document values of the fields its `sort` keys
/// name, takes `from`/`size` out of THAT order, and answers each hit's sort values beside it.
#[tokio::test]
async fn a_search_orders_its_hits_by_the_values_its_sort_keys_name() {
    let emulator = Emulator::new();
    given_indexed(&emulator).await;

    for row in order_rows() {
        let answered = search(&emulator, &row.path, row.body.clone()).await;
        assert_eq!(
            answered.status, 200,
            "case {:?}: expected 200, got {} with {:?}",
            row.case, answered.status, answered.body
        );
        let ids = hit_ids(&answered);
        assert_eq!(
            ids, row.ids,
            "case {:?}: expected the hits {:?}, got {ids:?}",
            row.case, row.ids
        );
        let first = answered.body["hits"]["hits"][0].clone();
        match &row.first_sort {
            Some(expected) => assert_eq!(
                &first["sort"], expected,
                "case {:?}: expected the hit's own sort values {expected:?}, got {:?}",
                row.case, first
            ),
            None => assert!(
                first.get("sort").is_none(),
                "case {:?}: expected no sort values on a hit of an unordered search, got {:?}",
                row.case,
                first
            ),
        }
    }

    // The whole selected set is ordered BEFORE the page is taken, so the pages of one order
    // partition the hits without repeating or dropping one.
    let mut paged: Vec<String> = Vec::new();
    for from in 0..3 {
        let page = search(
            &emulator,
            &format!("/{INDEX}/_search"),
            Some(json!({ "sort": [{ "title.raw": "asc" }], "from": from, "size": 1 })),
        )
        .await;
        paged.extend(hit_ids(&page));
    }
    assert_eq!(
        paged,
        vec!["d2".to_string(), "d3".to_string(), "d1".to_string()],
        "case \"paging through one order\": expected each document exactly once in order, got \
         {paged:?}"
    );

    // The reads above changed nothing a later caller sees.
    let after = search(&emulator, &format!("/{INDEX}/_search"), Some(json!({}))).await;
    assert_eq!(
        hit_ids(&after),
        vec!["d1".to_string(), "d2".to_string(), "d3".to_string()],
        "case \"a search after the ordered reads\": expected the same documents, got {:?}",
        after.body
    );
    // And each document reads back through the document API as it was written, so what was
    // ordered is what the index holds.
    let read = rest_prefixed(
        &emulator,
        DOMAIN,
        "GET",
        &format!("/{INDEX}/_doc/d1"),
        None,
    )
    .await;
    assert_eq!(
        read.body["_source"]["title"].as_str(),
        Some("gamma release"),
        "case \"reading an ordered document back\": expected the source the write carried, got \
         {:?}",
        read.body
    );
}

/// One sort this surface must refuse, or must serve, and what it must answer with.
struct RefusalRow {
    case: &'static str,
    /// The path the request is sent to — the index's own search path, unless the row states
    /// its order in the query string.
    path: String,
    /// The search body, or `None` for a row whose order is in the query string.
    body: Option<Value>,
    status: u16,
    /// The `error.type` the refusal carries, and the type of its root cause.
    error_type: &'static str,
    root_cause: &'static str,
    /// Text the reason must carry, naming what the caller has to change.
    reason: &'static str,
}

/// RULE: a sort key naming a field the index holds no per-document values for — an analyzed
/// `text` field with `fielddata` left off — is REFUSED, and so is a key naming a direction the
/// engine does not read; a key naming a field that does hold them is served.
#[tokio::test]
async fn a_sort_the_index_cannot_order_by_is_refused_rather_than_ignored() {
    let emulator = Emulator::new();
    given_indexed(&emulator).await;

    let rows: &[RefusalRow] = &[
        RefusalRow {
            case: "a sort on an analyzed text field",
            path: format!("/{INDEX}/_search"),
            body: Some(json!({ "sort": [{ "title": "asc" }] })),
            status: 400,
            error_type: "search_phase_execution_exception",
            root_cause: "illegal_argument_exception",
            reason: "fielddata=true on [title]",
        },
        RefusalRow {
            case: "a sort on a second analyzed text field, named with its direction",
            path: format!("/{INDEX}/_search"),
            body: Some(json!({ "sort": [{ "body": "desc" }] })),
            status: 400,
            error_type: "search_phase_execution_exception",
            root_cause: "illegal_argument_exception",
            reason: "fielddata=true on [body]",
        },
        RefusalRow {
            case: "an analyzed text field named in the query string",
            path: format!("/{INDEX}/_search?sort=title:asc"),
            body: None,
            status: 400,
            error_type: "search_phase_execution_exception",
            root_cause: "illegal_argument_exception",
            reason: "fielddata=true on [title]",
        },
        RefusalRow {
            case: "a sort key naming a direction the engine does not read",
            path: format!("/{INDEX}/_search"),
            body: Some(json!({ "sort": [{ "title.raw": { "order": "up" } }] })),
            status: 400,
            error_type: "x_content_parse_exception",
            root_cause: "x_content_parse_exception",
            reason: "[field_sort] failed to parse field [order]",
        },
        RefusalRow {
            case: "that same direction named in the query string",
            path: format!("/{INDEX}/_search?sort=title.raw:up"),
            body: None,
            status: 400,
            error_type: "x_content_parse_exception",
            root_cause: "x_content_parse_exception",
            reason: "[field_sort] failed to parse field [order]",
        },
    ];
    for row in rows {
        let answered = search(&emulator, &row.path, row.body.clone()).await;
        assert_eq!(
            answered.status, row.status,
            "case {:?}: expected {}, got {} with {:?}",
            row.case, row.status, answered.status, answered.body
        );
        assert_eq!(
            answered.body["error"]["type"].as_str(),
            Some(row.error_type),
            "case {:?}: expected the error type {}, got {:?}",
            row.case,
            row.error_type,
            answered.body
        );
        assert_eq!(
            answered.body["error"]["root_cause"][0]["type"].as_str(),
            Some(row.root_cause),
            "case {:?}: expected the root cause {}, got {:?}",
            row.case,
            row.root_cause,
            answered.body
        );
        let reason = answered.body["error"]["root_cause"][0]["reason"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            reason.contains(row.reason),
            "case {:?}: expected the reason to name {:?}, got {reason:?}",
            row.case,
            row.reason
        );
    }

    // The sub-field of that same text field DOES hold per-document values, so the same
    // search ordered by it is served — which is what the refusal above points the caller at.
    let served = search(
        &emulator,
        &format!("/{INDEX}/_search"),
        Some(json!({ "sort": [{ "title.raw": "asc" }] })),
    )
    .await;
    assert_eq!(
        (served.status, hit_ids(&served)),
        (200, vec!["d2".to_string(), "d3".to_string(), "d1".to_string()]),
        "case \"the keyword sub-field of the refused field\": expected the ordered hits, got \
         {:?}",
        served.body
    );

    // A query composing more clauses than the engine evaluates is refused on its BREADTH,
    // whatever those clauses select: a clause that selects every document is still asked of
    // every document. The rows span the clause this is easiest to compose many of and the
    // one a query ordinarily carries.
    for (case, clause) in [
        ("clauses that select every document", json!({ "match_all": {} })),
        ("clauses that select by a term", json!({ "term": { "shelf": "east" } })),
    ] {
        let many: Vec<Value> = (0..2_000).map(|_| clause.clone()).collect();
        let refused = search(
            &emulator,
            &format!("/{INDEX}/_search"),
            Some(json!({ "query": { "bool": { "must": many } } })),
        )
        .await;
        assert_eq!(
            (
                refused.status,
                refused.body["error"]["type"].as_str(),
                refused.body["error"]["reason"].as_str(),
            ),
            (400, Some("too_many_clauses"), Some("maxClauseCount is set to 1024")),
            "case {case:?}: expected the engine's clause-ceiling refusal, got {:?}",
            refused.body
        );
    }
}

/// RULE: one sort key whose documents hold values of SEVERAL kinds still orders them one
/// way — the order is total, so such a page is answered rather than refused, and the same
/// request answers the same page every time.
#[tokio::test]
async fn a_sort_key_whose_documents_hold_several_kinds_of_value_orders_them_one_way() {
    let emulator = Emulator::new();
    given_indexed(&emulator).await;

    // No mappings at all, so the field takes whatever a write holds at it — which is how one
    // key comes to hold values of several kinds.
    let created = put(
        &emulator,
        "/mixed",
        json!({ "settings": { "number_of_shards": 1, "number_of_replicas": 0 } }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case PUT /mixed: expected 200, got {} with {:?}",
        created.status, created.body
    );
    // (the id, what it holds at the key) — the pair 9/10/"5" is the one that orders
    // differently as numbers than as text, and the document holding nothing at all has no
    // value for the key at all.
    let documents: &[(&str, Value)] = &[
        ("m1", json!({ "n": 9 })),
        ("m2", json!({ "n": 10 })),
        ("m3", json!({ "n": "5" })),
        ("m4", json!({ "n": true })),
        ("m5", json!({ "other": 1 })),
    ];
    for (id, source) in documents {
        let written = put(&emulator, &format!("/mixed/_doc/{id}"), source.clone()).await;
        assert!(
            written.status == 200 || written.status == 201,
            "case write mixed/{id}: expected 200 or 201, got {} with {:?}",
            written.status,
            written.body
        );
    }
    // (the case, the direction, the ids in order) — the kinds order one way, the values
    // within a kind order by the direction, and the document holding no value is last either
    // way.
    let rows: &[(&str, &str, [&str; 5])] = &[
        (
            "ascending over several kinds",
            "asc",
            ["m4", "m1", "m2", "m3", "m5"],
        ),
        (
            "descending over several kinds",
            "desc",
            ["m3", "m2", "m1", "m4", "m5"],
        ),
    ];
    for (case, direction, expected) in rows {
        // Asked twice, because an order that is not a total order can answer two different
        // pages for one request.
        for attempt in 0..2 {
            let answered = search(
                &emulator,
                "/mixed/_search",
                Some(json!({ "sort": [{ "n": direction }] })),
            )
            .await;
            assert_eq!(
                (answered.status, hit_ids(&answered)),
                (
                    200,
                    expected.iter().map(|id| (*id).to_string()).collect::<Vec<_>>()
                ),
                "case {case:?} (attempt {attempt}): expected the kinds ordered one way, got {:?}",
                answered.body
            );
        }
    }
}

/// One highlight to ask for, and what each hit must carry.
struct HighlightRow {
    case: &'static str,
    path: String,
    body: Value,
    /// The hit id, and the `highlight` object it must carry — `None` for a hit that must
    /// carry no `highlight` member at all.
    expected: Vec<(&'static str, Option<Value>)>,
}

/// The highlight table: one row per way one request's fragments differ from another's — the
/// kind of field asked for, the clause that asked, `require_field_match`, the tags, a pattern
/// naming the field, a field nothing asked about, the number of fragments and their size.
fn highlight_rows() -> Vec<HighlightRow> {
    vec![
        HighlightRow {
            case: "a match on an analyzed text field, with its keyword sibling also asked for",
            path: format!("/{INDEX}/_search"),
            body: json!({
                "query": { "match": { "body": "vorpal" } },
                "highlight": { "fields": { "body": {}, "body.keyword": {} } }
            }),
            expected: vec![
                (
                    "d1",
                    Some(json!({
                        "body": ["the quick brown <em>vorpal</em> jumps over the lazy dog"]
                    })),
                ),
                (
                    "d2",
                    Some(json!({ "body": ["a <em>vorpal</em> blade went snicker snack"] })),
                ),
            ],
        },
        HighlightRow {
            case: "the keyword sibling alone, with require_field_match off",
            path: format!("/{INDEX}/_search"),
            body: json!({
                "query": { "match": { "body": "vorpal" } },
                "highlight": {
                    "require_field_match": false,
                    "fields": { "body.keyword": {} }
                }
            }),
            expected: vec![("d1", None), ("d2", None)],
        },
        HighlightRow {
            case: "an exact term on a field held whole",
            path: format!("/{INDEX}/_search"),
            body: json!({
                "query": { "term": { "body.keyword": "nothing to see here" } },
                "highlight": { "fields": { "body.keyword": {} } }
            }),
            expected: vec![(
                "d3",
                Some(json!({ "body.keyword": ["<em>nothing to see here</em>"] })),
            )],
        },
        HighlightRow {
            case: "the tags the request names instead of the defaults",
            path: format!("/{INDEX}/_search"),
            body: json!({
                "query": { "match": { "body": "snicker" } },
                "highlight": {
                    "pre_tags": ["<mark>"],
                    "post_tags": ["</mark>"],
                    "fields": { "body": {} }
                }
            }),
            expected: vec![(
                "d2",
                Some(json!({ "body": ["a vorpal blade went <mark>snicker</mark> snack"] })),
            )],
        },
        HighlightRow {
            case: "a pattern naming the field",
            path: format!("/{INDEX}/_search"),
            body: json!({
                "query": { "match": { "body": "snicker" } },
                "highlight": { "fields": { "bod*": {} } }
            }),
            expected: vec![(
                "d2",
                Some(json!({ "body": ["a vorpal blade went <em>snicker</em> snack"] })),
            )],
        },
        HighlightRow {
            case: "a field the query asked nothing about",
            path: format!("/{INDEX}/_search"),
            body: json!({
                "query": { "match": { "body": "snicker" } },
                "highlight": { "fields": { "shelf": {} } }
            }),
            expected: vec![("d2", None)],
        },
        HighlightRow {
            case: "a search carrying no highlight section at all",
            path: format!("/{INDEX}/_search"),
            body: json!({ "query": { "match": { "body": "snicker" } } }),
            expected: vec![("d2", None)],
        },
    ]
}

/// RULE: a `highlight` answers each hit's own value with the terms the query asked about
/// wrapped in the highlighter's tags, and a requested field carrying no such term contributes
/// no key.
#[tokio::test]
async fn a_highlight_wraps_the_terms_the_query_asked_each_field_about() {
    let emulator = Emulator::new();
    given_indexed(&emulator).await;

    for row in highlight_rows() {
        let answered = search(&emulator, &row.path, Some(row.body.clone())).await;
        assert_eq!(
            answered.status, 200,
            "case {:?}: expected 200, got {} with {:?}",
            row.case, answered.status, answered.body
        );
        assert_eq!(
            hit_ids(&answered).len(),
            row.expected.len(),
            "case {:?}: expected {} hits, got {:?}",
            row.case,
            row.expected.len(),
            answered.body
        );
        for (id, expected) in &row.expected {
            let hit = hit_of(&answered, id).unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected a hit for {id}, got {:?}",
                    row.case, answered.body
                )
            });
            match expected {
                Some(highlight) => assert_eq!(
                    &hit["highlight"], highlight,
                    "case {:?}: expected {id} to carry {highlight:?}, got {hit:?}",
                    row.case
                ),
                None => assert!(
                    hit.get("highlight").is_none(),
                    "case {:?}: expected {id} to carry no highlight, got {hit:?}",
                    row.case
                ),
            }
        }
    }

    // The fragments one long value is broken into: the default size takes the whole value,
    // a smaller one breaks it into the passages carrying a matched term, a count of one keeps
    // the first of them, and a count of zero answers the whole field as one fragment.
    // (the case, the field's options, how many fragments, what each must carry)
    let fragment_rows: &[(&str, Value, usize)] = &[
        ("the default fragment size over the whole value", json!({}), 1),
        (
            "a fragment size shorter than the value",
            json!({ "fragment_size": 20 }),
            2,
        ),
        (
            "a count of one over that same size",
            json!({ "fragment_size": 20, "number_of_fragments": 1 }),
            1,
        ),
        (
            "a count of zero, which is the whole field as one fragment",
            json!({ "fragment_size": 20, "number_of_fragments": 0 }),
            1,
        ),
    ];
    for (case, options, expected) in fragment_rows {
        let answered = search(
            &emulator,
            &format!("/{LONGFORM}/_search"),
            Some(json!({
                "query": { "match": { "body": "vorpal" } },
                "highlight": { "fields": { "body": options.clone() } }
            })),
        )
        .await;
        let fragments = answered.body["hits"]["hits"][0]["highlight"]["body"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            fragments.len(),
            *expected,
            "case {case:?}: expected {expected} fragments, got {fragments:?}"
        );
        for fragment in &fragments {
            let text = fragment.as_str().unwrap_or_default();
            assert!(
                text.contains("<em>vorpal</em>"),
                "case {case:?}: expected every fragment to wrap the matched term, got {text:?}"
            );
            assert!(
                LONG_BODY.contains(&text.replace("<em>", "").replace("</em>", "")),
                "case {case:?}: expected the fragment to be the value's own text, got {text:?}"
            );
        }
    }

    // A count of zero answers the whole value, with BOTH occurrences wrapped — the value as
    // the document holds it, and nothing else.
    let whole = search(
        &emulator,
        &format!("/{LONGFORM}/_search"),
        Some(json!({
            "query": { "match": { "body": "vorpal" } },
            "highlight": { "fields": { "body": { "number_of_fragments": 0 } } }
        })),
    )
    .await;
    assert_eq!(
        whole.body["hits"]["hits"][0]["highlight"]["body"][0].as_str(),
        Some(LONG_BODY.replace("vorpal", "<em>vorpal</em>").as_str()),
        "case \"the whole field as one fragment\": expected every occurrence wrapped in place, \
         got {:?}",
        whole.body
    );
}

/// RULE: what one request's fragments may ASK FOR is bounded — a tag, a field pattern and the
/// bytes one hit's fragments materialise — so a request cannot choose how much one search
/// answers with.
#[tokio::test]
async fn what_one_hit_s_fragments_may_materialise_is_bounded() {
    let emulator = Emulator::new();
    given_indexed(&emulator).await;

    // (the case, the highlight section, the member the refusal must name)
    let refused: &[(&str, Value, &str)] = &[
        (
            "a tag longer than any the highlighter wraps with",
            json!({ "pre_tags": ["x".repeat(65)], "fields": { "body": {} } }),
            "pre_tags",
        ),
        (
            "that same tag named on one field rather than the section",
            json!({ "fields": { "body": { "post_tags": ["y".repeat(1_000)] } } }),
            "post_tags",
        ),
        (
            "a field pattern longer than one may be",
            json!({ "fields": { format!("{}*", "b".repeat(300)): {} } }),
            "pattern",
        ),
    ];
    for (case, highlight, named) in refused {
        let answered = search(
            &emulator,
            &format!("/{INDEX}/_search"),
            Some(json!({
                "query": { "match": { "body": "vorpal" } },
                "highlight": highlight.clone()
            })),
        )
        .await;
        let reason = answered.body["error"]["reason"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            (answered.status, answered.body["error"]["type"].as_str()),
            (400, Some("illegal_argument_exception")),
            "case {case:?}: expected the refusal of a value past its bound, got {:?}",
            answered.body
        );
        assert!(
            reason.contains(named),
            "case {case:?}: expected the reason to name {named:?}, got {reason:?}"
        );
    }

    // A value of very many matched terms answers fragments, and only as many bytes of them as
    // one hit may carry — asked for in the one form that asks for a whole field at once.
    let repeated = "vorpal ".repeat(40_000);
    let created = put(
        &emulator,
        "/bulky",
        json!({
            "settings": { "number_of_shards": 1, "number_of_replicas": 0 },
            "mappings": { "properties": { "body": { "type": "text" } } }
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case PUT /bulky: expected 200, got {} with {:?}",
        created.status, created.body
    );
    let written = put(
        &emulator,
        "/bulky/_doc/one",
        json!({ "body": repeated.clone() }),
    )
    .await;
    assert!(
        written.status == 200 || written.status == 201,
        "case write bulky/one: expected 200 or 201, got {} with {:?}",
        written.status,
        written.body
    );
    let answered = search(
        &emulator,
        "/bulky/_search",
        Some(json!({
            "query": { "match": { "body": "vorpal" } },
            "highlight": { "fields": { "body": { "number_of_fragments": 0 } } }
        })),
    )
    .await;
    let fragments = answered.body["hits"]["hits"][0]["highlight"]["body"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let materialised: usize = fragments
        .iter()
        .map(|fragment| fragment.as_str().unwrap_or_default().len())
        .sum();
    assert_eq!(
        answered.status, 200,
        "case \"a value of very many matched terms\": expected 200, got {} with {:?}",
        answered.status, answered.body
    );
    assert!(
        materialised > 0 && materialised <= 128 * 1024,
        "case \"a value of very many matched terms\": expected some fragments within the bytes \
         one hit may carry, got {materialised}"
    );
    // The value itself is untouched by the bound: the document still reads back whole.
    let read = rest_prefixed(&emulator, DOMAIN, "GET", "/bulky/_doc/one", None).await;
    assert_eq!(
        read.body["_source"]["body"].as_str().map(str::len),
        Some(repeated.len()),
        "case \"the document behind a bounded highlight\": expected the value as written, got \
         {:?}",
        read.body["_source"]["body"].as_str().map(str::len)
    );
}
