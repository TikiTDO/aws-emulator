// SPDX-License-Identifier: CC0-1.0

//! Reading the text out of a RASTER document: the pixel side of
//! [`super::read_content`].
//!
//! Textract accepts a document "in PNG, JPEG, PDF, or TIFF format"; a raster in one of
//! those formats carries its text as ink rather than as characters, so it has to be
//! recognised. Nothing here is echoed from the request, keyed to a digest or
//! randomised: every line, word and character this module reports was measured off the
//! submitted pixels, and ink it cannot match to a glyph is reported as nothing at all.
//!
//! ## The pipeline
//!
//! 1. **Decode** — the `image` crate decodes the submitted bytes for the format its
//!    magic bytes name, bounded by [`MAX_RASTER_SIDE`], [`MAX_RASTER_PIXELS`] and
//!    [`MAX_RASTER_ALLOC`] so one request cannot turn a small compressed file into
//!    unbounded work.
//! 2. **Binarise** — Otsu's method (N. Otsu, 1979: the threshold that maximises the
//!    between-class variance of the grey histogram) splits the page into ink and
//!    background. Ink is the SMALLER of the two classes, so a light page with dark text
//!    and a dark page with light text are both read; two classes closer together than
//!    [`MIN_INK_CONTRAST`] are sensor or compression noise on a blank page, not ink.
//! 3. **Segment** — a horizontal projection profile of the ink gives the page's text
//!    lines; a vertical profile within one line gives its glyph candidates, and the way
//!    those candidates are spaced — against the cell a monospaced line sets them in, or
//!    against the pitch a proportional one advances by — gives the word and column breaks
//!    ([`Separators`]).
//! 4. **Recognise** — each candidate is resampled onto an 8x8 grid and scored against
//!    every printable ASCII glyph of the embedded public-domain 8x8 font (`font8x8`) by
//!    shape overlap (the Sørensen-Dice coefficient) plus the glyph's height, width and
//!    position inside its line. The best match above [`MIN_GLYPH_SCORE`] is the
//!    character; the mean winning score is the `Confidence` the blocks report, because
//!    it is the only confidence anything here actually measured — per word, per line and
//!    per page, each over the characters that block was read from.
//! 5. **Orient and separate** — steps 3 and 4 are run over the page as submitted AND over
//!    each of its quarter turns ([`Ink::turned`]), because a document can be scanned or
//!    photographed turned. The turn whose marks recognise best is the one the characters
//!    were drawn at, and the angle it was read at is what the `WORD` blocks report as
//!    `Geometry$RotationAngle`. At that turn the page is read once more every way something
//!    that is NOT a character can be joining the marks of a line — a rule drawn across it,
//!    a panel of light text on a dark ground ([`Ink::variants`]) — and again the reading
//!    whose marks recognise best is the one reported.
//! 6. **Measure** — every line and every word also reports WHERE its ink was found, as
//!    the fraction of the SUBMITTED page it covers ([`MeasuredLine`], [`MeasuredWord`],
//!    mapped back out of the turn it was read at by [`Orientation`]), so the `Geometry`
//!    the blocks carry is the position and the size the recogniser measured rather than a
//!    typographic grid. Each measurement is both the box around the ink and its
//!    [`Outline`] — the four extreme marks themselves — so a block can report the
//!    fine-grained polygon around its item as well as the coarse box. The page's own
//!    content is measured too ([`Recognised::content`]): every mark found on it, read as
//!    text or not.
//!
//! No maintained Rust crate performs offline OCR: the ones that exist either bind to a
//! C library or load neural-network weights from a file this emulator may not fetch, so
//! the recogniser itself is in-crate while the decoding (a format job) is the `image`
//! crate's.
//!
//! ## What it does not read
//!
//! One raster is ONE page. A multi-frame TIFF's later frames are not reported, because
//! the decoder hands this module a single image and a page nothing decoded is not a page
//! this service can describe; `DocumentMetadata.Pages` then says 1, which is what was
//! read. A PDF is not a raster and is not decoded here at all.
//!
//! ## Where it runs
//!
//! Every step here is bounded, but all of it is CPU-bound over a page whose size the
//! caller chose, so a request path reaches it through [`super::read_document`], which
//! runs it on tokio's BLOCKING pool: it occupies no async worker, and the
//! `spawn_blocking(..).await` that drives it is a real await point a timeout, a
//! cancellation or a shutdown can land on.
//!
//! One page is read several ways — a fixed four quarter turns, each read every way a rule
//! or a panel could be joining its marks, so at most a dozen readings — and each reading is
//! held to the same per-page ceilings ([`MAX_RASTER_PIXELS`], [`MAX_GLYPHS_PER_PAGE`]). The
//! work and the memory one request can cause therefore stay a fixed multiple of one page's,
//! which no request can choose.

use std::io::Cursor;
use std::sync::OnceLock;

use image::{ImageFormat, Limits};

use super::Unreadable;

/// The largest side, in pixels, of a raster this service decodes. NOT an AWS limit: an
/// emulator-side bound so one request cannot allocate without end.
pub const MAX_RASTER_SIDE: u32 = 10_000;

/// The most pixels one raster page may hold. NOT an AWS limit: see [`MAX_RASTER_SIDE`].
/// The scan is linear in the pixel count, so this bounds the per-request CPU too.
pub const MAX_RASTER_PIXELS: u64 = 8_000_000;

/// The most bytes the decoder may allocate for one raster. NOT an AWS limit: it is what
/// stops a small compressed file from expanding into a large allocation.
pub const MAX_RASTER_ALLOC: u64 = 96 * 1024 * 1024;

/// The most glyph candidates one page may hold. NOT an AWS limit: recognition costs one
/// comparison per candidate per template, so this bounds the work a single request can
/// cause.
pub const MAX_GLYPHS_PER_PAGE: usize = 10_000;

/// How far apart the two Otsu classes must be, in grey levels, for the darker one to be
/// ink. Below this the page holds one tone plus noise, which is a blank page.
const MIN_INK_CONTRAST: f32 = 24.0;

/// The score a glyph candidate must reach for its best-matching character to be
/// reported. Ink below it matched nothing, so it contributes no text.
const MIN_GLYPH_SCORE: f32 = 0.40;

/// How much of a candidate's ink must be SHARED with the template that matched it, as
/// the Sørensen-Dice coefficient of the two shapes.
///
/// A floor on the shape term alone, not just on the weighted total: the size and
/// position terms of [`score`] can carry up to 0.30 between them, so without this a mark
/// whose shape is nothing like any glyph could still clear [`MIN_GLYPH_SCORE`] and be
/// reported as a character. With it, ink that matches no glyph's shape contributes no
/// text — which is what this module promises.
const MIN_SHAPE_OVERLAP: f32 = 0.45;

/// How much better than the page as submitted a quarter turn must read for the page to be
/// reported as read at that turn, as a fraction of the recognition score. A turn that only
/// ties with the upright reading is a coincidence, not an orientation.
const MIN_ROTATION_GAIN: f32 = 0.05;

/// How much of the line's own character pitch a gap must cover to separate two WORDS on a
/// PROPORTIONAL line whose gaps show no boundary of their own: a space leaves a gap of a
/// good part of a pitch, while a gap inside a word is a letter's side bearing, a small part
/// of one.
const WORD_GAP_PITCH: f32 = 0.4;

/// The least of the line's own character pitch a gap must cover to be a space at all, which
/// is what keeps the boundary between two clusters of BEARINGS from being read as one
/// between bearings and spaces.
const MIN_SPACE_PITCH: f32 = 0.25;

/// How many of the line's own character pitches a gap must cover to separate two COLUMNS
/// rather than two words on a PROPORTIONAL line.
const COLUMN_GAP_PITCH: f32 = 2.0;

/// How many cells a step must cover to separate two WORDS on a MONOSPACED line: a letter
/// advances one cell, so a space's step covers two.
const LATTICE_WORD_CELLS: f32 = 1.5;

/// How many cells a step must cover to separate two COLUMNS rather than two words on a
/// MONOSPACED line.
const LATTICE_COLUMN_CELLS: f32 = 3.0;

/// How far from a whole number of cells, in cells, a monospaced line's marks may sit on
/// average. A cell holds a glyph's ink wherever the glyph puts it, so its centre wanders
/// inside the cell; a line whose marks wander further than this sits on no lattice, which
/// is what a line whose every letter advances by its own width does.
const MAX_LATTICE_DRIFT: f32 = 0.08;

/// Marks a line must carry for a lattice fit to say anything about it: a line of two marks
/// has one step, and one step fits any cell.
const MIN_LATTICE_MARKS: usize = 3;

