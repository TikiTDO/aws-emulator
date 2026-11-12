// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeEngineDefaultParameters`.
//!
//! The rule under test: the operation reports the engine default parameters the
//! service publishes for the named cache parameter group family — every
//! parameter of that family's engine catalog whose minimum engine version the
//! family's version line covers, each carrying its default value, description,
//! source, data type, allowed values, modifiability, minimum engine version and
//! change type; a parameter whose value depends on the cache node type is
//! reported among the cache-node-type-specific parameters with one value per
//! node type the service publishes; the answer is paged at the `MaxRecords`
//! window the member documents with a resumable `Marker` — over
//! `EngineDefaults.Parameters`, which are this operation's paginated items,
//! while the cache-node-type-specific list comes whole with every page; and a
//! family name the service publishes no catalog for is refused with
//! `InvalidParameterValue`.
//!
//! Every assertion goes over the wire through the same router the production
//! emulator mounts, reusing the harness in [`crate::cache_cluster_lifecycle`];
//! only the HTTP status and the XML body are inspected, so an answer that was
//! not computed from the persisted catalog cannot pass. The tables below range
//! over the axis the rule generalises over — the FAMILY, and with it the engine
//! catalog and the version line — so the family a recorded call happens to name
//! is one row of many.

use std::collections::BTreeSet;

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{params, Emulator, Reply};

/// The published families of each engine catalog, in version order
/// (`state::FAMILY_LINES`). A later family's line covers every version an
/// earlier one on the same chain does, which is the invariant
/// [`a_later_family_carries_every_parameter_an_earlier_one_does`] asserts.
const FAMILY_CHAINS: [&[&str]; 2] = [
    &["memcached1.4", "memcached1.5", "memcached1.6"],
    &[
        "redis2.6", "redis2.8", "redis3.2", "redis4.0", "redis5.0", "redis6.x", "redis7",
        "valkey7", "valkey8", "valkey9",
    ],
];

/// One spot-check row of
/// [`a_parameter_is_reported_with_the_value_and_value_set_its_engine_documents`]:
/// the family, the parameter, its engine default value, a value its
/// `AllowedValues` must carry, its data type, whether it is modifiable, and its
/// change type.
type ParameterCase = (
    &'static str,
    &'static str,
    Option<&'static str>,
    Option<&'static str>,
    &'static str,
    &'static str,
    &'static str,
);

/// Ask for the engine defaults of `family`, with whatever else `extra` names.
async fn describe_defaults(emulator: &Emulator, family: &str, extra: &[(&str, &str)]) -> Reply {
    let mut pairs = params(&[("CacheParameterGroupFamily", family)]);
    pairs.extend(params(extra));
    emulator
        .call("DescribeEngineDefaultParameters", &pairs)
        .await
}

/// The `ParameterName`s reported among the plain parameters, in page order.
fn plain_names(reply: &Reply) -> Vec<&str> {
    reply.all("Parameters/Parameter/ParameterName")
}

/// The `ParameterName`s reported among the cache-node-type-specific parameters,
/// in page order.
fn node_type_specific_names(reply: &Reply) -> Vec<&str> {
    reply.all("CacheNodeTypeSpecificParameters/CacheNodeTypeSpecificParameter/ParameterName")
}

/// Every parameter name a reply reports, from both lists.
fn reported_names(reply: &Reply) -> Vec<String> {
    plain_names(reply)
        .into_iter()
        .chain(node_type_specific_names(reply))
        .map(str::to_string)
        .collect()
}

/// The names of `family`'s parameters that the `MaxRecords` window PAGES: the
/// plain ones, which are the operation's paginated items
/// (`smithy_query(query="operation",
/// operation="DescribeEngineDefaultParameters")`: `items
/// EngineDefaults.Parameters`).
fn paged_catalog(family: &str) -> BTreeSet<String> {
    state::family_default_parameters(family)
        .expect("a published family has a catalog")
        .values()
        .filter(|parameter| !parameter.is_node_type_specific())
        .map(|parameter| parameter.name.clone())
        .collect()
}

/// The names of `family`'s parameters that the window does NOT page: the
/// cache-node-type-specific ones, which every page reports in full.
fn unpaged_catalog(family: &str) -> Vec<String> {
    state::family_default_parameters(family)
        .expect("a published family has a catalog")
        .values()
        .filter(|parameter| parameter.is_node_type_specific())
        .map(|parameter| parameter.name.clone())
        .collect()
}

