// SPDX-License-Identifier: CC0-1.0

//! The two DESCRIPTION strings the cache-engine-version catalog publishes.
//!
//! One rule, over a SET of entries rather than one recorded request:
//! `CacheEngineVersion$CacheEngineDescription` is the whole label ElastiCache publishes for
//! the ENGINE, and `CacheEngineVersion$CacheEngineVersionDescription` is `<engine> version
//! <release>` over the engine's API name and the full release the entry's `EngineVersion`
//! stands for — which is the version itself where it already names a release, and the patch
//! release behind the line where ElastiCache publishes a version by its line alone.
//!
//! The rows differ along what the rule generalises over: the engine, whether its versions
//! are published by release or by line, and which line — including entries no issue cited.
//! Every assertion goes over the wire through the same router the production emulator
//! mounts, and only the XML body is read, so a description that is not computed from the
//! published catalog cannot pass.

use crate::harness::{params, Emulator, Reply};

/// The label ElastiCache publishes for each engine, as `(engine, label)`.
///
/// Source: the `CacheEngineDescription` AWS answers `DescribeCacheEngineVersions` with per
/// engine — `memcached` and `Redis` under the engine's own product name, spelled the way
/// that project spells it, and the Valkey offering under its longer ElastiCache label.
const ENGINE_LABELS: [(&str, &str); 3] = [
    ("memcached", "memcached"),
    ("redis", "Redis"),
    ("valkey", "Valkey Engine for ElastiCache"),
];

/// The release each version ElastiCache publishes by its LINE alone stands for, as
/// `(engine, EngineVersion, release)`.
///
/// Source: the release AWS lists for each of those lines among the ElastiCache engine
/// versions it supports. Every other published version already NAMES a release (`1.4.14`,
/// `5.0.6`), so it is its own release and is absent from this table — see
/// [`expected_release`], which is how the expectation below covers every entry of the
/// catalog without naming them all.
///
/// Written here rather than read from the service, so a catalog that published a different
/// release would fail.
const LINE_RELEASES: [(&str, &str, &str); 7] = [
    ("redis", "6.0", "6.0.5"),
    ("redis", "6.2", "6.2.6"),
    ("redis", "7.0", "7.0.7"),
    ("redis", "7.1", "7.1.0"),
    ("valkey", "7.2", "7.2.6"),
    ("valkey", "8.0", "8.0.1"),
    // The Valkey 9.0 line this emulator's catalog publishes beside them has no AWS release
    // behind it: a line's first release is its `.0`, which is the form every engine line's
    // first release takes.
    ("valkey", "9.0", "9.0.0"),
];

/// The full release `version` of `engine` stands for: the row [`LINE_RELEASES`] holds for a
/// version published by its line, and the version itself where it already names a release.
fn expected_release<'a>(engine: &str, version: &'a str) -> &'a str {
    LINE_RELEASES
        .iter()
        .find(|(line_engine, line, _)| *line_engine == engine && *line == version)
        .map_or(version, |(_, _, release)| *release)
}

/// One `DescribeCacheEngineVersions` call that must succeed.
async fn describe(emulator: &Emulator, case: &str, pairs: &[(&str, &str)]) -> Reply {
    let reply = emulator
        .call("DescribeCacheEngineVersions", &params(pairs))
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case}: DescribeCacheEngineVersions must answer 200, got {} {}",
        reply.status, reply.body
    );
    reply
}

/// Every entry a reply reports, as `(engine, version, engine description, version
/// description)` in document order.
fn entries(case: &str, reply: &Reply) -> Vec<(String, String, String, String)> {
    let engines = reply.all("Engine");
    let versions = reply.all("EngineVersion");
    let labels = reply.all("CacheEngineDescription");
    let descriptions = reply.all("CacheEngineVersionDescription");
    for (member, column) in [
        ("EngineVersion", versions.len()),
        ("CacheEngineDescription", labels.len()),
        ("CacheEngineVersionDescription", descriptions.len()),
    ] {
        assert_eq!(
            column,
            engines.len(),
            "case {case}: {member} must be reported once per entry ({}), got {column}: {}",
            engines.len(),
            reply.body
        );
    }
    (0..engines.len())
        .map(|index| {
            (
                engines[index].to_string(),
                versions[index].to_string(),
                labels[index].to_string(),
                descriptions[index].to_string(),
            )
        })
        .collect()
}

#[tokio::test]
async fn every_entry_is_described_by_its_engines_label_and_its_own_release() {
    let emulator = Emulator::new();

    // The axis: the engine, over every engine with a published label. Each row asserts the
    // engine's WHOLE version list, so the label rule holds for every entry of it.
    for (engine, label) in ENGINE_LABELS {
        let reply = describe(&emulator, engine, &[("Engine", engine), ("MaxRecords", "100")]).await;
        let rows = entries(engine, &reply);
        assert!(
            !rows.is_empty(),
            "case {engine}: the catalog must publish versions of {engine}"
        );
        for (reported_engine, version, reported_label, description) in &rows {
            let case = format!("{engine} {version}");
            assert_eq!(
                reported_engine, engine,
                "case {case}: the Engine filter must select only {engine}, got \
                 {reported_engine:?}"
            );
            assert_eq!(
                reported_label, label,
                "case {case}: expected the engine label {label:?}, got {reported_label:?}"
            );
            // The EXACT description of this entry: `<engine> version <release>` over the
            // engine's API name and the release the entry's version stands for — asserted
            // for every entry of every engine rather than the few a table could name.
            let release = expected_release(engine, version);
            let expected = format!("{engine} version {release}");
            assert_eq!(
                description, &expected,
                "case {case}: expected the version description {expected:?}, got \
                 {description:?}"
            );
            assert_eq!(
                release.split('.').count(),
                3,
                "case {case}: every published version must stand for a patch-level release, \
                 got {release:?}"
            );
            assert!(
                release.starts_with(version.as_str()),
                "case {case}: the release {release:?} must belong to the published version \
                 {version:?}"
            );
        }
    }

    // The rows of [`LINE_RELEASES`]: each line-published entry's own description, read back
    // from the catalog filtered to that one version — the entries whose release the version
    // description is the only place to read.
    for (engine, version, release) in LINE_RELEASES {
        let case = format!("{engine} {version}");
        let reply = describe(
            &emulator,
            &case,
            &[("Engine", engine), ("EngineVersion", version)],
        )
        .await;
        let rows = entries(&case, &reply);
        let entry = rows
            .iter()
            .find(|(_, reported, _, _)| reported == version)
            .unwrap_or_else(|| panic!("case {case}: the catalog must publish {case}, got {rows:?}"));
        let (_, _, label, description) = entry;
        let expected_label = ENGINE_LABELS
            .iter()
            .find(|(labelled, _)| *labelled == engine)
            .map(|(_, label)| *label)
            .unwrap_or_else(|| panic!("case {case}: no label row for {engine}"));
        assert_eq!(
            label, expected_label,
            "case {case}: expected the engine label {expected_label:?}, got {label:?}"
        );
        let expected = format!("{engine} version {release}");
        assert_eq!(
            description, &expected,
            "case {case}: expected the version description {expected:?}, got {description:?}"
        );
    }

    // A read does not change what the next caller sees.
    let once = entries("twice-first", &describe(&emulator, "twice-first", &[]).await);
    let twice = entries("twice-second", &describe(&emulator, "twice-second", &[]).await);
    assert_eq!(
        once, twice,
        "case twice: two reads of a published catalog must report the same entries"
    );
}