/// How far either side of the line's own pitch a lattice cell is searched for, in
/// sixty-fourths of that pitch: the median step of a monospaced line is its cell, give or
/// take where two glyphs put their ink inside it, and a cell free to be anything else
/// would fit a proportional line by coincidence.
const LATTICE_SEARCH: i32 = 8;

/// Rows a run of inked rows must span to be a line of text rather than speckle.
const MIN_LINE_HEIGHT: usize = 2;

/// How much of a band's own width one row's single run of ink must span for that row to be
/// a RULE drawn across the line rather than part of a character.
const RULE_ROW_SHARE: f32 = 0.6;

/// How many times wider than it is tall a band must be to be read as a line of characters
/// with something drawn ACROSS it.
///
/// A rule runs across a line, and a panel holds one, so both are longer than the line is
/// tall, while no Latin character is twice as wide as its own line: a band narrower than
/// this holds one mark of the page's own ink, whose bar, crossbar or counters are no rule
/// and no text.
const MIN_RULE_BAND_ASPECT: f32 = 2.0;

/// The least a filled region may measure on a side, in pixels, to be read as a PANEL of
/// light text: a shorter or narrower one holds no character.
const MIN_PANEL_SIDE: usize = 8;

/// How much of a line's height a column's ink may cover for that column to be a HAIRLINE:
/// a sliver of ink no character's body leaves, which is what a bridge between two
/// characters looks like.
const MAX_HAIRLINE_SHARE: f32 = 0.08;

/// How much of its own box a band's ink must fill for the band to be a filled PANEL rather
/// than a line of characters: characters leave most of their line blank, and even the
/// heaviest single character leaves more of its own box blank than this.
const MIN_PANEL_FILL: f32 = 0.6;

/// Marks the ground inside a filled region must fall into for the region to be read as a
/// panel of light text rather than as one mark of its own: the counters and notches a
/// single heavy character leaves are fewer than this.
const MIN_PANEL_MARKS: usize = 3;

/// A run no taller than this that inks more than half the page's width is a rule or a
/// border, not a line of text.
const RULE_MAX_HEIGHT: usize = 3;

/// The most indent characters a line's left margin is reported as, so a page-wide
/// margin cannot stretch one line into a long string.
const MAX_INDENT: usize = 100;

/// A raster page's recognised text.
#[derive(Debug, Clone, PartialEq)]
pub struct Recognised {
    /// The page's text, one line per line of ink the recogniser read.
    pub text: String,
    /// The mean score of the characters recognised, as a percentage inside `Percent`'s
    /// `0..=100` range. Zero when the page carries no recognised text.
    pub confidence: f32,
    /// WHERE the recognised text sits on the page, one entry per line of `text` and in
    /// the same order, each with the words of that line in reading order.
    ///
    /// Every rectangle here was MEASURED off the submitted pixels — a word is the extent
    /// of the glyph run the line's text separates with a space or a tab, and a line the
    /// box enclosing its words — so the geometry the blocks report describes the document
    /// rather than a page template.
    pub lines: Vec<MeasuredLine>,
    /// The angle, in degrees, the page's characters were read at: `0.0` for a page read as
    /// it was submitted, and the quarter turn its characters are turned by when the page
    /// had to be turned to read them.
    pub rotation: f32,
    /// The extent of the page's own CONTENT: every mark found on it, whether or not the
    /// mark was read as a character, so a rule, a border or a logo is part of the content
    /// of the page it is on. `None` on a page carrying no ink at all.
    pub content: Option<Marks>,
}

/// Where something sits on a page, in the page-relative fractions `BoundingBox` and
/// `Point` are expressed in: `0.0` is the page's left or top edge and `1.0` its right or
/// bottom edge.
#[derive(Debug, Clone, PartialEq)]
pub struct Rect {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    /// The whole page: what was examined when there is no ink to bound.
    pub const fn whole_page() -> Self {
        Self {
            left: 0.0,
            top: 0.0,
            width: 1.0,
            height: 1.0,
        }
    }
}

/// One point of a page, in the same page-relative fractions as [`Rect`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertex {
    pub x: f32,
    pub y: f32,
}

/// The OUTLINE of a set of marks: the four extreme marks of the set — the topmost, the
/// rightmost, the bottommost and the leftmost mark — as points on the submitted page.
///
/// Each of them is a mark that was really found, so the outline is inscribed in the box
/// around the same marks and touches each of its edges, and it follows the set's own skew
/// instead of restating the box's corners: a line of text whose last glyph sits lower
/// than its first outlines a slanted quadrilateral, while a solid rectangle of ink
/// outlines its own rectangle.
pub type Outline = [Vertex; 4];

/// Where a set of marks was found on the page: the box enclosing them and their outline.
///
/// `BoundingBox` is "an axis-aligned COARSE representation of the location of the
/// recognized item" and `Polygon` is, "within the bounding box, a FINE-GRAINED polygon
/// around the recognized item", so a block needs both, measured over the same marks.
#[derive(Debug, Clone, PartialEq)]
pub struct Marks {
    pub rect: Rect,
    pub outline: Outline,
}

/// One word of a recognised line: where its ink sits, and the mean score its own
/// characters were recognised with.
#[derive(Debug, Clone, PartialEq)]
pub struct MeasuredWord {
    pub marks: Marks,
    pub confidence: f32,
}

/// One recognised line: the mean score its characters were recognised with, and its
/// words — each with its own ink — in reading order. The line's own extent is the box
/// enclosing its words, which is where the block layer takes it from.
#[derive(Debug, Clone, PartialEq)]
pub struct MeasuredLine {
    pub confidence: f32,
    pub words: Vec<MeasuredWord>,
}

/// A mean score as the percentage `Confidence` reports, inside `Percent`'s `0..=100`
/// range. Zero when nothing was recognised, because nothing was measured.
fn percentage(score_total: f32, score_count: usize) -> f32 {
    if score_count == 0 {
        0.0
    } else {
        (score_total / score_count as f32 * 100.0).clamp(0.0, 100.0)
    }
}

/// `Some(format)` when these bytes are a raster document format Textract reads on the
/// channel they were submitted on.
///
/// The format comes from the bytes' own magic numbers, never from a declared name. Which
/// formats a channel accepts is AWS's own split (see [`super::DocumentChannel`]): PNG and
/// JPEG are read whichever way the document arrived, while a TIFF is a document only an
/// Amazon S3 object may hold — `Document$Bytes` takes PNG and JPEG alone.
pub fn raster_format(bytes: &[u8], channel: super::DocumentChannel) -> Option<ImageFormat> {
    match image::guess_format(bytes) {
        Ok(format @ (ImageFormat::Png | ImageFormat::Jpeg)) => Some(format),
        Ok(ImageFormat::Tiff) if channel == super::DocumentChannel::S3Object => {
            Some(ImageFormat::Tiff)
        }
        _ => None,
    }
}

/// The name of the raster format these bytes DECLARE, whatever it is: `image`'s own
/// sniffing of the magic number at their head.
///
/// `None` says the bytes declare no raster format the sniffer knows — not that they are a
/// document. It is what lets [`super::read_content`] tell a document in a raster format
/// the service does not process from bytes that are not a raster at all.
pub fn declared_raster_format(bytes: &[u8]) -> Option<&'static str> {
    image::guess_format(bytes).ok().map(format_name)
}

/// Read the text of one raster document.
///
/// `Err` says why the document could not be read, with the variant the handlers answer
/// with the matching modelled error.
pub fn read_raster(bytes: &[u8], format: ImageFormat) -> Result<Recognised, Unreadable> {
    let name = format_name(format);
    // The header is read with the decoder's own size limits OFF, so a page over this
    // emulator's ceiling is refused as too large (what it is) rather than as unreadable.
    let (width, height) = reader(bytes, format, false)
        .into_dimensions()
        .map_err(|error| undecodable(name, &error))?;
    let pixels = u64::from(width) * u64::from(height);
    if width > MAX_RASTER_SIDE || height > MAX_RASTER_SIDE || pixels > MAX_RASTER_PIXELS {
        return Err(Unreadable::TooLarge(format!(
            "the document is {width}x{height} pixels; this emulator reads at most \
             {MAX_RASTER_SIDE} pixels on a side and {MAX_RASTER_PIXELS} pixels in all"
        )));
    }

    let decoded = reader(bytes, format, true)
        .decode()
        .map_err(|error| undecodable(name, &error))?;
    let gray = decoded.into_luma8();
    // The ceilings again, on the frame that was actually decoded: the scan, the
    // segmentation and the ink mask are all sized from THIS image, so the budget must not
    // depend on the header having agreed with it.
    if gray.width() > MAX_RASTER_SIDE
        || gray.height() > MAX_RASTER_SIDE
        || u64::from(gray.width()) * u64::from(gray.height()) > MAX_RASTER_PIXELS
    {
        return Err(Unreadable::TooLarge(format!(
            "the document decodes to {}x{} pixels; this emulator reads at most \
             {MAX_RASTER_SIDE} pixels on a side and {MAX_RASTER_PIXELS} pixels in all",
            gray.width(),
            gray.height()
        )));
    }
    let ink = Ink::of(&gray);
    // A document can be scanned or photographed turned, and a page's characters are only
    // recognisable at the angle they were drawn at, so the page is read as submitted AND
    // at each of its quarter turns: the turn whose marks recognise BEST is the one the
    // characters were drawn at. The reading of the page as submitted stands unless a turn
    // reads it clearly better ([`MIN_ROTATION_GAIN`]), so a page with little ink is never
    // reported sideways on a coincidence.
    let mut best = read_every_way(&ink, Orientation::upright(&ink))?;
    for turns in 1..4u8 {
        let orientation = Orientation::turned(&ink, turns);
        // A turn this page cannot be read at (its marks are over the page's own budget)
        // says nothing about the page, so the reading already in hand decides.
        let Ok(read) = read_every_way(&ink.turned(turns), orientation) else {
            continue;
        };
        if read.strength() > best.strength() * (1.0 + MIN_ROTATION_GAIN) {
            best = read;
        }
    }

    Ok(Recognised {
        confidence: percentage(best.score_total, best.score_count),
        text: best.text,
        lines: best.lines,
        rotation: best.rotation,
        content: ink.content(Orientation::upright(&ink)),
    })
}

