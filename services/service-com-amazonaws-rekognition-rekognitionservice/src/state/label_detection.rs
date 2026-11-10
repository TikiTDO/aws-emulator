// SPDX-License-Identifier: CC0-1.0

//! What a submitted image CARRIES, as `DetectLabels` reports it: the labels of the
//! real-world entities measured in its pixels, and the measured properties of the pixels
//! themselves.
//!
//! `DetectLabels` "detects instances of real-world entities within an image", and for each
//! one returns its name, a confidence, its ancestors in the label taxonomy and, "for all
//! instances of detected common object labels", a bounding box. So, exactly as for the face
//! side of this service ([`super::face_detection`]), an answer here is a MEASUREMENT of the
//! submitted pixels or it is nothing: no label is keyed to a file name, a digest, a
//! recorded answer or a random draw, and a member no step below measured is left ABSENT
//! rather than filled in.
//!
//! ## Which entities this process can measure
//!
//! Recognising an arbitrary entity (a flower, a wedding, a landscape) needs a trained
//! classifier over a label taxonomy, and no such classifier exists in this process. What
//! does exist is the geometric face detector in [`super::face_detection`], which measures
//! the faces an image carries off its own pixels. So the entity vocabulary this detector
//! reports is the one its measurement supports:
//!
//! * [`FACE_LABEL`] — one per face the detector measured, carrying an `Instance` with the
//!   box it measured for that face and the agreement score it measured for it.
//! * [`PERSON_LABEL`] — the ancestor of that label. The operation documents that
//!   "DetectLabels returns a hierarchical taxonomy of detected labels … The response
//!   includes the all ancestors for a label, where every ancestor is a unique label", so a
//!   measured face is reported together with the entity it is part of. It carries NO
//!   `Instance`: where the person extends in the image is not something the face detector
//!   measured, and a box the size of the face would not be that person's box.
//! * A COLOUR the frame carries, which is the one other thing about an image's content that
//!   is measured rather than classified. Its name is the simplified colour name this
//!   model's own `DominantColor.SimplifiedColor` member is defined over — "one of 12
//!   simplified color names", [`SIMPLIFIED_COLORS`] — capitalised the way a label name is.
//!   Every pixel is classified into one of those twelve families, and a family covering at
//!   least [`MIN_COLOR_SHARE`] of the frame is reported, with the confidence
//!   [`color_confidence`] gives for how much of the frame it covers. A colour is not one of
//!   the "common object labels" a box is returned for, so no `Instance` locates one.
//! * [`MONOCHROME_LABEL`] — a frame essentially ONE colour: a composition the pixels
//!   measure directly, reported when a single colour family covers at least
//!   [`MONOCHROME_SHARE`] of them, with the share it covers as its confidence.
//!
//! An image carrying no measured entity and no measurable colour is answered with an EMPTY
//! label list — the answer for an image in which nothing was found — and never with a
//! refusal: a refusal here is reserved for an image that could not be read at all. Every
//! label's `Categories` and `Aliases` stay absent: both come from the taxonomy a trained
//! model is published with, which this process does not hold, and a category or an alias
//! invented for a label would be a value nothing established.
//!
//! ## What the pixels themselves measure
//!
//! The `IMAGE_PROPERTIES` feature asks for "information regarding image color and quality",
//! and every part of that IS a measurement of the decoded frame:
//!
//! * `Quality.Brightness` — the mean intensity, as a percentage of the brightest a pixel
//!   can be.
//! * `Quality.Contrast` — the standard deviation of the intensity, as a percentage of the
//!   largest deviation a pixel population can have ([`MAX_DEVIATION`]).
//! * `Quality.Sharpness` — the root-mean-square gradient magnitude, as a percentage of the
//!   steepest a gradient can be: the same measure, on the same scale, that a reported face's
//!   own `Quality.Sharpness` carries, taken here over every pixel of the region with a
//!   neighbour on each side rather than over the bounded grid a face's own box is sampled on.
//! * `DominantColors` — the colours the frame actually holds: the pixels are grouped into
//!   buckets of [`COLOR_BUCKET_LEVELS`] levels per channel, the largest buckets are taken,
//!   and each is reported as the MEAN colour of its own pixels, the share of the image they
//!   cover and the simplified colour name its measured channels fall under
//!   ([`SIMPLIFIED_COLORS`]). `CSSColor` names a colour from the much larger published CSS
//!   catalog, which this emulator does not hold, so that one member stays absent rather
//!   than carrying a name nothing looked up.
//! * `Foreground` / `Background` — the frame split in two by the intensity threshold that
//!   best separates its pixels into two classes (N. Otsu, 1979: the threshold maximising the
//!   between-class variance of the intensity histogram), with the class holding most of the
//!   image's outermost ring of pixels taken as the background. Each side then reports the
//!   same measurements, over its own pixels. An image whose pixels admit no such separation
//!   — a flat frame, a single intensity — has no two sides to report, so both members stay
//!   absent for it.
//!
//! Nothing here persists anything: `DetectLabels` is a function of the submitted image, so
//! every value above is measured on the call that reports it.

