// SPDX-License-Identifier: CC0-1.0

//! The tags a Bedrock Data Automation resource carries: which ARN addresses one, where
//! that resource's tag list lives in this store, and how a tagging call changes it.
//!
//! RULE. A tagging operation of this service addresses a resource by the ARN of one of
//! the resource types its `TaggableResourceArn` admits
//! (`smithy_query(query="operation",operation="TagResource")`: `blueprint`,
//! `data-automation-project`, `blueprint-optimization-invocation`,
//! `data-automation-library`, `data-automation-library-ingestion-job`), reads or writes
//! the tag list the resource that ARN names already HOLDS, and refuses an ARN this store
//! holds no such resource under with the modelled `ResourceNotFoundException` (HTTP 404),
//! worded as the RECORDING for this error class words it: `<Kind> <resourceARN> does not
//! exist` ([`resource_not_found_message`], which records where that is recorded, and why it
//! differs from the sentence a resource-addressed read or delete of the same service gives).
//! The tags are held on the resource record itself — a create's `tags` member is the same
//! list — so a tag set through `TagResource` is read back by `ListTagsForResource` and a
//! resource released by its delete takes its tags with it.
//!
//! Single-homed here because all three tagging operations resolve the same ARN against the
//! same records: three copies of that resolution are three readings waiting to drift.

use super::ComAmazonawsBedrockdataautomationAmazonbedrockkeystonebuildtimeserviceState as ServiceState;
use crate::ops::types::Tag;

/// How many tags one resource of this service holds.
///
/// The bound the model itself declares for a tag collection: `TagList` is
/// `@length(min: 0, max: 200)` (`smithy_query(query="operation",operation="TagResource")`),
/// and the list a resource holds is read back through that same shape by
/// `ListTagsForResource` — so a merge that would leave a resource carrying more tags than
/// the modelled collection can report is past the service's own limit for it.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// What `TagResource` answers when the merge it was asked for would leave the resource
/// holding more tags than [`MAX_TAGS_PER_RESOURCE`].
///
/// This service's own quota voice — the sentence form its other quota refusals take
/// (`CreateBlueprint`'s "the quota of … per account has been reached; delete a blueprint
/// before creating another") — naming the quota and what to do about it. The only variable
/// part is the quota itself, read off [`MAX_TAGS_PER_RESOURCE`] rather than spelled twice;
/// the count the request asked for is NOT quoted back, so the refusal is the same sentence
/// whatever the request carried.
#[must_use]
pub fn tag_quota_message() -> String {
    format!(
        "the quota of {MAX_TAGS_PER_RESOURCE} tags per resource has been reached; remove a \
         tag before adding another"
    )
}

/// The resource types this service's tagging operations address, and the word this service
/// names each one by in the refusal below.
///
/// The TYPES are the alternation of the modelled `TaggableResourceArn`
/// (`smithy_query(query="operation",operation="TagResource,UntagResource,ListTagsForResource")`),
/// so no type is admitted here that the pattern refuses and none is missing that it
/// admits.
///
/// The WORDS are the ones this service already names those resource kinds by in its other
/// refusals about them ([`super::BLUEPRINT_ARN_NOT_FOUND_MESSAGE`] "Blueprint",
/// [`super::PROJECT_ARN_NOT_FOUND_MESSAGE`] "Project",
/// [`super::LIBRARY_ARN_NOT_FOUND_MESSAGE`] "Library"), and for the two kinds those cover
/// no sentence for, the word the model names the resource by (`BlueprintOptimizationInvocationArn`
/// -> "Invocation", `DataAutomationLibraryIngestionJobArn` -> "Ingestion Job"): one service,
/// one word per resource kind.
const TAGGABLE_RESOURCE_KINDS: &[(&str, &str)] = &[
    ("blueprint", "Blueprint"),
    ("data-automation-project", "Project"),
    ("data-automation-library", "Library"),
    ("blueprint-optimization-invocation", "Invocation"),
    ("data-automation-library-ingestion-job", "Ingestion Job"),
];

/// What a refusal names a resource whose ARN spells a type outside
/// [`TAGGABLE_RESOURCE_KINDS`]. Unreachable through the modelled `TaggableResourceArn`,
/// whose alternation is exactly that table — the framework refuses such an ARN before a
/// handler runs — so this is the honest word for an ARN no resource kind of this service
/// claims.
const UNNAMED_RESOURCE_KIND: &str = "Resource";

