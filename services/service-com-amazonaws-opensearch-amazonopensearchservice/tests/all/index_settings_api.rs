// SPDX-License-Identifier: CC0-1.0

//! The INDEX SETTINGS API of a domain's own REST data plane, driven over the real wire
//! path: a signed request to the endpoint `DescribeDomain` published, through the
//! multi-service router, into the mounted service. What is asserted is what a client sees.
//!
//! The rule under test, asserted over a table rather than one call: `PUT
//! /{index}/_settings` writes the DYNAMIC settings its body names on every index the path
//! names and answers `200 {"acknowledged": true}`, and `GET /{index}/_settings` reports the
//! settings each of those indexes then holds — so a setting a client writes is the setting
//! the cluster reads back, whichever of the spellings the engine takes it was written in. A
//! setting that can only be chosen when the index is created is refused and changes nothing,
//! and a path naming an index the domain does not hold is the engine's own not-found.
//!
//! The rows differ along what the rule generalises over: which spelling the body uses (flat
//! dotted with and without the `index.` prefix, and nested sections), which kind of value
//! the setting carries (a number, a string, a boolean, a deeply nested slow-log threshold),
//! how many settings one body names, whether the body RESETS a setting with `null`, whether
//! the setting is a dynamic or a static one, and how the path names the indexes (one name, a
//! comma-separated list, a wildcard, `_all`, no index at all, and a name no index carries).

use serde_json::{json, Value};

use super::data_source_registry::created_domain;
use super::domain_lifecycle::{Emulator, DEFAULT_ACCESS_KEY_ID};
use super::domain_rest_data_plane::{endpoint_of, error_type, rest};

/// The index every row writes its settings on.
const INDEX: &str = "catalog";

/// A second index, so a path naming several indexes and a wildcard are exercised against
/// more than one and an update cannot be the plane writing one hard-coded record.
const SECOND_INDEX: &str = "catalog-archive";

/// One row of the table: the body a `PUT` sends, and the settings the index has to report
/// afterwards — each as the dotted name of the setting and the value reported for it, or no
/// value at all for a setting the row RESETS.
struct Row {
    case: &'static str,
    body: Value,
    reported: Vec<(&'static str, Option<Value>)>,
}

/// The table. Every row is sent as the whole body of one `PUT /{index}/_settings`, and the
/// rows apply in order, so a later row reads the settings the earlier ones left.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a flat dotted name carrying the index prefix",
            body: json!({ "index.number_of_replicas": 2 }),
            reported: vec![("number_of_replicas", Some(json!(2)))],
        },
        Row {
            case: "the same setting without the index prefix",
            body: json!({ "number_of_replicas": 3 }),
            reported: vec![("number_of_replicas", Some(json!(3)))],
        },
        Row {
            case: "a nested section under index",
            body: json!({ "index": { "refresh_interval": "30s" } }),
            reported: vec![
                ("refresh_interval", Some(json!("30s"))),
                // A setting the body does not name is left exactly as it was.
                ("number_of_replicas", Some(json!(3))),
            ],
        },
        Row {
            case: "a boolean setting",
            body: json!({ "index": { "blocks": { "read_only": true } } }),
            reported: vec![("blocks.read_only", Some(json!(true)))],
        },
        Row {
            case: "the slow-log thresholds, written as deeply nested sections",
            body: json!({
                "index": {
                    "search": { "slowlog": { "threshold": { "query": { "warn": "0ms" } } } },
                    "indexing": { "slowlog": { "threshold": { "index": { "warn": "2s" } } } },
                }
            }),
            reported: vec![
                (
                    "search.slowlog.threshold.query.warn",
                    Some(json!("0ms")),
                ),
                (
                    "indexing.slowlog.threshold.index.warn",
                    Some(json!("2s")),
                ),
            ],
        },
        Row {
            case: "the same thresholds rewritten in the flat spelling",
            body: json!({
                "index.search.slowlog.threshold.query.warn": "500ms",
                "index.indexing.slowlog.threshold.index.warn": "-1",
            }),
            reported: vec![
                (
                    "search.slowlog.threshold.query.warn",
                    Some(json!("500ms")),
                ),
                (
                    "indexing.slowlog.threshold.index.warn",
                    Some(json!("-1")),
                ),
            ],
        },
        Row {
            case: "several settings at once",
            body: json!({
                "index": { "number_of_replicas": 1, "max_result_window": 5000 },
                "refresh_interval": "60s",
            }),
            reported: vec![
                ("number_of_replicas", Some(json!(1))),
                ("max_result_window", Some(json!(5000))),
                ("refresh_interval", Some(json!("60s"))),
            ],
        },
        Row {
            case: "a setting reset with null",
            body: json!({ "index": { "refresh_interval": Value::Null } }),
            reported: vec![
                ("refresh_interval", None),
                // The reset takes out the setting it names and nothing beside it.
                ("number_of_replicas", Some(json!(1))),
            ],
        },
        Row {
            case: "a body naming no setting at all",
            body: json!({}),
            reported: vec![("number_of_replicas", Some(json!(1)))],
        },
    ]
}

