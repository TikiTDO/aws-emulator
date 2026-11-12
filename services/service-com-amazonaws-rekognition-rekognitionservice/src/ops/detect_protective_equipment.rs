// SPDX-License-Identifier: CC0-1.0

//! @op DetectProtectiveEquipment
//!
//! ## What this handler does
//!
//! `DetectProtectiveEquipment` answers with the PERSONS the submitted image carries and with
//! what was detected about each of them. The image's bytes are the request's own inline
//! `Bytes` or the object this emulator's Amazon S3 holds at the `S3Object` it named
//! ([`crate::state::image_source`]), and a person is located by the geometric face detector
//! in [`crate::state::face_detection`] — the same detector `DetectFaces` reports — since a
//! face measured in the frame is a person detected in it. One answer covers the
//! [`MAX_PERSONS`] largest of them, because the operation documents that it "detects PPE worn
//! by up to 15 persons detected in an image", and the detector reports the faces it found
//! largest first. An image carrying no person is answered with an empty `Persons`, which is
//! what was measured, and a refusal is reserved for an image this service could not read at
//! all.
//!
//! It PERSISTS NOTHING, and reads no stored resource: "this is a stateless API operation.
//! That is, the operation does not persist any data" (the operation's own documentation,
//! below). The whole answer is a function of the submitted image, so two identical calls
//! answer identically without either of them having changed anything, and the store this
//! service keeps for collections is neither read nor written here.
//!
//! Each person reports the `Id` this call numbers it by — "the identifier is only unique for
//! a single call to DetectProtectiveEquipment" — and the `Confidence` the detector measured
//! for the face that located it, which is its own measured agreement that a person is there.
//!
//! ## What is not reported, and why
//!
//! * A person's `BoundingBox` is "a bounding box around the detected person", and where a
//!   person EXTENDS is not what a face detector measured: the face is one part of the person,
//!   not the person's reach, so no box is reported rather than the face's box relabelled as
//!   the person's. [`crate::state::label_detection`] leaves the `Person` label's extent
//!   unlocated for the same reason.
//! * `BodyParts` is "an array of body parts detected on a person's body", and this detector
//!   measures the FACE of a person and nothing else, so the face is the one body part
//!   reported — with the confidence measured for it. A head and two hands are structures
//!   nothing here measures, so neither is reported as detected.
//! * `EquipmentDetections` around a body part, an item's `Type` and whether it `CoversBodyPart`
//!   are a trained detector's judgement, and no such detector runs in this process (see
//!   [`crate::state`]). The member is therefore ABSENT rather than an empty list, which would
//!   claim the part was examined and carried nothing.
//!
//! ## The summary
//!
//! `Summary` answers only a request that asked for one: it carries "summary information for
//! the types of PPE specified in the SummarizationAttributes input parameter", so a request
//! naming no summarization attributes is answered without it.
//!
//! The summary covers the persons whose measured detection confidence reaches the request's
//! own floor — "Amazon Rekognition doesn't return summary information with a confidence
//! [lower] than this specified value", that floor never being under
//! [`MIN_SUMMARY_CONFIDENCE`] — and reports each of them on the one side a determination
//! nothing made belongs on: `PersonsIndeterminate`, "an array of IDs for persons where it was
//! not possible to determine if they are wearing personal protective equipment". No person is
//! reported as wearing the required equipment or as not wearing it, because each of those is
//! a claim about items nothing in this process detected.
//!
//! ---
//!
//! # Modeled contract for `DetectProtectiveEquipment`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-rekognition-rekognitionservice/smithy-model/com-amazonaws-rekognition-rekognitionservice.json")
//! ```
//!
//! Detects Personal Protective Equipment (PPE) worn by people detected in an image. Amazon
//! Rekognition can detect the following types of PPE. Face cover Hand cover Head cover You
//! pass the input image as base64-encoded image bytes or as a reference to an image in an
//! Amazon S3 bucket. The image must be either a PNG or JPG formatted file.
//! DetectProtectiveEquipment detects PPE worn by up to 15 persons detected in an image. For
//! each person detected in the image the API returns an array of body parts (face, head,
//! left-hand, right-hand). For each body part, an array of detected items of PPE is
//! returned, including an indicator of whether or not the PPE covers the body part. The API
//! returns the confidence it has in each detection (person, PPE, body part and body part
//! coverage). It also returns a bounding box (BoundingBox) for each detected person and
//! each detected item of PPE. You can optionally request a summary of detected PPE items
//! with the SummarizationAttributes input parameter. The summary provides the following
//! information. The persons detected as wearing all of the types of PPE that you specify.
//! The persons detected as not wearing all of the types PPE that you specify. The persons
//! detected where PPE adornment could not be determined. This is a stateless API operation.
//! That is, the operation does not persist any data. This operation requires permissions to
//! perform the rekognition:DetectProtectiveEquipment action.
//!
//! ## Input
//!
//! * `Image` (Image) **required** — The image in which you want to detect PPE on detected
//!   persons. The image can be passed as image bytes or you can reference an image stored in
//!   an Amazon S3 bucket.
//! * `SummarizationAttributes` (ProtectiveEquipmentSummarizationAttributes) — An array of PPE
//!   types that you want to summarize.
//!
//! ## Output
//!
//! * `Persons` (ProtectiveEquipmentPersons) — An array of persons detected in the image
//!   (including persons not wearing PPE).
//! * `ProtectiveEquipmentModelVersion` (String) — The version number of the PPE detection
//!   model used to detect PPE in the image.
//! * `Summary` (ProtectiveEquipmentSummary) — Summary information for the types of PPE
//!   specified in the SummarizationAttributes input parameter.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 400) — You are not authorized to perform the action.
//! * `ImageTooLargeException` (HTTP 400) — The input image size exceeds the allowed limit. If
//!   you are calling DetectProtectiveEquipment, the image size or resolution exceeds the
//!   allowed limit. For more information, see Guidelines and quotas in Amazon Rekognition in
//!   the Amazon Rekognition Developer Guide.
//! * `InternalServerError` (HTTP 500) — Amazon Rekognition experienced a service issue. Try
//!   your call again.
//! * `InvalidImageFormatException` (HTTP 400) — The provided image format is not supported.
//! * `InvalidParameterException` (HTTP 400) — Input parameter violated a constraint. Validate
//!   your parameter before calling the API operation again.
//! * `InvalidS3ObjectException` (HTTP 400) — Amazon Rekognition is unable to access the S3
//!   object specified in the request.
//! * `ProvisionedThroughputExceededException` (HTTP 400) — The number of requests exceeded
//!   your throughput limit. If you want to increase this limit, contact Amazon Rekognition.
//! * `ThrottlingException` (HTTP 500) — Amazon Rekognition is temporarily unable to process
//!   the request. Try your call again.