/// Read one turn of a page every way something that is NOT a character can be joining the
/// marks of its lines — a rule drawn across a line, a panel of light text on a dark ground
/// ([`Ink::variants`]) — and report the reading whose marks recognise best.
///
/// Every reading is of the same pixels, so a page that carries no such mark reads the same
/// either way: there is nothing in it to take out.
fn read_every_way(ink: &Ink, orientation: Orientation) -> Result<PageRead, Unreadable> {
    let mut best = recognise_page(ink, orientation)?;
    for variant in ink.variants() {
        let Ok(read) = recognise_page(&variant, orientation) else {
            continue;
        };
        if read.strength() > best.strength() {
            best = read;
        }
    }
    Ok(best)
}

/// One page read at ONE orientation: its text, what it was recognised with, and where its
/// words sit on the page as it was submitted.
struct PageRead {
    text: String,
    score_total: f32,
    score_count: usize,
    lines: Vec<MeasuredLine>,
    rotation: f32,
}

impl PageRead {
    /// How well this reading of the page holds up as a whole, which is what one
    /// orientation is chosen over another by.
    ///
    /// Each character counts for what its match beat the floor every reported character
    /// has to clear by ([`MIN_GLYPH_SCORE`]), so a page read at the turn its characters
    /// were drawn at — many marks, each matching its glyph well — stands far above the same
    /// page read sideways, where the slivers a turned line of text falls into match
    /// whatever they match barely at all.
    fn strength(&self) -> f32 {
        (self.score_total - MIN_GLYPH_SCORE * self.score_count as f32).max(0.0)
    }
}

/// Read the lines of ink of `ink`, which is the submitted page turned as `orientation`
/// says, measuring every word back in the frame of the page as it was submitted.
fn recognise_page(ink: &Ink, orientation: Orientation) -> Result<PageRead, Unreadable> {
    let lines = ink.text_lines()?;

    let mut text = String::new();
    let mut measured: Vec<MeasuredLine> = Vec::new();
    let mut score_total = 0.0f32;
    let mut score_count = 0usize;
    for line in &lines {
        let Some(rendered) = line.render(ink) else {
            continue;
        };
        text.push_str(&rendered.text);
        text.push('\n');
        score_total += rendered.score_total;
        score_count += rendered.score_count;
        measured.push(rendered.measured(ink, orientation));
    }

    Ok(PageRead {
        text,
        score_total,
        score_count,
        lines: measured,
        rotation: orientation.angle(),
    })
}

/// How a page was TURNED to read it, and the page's own size, which every measurement
/// taken at this orientation is expressed in.
///
/// A page read at a quarter turn is segmented and recognised in the turned frame, so every
/// rectangle it yields has to be mapped back out of that turn before it can describe the
/// document the caller submitted.
#[derive(Debug, Clone, Copy)]
struct Orientation {
    /// Clockwise quarter turns applied to the submitted page before it was segmented.
    turns: u8,
    /// The submitted page's width in pixels.
    width: usize,
    /// The submitted page's height in pixels.
    height: usize,
}

impl Orientation {
    /// Reading the page as it was submitted.
    fn upright(ink: &Ink) -> Self {
        Self::turned(ink, 0)
    }

    /// Reading the submitted page after `turns` clockwise quarter turns.
    fn turned(ink: &Ink, turns: u8) -> Self {
        Self {
            turns: turns % 4,
            width: ink.width,
            height: ink.height,
        }
    }

    /// The angle, in degrees, the characters read at this orientation are turned by ON the
    /// submitted page: turning the page clockwise to read it upright means its characters
    /// are themselves turned the other way.
    ///
    /// `Geometry$RotationAngle` is "a numerical value corresponding to the rotation of the
    /// WORD block. Possible values are 0, 90, 180, and 270", which is exactly the set of
    /// quarter turns a page is read at.
    fn angle(self) -> f32 {
        f32::from(u16::from((4 - self.turns) % 4)) * 90.0
    }

    /// Where the pixel `(x, y)` of the TURNED page sits on the submitted one.
    fn source(self, x: usize, y: usize) -> (usize, usize) {
        let last_x = self.width.saturating_sub(1);
        let last_y = self.height.saturating_sub(1);
        match self.turns {
            1 => (y, last_y.saturating_sub(x)),
            2 => (last_x.saturating_sub(x), last_y.saturating_sub(y)),
            3 => (last_x.saturating_sub(y), x),
            _ => (x, y),
        }
    }

    /// The rectangle `x0..=x1` by `y0..=y1` of the TURNED page, as a fraction of the page
    /// as it was submitted.
    fn rect(self, x0: usize, x1: usize, y0: usize, y1: usize) -> Rect {
        let (ax, ay) = self.source(x0, y0);
        let (bx, by) = self.source(x1, y1);
        let (left, right) = (ax.min(bx), ax.max(bx));
        let (top, bottom) = (ay.min(by), ay.max(by));
        let page_width = self.width.max(1) as f32;
        let page_height = self.height.max(1) as f32;
        Rect {
            left: (left as f32 / page_width).clamp(0.0, 1.0),
            top: (top as f32 / page_height).clamp(0.0, 1.0),
            width: ((right + 1 - left) as f32 / page_width).clamp(0.0, 1.0),
            height: ((bottom + 1 - top) as f32 / page_height).clamp(0.0, 1.0),
        }
    }

    /// Where the point `(u, v)` of the TURNED page — each a fraction of that page's own
    /// width and height — sits on the page as it was submitted.
    ///
    /// Turning the page back maps its corners onto the submitted page's: a quarter turn
    /// takes the turned page's top edge to one of the submitted page's sides, so a point
    /// measured while reading a turned page still names the pixel of the submitted page it
    /// was measured at.
    fn vertex(self, u: f32, v: f32) -> Vertex {
        let (x, y) = match self.turns {
            1 => (v, 1.0 - u),
            2 => (1.0 - u, 1.0 - v),
            3 => (1.0 - v, u),
            _ => (u, v),
        };
        Vertex {
            x: x.clamp(0.0, 1.0),
            y: y.clamp(0.0, 1.0),
        }
    }
}

/// The refusal for a document in an accepted container whose bytes the decoder cannot
/// read (a truncated or otherwise undecodable file).
///
/// Such a document is one no page can be read out of, which is what
/// [`Unreadable::UnsupportedFormat`] says, and that refusal carries no reason of its own:
/// what the decoder tripped over is a detail of this emulator's reader, so it is logged
/// here instead of being answered to the caller.
fn undecodable(name: &str, error: &image::ImageError) -> Unreadable {
    ::tracing::debug!(
        format = name,
        error = %error,
        "the document's bytes could not be decoded"
    );
    Unreadable::UnsupportedFormat
}

fn format_name(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "PNG",
        ImageFormat::Jpeg => "JPEG",
        ImageFormat::Tiff => "TIFF",
        // Reached through `declared_raster_format`, which names whatever format the
        // sniffer recognised so a refusal can be logged with it.
        other => other.extensions_str().first().copied().unwrap_or("raster"),
    }
}