/// The settings `GET /{path}/_settings` reports for `index`, flattened to the dotted names
/// the settings have, so a row states one expectation per setting whatever nesting the
/// report carries.
async fn reported_settings(
    emulator: &Emulator,
    host: &str,
    path: &str,
    index: &str,
    case: &str,
) -> Vec<(String, Value)> {
    let answer = rest(
        emulator,
        DEFAULT_ACCESS_KEY_ID,
        host,
        "GET",
        &format!("/{path}/_settings"),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected GET /{path}/_settings to answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    let section = answer.body[index]["settings"]["index"].clone();
    let mut flat = Vec::new();
    flatten(&section, String::new(), &mut flat);
    flat
}

/// `value` flattened to `(dotted name, value)` pairs — the shape a row's expectations are
/// written in.
fn flatten(value: &Value, prefix: String, into: &mut Vec<(String, Value)>) {
    match value {
        Value::Object(members) => {
            for (name, member) in members {
                let path = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}.{name}")
                };
                flatten(member, path, into);
            }
        }
        other => into.push((prefix, other.clone())),
    }
}

/// The value the reported settings give `name`, or `None` when they carry none.
fn setting(reported: &[(String, Value)], name: &str) -> Option<Value> {
    reported
        .iter()
        .find(|(reported, _)| reported == name)
        .map(|(_, value)| value.clone())
}

#[tokio::test]
async fn an_index_settings_put_writes_the_dynamic_settings_and_the_get_reports_them_back() {
    let emulator = Emulator::new();
    created_domain(&emulator, "settings-target").await;
    let host = endpoint_of(&emulator, "settings-target");

    for index in [INDEX, SECOND_INDEX] {
        let created = rest(
            &emulator,
            DEFAULT_ACCESS_KEY_ID,
            &host,
            "PUT",
            &format!("/{index}"),
            Some(json!({ "settings": { "number_of_shards": 2 } })),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case PUT /{index}: expected 200, got {} with {:?}",
            created.status, created.body
        );
    }

    for row in rows() {
        let Row {
            case,
            body,
            reported,
        } = row;
        let answer = rest(
            &emulator,
            DEFAULT_ACCESS_KEY_ID,
            &host,
            "PUT",
            &format!("/{INDEX}/_settings"),
            Some(body),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["acknowledged"].as_bool(),
            Some(true),
            "case {case}: expected the update to be acknowledged, got {:?}",
            answer.body
        );
        let held = reported_settings(&emulator, &host, INDEX, INDEX, case).await;
        for (name, expected) in reported {
            assert_eq!(
                setting(&held, name).as_ref(),
                expected.as_ref(),
                "case {case}: expected setting {name} to be {expected:?}, got {:?}",
                setting(&held, name)
            );
        }
        // The settings a create wrote are still there, under the canonical name they
        // have, and so is the name the cluster fills in itself.
        assert_eq!(
            setting(&held, "number_of_shards"),
            Some(json!(2)),
            "case {case}: expected the created shard count to survive the update, got {held:?}"
        );
        assert_eq!(
            setting(&held, "provided_name"),
            Some(json!(INDEX)),
            "case {case}: expected the index's own name in its settings, got {held:?}"
        );
        // And the update is of the index the path named: the second index is untouched.
        let other = reported_settings(&emulator, &host, SECOND_INDEX, SECOND_INDEX, case).await;
        assert_eq!(
            setting(&other, "number_of_replicas"),
            None,
            "case {case}: expected the index the path did not name to be unchanged, got \
             {other:?}"
        );
    }

    // The same settings are what a read of the whole index reports, so the settings API
    // and the index API cannot disagree about one index's settings.
    let index = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "GET",
        &format!("/{INDEX}"),
        None,
    )
    .await;
    assert_eq!(
        index.body[INDEX]["settings"]["index"]["number_of_replicas"],
        json!(1),
        "case \"GET /{INDEX}\": expected the settings the update left, got {:?}",
        index.body
    );

    // `flat_settings` reports the same settings under the dotted names they HAVE.
    let flat = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "GET",
        &format!("/{INDEX}/_settings?flat_settings=true"),
        None,
    )
    .await;
    assert_eq!(
        flat.body[INDEX]["settings"]["index"]["search.slowlog.threshold.query.warn"],
        json!("500ms"),
        "case \"flat_settings\": expected the dotted name, got {:?}",
        flat.body
    );
}

