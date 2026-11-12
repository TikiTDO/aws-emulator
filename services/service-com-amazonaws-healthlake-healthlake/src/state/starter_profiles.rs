// SPDX-License-Identifier: CC0-1.0

//! The STARTER PROFILE library this service creates a profile from when the
//! create's `Source` names a built-in starter profile.
//!
//! `CreateDataTransformationProfile` documents the variant as "Creates the profile
//! from a built-in starter profile. Valid only when the source format is
//! Consolidated Clinical Document Architecture (C-CDA)", so the content a starter
//! profile resolves to is content the SERVICE holds, not content the request
//! carries. This module is where this emulator holds it: a finite library of
//! C-CDA-to-FHIR-R4 starter profiles, one per C-CDA document type, whose DRAFT
//! content is BUILT from the two catalogs below. A create naming a library entry
//! stores that entry's content as the new profile's DRAFT, which
//! `GetDataTransformationProfile` then reads back; a name the library does not
//! hold resolves to nothing and is refused.
//!
//! # Where the content comes from
//!
//! AWS publishes neither the names nor the content of its own starter profiles, so
//! what this library holds is this emulator's, built from the published standards
//! the profiles convert BETWEEN rather than copied from anywhere:
//!
//! * the document-level templates of HL7 CDA R2 Implementation Guide: C-CDA
//!   Release 2.1, and the section templates each of those documents carries, are
//!   [`STARTER_PROFILES`]; and
//! * the FHIR R4 resource each of those sections' entries becomes, from HL7's
//!   C-CDA on FHIR mapping of the C-CDA section templates onto FHIR R4 resource
//!   types, is [`SECTION_TARGETS`].
//!
//! Each entry's mapping is COMPUTED from those two catalogs by
//! [`starter_profile_mapping`] — one file per section the document carries, plus
//! the manifest of the entry itself — so the library cannot hold a file for a
//! section no document has, or a target the mapping catalog does not name.

use std::collections::BTreeMap;

use super::{content_file_slug, normalized_content_name, profile_target_format, PROFILE_CONTENT_MANIFEST_FILE};

/// The directory the per-section mapping files live under.
const SECTION_DIRECTORY: &str = "sections";

/// The source format a starter profile converts FROM, as the modelled
/// `SourceFormat` enum spells it — the format the variant is documented as valid
/// for ("Valid only when the source format is Consolidated Clinical Document
/// Architecture (C-CDA)").
pub const STARTER_PROFILE_SOURCE_FORMAT: &str = "CCDA";

/// The FHIR R4 resource type the entries of each C-CDA section convert to.
///
/// From HL7's C-CDA on FHIR mapping of the C-CDA R2.1 section templates onto FHIR
/// R4 resource types. A section whose entries are narrative rather than coded maps
/// to the `Composition` the document itself becomes, which is where FHIR R4 carries
/// a document's narrative sections.
///
/// Sorted by section name, and read only through [`section_target`].
const SECTION_TARGETS: &[(&str, &str)] = &[
    ("Admission Diagnosis", "Condition"),
    ("Admission Medications", "MedicationStatement"),
    ("Advance Directives", "Consent"),
    ("Allergies and Intolerances", "AllergyIntolerance"),
    ("Anesthesia", "Procedure"),
    ("Assessment and Plan", "Composition"),
    ("Chief Complaint", "Composition"),
    ("Complications", "Condition"),
    ("Discharge Diagnosis", "Condition"),
    ("Discharge Medications", "MedicationStatement"),
    ("Encounters", "Encounter"),
    ("Family History", "FamilyMemberHistory"),
    ("Functional Status", "Observation"),
    ("Goals", "Goal"),
    ("Health Concerns", "Condition"),
    ("Health Status Evaluations and Outcomes", "Observation"),
    ("History of Present Illness", "Composition"),
    ("Hospital Course", "Encounter"),
    ("Hospital Discharge Instructions", "Composition"),
    ("Immunizations", "Immunization"),
    ("Instructions", "Composition"),
    ("Interventions", "Procedure"),
    ("Medical Equipment", "DeviceUseStatement"),
    ("Medications", "MedicationStatement"),
    ("Mental Status", "Observation"),
    ("Nutrition", "Observation"),
    ("Past Medical History", "Condition"),
    ("Payers", "Coverage"),
    ("Physical Exam", "Observation"),
    ("Plan of Treatment", "CarePlan"),
    ("Postoperative Diagnosis", "Condition"),
    ("Postprocedure Diagnosis", "Condition"),
    ("Preoperative Diagnosis", "Condition"),
    ("Problems", "Condition"),
    ("Procedure Description", "Procedure"),
    ("Procedure Findings", "Observation"),
    ("Procedure Indications", "Condition"),
    ("Procedures", "Procedure"),
    ("Reason for Referral", "ServiceRequest"),
    ("Results", "Observation"),
    ("Review of Systems", "Composition"),
    ("Social History", "Observation"),
    ("Vital Signs", "Observation"),
];

