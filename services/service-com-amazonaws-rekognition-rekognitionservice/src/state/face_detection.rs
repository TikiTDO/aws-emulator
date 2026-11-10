// SPDX-License-Identifier: CC0-1.0

//! Finding the FACES in a submitted image, and measuring what one face looks like: the
//! pixel side of `DetectFaces` and of `CompareFaces`.
//!
//! `DetectFaces` "detects faces within an image that is provided as input" and, for each
//! face, returns "a bounding box of the face, a confidence value (that the bounding box
//! contains a face)" and facial landmarks, pose and quality. So an answer here is a
//! MEASUREMENT of the submitted pixels or it is nothing: nothing in this module is echoed
//! from the request, keyed to a digest, randomised or taken from a recorded answer, and a
//! member no step below measured is left ABSENT rather than filled in.
//!
//! ## The pipeline
//!
//! 1. **Decode** — the `image` crate decodes the submitted bytes for the format their own
//!    magic numbers name. `DetectFaces` documents that "the image must be either a PNG or
//!    JPEG formatted file", so anything else is refused as an invalid image format, and the
//!    decode is bounded by [`MAX_IMAGE_SIDE`], [`MAX_IMAGE_PIXELS`] and [`MAX_IMAGE_ALLOC`]
//!    so one request cannot turn a small compressed file into unbounded work.
//! 2. **Smooth** — the grey image is averaged over a 3x3 window (through an integral
//!    image, so every window and every region mean below costs four lookups: F. Crow, 1984,
//!    summed-area tables), which is what keeps a single noisy pixel from reading as a
//!    facial feature.
//! 3. **Features** — a pixel darker than the mean of the window around it by
//!    [`MIN_FEATURE_CONTRAST`] grey levels is a FEATURE pixel (adaptive thresholding
//!    against a local mean: D. Bradley and G. Roth, 2007). Eyes, nostrils, a mouth and
//!    eyebrows are dark against the skin around them whatever the skin's own tone or the
//!    image's exposure, which is why the threshold is local and the measure is intensity
//!    rather than colour: a grey-scale image and a colour one are read alike, and no skin
//!    tone is privileged by a chrominance range. The window is taken at each of
//!    [`FEATURE_SCALES`], because how large a face's features are in an image is exactly
//!    what is not known yet: a window wider than the feature finds it, one narrower finds
//!    only its edge, so the search is run at three widths and every reading is offered to
//!    the steps below.
//! 4. **Blobs** — the feature pixels are grouped into 8-connected components, each with its
//!    extent, its centroid, its area and its mean intensity. Components too small to be a
//!    feature or large enough to be the background are dropped, and the largest
//!    [`MAX_FEATURE_BLOBS`] of the rest are what the search considers.
//! 5. **Pairs** — every pair of blobs is a candidate EYE PAIR. A pair is kept when the two
//!    are alike in size and height, level with each other to within
//!    [`MAX_EYE_LEVEL_RATIO`] of their separation, and each narrower than the gap between
//!    them. The face the pair stands for is derived from standard facial proportions: a
//!    face about two interocular distances wide, about 2.9 tall, with the eye line just
//!    above the middle ([`FACE_WIDTH_IN_EYE_SPANS`], [`FACE_HEIGHT_IN_EYE_SPANS`],
//!    [`EYE_LINE_FROM_TOP`]). A face at the frame's edge, or one closer to the lens than
//!    the frame is wide, carries part of that box outside the image, which is allowed as
//!    long as [`MIN_INSIDE_FRACTION`] of it is inside: the box reported is the part that
//!    is.
//! 6. **Verify** — a pair is only a face when the rest of the structure is there too: the
//!    bridge between the eyes and the cheek band below them are brighter than the eyes
//!    themselves by [`MIN_FEATURE_CONTRAST`], both of those regions are SMOOTH compared
//!    with that contrast ([`MAX_SKIN_TEXTURE_RATIO`]) — skin is a smooth region, while a
//!    region whose intensity swings as much as the features do is texture or noise — the
//!    box stands out from the ring around it by [`MIN_REGION_CONTRAST`] (figure against
//!    ground, where there is a ring to measure), [`MIN_FACE_SKIN_FRACTION`] of the box is
//!    that same skin, so the box is mostly the face rather than mostly whatever surrounds
//!    it, the skin just OUTSIDE one of the two eyes stands above them too
//!    ([`MIN_TEMPLE_CONTRAST_RATIO`]) — the eyes of a face have face on both sides of them,
//!    while two dark blobs with only background outside them flank something that is not a
//!    face — and there is a wide dark feature where a mouth belongs. A pair with no mouth,
//!    no smooth skin between the eyes, no skin across its box or beside it, or no
//!    separation from its surroundings is reported as nothing at all.
//! 7. **Score** — the `Confidence` reported is the detector's own measured agreement: how
//!    alike the two eyes are, how level, how far the bridge stands above them in intensity
//!    and how near the mouth sits to where the proportions put it, averaged. It is the only
//!    confidence anything here measured, and a face below [`MIN_CONFIDENCE`] is not
//!    reported.
//! 8. **Suppress** — ONE face is reported ONCE. The readings are walked best first (the one
//!    whose box the image holds whole before one it clips, then the stronger-scoring one),
//!    and a reading of a region already accounted for is dropped: either it shares more
//!    than [`MAX_OVERLAP`] of what the two cover together, or it lies more than
//!    [`MAX_SHARED_FRACTION`] inside the region already kept — a face read at a second
//!    window width, or a wider pair of its own features, is the same face and not a second
//!    one around it. Both measures are taken on the regions as REPORTED, the part of each
//!    box the image holds, since that is what an answer claims. What is left is sorted by
//!    that region, largest first, and cut to [`MAX_FACES`] — "DetectFaces detects the 100
//!    largest faces in the image".
//! 9. **Measure** — each reported face carries only members this pipeline measured:
//!    `BoundingBox` (the derived box, as the fractions of the image a `BoundingBox` is
//!    expressed in), `Confidence` (step 7), `Landmarks` (the eye centroids and the mouth's
//!    own extent), `Pose.Roll` (the angle of the eye line) and `Quality` (the mean
//!    intensity and the mean gradient magnitude of the face's own pixels).
//!
//! ## What it does not report
//!
//! `Pose.Yaw`, `Pose.Pitch` and every classified attribute — `AgeRange`, `Gender`,
//! `Emotions`, `Smile`, `Beard`, `Mustache`, `Eyeglasses`, `Sunglasses`, `EyesOpen`,
//! `MouthOpen`, `FaceOccluded`, `EyeDirection` — are what a trained classifier reports, and
//! no classifier runs in this process: no maintained Rust crate classifies a face offline
//! (the ones that detect or recognise faces load neural-network weights or a trained
//! cascade from a file this emulator may not fetch), so those members are absent whatever
//! `Attributes` asks for, rather than carrying a value nothing measured.
//!
//! The detector is therefore a geometric one: it finds frontal faces whose eyes and mouth
//! are visible and distinguishable from the skin around them, and it finds nothing in an
//! image that carries no such structure — which is the honest answer for an image with no
//! face in it.
//!
//! ## Telling two detected faces apart
//!
//! `CompareFaces` needs one more measurement than a detection: how ALIKE the face in one
//! image and a face in another look. [`FaceAppearance`] is that measurement, and it is a
//! measurement of the pixels inside a detected face's own box and of nothing else — not of
//! the request, not of a digest of the bytes, and not of a recorded answer:
//!
//! 1. the box is divided into an [`APPEARANCE_GRID`] x [`APPEARANCE_GRID`] grid and each
//!    cell is reduced to the mean intensity of the pixels in it (through the same
//!    summed-area table every region mean reads), so the measurement is the face's own
//!    layout of light and dark at a resolution that survives a change of scale;
//! 2. the grid is centred on its own mean and scaled to unit length, which is what makes
//!    two pictures of one face taken at different exposures measure alike: a shift of
//!    brightness and a change of contrast are divided out;
//! 3. [`FaceAppearance::similarity_to`] is then the correlation of two such grids, on the
//!    0..100 scale a `Similarity` carries. The same pixels correlate perfectly and score
//!    100; two faces laid out differently correlate little and score near zero.
//!
//! This is a normalised template correlation, the oldest of the appearance-based face
//! comparisons (M. Turk and A. Pentland, 1991, build their eigenface space from exactly
//! these normalised patches), and it is the strongest claim this process can honestly
//! make: it measures RESEMBLANCE of two face patches and never identity, since nothing
//! here holds a trained model of who a face belongs to. A `Similarity` it reports is
//! therefore its own measured correlation, not a graded identity score, and the module
//! reports no value nothing measured either way.
//!
//! ## Where it runs
//!
//! Every step is bounded, but all of it is CPU-bound over an image whose size the caller
//! chose, so a request path reaches it through [`faces_in_submitted_image`], which runs it
//! on tokio's BLOCKING pool: it occupies no async worker, and the `spawn_blocking(..).await`
//! that drives it is a real await point a timeout, a cancellation or a shutdown can land on.

