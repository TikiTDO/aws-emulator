// SPDX-License-Identifier: CC0-1.0

//! The committed `Cargo.lock` records each member's dependencies as the resolve cargo would
//! write, not merely the crates it needs.
//!
//! A release build resolves with `--locked`, which lets cargo write nothing: if the resolve
//! it computes differs from the committed lock in ANY respect, it stops the build before a
//! crate compiles with `cannot update the lock file ... because --locked was passed`. The
//! lock records a resolve as one `[[package]]` per crate, each carrying the edges to the
//! crates it depends on and, for a crate off a registry, the checksum of the archive to
//! fetch. Two of those parts can be missing while every crate a manifest names is still
//! present somewhere in the file:
//!
//! * a dependency added to one member's manifest whose crate another member already locks —
//!   the `[[package]]` is there, the EDGE from this member to it is not; and
//! * a `[[package]]` off a registry with no `checksum`, which cargo can only complete by
//!   writing the lock.
//!
//! The sibling suite `workspace_lockfile_covers_every_manifest` pairs the manifests with the
//! lock's `[[package]]` list, so it passes on both: this one pairs them with each member's
//! own edge list and holds every registry package to carrying its checksum. Together they
//! fail here — in the workspace's own tests — on a lock a `--locked` build would refuse,
//! instead of in a platform build nobody can rerun locally.
//!
//! It reads the two committed files only; it resolves nothing, spawns nothing and reaches no
//! network. Both files are read through the sibling suite, which owns the one reader of each
//! format: `lock_entries` for the lock's records, `dependencies_in` and `resolve_declared`
//! for a manifest's declarations, `root_and_member_manifests` for the texts themselves. Each
//! check here is a function over those records, driven once over the committed files and once
//! over synthetic ones, so it is known to REPORT what it is there to catch rather than only
//! to pass on the tree as it stands.

use std::collections::{BTreeMap, BTreeSet};

use crate::workspace_lockfile_covers_every_manifest::{
    committed_lock, dependencies_in, is_member_dependency_table, lock_entries, locked_crate_name,
    package_name, resolve_declared, root_and_member_manifests, workspace_dependency_table,
    Declared, LockEntry,
};

/// How many `[[package]]` records a read of the committed lock must return before a check
/// over it means anything: this workspace locks hundreds, so a smaller number is a reader
/// that stopped early rather than a lock that is in order.
const LEAST_EXPECTED_PACKAGES: usize = 100;

/// The lock's entries for the crates built out of this checkout — the workspace's members and
/// the vendored path dependencies — keyed by crate name. They carry no source, and a crate
/// name is locked once among them.
fn source_free_entries(entries: &[LockEntry]) -> BTreeMap<String, LockEntry> {
    entries
        .iter()
        .filter(|entry| entry.source.is_none())
        .map(|entry| (entry.name.clone(), entry.clone()))
        .collect()
}

/// The crates a member's manifest depends on, under the names the LOCK knows them by: a
/// declaration renamed with `package = "…"`, in the manifest itself or in the root
/// `[workspace.dependencies]` entry it defers to, reaches the lock under the renamed crate.
///
/// Every dependency table counts, because the lock keeps one edge list per package rather
/// than one per kind: a dev-dependency, a build-dependency and a platform-specific
/// dependency are all edges of the same `[[package]]`.
fn declared_crates(manifest: &str, workspace_deps: &BTreeMap<String, Declared>) -> BTreeSet<String> {
    let mut crates = BTreeSet::new();
    for (_table, dependency, spec) in dependencies_in(manifest, is_member_dependency_table) {
        // A `workspace = true` the root table does not declare is the sibling suite's report
        // to make; it leaves nothing for this one to pair.
        if let Some(resolved) = resolve_declared(&dependency, &spec, workspace_deps) {
            crates.insert(locked_crate_name(&dependency, &resolved));
        }
    }
    crates
}

