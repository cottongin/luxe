//! Fixture capture — the screenshot a tool card shows, drawn rather than loaded.

/// What the fixture's agent was asked, did, and verified — the brief's raw material.
/// The picture the `screenshot` fixture pretends to have captured.
///
/// Drawn rather than loaded: a fixture that reads a PNG off disk is a fixture that renders
/// differently depending on what is in the tree. This is a plausible dev-server page — a header, a
/// sidebar, and a column of cards — at a size that exercises real downscaling.
pub(super) fn fixture_capture() -> Vec<u8> {
    const WIDTH: u32 = 1280;
    const HEIGHT: u32 = 800;
    let paint = |x: u32, y: u32| -> [u8; 4] {
        let card = |top: u32, height: u32| y >= top && y < top + height && (300..1200).contains(&x);
        match () {
            _ if y < 72 => [24, 28, 44, 255],    // header bar
            _ if y < 80 => [122, 162, 247, 255], // accent underline
            _ if x < 260 => [30, 34, 52, 255],   // sidebar
            _ if card(140, 180) => [46, 52, 76, 255],
            _ if card(360, 120) => [46, 52, 76, 255],
            _ if card(520, 200) => [46, 52, 76, 255],
            _ => [18, 20, 32, 255], // page background
        }
    };
    let mut pixels = image::RgbaImage::new(WIDTH, HEIGHT);
    for (x, y, pixel) in pixels.enumerate_pixels_mut() {
        *pixel = image::Rgba(paint(x, y));
    }
    // A few sidebar rows and a title block, so the downscale has edges to resolve.
    for row in 0..6u32 {
        let top = 120 + row * 56;
        for y in top..top + 18 {
            for x in 40..220 {
                pixels.put_pixel(x, y, image::Rgba([88, 96, 130, 255]));
            }
        }
    }
    for y in 140..176 {
        for x in 330..820 {
            pixels.put_pixel(x, y, image::Rgba([158, 206, 106, 255]));
        }
    }

    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(pixels)
        .write_to(&mut png, image::ImageFormat::Png)
        .expect("the fixture capture encodes");
    png.into_inner()
}
