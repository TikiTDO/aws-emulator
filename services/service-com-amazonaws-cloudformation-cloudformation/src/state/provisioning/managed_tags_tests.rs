// SPDX-License-Identifier: CC0-1.0

//! Pure transport/ownership fixtures; real owner calls belong to the Trail lifecycle tests.

use super::{StackProvenance, StoredResource, carry_forward, managed_tags_for, successful_managed_tags};
use serde_json::json;
use std::collections::BTreeMap;

fn trail(properties: serde_json::Value) -> StoredResource {
    StoredResource {
        logical_id: "Trail".into(),
        resource_type: "AWS::CloudTrail::Trail".into(),
        physical_resource_id: "generic-audit-trail".into(),
        attributes: BTreeMap::new(),
        metadata: None,
        properties,
        materialised: true,
        managed_tags: None,
        deletion_policy: None,
    }
}

fn tags(items: &[(&str, &str)]) -> Vec<(String, String)> {
    items
        .iter()
        .map(|(key, value)| ((*key).into(), (*value).into()))
        .collect()
}

#[test]
fn witness_uses_effective_resolved_tags_and_preserves_resource_precedence() {
    let resolved = json!({"Tags": [
        {"Key": "shared", "Value": "resource-resolved"},
        {"Key": "resource-only", "Value": "resolved"}
    ]});
    let propagated = tags(&[
        ("shared", "stack"),
        ("stack-only", "propagated"),
        ("aws:cloudformation:stack-id", "reserved"),
    ]);
    assert_eq!(
        managed_tags_for("AWS::CloudTrail::Trail", &resolved, &propagated),
        Some(tags(&[
            ("shared", "resource-resolved"),
            ("resource-only", "resolved"),
            ("stack-only", "propagated")
        ]))
    );
}

#[test]
fn unknown_ownership_is_not_a_known_empty_set_or_authority_for_other_owners() {
    let imported = trail(json!({"Tags": [{"Key": "external", "Value": "live"}]}));
    assert_eq!(imported.managed_tags, None);
    assert_eq!(
        managed_tags_for("AWS::CloudTrail::Trail", &json!({}), &[]),
        Some(vec![])
    );
    assert_eq!(
        managed_tags_for("AWS::S3::Bucket", &imported.properties, &[]),
        None
    );
}

#[test]
fn drift_properties_do_not_promote_live_tags_into_the_carried_witness() {
    let mut existing = trail(json!({"Tags": [{"Key": "declared", "Value": "old"}]}));
    existing.managed_tags = Some(tags(&[("declared", "old"), ("old-stack", "old")]));
    // ExecuteChangeSet replaces the comparison properties with actual drift, not ownership.
    existing.properties = json!({"Tags": [
        {"Key": "declared", "Value": "drift"},
        {"Key": "external", "Value": "untouched"}
    ]});
    let mut declared = trail(json!({}));
    carry_forward(&existing, &mut declared);
    assert_eq!(declared.managed_tags, existing.managed_tags);
    assert!(
        !declared
            .managed_tags
            .as_ref()
            .unwrap()
            .iter()
            .any(|(key, _)| key == "external")
    );
    assert!(declared.attributes.is_empty());
    assert_eq!(declared.metadata, None);
}

#[test]
fn rollback_snapshots_keep_applied_ownership_separate_from_restore_intent() {
    let mut old = trail(json!({"Tags": [{"Key": "resource-old", "Value": "old"}]}));
    old.managed_tags = Some(tags(&[("resource-old", "old"), ("stack-old", "old")]));
    let mut applied = trail(json!({"Tags": [{"Key": "resource-new", "Value": "new"}]}));
    applied.managed_tags = Some(tags(&[("resource-new", "new"), ("stack-new", "new")]));
    let owed = crate::state::OwedRollback {
        previous: vec![old],
        applied: vec![applied],
    };
    let mut restoring = owed.previous[0].clone();
    let restore_properties = restoring.properties.clone();
    carry_forward(&owed.applied[0], &mut restoring);
    assert_eq!(restoring.managed_tags, owed.applied[0].managed_tags);
    assert_eq!(
        managed_tags_for(
            &restoring.resource_type,
            &restore_properties,
            &tags(&[("stack-old", "old")])
        ),
        owed.previous[0].managed_tags
    );
}

#[test]
fn successful_drift_rollback_does_not_claim_external_tags_for_the_next_update() {
    let mut before = trail(json!({"Tags": [
        {"Key": "declared", "Value": "drift"},
        {"Key": "external", "Value": "untouched"}
    ]}));
    before.managed_tags = Some(tags(&[("declared", "old"), ("old-stack", "old")]));
    let mut applied = trail(json!({"Tags": [{"Key": "replacement", "Value": "new"}]}));
    applied.managed_tags = Some(tags(&[("replacement", "new")]));
    // The reverse walk captures these separately BEFORE carrying the applied state.
    let restore_settings = before.properties.clone();
    let restore_witness = before.managed_tags.clone();
    carry_forward(&applied, &mut before);
    assert_eq!(before.managed_tags, applied.managed_tags);
    // This is the same success-selection function used by both create and retag.
    before.managed_tags = successful_managed_tags(
        &before.resource_type,
        &restore_settings,
        &tags(&[("old-stack", "old")]),
        true,
        &restore_witness,
    );
    assert_eq!(before.managed_tags, restore_witness);
    // The next ordinary update uses the restored witness as its previous-managed domain,
    // not every key present in the restored actual settings.
    let desired = successful_managed_tags(
        &before.resource_type,
        &json!({}),
        &[],
        StackProvenance::default().restoring_tag_ownership,
        &before.managed_tags,
    ).unwrap();
    let previous = before.managed_tags.as_ref().unwrap();
    assert!(previous.iter().chain(&desired).all(|(key, _)| key != "external"));
    assert!(desired.is_empty());
}

#[test]
fn restoring_unknown_ownership_does_not_invent_an_empty_or_actual_tag_witness() {
    let actual = json!({"Tags": [{"Key": "external", "Value": "untouched"}]});
    assert_eq!(
        successful_managed_tags("AWS::CloudTrail::Trail", &actual, &[], true, &None),
        None,
    );
    assert_eq!(
        successful_managed_tags("AWS::CloudTrail::Trail", &actual, &[], true, &Some(vec![])),
        Some(vec![]),
    );
    // The same key explicitly authored by a forward deployment IS owned afterwards.
    assert_eq!(
        successful_managed_tags("AWS::CloudTrail::Trail", &actual, &[], false, &None),
        Some(tags(&[("external", "untouched")])),
    );
}