/// The text of `field` on the plain parameter `name`, or `None` when the reply
/// does not report that field for it (an absent element also reads as `None`:
/// the empty default of `notify-keyspace-events` carries no text).
///
/// The search is bounded to the elements BETWEEN this parameter's
/// `ParameterName` and the next one's, so a parameter that reports no value
/// cannot borrow the next parameter's.
fn plain_field<'a>(reply: &'a Reply, name: &str, field: &str) -> Option<&'a str> {
    let suffix = format!("/{field}");
    let mut nodes = reply
        .nodes
        .iter()
        .skip_while(|(path, text)| !(path.ends_with("/ParameterName") && text == name));
    // The matching `ParameterName` itself.
    nodes.next()?;
    nodes
        .take_while(|(path, _)| !path.ends_with("/ParameterName"))
        .find(|(path, _)| path.ends_with(&suffix))
        .map(|(_, text)| text.as_str())
}

/// The PAGED parameters of `family`, walked page by page at `page_size`.
///
/// Every page is also held to the rule that the cache-node-type-specific list
/// is not paged: it is reported whole beside each window of plain parameters.
async fn walk(emulator: &Emulator, family: &str, page_size: &str) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut extra: Vec<(&str, &str)> = vec![("MaxRecords", page_size)];
        if let Some(token) = marker.as_deref() {
            extra.push(("Marker", token));
        }
        let reply = describe_defaults(emulator, family, &extra).await;
        assert_eq!(
            reply.status, 200,
            "case {family} page {pages}: must succeed, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            node_type_specific_names(&reply),
            unpaged_catalog(family),
            "case {family}@{page_size} page {pages}: every page reports the whole \
             cache-node-type-specific list, got {}",
            reply.body
        );
        seen.extend(plain_names(&reply).into_iter().map(str::to_string));
        pages += 1;
        assert!(
            pages <= 20,
            "case {family}: walking the catalog at {page_size} per page must terminate, \
             got {pages} pages"
        );
        match reply.marker() {
            Some(token) => marker = Some(token.to_string()),
            None => break,
        }
    }
    seen
}

#[tokio::test]
async fn every_published_family_reports_the_catalog_the_store_holds_for_it() {
    let emulator = Emulator::new();

    // The axis: the family. Every one the service publishes is a row, so the
    // catalog is exercised on both engines and on every version line.
    for family in state::PARAMETER_GROUP_FAMILIES {
        let expected = state::family_default_parameters(family)
            .unwrap_or_else(|| panic!("case {family}: a published family has a catalog"));
        let expected_names: BTreeSet<String> = expected
            .values()
            .map(|parameter| parameter.name.clone())
            .collect();

        let reply = describe_defaults(&emulator, family, &[]).await;
        assert_eq!(
            reply.status, 200,
            "case {family}: must succeed, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.text("CacheParameterGroupFamily"),
            Some(family),
            "case {family}: the answer must name the family it is for, got {}",
            reply.body
        );

        let reported: BTreeSet<String> = reported_names(&reply).into_iter().collect();
        assert_eq!(
            reported, expected_names,
            "case {family}: expected the family's stored catalog, got {reported:?}"
        );
        assert!(
            !reported.is_empty(),
            "case {family}: a published family has engine defaults, got {}",
            reply.body
        );
        assert_eq!(
            reported.len(),
            reported_names(&reply).len(),
            "case {family}: no parameter may be reported twice, got {:?}",
            reported_names(&reply)
        );
        // The default page holds the whole catalog of every published family, so
        // this answer is complete and carries no marker.
        assert_eq!(
            reply.marker(),
            None,
            "case {family}: a complete answer pages nothing, got {}",
            reply.body
        );

        // Each reported parameter carries the fields a client branches on.
        let sources = reply.all("Source");
        assert_eq!(
            sources.len(),
            reported.len(),
            "case {family}: every parameter reports its source, got {sources:?}"
        );
        assert!(
            sources
                .iter()
                .all(|source| *source == state::PARAMETER_SOURCE_SYSTEM),
            "case {family}: an engine default's source is {}, got {sources:?}",
            state::PARAMETER_SOURCE_SYSTEM
        );
        let change_types = reply.all("ChangeType");
        assert_eq!(
            change_types.len(),
            reported.len(),
            "case {family}: every parameter reports its change type, got {change_types:?}"
        );
        for change_type in &change_types {
            assert!(
                *change_type == state::CHANGE_TYPE_IMMEDIATE
                    || *change_type == state::CHANGE_TYPE_REQUIRES_REBOOT,
                "case {family}: a change type is one of the modelled `ChangeType` values, \
                 got {change_type}"
            );
        }
        let minimum_versions = reply.all("MinimumEngineVersion");
        assert_eq!(
            minimum_versions.len(),
            reported.len(),
            "case {family}: every parameter reports its minimum engine version, \
             got {minimum_versions:?}"
        );
        let data_types = reply.all("DataType");
        assert_eq!(
            data_types.len(),
            reported.len(),
            "case {family}: every parameter reports its data type, got {data_types:?}"
        );
        let modifiable = reply.all("IsModifiable");
        assert_eq!(
            modifiable.len(),
            reported.len(),
            "case {family}: every parameter reports whether it is modifiable, got {modifiable:?}"
        );
        assert!(
            modifiable
                .iter()
                .all(|flag| *flag == "true" || *flag == "false"),
            "case {family}: IsModifiable is a boolean, got {modifiable:?}"
        );

        // A describe is a read: asking again reports the same catalog.
        let again = describe_defaults(&emulator, family, &[]).await;
        assert_eq!(
            reported_names(&again),
            reported_names(&reply),
            "case {family}: a read must not change what the next caller sees"
        );
    }
}