/// A decoder for `bytes`, bounded so a small file cannot decode into a large one.
///
/// `bound_dimensions` turns the decoder's own width/height ceilings on, for the decode
/// itself; the header read leaves them off so this module can tell a page that is too
/// large from one that cannot be read at all.
fn reader(
    bytes: &[u8],
    format: ImageFormat,
    bound_dimensions: bool,
) -> image::ImageReader<Cursor<&[u8]>> {
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), format);
    // `Limits::no_limits()` rather than `default()`: the bounds below are this
    // emulator's, and starting from the crate's defaults would silently inherit them.
    let mut limits = Limits::no_limits();
    if bound_dimensions {
        limits.max_image_width = Some(MAX_RASTER_SIDE);
        limits.max_image_height = Some(MAX_RASTER_SIDE);
    }
    limits.max_alloc = Some(MAX_RASTER_ALLOC);
    reader.limits(limits);
    reader
}

// ---------------------------------------------------------------------------
// Binarisation
// ---------------------------------------------------------------------------

/// A page's ink: one bit per pixel, true where the recogniser sees a mark.
struct Ink {
    width: usize,
    height: usize,
    mask: Vec<bool>,
}

/// The four EXTREME marks of a set of pixels — the topmost, the rightmost, the bottommost
/// and the leftmost — accumulated as the pixels are scanned.
///
/// One scan yields both what a block reports: the box around the marks is their bounds,
/// and the fine-grained outline of the block is the extremes themselves. Ties go to the
/// mark furthest along the page's own reading order, so one set of marks has one outline.
#[derive(Debug, Default)]
struct Extremes {
    top: Option<(usize, usize)>,
    right: Option<(usize, usize)>,
    bottom: Option<(usize, usize)>,
    left: Option<(usize, usize)>,
}

impl Extremes {
    /// Take the mark at `(x, y)` into the four extremes.
    fn mark(&mut self, x: usize, y: usize) {
        if self.top.is_none_or(|(tx, ty)| y < ty || (y == ty && x < tx)) {
            self.top = Some((x, y));
        }
        if self.right.is_none_or(|(rx, ry)| x > rx || (x == rx && y < ry)) {
            self.right = Some((x, y));
        }
        if self.bottom.is_none_or(|(bx, by)| y > by || (y == by && x > bx)) {
            self.bottom = Some((x, y));
        }
        if self.left.is_none_or(|(lx, ly)| x < lx || (x == lx && y > ly)) {
            self.left = Some((x, y));
        }
    }

    /// The pixel bounds `x0..=x1` by `y0..=y1` of every mark taken in, or `None` when none
    /// was: the leftmost and rightmost mark give the columns, the topmost and bottommost
    /// the rows.
    fn bounds(&self) -> Option<(usize, usize, usize, usize)> {
        Some((self.left?.0, self.right?.0, self.top?.1, self.bottom?.1))
    }
}

impl Ink {
    /// Binarise a grey page with Otsu's method, taking the smaller class as ink.
    fn of(gray: &image::GrayImage) -> Self {
        let width = gray.width() as usize;
        let height = gray.height() as usize;
        let mut histogram = [0u64; 256];
        for pixel in gray.pixels() {
            histogram[pixel.0[0] as usize] += 1;
        }
        let total: u64 = histogram.iter().sum();
        let mask = match otsu(&histogram, total) {
            // One tone plus noise: a blank page carries no ink at all.
            None => vec![false; width * height],
            Some(split) => {
                let dark: u64 = histogram[..=split.threshold as usize].iter().sum();
                let ink_is_dark = dark * 2 <= total;
                gray.pixels()
                    .map(|pixel| {
                        let value = pixel.0[0];
                        if ink_is_dark {
                            value <= split.threshold
                        } else {
                            value > split.threshold
                        }
                    })
                    .collect()
            }
        };
        Self {
            width,
            height,
            mask,
        }
    }

    fn at(&self, x: usize, y: usize) -> bool {
        self.mask.get(y * self.width + x).copied().unwrap_or(false)
    }

    /// This page turned `turns` quarter turns clockwise, so text drawn at that turn reads
    /// along the rows of the result.
    fn turned(&self, turns: u8) -> Self {
        let orientation = Orientation::turned(self, turns);
        let (width, height) = if orientation.turns % 2 == 1 {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        };
        let mut mask = vec![false; width * height];
        for y in 0..height {
            for x in 0..width {
                let (sx, sy) = orientation.source(x, y);
                mask[y * width + x] = self.at(sx, sy);
            }
        }
        Self {
            width,
            height,
            mask,
        }
    }

    /// The extent of the page's CONTENT: the box enclosing every mark on it and the
    /// outline of those marks, as fractions of the page, or `None` for a page carrying no
    /// ink.
    ///
    /// Every mark, not only the marks that were read as characters: a rule, a border or a
    /// logo is part of the content of the page it is drawn on, and the `PAGE` block's
    /// `Geometry` bounds what is on the page rather than what could be named.
    ///
    /// ONE pass over the page: the box and the outline are the bounds and the extremes of
    /// the same marks, so both come out of the same scan ([`Extremes`]).
    fn content(&self, orientation: Orientation) -> Option<Marks> {
        let mut extremes = Extremes::default();
        for y in 0..self.height {
            for x in 0..self.width {
                if self.at(x, y) {
                    extremes.mark(x, y);
                }
            }
        }
        let (x0, x1, y0, y1) = extremes.bounds()?;
        Some(Marks {
            rect: orientation.rect(x0, x1, y0, y1),
            outline: self.outline_of(&extremes, (x0, x1, y0, y1), orientation),
        })
    }

    /// Where the ink inside `x0..=x1` by `y0..=y1` sits on the submitted page: the box
    /// enclosing it, and the outline of the marks themselves.
    ///
    /// The outline's four points are the four EXTREME marks of that ink — the topmost, the
    /// rightmost, the bottommost and the leftmost mark found, each at the corner of its own
    /// pixel that faces away from the others. Every one of them is a mark that is really
    /// there, so the outline lies inside the box and touches each of its edges, and it
    /// follows the ink's own shape: the marks of a line whose glyphs do not all start at
    /// its top outline a slanted quadrilateral rather than the box's four corners.
    ///
    /// A rectangle with no ink in it at all cannot be outlined, so its own corners are
    /// reported: that is the whole of what was measured for it.
    fn marks(&self, x0: usize, x1: usize, y0: usize, y1: usize, orientation: Orientation) -> Marks {
        let right = x1.min(self.width.saturating_sub(1));
        let bottom = y1.min(self.height.saturating_sub(1));
        let mut extremes = Extremes::default();
        for y in y0..=bottom {
            for x in x0..=right {
                if self.at(x, y) {
                    extremes.mark(x, y);
                }
            }
        }
        Marks {
            rect: orientation.rect(x0, right, y0, bottom),
            outline: self.outline_of(&extremes, (x0, right, y0, bottom), orientation),
        }
    }

    /// The extreme marks of `extremes` as points on the submitted page, falling back to
    /// the corners of `x0..=x1` by `y0..=y1` for a rectangle no mark was found in.
    fn outline_of(
        &self,
        extremes: &Extremes,
        (x0, x1, y0, y1): (usize, usize, usize, usize),
        orientation: Orientation,
    ) -> Outline {
        let page_width = self.width.max(1) as f32;
        let page_height = self.height.max(1) as f32;
        // A pixel covers the cell `x..x+1` by `y..y+1` of the page, which is the same cell
        // `Orientation::rect` measures the box over, so a corner of the outline and the
        // edge of the box it touches are the same number.
        let corner = |(x, y): (usize, usize), rightwards: bool, downwards: bool| {
            orientation.vertex(
                (x + usize::from(rightwards)) as f32 / page_width,
                (y + usize::from(downwards)) as f32 / page_height,
            )
        };
        [
            corner(extremes.top.unwrap_or((x0, y0)), false, false),
            corner(extremes.right.unwrap_or((x1, y0)), true, false),
            corner(extremes.bottom.unwrap_or((x1, y1)), true, true),
            corner(extremes.left.unwrap_or((x0, y1)), false, true),
        ]
    }

    fn row_inked(&self, y: usize) -> bool {
        (0..self.width).any(|x| self.at(x, y))
    }

