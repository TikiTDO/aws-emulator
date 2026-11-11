// SPDX-License-Identifier: CC0-1.0

//! The Amazon EMR RELEASE CATALOG: which releases this emulator offers, what each of them
//! bundles, and the Amazon Linux releases they are built on.
//!
//! One concern of this service's state, split out of [`super`]: `DescribeReleaseLabel` and
//! `ListReleaseLabels` publish this catalog, a cluster's `ReleaseLabel` is resolved in it,
//! and `Cluster$Applications` report the versions the cluster's release ships
//! ([`super::installed_applications`]) — so the one table lives here and every reader reaches
//! it through `crate::state`.

use super::*;

/// One Amazon EMR release line (`emr-<major>.<minor>`) and the patch releases published
/// on it. The labels the catalog offers are exactly these `(line, patch)` pairs.
#[derive(Debug, Clone, Copy)]
pub struct ReleaseLine {
    pub major: u32,
    pub minor: u32,
    pub patches: &'static [u32],
}

/// Every release line this emulator offers, oldest first.
pub const RELEASE_LINES: &[ReleaseLine] = &[
    ReleaseLine { major: 5, minor: 30, patches: &[0, 1, 2] },
    ReleaseLine { major: 5, minor: 31, patches: &[0] },
    ReleaseLine { major: 5, minor: 32, patches: &[0] },
    ReleaseLine { major: 5, minor: 33, patches: &[0, 1] },
    ReleaseLine { major: 5, minor: 34, patches: &[0] },
    ReleaseLine { major: 5, minor: 35, patches: &[0] },
    ReleaseLine { major: 5, minor: 36, patches: &[0, 1, 2] },
    ReleaseLine { major: 6, minor: 0, patches: &[0, 1] },
    ReleaseLine { major: 6, minor: 1, patches: &[0, 1] },
    ReleaseLine { major: 6, minor: 2, patches: &[0, 1] },
    ReleaseLine { major: 6, minor: 3, patches: &[0, 1] },
    ReleaseLine { major: 6, minor: 4, patches: &[0] },
    ReleaseLine { major: 6, minor: 5, patches: &[0] },
    ReleaseLine { major: 6, minor: 6, patches: &[0] },
    ReleaseLine { major: 6, minor: 7, patches: &[0] },
    ReleaseLine { major: 6, minor: 8, patches: &[0] },
    ReleaseLine { major: 6, minor: 9, patches: &[0] },
    ReleaseLine { major: 6, minor: 10, patches: &[0, 1] },
    ReleaseLine { major: 6, minor: 11, patches: &[0, 1] },
    ReleaseLine { major: 6, minor: 12, patches: &[0] },
    ReleaseLine { major: 6, minor: 13, patches: &[0] },
    ReleaseLine { major: 6, minor: 14, patches: &[0] },
    ReleaseLine { major: 6, minor: 15, patches: &[0] },
    ReleaseLine { major: 7, minor: 0, patches: &[0] },
    ReleaseLine { major: 7, minor: 1, patches: &[0] },
    ReleaseLine { major: 7, minor: 2, patches: &[0] },
    ReleaseLine { major: 7, minor: 3, patches: &[0] },
    ReleaseLine { major: 7, minor: 4, patches: &[0] },
    ReleaseLine { major: 7, minor: 5, patches: &[0] },
    // The lines published since: Amazon EMR offers every release of the series, so a
    // launch naming one of them resolves rather than being refused. Source for the labels
    // is the Release Guide's release-version table, as for every line above; each bundles
    // the application versions the newest row at or before its line carries
    // ([`CatalogApplication::version_at`]), which is the rule that already gives a patch
    // release its versions.
    ReleaseLine { major: 7, minor: 6, patches: &[0] },
    ReleaseLine { major: 7, minor: 7, patches: &[0] },
    ReleaseLine { major: 7, minor: 8, patches: &[0] },
    ReleaseLine { major: 7, minor: 9, patches: &[0] },
];

/// One application in the catalog.
#[derive(Debug, Clone, Copy)]
pub struct CatalogApplication {
    /// `SimplifiedApplication$Name`: the one identifier Amazon EMR publishes for this
    /// application, reported verbatim and never case-folded.
    pub name: &'static str,
    /// The version this application carries from a release line onward: a row applies
    /// to every release at or after its line, up to the next row's line. Rows are in
    /// ascending line order, and a release BEFORE the first row predates the
    /// application, which such a release does not bundle at all.
    pub versions: &'static [((u32, u32), &'static str)],
    /// The line from which Amazon EMR no longer bundles this application, when it
    /// dropped it.
    pub removed_from: Option<(u32, u32)>,
}

