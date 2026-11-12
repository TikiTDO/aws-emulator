// SPDX-License-Identifier: CC0-1.0

//! The structured `path` every read reports for a property or a composite model,
//! the notification block every reported asset property carries, and which of an
//! asset model's composite models is BUILT-IN.
//!
//! ONE place for the three rules a read of an asset or an asset model projects its
//! response with, so no two views of one stored record can disagree about them:
//!
//! * a `path` is the whole chain from the ROOT of the resource down to the thing
//!   it addresses — the asset or the asset model itself, then one hop per
//!   composite model containing it, then the property when the path names one
//!   (`AssetProperty$path` is "the structured path to the property from the root
//!   of the asset", `AssetModelProperty$path` the same from the root of the asset
//!   model, and an `AssetCompositeModelPathSegment` is "one level between a
//!   composite model and the root of the asset");
//! * an asset property's notification block states where IoT SiteWise would
//!   publish that property's value updates and whether it does
//!   ([`AssetRecord::property_notification`]); and
//! * a composite model is built-in or custom, which is what the two
//!   composite-model members of a read report separately
//!   ([`is_built_in_composite_model_type`]).
//!
//! The path a STORED asset property carries is the chain WITHIN the asset — the
//! composite models it sits in, then itself — because an asset's properties are
//! materialised from the asset model once for every asset of it
//! ([`ComAmazonawsIotsitewiseAwsiotsitewiseState::materialise_asset_properties`]),
//! so the asset's own hop, which differs per asset and moves when the asset is
//! renamed, is added by the read ([`AssetRecord::property_path`]) rather than
//! stored. A model property definition stores no path at all, for the same
//! reason: the reads derive what they report.

use super::*;
use crate::ops::types::{AssetModelPropertyPathSegment, PropertyNotification};

/// The one composite model type that is NOT built-in, as
/// [`COMPOSITE_MODEL_TYPES`] lists the catalog.
pub const COMPOSITE_MODEL_TYPE_CUSTOM: &str = "CUSTOM";

/// Whether a composite model of this type is one of the service's BUILT-IN
/// kinds — an alarm, an anomaly detector — rather than a custom one.
///
/// Which of the two members of a read reports a composite model: the
/// `assetModelCompositeModels` / `assetCompositeModels` list is "the list of
/// built-in composite models for the asset model, such as those with those of
/// type AWS/ALARMS" (`smithy_query(query="doc",
/// shape="DescribeAssetModelResponse", member="assetModelCompositeModels")`),
/// while the summaries member is "the list of the immediate child custom
/// composite model summaries". Every type in the documented catalog other than
/// [`COMPOSITE_MODEL_TYPE_CUSTOM`] names one of the service's own kinds, so the
/// question is asked of the type rather than of a hard-coded pair of names.
pub fn is_built_in_composite_model_type(composite_type: &str) -> bool {
    composite_type != COMPOSITE_MODEL_TYPE_CUSTOM
}

impl AssetModelRecord {
    /// The first hop of every structured path this asset model reports: the model
    /// itself, which is the root the paths are measured from.
    pub fn root_path_segment(&self) -> AssetModelCompositeModelPathSegment {
        AssetModelCompositeModelPathSegment {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
        }
    }

    /// The composite models from the outermost ancestor of `id` down to `id`
    /// itself, one hop each — the part of a path BELOW the model root.
    ///
    /// Empty for a composite model this model does not hold; one hop for a
    /// top-level one.
    pub fn composite_model_chain(&self, id: &str) -> Vec<AssetModelCompositeModelPathSegment> {
        let mut chain = Vec::new();
        let mut at = Some(id.to_string());
        // Bounded by the number of stored composite models, so a link that
        // somehow pointed into a cycle cannot spin here.
        while let Some(current) = at.take() {
            let Some(composite) = self.composite_model(&current) else {
                break;
            };
            chain.push(AssetModelCompositeModelPathSegment {
                id: Some(current.clone()),
                name: Some(composite.name.clone()),
            });
            if chain.len() >= self.composite_models.len() {
                break;
            }
            at = self.composite_model_parent(&current).map(ToString::to_string);
        }
        chain.reverse();
        chain
    }

    /// The `AssetModelCompositeModelPath` of composite model `id`: this asset
    /// model, then every composite model from the outermost ancestor of `id` down
    /// to `id` itself.
    pub fn composite_model_path(&self, id: &str) -> Vec<AssetModelCompositeModelPathSegment> {
        let mut path = vec![self.root_path_segment()];
        path.extend(self.composite_model_chain(id));
        path
    }

    /// The `AssetModelPropertyPath` of one property definition of this model:
    /// this asset model, the composite model `composite_id` contributes it
    /// through (and that composite model's own ancestors), then the property.
    ///
    /// `composite_id` is `None` for one of the model's own top-level definitions,
    /// whose path is therefore the model root followed by the property.
    pub fn property_path(
        &self,
        composite_id: Option<&str>,
        property: &AssetModelProperty,
    ) -> Vec<AssetModelPropertyPathSegment> {
        let mut path: Vec<AssetModelPropertyPathSegment> = composite_id
            .map(|composite_id| self.composite_model_path(composite_id))
            .unwrap_or_else(|| vec![self.root_path_segment()])
            .into_iter()
            .map(|segment| AssetModelPropertyPathSegment {
                id: segment.id,
                name: segment.name,
            })
            .collect();
        path.push(AssetModelPropertyPathSegment {
            id: property.id.clone(),
            name: Some(property.name.clone()),
        });
        path
    }

