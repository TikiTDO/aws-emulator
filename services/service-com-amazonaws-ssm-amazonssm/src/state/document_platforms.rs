// SPDX-License-Identifier: CC0-1.0

//! Which operating-system platforms an SSM document's own content is compatible with.
//!
//! "The list of operating system (OS) platforms compatible with this SSM document"
//! (`smithy_query(query="shape",shape="DocumentDescription")`, member `PlatformTypes`) is
//! DERIVED from the document, never from the request: a document's steps decide it, so
//! `CreateDocument`, `DescribeDocument`, `UpdateDocument` and `ListDocuments` all report
//! the platforms this module reads off the one parsed content
//! ([`super::parse_document_content`]).
//!
//! Two things in the content restrict a step to a platform, and a step runs where BOTH
//! allow it:
//!
//! * its ACTION — the plugin the step runs. A plugin that runs shell commands runs on the
//!   Unix-like platforms and the PowerShell one on Windows ([`plugin_platform_types`]).
//! * its `platformType` PRECONDITION (`precondition: {StringEquals: [platformType,
//!   Linux]}`), which narrows a step to the platforms it names.
//!
//! The document is compatible with the union over its steps, because any one of them
//! running on a platform makes the document runnable there. Content whose steps restrict
//! nothing — an Automation runbook, whose actions run in the service rather than on a
//! node — is compatible with every platform the model lists, which is what Systems Manager
//! reports for one.

use super::{mapping_get, scalar_string, DOCUMENT_PLATFORM_TYPES};

/// The platforms the plugin `action` runs on, or `None` for one that runs on every
/// platform the model lists.
///
/// The entries are the plugins the "command document plugin reference" of the Amazon Web
/// Services Systems Manager User Guide documents a platform restriction for; a plugin it
/// documents for every platform (`aws:updateSsmAgent`, `aws:downloadContent`,
/// `aws:runDocument`, `aws:softwareInventory`) and every Automation action restrict
/// nothing and so are absent here. The `aws:cloudWatch` spelling is the schema 1.2 name of
/// the same Windows-only plugin `aws:configureCloudWatch` is in later schemas.
pub fn plugin_platform_types(action: &str) -> Option<&'static [&'static str]> {
    match action {
        // Runs the commands through a Unix-like shell.
        "aws:runShellScript" => Some(&["Linux", "MacOS"]),
        // Windows-only: PowerShell, a PowerShell module, an MSI application, and the
        // CloudWatch agent configuration.
        "aws:runPowerShellScript" | "aws:psModule" | "aws:applications" | "aws:cloudWatch"
        | "aws:configureCloudWatch" => Some(&["Windows"]),
        _ => None,
    }
}

/// The platforms `content`'s steps leave the document compatible with, reported in the
/// order the modelled `PlatformType` enum lists them
/// ([`DOCUMENT_PLATFORM_TYPES`]) — so two reads of one document agree and a
/// caller reading `PlatformTypes[0]` reads the platform the service names first.
pub fn declared_platform_types(map: &::yaml_rust2::yaml::Hash) -> Vec<String> {
    let mut compatible: Vec<&str> = Vec::new();
    let mut saw_step = false;
    for (action, precondition) in declared_steps(map) {
        saw_step = true;
        let by_action = plugin_platform_types(&action).unwrap_or(DOCUMENT_PLATFORM_TYPES);
        for platform in by_action {
            let allowed = precondition.is_empty() || precondition.iter().any(|named| named == platform);
            if allowed && !compatible.contains(platform) {
                compatible.push(platform);
            }
        }
    }
    // Content that declares no step at all, and content whose steps contradict themselves
    // (a precondition naming no platform its own action runs on), restricts the document
    // to nothing in particular, so it is reported compatible with every platform.
    if !saw_step || compatible.is_empty() {
        return DOCUMENT_PLATFORM_TYPES
            .iter()
            .map(|platform| (*platform).to_string())
            .collect();
    }
    DOCUMENT_PLATFORM_TYPES
        .iter()
        .filter(|platform| compatible.contains(*platform))
        .map(|platform| (*platform).to_string())
        .collect()
}

/// One entry per step `content` declares, as `(the plugin it runs, the platforms its
/// `platformType` precondition names)`.
///
/// A schema 2.x document declares its steps as `mainSteps`, each with its own `action`,
/// and a schema 1.2 document declares them as the `runtimeConfig` plugin entries, whose
/// KEYS are the plugin names and which carry no precondition — so both spellings name the
/// plugin that decides the step's platforms.
fn declared_steps(map: &::yaml_rust2::yaml::Hash) -> Vec<(String, Vec<String>)> {
    if let Some(steps) = mapping_get(map, "mainSteps").and_then(::yaml_rust2::Yaml::as_vec) {
        return steps
            .iter()
            .filter_map(|step| {
                let step = step.as_hash()?;
                let action = mapping_get(step, "action")
                    .and_then(scalar_string)
                    .unwrap_or_default();
                Some((action, precondition_platform_types(step)))
            })
            .collect();
    }
    mapping_get(map, "runtimeConfig")
        .and_then(::yaml_rust2::Yaml::as_hash)
        .map(|plugins| {
            plugins
                .keys()
                .filter_map(::yaml_rust2::Yaml::as_str)
                .map(|plugin| (plugin.to_string(), Vec::new()))
                .collect()
        })
        .unwrap_or_default()
}