use ::image::DynamicImage;

use super::face_detection;
use super::image_source::{self, UnreadableImage};
use crate::ops::types::{
    BoundingBox, DetectLabelsImageBackground, DetectLabelsImageForeground,
    DetectLabelsImageProperties, DetectLabelsImageQuality, DominantColor, FaceDetail,
    GeneralLabelsSettings, Image, Instance, Label, Parent,
};

/// The confidence floor `DetectLabels` narrows to when the request names none: "if
/// MinConfidence is not specified, the operation returns labels with a confidence values
/// greater than or equal to 55 percent" (the member's own documentation).
pub const DEFAULT_MIN_CONFIDENCE: f32 = 55.0;

/// The most labels one answer carries: "the default and upper limit is 1000 labels" (the
/// operation's own documentation), so it is both the cap a request cannot exceed and the cap
/// a request naming no `MaxLabels` gets.
pub const MAX_LABELS: usize = 1_000;

/// The most dominant colours one answer carries when the request's
/// `Settings.ImageProperties` names none: "the default value is 10"
/// (`smithy_query(query="shape",shape="DetectLabelsImagePropertiesSettings")`, whose
/// `MaxDominantColors` member also carries `default: 0`, i.e. unset).
pub const DEFAULT_MAX_DOMINANT_COLORS: usize = 10;

/// The two analyses `Features` selects between
/// (`smithy_query(query="shape",shape="DetectLabelsFeatureName")`).
pub const GENERAL_LABELS_FEATURE: &str = "GENERAL_LABELS";
pub const IMAGE_PROPERTIES_FEATURE: &str = "IMAGE_PROPERTIES";

/// The entity label a measured face is reported as, and its one ancestor in the taxonomy.
pub const FACE_LABEL: &str = "Face";
pub const PERSON_LABEL: &str = "Person";

/// The label a frame of essentially one colour carries.
pub const MONOCHROME_LABEL: &str = "Monochrome";

/// How much of a frame one colour family must cover for the frame to be of that one colour.
pub const MONOCHROME_SHARE: f64 = 0.95;

/// The version of the label detection model that produced an answer, which `DetectLabels`
/// reports in `LabelModelVersion`. It is an opaque string on the wire; this detector is one
/// generation, so one value is pinned for it.
pub const LABEL_MODEL_VERSION: &str = "3.0";

/// How coarsely a colour is bucketed before the largest buckets are taken as the image's
/// dominant colours: this many levels per channel, so two pixels of nearly the same colour
/// count towards one dominant colour rather than two. The colour REPORTED for a bucket is
/// the mean of its own pixels, not the bucket's midpoint, so the quantisation groups the
/// pixels without rounding the answer.
pub const COLOR_BUCKET_LEVELS: u32 = 8;

/// The largest standard deviation an 8-bit intensity population can have — half the scale,
/// reached when the pixels sit entirely at the two ends — which is what `Quality.Contrast`
/// is expressed as a percentage of.
pub const MAX_DEVIATION: f64 = 127.5;

/// Bits dropped from each channel to reach [`COLOR_BUCKET_LEVELS`] levels.
const COLOR_BUCKET_SHIFT: u32 = 8 - COLOR_BUCKET_LEVELS.trailing_zeros();

/// The number of colour buckets: [`COLOR_BUCKET_LEVELS`] cubed.
const COLOR_BUCKETS: usize =
    (COLOR_BUCKET_LEVELS * COLOR_BUCKET_LEVELS * COLOR_BUCKET_LEVELS) as usize;

/// The twelve simplified colour names a measured colour is reported under: the vocabulary
/// `DominantColor.SimplifiedColor` is defined over — "one of 12 simplified color names"
/// (`smithy_query(query="doc",shape="DominantColor",member="SimplifiedColor")`) — which is
/// also the vocabulary the colour LABELS of an image are named from. Three of them are the
/// achromatic tones, told apart by lightness; the other nine are the chromatic families,
/// told apart by hue, with the two names that share a hue with another family (a dark warm
/// colour is brown, a pale red is pink) distinguished by lightness as well.
///
/// The index of a name in this list is the family [`color_family`] classifies a colour into.
pub const SIMPLIFIED_COLORS: [&str; 12] = [
    "black", "grey", "white", "red", "orange", "yellow", "green", "cyan", "blue", "purple",
    "pink", "brown",
];