/// One report per way the lock's edge list for the member `manifest` defines differs from the
/// dependencies that manifest declares: an edge the lock omits, which cargo would have to add,
/// and an edge the lock keeps that nothing declares any more, which cargo would have to remove.
fn edge_reports(
    manifest_path: &str,
    manifest: &str,
    workspace_deps: &BTreeMap<String, Declared>,
    locked: &BTreeMap<String, LockEntry>,
) -> Vec<String> {
    let Some(name) = package_name(manifest) else {
        return vec![format!(
            "case {manifest_path:?}: expected the manifest to name a [package], got none"
        )];
    };
    let Some(entry) = locked.get(&name) else {
        return vec![format!(
            "case {manifest_path:?}: expected Cargo.lock to carry the member package {name:?} \
             as a [[package]] built out of this checkout, got none"
        )];
    };
    let declared = declared_crates(manifest, workspace_deps);
    let mut reports = Vec::new();
    for crate_name in declared.difference(&entry.dependencies) {
        reports.push(format!(
            "case {manifest_path:?} -> {crate_name:?}: expected the [[package]] {name:?} in \
             Cargo.lock to list it among its {} dependencies, got none, so a --locked build \
             cannot resolve this manifest",
            entry.dependencies.len()
        ));
    }
    for crate_name in entry.dependencies.difference(&declared) {
        reports.push(format!(
            "case {manifest_path:?} -> {crate_name:?}: expected the manifest to declare the \
             dependency the [[package]] {name:?} in Cargo.lock lists, got no declaration, so a \
             --locked build cannot resolve this manifest"
        ));
    }
    reports
}

/// One report per registry package the lock carries without the checksum cargo needs to fetch
/// it; a package built out of this checkout has neither a source nor a checksum.
fn checksum_reports(entries: &[LockEntry]) -> Vec<String> {
    entries
        .iter()
        .filter(|entry| entry.is_from_registry() && entry.checksum.is_none())
        .map(|entry| {
            format!(
                "case {:?} {:?}: expected the locked registry package to carry the checksum \
                 cargo fetches it by, got none",
                entry.name, entry.version
            )
        })
        .collect()
}

/// One report per optional dependency a manifest declares. An optional dependency reaches the
/// lock's edge list only when a feature turns it on, so it is the one declaration this suite's
/// pairing cannot read as an edge; this workspace has none, and this reports the first.
fn optional_dependency_reports(manifest_path: &str, manifest: &str) -> Vec<String> {
    dependencies_in(manifest, |section| {
        is_member_dependency_table(section) || section == "[workspace.dependencies]"
    })
    .into_iter()
    .filter(|(_table, _dependency, spec)| spec.optional)
    .map(|(table, dependency, _spec)| {
        format!(
            "case {manifest_path:?} {table} -> {dependency:?}: expected every dependency to be \
             unconditional, which is what this suite pairs with the lock's edges, got \
             `optional = true`"
        )
    })
    .collect()
}

/// The root manifest, the lock's packages, and every member's manifest text.
fn workspace() -> (String, Vec<LockEntry>, Vec<(String, String)>) {
    let (root_manifest, manifests) = root_and_member_manifests();
    let entries = lock_entries(&committed_lock());
    assert!(
        entries.len() > LEAST_EXPECTED_PACKAGES,
        "expected Cargo.lock to carry the workspace's packages, read {} of them",
        entries.len()
    );
    (root_manifest, entries, manifests)
}

#[test]
fn every_dependency_a_member_declares_is_an_edge_of_its_locked_package() {
    let (root_manifest, entries, manifests) = workspace();
    let workspace_deps = workspace_dependency_table(&root_manifest);
    let locked = source_free_entries(&entries);
    let mut reports = Vec::new();
    for (manifest, text) in &manifests {
        reports.extend(edge_reports(manifest, text, &workspace_deps, &locked));
    }
    assert!(
        reports.is_empty(),
        "expected the lock's edges for each member to be exactly the dependencies it declares, \
         got {} that differ, so the release build cannot resolve with --locked: {reports:#?}",
        reports.len()
    );
}

