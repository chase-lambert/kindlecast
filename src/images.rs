use anyhow::{Context, Result, bail};
use dom_query::{Document, NodeRef};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType as PngFilterType, PngEncoder};
use image::imageops::FilterType;
use image::{
    DynamicImage, ExtendedColorType, GenericImageView, ImageDecoder, ImageEncoder, ImageFormat,
    ImageReader, Limits, metadata::Orientation,
};
use std::collections::HashMap;
use std::fs;
use std::io::{Cursor, Read};
use std::path::Path;
use std::time::Duration;
use url::Url;

use crate::sites::USER_AGENT;

const ASSET_DIR: &str = "images";
const MAX_IMAGE_EDGE: u32 = 1_600;
const JPEG_QUALITY: u8 = 82;
const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
const MAX_TOTAL_IMAGE_BYTES: u64 = 100 * 1024 * 1024;
const MAX_SOURCE_DIMENSION: u32 = 10_000;
const MAX_SOURCE_PIXELS: u64 = 25_000_000;
const MAX_DECODE_ALLOC: u64 = 128 * 1024 * 1024;
const MAX_REMOTE_FETCHES: usize = 100;
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImageStats {
    pub references: usize,
    pub localized: usize,
    pub skipped: usize,
    pub input_bytes: u64,
    pub output_bytes: u64,
}

impl ImageStats {
    pub fn summary(self) -> String {
        if self.references == 0 {
            return "no article images".to_string();
        }
        let skipped = if self.skipped == 0 {
            String::new()
        } else {
            format!("; {} omitted", self.skipped)
        };
        format!(
            "{} images; {} → {}{}",
            self.references,
            human_bytes(self.input_bytes),
            human_bytes(self.output_bytes),
            skipped
        )
    }
}

pub struct OptimizedHtml {
    pub html: String,
    pub stats: ImageStats,
}

pub fn optimize_html(
    html: &str,
    base_url: Option<&str>,
    build_dir: &Path,
    progress: &dyn Fn(&str),
) -> Result<OptimizedHtml> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(FETCH_TIMEOUT))
        .build()
        .into();
    optimize_html_with_fetch(html, base_url, build_dir, progress, |url, limit| {
        let mut response = agent
            .get(url.as_str())
            .header("User-Agent", USER_AGENT)
            .header("Accept", "image/webp,image/png,image/jpeg,image/gif")
            .call()
            .with_context(|| format!("failed to fetch image {url}"))?;
        let mut bytes = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(limit)
            .read_to_end(&mut bytes)
            .with_context(|| format!("failed to read image {url}"))?;
        Ok(bytes)
    })
}