    /// One property definition as a read of this model reports it: the stored
    /// definition carrying the [`Self::property_path`] that addresses it.
    pub fn reported_property(
        &self,
        composite_id: Option<&str>,
        property: &AssetModelProperty,
    ) -> AssetModelProperty {
        AssetModelProperty {
            path: Some(self.property_path(composite_id, property)),
            ..property.clone()
        }
    }

    /// Every definition the composite model `composite_id` contributes, as a read
    /// reports them — through [`Self::reported_property`], so each carries its
    /// path from the model root.
    pub fn reported_properties(
        &self,
        composite_id: Option<&str>,
        properties: &[AssetModelProperty],
    ) -> Vec<AssetModelProperty> {
        properties
            .iter()
            .map(|property| self.reported_property(composite_id, property))
            .collect()
    }

    /// Whether composite model `id` of this model is a BUILT-IN one; see
    /// [`is_built_in_composite_model_type`]. `false` for an id this model does
    /// not hold.
    pub fn is_built_in_composite_model(&self, id: &str) -> bool {
        self.composite_model(id)
            .is_some_and(|composite| is_built_in_composite_model_type(&composite.r#type))
    }

    /// The ids among `ids` whose composite models are built-in, in the order
    /// given: what the `assetModelCompositeModels` / `assetCompositeModels`
    /// member of a read reports.
    pub fn built_in_composite_model_ids(&self, ids: &[String]) -> Vec<String> {
        ids.iter()
            .filter(|id| self.is_built_in_composite_model(id))
            .cloned()
            .collect()
    }
}

impl AssetRecord {
    /// The first hop of every structured path this asset reports: the asset
    /// itself, which is the root the paths are measured from.
    pub fn root_path_segment(&self) -> AssetPropertyPathSegment {
        AssetPropertyPathSegment {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
        }
    }

    /// The `AssetPropertyPath` of one property of this asset: this asset, then the
    /// composite models the property sits inside, then the property itself.
    ///
    /// The hops below the asset are the ones the property was materialised with
    /// (which already end at the property); a property of the asset's own
    /// definitions sits inside no composite model, so its path is the asset
    /// followed by the property.
    pub fn property_path(&self, property: &AssetProperty) -> Vec<AssetPropertyPathSegment> {
        let mut path = vec![self.root_path_segment()];
        match property.path.as_deref() {
            Some(within) => path.extend(within.iter().cloned()),
            None => path.push(AssetPropertyPathSegment {
                id: Some(property.id.clone()),
                name: Some(property.name.clone()),
            }),
        }
        path
    }

    /// The `PropertyNotification` of one property of this asset: the MQTT topic
    /// IoT SiteWise publishes that property's value updates to, and whether it
    /// does.
    ///
    /// Both members are `@required`, so a property reports the block whether or
    /// not `UpdateAssetProperty` has ever been called on it: the topic is derived
    /// from the model, asset and property ids ([`property_notification_topic`]),
    /// and the state is the one last written, or
    /// [`NOTIFICATION_STATE_DISABLED`] — the member's own documented default —
    /// for a property no write has touched.
    pub fn property_notification(&self, property: &AssetProperty) -> PropertyNotification {
        PropertyNotification {
            topic: property_notification_topic(&self.asset_model_id, &self.id, &property.id),
            state: property
                .notification
                .as_ref()
                .map(|notification| notification.state.clone())
                .unwrap_or_else(|| NOTIFICATION_STATE_DISABLED.to_string()),
        }
    }

    /// One property as a read of this asset reports it: the stored property
    /// carrying the path that addresses it from the asset root and the
    /// notification block derived for it.
    pub fn reported_property(&self, property: &AssetProperty) -> AssetProperty {
        AssetProperty {
            path: Some(self.property_path(property)),
            notification: Some(self.property_notification(property)),
            ..property.clone()
        }
    }

    /// The `AssetCompositeModelPath` of this asset's instance of composite model
    /// `composite_id`: this asset, then the composite models from the outermost
    /// ancestor down to `composite_id` itself.
    ///
    /// An asset composite model is an instance of a composite model of `model`,
    /// so the chain of composite models is that model's own
    /// ([`AssetModelRecord::composite_model_chain`]); the root it hangs from is
    /// this asset, which is what the asset-side segment documents ("one level
    /// between a composite model and the root of the asset").
    pub fn composite_model_path(
        &self,
        model: &AssetModelRecord,
        composite_id: &str,
    ) -> Vec<AssetCompositeModelPathSegment> {
        let root = self.root_path_segment();
        let mut path = vec![AssetCompositeModelPathSegment {
            id: root.id,
            name: root.name,
        }];
        path.extend(
            model
                .composite_model_chain(composite_id)
                .into_iter()
                .map(|segment| AssetCompositeModelPathSegment {
                    id: segment.id,
                    name: segment.name,
                }),
        );
        path
    }

    /// Composite model `composite_id` of `model` as THIS asset's instance of it
    /// is summarised, derived from
    /// [`AssetModelRecord::composite_model_summary`] so the model view and the
    /// asset view of one stored composite model cannot disagree.
    ///
    /// The asset-side summary states its description unconditionally and carries
    /// the path rooted at the asset, which is the only difference.
    pub fn composite_model_summary(
        &self,
        model: &AssetModelRecord,
        composite_id: &str,
    ) -> Option<AssetCompositeModelSummary> {
        let summary = model.composite_model_summary(composite_id)?;
        Some(AssetCompositeModelSummary {
            id: summary.id,
            external_id: summary.external_id,
            name: summary.name,
            r#type: summary.r#type,
            description: summary.description.unwrap_or_default(),
            path: self.composite_model_path(model, composite_id),
        })
    }
}