impl CatalogApplication {
    /// The version this application carries on the release line `(major, minor)`, or
    /// `None` when that release does not bundle it.
    pub fn version_at(&self, major: u32, minor: u32) -> Option<&'static str> {
        if self.removed_from.is_some_and(|from| (major, minor) >= from) {
            return None;
        }
        self.versions
            .iter()
            .rfind(|(line, _)| *line <= (major, minor))
            .map(|&(_, version)| version)
    }
}

/// The applications Amazon EMR bundles, with the version each release line carries.
pub const APPLICATION_CATALOG: &[CatalogApplication] = &[
    CatalogApplication {
        name: "Flink",
        versions: &[
            ((5, 30), "1.10.0"),
            ((5, 33), "1.12.1"),
            ((5, 36), "1.14.2"),
            ((6, 0), "1.9.1"),
            ((6, 1), "1.11.2"),
            ((6, 6), "1.14.2"),
            ((6, 9), "1.15.2"),
            ((6, 12), "1.17.0"),
            ((6, 15), "1.17.1"),
            ((7, 0), "1.18.0"),
            ((7, 2), "1.19.0"),
        ],
        removed_from: None,
    },
    CatalogApplication {
        name: "Ganglia",
        versions: &[((5, 30), "3.7.2"), ((6, 0), "3.7.2")],
        // Amazon EMR 7.0.0 no longer bundles Ganglia.
        removed_from: Some((7, 0)),
    },
    CatalogApplication {
        name: "Hadoop",
        versions: &[
            ((5, 30), "2.8.5"),
            ((5, 31), "2.10.0"),
            ((5, 33), "2.10.1"),
            ((6, 0), "3.2.1"),
            ((6, 9), "3.3.3"),
            ((6, 15), "3.3.6"),
            ((7, 5), "3.4.0"),
        ],
        removed_from: None,
    },
    CatalogApplication {
        name: "HBase",
        versions: &[
            ((5, 30), "1.4.13"),
            ((6, 0), "2.2.3"),
            ((6, 6), "2.4.4"),
            ((6, 9), "2.4.13"),
            ((6, 15), "2.4.17"),
            ((7, 2), "2.5.8"),
        ],
        removed_from: None,
    },
    CatalogApplication {
        name: "Hive",
        versions: &[
            ((5, 30), "2.3.6"),
            ((5, 33), "2.3.7"),
            ((5, 36), "2.3.9"),
            ((6, 0), "3.1.2"),
            ((6, 9), "3.1.3"),
        ],
        removed_from: None,
    },
    CatalogApplication {
        name: "Hudi",
        versions: &[
            ((5, 30), "0.5.2"),
            ((5, 36), "0.10.0"),
            ((6, 0), "0.5.0"),
            ((6, 6), "0.10.1"),
            ((6, 9), "0.12.1"),
            ((6, 12), "0.13.1"),
            ((6, 15), "0.14.0"),
            ((7, 2), "0.15.0"),
        ],
        removed_from: None,
    },
    CatalogApplication {
        name: "Hue",
        versions: &[
            ((5, 30), "4.6.0"),
            ((5, 36), "4.10.0"),
            ((6, 0), "4.4.0"),
            ((6, 6), "4.10.0"),
            ((6, 9), "4.11.0"),
        ],
        removed_from: None,
    },
    CatalogApplication {
        name: "Iceberg",
        // Amazon EMR offers Iceberg from 6.5.0 onward.
        versions: &[
            ((6, 5), "0.11.0"),
            ((6, 9), "0.14.1"),
            ((6, 12), "1.2.1"),
            ((6, 15), "1.4.0"),
            ((7, 0), "1.4.2"),
            ((7, 2), "1.5.0"),
        ],
        removed_from: None,
    },
    CatalogApplication {
        name: "JupyterEnterpriseGateway",
        versions: &[((6, 0), "2.1.0"), ((6, 9), "2.6.0")],
        removed_from: None,
    },
    CatalogApplication {
        name: "JupyterHub",
        versions: &[((5, 30), "1.1.0"), ((5, 36), "1.4.1"), ((6, 0), "1.1.0"), ((6, 6), "1.4.1")],
        removed_from: None,
    },
    CatalogApplication {
        name: "Livy",
        versions: &[((5, 30), "0.7.0"), ((5, 36), "0.7.1"), ((6, 0), "0.7.0"), ((6, 6), "0.7.1")],
        removed_from: None,
    },
    CatalogApplication {
        name: "Oozie",
        versions: &[
            ((5, 30), "5.2.0"),
            ((5, 36), "5.2.1"),
            ((6, 0), "5.1.0"),
            ((6, 1), "5.2.0"),
            ((6, 6), "5.2.1"),
        ],
        removed_from: None,
    },
    CatalogApplication {
        name: "Phoenix",
        versions: &[
            ((5, 30), "4.14.3"),
            ((6, 0), "5.0.0"),
            ((6, 2), "5.1.2"),
            ((7, 0), "5.1.3"),
        ],
        removed_from: None,
    },
    CatalogApplication {
        name: "Pig",
        versions: &[((5, 30), "0.17.0"), ((6, 0), "0.17.0")],
        removed_from: None,
    },
    CatalogApplication {
        name: "Presto",
        versions: &[
            ((5, 30), "0.232"),
            ((5, 36), "0.267"),
            ((6, 0), "0.230"),
            ((6, 6), "0.267"),
            ((6, 9), "0.276"),
            ((6, 15), "0.283"),
            ((7, 0), "0.284"),
        ],
        removed_from: None,
    },
    CatalogApplication {
        name: "Spark",
        versions: &[
            ((5, 30), "2.4.5"),
            ((5, 33), "2.4.7"),
            ((5, 36), "2.4.8"),
            ((6, 0), "2.4.4"),
            ((6, 1), "3.0.0"),
            ((6, 3), "3.1.1"),
            ((6, 6), "3.2.0"),
            ((6, 8), "3.3.0"),
            ((6, 12), "3.4.0"),
            ((6, 15), "3.4.1"),
            ((7, 0), "3.5.0"),
            ((7, 2), "3.5.1"),
            ((7, 5), "3.5.2"),
            // The versions the Release Guide's table carries for the lines published
            // since, as for every row above.
            ((7, 6), "3.5.4"),
            ((7, 9), "3.5.5"),
        ],
        removed_from: None,
    },
    CatalogApplication {
        name: "Tez",
        versions: &[((5, 30), "0.9.2"), ((6, 0), "0.9.2"), ((6, 9), "0.10.2")],
        removed_from: None,
    },
    CatalogApplication {
        name: "Trino",
        // Amazon EMR offers Trino under that name from 6.4.0 onward.
        versions: &[
            ((6, 4), "359"),
            ((6, 6), "367"),
            ((6, 9), "398"),
            ((6, 12), "414"),
            ((6, 15), "426"),
            ((7, 2), "435"),
        ],
        removed_from: None,
    },
    CatalogApplication {
        name: "Zeppelin",
        versions: &[
            ((5, 30), "0.8.2"),
            ((5, 36), "0.10.0"),
            ((6, 0), "0.9.0"),
            ((6, 9), "0.10.1"),
            ((7, 2), "0.11.1"),
        ],
        removed_from: None,
    },
    CatalogApplication {
        name: "ZooKeeper",
        versions: &[((5, 30), "3.4.14"), ((6, 0), "3.4.14"), ((6, 6), "3.5.10"), ((7, 0), "3.9.1")],
        removed_from: None,
    },
];