/// One addressing row: the path the `PUT` is sent to, which indexes it has to change, and
/// the status it is answered with.
struct Addressed {
    case: &'static str,
    path: &'static str,
    status: u16,
    changes: &'static [&'static str],
}

#[tokio::test]
async fn an_index_settings_put_changes_every_index_the_path_names_and_nothing_else() {
    let emulator = Emulator::new();
    created_domain(&emulator, "settings-paths").await;
    let host = endpoint_of(&emulator, "settings-paths");
    for index in [INDEX, SECOND_INDEX, "ledger"] {
        let created = rest(
            &emulator,
            DEFAULT_ACCESS_KEY_ID,
            &host,
            "PUT",
            &format!("/{index}"),
            None,
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case PUT /{index}: expected 200, got {} with {:?}",
            created.status, created.body
        );
    }

    let addressings: &[Addressed] = &[
        Addressed {
            case: "one index by its name",
            path: "/catalog/_settings",
            status: 200,
            changes: &[INDEX],
        },
        Addressed {
            case: "two indexes by a comma-separated list",
            path: "/catalog,ledger/_settings",
            status: 200,
            changes: &[INDEX, "ledger"],
        },
        Addressed {
            case: "every index a wildcard matches",
            path: "/catalog*/_settings",
            status: 200,
            changes: &[INDEX, SECOND_INDEX],
        },
        Addressed {
            case: "every index the cluster holds, by _all",
            path: "/_all/_settings",
            status: 200,
            changes: &[INDEX, SECOND_INDEX, "ledger"],
        },
        Addressed {
            case: "every index the cluster holds, by naming none",
            path: "/_settings",
            status: 200,
            changes: &[INDEX, SECOND_INDEX, "ledger"],
        },
        Addressed {
            case: "an index the domain does not hold",
            path: "/absent/_settings",
            status: 404,
            changes: &[],
        },
    ];

    for (position, row) in addressings.iter().enumerate() {
        // A value per row, so what each index reports says WHICH rows reached it.
        let written = json!(format!("{}s", position + 1));
        let answer = rest(
            &emulator,
            DEFAULT_ACCESS_KEY_ID,
            &host,
            "PUT",
            row.path,
            Some(json!({ "index": { "refresh_interval": written.clone() } })),
        )
        .await;
        assert_eq!(
            answer.status, row.status,
            "case {}: expected {}, got {} with {:?}",
            row.case, row.status, answer.status, answer.body
        );
        if row.status != 200 {
            assert_eq!(
                error_type(&answer),
                "index_not_found_exception",
                "case {}: expected the engine's not-found, got {:?}",
                row.case,
                answer.body
            );
            continue;
        }
        for index in [INDEX, SECOND_INDEX, "ledger"] {
            let held = reported_settings(&emulator, &host, index, index, row.case).await;
            let expected = row.changes.contains(&index).then(|| written.clone());
            if let Some(expected) = expected {
                assert_eq!(
                    setting(&held, "refresh_interval"),
                    Some(expected.clone()),
                    "case {} on {index}: expected {expected:?}, got {:?}",
                    row.case,
                    setting(&held, "refresh_interval")
                );
            } else {
                assert_ne!(
                    setting(&held, "refresh_interval"),
                    Some(written.clone()),
                    "case {} on {index}: an index the path did not name must not change, got \
                     {held:?}",
                    row.case
                );
            }
        }
    }
}

