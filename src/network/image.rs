use super::*;

const MAX_IMAGE_DOWNLOAD_BYTES: usize = 2 * 1024 * 1024;
const MAX_IMAGE_DIMENSION: u32 = 2_048;
const MAX_IMAGE_DECODE_ALLOC: u64 = 32 * 1024 * 1024;
pub(super) const CACHED_IMAGE_SIZE: u32 = 128;

pub(super) async fn fetch_image(http: &HttpClient, url: &str) -> anyhow::Result<DynamicImage> {
    let mut response = http.get(url).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_IMAGE_DOWNLOAD_BYTES as u64)
    {
        anyhow::bail!("image response exceeds download limit");
    }

    let capacity = response
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
        .unwrap_or(0)
        .min(MAX_IMAGE_DOWNLOAD_BYTES);
    let mut bytes = Vec::with_capacity(capacity);
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > MAX_IMAGE_DOWNLOAD_BYTES {
            anyhow::bail!("image response exceeds download limit");
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.is_empty() {
        anyhow::bail!("empty image response");
    }

    tokio::task::spawn_blocking(move || decode_image(bytes))
        .await
        .map_err(|error| anyhow::anyhow!("image decoder task failed: {error}"))?
}

pub(super) fn decode_image(bytes: Vec<u8>) -> anyhow::Result<DynamicImage> {
    decode_raster_image(&bytes).or_else(|raster_error| {
        decode_svg_image(&bytes).map_err(|svg_error| {
            anyhow::anyhow!("unsupported raster image ({raster_error}); invalid SVG ({svg_error})")
        })
    })
}

fn decode_raster_image(bytes: &[u8]) -> anyhow::Result<DynamicImage> {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_IMAGE_DECODE_ALLOC);

    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    reader.limits(limits);
    let image = reader.decode()?;
    // Retain only a small, normalized RGBA image. The full decoded source is
    // dropped before the event crosses into the UI task.
    Ok(DynamicImage::ImageRgba8(
        image
            .resize(CACHED_IMAGE_SIZE, CACHED_IMAGE_SIZE, FilterType::Triangle)
            .to_rgba8(),
    ))
}

fn decode_svg_image(bytes: &[u8]) -> anyhow::Result<DynamicImage> {
    // `from_data_nested` deliberately ignores external file references. The
    // SVG itself is untrusted relay content and must not read local resources.
    let tree = resvg::usvg::Tree::from_data_nested(bytes, &resvg::usvg::Options::default())?;
    let source_size = tree.size();
    let source_width = source_size.width();
    let source_height = source_size.height();
    if !source_width.is_finite()
        || !source_height.is_finite()
        || source_width <= 0.0
        || source_height <= 0.0
        || source_width > MAX_IMAGE_DIMENSION as f32
        || source_height > MAX_IMAGE_DIMENSION as f32
    {
        anyhow::bail!("SVG dimensions are invalid or exceed the limit");
    }

    let scale =
        (CACHED_IMAGE_SIZE as f32 / source_width).min(CACHED_IMAGE_SIZE as f32 / source_height);
    let width = (source_width * scale).round().max(1.0) as u32;
    let height = (source_height * scale).round().max(1.0) as u32;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| anyhow::anyhow!("could not allocate SVG pixmap"))?;
    let transform = resvg::tiny_skia::Transform::from_scale(
        width as f32 / source_width,
        height as f32 / source_height,
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    let pixels = pixmap.take_demultiplied();
    let image = RgbaImage::from_raw(width, height, pixels)
        .ok_or_else(|| anyhow::anyhow!("SVG renderer returned an invalid pixel buffer"))?;
    Ok(DynamicImage::ImageRgba8(image))
}
