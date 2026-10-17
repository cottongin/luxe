//! Reading a file that turns out to be a picture.
//!
//! Split out of [`super`] because it answers a different question from the rest of `read`: not
//! "which lines" but "is this even text, and if not, can it be looked at". Nothing here touches the
//! offset/limit machinery, and the caller only has to ask once, before it opens the file as text.

use luxe_core::ContentBlock;
use luxe_core::ToolResultData;
use std::path::Path;

/// Longest edge (px) of the copy handed to the model.
///
/// The same bound `screenshot` uses, and for the same reason: it is Anthropic's recommended maximum
/// long edge, past which an image costs tokens without carrying more detail.
const MAX_INLINE_DIM: u32 = 1568;

/// Bytes a file may hold before `read` declines to decode it.
///
/// A read is a foreground call in a turn the user is waiting on, and a 60 MB TIFF is not something
/// anyone meant to look at inline.
const MAX_FILE_BYTES: u64 = 24 * 1024 * 1024;

/// Pixels an image may decode to. Four bytes each, so this is the real memory bound.
const MAX_PIXELS: u64 = 40_000_000;

/// What [`read`] concluded about the file.
pub(super) enum Outcome {
    /// Not an image; the caller should read it as text.
    NotAnImage,
    /// An image, and this is the answer — including when it could not be decoded, because at that
    /// point "this is a broken PNG" is the useful thing to say rather than "this is not UTF-8".
    Read(ToolResultData),
}

/// Reads `path` as an image, or reports that it is not one.
///
/// `display` is the path as the caller wrote it, so the result talks about the file they asked for
/// rather than the absolute path it resolved to.
pub(super) fn read(path: &Path, display: &str) -> Outcome {
    let Some(format) = sniff(path) else {
        return Outcome::NotAnImage;
    };
    let size = std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    if size > MAX_FILE_BYTES {
        return Outcome::Read(ToolResultData::error(format!(
            "'{display}' is a {} image of {} MB, too large to read inline. Open it outside the \
             session, or resize a copy first.",
            format_name(format),
            size / (1024 * 1024)
        )));
    }

    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return Outcome::Read(ToolResultData::error(format!(
                "Error reading image '{display}': {error}"
            )));
        }
    };
    let decoded = match image::load_from_memory_with_format(&bytes, format) {
        Ok(decoded) => decoded,
        Err(error) => {
            return Outcome::Read(ToolResultData::error(format!(
                "'{display}' looks like a {} image but could not be decoded: {error}",
                format_name(format)
            )));
        }
    };

    let (width, height) = (decoded.width(), decoded.height());
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Outcome::Read(ToolResultData::error(format!(
            "'{display}' is {width}x{height}, too many pixels to read inline."
        )));
    }

    // The saved file stays untouched; only the copy that travels to the model is bounded.
    let inline = if width > MAX_INLINE_DIM || height > MAX_INLINE_DIM {
        decoded.resize(MAX_INLINE_DIM, MAX_INLINE_DIM, image::imageops::FilterType::Lanczos3)
    } else {
        decoded
    };
    let mut png = std::io::Cursor::new(Vec::new());
    if let Err(error) = inline.write_to(&mut png, image::ImageFormat::Png) {
        return Outcome::Read(ToolResultData::error(format!(
            "'{display}' could not be re-encoded: {error}"
        )));
    }

    let scaled = (inline.width(), inline.height()) != (width, height);
    let mut summary =
        format!("{display}: {} image, {width}x{height}, {} KB.", format_name(format), size / 1024);
    if scaled {
        summary.push_str(&format!(
            " Shown at {}x{} \u{2014} the file is unchanged.",
            inline.width(),
            inline.height()
        ));
    }
    // Said out loud because a text-only model receives the sentence and nothing else, and silence
    // would leave it guessing why a file it asked for came back almost empty.
    summary.push_str(" The image itself follows; a model without vision sees only this line.");

    use base64::Engine as _;
    Outcome::Read(ToolResultData {
        content: vec![
            ContentBlock::Text(summary),
            ContentBlock::Image {
                data: base64::engine::general_purpose::STANDARD.encode(png.into_inner()),
                mime: "image/png".to_string(),
                name: Some(luxe_core::paths::display(path)),
            },
        ],
        is_error: false,
    })
}

/// The image format `path` really holds, by its magic bytes.
///
/// Content rather than extension: a `.txt` holding a PNG is a PNG, and a `.png` holding markup is
/// markup. Reading a header is also the only bound that holds before a decode — an extension check
/// would happily hand a decompression bomb to the decoder.
fn sniff(path: &Path) -> Option<image::ImageFormat> {
    use std::io::Read as _;
    let mut header = [0u8; 16];
    let mut file = std::fs::File::open(path).ok()?;
    let read = file.read(&mut header).ok()?;
    let format = image::guess_format(&header[..read]).ok()?;
    // Only the formats this build actually links a decoder for; anything else is not an image as
    // far as `read` is concerned, and saying so beats a decode error the caller cannot act on.
    matches!(
        format,
        image::ImageFormat::Png
            | image::ImageFormat::Jpeg
            | image::ImageFormat::Gif
            | image::ImageFormat::WebP
    )
    .then_some(format)
}