use std::io::Cursor;

use ::image::{ImageFormat, Limits};

use super::image_source::{self, UnreadableImage};
use crate::ops::types::{BoundingBox, ComparedFace, FaceDetail, Image, ImageQuality, Landmark, Pose};

/// The most faces one answer reports: `DetectFaces` "detects the 100 largest faces in the
/// image".
pub const MAX_FACES: usize = 100;

/// The largest side, in pixels, of an image this service analyses. NOT an AWS limit: an
/// emulator-side bound so one request cannot allocate without end.
pub const MAX_IMAGE_SIDE: u32 = 8_192;

/// The most pixels one image may hold. NOT an AWS limit: see [`MAX_IMAGE_SIDE`]. Every scan
/// below is linear in the pixel count, so this bounds the per-request CPU too.
pub const MAX_IMAGE_PIXELS: u64 = 4_000_000;

/// The most bytes the decoder may allocate for one image. NOT an AWS limit: it is what stops
/// a small compressed file from expanding into a large allocation.
pub const MAX_IMAGE_ALLOC: u64 = 64 * 1024 * 1024;

/// The most feature components ONE window width contributes to the eye-pair search,
/// largest by area first. NOT an AWS limit: the search costs one verification per pair, so
/// this bounds the work a single request can cause.
pub const MAX_FEATURE_BLOBS: usize = 210;

/// The window widths the feature search is run at, as divisors of the image's shorter side:
/// one narrower than a small face's features, one about a large face's, and one between.
pub const FEATURE_SCALES: [f32; 3] = [32.0, 16.0, 8.0];

/// How far below the mean of the window around it, in grey levels, a pixel must sit to be a
/// facial feature — and how far above the eyes the skin between and below them must sit for
/// the pair to be a face.
pub const MIN_FEATURE_CONTRAST: f32 = 18.0;

/// How far the mean intensity of a face box must differ from the ring around it, in grey
/// levels, for the box to be a figure against its ground.
pub const MIN_REGION_CONTRAST: f32 = 4.0;

/// How much the skin a face shows may VARY, as a fraction of how far that skin stands above
/// the face's own dark features: a cheek or a nose bridge is a smooth region, so its
/// root-mean-square gradient is a fraction of the contrast between it and the eyes, while a
/// patch of noise or texture swings by as much as that contrast or more and is not skin.
pub const MAX_SKIN_TEXTURE_RATIO: f32 = 0.5;

/// How far out of level, as a fraction of their separation, two blobs may be and still be a
/// pair of eyes. It is also what bounds the `Roll` this detector can measure.
pub const MAX_EYE_LEVEL_RATIO: f32 = 0.3;

/// The face a pair of eyes stands for, in multiples of their separation, and where the eye
/// line sits in it as a fraction of its height: the standard proportions of a face, in which
/// the interocular distance is about half the face's width and the eyes sit just above the
/// middle.
pub const FACE_WIDTH_IN_EYE_SPANS: f32 = 2.0;
pub const FACE_HEIGHT_IN_EYE_SPANS: f32 = 2.9;
pub const EYE_LINE_FROM_TOP: f32 = 0.45;

/// The agreement score a candidate must reach to be reported as a face, on the same 0..100
/// scale as the `Confidence` it carries.
pub const MIN_CONFIDENCE: f32 = 50.0;

/// How much of an already accepted face's region a later reading may share, as the
/// intersection over the union of the two, before it is taken for the same face.
pub const MAX_OVERLAP: f32 = 0.3;

/// How much of the SMALLER of two readings' regions may lie inside the larger before the
/// two are ONE face read twice. A pair of features read at another window width, or a
/// wider pair taken off the same face, derives a box of another size around the same
/// pixels, and a box nested in another box is not a second face beside it — which the
/// intersection over the UNION misses exactly when the two differ most in size.
pub const MAX_SHARED_FRACTION: f32 = 0.6;

/// How much of a reading's own box the image must hold for the reading to have measured
/// the whole of that box, rather than only the part of it the frame carries.
const MIN_WHOLE_BOX_FRACTION: f32 = 0.98;

/// How many cells on a side the appearance of a face is measured on: fine enough to carry
/// where that face's own features sit inside its box, coarse enough that one cell still
/// averages many pixels for a small face.
pub const APPEARANCE_GRID: usize = 8;

/// The `Quality.Sharpness` a detected face must measure to be compared at each
/// `QualityFilter` level (`smithy_query(query="shape",shape="QualityFilter")`: NONE | AUTO
/// | LOW | MEDIUM | HIGH), on the measured percentage scale [`sharpness`] reports.
///
/// AWS publishes no number for its own quality bar — "the quality bar is based on a variety
/// of common use cases" — so these are the EMULATOR's bars on the one quality this process
/// measures: how sharp the face's own pixels are, which is what tells a face that is too
/// blurry to compare from one that is not. `NONE` filters nothing, which is the documented
/// default, and `AUTO`, where "Amazon Rekognition chooses the quality bar", is this
/// emulator choosing the lowest bar that still filters.
pub const QUALITY_BAR_LOW: f32 = 1.0;
pub const QUALITY_BAR_MEDIUM: f32 = 3.0;
pub const QUALITY_BAR_HIGH: f32 = 6.0;

/// How much of a face's box must be the face's own SKIN — the tone measured between and
/// below its eyes — for the box to be a face at all: a box that is in the majority
/// something else (the background, a garment, hair, the side of a neck) is a region of the
/// picture a pair of dark specks lined up in, not a face.
pub const MIN_FACE_SKIN_FRACTION: f32 = 0.5;

/// How far from that skin's own tone a pixel may sit and still BE that skin, as a fraction
/// of how far the skin stands above the face's dark features: the features set into a face
/// are what that contrast measures, so half of it is the span of tone the skin itself
/// covers as the light falls across it.
const MAX_SKIN_DEVIATION: f32 = 0.5;

/// How many points, on a side, the skin coverage of a face box samples, which is never
/// zero. NOT an AWS limit: it bounds the measurement's cost by a constant whatever the
/// box's size.
const SKIN_SAMPLES: usize = 32;

/// How far OUTSIDE each eye, along the eye line and in multiples of the eye span, the skin
/// at the side of the face is measured, and the half-size of that measurement: inside the
/// box the proportions derive, and far enough out that it is the face beside the eye rather
/// than the eye itself.
const TEMPLE_OUTSIDE_EYE_IN_SPANS: f32 = 0.25;
const TEMPLE_HALF_SIZE_IN_SPANS: f32 = 0.1;