/// Each colour family's own index into [`SIMPLIFIED_COLORS`], which is what
/// [`color_family`] answers with.
const BLACK: usize = 0;
const GREY: usize = 1;
const WHITE: usize = 2;
const RED: usize = 3;
const ORANGE: usize = 4;
const YELLOW: usize = 5;
const GREEN: usize = 6;
const CYAN: usize = 7;
const BLUE: usize = 8;
const PURPLE: usize = 9;
const PINK: usize = 10;
const BROWN: usize = 11;

/// How much of a frame a colour family must cover for the frame to be reported as carrying
/// that colour: a colour a handful of pixels hold is a colour IN the image, not a label OF
/// it.
pub const MIN_COLOR_SHARE: f64 = 0.01;

/// How much of a frame a colour family covers to be reported with full confidence. Below it
/// the confidence falls off linearly to [`MIN_COLOR_CONFIDENCE`] at [`MIN_COLOR_SHARE`].
/// The scale is this one, not a scale AWS publishes, because none is published.
pub const FULL_COLOR_SHARE: f64 = 0.05;

/// The confidence a colour covering none of the frame would carry, which is the floor the
/// fall-off above runs down to.
pub const MIN_COLOR_CONFIDENCE: f64 = 50.0;

/// How little chroma a pixel may carry and still be one of the achromatic tones, on the
/// 0..1 scale `max(R,G,B) - min(R,G,B)` is measured on.
const ACHROMATIC_CHROMA: f64 = 0.1;

/// Where the achromatic tones divide: below the first a pixel is black, above the second it
/// is white, between them it is grey.
const BLACK_LIGHTNESS: f64 = 0.2;
const WHITE_LIGHTNESS: f64 = 0.8;

/// Where a warm hue becomes brown rather than red or orange, and where a red becomes pink,
/// on the same lightness scale.
const BROWN_LIGHTNESS: f64 = 0.35;
const PINK_LIGHTNESS: f64 = 0.75;

/// Which analyses one request asked for, and how much of the colour answer it asked for.
#[derive(Debug, Clone, Copy)]
pub struct Requested {
    /// `GENERAL_LABELS`: the labels of the entities measured in the image.
    pub general_labels: bool,
    /// `IMAGE_PROPERTIES`: the measured quality and colours of the pixels.
    pub image_properties: bool,
    /// `Settings.ImageProperties.MaxDominantColors`, defaulted.
    pub max_dominant_colors: usize,
}

impl Default for Requested {
    /// What a request naming no `Features` asked for: "if no option is specified
    /// GENERAL_LABELS is used by default".
    fn default() -> Self {
        Self {
            general_labels: true,
            image_properties: false,
            max_dominant_colors: DEFAULT_MAX_DOMINANT_COLORS,
        }
    }
}

/// Everything measured off one submitted image, before the request's own narrowing.
#[derive(Debug, Clone, Default)]
pub struct ImageAnalysis {
    /// The labels of the entities measured in the image, in detection order.
    pub labels: Vec<Label>,
    /// The measured properties of the pixels, present only when they were asked for.
    pub image_properties: Option<DetectLabelsImageProperties>,
}

/// What the image a request submitted carries — its own inline `Bytes`, or the object this
/// emulator's Amazon S3 holds at the location it named — measured OFF the async worker.
pub async fn analysed_submitted_image(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    image: &Image,
    requested: Requested,
) -> Result<ImageAnalysis, UnreadableImage> {
    let bytes = image_source::image_bytes(ctx, image).await?;
    match ::tokio::task::spawn_blocking(move || analysed_image(&bytes, requested)).await {
        Ok(analysis) => analysis,
        Err(error) => {
            ::tracing::error!(error = %error, "analysing the labels of an image did not complete");
            Err(UnreadableImage::NotAnalysed(
                "Detecting the labels in the image did not complete".to_string(),
            ))
        }
    }
}

/// What one image's bytes carry.
///
/// CPU-bound: a request path reaches it through [`analysed_submitted_image`], which runs it
/// on the blocking pool.
pub fn analysed_image(
    bytes: &[u8],
    requested: Requested,
) -> Result<ImageAnalysis, UnreadableImage> {
    // One decode for every measurement taken off the frame, and the one place an image this
    // service cannot read is refused.
    let decoded = face_detection::decoded_image(bytes)?;
    let plane = Plane::of(&decoded);
    let labels = if requested.general_labels {
        let mut labels = detected_labels(
            &face_detection::faces_in_decoded_image(&decoded),
            requested
                .image_properties
                .then_some((&plane, requested.max_dominant_colors)),
        );
        // The entities measured come first and the colours of the frame after them: a label
        // of what the picture HOLDS before a label of what it LOOKS like, which is the order
        // two equally confident labels are then reported in.
        labels.extend(color_labels(&plane));
        labels
    } else {
        Vec::new()
    };
    let image_properties = requested
        .image_properties
        .then(|| measured_properties(&plane, requested.max_dominant_colors));
    Ok(ImageAnalysis {
        labels,
        image_properties,
    })
}

