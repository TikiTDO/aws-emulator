// SPDX-License-Identifier: CC0-1.0

//! The display data AWS publishes for a Region, as the zones of that Region report it.
//!
//! `DescribeAvailabilityZones` reports three members about where a zone IS rather than
//! what it is: `GroupLongName` ("the long name of the Availability Zone group"),
//! `Geography` ("the geography information for the Availability Zone … returned as a
//! list", whose `Name` is "for example, `United States of America`") and `SubGeography`
//! (whose `Name` is "for example, `Oregon`"). None of the three can be derived from a
//! Region code, so they are held here as the catalog they are — the same way
//! `super::local_gateways_and_filter_patterns` holds a Region's endpoint and opt-in
//! status — and read by the ONE projection of a zone
//! ([`super::AvailabilityZoneRecord::to_availability_zone`]), so a report and the
//! `group-long-name` filter cannot disagree about one zone.
//!
//! A Region the catalog does not name publishes none of the three, and its zones report
//! the members absent rather than inventing them: that is every Region of an isolated
//! partition, for which AWS publishes no display name at all.

/// What AWS publishes about where ONE Region is.
///
/// `long_name` is the Region's published display name — the name the operation's
/// `group-long-name` filter doc spells out for a zone group of `us-west-2`, minus the
/// group's own number: "the long name of the zone group for the Availability Zone (for
/// example, `US West (Oregon) 1`)". `geography` is the country that Region sits in, and
/// `sub_geography` the location inside it that the display name names — absent for a
/// Region whose display name names only the country or a compass area, which publishes no
/// location below it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionDisplay {
    /// `US West (Oregon)` — the Region's published display name.
    pub long_name: &'static str,
    /// `United States of America` — the country the Region sits in.
    pub geography: &'static str,
    /// `Oregon` — the location inside that country, when the display name names one.
    pub sub_geography: Option<&'static str>,
}

