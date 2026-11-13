// SPDX-License-Identifier: CC0-1.0

//! The merges a repository has already made: which commit each merge of one
//! commit into another wrote.
//!
//! Split out of [`super`] so the one reading of "the merge commit of these two
//! commits" lives in a single place, reached by every operation that MAKES a
//! merge (the branch, pull-request and unreferenced merge handlers, which record
//! what they wrote) and by the operation that REPORTS one (`GetMergeCommit`,
//! which reads it). Everything here is reached through [`super::Repository`], so
//! the module adds no new names.
//!
//! The record is needed because a merge commit cannot be recognised from the
//! commit object alone: a three-way merge records both merged tips as parents,
//! but a squash merge records only the destination tip, which is exactly the
//! shape of an ordinary commit made on top of it.

use super::*;

impl Repository {
    /// Record that merging `source_commit_id` into `destination_commit_id` wrote
    /// `merged_commit_id`.
    ///
    /// The newest merge of a pair of commits replaces the one before it, so the
    /// commit reported for that pair is the merge this repository last made of
    /// it.
    pub fn record_merge_commit(
        &mut self,
        source_commit_id: &str,
        destination_commit_id: &str,
        merged_commit_id: &str,
    ) {
        self.merge_commits.insert(
            (
                source_commit_id.to_string(),
                destination_commit_id.to_string(),
            ),
            merged_commit_id.to_string(),
        );
    }

    /// The commit the merge of `source_commit_id` into `destination_commit_id`
    /// wrote, or `None` where this repository has made no such merge.
    ///
    /// An id whose commit the repository no longer holds is no answer: history
    /// is bounded ([`Repository::prune_history`]), so a merge commit that was
    /// released is reported as no merge commit rather than as an id that
    /// resolves to nothing.
    pub fn merge_commit_of(
        &self,
        source_commit_id: &str,
        destination_commit_id: &str,
    ) -> Option<&str> {
        self.merge_commits
            .get(&(
                source_commit_id.to_string(),
                destination_commit_id.to_string(),
            ))
            .filter(|merged| self.commits.contains_key(merged.as_str()))
            .map(String::as_str)
    }

    /// Release every record this repository can no longer answer with: one whose
    /// merge commit it no longer holds, and one whose merged PAIR it no longer
    /// holds either — a specifier can only resolve to a commit the projection
    /// still carries, so such a record is unreachable. Releasing them is what
    /// keeps the records bounded by the commit projection itself.
    pub fn release_stale_merge_commits(&mut self) {
        let Self {
            commits,
            merge_commits,
            ..
        } = self;
        merge_commits.retain(|(source, destination), merged| {
            commits.contains_key(merged)
                && commits.contains_key(source)
                && commits.contains_key(destination)
        });
    }
}