/// How far above the eyes the brighter of those two sides must stand, as a fraction of the
/// contrast measured between this face's own skin and its own eyes.
pub const MIN_TEMPLE_CONTRAST_RATIO: f32 = 0.5;

/// The radius of the averaging window the feature mask and every region mean read through.
const SMOOTH_RADIUS: usize = 1;

/// How small a face this detector looks for: the eyes must be at least this many pixels
/// apart, and at least this fraction of the image's shorter side apart, so a request cannot
/// make a face out of two specks.
const MIN_EYE_SPAN_PIXELS: f32 = 6.0;
const MIN_EYE_SPAN_FRACTION: f32 = 0.02;

/// How much of its derived box must lie inside the frame for a face to be reported: below
/// this, too little of the face was in the image to measure.
pub const MIN_INSIDE_FRACTION: f32 = 0.5;

/// How alike in area and in height the two blobs of a pair of eyes must be.
const MAX_EYE_AREA_RATIO: f32 = 4.0;
const MAX_EYE_HEIGHT_RATIO: f32 = 3.0;

/// How wide one eye may be, and must be, in multiples of the gap between the two: eyes are
/// separated by more than their own width.
const MAX_EYE_WIDTH_IN_SPANS: f32 = 0.6;
const MIN_EYE_WIDTH_IN_SPANS: f32 = 0.04;

/// Where a mouth sits below the eye line, and how wide it is, in multiples of the eye span:
/// the band searched, the offset from the face's axis allowed, and the width accepted.
const MOUTH_BELOW_EYES: f32 = 1.0;
const MIN_MOUTH_BELOW_EYES: f32 = 0.45;
const MAX_MOUTH_BELOW_EYES: f32 = 1.7;
const MAX_MOUTH_OFFSET_IN_SPANS: f32 = 0.6;
const MIN_MOUTH_WIDTH_IN_SPANS: f32 = 0.22;
const MAX_MOUTH_WIDTH_IN_SPANS: f32 = 1.6;

/// How far from where the proportions put it a mouth may sit before it contributes nothing
/// to the score, in multiples of the eye span.
const MOUTH_FIT_IN_SPANS: f32 = 0.5;

/// The intensity difference between the bridge of the nose and the eyes at which the
/// contrast term of the score is full, in grey levels.
const FULL_FEATURE_CONTRAST: f32 = 96.0;

/// The largest blob the feature search keeps, as a fraction of the image's shorter side:
/// anything that wide or tall is a region of the image rather than a facial feature.
const MAX_BLOB_SIDE_FRACTION: f32 = 0.45;

/// The smallest blob the feature search keeps, in pixels and as a fraction of the image's
/// area, so speckle is not a feature at any image size.
const MIN_BLOB_PIXELS: u32 = 4;
const MIN_BLOB_AREA_FRACTION: f32 = 1.0 / 40_000.0;

/// How much wider than its own height an eye or a mouth blob may be, either way round.
const MAX_BLOB_ASPECT: f32 = 8.0;

/// How many points, on a side, a region's gradient measurement samples. NOT an AWS limit:
/// it bounds the measurement's cost by a constant whatever the region's size. The quality a
/// face reports is measured over its whole box; the skin-texture check over the small
/// regions it verifies.
const SHARPNESS_SAMPLES: usize = 48;
const TEXTURE_SAMPLES: usize = 16;

/// The landmark types this detector measures, as the model's `LandmarkType` names them
/// (`smithy_query(query="shape",shape="LandmarkType")`). The eye landmarks are named for
/// the side of the IMAGE they were found on, which is also the order the pair is read in.
const LANDMARK_EYE_LEFT: &str = "eyeLeft";
const LANDMARK_EYE_RIGHT: &str = "eyeRight";
const LANDMARK_MOUTH_LEFT: &str = "mouthLeft";
const LANDMARK_MOUTH_RIGHT: &str = "mouthRight";

/// `Some(format)` when these bytes are an image in a format `DetectFaces` accepts: "the
/// image must be either a PNG or JPEG formatted file". The format comes from the bytes' own
/// magic numbers, never from a declared name.
pub fn image_format(bytes: &[u8]) -> Option<ImageFormat> {
    match ::image::guess_format(bytes) {
        Ok(format @ (ImageFormat::Png | ImageFormat::Jpeg)) => Some(format),
        _ => None,
    }
}

/// The faces in the image a request submitted — its own inline `Bytes`, or the object this
/// emulator's Amazon S3 holds at the location it named — detected OFF the async worker.
pub async fn faces_in_submitted_image(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    image: &Image,
) -> Result<Vec<FaceDetail>, UnreadableImage> {
    Ok(measured_faces_in_submitted_image(ctx, image)
        .await?
        .into_iter()
        .map(|face| face.detail)
        .collect())
}

/// The faces in the image a request submitted, each with the appearance that lets it be
/// compared with a face of another image. The same detection as
/// [`faces_in_submitted_image`], which is what an operation that only REPORTS faces uses.
pub async fn measured_faces_in_submitted_image(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    image: &Image,
) -> Result<Vec<MeasuredFace>, UnreadableImage> {
    let bytes = image_source::image_bytes(ctx, image).await?;
    match ::tokio::task::spawn_blocking(move || measured_faces_in_image(&bytes)).await {
        Ok(detected) => detected,
        Err(error) => {
            ::tracing::error!(error = %error, "detecting the faces in an image did not complete");
            Err(UnreadableImage::NotAnalysed(
                "Detecting the faces in the image did not complete".to_string(),
            ))
        }
    }
}

/// The faces in one image's bytes.
///
/// CPU-bound: a request path reaches it through [`faces_in_submitted_image`], which runs it
/// on the blocking pool.
pub fn faces_in_image(bytes: &[u8]) -> Result<Vec<FaceDetail>, UnreadableImage> {
    Ok(measured_faces_in_image(bytes)?
        .into_iter()
        .map(|face| face.detail)
        .collect())
}

/// The faces in one image's bytes, each with its measured appearance.
///
/// CPU-bound: a request path reaches it through [`measured_faces_in_submitted_image`],
/// which runs it on the blocking pool.
pub fn measured_faces_in_image(bytes: &[u8]) -> Result<Vec<MeasuredFace>, UnreadableImage> {
    Ok(measured_faces_in_decoded_image(&decoded_image(bytes)?))
}

/// The frame one image's bytes decode to, in the format the bytes themselves name and
/// bounded so a small file cannot decode into a large one.
///
/// This is the one decode every analysis of a submitted image goes through, so an image is
/// accepted, refused and bounded identically whatever is being measured off it.
pub fn decoded_image(bytes: &[u8]) -> Result<::image::DynamicImage, UnreadableImage> {
    let Some(format) = image_format(bytes) else {
        return Err(UnreadableImage::invalid_format());
    };
    // The header is read with the decoder's own size limits OFF, so an image over this
    // emulator's ceiling is refused as too large (what it is) rather than as unreadable.
    let (width, height) = reader(bytes, format, false)
        .into_dimensions()
        .map_err(|_| UnreadableImage::invalid_format())?;
    if over_ceiling(width, height) {
        return Err(oversized(width, height));
    }
    let decoded = reader(bytes, format, true)
        .decode()
        .map_err(|_| UnreadableImage::invalid_format())?;
    // The ceiling again, on the frame that was actually decoded: every scan sized from THIS
    // image must not depend on the header having agreed with it.
    if over_ceiling(decoded.width(), decoded.height()) {
        return Err(oversized(decoded.width(), decoded.height()));
    }
    Ok(decoded)
}