/// The labels a request asked for, narrowed the way it asked for them: the filters its
/// `Settings.GeneralLabels` names, then the confidence floor, then the highest-confidence
/// `MaxLabels` of what is left.
///
/// "The service returns the specified number of highest confidence labels", so the cap is
/// applied to labels ordered by descending confidence; labels of equal confidence keep
/// detection order, which puts a measured label before the ancestor it brought with it.
pub fn narrowed(
    labels: Vec<Label>,
    settings: Option<&GeneralLabelsSettings>,
    min_confidence: Option<f32>,
    max_labels: Option<i32>,
) -> Vec<Label> {
    let floor = min_confidence.unwrap_or(DEFAULT_MIN_CONFIDENCE);
    let mut kept: Vec<Label> = labels
        .into_iter()
        .filter(|label| label.confidence.unwrap_or_default() >= floor)
        .filter(|label| admitted(label, settings))
        .collect();
    // A stable sort, so equal confidences stay in detection order.
    kept.sort_by(|left, right| {
        right
            .confidence
            .unwrap_or_default()
            .total_cmp(&left.confidence.unwrap_or_default())
    });
    let most = max_labels
        .and_then(|requested| usize::try_from(requested).ok())
        .unwrap_or(MAX_LABELS)
        .min(MAX_LABELS);
    kept.truncate(most);
    kept
}

/// Whether the request's own filters admit this label.
///
/// "The exact label names or label categories must be supplied", so a filter is compared
/// against the label's own name and categories as supplied. An inclusion filter list that
/// names anything narrows the answer to what it names; an exclusion filter list removes what
/// it names. No label this detector reports carries a category (see this module's docs), so
/// a category inclusion filter admits none of them and a category exclusion filter removes
/// none of them.
fn admitted(label: &Label, settings: Option<&GeneralLabelsSettings>) -> bool {
    let Some(settings) = settings else {
        return true;
    };
    let name = label.name.as_deref().unwrap_or_default();
    let categories: Vec<&str> = label
        .categories
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter_map(|category| category.name.as_deref())
        .collect();
    let names = [name];
    !excluded(&settings.label_inclusion_filters, &names, true)
        && !excluded(
            &settings.label_category_inclusion_filters,
            &categories,
            true,
        )
        && !excluded(&settings.label_exclusion_filters, &names, false)
        && !excluded(
            &settings.label_category_exclusion_filters,
            &categories,
            false,
        )
}

/// Whether `filters` removes a label whose own values are `values`.
///
/// An INCLUSION list (`inclusive`) removes a label none of whose values it names, and an
/// empty or absent list removes nothing. An EXCLUSION list removes a label one of whose
/// values it names.
fn excluded(filters: &Option<Vec<String>>, values: &[&str], inclusive: bool) -> bool {
    let filters = filters.as_deref().unwrap_or_default();
    if filters.is_empty() {
        return false;
    }
    let named = filters
        .iter()
        .any(|filter| values.contains(&filter.as_str()));
    named != inclusive
}

/// The labels of the entities measured in one frame: one [`FACE_LABEL`] carrying an instance
/// per measured face, and the [`PERSON_LABEL`] ancestor it brings with it. An empty list
/// when nothing was measured.
///
/// `colors`, when the request asked for image properties, is the frame and the colour cap
/// each instance's own `DominantColors` are measured over.
fn detected_labels(faces: &[FaceDetail], colors: Option<(&Plane, usize)>) -> Vec<Label> {
    if faces.is_empty() {
        return Vec::new();
    }
    let instances: Vec<Instance> = faces
        .iter()
        .map(|face| Instance {
            bounding_box: face.bounding_box.clone(),
            confidence: face.confidence,
            dominant_colors: colors.and_then(|(plane, most)| {
                face.bounding_box.as_ref().map(|bounds| {
                    dominant_colors(plane, &Region::within(plane, bounds), most)
                })
            }),
        })
        .collect();
    // The confidence of a label is the confidence of the entity found: the strongest reading
    // behind it, which is what the detector measured for the clearest of these faces.
    let confidence = faces
        .iter()
        .filter_map(|face| face.confidence)
        .fold(0.0_f32, f32::max);
    vec![
        Label {
            name: Some(FACE_LABEL.to_string()),
            confidence: Some(confidence),
            instances: Some(instances),
            parents: Some(vec![Parent {
                name: Some(PERSON_LABEL.to_string()),
            }]),
            // A category or an alias comes from a published taxonomy this process does not
            // hold: see this module's docs.
            ..Label::default()
        },
        Label {
            name: Some(PERSON_LABEL.to_string()),
            confidence: Some(confidence),
            // Where the person extends is not what the face detector measured, so this
            // label locates no instance; its ancestors are none, being the root of the two.
            instances: None,
            parents: Some(Vec::new()),
            ..Label::default()
        },
    ]
}