/// Every Region whose display data AWS publishes, in Region-code order.
///
/// The display names are the ones AWS publishes for each Region; where a display name
/// abbreviates a location (`N. Virginia`), the location is written out, because the
/// sub-geography names the place rather than the Region. A Region whose display name names
/// the country itself (`Europe (Ireland)`) or a part of one with no published place
/// (`Canada (Central)`, `AWS GovCloud (US-West)`) carries no sub-geography.
const REGION_DISPLAY: &[(&str, RegionDisplay)] = &[
    (
        "af-south-1",
        RegionDisplay {
            long_name: "Africa (Cape Town)",
            geography: "South Africa",
            sub_geography: Some("Cape Town"),
        },
    ),
    (
        "ap-east-1",
        RegionDisplay {
            long_name: "Asia Pacific (Hong Kong)",
            geography: "China",
            sub_geography: Some("Hong Kong"),
        },
    ),
    (
        "ap-east-2",
        RegionDisplay {
            long_name: "Asia Pacific (Taipei)",
            geography: "Taiwan",
            sub_geography: Some("Taipei"),
        },
    ),
    (
        "ap-northeast-1",
        RegionDisplay {
            long_name: "Asia Pacific (Tokyo)",
            geography: "Japan",
            sub_geography: Some("Tokyo"),
        },
    ),
    (
        "ap-northeast-2",
        RegionDisplay {
            long_name: "Asia Pacific (Seoul)",
            geography: "South Korea",
            sub_geography: Some("Seoul"),
        },
    ),
    (
        "ap-northeast-3",
        RegionDisplay {
            long_name: "Asia Pacific (Osaka)",
            geography: "Japan",
            sub_geography: Some("Osaka"),
        },
    ),
    (
        "ap-south-1",
        RegionDisplay {
            long_name: "Asia Pacific (Mumbai)",
            geography: "India",
            sub_geography: Some("Mumbai"),
        },
    ),
    (
        "ap-south-2",
        RegionDisplay {
            long_name: "Asia Pacific (Hyderabad)",
            geography: "India",
            sub_geography: Some("Hyderabad"),
        },
    ),
    (
        "ap-southeast-1",
        RegionDisplay {
            long_name: "Asia Pacific (Singapore)",
            geography: "Singapore",
            sub_geography: None,
        },
    ),
    (
        "ap-southeast-2",
        RegionDisplay {
            long_name: "Asia Pacific (Sydney)",
            geography: "Australia",
            sub_geography: Some("Sydney"),
        },
    ),
    (
        "ap-southeast-3",
        RegionDisplay {
            long_name: "Asia Pacific (Jakarta)",
            geography: "Indonesia",
            sub_geography: Some("Jakarta"),
        },
    ),
    (
        "ap-southeast-4",
        RegionDisplay {
            long_name: "Asia Pacific (Melbourne)",
            geography: "Australia",
            sub_geography: Some("Melbourne"),
        },
    ),
    (
        "ap-southeast-5",
        RegionDisplay {
            long_name: "Asia Pacific (Malaysia)",
            geography: "Malaysia",
            sub_geography: None,
        },
    ),
    (
        "ap-southeast-6",
        RegionDisplay {
            long_name: "Asia Pacific (New Zealand)",
            geography: "New Zealand",
            sub_geography: None,
        },
    ),
    (
        "ap-southeast-7",
        RegionDisplay {
            long_name: "Asia Pacific (Thailand)",
            geography: "Thailand",
            sub_geography: None,
        },
    ),
    (
        "ca-central-1",
        RegionDisplay {
            long_name: "Canada (Central)",
            geography: "Canada",
            sub_geography: None,
        },
    ),
    (
        "ca-west-1",
        RegionDisplay {
            long_name: "Canada West (Calgary)",
            geography: "Canada",
            sub_geography: Some("Calgary"),
        },
    ),
    (
        "cn-north-1",
        RegionDisplay {
            long_name: "China (Beijing)",
            geography: "China",
            sub_geography: Some("Beijing"),
        },
    ),
    (
        "cn-northwest-1",
        RegionDisplay {
            long_name: "China (Ningxia)",
            geography: "China",
            sub_geography: Some("Ningxia"),
        },
    ),
    (
        "eu-central-1",
        RegionDisplay {
            long_name: "Europe (Frankfurt)",
            geography: "Germany",
            sub_geography: Some("Frankfurt"),
        },
    ),
    (
        "eu-central-2",
        RegionDisplay {
            long_name: "Europe (Zurich)",
            geography: "Switzerland",
            sub_geography: Some("Zurich"),
        },
    ),
    (
        "eu-north-1",
        RegionDisplay {
            long_name: "Europe (Stockholm)",
            geography: "Sweden",
            sub_geography: Some("Stockholm"),
        },
    ),
    (
        "eu-south-1",
        RegionDisplay {
            long_name: "Europe (Milan)",
            geography: "Italy",
            sub_geography: Some("Milan"),
        },
    ),
    (
        "eu-south-2",
        RegionDisplay {
            long_name: "Europe (Spain)",
            geography: "Spain",
            sub_geography: None,
        },
    ),
    (
        "eu-west-1",
        RegionDisplay {
            long_name: "Europe (Ireland)",
            geography: "Ireland",
            sub_geography: None,
        },
    ),
    (
        "eu-west-2",
        RegionDisplay {
            long_name: "Europe (London)",
            geography: "United Kingdom",
            sub_geography: Some("London"),
        },
    ),
    (
        "eu-west-3",
        RegionDisplay {
            long_name: "Europe (Paris)",
            geography: "France",
            sub_geography: Some("Paris"),
        },
    ),
    (
        "il-central-1",
        RegionDisplay {
            long_name: "Israel (Tel Aviv)",
            geography: "Israel",
            sub_geography: Some("Tel Aviv"),
        },
    ),
    (
        "me-central-1",
        RegionDisplay {
            long_name: "Middle East (UAE)",
            geography: "United Arab Emirates",
            sub_geography: None,
        },
    ),
    (
        "me-south-1",
        RegionDisplay {
            long_name: "Middle East (Bahrain)",
            geography: "Bahrain",
            sub_geography: None,
        },
    ),
    (
        "mx-central-1",
        RegionDisplay {
            long_name: "Mexico (Central)",
            geography: "Mexico",
            sub_geography: None,
        },
    ),
    (
        "sa-east-1",
        RegionDisplay {
            long_name: "South America (São Paulo)",
            geography: "Brazil",
            sub_geography: Some("São Paulo"),
        },
    ),
    (
        "us-east-1",
        RegionDisplay {
            long_name: "US East (N. Virginia)",
            geography: "United States of America",
            sub_geography: Some("Virginia"),
        },
    ),
    (
        "us-east-2",
        RegionDisplay {
            long_name: "US East (Ohio)",
            geography: "United States of America",
            sub_geography: Some("Ohio"),
        },
    ),
    (
        "us-gov-east-1",
        RegionDisplay {
            long_name: "AWS GovCloud (US-East)",
            geography: "United States of America",
            sub_geography: None,
        },
    ),
    (
        "us-gov-west-1",
        RegionDisplay {
            long_name: "AWS GovCloud (US-West)",
            geography: "United States of America",
            sub_geography: None,
        },
    ),
    (
        "us-west-1",
        RegionDisplay {
            long_name: "US West (N. California)",
            geography: "United States of America",
            sub_geography: Some("California"),
        },
    ),
    (
        "us-west-2",
        RegionDisplay {
            long_name: "US West (Oregon)",
            geography: "United States of America",
            sub_geography: Some("Oregon"),
        },
    ),
];

/// What AWS publishes about where `region` is, or `None` for a Region it publishes no
/// display data for.
pub fn region_display(region: &str) -> Option<RegionDisplay> {
    REGION_DISPLAY
        .iter()
        .find(|(name, _)| *name == region)
        .map(|(_, display)| *display)
}

/// The long name of the zone group `group_name` of `region` — the Region's display name
/// followed by the group's own number — or `None` when the Region publishes no display
/// name or `group_name` carries no number.
///
/// Derived from the group's NAME rather than counted again here, so the two members a zone
/// reports for its group (`us-west-2-zg-1` and `US West (Oregon) 1`) cannot come to name
/// two different groups.
pub fn zone_group_long_name(region: &str, group_name: &str) -> Option<String> {
    let display = region_display(region)?;
    let number = group_name.rsplit('-').next()?;
    if number.is_empty() || !number.chars().all(|digit| digit.is_ascii_digit()) {
        return None;
    }
    Some(format!("{} {number}", display.long_name))
}