#[tokio::test]
async fn a_parameter_is_reported_with_the_value_and_value_set_its_engine_documents() {
    let emulator = Emulator::new();

    // Rows the issue's own units never name, one per engine catalog: the
    // parameter, the family, its engine default value, a value its
    // `AllowedValues` must carry, its data type, whether it is modifiable and
    // its change type. Source: the ElastiCache User Guide parameter tables this
    // crate's catalog is built from — these are LITERALS, so a catalog value
    // that drifts fails here rather than being echoed back.
    let cases: [ParameterCase; 6] = [
        (
            "redis7",
            "timeout",
            Some("0"),
            Some("0,20-"),
            "integer",
            "true",
            "immediate",
        ),
        (
            "redis7",
            "maxmemory-policy",
            Some("volatile-lru"),
            Some("noeviction"),
            "string",
            "true",
            "immediate",
        ),
        (
            "redis6.x",
            "reserved-memory-percent",
            Some("25"),
            Some("0-100"),
            "integer",
            "true",
            "immediate",
        ),
        (
            "memcached1.6",
            "binding_protocol",
            Some("auto"),
            Some("auto,ascii,binary"),
            "string",
            "true",
            "requires-reboot",
        ),
        (
            "memcached1.4",
            "cas_disabled",
            Some("0"),
            Some("0,1"),
            "boolean",
            "true",
            "requires-reboot",
        ),
        // A parameter a customer may NOT modify, and whose change needs a
        // reboot: both flags are reported from the catalog, not assumed.
        (
            "redis2.6",
            "activerehashing",
            Some("yes"),
            Some("yes,no"),
            "string",
            "false",
            "requires-reboot",
        ),
    ];
    for (family, parameter, value, allowed, data_type, modifiable, change_type) in cases {
        let reply = describe_defaults(&emulator, family, &[]).await;
        assert_eq!(
            reply.status, 200,
            "case {family}/{parameter}: must succeed, got {} {}",
            reply.status, reply.body
        );
        assert!(
            plain_names(&reply).contains(&parameter),
            "case {family}/{parameter}: expected the parameter to be reported, got {:?}",
            plain_names(&reply)
        );
        assert_eq!(
            plain_field(&reply, parameter, "ParameterValue"),
            value,
            "case {family}/{parameter}: expected the engine default {value:?}, got {}",
            reply.body
        );
        let allowed_values = plain_field(&reply, parameter, "AllowedValues");
        match allowed {
            Some(allowed) => assert!(
                allowed_values.is_some_and(|values| values.contains(allowed)),
                "case {family}/{parameter}: expected an allowed-value set carrying {allowed}, \
                 got {allowed_values:?}"
            ),
            None => assert_eq!(
                allowed_values, None,
                "case {family}/{parameter}: a parameter the engine constrains to no published \
                 set reports none, got {allowed_values:?}"
            ),
        }
        assert_eq!(
            plain_field(&reply, parameter, "DataType"),
            Some(data_type),
            "case {family}/{parameter}: expected the data type {data_type}, got {}",
            reply.body
        );
        assert_eq!(
            plain_field(&reply, parameter, "IsModifiable"),
            Some(modifiable),
            "case {family}/{parameter}: expected IsModifiable {modifiable}, got {}",
            reply.body
        );
        assert_eq!(
            plain_field(&reply, parameter, "ChangeType"),
            Some(change_type),
            "case {family}/{parameter}: expected the change type {change_type}, got {}",
            reply.body
        );
    }

    // An engine default that is the empty string carries no text, and a
    // parameter the engine constrains to no published set carries no
    // `AllowedValues` element at all.
    let reply = describe_defaults(&emulator, "redis7", &[]).await;
    assert!(
        plain_names(&reply).contains(&"notify-keyspace-events"),
        "case redis7/notify-keyspace-events: expected the parameter to be reported, got {:?}",
        plain_names(&reply)
    );
    for field in ["ParameterValue", "AllowedValues"] {
        assert_eq!(
            plain_field(&reply, "notify-keyspace-events", field),
            None,
            "case redis7/notify-keyspace-events: expected no {field}, got {}",
            reply.body
        );
    }
}