/// The colours one frame carries, as labels of it: one per simplified colour family
/// covering at least [`MIN_COLOR_SHARE`] of the pixels, most covered first, and the
/// composition they add up to where the frame is essentially one colour.
fn color_labels(plane: &Plane) -> Vec<Label> {
    let pixels = plane.pixels();
    if pixels == 0 {
        return Vec::new();
    }
    let mut covered = [0_u64; SIMPLIFIED_COLORS.len()];
    for index in 0..pixels {
        let (red, green, blue) = plane.rgb_at(index);
        covered[color_family(red, green, blue)] += 1;
    }
    let mut families: Vec<(usize, f64)> = covered
        .iter()
        .enumerate()
        .map(|(family, count)| (family, *count as f64 / pixels as f64))
        .filter(|(_, share)| *share >= MIN_COLOR_SHARE)
        .collect();
    // Most covered first; two families covering equally much are ordered by the family, so
    // one frame's answer is one answer.
    families.sort_by(|left, right| right.1.total_cmp(&left.1).then(left.0.cmp(&right.0)));
    let monochrome = families
        .first()
        .map(|(_, share)| *share)
        .filter(|share| *share >= MONOCHROME_SHARE);
    let mut labels: Vec<Label> = families
        .into_iter()
        .map(|(family, share)| Label {
            name: Some(color_label_name(family)),
            confidence: Some(color_confidence(share)),
            // A colour is not one of the "common object labels" a bounding box is returned
            // for, and it has no ancestor in the colour vocabulary.
            instances: None,
            parents: Some(Vec::new()),
            ..Label::default()
        })
        .collect();
    // What the frame LOOKS like as a whole comes after the colours it is made of.
    if let Some(share) = monochrome {
        labels.push(Label {
            name: Some(MONOCHROME_LABEL.to_string()),
            confidence: Some((share * 100.0).clamp(0.0, 100.0) as f32),
            instances: None,
            parents: Some(Vec::new()),
            ..Label::default()
        });
    }
    labels
}

/// The label name of a colour family: its simplified colour name, capitalised the way a
/// label name is.
fn color_label_name(family: usize) -> String {
    let name = SIMPLIFIED_COLORS[family];
    let mut capitalised = name.to_string();
    capitalised[..1].make_ascii_uppercase();
    capitalised
}

/// How confident a colour label covering `share` of the frame is: full confidence at
/// [`FULL_COLOR_SHARE`] of the pixels and down to [`MIN_COLOR_CONFIDENCE`] as the share
/// goes to nothing.
pub fn color_confidence(share: f64) -> f32 {
    let measured =
        MIN_COLOR_CONFIDENCE + (100.0 - MIN_COLOR_CONFIDENCE) * (share / FULL_COLOR_SHARE).min(1.0);
    measured.clamp(0.0, 100.0) as f32
}

/// The simplified colour name a measured colour falls under: one of [`SIMPLIFIED_COLORS`].
pub fn simplified_color(red: u8, green: u8, blue: u8) -> &'static str {
    SIMPLIFIED_COLORS[color_family(red, green, blue)]
}

/// Which of [`SIMPLIFIED_COLORS`] a colour belongs to, by its lightness, its chroma and its
/// hue — the three measurements that tell one colour name from another. The answer is the
/// INDEX of the name, so classifying a frame's every pixel costs no name lookup.
fn color_family(red: u8, green: u8, blue: u8) -> usize {
    let (red, green, blue) = (
        f64::from(red) / 255.0,
        f64::from(green) / 255.0,
        f64::from(blue) / 255.0,
    );
    let high = red.max(green).max(blue);
    let low = red.min(green).min(blue);
    let chroma = high - low;
    let lightness = (high + low) / 2.0;
    if chroma < ACHROMATIC_CHROMA {
        return if lightness < BLACK_LIGHTNESS {
            BLACK
        } else if lightness > WHITE_LIGHTNESS {
            WHITE
        } else {
            GREY
        };
    }
    // The hue, in degrees around the colour wheel, as it is defined for RGB.
    let hue = if high == red {
        60.0 * (((green - blue) / chroma) % 6.0)
    } else if high == green {
        60.0 * ((blue - red) / chroma + 2.0)
    } else {
        60.0 * ((red - green) / chroma + 4.0)
    };
    let hue = (hue + 360.0) % 360.0;
    match hue {
        // A dark warm colour is brown rather than the red or orange it shares its hue with,
        // and a pale red is pink rather than red.
        hue if hue < 45.0 || hue >= 345.0 => {
            if lightness < BROWN_LIGHTNESS {
                BROWN
            } else if lightness > PINK_LIGHTNESS {
                PINK
            } else if hue < 15.0 || hue >= 345.0 {
                RED
            } else {
                ORANGE
            }
        }
        hue if hue < 70.0 => {
            if lightness < BROWN_LIGHTNESS {
                BROWN
            } else {
                YELLOW
            }
        }
        hue if hue < 170.0 => GREEN,
        hue if hue < 200.0 => CYAN,
        hue if hue < 260.0 => BLUE,
        hue if hue < 290.0 => PURPLE,
        _ => PINK,
    }
}