/// The version of the application `name` that the release `release_label` ships, or `None`
/// when that release bundles no such application and when the label names no release of this
/// catalog.
///
/// The one lookup behind both readers of it: `Cluster$Applications` report what the cluster's
/// release installs ([`super::installed_applications`]), and a Spark application running on
/// the cluster is told the version of the engine it is running on
/// ([`super::spark_runtime`]).
pub fn release_application_version(release_label: &str, name: &str) -> Option<&'static str> {
    let (major, minor, _) = catalog_release(release_label)?;
    APPLICATION_CATALOG
        .iter()
        .find(|application| application.name.eq_ignore_ascii_case(name))
        .and_then(|application| application.version_at(major, minor))
}

/// The Amazon Linux 2 releases offered for an Amazon EMR release built on it (Amazon
/// EMR 5.30.0 through 6.x). `OSRelease$Label` documents the format: "as shown in Amazon
/// Linux 2 Release Notes. For example, 2.0.20220218.1."
pub(super) const AMAZON_LINUX_2_OS_RELEASES: &[&str] = &["2.0.20230808.0", "2.0.20231116.0"];

/// The Amazon Linux 2023 releases offered for an Amazon EMR release built on it (Amazon
/// EMR 7.x), in the same dotted release-note form.
pub(super) const AMAZON_LINUX_2023_OS_RELEASES: &[&str] = &["2023.4.20240429.0", "2023.6.20241010.0"];