    /// The bands of inked rows of this page, top to bottom: one per line of marks.
    fn bands(&self) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut y = 0usize;
        while y < self.height {
            if !self.row_inked(y) {
                y += 1;
                continue;
            }
            let top = y;
            while y < self.height && self.row_inked(y) {
                y += 1;
            }
            out.push((top, y - 1));
        }
        out
    }

    /// The leftmost and the rightmost inked column of the band `top..=bottom`.
    fn ink_columns(&self, top: usize, bottom: usize) -> Option<(usize, usize)> {
        let inked = |x: usize| (top..=bottom).any(|y| self.at(x, y));
        let left = (0..self.width).find(|x| inked(*x))?;
        let right = (left..self.width).rfind(|x| inked(*x))?;
        Some((left, right))
    }

    /// Which rows of the band `top..=bottom` carry a RULE: a single run of ink spanning
    /// most of the band's own width ([`RULE_ROW_SHARE`]).
    ///
    /// A rule drawn ACROSS a line — an underline, a cell border, a strike-through — joins
    /// every character it runs through into one mark, and a panel of light text on a dark
    /// ground is such a run in every one of its rows.
    fn rule_rows(&self, top: usize, bottom: usize, left: usize, right: usize) -> Vec<bool> {
        let span = RULE_ROW_SHARE * (right + 1 - left) as f32;
        (top..=bottom)
            .map(|y| {
                let mut longest = 0usize;
                let mut run = 0usize;
                for x in left..=right {
                    if self.at(x, y) {
                        run += 1;
                        longest = longest.max(run);
                    } else {
                        run = 0;
                    }
                }
                longest as f32 >= span
            })
            .collect()
    }

    /// The maximal runs of columns the mask `inked` marks within the rectangle
    /// `left..=right` by `top..=bottom`, left to right, each with its own extent.
    fn runs_in(
        &self,
        left: usize,
        right: usize,
        top: usize,
        bottom: usize,
        inked: impl Fn(usize, usize) -> bool,
    ) -> Vec<GlyphBox> {
        let column = |x: usize| (top..=bottom).any(|y| inked(x, y));
        let mut out = Vec::new();
        let mut x = left;
        while x <= right {
            if !column(x) {
                x += 1;
                continue;
            }
            let x0 = x;
            while x <= right && column(x) {
                x += 1;
            }
            let x1 = x - 1;
            let mut y0 = bottom;
            let mut y1 = top;
            for y in top..=bottom {
                if (x0..=x1).any(|cx| inked(cx, y)) {
                    y0 = y0.min(y);
                    y1 = y1.max(y);
                }
            }
            out.push(GlyphBox { x0, x1, y0, y1 });
        }
        out
    }

    /// This page read a second way, where something that is NOT a character joins the
    /// marks of a line into one: a rule drawn across the line, a hairline of ink spread
    /// between two characters, or a panel of light text on a dark ground.
    ///
    /// Each is a reading of the same pixels, not a guess about them: the caller reads the
    /// page every way and keeps the reading whose marks recognise best, so a page that
    /// carries none of them is unaffected by all.
    fn variants(&self) -> Vec<Self> {
        let mut out = Vec::new();
        out.extend(self.without_rules());
        out.extend(self.without_hairlines());
        out.extend(self.panels_read());
        out
    }

    /// This page with the HAIRLINES that bridge the marks of a line cleared, or `None` when
    /// it carries none.
    ///
    /// A column of a line carrying ink over only a sliver of the line's height
    /// ([`MAX_HAIRLINE_SHARE`]) is no part of a character's body: it is what ink spread
    /// between two characters leaves — the bridge a heavy stroke, a coarse threshold or a
    /// lossy coding puts between them — and it joins them into one mark. Whether such a
    /// column IS a bridge is established the way a rule is: by clearing it, and keeping the
    /// result only where what remains falls into more marks than before.
    fn without_hairlines(&self) -> Option<Self> {
        let mut cleared: Option<Vec<bool>> = None;
        for (top, bottom) in self.bands() {
            let Some((left, right)) = self.ink_columns(top, bottom) else {
                continue;
            };
            let band = bottom + 1 - top;
            let sliver = MAX_HAIRLINE_SHARE * band as f32;
            // Measured once per column of the band, not once per look: the segmentation
            // below reads the mask a column at a time, over a page whose size the caller
            // chose.
            let hairlines: Vec<bool> = (left..=right)
                .map(|x| {
                    let ink = (top..=bottom).filter(|y| self.at(x, *y)).count();
                    ink > 0 && (ink as f32) <= sliver
                })
                .collect();
            if !hairlines.iter().any(|bridge| *bridge) {
                continue;
            }
            let joined = self.runs_in(left, right, top, bottom, |x, y| self.at(x, y));
            let apart = self.runs_in(left, right, top, bottom, |x, y| {
                self.at(x, y) && !hairlines[x - left]
            });
            if apart.len() <= joined.len() {
                continue;
            }
            let mask = cleared.get_or_insert_with(|| self.mask.clone());
            for (offset, _) in hairlines.iter().enumerate().filter(|(_, bridge)| **bridge) {
                for y in top..=bottom {
                    mask[y * self.width + left + offset] = false;
                }
            }
        }
        cleared.map(|mask| Self {
            width: self.width,
            height: self.height,
            mask,
        })
    }

    /// This page with the RULES drawn across its lines taken out, or `None` when it carries
    /// none.
    ///
    /// Whether a long run of ink is a rule BETWEEN characters is established by taking it
    /// out: if what remains falls into more marks than before, the run was joining
    /// characters; if it does not, the ink was part of one — the bar of a 'T', the crossbar
    /// of an 'H' — and it stays. A band too narrow to hold a line of characters
    /// ([`MIN_RULE_BAND_ASPECT`]) carries no rule at all, for the same reason.
    fn without_rules(&self) -> Option<Self> {
        let mut cleaned: Option<Vec<bool>> = None;
        for (top, bottom) in self.bands() {
            let Some((left, right)) = self.ink_columns(top, bottom) else {
                continue;
            };
            let band = bottom + 1 - top;
            if ((right + 1 - left) as f32) < MIN_RULE_BAND_ASPECT * band as f32 {
                continue;
            }
            let rules = self.rule_rows(top, bottom, left, right);
            if !rules.iter().any(|rule| *rule) {
                continue;
            }
            let joined = self.runs_in(left, right, top, bottom, |x, y| self.at(x, y));
            let apart = self.runs_in(left, right, top, bottom, |x, y| {
                self.at(x, y) && !rules[y - top]
            });
            if apart.len() <= joined.len() {
                continue;
            }
            let mask = cleaned.get_or_insert_with(|| self.mask.clone());
            for (offset, _) in rules.iter().enumerate().filter(|(_, rule)| **rule) {
                for x in left..=right {
                    mask[(top + offset) * self.width + x] = false;
                }
            }
        }
        cleaned.map(|mask| Self {
            width: self.width,
            height: self.height,
            mask,
        })
    }

    /// This page with its PANELS read as the light text they hold, or `None` when it
    /// carries none.
    ///
    /// A band whose ink FILLS most of its own box ([`MIN_PANEL_FILL`]) is not a line of
    /// characters: characters leave most of their line blank. It is a filled panel, and the
    /// characters of such a panel are the ground it leaves inside its own edge — light text
    /// on a dark ground is text all the same — so the panel is read by taking its ground for
    /// its ink, and only where that ground falls into marks of its own.
    fn panels_read(&self) -> Option<Self> {
        let mut read: Option<Vec<bool>> = None;
        for (top, bottom) in self.bands() {
            let Some((left, right)) = self.ink_columns(top, bottom) else {
                continue;
            };
            let (band, width) = (bottom + 1 - top, right + 1 - left);
            if band < MIN_PANEL_SIDE || width < MIN_PANEL_SIDE {
                continue;
            }
            // A panel holds a LINE of text, so it is wider than it is tall. A band that is
            // not is one mark of the page's own ink — a heavy character, a mark of a
            // drawing — whose ground is the shape of that mark rather than text.
            if (width as f32) < MIN_RULE_BAND_ASPECT * band as f32 {
                continue;
            }
            let inked = (top..=bottom)
                .map(|y| (left..=right).filter(|x| self.at(*x, y)).count())
                .sum::<usize>();
            if (inked as f32) < MIN_PANEL_FILL * (band * width) as f32 {
                continue;
            }
            let (inner_left, inner_right) = (left + 1, right - 1);
            let (inner_top, inner_bottom) = (top + 1, bottom - 1);
            let ground = self.runs_in(inner_left, inner_right, inner_top, inner_bottom, |x, y| {
                !self.at(x, y)
            });
            if ground.len() < MIN_PANEL_MARKS {
                continue;
            }
            let mask = read.get_or_insert_with(|| self.mask.clone());
            for y in top..=bottom {
                for x in left..=right {
                    let inside = (inner_left..=inner_right).contains(&x)
                        && (inner_top..=inner_bottom).contains(&y);
                    mask[y * self.width + x] = inside && !self.at(x, y);
                }
            }
        }
        read.map(|mask| Self {
            width: self.width,
            height: self.height,
            mask,
        })
    }

    /// The page's lines of text, top to bottom, with the glyph candidates of each.
    fn text_lines(&self) -> Result<Vec<InkLine>, Unreadable> {
        let mut lines = Vec::new();
        let mut glyph_budget = MAX_GLYPHS_PER_PAGE;
        for (top, bottom) in self.bands() {
            if bottom + 1 - top < MIN_LINE_HEIGHT {
                continue;
            }
            let glyphs = self.glyphs_between(top, bottom);
            if glyphs.is_empty() {
                continue;
            }
            // A short, page-wide run of ink is a rule or a border, not text.
            let inked_width = glyphs.last().map_or(0, |g| g.x1) + 1 - glyphs[0].x0;
            if bottom + 1 - top <= RULE_MAX_HEIGHT && inked_width * 2 > self.width {
                continue;
            }
            if glyphs.len() > glyph_budget {
                return Err(Unreadable::TooLarge(format!(
                    "the document holds more than {MAX_GLYPHS_PER_PAGE} marks to recognise; \
                     this emulator reads at most that many on one page"
                )));
            }
            glyph_budget -= glyphs.len();
            lines.push(InkLine {
                top,
                bottom,
                glyphs,
            });
        }
        Ok(lines)
    }

    /// The glyph candidates of the band `top..=bottom`: the maximal runs of inked
    /// columns, left to right, each with its own ink extent.
    fn glyphs_between(&self, top: usize, bottom: usize) -> Vec<GlyphBox> {
        self.runs_in(0, self.width.saturating_sub(1), top, bottom, |x, y| {
            self.at(x, y)
        })
    }
}