/// The faces in an already decoded frame, each with its measured appearance: what an
/// analysis that measures something ELSE off the same frame reuses, so one image is decoded
/// once however many of its properties are reported.
///
/// CPU-bound, like [`measured_faces_in_image`], which is this with the decode in front.
pub fn measured_faces_in_decoded_image(decoded: &::image::DynamicImage) -> Vec<MeasuredFace> {
    let gray = decoded.to_luma8();
    if gray.width() == 0 || gray.height() == 0 {
        return Vec::new();
    }
    faces(&Pixels::of(gray))
}

/// The faces in an already decoded frame, as the details an operation reports.
pub fn faces_in_decoded_image(decoded: &::image::DynamicImage) -> Vec<FaceDetail> {
    measured_faces_in_decoded_image(decoded)
        .into_iter()
        .map(|face| face.detail)
        .collect()
}

/// Whether an image of these dimensions is past what this emulator analyses.
fn over_ceiling(width: u32, height: u32) -> bool {
    width > MAX_IMAGE_SIDE
        || height > MAX_IMAGE_SIDE
        || u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS
}

/// The refusal for an image past that ceiling, stating what was measured and the limit.
fn oversized(width: u32, height: u32) -> UnreadableImage {
    UnreadableImage::TooLarge(format!(
        "Image size is too large: the image is {width}x{height} pixels; this emulator \
         analyses at most {MAX_IMAGE_SIDE} pixels on a side and {MAX_IMAGE_PIXELS} pixels \
         in all"
    ))
}

/// A decoder for `bytes`, bounded so a small file cannot decode into a large one.
///
/// `bound_dimensions` turns the decoder's own width/height ceilings on, for the decode
/// itself; the header read leaves them off so this module can tell an image that is too
/// large from one that cannot be read at all.
fn reader(
    bytes: &[u8],
    format: ImageFormat,
    bound_dimensions: bool,
) -> ::image::ImageReader<Cursor<&[u8]>> {
    let mut reader = ::image::ImageReader::with_format(Cursor::new(bytes), format);
    // `Limits::no_limits()` rather than `default()`: the bounds here are this emulator's,
    // and starting from the crate's defaults would silently inherit them.
    let mut limits = Limits::no_limits();
    if bound_dimensions {
        limits.max_image_width = Some(MAX_IMAGE_SIDE);
        limits.max_image_height = Some(MAX_IMAGE_SIDE);
    }
    limits.max_alloc = Some(MAX_IMAGE_ALLOC);
    reader.limits(limits);
    reader
}

// ---------------------------------------------------------------------------
// Pixels, and the measurements taken off them
// ---------------------------------------------------------------------------

/// A rectangle of pixels. `x1` and `y1` are EXCLUSIVE, and every instance is already
/// clamped to its image, so a rectangle can be empty but never out of bounds.
#[derive(Debug, Clone, Copy)]
struct PixelRect {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

impl PixelRect {
    fn area(&self) -> usize {
        self.x1.saturating_sub(self.x0) * self.y1.saturating_sub(self.y0)
    }

    fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }
}

/// The pixels of `left..right` x `top..bottom`, clamped to an image of `width` x `height`.
fn pixel_rect(
    bounds: (f32, f32, f32, f32),
    width: usize,
    height: usize,
) -> PixelRect {
    let (left, top, right, bottom) = bounds;
    // A cast from a float to an integer saturates, and a negative coordinate is outside the
    // image on the low side, so both ends land inside the image.
    let index = |value: f32, bound: usize| -> usize {
        if value <= 0.0 {
            0
        } else {
            (value as usize).min(bound)
        }
    };
    PixelRect {
        x0: index(left.floor(), width),
        y0: index(top.floor(), height),
        x1: index(right.ceil(), width),
        y1: index(bottom.ceil(), height),
    }
}

/// The summed-area table of one grey plane: the sum over any rectangle in four lookups.
struct Integral {
    width: usize,
    height: usize,
    sums: Vec<u32>,
}

impl Integral {
    /// The table of `pixels`, which is `width` x `height` in row-major order.
    ///
    /// `u32` holds it: the largest sum is 255 times the pixel count, and the pixel count is
    /// bounded by [`MAX_IMAGE_PIXELS`].
    fn of(pixels: &[u8], width: usize, height: usize) -> Self {
        let stride = width + 1;
        let mut sums = vec![0_u32; stride * (height + 1)];
        for y in 0..height {
            let (above, current) = sums.split_at_mut((y + 1) * stride);
            let above = &above[y * stride..];
            let mut row = 0_u32;
            for (x, value) in pixels[y * width..(y + 1) * width].iter().enumerate() {
                row += u32::from(*value);
                current[x + 1] = above[x + 1] + row;
            }
        }
        Self {
            width,
            height,
            sums,
        }
    }

    fn sum(&self, rect: PixelRect) -> u32 {
        let stride = self.width + 1;
        self.sums[rect.y1 * stride + rect.x1] + self.sums[rect.y0 * stride + rect.x0]
            - self.sums[rect.y0 * stride + rect.x1]
            - self.sums[rect.y1 * stride + rect.x0]
    }

    /// The mean intensity over `rect`, or `None` when it holds no pixel to measure.
    fn mean(&self, rect: PixelRect) -> Option<f32> {
        if rect.is_empty() {
            return None;
        }
        Some(self.sum(rect) as f32 / rect.area() as f32)
    }

    /// The mean intensity of the window of `radius` around `(x, y)`, clipped at the edges.
    fn window_mean(&self, x: usize, y: usize, radius: usize) -> f32 {
        let rect = PixelRect {
            x0: x.saturating_sub(radius),
            y0: y.saturating_sub(radius),
            x1: (x + radius + 1).min(self.width),
            y1: (y + radius + 1).min(self.height),
        };
        self.mean(rect).unwrap_or_default()
    }
}

/// A decoded image, with the two planes every measurement reads: the pixels as decoded, and
/// an averaged copy with its summed-area table.
struct Pixels {
    width: usize,
    height: usize,
    /// As decoded: what the sharpness measurement reads, because averaging the pixels is
    /// exactly what would hide the gradients it measures.
    decoded: Vec<u8>,
    /// Averaged over a 3x3 window: what the feature mask and every region mean read, so one
    /// noisy pixel is neither a feature nor a measurement.
    smoothed: Vec<u8>,
    integral: Integral,
}

impl Pixels {
    fn of(gray: ::image::GrayImage) -> Self {
        let width = gray.width() as usize;
        let height = gray.height() as usize;
        let decoded = gray.into_raw();
        let decoded_integral = Integral::of(&decoded, width, height);
        let smoothed: Vec<u8> = (0..decoded.len())
            .map(|index| {
                let mean =
                    decoded_integral.window_mean(index % width, index / width, SMOOTH_RADIUS);
                mean.round().clamp(0.0, 255.0) as u8
            })
            .collect();
        let integral = Integral::of(&smoothed, width, height);
        Self {
            width,
            height,
            decoded,
            smoothed,
            integral,
        }
    }

    fn width_f32(&self) -> f32 {
        self.width as f32
    }

    fn height_f32(&self) -> f32 {
        self.height as f32
    }

    /// The shorter side of the image, which is what the size bounds scale with.
    fn shorter_side(&self) -> f32 {
        self.width_f32().min(self.height_f32())
    }

    fn rect(&self, bounds: (f32, f32, f32, f32)) -> PixelRect {
        pixel_rect(bounds, self.width, self.height)
    }

    /// The rectangle of half-width `half_width` and half-height `half_height` around
    /// `(x, y)`.
    fn around(&self, x: f32, y: f32, half_width: f32, half_height: f32) -> PixelRect {
        self.rect((x - half_width, y - half_height, x + half_width, y + half_height))
    }