/// The first Amazon EMR major release built on Amazon Linux 2023 rather than Amazon
/// Linux 2.
pub(super) const FIRST_AMAZON_LINUX_2023_EMR_MAJOR: u32 = 7;

/// The Amazon Linux releases available for an Amazon EMR release, by the generation its
/// major release is built on.
pub(super) fn os_releases_for(major: u32) -> &'static [&'static str] {
    if major >= FIRST_AMAZON_LINUX_2023_EMR_MAJOR {
        AMAZON_LINUX_2023_OS_RELEASES
    } else {
        AMAZON_LINUX_2_OS_RELEASES
    }
}

/// The label of the release `major.minor.patch`, as Amazon EMR spells it.
pub fn release_label(major: u32, minor: u32, patch: u32) -> String {
    format!("emr-{major}.{minor}.{patch}")
}

/// The `(major, minor, patch)` release an `emr-<major>.<minor>.<patch>` label names, or
/// `None` when the label is not in that form.
pub fn release_version(label: &str) -> Option<(u32, u32, u32)> {
    let mut parts = label.strip_prefix("emr-")?.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((major, minor, patch))
}

/// The release `label` names, when the catalog offers it.
///
/// A label has to be spelled exactly as [`release_label`] renders it — so a padded or
/// differently cased variant names no release — AND name a patch release published on
/// one of the lines in [`RELEASE_LINES`]. The Spark release family carries its own prefix
/// and its own lines ([`spark_release`]), and a label of it resolves here too: it is one
/// catalog.
pub fn catalog_release(label: &str) -> Option<(u32, u32, u32)> {
    if let Some(release) = spark_release(label) {
        return Some(release);
    }
    let (major, minor, patch) = release_version(label)?;
    if label != release_label(major, minor, patch) {
        return None;
    }
    RELEASE_LINES
        .iter()
        .find(|line| line.major == major && line.minor == minor && line.patches.contains(&patch))
        .map(|_| (major, minor, patch))
}

/// Every release label the catalog offers, NEWEST release first — the order
/// `ListReleaseLabels` reports its first page in ("the first page of results are
/// determined by other filtering parameters or by the latest version",
/// `ListReleaseLabelsInput$NextToken` doc).
pub fn catalog_release_labels() -> Vec<String> {
    let mut releases: Vec<(u32, u32, u32)> = RELEASE_LINES
        .iter()
        .flat_map(|line| {
            line.patches
                .iter()
                .map(move |&patch| (line.major, line.minor, patch))
        })
        .collect();
    releases.sort_unstable_by(|left, right| right.cmp(left));
    releases
        .into_iter()
        .map(|(major, minor, patch)| release_label(major, minor, patch))
        .collect()
}

/// The applications the release line `(major, minor)` bundles, as the wire shapes, in
/// application-name order so every caller sees the same list in the same order.
pub fn release_line_applications(major: u32, minor: u32) -> Vec<SimplifiedApplication> {
    let mut applications: Vec<SimplifiedApplication> = APPLICATION_CATALOG
        .iter()
        .filter_map(|application| {
            Some(SimplifiedApplication {
                name: Some(application.name.to_string()),
                version: Some(application.version_at(major, minor)?.to_string()),
            })
        })
        .collect();
    applications.sort_by(|left, right| left.name.cmp(&right.name));
    applications
}

/// What `DescribeReleaseLabel` reports for one release of the catalog.
#[derive(Debug, Clone)]
pub struct ReleaseLabelRecord {
    pub label: String,
    pub applications: Vec<SimplifiedApplication>,
    pub available_os_releases: Vec<OSRelease>,
}

/// The catalog's entry for `label`, or `None` when it names no release this emulator
/// offers.
pub fn release_label_record(label: &str) -> Option<ReleaseLabelRecord> {
    if let Some(record) = spark_release_label_record(label) {
        return Some(record);
    }
    let (major, minor, patch) = catalog_release(label)?;
    Some(ReleaseLabelRecord {
        label: release_label(major, minor, patch),
        applications: release_line_applications(major, minor),
        available_os_releases: os_releases_for(major)
            .iter()
            .map(|&label| OSRelease {
                label: Some(label.to_string()),
            })
            .collect(),
    })
}