#[tokio::test]
async fn a_later_family_carries_every_parameter_an_earlier_one_does() {
    let emulator = Emulator::new();

    // The invariant: a family's parameters are its engine's catalog up to the
    // version its line covers, so along one engine's chain the sets only grow —
    // and a parameter a later version introduced is absent from the earlier
    // families.
    for chain in FAMILY_CHAINS {
        let mut previous: Option<(&str, BTreeSet<String>)> = None;
        let mut first: Option<BTreeSet<String>> = None;
        for family in chain {
            let family = *family;
            let reply = describe_defaults(&emulator, family, &[]).await;
            assert_eq!(
                reply.status, 200,
                "case {family}: must succeed, got {} {}",
                reply.status, reply.body
            );
            let names: BTreeSet<String> = reported_names(&reply).into_iter().collect();
            if let Some((earlier_family, earlier)) = previous.as_ref() {
                assert!(
                    earlier.is_subset(&names),
                    "case {family}: must carry every parameter {earlier_family} does, missing {:?}",
                    earlier.difference(&names).collect::<Vec<_>>()
                );
            }
            first.get_or_insert_with(|| names.clone());
            previous = Some((family, names));
        }
        let (last_family, last) = previous.expect("a chain names at least one family");
        let first = first.expect("a chain names at least one family");
        assert!(
            last.len() > first.len(),
            "case {last_family}: the newest family of a chain carries parameters the oldest \
             does not, got {} against {}",
            last.len(),
            first.len()
        );
    }

    // The version rule, spelled out on one parameter per engine: it is reported
    // exactly by the families whose line covers the version that introduced it.
    let introduced: [(&str, &str, &[&str], &[&str]); 2] = [
        (
            // `acl-pubsub-default` arrived with Redis OSS 6.2.
            "acl-pubsub-default",
            "6.2.6",
            &["redis6.x", "redis7", "valkey7", "valkey8"],
            &["redis2.6", "redis5.0"],
        ),
        (
            // `slab_chunk_max` arrived with Memcached 1.5.
            "slab_chunk_max",
            "1.5.10",
            &["memcached1.5", "memcached1.6"],
            &["memcached1.4"],
        ),
    ];
    for (parameter, version, carried_by, absent_from) in introduced {
        for family in carried_by.iter().copied() {
            let reply = describe_defaults(&emulator, family, &[]).await;
            assert!(
                reported_names(&reply).iter().any(|name| name == parameter),
                "case {family}/{parameter}: a family whose line covers {version} carries it, \
                 got {:?}",
                reported_names(&reply)
            );
        }
        for family in absent_from.iter().copied() {
            let reply = describe_defaults(&emulator, family, &[]).await;
            assert!(
                !reported_names(&reply).iter().any(|name| name == parameter),
                "case {family}/{parameter}: a family whose line predates {version} does not \
                 carry it, got {:?}",
                reported_names(&reply)
            );
        }
    }
}