    fn decoded_at(&self, x: usize, y: usize) -> f32 {
        f32::from(self.decoded[y * self.width + x])
    }

    /// The averaged plane at one pixel: what every measurement of a region's TONE reads, so
    /// one noisy pixel is not a measurement.
    fn smoothed_at(&self, x: usize, y: usize) -> f32 {
        f32::from(self.smoothed[y * self.width + x])
    }
}

/// One connected run of feature pixels: a candidate eye, nostril, brow or mouth.
#[derive(Debug, Clone, Copy)]
struct Blob {
    area: u32,
    rect: PixelRect,
    cx: f32,
    cy: f32,
    /// The mean intensity of the blob's own pixels.
    mean: f32,
}

impl Blob {
    fn width(&self) -> f32 {
        (self.rect.x1 - self.rect.x0) as f32
    }

    fn height(&self) -> f32 {
        (self.rect.y1 - self.rect.y0) as f32
    }
}

/// A face box in pixel coordinates.
#[derive(Debug, Clone, Copy)]
struct FaceRect {
    left: f32,
    top: f32,
    width: f32,
    height: f32,
}

impl FaceRect {
    fn right(&self) -> f32 {
        self.left + self.width
    }

    fn bottom(&self) -> f32 {
        self.top + self.height
    }

    fn area(&self) -> f32 {
        self.width * self.height
    }

    fn bounds(&self) -> (f32, f32, f32, f32) {
        (self.left, self.top, self.right(), self.bottom())
    }
}

/// The mouth of a detected face: the extent of the feature blob it was read from.
#[derive(Debug, Clone, Copy)]
struct Mouth {
    x0: f32,
    x1: f32,
    cy: f32,
}

/// One face the detector found, with the measurements its `FaceDetail` is built from.
#[derive(Debug, Clone, Copy)]
struct Candidate {
    rect: FaceRect,
    confidence: f32,
    left_eye: (f32, f32),
    right_eye: (f32, f32),
    mouth: Mouth,
    roll: f32,
}

/// Every face in one decoded image, largest first and cut to [`MAX_FACES`].
fn faces(pixels: &Pixels) -> Vec<MeasuredFace> {
    let blobs = feature_blobs(pixels);
    let mut found: Vec<Candidate> = Vec::new();
    for first in 0..blobs.len() {
        for second in (first + 1)..blobs.len() {
            if let Some(candidate) = candidate(pixels, &blobs, first, second) {
                found.push(candidate);
            }
        }
    }
    // Each reading paired with the region it REPORTS — the part of its box the image holds,
    // which is what an answer claims and so what decides both which readings stand for one
    // face and how large each answered face is. Measured once per reading rather than once
    // per comparison.
    let mut found: Vec<(Candidate, FaceRect)> = found
        .into_iter()
        .map(|candidate| {
            let region = reported_region(pixels, &candidate.rect);
            (candidate, region)
        })
        .collect();
    // The best reading first, so the box kept for a face is that reading of it and every
    // other reading of the same pixels is suppressed rather than reported beside it. A
    // reading whose box the image holds WHOLE measured all of that face; one whose box runs
    // off the frame measured only the part of it the pixels show, so the complete reading is
    // the better measurement of the same face, and between two readings measured alike the
    // stronger-scoring one is.
    found.sort_by(|(left, left_region), (right, right_region)| {
        measured_whole(right, right_region)
            .cmp(&measured_whole(left, left_region))
            .then_with(|| right.confidence.total_cmp(&left.confidence))
    });
    let mut kept: Vec<(Candidate, FaceRect)> = Vec::new();
    for (candidate, region) in found {
        if kept
            .iter()
            .all(|(_, other)| !same_face(&region, other))
        {
            kept.push((candidate, region));
        }
    }
    // "The faces that are returned by IndexFaces are sorted by the largest face bounding box
    // size to the smallest": the box each face reports, which is the region above.
    kept.sort_by(|(_, left), (_, right)| right.area().total_cmp(&left.area()));
    kept.truncate(MAX_FACES);
    kept.iter()
        .map(|(face, _)| MeasuredFace {
            detail: detail(pixels, face),
            appearance: appearance(pixels, &face.rect),
        })
        .collect()
}

/// One detected face: what it REPORTS (the members this detector measured) and what it is
/// COMPARED by (the appearance of its own pixels). Both come off the same detection, so a
/// face reported and a face compared are the same face.
#[derive(Debug, Clone)]
pub struct MeasuredFace {
    pub detail: FaceDetail,
    pub appearance: FaceAppearance,
}

impl MeasuredFace {
    /// Whether this face is sharp enough to be compared at `bar`, the
    /// `Quality.Sharpness` floor a `QualityFilter` level sets: see [`quality_bar`].
    pub fn meets_quality_bar(&self, bar: f32) -> bool {
        self.sharpness() >= bar
    }

    /// The `Quality.Sharpness` measured for this face's own pixels.
    fn sharpness(&self) -> f32 {
        self.detail
            .quality
            .as_ref()
            .and_then(|quality| quality.sharpness)
            .unwrap_or_default()
    }
}

/// A detected face as the `ComparedFace` the operations that report one carry it as:
/// "provides face metadata for target image faces that are analyzed by `CompareFaces` and
/// `RecognizeCelebrities`" (`smithy_query(query="shape",shape="ComparedFace")`). Every
/// member the detector measured is carried across unchanged; `Emotions` and `Smile` are a
/// classifier's judgement and no classifier runs here, so they stay absent rather than
/// being filled in. One conversion, so the two operations that report a compared face
/// report the same face.
pub fn compared_face(detail: &FaceDetail) -> ComparedFace {
    ComparedFace {
        bounding_box: detail.bounding_box.clone(),
        confidence: detail.confidence,
        landmarks: detail.landmarks.clone(),
        pose: detail.pose.clone(),
        quality: detail.quality.clone(),
        emotions: None,
        smile: None,
    }
}

/// The `Quality.Sharpness` floor a `QualityFilter` sets, which is none at all for the
/// `NONE` the member documents as its default and for a request that sends no filter.
pub fn quality_bar(filter: Option<&str>) -> f32 {
    match filter.unwrap_or("NONE") {
        "LOW" | "AUTO" => QUALITY_BAR_LOW,
        "MEDIUM" => QUALITY_BAR_MEDIUM,
        "HIGH" => QUALITY_BAR_HIGH,
        // `NONE` filters nothing. A value the enum does not name cannot reach here: the
        // model's own enum constraint refuses it before the handler runs.
        _ => 0.0,
    }
}

/// What one detected face LOOKS LIKE: the mean intensity of each cell of a grid over its
/// own box, centred and scaled to unit length so a shift of brightness or a change of
/// contrast between two pictures of one face is divided out. See this module's docs.
#[derive(Debug, Clone, PartialEq)]
pub struct FaceAppearance {
    cells: Vec<f32>,
}

impl FaceAppearance {
    /// How alike two faces look, as the correlation of their two grids on the 0..100 scale
    /// a `Similarity` carries: 100 for two faces whose cells rise and fall together
    /// exactly, and 0 for two that do not resemble each other at all.
    ///
    /// Both grids are unit-length, so their dot product IS that correlation and cannot
    /// leave -1..1. A negative correlation is no resemblance rather than an anti-face, so
    /// the scale starts at zero.
    pub fn similarity_to(&self, other: &Self) -> f32 {
        let correlation: f32 = self
            .cells
            .iter()
            .zip(other.cells.iter())
            .map(|(own, their)| own * their)
            .sum();
        (correlation * 100.0).clamp(0.0, 100.0)
    }
}