fn optimize_html_with_fetch<F>(
    html: &str,
    base_url: Option<&str>,
    build_dir: &Path,
    progress: &dyn Fn(&str),
    mut fetch: F,
) -> Result<OptimizedHtml>
where
    F: FnMut(&Url, u64) -> Result<Vec<u8>>,
{
    let document = Document::from(html);
    document.select("picture source").remove();
    document.select("img").remove_attr("srcset");

    let image_nodes = document.select("img").nodes().to_vec();
    let mut stats = ImageStats {
        references: image_nodes.len(),
        ..ImageStats::default()
    };
    if image_nodes.is_empty() {
        return Ok(OptimizedHtml {
            html: document.html().to_string(),
            stats,
        });
    }

    progress(&format!("optimizing {} images", image_nodes.len()));
    let asset_dir = build_dir.join(ASSET_DIR);
    fs::create_dir_all(&asset_dir)
        .with_context(|| format!("failed to create image directory {}", asset_dir.display()))?;

    let base_url = base_url
        .and_then(|value| Url::parse(value).ok())
        .filter(is_http_url);
    let mut cache: HashMap<String, Option<String>> = HashMap::new();
    let mut aggregate_exhausted = false;
    let mut downloaded_bytes = 0_u64;
    let mut remote_fetches = 0_usize;

    for node in image_nodes {
        let Some(url) = image_url(&node, base_url.as_ref()) else {
            omit_image(&node);
            stats.skipped += 1;
            continue;
        };

        node.remove_attrs(&["data-src", "data-lazy-src", "srcset"]);
        if let Some(cached) = cache.get(url.as_str()) {
            match cached {
                Some(local_src) => node.set_attr("src", local_src),
                None => {
                    omit_image(&node);
                    stats.skipped += 1;
                }
            }
            continue;
        }

        if aggregate_exhausted {
            cache.insert(url.to_string(), None);
            omit_image(&node);
            stats.skipped += 1;
            continue;
        }
        if remote_fetches >= MAX_REMOTE_FETCHES {
            cache.insert(url.to_string(), None);
            omit_image(&node);
            stats.skipped += 1;
            continue;
        }

        let remaining = MAX_TOTAL_IMAGE_BYTES.saturating_sub(downloaded_bytes);
        if remaining == 0 {
            aggregate_exhausted = true;
            cache.insert(url.to_string(), None);
            omit_image(&node);
            stats.skipped += 1;
            continue;
        }
        let allowed = remaining.min(MAX_IMAGE_BYTES);
        let fetch_limit = allowed.saturating_add(1);
        remote_fetches += 1;
        let processed = match fetch(&url, fetch_limit) {
            Ok(bytes) => {
                let source_bytes = bytes.len() as u64;
                downloaded_bytes += source_bytes;
                if source_bytes > allowed {
                    aggregate_exhausted = downloaded_bytes >= MAX_TOTAL_IMAGE_BYTES;
                    Err(anyhow::anyhow!("image exceeds download budget"))
                } else {
                    optimize_image(&bytes).map(|encoded| (source_bytes, encoded))
                }
            }
            Err(error) => Err(error),
        };

        match processed {
            Ok((source_bytes, encoded)) => {
                let asset_number = stats.localized + 1;
                let filename = format!("image-{asset_number:04}.{}", encoded.extension);
                let asset_path = asset_dir.join(&filename);
                fs::write(&asset_path, &encoded.bytes).with_context(|| {
                    format!("failed to write optimized image {}", asset_path.display())
                })?;
                let local_src = format!("{ASSET_DIR}/{filename}");
                node.set_attr("src", &local_src);
                cache.insert(url.to_string(), Some(local_src));
                stats.localized += 1;
                stats.input_bytes += source_bytes;
                stats.output_bytes += encoded.bytes.len() as u64;
            }
            Err(_) => {
                cache.insert(url.to_string(), None);
                omit_image(&node);
                stats.skipped += 1;
            }
        }
    }

    Ok(OptimizedHtml {
        html: document.html().to_string(),
        stats,
    })
}

struct EncodedImage {
    bytes: Vec<u8>,
    extension: &'static str,
}

fn optimize_image(bytes: &[u8]) -> Result<EncodedImage> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .context("failed to inspect image format")?;
    let format = reader.format().context("unknown image format")?;
    if !matches!(
        format,
        ImageFormat::Gif | ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::WebP
    ) {
        bail!("unsupported image format");
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_DIMENSION);
    limits.max_image_height = Some(MAX_SOURCE_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    reader.limits(limits);

    let mut decoder = reader.into_decoder().context("failed to decode image")?;
    let (width, height) = decoder.dimensions();
    validate_decoded_image(width, height, decoder.total_bytes())?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder).context("failed to decode image pixels")?;
    image.apply_orientation(orientation);
    let image = resize_for_kindle(image);

    if has_meaningful_alpha(&image) {
        encode_png(&image)
    } else {
        encode_jpeg(&image)
    }
}

fn validate_decoded_image(width: u32, height: u32, decoded_bytes: u64) -> Result<()> {
    if width > MAX_SOURCE_DIMENSION || height > MAX_SOURCE_DIMENSION {
        bail!("image exceeds dimension budget");
    }
    if u64::from(width) * u64::from(height) > MAX_SOURCE_PIXELS {
        bail!("image exceeds decoded pixel budget");
    }
    if decoded_bytes > MAX_DECODE_ALLOC {
        bail!("image exceeds decoded allocation budget");
    }
    Ok(())
}

fn resize_for_kindle(image: DynamicImage) -> DynamicImage {
    let (width, height) = image.dimensions();
    if width <= MAX_IMAGE_EDGE && height <= MAX_IMAGE_EDGE {
        image
    } else {
        image.resize(MAX_IMAGE_EDGE, MAX_IMAGE_EDGE, FilterType::Lanczos3)
    }
}