#[allow(unused_imports)]
use super::types::*;

// Every modelled error this handler answers with is built through this trait, so each
// one carries its `Code` member beside its `Message`.
use crate::state::modeled_error::ModeledError;

/// The most persons one answer covers: "DetectProtectiveEquipment detects PPE worn by up to
/// 15 persons detected in an image" (the operation's own documentation, below). The detector
/// reports the faces it measured largest first, so the persons past the cap are the ones
/// located by the smallest faces.
pub const MAX_PERSONS: usize = 15;

/// The version of the PPE detection model behind an answer, reported in
/// `ProtectiveEquipmentModelVersion`. It is an opaque string on the wire and what runs here
/// is one generation of one detector, so one value is pinned for it — the way the label
/// detector pins its own ([`crate::state::label_detection::LABEL_MODEL_VERSION`]).
pub const PROTECTIVE_EQUIPMENT_MODEL_VERSION: &str = "1.0";

/// The one body part this detector measures, as the model's `BodyPart` enum names it
/// (`super::types::BODY_PART_VALUES`).
pub const MEASURED_BODY_PART: &str = "FACE";

/// The confidence floor a summary cannot go below, whatever the request asks for: "specify a
/// MinConfidence value that is between 50-100% as DetectProtectiveEquipment returns
/// predictions only where the detection confidence is between 50% - 100%. If you specify a
/// value that is less than 50%, the results are the same specifying a value of 50%" (the
/// member's own documentation).
pub const MIN_SUMMARY_CONFIDENCE: f32 = 50.0;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DetectProtectiveEquipmentError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-rekognition-rekognitionservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DetectProtectiveEquipmentError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.rekognition#DetectProtectiveEquipment")]
#[derive(Debug)]
pub enum DetectProtectiveEquipmentErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ImageTooLargeException(ImageTooLargeException),
    InternalServerError(InternalServerError),
    InvalidImageFormatException(InvalidImageFormatException),
    InvalidParameterException(InvalidParameterException),
    InvalidS3ObjectException(InvalidS3ObjectException),
    ProvisionedThroughputExceededException(ProvisionedThroughputExceededException),
    ThrottlingException(ThrottlingException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::detect_protective_equipment::DetectProtectiveEquipmentError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DetectProtectiveEquipmentError = crate::scaffolded::Scaffolded<DetectProtectiveEquipmentErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DetectProtectiveEquipmentErrorModeled> for DetectProtectiveEquipmentError {
    fn from(modeled: DetectProtectiveEquipmentErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DetectProtectiveEquipment` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DetectProtectiveEquipment<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DetectProtectiveEquipment<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DetectProtectiveEquipment<Eng>
{
    type Input = DetectProtectiveEquipmentInput;
    type Output = DetectProtectiveEquipmentOutput;
    type Error = DetectProtectiveEquipmentError;

    const NAME: &'static str = "DetectProtectiveEquipment";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DETECT_PROTECTIVE_EQUIPMENT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DETECT_PROTECTIVE_EQUIPMENT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Stateless: "the operation does not persist any data", and the whole answer is
        // measured off the image the request submitted, so there is no stored resource to
        // read or write and the engine this operation was constructed with goes unused.
        // `ctx` is what the S3-sourced read of the image goes through.
        let _ = &self.engine;
        let faces = crate::state::face_detection::faces_in_submitted_image(ctx, &input.image)
            .await
            .map_err(unreadable_error)?;
        // "The API returns all persons detected in the input image": the request's own
        // `MinConfidence` narrows the SUMMARY, never this list.
        let persons: Vec<ProtectiveEquipmentPerson> = faces
            .iter()
            .take(MAX_PERSONS)
            .enumerate()
            .map(|(index, face)| detected_person(index, face))
            .collect();
        Ok(DetectProtectiveEquipmentOutput {
            // The model behind the detection, which ran whatever it found.
            protective_equipment_model_version: Some(
                PROTECTIVE_EQUIPMENT_MODEL_VERSION.to_string(),
            ),
            // "Summary information for the types of PPE specified in the
            // SummarizationAttributes input parameter": a request that specified none is
            // answered without a summary, since there is nothing it asked to be summarized.
            summary: input
                .summarization_attributes
                .as_ref()
                .map(|attributes| summary(&persons, attributes)),
            // Empty for an image carrying no person, which is what was measured.
            persons: Some(persons),
        })
    }
}

/// The person a measured face locates: the id this call numbers it by, the confidence the
/// detector measured for that face, and the one body part the detector measured. See this
/// module's docs for the members left absent.
fn detected_person(index: usize, face: &FaceDetail) -> ProtectiveEquipmentPerson {
    ProtectiveEquipmentPerson {
        body_parts: Some(vec![ProtectiveEquipmentBodyPart {
            name: Some(MEASURED_BODY_PART.to_string()),
            // "The confidence that Amazon Rekognition has in the detection accuracy of the
            // detected body part", which for this part is the detector's own measurement.
            confidence: face.confidence,
            // No item detector runs here, so nothing was detected around the part and
            // nothing is claimed about it.
            equipment_detections: None,
        }]),
        // Where the person extends is not what a face detector measured.
        bounding_box: None,
        confidence: face.confidence,
        // "The identifier is only unique for a single call": the person's position in this
        // call's own answer. The cap above keeps the count far inside the member's range.
        id: i32::try_from(index).ok(),
    }
}

/// The summary a request asked for: which of the persons detected were wearing the required
/// equipment, which were not, and for which it could not be determined.
///
/// A person is covered by the summary only when the confidence measured for their detection
/// reaches the request's own floor, which is never under [`MIN_SUMMARY_CONFIDENCE`]. Whether
/// a covered person wears a required item is a judgement about the ITEMS around a body part,
/// which nothing in this process detects, so each of them is reported on the side the
/// operation itself keeps for a person whose PPE adornment could not be determined.
fn summary(
    persons: &[ProtectiveEquipmentPerson],
    attributes: &ProtectiveEquipmentSummarizationAttributes,
) -> ProtectiveEquipmentSummary {
    let floor = attributes.min_confidence.max(MIN_SUMMARY_CONFIDENCE);
    let indeterminate: Vec<i32> = persons
        .iter()
        .filter(|person| {
            person
                .confidence
                .is_some_and(|confidence| confidence >= floor)
        })
        .filter_map(|person| person.id)
        .collect();
    ProtectiveEquipmentSummary {
        // "An array of IDs for persons who are wearing detected personal protective
        // equipment": no equipment was detected, so this names nobody.
        persons_with_required_equipment: Some(Vec::new()),
        // Not wearing a required type is equally a determination about undetected items, so
        // nobody is reported on this side either.
        persons_without_required_equipment: Some(Vec::new()),
        persons_indeterminate: Some(indeterminate),
    }
}

/// The modelled answer for an image this service could not analyse: one modelled error per
/// reason, each carrying the reason's own message. The reason type and its wording are shared
/// (`crate::state::image_source`); the mapping is written per operation because each
/// operation's error enum is its own type, generated from the errors the model declares for
/// it, so there is no one enum for a shared function to return.
fn unreadable_error(
    reason: crate::state::image_source::UnreadableImage,
) -> DetectProtectiveEquipmentError {
    use crate::state::image_source::UnreadableImage;

    let message = reason.message().to_string();
    match reason {
        UnreadableImage::InvalidFormat(_) => {
            DetectProtectiveEquipmentErrorModeled::InvalidImageFormatException(
                InvalidImageFormatException::with_message(message),
            )
        }
        UnreadableImage::TooLarge(_) => {
            DetectProtectiveEquipmentErrorModeled::ImageTooLargeException(
                ImageTooLargeException::with_message(message),
            )
        }
        UnreadableImage::S3Unreadable(_) => {
            DetectProtectiveEquipmentErrorModeled::InvalidS3ObjectException(
                InvalidS3ObjectException::with_message(message),
            )
        }
        UnreadableImage::InvalidParameter(_) => {
            DetectProtectiveEquipmentErrorModeled::InvalidParameterException(
                InvalidParameterException::with_message(message),
            )
        }
        // Nothing was established about the image itself, so this is the server fault it is
        // rather than anything about the request.
        UnreadableImage::NotAnalysed(_) => {
            DetectProtectiveEquipmentErrorModeled::InternalServerError(
                InternalServerError::with_message(message),
            )
        }
    }
    .into()
}