fn format_name(format: image::ImageFormat) -> &'static str {
    match format {
        image::ImageFormat::Png => "PNG",
        image::ImageFormat::Jpeg => "JPEG",
        image::ImageFormat::Gif => "GIF",
        image::ImageFormat::WebP => "WebP",
        _ => "image",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_png(dir: &Path, name: &str, width: u32, height: u32) -> std::path::PathBuf {
        let path = dir.join(name);
        let pixels = image::RgbaImage::from_pixel(width, height, image::Rgba([9, 40, 120, 255]));
        image::DynamicImage::ImageRgba8(pixels).save(&path).expect("write png");
        path
    }

    fn image_block(result: &ToolResultData) -> Option<(&String, &String)> {
        result.content.iter().find_map(|block| match block {
            ContentBlock::Image { data, mime, .. } => Some((data, mime)),
            _ => None,
        })
    }

    fn text_of(result: &ToolResultData) -> String {
        result
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    /// The point of the whole module: an image file comes back as an image, with a sentence a
    /// text-only model can still act on.
    #[test]
    fn an_image_file_comes_back_as_an_image() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = write_png(dir.path(), "design.png", 40, 20);

        let Outcome::Read(result) = read(&path, "design.png") else {
            panic!("a png is an image");
        };
        assert!(!result.is_error);
        let (data, mime) = image_block(&result).expect("an image block");
        assert_eq!(mime, "image/png");
        assert!(!data.is_empty());

        let text = text_of(&result);
        assert!(text.contains("design.png") && text.contains("40x20"), "{text}");
        assert!(text.contains("without vision"), "a text-only model is told what it is missing");
    }

    /// The image block carries where it came from, so a front-end can offer to open the real file
    /// and a model looking at three pictures can tell them apart.
    #[test]
    fn an_image_block_names_the_file_it_came_from() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = write_png(dir.path(), "chart.png", 8, 8);
        let Outcome::Read(result) = read(&path, "chart.png") else { panic!("an image") };
        let named = result.content.iter().any(|block| {
            matches!(block, ContentBlock::Image { name: Some(name), .. } if name.ends_with("chart.png"))
        });
        assert!(named, "the image block names its file");
    }

    /// Format is decided by content. An extension check would call this text and hand a decoder a
    /// file it never validated.
    #[test]
    fn the_contents_decide_the_format_not_the_extension() {
        let dir = tempfile::tempdir().expect("temp dir");
        let lying = write_png(dir.path(), "notes.png", 4, 4);
        std::fs::rename(&lying, dir.path().join("notes.txt")).expect("rename");
        let renamed = dir.path().join("notes.txt");
        assert!(
            matches!(read(&renamed, "notes.txt"), Outcome::Read(_)),
            "a PNG named .txt is a PNG"
        );

        let markup = dir.path().join("page.png");
        std::fs::write(&markup, "<html>not really a png</html>").expect("write");
        assert!(
            matches!(read(&markup, "page.png"), Outcome::NotAnImage),
            "markup named .png is markup, and the text path should have it"
        );
    }

    /// Source files must be untouched by any of this, or every read pays a header probe's worth of
    /// wrong answers.
    #[test]
    fn ordinary_files_are_left_to_the_text_path() {
        let dir = tempfile::tempdir().expect("temp dir");
        for (name, body) in
            [("lib.rs", "fn main() {}"), ("empty.txt", ""), ("data.json", "{\"a\":1}")]
        {
            let path = dir.path().join(name);
            std::fs::write(&path, body).expect("write");
            assert!(matches!(read(&path, name), Outcome::NotAnImage), "{name}");
        }
        assert!(matches!(read(&dir.path().join("gone.png"), "gone.png"), Outcome::NotAnImage));
    }

    /// A big picture is scaled for the model and the file is left alone — and the summary says so,
    /// so nobody concludes the file on disk was rewritten.
    #[test]
    fn a_large_image_is_scaled_for_the_model_and_says_so() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = write_png(dir.path(), "huge.png", MAX_INLINE_DIM + 400, 600);

        let Outcome::Read(result) = read(&path, "huge.png") else { panic!("an image") };
        let text = text_of(&result);
        assert!(text.contains(&format!("{}x600", MAX_INLINE_DIM + 400)), "true size: {text}");
        assert!(text.contains("Shown at"), "and the size it was shown at: {text}");
        assert!(text.contains("file is unchanged"), "{text}");

        use base64::Engine as _;
        let (data, _) = image_block(&result).expect("an image block");
        let decoded = base64::engine::general_purpose::STANDARD.decode(data).expect("base64");
        let inline = image::load_from_memory(&decoded).expect("a png");
        assert!(inline.width() <= MAX_INLINE_DIM && inline.height() <= MAX_INLINE_DIM);
    }

    /// A file that claims to be a picture and is not decodable is an error about *that*, not a
    /// UTF-8 complaint from the text path.
    #[test]
    fn a_corrupt_image_says_what_is_wrong_with_it() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("truncated.png");
        // A valid PNG signature and header, then nothing.
        std::fs::write(&path, [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13])
            .expect("write");

        let Outcome::Read(result) = read(&path, "truncated.png") else {
            panic!("it announced itself as a png");
        };
        assert!(result.is_error);
        let text = text_of(&result);
        assert!(text.contains("PNG") && text.contains("could not be decoded"), "{text}");
        assert!(!text.contains("UTF-8"), "not the text path's words: {text}");
    }
}