/// Whether `resource_arn` addresses a resource of THIS service: an ARN of the Bedrock
/// namespace whose resource type is one this service's tagging surface serves
/// ([`TAGGABLE_RESOURCE_KINDS`], the modelled `TaggableResourceArn` alternation).
///
/// `pub` because the Bedrock control plane binds the same three tagging paths this service
/// does, so a request on one of them reaches that plane's front door first; this is what
/// that front door asks to tell a request about a Bedrock Data Automation resource from one
/// about a resource of its own, and the answer has to come from the model this service
/// serves rather than from a second list beside it.
#[must_use]
pub fn addresses_a_resource_of_this_service(resource_arn: &str) -> bool {
    let Some(parsed) = ::service_common::arn::parse_of(resource_arn, super::ARN_SERVICE) else {
        return false;
    };
    let Some((resource_type, _id)) = parsed.resource.split_once('/') else {
        return false;
    };
    TAGGABLE_RESOURCE_KINDS
        .iter()
        .any(|(declared, _)| *declared == resource_type)
}

/// The resource type an ARN spells, as the segment before the `/` of its resource field.
///
/// Read through the shared `service_common::arn` parse, so this service and its siblings
/// agree on what an ARN's resource field is.
fn resource_type(resource_arn: &str) -> Option<&str> {
    let parsed = ::service_common::arn::parse(resource_arn)?;
    let (resource_type, _id) = parsed.resource.split_once('/')?;
    Some(resource_type)
}

/// The word this service names the resource `resource_arn` addresses by
/// ([`TAGGABLE_RESOURCE_KINDS`]).
fn resource_kind(resource_arn: &str) -> &'static str {
    let Some(spelled) = resource_type(resource_arn) else {
        return UNNAMED_RESOURCE_KIND;
    };
    TAGGABLE_RESOURCE_KINDS
        .iter()
        .find(|(declared, _)| *declared == spelled)
        .map_or(UNNAMED_RESOURCE_KIND, |(_, kind)| kind)
}

/// What a TAGGING operation answers when the ARN it was handed names no resource this store
/// holds: `<Kind> <resourceARN> does not exist`, the resource kind the ARN spells around the
/// ARN the request carried.
///
/// THE RECORDED SENTENCE for this error class, which is NOT the one the same service words a
/// resource-addressed read or delete with. AWS words the two differently, and both wordings
/// are recorded:
///
/// * a resource-addressed read, delete or reference — `GetBlueprint`, `DeleteBlueprint`,
///   `GetDataAutomationProject` — answers the fixed [`super::BLUEPRINT_ARN_NOT_FOUND_MESSAGE`]
///   / [`super::PROJECT_ARN_NOT_FOUND_MESSAGE`], which take nothing from the request;
/// * the three TAGGING operations answer this one. The behavioural check
///   `bedrock-data-automation.concurrent-tag-resource-calls-merge-under-contention` asserts
///   `tag_message == f"Blueprint {arn} does not exist"` for a `TagResource` on a blueprint
///   ARN that was deleted under it, and the recorded wire refusals of all three operations
///   carry the same sentence (`issues/.../20261029T170936Z-bedrock-data-automation-tagging-operations-reject-every-bedrock-data-automation-resource.md`,
///   Causes 4-6: "the message 'Blueprint <resourceARN> does not exist'").
///
/// So the ARN here is a REQUIREMENT of this error class rather than a value this service
/// chose to echo: it is taken from the request, as the variable part of a sentence whose
/// fixed text is `" does not exist"` and the kind word before it.
#[must_use]
pub fn resource_not_found_message(resource_arn: &str) -> String {
    format!("{} {resource_arn} does not exist", resource_kind(resource_arn))
}

impl ServiceState {
    /// The tags the resource `resource_arn` names HOLDS, or `None` when this store holds
    /// no resource under that ARN.
    ///
    /// Every resource kind this store holds keeps its tags on its own record, so the
    /// lookup is by ARN across those records rather than against a second tag map that
    /// could outlive the resource it describes.
    pub fn resource_tags(&self, resource_arn: &str) -> Option<&Vec<Tag>> {
        if let Some(blueprint) = self.blueprints.get(resource_arn) {
            return Some(&blueprint.tags);
        }
        if let Some(project) = self.projects.get(resource_arn) {
            return Some(&project.tags);
        }
        if let Some(library) = self.libraries.get(resource_arn) {
            return Some(&library.tags);
        }
        if let Some(job) = self.optimization_jobs.get(resource_arn) {
            return Some(&job.tags);
        }
        self.ingestion_jobs.get(resource_arn).map(|job| &job.tags)
    }