/// The threshold Otsu's method picks, and how far apart the classes it splits are.
struct Split {
    threshold: u8,
    separation: f32,
}

/// Otsu's threshold for a grey histogram, or `None` when the two classes it would split
/// are closer together than [`MIN_INK_CONTRAST`] — one tone plus noise, so no ink.
fn otsu(histogram: &[u64; 256], total: u64) -> Option<Split> {
    if total == 0 {
        return None;
    }
    let total = total as f64;
    let sum: f64 = histogram
        .iter()
        .enumerate()
        .map(|(value, count)| value as f64 * *count as f64)
        .sum();
    let mut below_weight = 0f64;
    let mut below_sum = 0f64;
    let mut best: Option<Split> = None;
    let mut best_variance = -1f64;
    for (value, count) in histogram.iter().enumerate().take(255) {
        below_weight += *count as f64;
        below_sum += value as f64 * *count as f64;
        let above_weight = total - below_weight;
        if below_weight == 0.0 || above_weight == 0.0 {
            continue;
        }
        let below_mean = below_sum / below_weight;
        let above_mean = (sum - below_sum) / above_weight;
        let delta = above_mean - below_mean;
        let variance = below_weight * above_weight * delta * delta;
        if variance > best_variance {
            best_variance = variance;
            best = Some(Split {
                threshold: u8::try_from(value).unwrap_or(u8::MAX),
                separation: delta as f32,
            });
        }
    }
    best.filter(|split| split.separation >= MIN_INK_CONTRAST)
}

// ---------------------------------------------------------------------------
// Segmentation
// ---------------------------------------------------------------------------

/// One candidate mark: the rectangle its ink occupies.
struct GlyphBox {
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
}

impl GlyphBox {
    fn width(&self) -> usize {
        self.x1 + 1 - self.x0
    }
}

/// A line of ink on the page.
struct InkLine {
    top: usize,
    bottom: usize,
    glyphs: Vec<GlyphBox>,
}

/// One line's recognised text, and the scores it was recognised with.
struct RenderedLine {
    text: String,
    score_total: f32,
    score_count: usize,
    /// The line's words, in reading order: one per run of glyphs the text does NOT
    /// separate with a space or a tab, so `words[k]` is the k-th whitespace-separated
    /// word of `text`.
    words: Vec<WordRun>,
}

/// One word's measured ink, in page pixels, and the scores its characters matched with.
struct WordRun {
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
    score_total: f32,
    score_count: usize,
}

impl WordRun {
    /// A word starting at `glyph`.
    fn starting_at(glyph: &GlyphBox, score: f32) -> Self {
        Self {
            x0: glyph.x0,
            x1: glyph.x1,
            y0: glyph.y0,
            y1: glyph.y1,
            score_total: score,
            score_count: 1,
        }
    }

    /// Take `glyph` into this word.
    fn extend(&mut self, glyph: &GlyphBox, score: f32) {
        self.x0 = self.x0.min(glyph.x0);
        self.x1 = self.x1.max(glyph.x1);
        self.y0 = self.y0.min(glyph.y0);
        self.y1 = self.y1.max(glyph.y1);
        self.score_total += score;
        self.score_count += 1;
    }

    /// This word's ink as it sits on the page it was submitted as, whichever turn of that
    /// page it was read at: the box enclosing the ink, and the outline of the marks.
    fn marks(&self, ink: &Ink, orientation: Orientation) -> Marks {
        ink.marks(self.x0, self.x1, self.y0, self.y1, orientation)
    }
}

impl RenderedLine {
    /// This line's measurements, as fractions of the page as it was SUBMITTED: the extent
    /// and the outline of each word's own ink, and the score each was recognised with.
    fn measured(&self, ink: &Ink, orientation: Orientation) -> MeasuredLine {
        MeasuredLine {
            confidence: percentage(self.score_total, self.score_count),
            words: self
                .words
                .iter()
                .map(|run| MeasuredWord {
                    marks: run.marks(ink, orientation),
                    confidence: percentage(run.score_total, run.score_count),
                })
                .collect(),
        }
    }
}

impl InkLine {
    fn band(&self) -> usize {
        self.bottom + 1 - self.top
    }

    /// Recognise this line: its characters, and the word and column breaks between them.
    ///
    /// `None` when nothing in the line matched a glyph, so the line carries no text.
    fn render(&self, ink: &Ink) -> Option<RenderedLine> {
        let band = self.band();
        let median_width = median(self.glyphs.iter().map(GlyphBox::width)).unwrap_or(1.0);
        let gaps: Vec<usize> = self
            .glyphs
            .windows(2)
            .map(|pair| pair[1].x0.saturating_sub(pair[0].x1 + 1))
            .collect();
        // The STEP from each mark to the next: the pitch the line's characters are set at,
        // which is what its word and column separators are measured against.
        let steps: Vec<usize> = self
            .glyphs
            .windows(2)
            .map(|pair| pair[1].x0.saturating_sub(pair[0].x0))
            .collect();
        // Which marks a line separates into words, and which into COLUMNS, is a property
        // of THIS line's own marks: see [`Separators`].
        let separators = Separators::of(&self.glyphs, &gaps, &steps, median_width);
        // The character cell the line's left margin is reported in, so a line that
        // starts half way across the page is laid out half way across its text.
        let cell = separators.cell();

        let mut text = String::new();
        let mut score_total = 0.0f32;
        let mut score_count = 0usize;
        let mut words: Vec<WordRun> = Vec::new();
        let mut previous: Option<&GlyphBox> = None;
        for glyph in &self.glyphs {
            let Some((ch, score)) = recognise(ink, glyph, self.top, band) else {
                continue;
            };
            // Whether this glyph starts a new word is decided by the same gaps the text
            // is separated on, so the measured words and the words of the text are one
            // and the same list.
            let starts_word = match previous {
                None => {
                    let indent = ((glyph.x0 as f32 / cell).round() as usize).min(MAX_INDENT);
                    text.push_str(&" ".repeat(indent));
                    true
                }
                Some(before) => match separators.between(before, glyph) {
                    Separation::Column => {
                        text.push('\t');
                        true
                    }
                    Separation::Word => {
                        text.push(' ');
                        true
                    }
                    Separation::None => false,
                },
            };
            text.push(ch);
            match words.last_mut() {
                Some(word) if !starts_word => word.extend(glyph, score),
                _ => words.push(WordRun::starting_at(glyph, score)),
            }
            score_total += score;
            score_count += 1;
            previous = Some(glyph);
        }
        if score_count == 0 {
            return None;
        }
        Some(RenderedLine {
            text,
            score_total,
            score_count,
            words,
        })
    }
}

/// The median of `values`, or `None` when there are none.
fn median(values: impl Iterator<Item = usize>) -> Option<f32> {
    let mut values: Vec<usize> = values.collect();
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    Some(values[values.len() / 2] as f32)
}

/// What separates two neighbouring marks of one line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Separation {
    /// Nothing: they are two marks of one word.
    None,
    /// A word space.
    Word,
    /// A column break — the gap between two cells of a table row.
    Column,
}

