// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetCompatibleVersions`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`]; this suite adds no second copy of it.
//!
//! The tests assert the RULE over sets of inputs rather than one recorded call: the
//! versions a source version can be upgraded to are the published releases of its own
//! upgrade LINE that are later than it, plus — from the NEWEST release of that line only
//! — every release of the line that immediately succeeds it. A line is an engine's major
//! release, except that the Elasticsearch 7 releases and the OpenSearch 1 releases are
//! one line, OpenSearch 1.0 being the fork of Elasticsearch 7.10. So an upgrade walks the
//! minors of a line freely and leaves the line only from its end. So:
//!
//! * the unfiltered answer is the published upgrade table: a row per release the
//!   service still offers an upgrade from — those at or after the published floor that
//!   have somewhere to go — oldest first, each with the targets the rule gives it;
//! * an upgrade only goes FORWARDS — no version is among its own targets, every
//!   target ranks above its source, targets are ascending, and a version's
//!   same-line targets are exactly the later published releases of that line;
//! * crossing a line boundary reaches the NEXT line only, and only from the end of a
//!   line: a source with later releases of its own line ahead of it reaches those and
//!   stops, and one at the end of its line reaches the whole of the succeeding line and
//!   nothing beyond it;
//! * `DomainName` limits the answer to the one version THAT domain runs, over a
//!   table of domains spanning both engines, several majors, the newest published
//!   version (which has no target), the default version a request omitting
//!   `EngineVersion` gets, and a pattern-valid version the catalog does not publish;
//!   a name no domain has is refused with the modelled error;
//! * the read changes nothing a later caller sees.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    SUPPORTED_ENGINE_VERSIONS,
};

use super::domain_lifecycle::{
    error_code, string_member, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS, SETTLE_WAIT,
};

/// The oldest release the published upgrade table has a row for — the suite's own
/// expectation of the floor the service offers an upgrade from.
const UPGRADE_FLOOR: &str = "Elasticsearch_5.1";

/// `GetCompatibleVersions`: `GET /2021-01-01/opensearch/compatibleVersions`, with
/// `DomainName` as its one `@httpQuery` member.
pub(crate) async fn compatible(emulator: &Emulator, domain_name: Option<&str>) -> Answer {
    let uri = match domain_name {
        None => format!("{PREFIX}/opensearch/compatibleVersions"),
        Some(name) => format!("{PREFIX}/opensearch/compatibleVersions?domainName={name}"),
    };
    emulator.call("GET", &uri, None).await
}

/// The `CompatibleVersions` list of a successful answer, with the case named on
/// failure.
pub(crate) fn entries(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected HTTP 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.body["CompatibleVersions"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a CompatibleVersions array, got {:?}",
                answer.body
            )
        })
        .clone()
}

/// One entry's `TargetVersions`, as strings. The member is always present: a version
/// with nothing to upgrade to reports an empty list.
pub(crate) fn target_versions(entry: &Value, case: &str) -> Vec<String> {
    entry["TargetVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a TargetVersions array, got {entry:?}"))
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected a string target, got {value:?}"))
                .to_owned()
        })
        .collect()
}

/// Where a version sits in the upgrade order, parsed HERE rather than read from the
/// service, so the expectations below are independent of the handler's own parse:
/// the engine generation first (the OpenSearch line succeeds the Elasticsearch one),
/// then the major and minor numbers as NUMBERS.
///
/// Panics for a value `VersionString`'s pattern
/// `^Elasticsearch_[0-9]{1}\.[0-9]{1,2}$|^OpenSearch_[0-9]{1,2}\.[0-9]{1,2}$` does
/// not allow, which is what makes it an assertion about every version the service
/// reports.
pub(crate) fn rank(version: &str) -> (u8, u32, u32) {
    let (generation, numbers) = if let Some(rest) = version.strip_prefix("OpenSearch_") {
        (1_u8, rest)
    } else if let Some(rest) = version.strip_prefix("Elasticsearch_") {
        (0_u8, rest)
    } else {
        panic!("case {version}: expected an Elasticsearch_ or OpenSearch_ version");
    };
    let (major, minor) = numbers
        .split_once('.')
        .unwrap_or_else(|| panic!("case {version}: expected a <major>.<minor> version"));
    let parse = |part: &str, which: &str| {
        part.parse::<u32>()
            .unwrap_or_else(|_| panic!("case {version}: expected a numeric {which}, got {part}"))
    };
    (generation, parse(major, "major"), parse(minor, "minor"))
}