    /// [`Self::resource_tags`] for a call that CHANGES the tags: the same lookup over the
    /// same records, so a tag written here is the one a read reports.
    pub fn resource_tags_mut(&mut self, resource_arn: &str) -> Option<&mut Vec<Tag>> {
        if let Some(blueprint) = self.blueprints.get_mut(resource_arn) {
            return Some(&mut blueprint.tags);
        }
        if let Some(project) = self.projects.get_mut(resource_arn) {
            return Some(&mut project.tags);
        }
        if let Some(library) = self.libraries.get_mut(resource_arn) {
            return Some(&mut library.tags);
        }
        if let Some(job) = self.optimization_jobs.get_mut(resource_arn) {
            return Some(&mut job.tags);
        }
        self.ingestion_jobs
            .get_mut(resource_arn)
            .map(|job| &mut job.tags)
    }
}

/// How many tags `held` would carry once `applied` is merged into it: the keys it already
/// carries keep their place and take the new value, and a key it does not carry is added.
#[must_use]
pub fn tag_count_after(held: &[Tag], applied: &[Tag]) -> usize {
    let mut keys: Vec<&str> = held.iter().map(|tag| tag.key.as_str()).collect();
    for tag in applied {
        let key = tag.key.as_str();
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys.len()
}

/// Merge `applied` into `held`: a key already present takes the submitted value, a key
/// that is not present is appended. Tagging is a MERGE — a call that names one tag leaves
/// every other tag of the resource as it was.
pub fn apply_tags(held: &mut Vec<Tag>, applied: Vec<Tag>) {
    for tag in applied {
        match held.iter_mut().find(|existing| existing.key == tag.key) {
            Some(existing) => existing.value = tag.value,
            None => held.push(tag),
        }
    }
}

/// Remove every tag of `held` whose key is one of `keys`. A key the resource does not
/// carry removes nothing: untagging states the keys a resource must no longer carry, which
/// is already true of a key it never carried.
pub fn remove_tags(held: &mut Vec<Tag>, keys: &[String]) {
    held.retain(|tag| !keys.contains(&tag.key));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(key: &str, value: &str) -> Tag {
        Tag {
            key: key.to_string(),
            value: value.to_string(),
        }
    }

    /// RULE: the refusal is `<Kind> <resourceARN> does not exist` — the kind word this
    /// service names the resource kind the ARN spells by, around the ARN the request
    /// carried, which is the recorded sentence for this error class
    /// ([`resource_not_found_message`] cites where).
    ///
    /// The rows span every type of the modelled alternation plus a value outside it, and
    /// each expectation is the WHOLE sentence spelled out here rather than read back off the
    /// formatter under test.
    #[test]
    fn the_not_found_sentence_names_the_kind_and_the_arn() {
        let rows: [(&str, &str, &str); 6] = [
            (
                "a blueprint",
                "arn:aws:bedrock:us-west-2:000000000000:blueprint/83f78afe19be",
                "Blueprint arn:aws:bedrock:us-west-2:000000000000:blueprint/83f78afe19be \
                 does not exist",
            ),
            (
                "a project",
                "arn:aws:bedrock:us-west-2:000000000000:data-automation-project/37ccb70b0000",
                "Project arn:aws:bedrock:us-west-2:000000000000:data-automation-project/\
                 37ccb70b0000 does not exist",
            ),
            (
                "a library",
                "arn:aws:bedrock:eu-west-1:000000000000:data-automation-library/abcdefabcdef",
                "Library arn:aws:bedrock:eu-west-1:000000000000:data-automation-library/\
                 abcdefabcdef does not exist",
            ),
            (
                "an optimization invocation",
                "arn:aws:bedrock:us-west-2:000000000000:blueprint-optimization-invocation/abcdefabcdef",
                "Invocation arn:aws:bedrock:us-west-2:000000000000:\
                 blueprint-optimization-invocation/abcdefabcdef does not exist",
            ),
            (
                "a library ingestion job",
                "arn:aws:bedrock:us-west-2:000000000000:data-automation-library-ingestion-job/abcdefabcdef",
                "Ingestion Job arn:aws:bedrock:us-west-2:000000000000:\
                 data-automation-library-ingestion-job/abcdefabcdef does not exist",
            ),
            (
                "an ARN of no type this surface addresses",
                "arn:aws:bedrock:us-west-2:000000000000:guardrail/abcdefabcdef",
                "Resource arn:aws:bedrock:us-west-2:000000000000:guardrail/abcdefabcdef \
                 does not exist",
            ),
        ];
        for (case, arn, expected) in rows {
            let actual = resource_not_found_message(arn);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: the ARNs this service's tagging surface serves are exactly the ARNs of the
    /// Bedrock namespace whose resource type the modelled alternation admits — which is
    /// what tells a tagging request about one of this service's resources from one about a
    /// resource of the Bedrock plane that binds the same paths.
    ///
    /// The rows span every admitted type, a Bedrock ARN of a type outside the alternation,
    /// another vendor's ARN, and values that are no ARN at all.
    #[test]
    fn only_this_services_taggable_arns_are_claimed() {
        let rows: [(&str, &str, bool); 8] = [
            (
                "a blueprint",
                "arn:aws:bedrock:us-west-2:000000000000:blueprint/83f78afe19be",
                true,
            ),
            (
                "a project",
                "arn:aws:bedrock:us-west-2:000000000000:data-automation-project/37ccb70b0000",
                true,
            ),
            (
                "a library",
                "arn:aws:bedrock:us-west-2:000000000000:data-automation-library/abcdefabcdef",
                true,
            ),
            (
                "an optimization invocation",
                "arn:aws:bedrock:us-west-2:000000000000:blueprint-optimization-invocation/abcdef",
                true,
            ),
            (
                "a guardrail, a Bedrock resource of another plane",
                "arn:aws:bedrock:us-west-2:000000000000:guardrail/abcdefabcdef",
                false,
            ),
            (
                "another vendor's ARN",
                "arn:aws:sqs:us-west-2:000000000000:blueprint/abcdefabcdef",
                false,
            ),
            ("a bare resource name", "my-custom-model", false),
            ("a Bedrock ARN with no resource type", "arn:aws:bedrock:::", false),
        ];
        for (case, arn, expected) in rows {
            let actual = addresses_a_resource_of_this_service(arn);
            assert_eq!(
                actual, expected,
                "case {case:?} ({arn}): expected {expected}, got {actual}"
            );
        }
    }

    /// RULE: tagging MERGES — a key already carried takes the new value in place, a new
    /// key is appended, and nothing else moves; untagging drops exactly the named keys.
    #[test]
    fn tags_merge_and_untag_drops_only_the_named_keys() {
        let mut held = vec![tag("team", "data-eng"), tag("env", "test")];
        apply_tags(&mut held, vec![tag("env", "prod"), tag("owner", "ada")]);
        let after: Vec<(String, String)> = held
            .iter()
            .map(|tag| (tag.key.clone(), tag.value.clone()))
            .collect();
        assert_eq!(
            after,
            vec![
                ("team".to_string(), "data-eng".to_string()),
                ("env".to_string(), "prod".to_string()),
                ("owner".to_string(), "ada".to_string()),
            ],
            "case \"merge\": expected the held key updated in place and the new one \
             appended, got {after:?}"
        );

        remove_tags(&mut held, &["env".to_string(), "absent".to_string()]);
        let keys: Vec<&str> = held.iter().map(|tag| tag.key.as_str()).collect();
        assert_eq!(
            keys,
            vec!["team", "owner"],
            "case \"untag\": expected only the named key dropped, got {keys:?}"
        );
    }

    /// RULE: the count a merge would leave counts only the keys the resource does not
    /// already carry, and counts a key repeated in one request once.
    #[test]
    fn the_merged_count_counts_each_key_once() {
        let held = vec![tag("team", "data-eng")];
        // (case, the tags applied, the count the merge leaves)
        let rows: [(&str, Vec<Tag>, usize); 4] = [
            ("nothing applied", Vec::new(), 1),
            ("a key already held", vec![tag("team", "platform")], 1),
            ("a new key", vec![tag("env", "test")], 2),
            (
                "the same new key twice",
                vec![tag("env", "test"), tag("env", "prod")],
                2,
            ),
        ];
        for (case, applied, expected) in rows {
            let actual = tag_count_after(&held, &applied);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected}, got {actual}"
            );
        }
    }
}