/// How one line's marks are separated into words and into columns, measured off THAT
/// line's own marks.
///
/// A word space is a character cell nothing was drawn in, so what identifies one is the
/// cell, and a line is set in one of two ways:
///
/// * MONOSPACED — every mark sits a whole number of cells from the line's first mark
///   ([`Separators::lattice_cell`]). The gaps a letter's own side bearings leave on such a
///   line are a large part of its cell and no narrower than a space's, so what separates
///   two words here is a STEP that skips a cell.
/// * PROPORTIONAL — the marks fit no cell, because each letter advances by its own width.
///   The side bearings are then a small part of the pitch while a space is most of one, so
///   the GAP between two marks separates them: the line's own gaps fall into a cluster of
///   bearings and a cluster of spaces, and the boundary between the two is where the line
///   separates its words ([`Separators::clustered_gap`]). A line that shows no such
///   boundary is separated at a fixed part of its pitch ([`WORD_GAP_PITCH`]), which is
///   what a line of one word needs so that nothing inside it is read as a space.
///
/// Measuring against the line's pitch rather than against an absolute width is what reads
/// text drawn small and text drawn large by the one rule.
enum Separators {
    /// The line is set on a lattice of this cell, in pixels.
    Monospaced { cell: f32 },
    /// The line's marks fit no cell; its characters are set at this pitch, in pixels, and
    /// a gap of `word` pixels or more separates two of its words.
    Proportional { pitch: f32, word: f32 },
}

impl Separators {
    /// The separators of the line `glyphs` are the marks of, with the gaps and the steps
    /// between them already taken.
    fn of(glyphs: &[GlyphBox], gaps: &[usize], steps: &[usize], median_width: f32) -> Self {
        // The pitch is the line's median step, except where more of its marks stand apart
        // than together (a line of single-letter words): there the median step is a WORD's
        // step rather than a character's, and the median mark plus the median gap is the
        // narrower estimate.
        let median_gap = median(gaps.iter().copied()).unwrap_or(0.0);
        let typical = median_width + median_gap;
        let pitch = median(steps.iter().copied())
            .unwrap_or(typical)
            .min(typical)
            .max(1.0);
        match Self::lattice_cell(glyphs, pitch) {
            Some(cell) => Self::Monospaced { cell },
            None => Self::Proportional {
                pitch,
                word: Self::clustered_gap(gaps, pitch).unwrap_or(WORD_GAP_PITCH * pitch),
            },
        }
    }

    /// The gap at which a PROPORTIONAL line separates its words, read off the clusters its
    /// own gaps fall into: the bearings inside its words, and the spaces between them.
    ///
    /// The boundary is the lowest jump in the line's sorted gaps that both doubles the gaps
    /// below it and leaves a gap of at least [`MIN_SPACE_PITCH`] of the line's pitch above
    /// it — a space takes a part of a character cell, while a bearing is the sliver a
    /// letter's own shape leaves. Reading the boundary off the line rather than fixing it
    /// is what separates the words of a line set TIGHT, whose spaces are narrow but still
    /// stand clear of its bearings.
    ///
    /// `None` for a line whose gaps show no such boundary, which is what a line of one word
    /// looks like.
    fn clustered_gap(gaps: &[usize], pitch: f32) -> Option<f32> {
        let mut sorted: Vec<usize> = gaps.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        sorted
            .windows(2)
            .find(|pair| {
                let (bearing, space) = (pair[0], pair[1]);
                space >= 2 * bearing.max(1) && space as f32 >= MIN_SPACE_PITCH * pitch
            })
            .map(|pair| (pair[0] + pair[1]) as f32 / 2.0)
    }

    /// The cell a line's marks are set in, when they sit on a LATTICE: the cell that leaves
    /// every mark closest to a whole number of cells from the line's first mark, when even
    /// the closest leaves them no further out than [`MAX_LATTICE_DRIFT`].
    ///
    /// `None` for a line whose marks fit no cell — a proportional rendering, where each
    /// letter advances by its own width — and for a line of too few marks
    /// ([`MIN_LATTICE_MARKS`]) for the fit to say anything.
    ///
    /// The marks are placed by their CENTRES, which is where a cell holds them whatever ink
    /// the glyph puts in it, and the cell is searched for around `pitch`, the step most of
    /// the line's marks are a cell apart by.
    fn lattice_cell(glyphs: &[GlyphBox], pitch: f32) -> Option<f32> {
        if glyphs.len() < MIN_LATTICE_MARKS || pitch < 2.0 {
            return None;
        }
        let centres: Vec<f32> = glyphs
            .iter()
            .map(|glyph| (glyph.x0 + glyph.x1) as f32 / 2.0)
            .collect();
        let mut best: Option<(f32, f32)> = None;
        for offset in -LATTICE_SEARCH..=LATTICE_SEARCH {
            let cell = pitch * (1.0 + offset as f32 / 64.0);
            let drift = Self::lattice_drift(&centres, cell);
            if best.is_none_or(|(lowest, _)| drift < lowest) {
                best = Some((drift, cell));
            }
        }
        best.filter(|(drift, _)| *drift <= MAX_LATTICE_DRIFT)
            .map(|(_, cell)| cell)
    }

    /// How far from a whole number of `cell`s the marks at `centres` sit, in cells, on
    /// average over the line.
    fn lattice_drift(centres: &[f32], cell: f32) -> f32 {
        let first = centres.first().copied().unwrap_or_default();
        let total: f32 = centres
            .iter()
            .map(|centre| {
                let cells = (centre - first) / cell;
                (cells - cells.round()).abs()
            })
            .sum();
        total / centres.len() as f32
    }

    /// What separates the mark `before` from the mark `after`, which follows it on the
    /// same line.
    fn between(&self, before: &GlyphBox, after: &GlyphBox) -> Separation {
        let (measured, word, column) = match *self {
            Self::Monospaced { cell } => (
                after.x0.saturating_sub(before.x0) as f32,
                LATTICE_WORD_CELLS * cell,
                LATTICE_COLUMN_CELLS * cell,
            ),
            Self::Proportional { pitch, word } => (
                after.x0.saturating_sub(before.x1 + 1) as f32,
                word,
                COLUMN_GAP_PITCH * pitch,
            ),
        };
        if measured >= column {
            Separation::Column
        } else if measured >= word {
            Separation::Word
        } else {
            Separation::None
        }
    }

    /// The character cell this line's left margin is reported in, so a line that starts
    /// half way across the page is laid out half way across its text.
    fn cell(&self) -> f32 {
        match *self {
            Self::Monospaced { cell } => cell,
            // A proportional line has no cell; its characters take a pitch each.
            Self::Proportional { pitch, .. } => pitch,
        }
        .max(1.0)
    }
}

// ---------------------------------------------------------------------------
// Recognition
// ---------------------------------------------------------------------------

/// The side of the grid a glyph and a template are compared on: the embedded font's own
/// cell size.
const CELL: usize = 8;

/// One candidate's measurements, in the frame of its line.
struct Candidate {
    /// Its ink, resampled over its own bounding box onto a `CELL` x `CELL` grid.
    shape: [f32; CELL * CELL],
    /// How much ink that grid holds, for the Dice denominator.
    ink: f32,
    /// Its height, width and top offset as fractions of the line's band, which is what
    /// tells 'o' from 'O' and '.' from '-'.
    height: f32,
    width: f32,
    top: f32,
}

/// One recognition template: a printable ASCII glyph of the embedded 8x8 font, measured
/// exactly as a candidate is.
struct Template {
    ch: char,
    shape: [f32; CELL * CELL],
    ink: f32,
    height: f32,
    width: f32,
    top: f32,
}

/// Every printable ASCII glyph of the embedded font, as a template.
///
/// `font8x8::legacy::BASIC_LEGACY` is the public-domain 8x8 bitmap font: one row per
/// byte, bit 0 the leftmost pixel.
fn templates() -> &'static [Template] {
    static TEMPLATES: OnceLock<Vec<Template>> = OnceLock::new();
    TEMPLATES.get_or_init(|| (b'!'..=b'~').filter_map(|code| template(code as char)).collect())
}

fn template(ch: char) -> Option<Template> {
    // `get`, not an index: the table holds 128 entries and this function's signature
    // accepts any `char`, so a non-ASCII one is no template rather than a panic inside
    // the `OnceLock` initialiser.
    let rows = *font8x8::legacy::BASIC_LEGACY.get(ch as usize)?;
    let lit = |x: usize, y: usize| rows[y] & (1 << x) != 0;
    let mut x0 = CELL;
    let mut x1 = 0usize;
    let mut y0 = CELL;
    let mut y1 = 0usize;
    for y in 0..CELL {
        for x in 0..CELL {
            if lit(x, y) {
                x0 = x0.min(x);
                x1 = x1.max(x);
                y0 = y0.min(y);
                y1 = y1.max(y);
            }
        }
    }
    if x0 > x1 || y0 > y1 {
        // A glyph the font leaves blank is no template at all.
        return None;
    }
    let shape = resample(lit, x0, x1, y0, y1);
    Some(Template {
        ch,
        ink: shape.iter().sum(),
        shape,
        height: (y1 + 1 - y0) as f32 / CELL as f32,
        width: (x1 + 1 - x0) as f32 / CELL as f32,
        top: y0 as f32 / CELL as f32,
    })
}