/// The appearance of the face at `rect`: its box resampled onto an
/// [`APPEARANCE_GRID`]-square grid of region means, then centred and normalised.
///
/// Resampling makes the measurement independent of how large the face is in the image: the
/// same face photographed closer fills more pixels but the same grid. A cell with no pixel
/// inside the frame — the part of a face at the edge of the image that was not
/// photographed — takes the mean of the face's own box, the one value that adds nothing to
/// the comparison, because nothing measured that cell.
fn appearance(pixels: &Pixels, rect: &FaceRect) -> FaceAppearance {
    let whole = pixels.integral.mean(pixels.rect(rect.bounds()));
    let span = APPEARANCE_GRID as f32;
    let mut cells = Vec::with_capacity(APPEARANCE_GRID * APPEARANCE_GRID);
    for row in 0..APPEARANCE_GRID {
        for column in 0..APPEARANCE_GRID {
            let cell = pixels.rect((
                rect.left + rect.width * column as f32 / span,
                rect.top + rect.height * row as f32 / span,
                rect.left + rect.width * (column + 1) as f32 / span,
                rect.top + rect.height * (row + 1) as f32 / span,
            ));
            cells.push(
                pixels
                    .integral
                    .mean(cell)
                    .or(whole)
                    .unwrap_or_default(),
            );
        }
    }
    normalised(cells)
}

/// `cells` centred on their own mean and scaled to unit length: the form two appearances
/// are correlated in. A grid whose cells are all the same value carries no variation to
/// correlate — it is a flat patch, not a layout of features — so it stays at zero and
/// resembles nothing, including itself.
fn normalised(mut cells: Vec<f32>) -> FaceAppearance {
    let mean = cells.iter().sum::<f32>() / cells.len().max(1) as f32;
    for cell in &mut cells {
        *cell -= mean;
    }
    let length = cells.iter().map(|cell| cell * cell).sum::<f32>().sqrt();
    if length > 0.0 {
        for cell in &mut cells {
            *cell /= length;
        }
    }
    FaceAppearance { cells }
}

/// The feature components of an image: the pixels darker than their surroundings, grouped,
/// filtered to what can be a facial feature, and cut to the largest [`MAX_FEATURE_BLOBS`]
/// per window width.
///
/// A component found at two window widths is offered twice, and the two readings of it stand
/// for the same face, which the overlap suppression in [`faces`] then keeps once.
fn feature_blobs(pixels: &Pixels) -> Vec<Blob> {
    let min_area = MIN_BLOB_PIXELS.max(
        (MIN_BLOB_AREA_FRACTION * pixels.width_f32() * pixels.height_f32()).round() as u32,
    );
    let max_side = MAX_BLOB_SIDE_FRACTION * pixels.shorter_side();
    let per_scale = MAX_FEATURE_BLOBS / FEATURE_SCALES.len();
    let mut blobs: Vec<Blob> = Vec::new();
    for divisor in FEATURE_SCALES {
        let radius = ((pixels.shorter_side() / divisor).round() as usize).clamp(2, 60);
        let mut found = components(pixels, &feature_mask(pixels, radius));
        found.retain(|blob| {
            blob.area >= min_area
                && blob.width() <= max_side
                && blob.height() <= max_side
                && blob.width() <= MAX_BLOB_ASPECT * blob.height()
                && blob.height() <= MAX_BLOB_ASPECT * blob.width()
        });
        found.sort_by_key(|blob| ::std::cmp::Reverse(blob.area));
        found.truncate(per_scale);
        blobs.append(&mut found);
    }
    blobs
}

/// One bit per pixel, true where the pixel is darker than the mean of the window of
/// `radius` around it by [`MIN_FEATURE_CONTRAST`].
fn feature_mask(pixels: &Pixels, radius: usize) -> Vec<bool> {
    pixels
        .smoothed
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let local = pixels
                .integral
                .window_mean(index % pixels.width, index / pixels.width, radius);
            f32::from(*value) + MIN_FEATURE_CONTRAST <= local
        })
        .collect()
}

