// SPDX-License-Identifier: CC0-1.0

//! The related-item resources an OpsItem carries, and the rules an association of one is
//! held to.
//!
//! A related item is an association between an OpsItem and the ARN of another Amazon Web
//! Services resource: `AssociateOpsItemRelatedItem` creates one and
//! `ListOpsItemRelatedItems` reports it, both through the one store below
//! ([`RelatedItemRecord`]), so whatever an association persisted is what a later listing
//! reports.
//!
//! An association is identified by a server-minted `AssociationId` and belongs to the
//! OpsItem it was created against, which is reached through the endpoint of the Region
//! that OpsItem lives in — so the store is keyed by
//! `(region, ops item id, association id)`, which also gives a listing a stable, total
//! order to page through.
//!
//! Two rules an association is held to are documented rather than modelled, so they are
//! stated here once:
//!
//! * the association type is one OpsCenter supports ([`ASSOCIATION_TYPES`]) and the
//!   resource type one of the types it supports ([`RESOURCE_TYPES`]);
//! * one resource ARN is associated with an OpsItem at most once, which is the one thing
//!   `OpsItemRelatedItemAlreadyExistsException` ("The Amazon Resource Name (ARN) is
//!   already associated with the OpsItem") reports.

use std::time::SystemTime;

use super::ops_items::ParameterRejection;
use super::ComAmazonawsSsmAmazonssmState;
use crate::ops::types::{OpsItemIdentity, OpsItemRelatedItemSummary};

/// The association types "OpsCenter supports" between an OpsItem and a resource
/// (`AssociateOpsItemRelatedItemRequest$AssociationType`'s doc: "OpsCenter supports
/// IsParentOf and RelatesTo association types").
pub const ASSOCIATION_TYPES: &[&str] = &["IsParentOf", "RelatesTo"];

/// The resource types OpsCenter supports as a related item
/// (`AssociateOpsItemRelatedItemRequest$ResourceType`'s doc: "AWS::SSMIncidents::IncidentRecord:
/// an Incident Manager incident. AWS::SSM::Document: a Systems Manager (SSM) document").
pub const RESOURCE_TYPES: &[&str] = &["AWS::SSMIncidents::IncidentRecord", "AWS::SSM::Document"];

/// The wire code of the refusal of a value outside those sets, which is the modelled
/// exception's own shape name (`com.amazonaws.ssm#OpsItemInvalidParameterException`), and
/// so what the frame below names as its error code.
pub const INVALID_PARAMETER_CODE: &str = "OpsItemInvalidParameterException";

/// The HTTP status that refusal carries, which is the status the model declares for it
/// (`OpsItemInvalidParameterException`, httpError 400), and so what the frame below names
/// as its status.
pub const INVALID_PARAMETER_STATUS: u16 = 400;

/// The member OpsCenter reports as refused when the related item's resource type is not
/// one it supports: the service names this member `AssociatedResourceType`, which is not
/// the wire member name a request spells it under.
pub const ASSOCIATED_RESOURCE_TYPE_MEMBER: &str = "AssociatedResourceType";

/// How OpsCenter words the refusal of a value outside the set it supports for `member`:
/// the one fixed sentence of that class, naming the member as a request spells it in
/// lowerCamel and carrying nothing of the submitted value, followed by the frame this
/// service appends to a refusal it worded itself ([`super::with_sdk_error_envelope`], whose
/// only variable part is the serving response's own request id).
pub fn not_valid_message(member: &str) -> String {
    super::with_sdk_error_envelope(
        &format!("The {member} entered is not valid."),
        INVALID_PARAMETER_STATUS,
        INVALID_PARAMETER_CODE,
    )
}

/// The member an already-associated resource collided on, which is what Systems Manager
/// names as at fault in `OpsItemRelatedItemAlreadyExistsException`'s message: the input
/// member whose value is already a related item of the OpsItem.
pub const COLLIDING_MEMBER: &str = "ResourceUri";