#[tokio::test]
async fn a_node_type_specific_parameter_carries_one_value_per_published_node_type() {
    let emulator = Emulator::new();

    // The axis: the family, and with it which parameters depend on the node
    // type. Each row names the parameters that must be reported among the
    // cache-node-type-specific ones.
    let cases: [(&str, &[&str]); 3] = [
        ("redis7", &["maxmemory"]),
        ("redis2.6", &["maxmemory"]),
        ("memcached1.6", &["max_cache_memory", "num_threads"]),
    ];
    let published: BTreeSet<&str> = state::OFFERING_NODE_TYPES
        .iter()
        .map(|(node_type, _)| *node_type)
        .collect();
    for (family, expected) in cases {
        let reply = describe_defaults(&emulator, family, &[]).await;
        assert_eq!(
            reply.status, 200,
            "case {family}: must succeed, got {} {}",
            reply.status, reply.body
        );
        let names = node_type_specific_names(&reply);
        assert_eq!(
            names,
            expected.to_vec(),
            "case {family}: expected exactly the node-type-specific parameters {expected:?}, \
             got {names:?}"
        );
        // A node-type-specific parameter is reported in that list and NOT among
        // the plain parameters, because it has no single value.
        for parameter in expected {
            assert!(
                !plain_names(&reply).contains(parameter),
                "case {family}/{parameter}: a node-type-specific parameter carries no single \
                 value, got {}",
                reply.body
            );
        }

        let node_types: Vec<&str> = reply.all("CacheNodeTypeSpecificValue/CacheNodeType");
        let values: Vec<&str> = reply.all("CacheNodeTypeSpecificValue/Value");
        assert_eq!(
            node_types.len(),
            published.len() * expected.len(),
            "case {family}: expected one value per published node type for each of \
             {expected:?}, got {node_types:?}"
        );
        assert_eq!(
            values.len(),
            node_types.len(),
            "case {family}: every node type reports a value, got {values:?}"
        );
        let reported: BTreeSet<&str> = node_types.into_iter().collect();
        assert_eq!(
            reported, published,
            "case {family}: the node types must be the ones this service publishes, \
             got {reported:?}"
        );
        for value in &values {
            let parsed: i64 = value
                .parse()
                .unwrap_or_else(|_| panic!("case {family}: expected a number, got {value}"));
            assert!(
                parsed > 0,
                "case {family}: a node-type-specific value is positive, got {parsed}"
            );
        }
    }

    // The values rise with the node size, so a bigger node type never reports
    // less capacity than a smaller one of the same family.
    let reply = describe_defaults(&emulator, "redis7", &[]).await;
    let pairs: Vec<(&str, &str)> = reply
        .all("CacheNodeTypeSpecificValue/CacheNodeType")
        .into_iter()
        .zip(reply.all("CacheNodeTypeSpecificValue/Value"))
        .collect();
    let value_of = |node_type: &str| -> i64 {
        pairs
            .iter()
            .find(|(reported, _)| *reported == node_type)
            .map(|(_, value)| value.parse().unwrap_or_default())
            .unwrap_or_else(|| panic!("case {node_type}: expected a maxmemory value, got {pairs:?}"))
    };
    for (smaller, bigger) in [
        ("cache.m5.large", "cache.m5.xlarge"),
        ("cache.m5.xlarge", "cache.m5.2xlarge"),
        ("cache.t3.micro", "cache.t3.small"),
        ("cache.t3.small", "cache.t3.medium"),
    ] {
        assert!(
            value_of(smaller) < value_of(bigger),
            "case {smaller} vs {bigger}: maxmemory must rise with the node size, got {} and {}",
            value_of(smaller),
            value_of(bigger)
        );
    }
}