/// One entry of the library: the name it is addressed by, the C-CDA document type
/// it converts, and the section templates that document carries.
#[derive(Debug, Clone, Copy)]
pub struct StarterProfile {
    /// The name a create addresses this entry by, as the library publishes it.
    pub name: &'static str,
    /// The C-CDA document-level template this entry converts.
    pub document_type: &'static str,
    /// The section templates that document carries, each of which becomes one
    /// mapping file. Every one of them is named by [`SECTION_TARGETS`].
    pub sections: &'static [&'static str],
}

/// The library: one entry per C-CDA R2.1 document-level template this emulator
/// ships a starter profile for, with the section templates that document carries.
///
/// Sorted by name, which is the order [`starter_profile_names`] reports and a
/// refusal lists them in. Read only through [`starter_profile`].
pub const STARTER_PROFILES: &[StarterProfile] = &[
    StarterProfile {
        name: "CARE_PLAN",
        document_type: "Care Plan",
        sections: &[
            "Health Concerns",
            "Goals",
            "Interventions",
            "Health Status Evaluations and Outcomes",
            "Problems",
            "Allergies and Intolerances",
            "Medications",
            "Plan of Treatment",
        ],
    },
    StarterProfile {
        name: "CONSULTATION_NOTE",
        document_type: "Consultation Note",
        sections: &[
            "Reason for Referral",
            "History of Present Illness",
            "Assessment and Plan",
            "Allergies and Intolerances",
            "Medications",
            "Problems",
            "Results",
            "Physical Exam",
            "Vital Signs",
            "Social History",
            "Family History",
            "Review of Systems",
        ],
    },
    StarterProfile {
        name: "CONTINUITY_OF_CARE_DOCUMENT",
        document_type: "Continuity of Care Document",
        sections: &[
            "Allergies and Intolerances",
            "Medications",
            "Problems",
            "Procedures",
            "Results",
            "Social History",
            "Vital Signs",
            "Encounters",
            "Immunizations",
            "Medical Equipment",
            "Payers",
            "Plan of Treatment",
            "Functional Status",
            "Mental Status",
            "Nutrition",
            "Advance Directives",
            "Family History",
            "Goals",
            "Health Concerns",
        ],
    },
    StarterProfile {
        name: "DISCHARGE_SUMMARY",
        document_type: "Discharge Summary",
        sections: &[
            "Admission Diagnosis",
            "Admission Medications",
            "Allergies and Intolerances",
            "Chief Complaint",
            "Discharge Diagnosis",
            "Discharge Medications",
            "Hospital Course",
            "Hospital Discharge Instructions",
            "Problems",
            "Procedures",
            "Results",
            "Vital Signs",
            "Immunizations",
            "Functional Status",
            "Advance Directives",
            "Plan of Treatment",
        ],
    },
    StarterProfile {
        name: "HISTORY_AND_PHYSICAL",
        document_type: "History and Physical",
        sections: &[
            "Chief Complaint",
            "History of Present Illness",
            "Past Medical History",
            "Allergies and Intolerances",
            "Medications",
            "Problems",
            "Family History",
            "Social History",
            "Review of Systems",
            "Physical Exam",
            "Vital Signs",
            "Results",
            "Assessment and Plan",
        ],
    },
    StarterProfile {
        name: "OPERATIVE_NOTE",
        document_type: "Operative Note",
        sections: &[
            "Anesthesia",
            "Complications",
            "Preoperative Diagnosis",
            "Postoperative Diagnosis",
            "Procedure Description",
            "Procedure Findings",
            "Allergies and Intolerances",
            "Medications",
            "Vital Signs",
        ],
    },
    StarterProfile {
        name: "PROCEDURE_NOTE",
        document_type: "Procedure Note",
        sections: &[
            "Procedure Indications",
            "Procedure Description",
            "Procedure Findings",
            "Complications",
            "Postprocedure Diagnosis",
            "Allergies and Intolerances",
            "Medications",
            "Vital Signs",
        ],
    },
    StarterProfile {
        name: "PROGRESS_NOTE",
        document_type: "Progress Note",
        sections: &[
            "Assessment and Plan",
            "Allergies and Intolerances",
            "Medications",
            "Problems",
            "Results",
            "Vital Signs",
            "Physical Exam",
            "Instructions",
            "Interventions",
        ],
    },
    StarterProfile {
        name: "REFERRAL_NOTE",
        document_type: "Referral Note",
        sections: &[
            "Reason for Referral",
            "Allergies and Intolerances",
            "Medications",
            "Problems",
            "Results",
            "Vital Signs",
            "Immunizations",
            "Family History",
            "Social History",
            "Plan of Treatment",
            "Functional Status",
        ],
    },
    StarterProfile {
        name: "TRANSFER_SUMMARY",
        document_type: "Transfer Summary",
        sections: &[
            "Reason for Referral",
            "Allergies and Intolerances",
            "Medications",
            "Problems",
            "Procedures",
            "Results",
            "Vital Signs",
            "Immunizations",
            "Plan of Treatment",
            "Functional Status",
            "Mental Status",
            "Advance Directives",
            "Social History",
            "Payers",
            "Medical Equipment",
        ],
    },
];

/// The names the library publishes, in the order a refusal lists them.
pub fn starter_profile_names() -> Vec<&'static str> {
    STARTER_PROFILES.iter().map(|entry| entry.name).collect()
}