/// The wire code of that refusal, which is the exception's own shape name
/// (`com.amazonaws.ssm#OpsItemRelatedItemAlreadyExistsException`), and so what the frame
/// below names as its error code.
pub const ALREADY_EXISTS_CODE: &str = "OpsItemRelatedItemAlreadyExistsException";

/// The HTTP status that refusal carries, which is the status the model declares for it
/// (`AssociateOpsItemRelatedItem`'s `OpsItemRelatedItemAlreadyExistsException`, httpError
/// 400), and so what the frame below names as its status.
pub const ALREADY_EXISTS_STATUS: u16 = 400;

/// The service the exception frame names, which is the service shape the operation targets
/// (`com.amazonaws.ssm#AmazonSSM`, the `AmazonSSM.<Operation>` of every request's
/// `X-Amz-Target`).
pub const EXCEPTION_SOURCE_SERVICE: &str = "AmazonSSM";

/// How the frame reads the request id of a response that carries none, which is how the
/// recorded refusal of this operation reads it: this service's protocol puts no request id
/// on a response, so the frame has no id of this request to name.
pub const ABSENT_REQUEST_ID: &str = "None";

/// How Systems Manager words the refusal of an association of a resource that is already a
/// related item of the OpsItem: the member at fault, then the frame the service appends to
/// a refusal of this operation — the service the refusal came from, the status and error
/// code it carries, and the request id of the response it travels on.
///
/// `request_id` is that response's own request id, so the frame restates the refusal a
/// client already has rather than inventing anything; a response carrying none is read as
/// [`ABSENT_REQUEST_ID`]. The ARN that collided and the OpsItem it collided on travel in
/// the exception's own members rather than in this sentence.
pub fn already_exists_message(request_id: Option<&str>) -> String {
    format!(
        "{COLLIDING_MEMBER} (Service: {EXCEPTION_SOURCE_SERVICE}; Status Code: \
         {ALREADY_EXISTS_STATUS}; Error Code: {ALREADY_EXISTS_CODE}; Request ID: {}; Proxy: \
         null)",
        request_id.unwrap_or(ABSENT_REQUEST_ID)
    )
}

/// The bounds the modelled `OpsItemRelatedItemsMaxResults` carries (range 1..50).
pub const MIN_RELATED_ITEM_PAGE_SIZE: i32 = 1;

/// See [`MIN_RELATED_ITEM_PAGE_SIZE`]. It is also the page size a request that names no
/// `MaxResults` gets, the way every other read of this service serves one.
pub const MAX_RELATED_ITEM_PAGE_SIZE: i32 = 50;

/// The most related-item associations this emulator holds across every Region.
///
/// The Region and the OpsItem an association is stored under come from the request, so
/// without a total bound the store would grow without end. The published OpsCenter quota
/// is NOT restated here: this bound is the emulator's own, and the refusal says so. It is
/// enforced at the store's ONE write path
/// ([`ComAmazonawsSsmAmazonssmState::insert_related_item`]).
pub const MAX_RELATED_ITEMS_TOTAL: usize = 10_000;

/// The page size a related-item listing serves: a request naming no `MaxResults` is served
/// up to the modelled maximum, and one naming a value outside the modelled range is
/// refused (the boundary walk already refuses it, so this is a backstop).
pub fn related_item_page_size(max_results: Option<i32>) -> Result<usize, String> {
    super::page_size(
        max_results,
        MIN_RELATED_ITEM_PAGE_SIZE,
        MAX_RELATED_ITEM_PAGE_SIZE,
    )
}

/// Why `association_type` is not an association type OpsCenter supports, or `None` when it
/// is one of [`ASSOCIATION_TYPES`].
///
/// Worded the way OpsCenter words this class ([`not_valid_message`]) and naming the member
/// the service reports, which for the association type is the member a request spells.
pub fn association_type_rejection(association_type: &str) -> Option<ParameterRejection> {
    (!ASSOCIATION_TYPES.contains(&association_type)).then(|| {
        ParameterRejection::new("AssociationType", not_valid_message("associationType"))
    })
}