#[tokio::test]
async fn pages_are_bounded_resumable_and_cover_the_whole_catalog() {
    let emulator = Emulator::new();

    // `redis7` carries more parameters than the smallest page this service
    // serves, so it is the family the page window shows on. The window pages
    // the PLAIN parameters only — they are this operation's paginated items —
    // so the total it is held against is their count.
    let family = "redis7";
    let total = paged_catalog(family).len();
    assert!(
        total > 20,
        "case {family}: this suite needs a catalog larger than the smallest page, got {total}"
    );

    // Each row is a page size: the published bounds of "minimum 20; maximum
    // 100", one past each, and the default when the member is absent.
    let cases: [(&str, Option<usize>); 6] = [
        ("20", Some(20)),
        ("21", Some(21)),
        ("100", Some(total.min(100))),
        ("19", None),
        ("101", None),
        ("0", None),
    ];
    for (max_records, accepted) in cases {
        let reply = describe_defaults(&emulator, family, &[("MaxRecords", max_records)]).await;
        match accepted {
            Some(expected) => {
                assert_eq!(
                    reply.status, 200,
                    "case MaxRecords={max_records}: must be accepted, got {} {}",
                    reply.status, reply.body
                );
                let page = plain_names(&reply);
                assert_eq!(
                    page.len(),
                    expected,
                    "case MaxRecords={max_records}: expected {expected} parameters, got {page:?}"
                );
                // The cache-node-type-specific list is not paged: whatever the
                // window is, it comes whole.
                assert_eq!(
                    node_type_specific_names(&reply),
                    unpaged_catalog(family),
                    "case MaxRecords={max_records}: the cache-node-type-specific list is not \
                     paged, got {}",
                    reply.body
                );
                assert_eq!(
                    reply.marker().is_some(),
                    total > expected,
                    "case MaxRecords={max_records}: a marker is present exactly when more \
                     records remain, got {}",
                    reply.body
                );
            }
            None => {
                assert_eq!(
                    reply.status, 400,
                    "case MaxRecords={max_records}: must be refused 400, got {} {}",
                    reply.status, reply.body
                );
                assert_eq!(
                    reply.error_code(),
                    "InvalidParameterValue",
                    "case MaxRecords={max_records}: expected InvalidParameterValue, got {}",
                    reply.body
                );
            }
        }
    }

    // A `Marker` this service did not mint is refused rather than silently
    // serving the first page.
    let bogus = describe_defaults(&emulator, family, &[("Marker", "not-a-marker")]).await;
    assert_eq!(
        bogus.status, 400,
        "case Marker=not-a-marker: must be refused 400, got {} {}",
        bogus.status, bogus.body
    );
    assert_eq!(
        bogus.error_code(),
        "InvalidParameterValue",
        "case Marker=not-a-marker: expected InvalidParameterValue, got {}",
        bogus.body
    );

    // A marker is only meaningful inside the family it was minted for: carried
    // to another family it names a page of a catalog it does not belong to, and
    // is refused rather than resolving to an arbitrary offset into it.
    let first = describe_defaults(&emulator, family, &[("MaxRecords", "20")]).await;
    let token = first
        .marker()
        .expect("a bounded first page of a larger catalog carries a marker")
        .to_string();
    for other in ["memcached1.6", "redis2.6", "valkey9"] {
        let crossed = describe_defaults(&emulator, other, &[("Marker", &token)]).await;
        assert_eq!(
            crossed.status, 400,
            "case marker of {family} against {other}: must be refused 400, got {} {}",
            crossed.status, crossed.body
        );
        assert_eq!(
            crossed.error_code(),
            "InvalidParameterValue",
            "case marker of {family} against {other}: expected InvalidParameterValue, got {}",
            crossed.body
        );
    }
    // The same token against the family that minted it resumes its page.
    let resumed = describe_defaults(&emulator, family, &[("Marker", &token)]).await;
    assert_eq!(
        resumed.status, 200,
        "case marker of {family} against {family}: must succeed, got {} {}",
        resumed.status, resumed.body
    );
    assert!(
        !reported_names(&resumed).is_empty(),
        "case marker of {family} against {family}: the resumed page carries records, got {}",
        resumed.body
    );

    // Walking the pages of a family visits every PAGED parameter of its catalog
    // exactly once, whatever the page size, while the node-type-specific list —
    // which the `Marker` does not page — comes whole with every page (asserted
    // inside the walk).
    for (family, page_size) in [
        ("redis7", "20"),
        ("redis7", "21"),
        ("memcached1.6", "20"),
        ("valkey9", "20"),
    ] {
        let expected = paged_catalog(family);
        let seen = walk(&emulator, family, page_size).await;
        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case {family}@{page_size}: the pages must not repeat a parameter, got {seen:?}"
        );
        assert_eq!(
            unique, expected,
            "case {family}@{page_size}: expected the whole catalog, got {unique:?}"
        );
    }
}

#[tokio::test]
async fn a_family_the_service_publishes_no_catalog_for_is_refused() {
    let emulator = Emulator::new();

    // The axis: the family name. None of these is a family this service
    // publishes, so each has no engine defaults to report — a value the member
    // may not take rather than an empty answer.
    let refused = [
        "redis9",
        "memcached2.0",
        "valkey",
        "redis",
        "not-a-family",
        "default.redis7",
    ];
    for family in refused {
        let reply = describe_defaults(&emulator, family, &[]).await;
        assert_eq!(
            reply.status, 400,
            "case {family}: must be refused 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterValue",
            "case {family}: expected InvalidParameterValue, got {}",
            reply.body
        );
    }
}
