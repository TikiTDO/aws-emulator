// SPDX-License-Identifier: CC0-1.0

//! The map styles a map resource can be created with, and the data provider each
//! one is sourced from.
//!
//! `MapConfiguration$Style` is a plain string in the model (`len 1..100`, pattern
//! `^[-._\w]+$`), so the request boundary accepts any word-shaped value and WHICH
//! styles exist is this service's own check — the same split as the route
//! calculator's data provider check beside it in [`super`].
//!
//! The catalog below is the style list the `MapConfiguration$Style` member doc
//! publishes, grouped there under one heading per data provider
//! (`smithy_query(query="doc",shape="MapConfiguration",member="Style")`). A map
//! read reports the provider of the style the map was created with
//! (`DescribeMapResponse$DataSource`: "Specifies the data provider for the
//! associated map tiles"), so the catalog carries the provider beside each style
//! and is the one place either is written down.

/// The Esri data provider, spelled as this API spells a provider name — the same
/// spelling the `CreateRouteCalculatorRequest$DataSource` member doc lists.
pub const PROVIDER_ESRI: &str = "Esri";

/// The HERE Technologies data provider, spelled as that same member doc lists it.
pub const PROVIDER_HERE: &str = "Here";

/// The GrabMaps data provider, spelled as that same member doc lists it.
pub const PROVIDER_GRAB: &str = "Grab";

/// The Open Data provider, which the `MapConfiguration$Style` doc heads its last
/// style group with ("Valid Open Data map styles"). The routes API offers no open
/// data provider, so this is the only place the name appears.
pub const PROVIDER_OPEN_DATA: &str = "Open Data";

/// One catalog entry: a style a map may be created with, and the data provider the
/// style's tiles come from.
#[derive(Debug, Clone, Copy)]
pub struct MapStyle {
    /// The style, spelled exactly as a request must spell it — the member doc notes
    /// each style by its own name and nothing normalises it.
    pub style: &'static str,
    /// The provider a map carrying this style reports as its `DataSource`.
    pub data_source: &'static str,
}

/// Every style the `MapConfiguration$Style` doc publishes, in the order that doc
/// lists them: the Esri group, the HERE Technologies group, the GrabMaps group,
/// then the Open Data group.
///
/// `VectorHereBerlin` is in the HERE group because the doc keeps it there: "The
/// `VectorHereContrast` style has been renamed from `VectorHereBerlin`.
/// `VectorHereBerlin` has been deprecated, but will continue to work in
/// applications that use it."
pub const MAP_STYLES: [MapStyle; 18] = [
    MapStyle {
        style: "VectorEsriDarkGrayCanvas",
        data_source: PROVIDER_ESRI,
    },
    MapStyle {
        style: "RasterEsriImagery",
        data_source: PROVIDER_ESRI,
    },
    MapStyle {
        style: "VectorEsriLightGrayCanvas",
        data_source: PROVIDER_ESRI,
    },
    MapStyle {
        style: "VectorEsriTopographic",
        data_source: PROVIDER_ESRI,
    },
    MapStyle {
        style: "VectorEsriStreets",
        data_source: PROVIDER_ESRI,
    },
    MapStyle {
        style: "VectorEsriNavigation",
        data_source: PROVIDER_ESRI,
    },
    MapStyle {
        style: "VectorHereContrast",
        data_source: PROVIDER_HERE,
    },
    MapStyle {
        style: "VectorHereBerlin",
        data_source: PROVIDER_HERE,
    },
    MapStyle {
        style: "VectorHereExplore",
        data_source: PROVIDER_HERE,
    },
    MapStyle {
        style: "VectorHereExploreTruck",
        data_source: PROVIDER_HERE,
    },
    MapStyle {
        style: "RasterHereExploreSatellite",
        data_source: PROVIDER_HERE,
    },
    MapStyle {
        style: "HybridHereExploreSatellite",
        data_source: PROVIDER_HERE,
    },
    MapStyle {
        style: "VectorGrabStandardLight",
        data_source: PROVIDER_GRAB,
    },
    MapStyle {
        style: "VectorGrabStandardDark",
        data_source: PROVIDER_GRAB,
    },
    MapStyle {
        style: "VectorOpenDataStandardLight",
        data_source: PROVIDER_OPEN_DATA,
    },
    MapStyle {
        style: "VectorOpenDataStandardDark",
        data_source: PROVIDER_OPEN_DATA,
    },
    MapStyle {
        style: "VectorOpenDataVisualizationLight",
        data_source: PROVIDER_OPEN_DATA,
    },
    MapStyle {
        style: "VectorOpenDataVisualizationDark",
        data_source: PROVIDER_OPEN_DATA,
    },
];

/// The member a refusal about the requested style names: the `MapConfiguration`
/// member that carries it, as the model spells it.
pub const MAP_STYLE_MEMBER: &str = "Style";

/// The catalog entry for `style`, matched exactly as the doc spells it, or `None`
/// when no published style is spelled that way.
pub fn map_style(style: &str) -> Option<MapStyle> {
    MAP_STYLES.into_iter().find(|entry| entry.style == style)
}

/// The data provider a map carrying `style` sources its tiles from, or the sentence
/// a style outside the published catalog is refused with.
///
/// The sentence is the one [`super::documented_value_set_refusal`] words for every
/// member this
/// service constrains to a documented value set, and its `fieldList` entry repeats
/// it. It names the whole published set, so a caller reads which styles exist out of
/// the refusal itself, and never the value the request sent.
pub fn map_data_source(style: &str) -> Result<&'static str, String> {
    match map_style(style) {
        Some(entry) => Ok(entry.data_source),
        None => Err(super::documented_value_set_refusal(
            MAP_STYLE_MEMBER,
            MAP_STYLES.iter().map(|entry| entry.style),
        )),
    }
}