/// The library entry `name` addresses, or `None` for a name the library does not
/// hold.
///
/// A name is matched on its letters and digits alone, so an entry is addressable
/// however a caller spells the published name's word separators and case. Nothing
/// about the name reaches the content: what a match resolves to is the entry's own
/// catalog row.
pub fn starter_profile(name: &str) -> Option<&'static StarterProfile> {
    let addressed = normalized_content_name(name);
    STARTER_PROFILES
        .iter()
        .find(|entry| normalized_content_name(entry.name) == addressed)
}

/// The DRAFT content the starter profile `name` resolves to, or `None` for a name
/// the library does not hold.
///
/// Built from the entry's catalog row: one mapping file per section the document
/// carries, naming the section and the FHIR R4 resource its entries convert to,
/// plus the manifest of the entry itself. The result satisfies the modelled
/// `ProfileMapping` bounds — at most one file per section of one document, each
/// well inside the per-entry bounds — which the create validates for every source
/// alike.
pub fn starter_profile_mapping(name: &str) -> Option<BTreeMap<String, String>> {
    let entry = starter_profile(name)?;
    let mut mapping: BTreeMap<String, String> = BTreeMap::new();
    let mut mapped_sections: Vec<&str> = Vec::new();
    for section in entry.sections {
        let Some(target) = section_target(section) else {
            // Unreachable: every section of every library entry is named by
            // `SECTION_TARGETS`, which `the_library_maps_every_section_it_carries`
            // pins. A section with no target maps to nothing, so it contributes no
            // file rather than a file naming no FHIR R4 resource.
            continue;
        };
        mapping.insert(
            format!("{SECTION_DIRECTORY}/{}.json", content_file_slug(section)),
            ::serde_json::json!({
                "sourceFormat": STARTER_PROFILE_SOURCE_FORMAT,
                "sourceSection": section,
                "targetFormat": profile_target_format(),
                "targetResourceType": target,
            })
            .to_string(),
        );
        mapped_sections.push(section);
    }
    mapping.insert(
        PROFILE_CONTENT_MANIFEST_FILE.to_string(),
        ::serde_json::json!({
            "starterProfileName": entry.name,
            "sourceFormat": STARTER_PROFILE_SOURCE_FORMAT,
            "sourceDocumentType": entry.document_type,
            "targetFormat": profile_target_format(),
            "sections": mapped_sections,
        })
        .to_string(),
    );
    Some(mapping)
}

/// The sentence a create is refused with when its `Source` names a starter profile
/// this library does not hold: the member at fault and the names it does hold, so a
/// caller correcting its request learns what to send. The name it sent is not quoted
/// back, as no refusal of this service quotes a caller's value back.
pub fn starter_profile_not_found_message() -> String {
    format!(
        "Source.StarterProfile.StarterProfileName must name a built-in starter profile: [{}]",
        starter_profile_names().join(", ")
    )
}

/// The FHIR R4 resource type the entries of the C-CDA section `section` convert to,
/// or `None` for a section [`SECTION_TARGETS`] does not name.
fn section_target(section: &str) -> Option<&'static str> {
    SECTION_TARGETS
        .iter()
        .find(|(name, _)| *name == section)
        .map(|(_, target)| *target)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: every section of every library entry has a FHIR R4 target, so an
    /// entry's content is one file per section it carries and the mapping cannot
    /// silently drop one.
    #[test]
    fn the_library_maps_every_section_it_carries() {
        for entry in STARTER_PROFILES {
            for section in entry.sections {
                assert!(
                    section_target(section).is_some(),
                    "case {:?}: the section {section:?} has no FHIR R4 target",
                    entry.name
                );
            }
            let mapping = starter_profile_mapping(entry.name)
                .unwrap_or_else(|| panic!("case {:?}: the library must hold it", entry.name));
            let expected = entry.sections.len() + 1;
            assert_eq!(
                mapping.len(),
                expected,
                "case {:?}: expected one file per section plus the manifest ({expected}), got {}",
                entry.name,
                mapping.len()
            );
        }
    }

    /// RULE: a library name is addressed by its letters and digits, so case and
    /// word separators do not decide whether an entry resolves.
    #[test]
    fn a_name_is_addressed_by_its_letters_and_digits() {
        let rows: &[(&str, &str, Option<&str>)] = &[
            ("the published spelling", "PROGRESS_NOTE", Some("PROGRESS_NOTE")),
            ("lowercased", "progress_note", Some("PROGRESS_NOTE")),
            ("hyphenated", "Progress-Note", Some("PROGRESS_NOTE")),
            ("spaced", "progress note", Some("PROGRESS_NOTE")),
            ("another entry", "care-plan", Some("CARE_PLAN")),
            ("a name the library does not hold", "PROGRESS", None),
            ("the empty name", "", None),
        ];
        for (case, addressed, expected) in rows {
            let resolved = starter_profile(addressed).map(|entry| entry.name);
            assert_eq!(
                resolved, *expected,
                "case {case:?}: expected {expected:?}, got {resolved:?}"
            );
        }
    }
}