#[test]
fn every_locked_registry_package_carries_its_checksum() {
    let (_, entries, _) = workspace();
    let reports = checksum_reports(&entries);
    assert!(
        reports.is_empty(),
        "expected every locked registry package to carry a checksum, got {} without one, so \
         the release build cannot resolve with --locked: {reports:#?}",
        reports.len()
    );
    let from_registry = entries
        .iter()
        .filter(|entry| entry.is_from_registry())
        .count();
    assert!(
        from_registry > LEAST_EXPECTED_PACKAGES,
        "expected the lock to carry the workspace's registry packages, counted {from_registry}"
    );
}

#[test]
fn no_manifest_declares_an_optional_dependency() {
    let (root_manifest, _, manifests) = workspace();
    let mut reports = optional_dependency_reports("Cargo.toml", &root_manifest);
    for (manifest, text) in &manifests {
        reports.extend(optional_dependency_reports(manifest, text));
    }
    assert!(
        reports.is_empty(),
        "expected no manifest to declare an optional dependency, got {} that do: {reports:#?}",
        reports.len()
    );
    // The declarations this guard is here to catch, and those beside them that stay accepted —
    // including a feature whose NAME carries the word, which turns nothing optional.
    let cases = [
        (
            "an optional dependency",
            "[dependencies]\nlocked-crate = { version = \"1\", optional = true }\n",
            1,
        ),
        (
            "an optional root dependency",
            "[workspace.dependencies]\nlocked-crate = { version = \"1\", optional = true }\n",
            1,
        ),
        (
            "an optional dependency declared over several lines",
            "[dependencies]\nlocked-crate = { version = \"1\", features = [\n    \"one\",\n], optional = true }\n",
            1,
        ),
        (
            "an unconditional dependency",
            "[dependencies]\nlocked-crate = \"1\"\n",
            0,
        ),
        (
            "a feature named for the word",
            "[dependencies]\nlocked-crate = { version = \"1\", features = [\"optional-tls\"] }\n",
            0,
        ),
        (
            "the word in a comment",
            "[dependencies]\n# optional one day\nlocked-crate = \"1\"\n",
            0,
        ),
    ];
    for case in cases {
        let (name, manifest, expected) = case;
        let reports = optional_dependency_reports(name, manifest);
        assert_eq!(
            reports.len(),
            expected,
            "case {name:?}: expected {expected} report(s), got {reports:#?}"
        );
    }
}

/// A lock holding one member crate, the registry crates it depends on — one of them locked
/// twice, so its edge carries a version — and a registry crate with no checksum.
const SYNTHETIC_LOCK: &str = r#"
version = 4

[[package]]
name = "member-crate"
version = "0.1.0"
dependencies = [
 "locked-crate",
 "renamed-crate",
 "sha2 0.10.9",
]

[[package]]
name = "locked-crate"
version = "1.2.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "1111"
dependencies = [
 "sha2 0.9.9",
]

[[package]]
name = "renamed-crate"
version = "0.4.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "2222"

[[package]]
name = "sha2"
version = "0.9.9"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "3333"

[[package]]
name = "sha2"
version = "0.10.9"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[patch.unused]]
name = "unused-crate"
version = "1.0.0"
"#;

#[test]
fn the_lock_reader_reads_a_package_its_source_and_its_edges() {
    let entries = lock_entries(SYNTHETIC_LOCK);
    let names: Vec<(&str, &str)> = entries
        .iter()
        .map(|entry| (entry.name.as_str(), entry.version.as_str()))
        .collect();
    assert_eq!(
        names,
        vec![
            ("member-crate", "0.1.0"),
            ("locked-crate", "1.2.3"),
            ("renamed-crate", "0.4.0"),
            ("sha2", "0.9.9"),
            ("sha2", "0.10.9"),
        ],
        "case the synthetic lock: expected one entry per [[package]] and nothing from the \
         [[patch.unused]] table after them, got {names:?}"
    );
    let member = &entries[0];
    assert!(
        !member.is_from_registry() && member.checksum.is_none(),
        "case \"member-crate\": expected the member package to carry no source and no \
         checksum, got {member:?}"
    );
    let edges: Vec<&str> = member.dependencies.iter().map(String::as_str).collect();
    assert_eq!(
        edges,
        vec!["locked-crate", "renamed-crate", "sha2"],
        "case \"member-crate\": expected the edge list under the crate names a manifest \
         declares, with the version of the twice-locked crate dropped, got {edges:?}"
    );
    let versioned_edge: Vec<&str> = entries[1].dependencies.iter().map(String::as_str).collect();
    assert_eq!(
        versioned_edge,
        vec!["sha2"],
        "case \"locked-crate\": expected an edge written with its version to read as the \
         crate name, got {versioned_edge:?}"
    );
}