/// The platforms a step's `platformType` precondition names, or an EMPTY list for a step
/// that carries no such precondition and so is restricted by its action alone.
///
/// A precondition is written as `precondition: {StringEquals: [platformType, <platform>]}`,
/// so the first element names what is compared and the rest the values it may equal. A
/// value outside the modelled enum names no platform and is ignored.
fn precondition_platform_types(step: &::yaml_rust2::yaml::Hash) -> Vec<String> {
    let Some(equals) = mapping_get(step, "precondition")
        .and_then(::yaml_rust2::Yaml::as_hash)
        .and_then(|precondition| mapping_get(precondition, "StringEquals"))
        .and_then(::yaml_rust2::Yaml::as_vec)
    else {
        return Vec::new();
    };
    if equals.first().and_then(::yaml_rust2::Yaml::as_str) != Some("platformType") {
        return Vec::new();
    }
    equals
        .iter()
        .skip(1)
        .filter_map(::yaml_rust2::Yaml::as_str)
        .filter(|platform| DOCUMENT_PLATFORM_TYPES.contains(platform))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The platforms the content declares, read through the one content reader a create
    /// reads it with.
    fn platforms(content: &str) -> Vec<String> {
        let document = super::super::load_single_document(content).expect("content parses");
        let map = document.as_hash().expect("content is a mapping").clone();
        declared_platform_types(&map)
    }

    /// RULE: a step runs where its plugin AND its precondition both allow it, and the
    /// document is compatible with the union over its steps, in the modelled enum's order.
    #[test]
    fn the_platforms_are_the_union_over_the_steps() {
        // (case, content, expected platforms)
        let rows: &[(&str, &str, &[&str])] = &[
            (
                "a shell step with no precondition",
                r#"{"schemaVersion":"2.2","mainSteps":[{"name":"s","action":"aws:runShellScript"}]}"#,
                &["Linux", "MacOS"],
            ),
            (
                "a PowerShell step with no precondition",
                r#"{"schemaVersion":"2.2","mainSteps":[{"name":"s","action":"aws:runPowerShellScript"}]}"#,
                &["Windows"],
            ),
            (
                "a shell step narrowed to MacOS",
                r#"{"schemaVersion":"2.2","mainSteps":[{"name":"s","action":"aws:runShellScript",
                   "precondition":{"StringEquals":["platformType","MacOS"]}}]}"#,
                &["MacOS"],
            ),
            (
                "a shell step and a PowerShell step",
                r#"{"schemaVersion":"2.2","mainSteps":[{"name":"a","action":"aws:runPowerShellScript"},
                   {"name":"b","action":"aws:runShellScript"}]}"#,
                &["Windows", "Linux", "MacOS"],
            ),
            (
                "a step whose precondition contradicts its plugin",
                r#"{"schemaVersion":"2.2","mainSteps":[{"name":"s","action":"aws:runShellScript",
                   "precondition":{"StringEquals":["platformType","Windows"]}}]}"#,
                &["Windows", "Linux", "MacOS"],
            ),
            (
                "an Automation action, which restricts nothing",
                r#"{"schemaVersion":"0.3","mainSteps":[{"name":"s","action":"aws:executeAwsApi"}]}"#,
                &["Windows", "Linux", "MacOS"],
            ),
            (
                "no step at all",
                r#"{"schemaVersion":"1.0","description":"no steps"}"#,
                &["Windows", "Linux", "MacOS"],
            ),
            (
                "a schema 1.2 runtimeConfig plugin",
                r#"{"schemaVersion":"1.2","runtimeConfig":{"aws:runShellScript":{"properties":[]}}}"#,
                &["Linux", "MacOS"],
            ),
            (
                "a schema 1.2 Windows-only plugin",
                r#"{"schemaVersion":"1.2","runtimeConfig":{"aws:psModule":{"properties":[]}}}"#,
                &["Windows"],
            ),
        ];
        for (case, content, expected) in rows {
            let reported = platforms(content);
            assert_eq!(
                reported, *expected,
                "case {case:?}: expected PlatformTypes {expected:?}, got {reported:?}"
            );
        }
    }

    /// RULE: the order a document's platforms are reported in is the modelled enum's, so
    /// the hand-written listing may not drift from the generated value set.
    #[test]
    fn the_listing_carries_every_modelled_platform() {
        let mut listed: Vec<&str> = DOCUMENT_PLATFORM_TYPES.to_vec();
        listed.sort_unstable();
        let mut modelled: Vec<&str> = crate::ops::types::PLATFORM_TYPE_VALUES.to_vec();
        modelled.sort_unstable();
        assert_eq!(
            listed, modelled,
            "the published listing must hold exactly the modelled PlatformType values, \
             got {listed:?} against {modelled:?}"
        );
    }
}