fn has_meaningful_alpha(image: &DynamicImage) -> bool {
    if !image.color().has_alpha() {
        return false;
    }
    image.to_rgba8().pixels().any(|pixel| pixel[3] < u8::MAX)
}

fn encode_jpeg(image: &DynamicImage) -> Result<EncodedImage> {
    let rgb = image.to_rgb8();
    let (width, height) = rgb.dimensions();
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, JPEG_QUALITY)
        .write_image(&rgb, width, height, ExtendedColorType::Rgb8)
        .context("failed to encode JPEG")?;
    Ok(EncodedImage {
        bytes,
        extension: "jpg",
    })
}

fn encode_png(image: &DynamicImage) -> Result<EncodedImage> {
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    let mut bytes = Vec::new();
    PngEncoder::new_with_quality(&mut bytes, CompressionType::Best, PngFilterType::Adaptive)
        .write_image(&rgba, width, height, ExtendedColorType::Rgba8)
        .context("failed to encode PNG")?;
    Ok(EncodedImage {
        bytes,
        extension: "png",
    })
}

fn image_url(node: &NodeRef<'_>, base_url: Option<&Url>) -> Option<Url> {
    ["src", "data-src", "data-lazy-src"]
        .into_iter()
        .filter_map(|attribute| node.attr(attribute))
        .find_map(|source| resolve_image_url(source.trim(), base_url))
}

fn resolve_image_url(source: &str, base_url: Option<&Url>) -> Option<Url> {
    if source.is_empty() {
        return None;
    }
    Url::parse(source)
        .ok()
        .or_else(|| base_url.and_then(|base| base.join(source).ok()))
        .filter(is_http_url)
}

fn is_http_url(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
}

fn omit_image(node: &NodeRef<'_>) {
    let alt = node
        .attr("alt")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let label = match alt {
        Some(alt) => format!("Image omitted: {alt}"),
        None => "Image omitted".to_string(),
    };
    node.replace_with_html(format!(
        "<span class=\"image-omitted\">[{}]</span>",
        html_escape::encode_text(&label)
    ));
}