/// The measured properties of one frame: the whole image's quality and dominant colours, and
/// the same two over each side of the foreground/background split when the pixels admit one.
fn measured_properties(plane: &Plane, most_colors: usize) -> DetectLabelsImageProperties {
    let split = plane.split();
    DetectLabelsImageProperties {
        quality: Some(quality(plane, &Region::whole(plane))),
        dominant_colors: Some(dominant_colors(plane, &Region::whole(plane), most_colors)),
        foreground: split.as_ref().map(|split| {
            let side = Region::pixels(plane, &split.foreground);
            DetectLabelsImageForeground {
                quality: Some(quality(plane, &side)),
                dominant_colors: Some(dominant_colors(plane, &side, most_colors)),
            }
        }),
        background: split.as_ref().map(|split| {
            let side = Region::pixels(plane, &split.background);
            DetectLabelsImageBackground {
                quality: Some(quality(plane, &side)),
                dominant_colors: Some(dominant_colors(plane, &side, most_colors)),
            }
        }),
    }
}

/// The quality of a region's pixels: their brightness, their sharpness and their contrast,
/// each measured and each expressed as a percentage of its own full scale. A region holding
/// no pixel measures nothing, so every member of it stays absent.
fn quality(plane: &Plane, region: &Region) -> DetectLabelsImageQuality {
    let mut count = 0_u64;
    let mut total = 0.0_f64;
    let mut total_squared = 0.0_f64;
    for index in region.indices(plane) {
        let value = plane.luma_at(index);
        count += 1;
        total += value;
        total_squared += value * value;
    }
    if count == 0 {
        return DetectLabelsImageQuality::default();
    }
    let mean = total / count as f64;
    // The population variance, floored at zero: the two sums are accumulated in order, so
    // rounding can leave the difference a hair below zero for a flat region.
    let variance = (total_squared / count as f64 - mean * mean).max(0.0);
    DetectLabelsImageQuality {
        brightness: Some(percent_of(mean, 255.0)),
        sharpness: Some(sharpness(plane, region)),
        contrast: Some(percent_of(variance.sqrt(), MAX_DEVIATION)),
    }
}

/// The root-mean-square gradient magnitude across a region's pixels, as a percentage of
/// the steepest a gradient can be: how much the pixels change from one to the next, which is
/// what separates a crisp frame from a soft one. The same measure, on the same scale, a
/// face's own `Quality.Sharpness` carries, read over every pixel of the region that has a
/// neighbour on each side.
fn sharpness(plane: &Plane, region: &Region) -> f32 {
    let mut total = 0.0_f64;
    let mut count = 0_u64;
    // Central differences need a pixel on either side, so the frame's own border is not a
    // place a gradient can be measured.
    for index in region.inner_indices(plane) {
        // Halved, because a central difference spans two pixels.
        let horizontal = (plane.luma_at(index + 1) - plane.luma_at(index - 1)) / 2.0;
        let vertical =
            (plane.luma_at(index + plane.width) - plane.luma_at(index - plane.width)) / 2.0;
        total += horizontal * horizontal + vertical * vertical;
        count += 1;
    }
    if count == 0 {
        return 0.0;
    }
    percent_of((total / count as f64).sqrt(), 255.0)
}

/// The dominant colours of a region's pixels: the largest colour buckets, each reported as
/// the mean colour of its own pixels and the share of the region they cover, most covered
/// first. At most `most` of them, and none at all for a region holding no pixel.
fn dominant_colors(plane: &Plane, region: &Region, most: usize) -> Vec<DominantColor> {
    let mut buckets = vec![ColorBucket::default(); COLOR_BUCKETS];
    let mut total = 0_u64;
    for index in region.indices(plane) {
        let (red, green, blue) = plane.rgb_at(index);
        buckets[bucket_of(red, green, blue)].add(red, green, blue);
        total += 1;
    }
    if total == 0 || most == 0 {
        return Vec::new();
    }
    let mut order: Vec<usize> = (0..COLOR_BUCKETS)
        .filter(|bucket| buckets[*bucket].count > 0)
        .collect();
    // Most pixels first; two buckets covering equally many pixels are ordered by the bucket
    // itself, so one image's answer is one answer.
    order.sort_by(|left, right| {
        buckets[*right]
            .count
            .cmp(&buckets[*left].count)
            .then(left.cmp(right))
    });
    order.truncate(most);
    order
        .into_iter()
        .map(|bucket| buckets[bucket].reported(total))
        .collect()
}