/// The best-matching character for one candidate, and the score it matched with, or
/// `None` when nothing matched it well enough to report.
fn recognise(ink: &Ink, glyph: &GlyphBox, band_top: usize, band: usize) -> Option<(char, f32)> {
    let mask = |x: usize, y: usize| ink.at(x, y);
    let shape = resample(mask, glyph.x0, glyph.x1, glyph.y0, glyph.y1);
    let band = band as f32;
    let candidate = Candidate {
        ink: shape.iter().sum(),
        shape,
        height: ((glyph.y1 + 1 - glyph.y0) as f32 / band).clamp(0.0, 1.0),
        width: (glyph.width() as f32 / band).clamp(0.0, 1.0),
        top: (glyph.y0.saturating_sub(band_top) as f32 / band).clamp(0.0, 1.0),
    };
    let mut best: Option<Match> = None;
    for template in templates() {
        let scored = score(&candidate, template);
        if best.is_none_or(|best: Match| scored.total > best.total) {
            best = Some(Match {
                ch: template.ch,
                ..scored
            });
        }
    }
    // Both floors: the mark's shape must really be this glyph's shape, and the whole
    // measurement must hold up. Ink that clears neither matched nothing.
    best.filter(|found| found.shape_overlap >= MIN_SHAPE_OVERLAP && found.total >= MIN_GLYPH_SCORE)
        .map(|found| (found.ch, found.total))
}

/// How well one candidate matched one template.
#[derive(Debug, Clone, Copy)]
struct Match {
    ch: char,
    /// The weighted measurement, which is what the `Confidence` reports.
    total: f32,
    /// The shape term on its own, floored by [`MIN_SHAPE_OVERLAP`].
    shape_overlap: f32,
}

/// How well a candidate matches a template: mostly the overlap of the two shapes (the
/// Sørensen-Dice coefficient, which is 1 for identical ink and 0 for disjoint ink), plus
/// how closely the glyph's size and position inside its line agree with the template's
/// inside its cell.
fn score(candidate: &Candidate, template: &Template) -> Match {
    let overlap: f32 = candidate
        .shape
        .iter()
        .zip(template.shape.iter())
        .map(|(c, t)| c.min(*t))
        .sum();
    let denominator = candidate.ink + template.ink;
    let dice = if denominator > 0.0 {
        2.0 * overlap / denominator
    } else {
        0.0
    };
    let agree = |a: f32, b: f32| 1.0 - (a - b).abs().min(1.0);
    Match {
        ch: template.ch,
        total: 0.70 * dice
            + 0.12 * agree(candidate.height, template.height)
            + 0.10 * agree(candidate.width, template.width)
            + 0.08 * agree(candidate.top, template.top),
        shape_overlap: dice,
    }
}

/// Resample the ink of `x0..=x1` by `y0..=y1` onto a `CELL` x `CELL` grid of densities.
///
/// Each cell averages the source rectangle it covers, and never fewer than one pixel, so
/// the same routine both shrinks a large glyph and stretches a one-pixel-wide one.
///
/// Generic over the mask rather than taking `&dyn Fn`, so the closure inlines: this runs
/// `CELL * CELL` sampled rectangles per candidate, for up to [`MAX_GLYPHS_PER_PAGE`]
/// candidates on a page.
fn resample(
    mask: impl Fn(usize, usize) -> bool,
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
) -> [f32; CELL * CELL] {
    let width = x1 + 1 - x0;
    let height = y1 + 1 - y0;
    let mut out = [0f32; CELL * CELL];
    for row in 0..CELL {
        let sy0 = y0 + row * height / CELL;
        let sy1 = (y0 + (row + 1) * height / CELL).max(sy0 + 1).min(y1 + 1);
        for column in 0..CELL {
            let sx0 = x0 + column * width / CELL;
            let sx1 = (x0 + (column + 1) * width / CELL).max(sx0 + 1).min(x1 + 1);
            let mut inked = 0usize;
            let mut seen = 0usize;
            for y in sy0..sy1 {
                for x in sx0..sx1 {
                    seen += 1;
                    if mask(x, y) {
                        inked += 1;
                    }
                }
            }
            if seen > 0 {
                out[row * CELL + column] = inked as f32 / seen as f32;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The columns of the font's cell that `ch` puts ink in.
    fn glyph_extent(ch: char) -> (usize, usize) {
        let rows = font8x8::legacy::BASIC_LEGACY[ch as usize];
        let mut first = CELL;
        let mut last = 0usize;
        for bits in &rows {
            for bit in 0..CELL {
                if bits & (1 << bit) != 0 {
                    first = first.min(bit);
                    last = last.max(bit);
                }
            }
        }
        (first, last)
    }

    /// The marks of a line drawn PROPORTIONALLY: each letter takes the width of its own
    /// ink, and the space between two words is narrower than a letter.
    fn proportional(line: &str, scale: usize, letter_gap: usize, word_gap: usize) -> Vec<GlyphBox> {
        let mut out = Vec::new();
        let mut x = 0usize;
        for (index, word) in line.split(' ').enumerate() {
            if index > 0 {
                x += word_gap;
            }
            for (letter, ch) in word.chars().enumerate() {
                if letter > 0 {
                    x += letter_gap;
                }
                let (first, last) = glyph_extent(ch);
                let width = (last + 1 - first) * scale;
                out.push(GlyphBox {
                    x0: x,
                    x1: x + width - 1,
                    y0: 0,
                    y1: CELL * scale,
                });
                x += width;
            }
        }
        out
    }

    /// The marks of a line drawn MONOSPACED: every character in a cell of one width, where
    /// the gaps a letter's own ink leaves inside its cell are as wide as a space's.
    fn monospaced(line: &str, scale: usize) -> Vec<GlyphBox> {
        line.chars()
            .enumerate()
            .filter(|(_, ch)| *ch != ' ')
            .map(|(column, ch)| {
                let (first, last) = glyph_extent(ch);
                GlyphBox {
                    x0: (column * CELL + first) * scale,
                    x1: (column * CELL + last) * scale + scale - 1,
                    y0: 0,
                    y1: CELL * scale,
                }
            })
            .collect()
    }

    /// How the separators read one line's marks: one entry per pair of neighbours.
    fn separations(glyphs: &[GlyphBox]) -> Vec<Separation> {
        let median_width = median(glyphs.iter().map(GlyphBox::width)).unwrap_or(1.0);
        let gaps: Vec<usize> = glyphs
            .windows(2)
            .map(|pair| pair[1].x0.saturating_sub(pair[0].x1 + 1))
            .collect();
        let steps: Vec<usize> = glyphs
            .windows(2)
            .map(|pair| pair[1].x0.saturating_sub(pair[0].x0))
            .collect();
        let separators = Separators::of(glyphs, &gaps, &steps, median_width);
        glyphs
            .windows(2)
            .map(|pair| separators.between(&pair[0], &pair[1]))
            .collect()
    }

    /// What `line`'s own spaces separate its marks into.
    fn drawn(line: &str) -> Vec<Separation> {
        let mut out = Vec::new();
        for (index, word) in line.split(' ').enumerate() {
            if index > 0 {
                out.push(Separation::Word);
            }
            out.extend((1..word.chars().count()).map(|_| Separation::None));
        }
        out
    }

    /// The marks of a line are separated into the words the line was drawn from, whether it
    /// sets its letters in cells of one width or advances each letter by its own width, and
    /// at every size it is drawn at.
    #[test]
    fn a_lines_marks_are_separated_into_the_words_it_was_drawn_from() {
        let labelled = "Account Number: 48213";
        let capitals = "SIMPARITY LEDGER";
        let cases: Vec<(&str, &str, Vec<GlyphBox>)> = vec![
            (
                "proportional, drawn small",
                labelled,
                proportional(labelled, 1, 1, 4),
            ),
            ("proportional", labelled, proportional(labelled, 2, 1, 7)),
            (
                "proportional, drawn large with wide letter spacing",
                labelled,
                proportional(labelled, 4, 4, 22),
            ),
            (
                "proportional capitals, whose widths hardly differ",
                capitals,
                proportional(capitals, 2, 1, 7),
            ),
            (
                "monospaced, drawn small",
                labelled,
                monospaced(labelled, 1),
            ),
            ("monospaced", labelled, monospaced(labelled, 3)),
            ("monospaced capitals", capitals, monospaced(capitals, 2)),
        ];
        for (case, line, glyphs) in cases {
            assert_eq!(
                separations(&glyphs),
                drawn(line),
                "case {case:?}: the marks of {line:?} must be separated into its own words"
            );
        }
    }
}