/// The upgrade LINE of a version: its engine generation and major release number, the
/// unit an upgrade path is published along — except that the Elasticsearch 7 releases
/// and the OpenSearch 1 releases are ONE line, because OpenSearch 1.0 is the fork of
/// Elasticsearch 7.10.
pub(crate) fn line(version: &str) -> (u8, u32) {
    let (generation, major, _) = rank(version);
    if (generation, major) == (1, 1) {
        return (0, 7);
    }
    (generation, major)
}

/// Whether a version is the NEWEST release its line publishes — the one release of a
/// line the service offers an upgrade INTO the next line from.
fn is_newest_of_line(version: &str) -> bool {
    SUPPORTED_ENGINE_VERSIONS
        .iter()
        .filter(|candidate| line(candidate) == line(version))
        .all(|candidate| rank(candidate) <= rank(version))
}

/// Every release line the published catalog carries, oldest first and each once.
fn published_lines() -> Vec<(u8, u32)> {
    let mut lines: Vec<(u8, u32)> = SUPPORTED_ENGINE_VERSIONS.iter().map(|v| line(v)).collect();
    lines.sort_unstable();
    lines.dedup();
    lines
}

/// The line that immediately succeeds one line, or `None` for the newest.
fn succeeding_line(of: (u8, u32)) -> Option<(u8, u32)> {
    published_lines().into_iter().find(|candidate| *candidate > of)
}

/// The rule, applied to one source version: the later published releases of its own
/// line, plus — from the newest release of that line only — every release of the line
/// that immediately succeeds it. Oldest first.
fn expected_targets(source_version: &str) -> Vec<String> {
    let source = rank(source_version);
    let source_line = line(source_version);
    let next_line = if is_newest_of_line(source_version) {
        succeeding_line(source_line)
    } else {
        None
    };
    SUPPORTED_ENGINE_VERSIONS
        .iter()
        .filter(|candidate| {
            let candidate_line = line(candidate);
            (candidate_line == source_line && rank(candidate) > source)
                || (next_line.is_some() && Some(candidate_line) == next_line)
        })
        .map(|candidate| (*candidate).to_string())
        .collect()
}

/// The source versions the published table lists, oldest first: the releases at or
/// after the published upgrade floor that have at least one target. The floor is
/// restated here as the suite's OWN expectation rather than imported from
/// `crate::state`, so moving it in the handler fails this suite instead of being
/// asserted against itself.
fn expected_sources() -> Vec<String> {
    SUPPORTED_ENGINE_VERSIONS
        .iter()
        .filter(|version| rank(version) >= rank(UPGRADE_FLOOR))
        .filter(|version| !expected_targets(version).is_empty())
        .map(|version| (*version).to_string())
        .collect()
}

/// Create a domain, optionally at a named engine version, and let the provisioning
/// span fall due so the record is settled before it is read.
async fn settled_domain(emulator: &Emulator, name: &str, engine_version: Option<&str>) {
    let mut body = json!({ "DomainName": name });
    if let Some(version) = engine_version {
        body["EngineVersion"] = json!(version);
    }
    let created = emulator.create(body).await;
    assert_eq!(
        created.status, 200,
        "case {name} / {engine_version:?}: expected the domain to be created, got {} {:?}",
        created.status, created.body
    );
    tokio::time::sleep(SETTLE_WAIT).await;
}

#[tokio::test]
async fn table_reports_every_upgradable_version_in_upgrade_order() {
    let emulator = Emulator::new();
    let table = entries(&compatible(&emulator, None).await, "whole table");

    let sources: Vec<String> = table
        .iter()
        .map(|entry| string_member(entry, "SourceVersion", "table entry"))
        .collect();
    assert_eq!(
        sources,
        expected_sources(),
        "case whole table: expected a row per release the service offers an upgrade from, \
         oldest first, got {sources:?}"
    );

    // The published set is itself in upgrade order, so the table a caller walks is
    // ordered oldest first.
    for pair in SUPPORTED_ENGINE_VERSIONS.windows(2) {
        assert!(
            rank(pair[0]) < rank(pair[1]),
            "case {} vs {}: expected the published set in upgrade order, got ranks {:?} \
             and {:?}",
            pair[0],
            pair[1],
            rank(pair[0]),
            rank(pair[1])
        );
    }

    for entry in &table {
        let source = string_member(entry, "SourceVersion", "table entry");
        let targets = target_versions(entry, &source);
        assert_eq!(
            targets,
            expected_targets(&source),
            "case {source}: expected the releases its line and the next one publish ahead \
             of it, got {targets:?}"
        );
        // A table of "the versions you can upgrade to" has a row only where there is
        // somewhere to go.
        assert!(
            !targets.is_empty(),
            "case {source}: expected a listed source to have a target, got {entry:?}"
        );
    }

    // The newest published version is the end of the path: nothing to upgrade to, so
    // the table has no row for it at all.
    let newest = SUPPORTED_ENGINE_VERSIONS[SUPPORTED_ENGINE_VERSIONS.len() - 1];
    assert!(
        !sources.contains(&newest.to_string()),
        "case {newest}: the newest published version has nothing to upgrade to, so the \
         table lists no row for it, got {sources:?}"
    );
    // Nor has it a row through a release older than the published floor.
    for retired in SUPPORTED_ENGINE_VERSIONS
        .iter()
        .filter(|version| rank(version) < rank(UPGRADE_FLOOR))
    {
        assert!(
            !sources.contains(&(*retired).to_string()),
            "case {retired}: expected no row for a release the service offers no upgrade \
             from, got {sources:?}"
        );
    }
    assert_eq!(
        sources.first().map(String::as_str),
        Some(UPGRADE_FLOOR),
        "case the table's first row: expected the published floor, got {sources:?}"
    );
}