/// The colour bucket `(red, green, blue)` falls in.
fn bucket_of(red: u8, green: u8, blue: u8) -> usize {
    let level = |channel: u8| usize::from(channel >> COLOR_BUCKET_SHIFT);
    let levels = COLOR_BUCKET_LEVELS as usize;
    (level(red) * levels + level(green)) * levels + level(blue)
}

/// Whether the measurement reads the pixel at `index`: every pixel of the region's box when
/// it selects none of them in particular, otherwise the ones it selects.
fn selected(selection: Option<&[bool]>, index: usize) -> bool {
    selection.is_none_or(|selection| selection[index])
}

/// `value` as a percentage of `full`, clamped to the 0..100 the reported members carry.
fn percent_of(value: f64, full: f64) -> f32 {
    ((value / full) * 100.0).clamp(0.0, 100.0) as f32
}

/// The pixels of a frame one measurement reads: a rectangle of it, and which of that
/// rectangle's pixels are the region's own.
///
/// A measurement costs the rectangle rather than the frame, so measuring the pixels of a
/// reported instance's box costs that box — not one scan of the whole image per instance.
struct Region<'a> {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    /// Which pixels of the frame the region holds, indexed like the frame's own planes;
    /// `None` means every pixel of the rectangle above.
    holds: Option<&'a [bool]>,
}

impl<'a> Region<'a> {
    /// Every pixel of the frame.
    fn whole(plane: &Plane) -> Self {
        Self {
            x0: 0,
            y0: 0,
            x1: plane.width,
            y1: plane.height,
            holds: None,
        }
    }

    /// The pixels `holds` selects, wherever in the frame they are.
    fn pixels(plane: &Plane, holds: &'a [bool]) -> Self {
        Self {
            holds: Some(holds),
            ..Self::whole(plane)
        }
    }

    /// The pixels a reported `BoundingBox` covers, which carries fractions of the image.
    fn within(plane: &Plane, bounds: &BoundingBox) -> Self {
        let span = |start: Option<f32>, length: Option<f32>, bound: usize| -> (usize, usize) {
            let start = start.unwrap_or(0.0).clamp(0.0, 1.0);
            let end = (start + length.unwrap_or(0.0).max(0.0)).clamp(0.0, 1.0);
            let scale = bound as f32;
            (
                (start * scale).floor().max(0.0) as usize,
                ((end * scale).ceil() as usize).min(bound),
            )
        };
        let (x0, x1) = span(bounds.left, bounds.width, plane.width);
        let (y0, y1) = span(bounds.top, bounds.height, plane.height);
        Self {
            x0,
            y0,
            x1,
            y1,
            holds: None,
        }
    }

    /// The index into the frame's planes of each pixel this region holds.
    fn indices<'plane>(
        &'plane self,
        plane: &'plane Plane,
    ) -> impl Iterator<Item = usize> + 'plane {
        self.spans(plane, self.y0, self.y1, self.x0, self.x1)
    }

    /// The same, without the frame's outermost row and column: where a central difference
    /// has a pixel on either side to read.
    fn inner_indices<'plane>(
        &'plane self,
        plane: &'plane Plane,
    ) -> impl Iterator<Item = usize> + 'plane {
        self.spans(
            plane,
            self.y0.max(1),
            self.y1.min(plane.height.saturating_sub(1)),
            self.x0.max(1),
            self.x1.min(plane.width.saturating_sub(1)),
        )
    }

    fn spans<'plane>(
        &'plane self,
        plane: &'plane Plane,
        y0: usize,
        y1: usize,
        x0: usize,
        x1: usize,
    ) -> impl Iterator<Item = usize> + 'plane {
        (y0..y1).flat_map(move |y| {
            (x0..x1)
                .map(move |x| y * plane.width + x)
                .filter(move |index| selected(self.holds, *index))
        })
    }
}

// ---------------------------------------------------------------------------
// The frame, and the two planes every measurement above reads
// ---------------------------------------------------------------------------

/// A decoded frame: its colour pixels, which the colour answer reads, and its intensity
/// pixels, which every quality measurement reads.
struct Plane {
    width: usize,
    height: usize,
    /// Three bytes per pixel, row-major.
    rgb: Vec<u8>,
    /// One byte per pixel, row-major: the decoder's own intensity conversion.
    luma: Vec<u8>,
}

impl Plane {
    fn of(decoded: &DynamicImage) -> Self {
        let rgb = decoded.to_rgb8();
        let (width, height) = (rgb.width() as usize, rgb.height() as usize);
        Self {
            width,
            height,
            rgb: rgb.into_raw(),
            luma: decoded.to_luma8().into_raw(),
        }
    }

    fn pixels(&self) -> usize {
        self.width * self.height
    }