/// Why `resource_type` is not a resource type OpsCenter supports as a related item, or
/// `None` when it is one of [`RESOURCE_TYPES`].
///
/// Worded as [`association_type_rejection`] is, and naming the member OpsCenter reports
/// for a related item's resource type ([`ASSOCIATED_RESOURCE_TYPE_MEMBER`]).
pub fn resource_type_rejection(resource_type: &str) -> Option<ParameterRejection> {
    (!RESOURCE_TYPES.contains(&resource_type)).then(|| {
        ParameterRejection::new(
            ASSOCIATED_RESOURCE_TYPE_MEMBER,
            not_valid_message("resourceType"),
        )
    })
}

/// Why `filter` does not name a property of a related item, or `None` when it does.
///
/// A filter has to name one of the keys the model's `OpsItemRelatedItemsFilterKey` declares
/// and the one operator it declares ("The only supported operator is EQUAL"), and has to
/// carry a value to compare against: anything else narrows a listing by nothing a related
/// item holds, so it is refused rather than served as if it had been applied.
pub fn filter_rejection(
    key: &str,
    operator: &str,
    values: &[String],
) -> Option<ParameterRejection> {
    if !crate::ops::types::OPS_ITEM_RELATED_ITEMS_FILTER_KEY_VALUES.contains(&key) {
        return Some(ParameterRejection::new(
            "Filters",
            format!(
                "The filter key {key} isn't supported. Supported values include {}",
                crate::ops::types::OPS_ITEM_RELATED_ITEMS_FILTER_KEY_VALUES.join(", ")
            ),
        ));
    }
    if !crate::ops::types::OPS_ITEM_RELATED_ITEMS_FILTER_OPERATOR_VALUES.contains(&operator) {
        return Some(ParameterRejection::new(
            "Filters",
            format!(
                "The filter operator {operator} isn't supported. The only supported operator is \
                 {}",
                crate::ops::types::OPS_ITEM_RELATED_ITEMS_FILTER_OPERATOR_VALUES.join(", ")
            ),
        ));
    }
    values.is_empty().then(|| {
        ParameterRejection::new(
            "Filters",
            format!("The filter {key} carries no values to compare against"),
        )
    })
}

/// One related-item association as the store holds it.
///
/// Every member `ListOpsItemRelatedItems` reports is here, so a listing projects this
/// record ([`RelatedItemRecord::to_summary`]) rather than recomputing anything.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct RelatedItemRecord {
    /// The server-minted association id, unique within the store.
    pub association_id: String,
    /// The OpsItem this resource is a related item of.
    pub ops_item_id: String,
    /// The Region whose endpoint the association was created through, which is the Region
    /// of the OpsItem it belongs to.
    pub region: String,
    /// "The type of resource that you want to associate with an OpsItem"
    /// ([`RESOURCE_TYPES`]).
    pub resource_type: String,
    /// "The type of association that you want to create between an OpsItem and a
    /// resource" ([`ASSOCIATION_TYPES`]).
    pub association_type: String,
    /// "The Amazon Resource Name (ARN) of the Amazon Web Services resource that you want
    /// to associate with the OpsItem".
    pub resource_uri: String,
    /// The ARN of the IAM entity that created the association.
    pub created_by: String,
    /// "The time the related-item association was created".
    pub created_time: SystemTime,
    /// The ARN of the IAM entity that last updated the association.
    pub last_modified_by: String,
    /// "The time the related-item association was last updated".
    pub last_modified_time: SystemTime,
}

impl RelatedItemRecord {
    /// The modelled `OpsItemRelatedItemSummary` for this record — every member projected
    /// from what was persisted.
    pub fn to_summary(&self) -> OpsItemRelatedItemSummary {
        OpsItemRelatedItemSummary {
            ops_item_id: Some(self.ops_item_id.clone()),
            association_id: Some(self.association_id.clone()),
            resource_type: Some(self.resource_type.clone()),
            association_type: Some(self.association_type.clone()),
            resource_uri: Some(self.resource_uri.clone()),
            created_by: Some(OpsItemIdentity {
                arn: Some(self.created_by.clone()),
            }),
            created_time: Some(::aws_smithy_types::DateTime::from(self.created_time)),
            last_modified_by: Some(OpsItemIdentity {
                arn: Some(self.last_modified_by.clone()),
            }),
            last_modified_time: Some(::aws_smithy_types::DateTime::from(self.last_modified_time)),
        }
    }