fn human_bytes(bytes: u64) -> String {
    const MIB: f64 = (1024 * 1024) as f64;
    if bytes >= 1024 * 1024 {
        format!("{:.2} MiB", bytes as f64 / MIB)
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb, Rgba};
    use std::cell::RefCell;
    use tempfile::tempdir;

    fn opaque_png(width: u32, height: u32) -> Vec<u8> {
        let image = DynamicImage::ImageRgb8(ImageBuffer::from_fn(width, height, |x, y| {
            Rgb([(x % 255) as u8, (y % 255) as u8, 120])
        }));
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        bytes
    }

    fn transparent_png() -> Vec<u8> {
        let image =
            DynamicImage::ImageRgba8(ImageBuffer::from_pixel(2, 2, Rgba([20, 40, 60, 100])));
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        bytes
    }

    #[test]
    fn opaque_images_become_bounded_jpegs() {
        let encoded = optimize_image(&opaque_png(2_000, 1_000)).unwrap();
        assert_eq!(encoded.extension, "jpg");
        let decoded = image::load_from_memory(&encoded.bytes).unwrap();
        assert_eq!(decoded.dimensions(), (1_600, 800));
    }

    #[test]
    fn meaningful_transparency_stays_png() {
        let encoded = optimize_image(&transparent_png()).unwrap();
        assert_eq!(encoded.extension, "png");
        assert_eq!(
            image::guess_format(&encoded.bytes).unwrap(),
            ImageFormat::Png
        );
    }

    #[test]
    fn unused_alpha_channel_does_not_force_png() {
        let image =
            DynamicImage::ImageRgba8(ImageBuffer::from_pixel(2, 2, Rgba([20, 40, 60, 255])));
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();

        let encoded = optimize_image(&bytes).unwrap();

        assert_eq!(encoded.extension, "jpg");
    }

    #[test]
    fn rewrites_lazy_images_and_closes_remote_sources() {
        let dir = tempdir().unwrap();
        let fetched_urls = RefCell::new(Vec::new());
        let html = r#"<!doctype html><html><body>
            <h1 class="t-head">Chapter</h1>
            <picture>
              <source srcset="https://cdn.example/huge.png 2x">
              <img src="data:image/gif;base64,placeholder"
                   data-src="/photo.png"
                   srcset="https://cdn.example/huge.png 2x"
                   alt="Expo">
            </picture>
        </body></html>"#;

        let result = optimize_html_with_fetch(
            html,
            Some("https://example.com/posts/one"),
            dir.path(),
            &|_| {},
            |url, _| {
                fetched_urls.borrow_mut().push(url.to_string());
                Ok(opaque_png(20, 10))
            },
        )
        .unwrap();

        assert_eq!(
            fetched_urls.into_inner(),
            vec!["https://example.com/photo.png"]
        );
        assert!(result.html.contains("images/image-0001.jpg"));
        assert!(!result.html.contains("https://cdn.example"));
        assert!(!result.html.contains("data-src"));
        assert!(!result.html.contains("srcset"));
        assert!(result.html.contains(r#"<h1 class="t-head">Chapter</h1>"#));
    }

    #[test]
    fn failed_images_leave_escaped_alt_context() {
        let dir = tempdir().unwrap();
        let result = optimize_html_with_fetch(
            r#"<html><body><img src="https://example.com/nope" alt="A &amp; B"></body></html>"#,
            None,
            dir.path(),
            &|_| {},
            |_, _| Err(anyhow::anyhow!("offline")),
        )
        .unwrap();

        assert!(result.html.contains("[Image omitted: A &amp; B]"));
        assert!(!result.html.contains("https://example.com/nope"));
        assert_eq!(result.stats.skipped, 1);
    }

    #[test]
    fn oversized_download_is_omitted() {
        let dir = tempdir().unwrap();
        let result = optimize_html_with_fetch(
            r#"<html><body><img src="https://example.com/huge.png"></body></html>"#,
            None,
            dir.path(),
            &|_| {},
            |_, limit| Ok(vec![0; limit as usize]),
        )
        .unwrap();

        assert_eq!(result.stats.localized, 0);
        assert_eq!(result.stats.skipped, 1);
        assert!(!result.html.contains("https://example.com/huge.png"));
    }

    #[test]
    fn aggregate_budget_counts_downloads_that_fail_to_decode() {
        let dir = tempdir().unwrap();
        let html = (0..60)
            .map(|index| format!(r#"<img src="https://example.com/{index}.bin">"#))
            .collect::<String>();
        let fetches = RefCell::new(0_usize);

        let result = optimize_html_with_fetch(&html, None, dir.path(), &|_| {}, |_, _| {
            *fetches.borrow_mut() += 1;
            Ok(vec![0; 2 * 1024 * 1024])
        })
        .unwrap();

        assert_eq!(*fetches.borrow(), 50);
        assert_eq!(result.stats.skipped, 60);
        assert!(!result.html.contains("https://example.com"));
    }

    #[test]
    fn remote_fetch_count_is_bounded_even_when_requests_fail() {
        let dir = tempdir().unwrap();
        let html = (0..101)
            .map(|index| format!(r#"<img src="https://example.com/{index}.png">"#))
            .collect::<String>();
        let fetches = RefCell::new(0_usize);

        let result = optimize_html_with_fetch(&html, None, dir.path(), &|_| {}, |_, _| {
            *fetches.borrow_mut() += 1;
            Err(anyhow::anyhow!("timeout"))
        })
        .unwrap();

        assert_eq!(*fetches.borrow(), MAX_REMOTE_FETCHES);
        assert_eq!(result.stats.skipped, 101);
    }

    #[test]
    fn decoded_image_limits_cover_dimensions_pixels_and_allocation() {
        assert!(validate_decoded_image(1_600, 1_600, 10_000_000).is_ok());
        assert!(validate_decoded_image(MAX_SOURCE_DIMENSION + 1, 1, 1).is_err());
        assert!(validate_decoded_image(5_001, 5_000, 1).is_err());
        assert!(validate_decoded_image(1, 1, MAX_DECODE_ALLOC + 1).is_err());
    }
}