    fn luma_at(&self, index: usize) -> f64 {
        f64::from(self.luma[index])
    }

    fn rgb_at(&self, index: usize) -> (u8, u8, u8) {
        (
            self.rgb[index * 3],
            self.rgb[index * 3 + 1],
            self.rgb[index * 3 + 2],
        )
    }

    /// This frame's pixels split in two by the intensity threshold that best separates them,
    /// or `None` when they admit no such separation.
    fn split(&self) -> Option<Split> {
        if self.pixels() == 0 {
            return None;
        }
        let mut histogram = [0_u64; 256];
        for value in &self.luma {
            histogram[usize::from(*value)] += 1;
        }
        let threshold = otsu_threshold(&histogram, self.pixels() as u64)?;
        // Which of the two classes is the BACKGROUND: the one holding most of the frame's
        // outermost ring of pixels, the part of a picture that surrounds its subject.
        let mut ring_above = 0_u64;
        let mut ring_below = 0_u64;
        for (index, value) in self.luma.iter().enumerate() {
            let (x, y) = (index % self.width, index / self.width);
            if x == 0 || y == 0 || x + 1 == self.width || y + 1 == self.height {
                if u32::from(*value) > threshold {
                    ring_above += 1;
                } else {
                    ring_below += 1;
                }
            }
        }
        let background_above = ring_above >= ring_below;
        let background: Vec<bool> = self
            .luma
            .iter()
            .map(|value| (u32::from(*value) > threshold) == background_above)
            .collect();
        let foreground: Vec<bool> = background.iter().map(|side| !side).collect();
        if !background.contains(&true) || !foreground.contains(&true) {
            return None;
        }
        Some(Split {
            foreground,
            background,
        })
    }
}

/// One frame's pixels, divided into the two sides reported separately. Each side selects the
/// pixels it holds.
struct Split {
    foreground: Vec<bool>,
    background: Vec<bool>,
}

/// The intensity threshold that best separates `histogram`'s pixels into the class at or
/// below it and the class above it: the one maximising the variance BETWEEN the two classes
/// (N. Otsu, 1979). `None` when no threshold separates anything, which is a frame whose
/// pixels all share one intensity.
fn otsu_threshold(histogram: &[u64; 256], total: u64) -> Option<u32> {
    let weighted = |value: usize, count: u64| value as f64 * count as f64;
    let whole: f64 = histogram
        .iter()
        .enumerate()
        .map(|(value, count)| weighted(value, *count))
        .sum();
    let mut below_count = 0.0_f64;
    let mut below_total = 0.0_f64;
    let mut best: Option<(f64, u32)> = None;
    for (value, count) in histogram.iter().enumerate() {
        below_count += *count as f64;
        below_total += weighted(value, *count);
        let above_count = total as f64 - below_count;
        if below_count == 0.0 || above_count == 0.0 {
            continue;
        }
        let gap = below_total / below_count - (whole - below_total) / above_count;
        // The between-class variance, scaled by the pixel count squared, which does not
        // move where the maximum is.
        let separation = below_count * above_count * gap * gap;
        if best.is_none_or(|(found, _)| separation > found) {
            best = Some((separation, value as u32));
        }
    }
    best.filter(|(separation, _)| *separation > 0.0)
        .map(|(_, threshold)| threshold)
}

/// One colour bucket: how many of the selected pixels fell in it, and the sum of their
/// channels, so the colour reported for it is the mean of its own pixels.
#[derive(Debug, Clone, Copy, Default)]
struct ColorBucket {
    count: u64,
    red: u64,
    green: u64,
    blue: u64,
}

impl ColorBucket {
    fn add(&mut self, red: u8, green: u8, blue: u8) {
        self.count += 1;
        self.red += u64::from(red);
        self.green += u64::from(green);
        self.blue += u64::from(blue);
    }

    /// This bucket as a reported dominant colour of a selection covering `total` pixels.
    fn reported(&self, total: u64) -> DominantColor {
        let mean = |channel: u64| -> i32 {
            // The count is non-zero for every reported bucket, and a mean of 8-bit
            // channels is an 8-bit value.
            i32::try_from(channel / self.count.max(1)).unwrap_or_default()
        };
        let (red, green, blue) = (mean(self.red), mean(self.green), mean(self.blue));
        let channel = |value: i32| u8::try_from(value.clamp(0, 255)).unwrap_or_default();
        DominantColor {
            red: Some(red),
            green: Some(green),
            blue: Some(blue),
            hex_code: Some(format!("#{red:02X}{green:02X}{blue:02X}")),
            // A colour's CSS name comes from a published catalog this process does not hold:
            // see this module's docs.
            css_color: None,
            simplified_color: Some(
                simplified_color(channel(red), channel(green), channel(blue)).to_string(),
            ),
            pixel_percent: Some(percent_of(self.count as f64, total as f64)),
        }
    }
}
