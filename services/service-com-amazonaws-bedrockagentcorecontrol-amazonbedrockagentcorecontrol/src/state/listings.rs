// SPDX-License-Identifier: CC0-1.0

//! The summaries this service's paginated listings report, for the resources
//! whose own record lives in `state.rs`.
//!
//! One concern, kept beside the records rather than inside each `ops/` listing
//! file: a summary is the SAME record a read of the resource reports, projected
//! onto the summary shape the list declares, so a listing and a read cannot
//! disagree about a resource's status, name or instants.

use super::*;
use crate::ops::types::{BrowserSummary, ConfigurationBundleSummary};

/// `ResourceType` values, which a listing filters on: the resources the
/// SERVICE provides in every account, and the ones this account created. The
/// enum's full value list is `SYSTEM | CUSTOM`
/// (`crate::ops::types::RESOURCE_TYPE_VALUES`, from `smithy_query
/// shape=ResourceType`).
pub const RESOURCE_TYPE_SYSTEM: &str = "SYSTEM";
pub const RESOURCE_TYPE_CUSTOM: &str = "CUSTOM";

impl BrowserRecord {
    /// This browser as `ListBrowsers` reports it: the members `BrowserSummary`
    /// declares, every one of them off the same record [`Self::get_output`]
    /// renders, so the listing and the read report one browser alike.
    pub fn summary(&self) -> BrowserSummary {
        BrowserSummary {
            browser_id: self.browser_id.clone(),
            browser_arn: self.browser_arn.clone(),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            status: self.status.clone(),
            created_at: timestamp(self.created_at),
            last_updated_at: Some(timestamp(self.updated_at)),
        }
    }
}

impl ConfigurationBundleRecord {
    /// This bundle as `ListConfigurationBundles` reports it: the members
    /// `ConfigurationBundleSummary` declares, off the same record a read of the
    /// bundle answers from.
    ///
    /// The description belongs to a VERSION — a commit carries it, and a later
    /// commit can change it — so the summary reports the description of the
    /// bundle's latest commit, which is the one a read of the bundle reports
    /// too.
    pub fn summary(&self) -> ConfigurationBundleSummary {
        ConfigurationBundleSummary {
            bundle_arn: self.bundle_arn.clone(),
            bundle_id: self.bundle_id.clone(),
            bundle_name: self.bundle_name.clone(),
            description: self
                .versions()
                .last()
                .and_then(|version| version.description.clone()),
            created_at: Some(timestamp(self.created_at)),
        }
    }
}