/// One row of the static-settings table: the setting a body names and whether the index may
/// be given it after it exists.
struct Dynamic {
    case: &'static str,
    body: Value,
    /// The setting's full name, as a refusal names it — `None` for a row the rule accepts.
    refused: Option<&'static str>,
}

#[tokio::test]
async fn a_setting_only_a_create_may_choose_is_refused_on_an_index_that_exists() {
    let emulator = Emulator::new();
    created_domain(&emulator, "settings-dynamic").await;
    let host = endpoint_of(&emulator, "settings-dynamic");
    let created = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "PUT",
        &format!("/{INDEX}"),
        Some(json!({ "settings": { "number_of_shards": 2, "number_of_replicas": 0 } })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case PUT /{INDEX}: expected 200, got {} with {:?}",
        created.status, created.body
    );

    // The static settings are the ones the engine documents as settable at index creation
    // only; everything else an index carries is dynamic.
    let rows: &[Dynamic] = &[
        Dynamic {
            case: "the shard count",
            body: json!({ "index": { "number_of_shards": 4 } }),
            refused: Some("index.number_of_shards"),
        },
        Dynamic {
            case: "the routing partition size",
            body: json!({ "index.routing_partition_size": 2 }),
            refused: Some("index.routing_partition_size"),
        },
        Dynamic {
            case: "the codec",
            body: json!({ "index": { "codec": "best_compression" } }),
            refused: Some("index.codec"),
        },
        Dynamic {
            case: "a static setting beside a dynamic one",
            body: json!({ "index": { "number_of_replicas": 2, "number_of_shards": 8 } }),
            refused: Some("index.number_of_shards"),
        },
        Dynamic {
            case: "the replica count",
            body: json!({ "index": { "number_of_replicas": 2 } }),
            refused: None,
        },
        Dynamic {
            case: "the refresh interval",
            body: json!({ "index": { "refresh_interval": "10s" } }),
            refused: None,
        },
    ];

    for row in rows {
        let answer = rest(
            &emulator,
            DEFAULT_ACCESS_KEY_ID,
            &host,
            "PUT",
            &format!("/{INDEX}/_settings"),
            Some(row.body.clone()),
        )
        .await;
        let held = reported_settings(&emulator, &host, INDEX, INDEX, row.case).await;
        match row.refused {
            Some(named) => {
                assert_eq!(
                    answer.status, 400,
                    "case {}: expected 400, got {} with {:?}",
                    row.case, answer.status, answer.body
                );
                assert!(
                    answer.body["error"]["reason"]
                        .as_str()
                        .unwrap_or_default()
                        .contains(named),
                    "case {}: expected the refusal to name {named}, got {:?}",
                    row.case,
                    answer.body
                );
                // A refused update stores NOTHING, not even the dynamic settings it
                // carried beside the static one.
                assert_eq!(
                    setting(&held, "number_of_shards"),
                    Some(json!(2)),
                    "case {}: expected the shard count to be unchanged, got {held:?}",
                    row.case
                );
                assert_ne!(
                    setting(&held, "number_of_replicas"),
                    Some(json!(2)),
                    "case {}: a refused update must store none of its settings, got {held:?}",
                    row.case
                );
            }
            None => {
                assert_eq!(
                    answer.status, 200,
                    "case {}: expected 200, got {} with {:?}",
                    row.case, answer.status, answer.body
                );
            }
        }
    }
}
