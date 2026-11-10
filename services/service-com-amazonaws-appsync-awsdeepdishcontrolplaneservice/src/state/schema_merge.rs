// SPDX-License-Identifier: CC0-1.0

//! The merge of a source API's schema into the Merged API it is associated with.
//!
//! An association's merge is "the merge of the source API's schema into the Merged API"
//! (`StartSchemaMerge`, `SourceApiAssociation$lastSuccessfulMergeDate`), so what a merge
//! that has been PERFORMED leaves behind is the source API's types on the Merged API:
//! that is what makes the Merged API's own schema readable and its types addressable,
//! and a merge that moved only a status would report success while the Merged API held
//! nothing.
//!
//! The merge DEPENDS on the source API's schema: a creation the clock still owes on the
//! source API has not installed its types yet, so a merge that would come due before it
//! is held until that creation is due — a composite reaches its terminal state only once
//! what it depends on is itself ready. That is why the dues are applied together, in that
//! order ([`super::ComAmazonawsAppsyncAwsdeepdishcontrolplaneserviceState::settle_schema_creations`]).

use std::collections::BTreeMap;
use std::time::SystemTime;

use super::{
    arn_region, type_resource, ComAmazonawsAppsyncAwsdeepdishcontrolplaneserviceState,
    TypeRecord, ARN_SERVICE, DEFAULT_REGION,
};

impl ComAmazonawsAppsyncAwsdeepdishcontrolplaneserviceState {
    /// Perform every merge the CLOCK already made due at `now`, and hold the ones whose
    /// source API still owes a schema creation until that creation is due.
    ///
    /// Owed by the clock alone: the due instant was recorded by the call that scheduled
    /// the merge, so the same transition is applied whether or not anybody read either
    /// API in between.
    pub(super) fn settle_due_merges(&mut self, now: SystemTime) {
        let mut due: Vec<(String, String)> = Vec::new();
        for association in self.source_api_associations.values_mut() {
            if association.merge_due_at.is_none_or(|at| now < at) {
                continue;
            }
            // The types this merge carries are the ones the source API's own creation
            // installs, so a merge that would run first is pushed to that creation's due
            // instant rather than merging a schema that is not there yet.
            let creation_due = self
                .graphql_apis
                .get(&association.source_api_id)
                .and_then(|api| api.schema.as_ref())
                .and_then(|schema| schema.due_at);
            match creation_due {
                Some(creation_due) => association.merge_due_at = Some(creation_due),
                None => {
                    association.settle(now);
                    due.push((
                        association.source_api_id.clone(),
                        association.merged_api_id.clone(),
                    ));
                }
            }
        }
        for (source_api_id, merged_api_id) in due {
            self.merge_types(&source_api_id, &merged_api_id);
        }
    }

    /// Install the types the source API `source_api_id` holds onto the Merged API
    /// `merged_api_id`.
    ///
    /// A merged type is addressed by the same name its source declares it under and
    /// carries the same definition, so the Merged API reports the source's own type —
    /// under the MERGED API's own path, because that is where the type now lives and what
    /// a type operation on the Merged API addresses it by. A name the Merged API already
    /// holds keeps its place in the listing and its ARN, and takes the definition this
    /// merge carries: the merge that ran last is the one whose schema the Merged API
    /// holds for that name.
    fn merge_types(&mut self, source_api_id: &str, merged_api_id: &str) {
        let Some(source_types) = self
            .graphql_apis
            .get(source_api_id)
            .map(|api| api.types.clone())
        else {
            return;
        };
        let Some((region, held)) = self.graphql_apis.get(merged_api_id).map(|merged| {
            let held: BTreeMap<String, (u64, String)> = merged
                .types
                .iter()
                .map(|(name, record)| (name.clone(), (record.seq, record.arn.clone())))
                .collect();
            // The Merged API is reported in the Region it was created in, so its types
            // are named under that Region rather than the source API's.
            (
                arn_region(&merged.arn).unwrap_or(DEFAULT_REGION).to_string(),
                held,
            )
        }) else {
            return;
        };

        let mut merged_types: BTreeMap<String, TypeRecord> = BTreeMap::new();
        for (name, mut record) in source_types {
            match held.get(&name) {
                Some((seq, arn)) => {
                    record.seq = *seq;
                    record.arn = arn.clone();
                }
                None => {
                    record.seq = self.next_seq();
                    record.arn = ::engine::emulator_arn(
                        ARN_SERVICE,
                        &region,
                        &type_resource(merged_api_id, &name),
                    );
                }
            }
            merged_types.insert(name, record);
        }

        if let Some(merged) = self.graphql_apis.get_mut(merged_api_id) {
            // Extended rather than replaced: a Merged API combines the schemas of EVERY
            // source API associated with it, so a type another source's merge installed
            // stays.
            merged.types.extend(merged_types);
        }
    }
}
