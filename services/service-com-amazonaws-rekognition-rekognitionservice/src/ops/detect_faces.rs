// SPDX-License-Identifier: CC0-1.0

//! @op DetectFaces
//!
//! ## What this handler does
//!
//! `DetectFaces` answers with the faces the submitted image carries, and with nothing
//! else: the image's bytes are the request's own inline `Bytes` or the object this
//! emulator's Amazon S3 holds at the `S3Object` it named
//! ([`crate::state::image_source`]), and the faces are MEASURED off the decoded pixels by
//! [`crate::state::face_detection`], which reports a `BoundingBox`, a `Confidence`,
//! `Landmarks`, a `Pose.Roll` and a `Quality` it measured and leaves absent every member
//! only a trained classifier could have judged. An image carrying no face structure is
//! answered with an empty `FaceDetails`, which is what was measured, and a refusal is
//! reserved for an image this service could not read at all.
//!
//! It PERSISTS NOTHING, and reads no stored resource: "this is a stateless API operation.
//! That is, the operation does not persist any data" (the operation's own documentation,
//! below). Its whole answer is a function of the submitted image, so the store this
//! service keeps for collections is neither read nor written here, and two identical
//! calls answer identically without either of them having changed anything.
//!
//! `Attributes` asks for facial attributes beyond the default subset (an age range, a
//! gender, an emotion, eyeglasses, …). Every one of them is a classifier's judgement and
//! no classifier runs in this process, so none is reported for any value of that member —
//! absent rather than guessed. See [`crate::state::face_detection`] for why.
//!
//! `OrientationCorrection` is absent because the member's own documentation says it is:
//! "the value of OrientationCorrection is always null".
//!
//! ---
//!
//! # Modeled contract for `DetectFaces`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-rekognition-rekognitionservice/smithy-model/com-amazonaws-rekognition-rekognitionservice.json")
//! ```
//!
//! Detects faces within an image that is provided as input. DetectFaces detects the 100
//! largest faces in the image. For each face detected, the operation returns face details.
//! These details include a bounding box of the face, a confidence value (that the bounding
//! box contains a face), and a fixed set of attributes such as facial landmarks (for
//! example, coordinates of eye and mouth), pose, presence of facial occlusion, and so on.
//! The face-detection algorithm is most effective on frontal faces. For non-frontal or
//! obscured faces, the algorithm might not detect the faces or might detect faces with
//! lower confidence. You pass the input image either as base64-encoded image bytes or as a
//! reference to an image in an Amazon S3 bucket. If you use the AWS CLI to call Amazon
//! Rekognition operations, passing image bytes is not supported. The image must be either a
//! PNG or JPEG formatted file. This is a stateless API operation. That is, the operation
//! does not persist any data. This operation requires permissions to perform the
//! rekognition:DetectFaces action.
//!
//! ## Input
//!
//! * `Attributes` (Attributes) — An array of facial attributes you want to be returned. A
//!   DEFAULT subset of facial attributes - BoundingBox, Confidence, Pose, Quality, and
//!   Landmarks - will always be returned. You can request for specific facial attributes (in
//!   addition to the default list) - by using ["DEFAULT", "FACE_OCCLUDED"] or just
//!   ["FACE_OCCLUDED"]. You can request for all facial attributes by using ["ALL"].
//!   Requesting more attributes may increase response time. If you provide both, ["ALL",
//!   "DEFAULT"], the service uses a logical "AND" operator to determine which attributes to
//!   return (in this case, all attributes). Note that while the FaceOccluded and EyeDirection
//!   attributes are supported when using DetectFaces, they …
//! * `Image` (Image) **required** — The input image as base64-encoded bytes or an S3 object.
//!   If you use the AWS CLI to call Amazon Rekognition operations, passing base64-encoded
//!   image bytes is not supported. If you are using an AWS SDK to call Amazon Rekognition,
//!   you might not need to base64-encode image bytes passed using the Bytes field. For more
//!   information, see Images in the Amazon Rekognition developer guide.
//!
//! ## Output
//!
//! * `FaceDetails` (FaceDetailList) — Details of each face found in the image.
//! * `OrientationCorrection` (OrientationCorrection, one of: ROTATE_0 | ROTATE_180 |
//!   ROTATE_270 | ROTATE_90) — The value of OrientationCorrection is always null. If the
//!   input image is in .jpeg format, it might contain exchangeable image file format (Exif)
//!   metadata that includes the image's orientation. Amazon Rekognition uses this orientation
//!   information to perform image correction. The bounding box coordinates are translated to
//!   represent object locations after the orientation information in the Exif metadata is
//!   used to correct the image orientation. Images in .png format don't contain Exif
//!   metadata. Amazon Rekognition doesn’t perform image correction for images in .png format
//!   and .jpeg images without orientation information in the image Exif metadata. The
//!   bounding box coordinates aren't translated …
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

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DetectFacesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-rekognition-rekognitionservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DetectFacesError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.rekognition#DetectFaces")]
#[derive(Debug)]
pub enum DetectFacesErrorModeled {
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
/// (`crate::ops::detect_faces::DetectFacesError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DetectFacesError = crate::scaffolded::Scaffolded<DetectFacesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DetectFacesErrorModeled> for DetectFacesError {
    fn from(modeled: DetectFacesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DetectFaces` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DetectFaces<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DetectFaces<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DetectFaces<Eng>
{
    type Input = DetectFacesInput;
    type Output = DetectFacesOutput;
    type Error = DetectFacesError;

    const NAME: &'static str = "DetectFaces";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DETECT_FACES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DETECT_FACES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let faces = if let Some(provider) =
            crate::state::vision_provider::configured_endpoint(&*self.engine)
                .map_err(unreadable_error)?
        {
            let bytes = crate::state::image_source::image_bytes(ctx, &input.image)
                .await
                .map_err(unreadable_error)?;
            provider
                .detect_faces(&bytes, input.attributes.as_deref())
                .await
                .map_err(unreadable_error)?
        } else {
            crate::state::face_detection::faces_in_submitted_image(ctx, &input.image)
                .await
                .map_err(unreadable_error)?
        };
        Ok(DetectFacesOutput {
            // The faces that were found, which is an empty list for an image carrying
            // none — the answer AWS gives for an image with no face in it.
            face_details: Some(faces),
            // "The value of OrientationCorrection is always null."
            orientation_correction: None,
        })
    }
}

/// The modelled answer for an image whose faces could not be detected: one modelled error
/// per reason, each carrying the reason's own message.
fn unreadable_error(reason: crate::state::image_source::UnreadableImage) -> DetectFacesError {
    use crate::state::image_source::UnreadableImage;

    let message = reason.message().to_string();
    match reason {
        UnreadableImage::InvalidFormat(_) => DetectFacesErrorModeled::InvalidImageFormatException(
            InvalidImageFormatException::with_message(message),
        ),
        UnreadableImage::TooLarge(_) => DetectFacesErrorModeled::ImageTooLargeException(
            ImageTooLargeException::with_message(message),
        ),
        UnreadableImage::S3Unreadable(_) => DetectFacesErrorModeled::InvalidS3ObjectException(
            InvalidS3ObjectException::with_message(message),
        ),
        UnreadableImage::InvalidParameter(_) => DetectFacesErrorModeled::InvalidParameterException(
            InvalidParameterException::with_message(message),
        ),
        // Nothing was established about the image itself, so this is the server fault it
        // is rather than anything about the request.
        UnreadableImage::NotAnalysed(_) => DetectFacesErrorModeled::InternalServerError(
            InternalServerError::with_message(message),
        ),
    }
    .into()
}
