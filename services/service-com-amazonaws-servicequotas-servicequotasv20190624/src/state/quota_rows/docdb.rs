// SPDX-License-Identifier: CC0-1.0

//! Amazon DocumentDB's published default-quota catalog.

use super::*;

/// Amazon DocumentDB's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `docdb` service code (the "Quotas
/// and limits in Amazon DocumentDB" page of its Developer Guide), and the recorded
/// default-quota listing of that code for which `QuotaCode` names which quota, for the
/// value and adjustability each publishes and for the `Description` the service publishes
/// under a code the listing states one for — a terse sentence naming the plane the quota
/// counts, with no trailing stop. The remaining codes that listing reports name quotas
/// whose published name or value no source available here states, and a code identifies
/// ONE quota, so those rows are left to the default-quota-set gap rather than published
/// under a guessed binding (see [`crate::state::descriptive_quota_code`]).
pub(in crate::state) fn docdb_quotas() -> Vec<QuotaRow> {
    vec![
        quota("Subnet groups", 50.0)
            .code("L-02DEA053")
            .describe("The maximum number of DB subnet groups"),
        quota("Clusters", 40.0)
            .code("L-13F31459")
            // The `Description`s on this row and the next are this catalog's own wording,
            // in the terse form this service's published descriptions take: it publishes
            // one for each of these quotas and its text is not among the data available
            // here.
            .describe("The maximum number of DB clusters"),
        quota("Read replicas per cluster", 15.0)
            .code("L-5BA57179")
            .describe("The maximum number of read replicas per cluster"),
        quota("Manual cluster snapshots", 100.0)
            .code("L-B2551F83")
            .describe("The maximum number of manual snapshots"),
        // The security-group plane of a VPC, which AWS publishes as fixed.
        quota("VPC security groups per instance", 5.0)
            .code("L-D02D85EA")
            .fixed()
            .describe("The maximum number of DB security groups per Amazon VPC"),
        quota("Event subscriptions", 20.0)
            .code("L-F7FABF71")
            .describe("The maximum number of event subscriptions"),
        // The recording states this code's name but not its value. SOURCE for the 50:
        // the "Quotas and limits in Amazon DocumentDB" page, whose tag row publishes 50
        // tags per resource. AWS publishes it as a hard limit rather than as adjustable,
        // and describes it as a quota of the Amazon RDS resource the cluster is one of.
        quota("Tags per resource", 50.0)
            .code("L-3B0E1499")
            .fixed()
            .describe("The maximum number of tags per Amazon RDS resource"),
    ]
}