/// The 8-connected components of a feature mask, each with its extent, centroid, area and
/// mean intensity. Each pixel is visited once, so this is linear in the image.
fn components(pixels: &Pixels, mask: &[bool]) -> Vec<Blob> {
    let (width, height) = (pixels.width, pixels.height);
    let mut seen = vec![false; mask.len()];
    let mut stack: Vec<usize> = Vec::new();
    let mut blobs: Vec<Blob> = Vec::new();
    for (start, is_feature) in mask.iter().enumerate() {
        if !is_feature || seen[start] {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let mut area = 0_u32;
        let mut sum_x = 0_u64;
        let mut sum_y = 0_u64;
        let mut sum_intensity = 0_u64;
        let mut rect = PixelRect {
            x0: width,
            y0: height,
            x1: 0,
            y1: 0,
        };
        while let Some(index) = stack.pop() {
            let (x, y) = (index % width, index / width);
            area += 1;
            sum_x += x as u64;
            sum_y += y as u64;
            sum_intensity += u64::from(pixels.smoothed[index]);
            rect.x0 = rect.x0.min(x);
            rect.y0 = rect.y0.min(y);
            rect.x1 = rect.x1.max(x + 1);
            rect.y1 = rect.y1.max(y + 1);
            let x_to = (x + 1).min(width - 1);
            let y_to = (y + 1).min(height - 1);
            for neighbour_y in y.saturating_sub(1)..=y_to {
                for neighbour_x in x.saturating_sub(1)..=x_to {
                    let neighbour = neighbour_y * width + neighbour_x;
                    if mask[neighbour] && !seen[neighbour] {
                        seen[neighbour] = true;
                        stack.push(neighbour);
                    }
                }
            }
        }
        let area_f32 = area as f32;
        blobs.push(Blob {
            area,
            rect,
            cx: sum_x as f32 / area_f32,
            cy: sum_y as f32 / area_f32,
            mean: sum_intensity as f32 / area_f32,
        });
    }
    blobs
}

/// The face a pair of feature blobs stands for, when the pixels around them carry the rest
/// of a face's structure; `None` when they do not, which is most pairs.
fn candidate(pixels: &Pixels, blobs: &[Blob], first: usize, second: usize) -> Option<Candidate> {
    let (left, right) = if blobs[first].cx <= blobs[second].cx {
        (blobs[first], blobs[second])
    } else {
        (blobs[second], blobs[first])
    };
    let dx = right.cx - left.cx;
    let dy = right.cy - left.cy;
    let span = dx.hypot(dy);
    if span < MIN_EYE_SPAN_PIXELS.max(MIN_EYE_SPAN_FRACTION * pixels.shorter_side())
        || dy.abs() > MAX_EYE_LEVEL_RATIO * span
    {
        return None;
    }
    // Two eyes of one face are alike in size and in height, and each is narrower than the
    // gap between them.
    let smaller = f32::min(left.area as f32, right.area as f32);
    let larger = f32::max(left.area as f32, right.area as f32);
    if larger > MAX_EYE_AREA_RATIO * smaller
        || left.height().max(right.height()) > MAX_EYE_HEIGHT_RATIO * left.height().min(right.height())
        || left.width().max(right.width()) > MAX_EYE_WIDTH_IN_SPANS * span
        || left.width().min(right.width()) < MIN_EYE_WIDTH_IN_SPANS * span
    {
        return None;
    }

    let mid_x = (left.cx + right.cx) / 2.0;
    let mid_y = (left.cy + right.cy) / 2.0;
    let width = FACE_WIDTH_IN_EYE_SPANS * span;
    let height = FACE_HEIGHT_IN_EYE_SPANS * span;
    let rect = FaceRect {
        left: mid_x - width / 2.0,
        top: mid_y - EYE_LINE_FROM_TOP * height,
        width,
        height,
    };
    // A face box mostly outside the frame was not measured: most of the pixels that would
    // show it are not here.
    if (pixels.rect(rect.bounds()).area() as f32) < MIN_INSIDE_FRACTION * rect.area() {
        return None;
    }

    // The skin a face shows between and below its dark features: the bridge between the
    // eyes and the band across the nose and cheeks. Both stand above the eyes in intensity,
    // whatever the tone of the skin or the exposure of the image.
    let eyes = (left.mean * left.area as f32 + right.mean * right.area as f32)
        / (left.area + right.area) as f32;
    let bridge_rect = pixels.around(mid_x, mid_y, 0.2 * span, 0.2 * span);
    let cheeks_rect = pixels.around(mid_x, mid_y + 0.55 * span, 0.35 * span, 0.2 * span);
    let bridge = pixels.integral.mean(bridge_rect)?;
    let cheeks = pixels.integral.mean(cheeks_rect)?;
    if bridge < eyes + MIN_FEATURE_CONTRAST || cheeks < eyes + MIN_FEATURE_CONTRAST {
        return None;
    }
    // Skin is SMOOTH: it varies by a fraction of how far it stands above the features it
    // surrounds. A region that swings by as much as that contrast is texture or noise, and
    // two dark specks in a field of noise are not a face.
    let contrast = bridge.min(cheeks) - eyes;
    for region in [bridge_rect, cheeks_rect] {
        if rms_gradient(pixels, region, TEXTURE_SAMPLES) > MAX_SKIN_TEXTURE_RATIO * contrast {
            return None;
        }
    }
    if !stands_out(pixels, &rect, span) {
        return None;
    }
    // A face's box is mostly the FACE: the skin measured between and below its eyes covers
    // it, with the features set into that skin. A box that is in the majority something
    // else was not read off a face.
    let skin = (bridge + cheeks) / 2.0;
    if skin_coverage(pixels, pixels.rect(rect.bounds()), skin, contrast) < MIN_FACE_SKIN_FRACTION
    {
        return None;
    }
    // The eyes are set INTO a face: outside each of them the face carries on toward the side
    // of the head, so the skin just beyond an eye stands well above that eye in intensity.
    // A pair of dark blobs with nothing but background outside them flanks something that is
    // not a face at all — the side of a neck, a gap between two objects, the shoulders of a
    // figure — and reads as a face only if nothing outside the pair is ever measured. The
    // measurement follows the eye line, so a tilted face is read along its own axis, and
    // only the BRIGHTER of the two sides has to stand out, because a face can be lit from
    // one side or carry hair across the other.
    let outward = TEMPLE_OUTSIDE_EYE_IN_SPANS * span;
    let (along_x, along_y) = (dx / span, dy / span);
    let temples = [
        (left.cx - along_x * outward, left.cy - along_y * outward),
        (right.cx + along_x * outward, right.cy + along_y * outward),
    ]
    .into_iter()
    .filter_map(|(x, y)| {
        let half = TEMPLE_HALF_SIZE_IN_SPANS * span;
        pixels.integral.mean(pixels.around(x, y, half, half))
    })
    .fold(f32::NEG_INFINITY, f32::max);
    if temples < eyes + MIN_TEMPLE_CONTRAST_RATIO * contrast {
        return None;
    }
    let mouth = mouth_of(blobs, mid_x, mid_y, span)?;

    // The score is what was measured about this reading of the pixels: how alike the two
    // eyes are in size, how level they sit, how far the bridge between them stands above
    // them in intensity, and how near the mouth is to where the proportions put it.
    let balance = 1.0 - (larger - smaller) / (larger + smaller);
    let level = 1.0 - (dy.abs() / (MAX_EYE_LEVEL_RATIO * span)).clamp(0.0, 1.0);
    let standing = ((bridge - eyes) / FULL_FEATURE_CONTRAST).clamp(0.0, 1.0);
    let expected_mouth = mid_y + MOUTH_BELOW_EYES * span;
    let mouth_fit =
        1.0 - ((mouth.cy - expected_mouth).abs() / (MOUTH_FIT_IN_SPANS * span)).clamp(0.0, 1.0);
    let confidence = (100.0 * (balance + level + standing + mouth_fit) / 4.0).clamp(0.0, 100.0);
    if confidence < MIN_CONFIDENCE {
        return None;
    }

    Some(Candidate {
        rect,
        confidence,
        left_eye: (left.cx, left.cy),
        right_eye: (right.cx, right.cy),
        mouth,
        roll: dy.atan2(dx).to_degrees().clamp(-180.0, 180.0),
    })
}

/// What fraction of a face box's own pixels are that face's SKIN: those within
/// [`MAX_SKIN_DEVIATION`] of `contrast` of the `skin` tone measured on the face.
///
/// Sampled on a bounded grid — at most [`SKIN_SAMPLES`] points on a side — so the cost is a
/// constant whatever the size of the box.
fn skin_coverage(pixels: &Pixels, rect: PixelRect, skin: f32, contrast: f32) -> f32 {
    if rect.is_empty() {
        return 0.0;
    }
    let band = MAX_SKIN_DEVIATION * contrast;
    let step_x = ((rect.x1 - rect.x0) / SKIN_SAMPLES).max(1);
    let step_y = ((rect.y1 - rect.y0) / SKIN_SAMPLES).max(1);
    let mut skin_like = 0_u32;
    let mut count = 0_u32;
    let mut y = rect.y0;
    while y < rect.y1 {
        let mut x = rect.x0;
        while x < rect.x1 {
            if (pixels.smoothed_at(x, y) - skin).abs() <= band {
                skin_like += 1;
            }
            count += 1;
            x += step_x;
        }
        y += step_y;
    }
    if count == 0 {
        return 0.0;
    }
    skin_like as f32 / count as f32
}

/// Whether a face box differs in mean intensity from the ring around it — figure against
/// ground. A box at the frame's edge has no ring to measure, and nothing unmeasured is held
/// against it.
fn stands_out(pixels: &Pixels, rect: &FaceRect, span: f32) -> bool {
    let margin = 0.25 * span;
    let inner = pixels.around(
        rect.left + rect.width / 2.0,
        rect.top + rect.height / 2.0,
        0.35 * rect.width,
        0.35 * rect.height,
    );
    let around = pixels.rect((
        rect.left - margin,
        rect.top - margin,
        rect.right() + margin,
        rect.bottom() + margin,
    ));
    let box_rect = pixels.rect(rect.bounds());
    let Some(interior) = pixels.integral.mean(inner) else {
        return false;
    };
    let ring_area = around.area().saturating_sub(box_rect.area());
    // Too little of the ring is inside the frame to measure a mean from.
    if ring_area * 10 < box_rect.area() {
        return true;
    }
    let ring = (pixels.integral.sum(around) - pixels.integral.sum(box_rect)) as f32
        / ring_area as f32;
    (interior - ring).abs() >= MIN_REGION_CONTRAST
}

/// The mouth of the face a pair of eyes stands for: the feature blob nearest to where the
/// proportions put one, out of those wide enough and placed to be a mouth at all.
fn mouth_of(blobs: &[Blob], mid_x: f32, mid_y: f32, span: f32) -> Option<Mouth> {
    let expected = mid_y + MOUTH_BELOW_EYES * span;
    blobs
        .iter()
        .filter(|blob| {
            (blob.cx - mid_x).abs() <= MAX_MOUTH_OFFSET_IN_SPANS * span
                && blob.cy >= mid_y + MIN_MOUTH_BELOW_EYES * span
                && blob.cy <= mid_y + MAX_MOUTH_BELOW_EYES * span
                && blob.width() >= MIN_MOUTH_WIDTH_IN_SPANS * span
                && blob.width() <= MAX_MOUTH_WIDTH_IN_SPANS * span
        })
        .min_by(|left, right| {
            (left.cy - expected)
                .abs()
                .total_cmp(&(right.cy - expected).abs())
        })
        .map(|blob| Mouth {
            x0: blob.rect.x0 as f32,
            x1: (blob.rect.x1.saturating_sub(1)) as f32,
            cy: blob.cy,
        })
}

/// The region a reading REPORTS: its box as far as the image holds it. A box partly outside
/// the frame was measured only where there are pixels to measure, and the part of it the
/// image carries is both what is answered and what stands for the face here.
fn reported_region(pixels: &Pixels, rect: &FaceRect) -> FaceRect {
    let left = rect.left.clamp(0.0, pixels.width_f32());
    let top = rect.top.clamp(0.0, pixels.height_f32());
    let right = rect.right().clamp(0.0, pixels.width_f32());
    let bottom = rect.bottom().clamp(0.0, pixels.height_f32());
    FaceRect {
        left,
        top,
        width: (right - left).max(0.0),
        height: (bottom - top).max(0.0),
    }
}

/// Whether the image holds the whole of a reading's box, so that reading measured all of
/// the face it stands for rather than the part of it the frame carries. `region` is the
/// reading's own [`reported_region`].
fn measured_whole(reading: &Candidate, region: &FaceRect) -> bool {
    reading.rect.area() > 0.0 && region.area() >= MIN_WHOLE_BOX_FRACTION * reading.rect.area()
}

/// Whether two reported regions stand for ONE face: they either share more than
/// [`MAX_OVERLAP`] of what they cover together, or the smaller of them lies more than
/// [`MAX_SHARED_FRACTION`] inside the larger — one face read at two scales, not two faces.
fn same_face(left: &FaceRect, right: &FaceRect) -> bool {
    let shared = shared_area(left, right);
    let union = left.area() + right.area() - shared;
    let smaller = left.area().min(right.area());
    (union > 0.0 && shared / union > MAX_OVERLAP)
        || (smaller > 0.0 && shared / smaller > MAX_SHARED_FRACTION)
}

/// How much two face boxes share, in the units their sides are in.
fn shared_area(left: &FaceRect, right: &FaceRect) -> f32 {
    let shared_width = (left.right().min(right.right()) - left.left.max(right.left)).max(0.0);
    let shared_height = (left.bottom().min(right.bottom()) - left.top.max(right.top)).max(0.0);
    shared_width * shared_height
}

/// The `FaceDetail` of one detected face: the members this detector measured, and no other.
fn detail(pixels: &Pixels, face: &Candidate) -> FaceDetail {
    let (width, height) = (pixels.width_f32(), pixels.height_f32());
    // A `BoundingBox` is expressed in fractions of the image, and only the part of the box
    // inside the image was measured, so the fractions are clamped to it.
    let left = (face.rect.left / width).clamp(0.0, 1.0);
    let top = (face.rect.top / height).clamp(0.0, 1.0);
    let right = (face.rect.right() / width).clamp(0.0, 1.0);
    let bottom = (face.rect.bottom() / height).clamp(0.0, 1.0);
    let box_rect = pixels.rect(face.rect.bounds());
    FaceDetail {
        bounding_box: Some(BoundingBox {
            left: Some(left),
            top: Some(top),
            width: Some(right - left),
            height: Some(bottom - top),
        }),
        confidence: Some(face.confidence),
        landmarks: Some(vec![
            landmark(LANDMARK_EYE_LEFT, face.left_eye, pixels),
            landmark(LANDMARK_EYE_RIGHT, face.right_eye, pixels),
            landmark(LANDMARK_MOUTH_LEFT, (face.mouth.x0, face.mouth.cy), pixels),
            landmark(LANDMARK_MOUTH_RIGHT, (face.mouth.x1, face.mouth.cy), pixels),
        ]),
        pose: Some(Pose {
            roll: Some(face.roll),
            // The angle of the eye line is what two eye positions establish; a yaw or a
            // pitch would need a third dimension nothing here measured.
            yaw: None,
            pitch: None,
        }),
        quality: Some(ImageQuality {
            brightness: Some(brightness(pixels, box_rect)),
            sharpness: Some(sharpness(pixels, box_rect)),
        }),
        // Every other member of a `FaceDetail` is a classifier's judgement, and no
        // classifier runs here: see this module's docs.
        ..FaceDetail::default()
    }
}

/// One measured landmark, as the fractions of the image a `Landmark` is expressed in.
fn landmark(kind: &str, at: (f32, f32), pixels: &Pixels) -> Landmark {
    Landmark {
        r#type: Some(kind.to_string()),
        x: Some((at.0 / pixels.width_f32()).clamp(0.0, 1.0)),
        y: Some((at.1 / pixels.height_f32()).clamp(0.0, 1.0)),
    }
}

/// The mean intensity of a face's own pixels, as a percentage of the brightest a pixel can
/// be: what `Quality.Brightness` reports, measured rather than assumed.
fn brightness(pixels: &Pixels, rect: PixelRect) -> f32 {
    pixels
        .integral
        .mean(rect)
        .map_or(0.0, |mean| (mean / 255.0 * 100.0).clamp(0.0, 100.0))
}

/// The ROOT-MEAN-SQUARE gradient magnitude across a face's own pixels, as a percentage of
/// the steepest a gradient can be: what `Quality.Sharpness` reports. Blur spreads an edge
/// over more pixels without adding any contrast to it, which leaves the mean gradient of a
/// region almost unchanged but its gradient ENERGY lower, so the root mean square is what
/// separates a crisp picture of a face from a soft one. The scale is this one, not a scale
/// AWS publishes, because none is published.
fn sharpness(pixels: &Pixels, rect: PixelRect) -> f32 {
    (rms_gradient(pixels, rect, SHARPNESS_SAMPLES) / 255.0 * 100.0).clamp(0.0, 100.0)
}

/// The root-mean-square gradient magnitude over `rect`, in grey levels: how much the pixels
/// there change from one to the next, which is what tells a smooth region from a textured
/// one and a crisp edge from a blurred one.
///
/// Sampled on a bounded grid — at most `samples` points on a side — so the cost is a
/// constant whatever the size of the region.
fn rms_gradient(pixels: &Pixels, rect: PixelRect, samples: usize) -> f32 {
    // Central differences need a pixel on either side, so the sampled area is the region
    // without its outermost row and column, inside the image's own border.
    let x0 = rect.x0.max(1);
    let y0 = rect.y0.max(1);
    let x1 = rect.x1.min(pixels.width.saturating_sub(1));
    let y1 = rect.y1.min(pixels.height.saturating_sub(1));
    if x1 <= x0 || y1 <= y0 {
        return 0.0;
    }
    let step_x = ((x1 - x0) / samples.max(1)).max(1);
    let step_y = ((y1 - y0) / samples.max(1)).max(1);
    let mut total = 0.0_f32;
    let mut count = 0_u32;
    let mut y = y0;
    while y < y1 {
        let mut x = x0;
        while x < x1 {
            // Halved, because a central difference spans two pixels.
            let horizontal = (pixels.decoded_at(x + 1, y) - pixels.decoded_at(x - 1, y)) / 2.0;
            let vertical = (pixels.decoded_at(x, y + 1) - pixels.decoded_at(x, y - 1)) / 2.0;
            total += horizontal * horizontal + vertical * vertical;
            count += 1;
            x += step_x;
        }
        y += step_y;
    }
    if count == 0 {
        return 0.0;
    }
    (total / count as f32).sqrt()
}