    /// What this record holds for the filter key `key`, which is the property the modelled
    /// `OpsItemRelatedItemsFilterKey` names.
    fn filter_value(&self, key: &str) -> Option<&str> {
        match key {
            "AssociationId" => Some(self.association_id.as_str()),
            "ResourceType" => Some(self.resource_type.as_str()),
            "ResourceUri" => Some(self.resource_uri.as_str()),
            _ => None,
        }
    }

    /// Whether this record satisfies one filter: the values of a filter are alternatives
    /// for the property it names, compared for equality (the one operator the model
    /// declares).
    pub fn matches_filter(&self, key: &str, values: &[String]) -> bool {
        self.filter_value(key)
            .is_some_and(|held| values.iter().any(|want| want == held))
    }
}

impl ComAmazonawsSsmAmazonssmState {
    /// Every related item `region` holds, in ascending `(ops item id, association id)`
    /// order, so a listing walks a stable, total order and can resume a page from the
    /// association id it stopped at.
    ///
    /// The walk starts at the Region's own first key and stops at its last, so it visits
    /// only the records of that Region rather than every record the store holds.
    pub fn related_items_in_region<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = &'a RelatedItemRecord> + 'a {
        self.ops_item_related_items
            .range((region.to_string(), String::new(), String::new())..)
            .take_while(move |((held_region, _, _), _)| held_region == region)
            .map(|(_, record)| record)
    }

    /// The association `ops_item_id` already holds for `resource_uri`, or `None` when that
    /// ARN is not associated with the OpsItem yet: one resource is a related item of an
    /// OpsItem at most once.
    ///
    /// The walk is over that OpsItem's own keys alone, for the reason above.
    pub fn related_item_for_resource(
        &self,
        region: &str,
        ops_item_id: &str,
        resource_uri: &str,
    ) -> Option<&RelatedItemRecord> {
        self.ops_item_related_items
            .range((region.to_string(), ops_item_id.to_string(), String::new())..)
            .take_while(|((held_region, held_ops_item, _), _)| {
                held_region == region && held_ops_item == ops_item_id
            })
            .map(|(_, record)| record)
            .find(|held| held.resource_uri == resource_uri)
    }

    /// An association id no association of this store carries, and the mint counter
    /// advanced past it.
    ///
    /// "The association ID" is minted through this store's ONE identifier minter
    /// ([`super::minted_uuid`]), so it carries nothing of the request that created it.
    pub fn next_related_item_association_id(&mut self) -> String {
        let since_epoch = self
            .created_at
            .duration_since(::std::time::UNIX_EPOCH)
            .unwrap_or_default();
        loop {
            self.ops_item_related_item_sequence += 1;
            let candidate =
                super::minted_uuid(since_epoch.as_nanos(), self.ops_item_related_item_sequence);
            if !self
                .ops_item_related_items
                .keys()
                .any(|(_, _, association_id)| association_id == &candidate)
            {
                return candidate;
            }
        }
    }

    /// Store `record` as its OpsItem's association of that id.
    ///
    /// Refuses with the bound ([`MAX_RELATED_ITEMS_TOTAL`]) when storing it would take the
    /// store past it, leaving the store untouched: this is the one write path, so the bound
    /// holds however a caller reached it.
    pub fn insert_related_item(&mut self, record: RelatedItemRecord) -> Result<(), usize> {
        let key = (
            record.region.clone(),
            record.ops_item_id.clone(),
            record.association_id.clone(),
        );
        if !self.ops_item_related_items.contains_key(&key)
            && self.ops_item_related_items.len() >= MAX_RELATED_ITEMS_TOTAL
        {
            return Err(MAX_RELATED_ITEMS_TOTAL);
        }
        self.ops_item_related_items.insert(key, record);
        Ok(())
    }
}
