// SPDX-License-Identifier: CC0-1.0

//! The rule a user's — or an access grant's — LOGICAL home directory has to satisfy
//! before it can be stored: its `Entry` paths may not OVERLAP.
//!
//! `HomeDirectoryMappings` is "logical directory mappings that specify what Amazon S3
//! or Amazon EFS paths and keys should be visible to your user and how you want to make
//! them visible … where `Entry` shows how the path is made visible and `Target` is the
//! actual Amazon S3 or Amazon EFS path"
//! (`smithy_query(query="doc",shape="CreateUserRequest",member="HomeDirectoryMappings")`),
//! so the entries together ARE the directory tree the session sees: each `Entry` is the
//! absolute path one `Target` is mounted at (`MapEntry`, pattern `^/.*$`). Two entries
//! one of which is the other, or an ancestor directory of the other, mount two targets
//! at one path — the chroot example the member documents, `Entry` `/`, mounts the whole
//! visible tree — so which `Target` a path resolves to would not be defined. The service
//! refuses such a configuration where it is WRITTEN, which is why the check lives beside
//! the other cross-member requirements on these members
//! ([`super::home_directory_violation`]) and runs for every operation that writes them,
//! before any state is changed.
//!
//! Overlap is decided on PATH SEGMENTS, not on text: `/inbound` and `/inbox` share a
//! text prefix and name two different directories, while `/inbound` and
//! `/inbound/2024` name one directory and something inside it. Empty segments carry no
//! name, so a trailing or a doubled separator does not change which path an entry names.
//!
//! WHAT BOUNDS IT: `HomeDirectoryMappings` is modelled `@length(1..50000)` and each
//! `Entry` `@length(0..1024)` (both enforced by the framework's boundary validator before
//! a handler runs), so the number of entries AND the depth of each path are the CALLER's
//! to choose — and comparing paths with each other, or probing a sorted set of whole
//! paths once per segment, costs more than the request's own size. The mappings are
//! therefore mounted into a TREE of their segments ([`Mounted`]): one entry is mounted by
//! walking its OWN segments once, descending one child link per segment, which is what
//! makes both ways it can overlap a mapping already mounted answerable in that one walk —
//! a mapping AT or ABOVE this path is a mount recorded on a node the walk passes through,
//! and a mapping BELOW it is a mount recorded under the node the walk ends at. So the
//! whole check costs one descent per segment of the request's own paths, with no
//! comparison against an unrelated path and nothing allocated per entry beyond the tree
//! links themselves.

use std::collections::BTreeMap;

use crate::ops::types::HomeDirectoryMapEntry;

/// The refusal of a mapping set whose entries overlap, worded as the service states it:
/// the whole sentence is fixed apart from the `Entry` it names, which is read off the
/// REQUEST — the service names the path that collides, because which of the entries the
/// caller sent is not derivable from the condition alone.
fn overlapping_mapping(entry: &str) -> String {
    format!("[Contains overlapping mapping with: {entry}]")
}

/// The violation `mappings` carries when two of its entries overlap, or `None` when
/// every entry names a path of its own.
///
/// The entry reported is the FIRST one, in the order the request lists the mappings,
/// that names a path another mapping listed before it already covers — the same path,
/// a directory above it, or something inside it.
pub fn overlapping_entry(mappings: &[HomeDirectoryMapEntry]) -> Option<String> {
    let mut mounted = Mounted::default();
    for mapping in mappings {
        if !mounted.mount(path_segments(&mapping.entry)) {
            return Some(overlapping_mapping(&mapping.entry));
        }
    }
    None
}

/// The paths the mappings read so far are mounted at, as a tree of their segments: one
/// node per named segment, carrying whether a mapping is mounted AT that path and whether
/// one is mounted anywhere BELOW it.
///
/// Those two marks are what let one walk decide the whole rule, since a path overlaps
/// another exactly when one of them is a prefix of the other: an ancestor is a mount the
/// walk passes, and a descendant is a mount recorded below where it ends.
#[derive(Debug, Default)]
struct Mounted<'a> {
    /// The paths one segment further down, by that segment.
    children: BTreeMap<&'a str, Mounted<'a>>,
    /// Whether a mapping is mounted at exactly this path.
    at: bool,
    /// Whether a mapping is mounted at any path strictly below this one.
    below: bool,
}

impl<'a> Mounted<'a> {
    /// Mount the path `segments` names, answering whether it was mounted: `false` is the
    /// path overlapping one already mounted, which is the condition the caller refuses.
    ///
    /// A walk that fails leaves the tree with `below` marked along the path it got
    /// through, which no later walk reads: the first overlap ends the whole check.
    fn mount(&mut self, segments: impl Iterator<Item = &'a str>) -> bool {
        let mut node = self;
        for segment in segments {
            // A mapping mounted here covers everything below it, so this path is inside
            // one the set already mounts.
            if node.at {
                return false;
            }
            node.below = true;
            node = node.children.entry(segment).or_default();
        }
        // The path's own node: a mapping mounted at it is this very path, and one mounted
        // below it is a path this one would cover.
        if node.at || node.below {
            return false;
        }
        node.at = true;
        true
    }
}

/// The named segments of `path`: an empty segment carries no name, so `/`, `//` and
/// `/inbound/` are read as the paths they name.
fn path_segments(path: &str) -> impl Iterator<Item = &str> {
    path.split('/').filter(|segment| !segment.is_empty())
}