#[tokio::test]
async fn targets_only_ever_go_forwards() {
    let emulator = Emulator::new();
    let table = entries(&compatible(&emulator, None).await, "forwards");

    let published: Vec<String> = SUPPORTED_ENGINE_VERSIONS
        .iter()
        .map(|version| (*version).to_string())
        .collect();

    let mut previous: Option<(String, Vec<String>)> = None;
    for entry in &table {
        let source = string_member(entry, "SourceVersion", "forwards entry");
        let targets = target_versions(entry, &source);

        assert!(
            !targets.contains(&source),
            "case {source}: an upgrade never targets the version it starts from, got \
             {targets:?}"
        );
        for target in &targets {
            assert!(
                rank(target) > rank(&source),
                "case {source} -> {target}: expected a target later than the source, got \
                 ranks {:?} and {:?}",
                rank(&source),
                rank(target)
            );
            assert!(
                published.contains(target),
                "case {source} -> {target}: expected a published version as a target"
            );
        }
        for pair in targets.windows(2) {
            assert!(
                rank(&pair[0]) < rank(&pair[1]),
                "case {source}: expected the targets oldest first, got {:?} then {:?}",
                pair[0],
                pair[1]
            );
        }

        // Within one release line, the targets are exactly the published releases of
        // that line after the source: no hop is missing and none is invented.
        let same_line: Vec<String> = targets
            .iter()
            .filter(|target| line(target) == line(&source))
            .cloned()
            .collect();
        let expected_same_line: Vec<String> = published
            .iter()
            .filter(|candidate| {
                line(candidate) == line(&source) && rank(candidate) > rank(&source)
            })
            .cloned()
            .collect();
        assert_eq!(
            same_line, expected_same_line,
            "case {source}: expected every later published release of its own line, got \
             {same_line:?}"
        );

        // Crossing a line boundary reaches the NEXT line only, and only from the end of
        // a line: the whole of the line that succeeds the source's when the source is the
        // newest release of its own, and nothing at all otherwise.
        let crossed: Vec<String> = targets
            .iter()
            .filter(|target| line(target) != line(&source))
            .cloned()
            .collect();
        let expected_crossed: Vec<String> = match succeeding_line(line(&source)) {
            Some(next) if is_newest_of_line(&source) => published
                .iter()
                .filter(|candidate| line(candidate) == next)
                .cloned()
                .collect(),
            _ => Vec::new(),
        };
        assert_eq!(
            crossed, expected_crossed,
            "case {source}: a release reaches the whole of the line that succeeds its own \
             and no line beyond it, got {crossed:?}"
        );

        // Within one line, each listed version's targets are the previous one's minus
        // that version itself — the table is one monotone path, not a set of unrelated
        // answers. The release at the END of a line is where the path leaves it, so it
        // is the one source the step does not hold for.
        if let Some((earlier, earlier_targets)) = previous {
            if line(&earlier) == line(&source) && !is_newest_of_line(&source) {
                assert_eq!(
                    earlier_targets.first().map(String::as_str),
                    Some(source.as_str()),
                    "case {earlier}: expected the next published version first among its \
                     targets, got {earlier_targets:?}"
                );
                assert_eq!(
                    targets,
                    earlier_targets[1..].to_vec(),
                    "case {earlier} -> {source}: expected the later source's targets to be \
                     the earlier one's minus itself, got {targets:?}"
                );
            }
        }
        previous = Some((source, targets));
    }
}