#[test]
fn a_registry_package_without_a_checksum_is_reported() {
    let reports = checksum_reports(&lock_entries(SYNTHETIC_LOCK));
    assert_eq!(
        reports.len(),
        1,
        "case the synthetic lock: expected the one registry package written without a \
         checksum to be reported, and neither the member package nor the registry packages \
         that carry one, got {reports:#?}"
    );
    assert!(
        reports[0].contains("\"sha2\"") && reports[0].contains("\"0.10.9\""),
        "case the synthetic lock: expected the report to name the package and version it is \
         about, got {:?}",
        reports[0]
    );
}

#[test]
fn an_edge_the_lock_omits_or_keeps_is_reported() {
    let locked = source_free_entries(&lock_entries(SYNTHETIC_LOCK));
    let workspace_deps = workspace_dependency_table(
        "[workspace.dependencies]\n\
         locked-crate = \"1\"\n\
         alias = { version = \"0.4\", package = \"renamed-crate\" }\n\
         sha2 = \"0.10\"\n",
    );
    // Each manifest names `member-crate`, whose locked edges are `locked-crate`,
    // `renamed-crate` and `sha2`, so the rows differ only in what it declares and how.
    let member = "[package]\nname = \"member-crate\"\n\n";
    let whole_edge_list =
        "[dependencies]\nlocked-crate = \"1\"\nalias = { workspace = true }\nsha2 = \"0.10\"\n";
    let cases = [
        (
            "every edge, declared plainly",
            format!("{member}{whole_edge_list}"),
            0,
        ),
        (
            "every edge, one renamed in the manifest itself",
            format!("{member}[dependencies]\nlocked-crate = \"1\"\nalias = {{ version = \"0.4\", package = \"renamed-crate\" }}\nsha2 = \"0.10\"\n"),
            0,
        ),
        (
            "every edge, spread over the three dependency kinds",
            format!("{member}[dependencies]\nlocked-crate = \"1\"\n\n[dev-dependencies]\nalias = {{ workspace = true }}\n\n[build-dependencies]\nsha2 = \"0.10\"\n"),
            0,
        ),
        (
            "every edge, one of them platform-specific",
            format!("{member}[dependencies]\nlocked-crate = \"1\"\nalias = {{ workspace = true }}\n\n[target.'cfg(unix)'.dependencies]\nsha2 = \"0.10\"\n"),
            0,
        ),
        (
            "every edge, and a declaration the lock has no edge for",
            format!("{member}{whole_edge_list}absent-crate = \"1\"\n"),
            1,
        ),
        (
            "every edge, and two declarations the lock has no edge for",
            format!("{member}{whole_edge_list}absent-crate = \"1\"\nanother-absent-crate = \"2\"\n"),
            2,
        ),
        (
            "an edge dropped from the manifest",
            format!("{member}[dependencies]\nlocked-crate = \"1\"\nsha2 = \"0.10\"\n"),
            1,
        ),
        (
            "a renamed declaration left under the name the manifest uses",
            format!("{member}[dependencies]\nlocked-crate = \"1\"\nalias = \"0.4\"\nsha2 = \"0.10\"\n"),
            2,
        ),
        (
            "a member the lock does not carry",
            "[package]\nname = \"absent-member\"\n\n[dependencies]\nlocked-crate = \"1\"\n"
                .to_string(),
            1,
        ),
        (
            "a manifest that names no package",
            whole_edge_list.to_string(),
            1,
        ),
    ];
    for case in &cases {
        let (name, manifest, expected) = case;
        let reports = edge_reports(name, manifest, &workspace_deps, &locked);
        assert_eq!(
            reports.len(),
            *expected,
            "case {name:?}: expected {expected} report(s), got {reports:#?}"
        );
    }
}