#[tokio::test]
async fn domain_filter_reports_the_version_that_domain_runs() {
    let emulator = Emulator::new();

    // One row per way a domain's engine version can differ: both engines, several
    // majors, a two-digit minor, the newest published version (no target at all), the
    // default a request omitting `EngineVersion` gets, and a pattern-valid version the
    // catalog does not publish.
    let newest = SUPPORTED_ENGINE_VERSIONS[SUPPORTED_ENGINE_VERSIONS.len() - 1];
    let rows: [(&str, Option<&str>, &str); 7] = [
        ("filter-es-five", Some("Elasticsearch_5.6"), "Elasticsearch_5.6"),
        ("filter-es-six", Some("Elasticsearch_6.8"), "Elasticsearch_6.8"),
        (
            "filter-es-seven-ten",
            Some("Elasticsearch_7.10"),
            "Elasticsearch_7.10",
        ),
        ("filter-os-one", Some("OpenSearch_1.3"), "OpenSearch_1.3"),
        ("filter-os-newest", Some(newest), newest),
        (
            "filter-default",
            None,
            super::domain_lifecycle::default_engine_version(),
        ),
        (
            "filter-unpublished",
            Some("OpenSearch_2.12"),
            "OpenSearch_2.12",
        ),
    ];

    for (name, requested, expected_source) in rows {
        settled_domain(&emulator, name, requested).await;
        let answer = compatible(&emulator, Some(name)).await;
        let filtered = entries(&answer, name);
        assert_eq!(
            filtered.len(),
            1,
            "case {name}: naming a domain limits the answer to that one domain's version, \
             got {filtered:?}"
        );
        assert_eq!(
            string_member(&filtered[0], "SourceVersion", name),
            expected_source,
            "case {name}: expected the version the domain runs, got {:?}",
            filtered[0]
        );
        let targets = target_versions(&filtered[0], name);
        assert_eq!(
            targets,
            expected_targets(expected_source),
            "case {name}: expected the published versions later than {expected_source}, got \
             {targets:?}"
        );
    }

    // The newest published version is a terminus even when it is a domain's version:
    // the filtered answer reports its row with an empty target list, which the
    // unfiltered table has no row for at all.
    let terminus = compatible(&emulator, Some("filter-os-newest")).await;
    assert!(
        target_versions(&entries(&terminus, "filter-os-newest")[0], "filter-os-newest").is_empty(),
        "case filter-os-newest: a domain on the newest version has nothing to upgrade to, \
         got {:?}",
        terminus.body
    );

    // A pattern-valid name no domain has names no resource.
    for absent in ["no-such-domain", "filter-es-fivex", "abc"] {
        let answer = compatible(&emulator, Some(absent)).await;
        assert_eq!(
            answer.status, RECORDED_NOT_FOUND_STATUS,
            "case {absent}: expected HTTP {RECORDED_NOT_FOUND_STATUS}, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {absent}: expected the modelled not-found refusal, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn reading_the_table_changes_nothing() {
    let emulator = Emulator::new();
    settled_domain(&emulator, "compat-read-twice", Some("Elasticsearch_6.8")).await;

    let first_table = entries(&compatible(&emulator, None).await, "first read");
    let first_filtered = entries(
        &compatible(&emulator, Some("compat-read-twice")).await,
        "first filtered read",
    );
    let described = emulator.describe("compat-read-twice").await.body;

    // A refused read reports nothing and changes nothing either.
    compatible(&emulator, Some("no-such-domain")).await;

    let second_table = entries(&compatible(&emulator, None).await, "second read");
    let second_filtered = entries(
        &compatible(&emulator, Some("compat-read-twice")).await,
        "second filtered read",
    );
    assert_eq!(
        first_table, second_table,
        "case repeat read: expected the same table the first read reported, got {} entries",
        second_table.len()
    );
    assert_eq!(
        first_filtered, second_filtered,
        "case repeat filtered read: expected the same answer the first read reported, got \
         {second_filtered:?}"
    );
    assert_eq!(
        described,
        emulator.describe("compat-read-twice").await.body,
        "case describe after reads: expected the domain record the first describe \
         reported"
    );

    let store = emulator.store();
    let state = store.lock();
    assert_eq!(
        state.domains.len(),
        1,
        "case stored domains: reporting compatible versions creates no domain, got {:?}",
        state.domains.keys().collect::<Vec<_>>()
    );
    let domain = state
        .domains
        .get("compat-read-twice")
        .expect("the created domain is stored");
    assert_eq!(
        domain.engine_version, "Elasticsearch_6.8",
        "case stored engine version: reporting compatible versions upgrades nothing, got {}",
        domain.engine_version
    );
}
