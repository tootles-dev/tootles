//! Original image loading shared by terminal previews and native viewers.
//!
//! Downloads never use the account HTTP client. Encoded bytes are preserved
//! exactly, while a bounded decode validates them and prepares a small terminal
//! preview. Opening a viewer can therefore retain resolution, metadata, and GIF
//! animation without bypassing the existing image resource limits.
use std::{
    io::{Cursor, Read},
    path::PathBuf,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use image::{DynamicImage, ImageFormat, ImageReader, Limits};
use url::Url;

pub const MAX_DOWNLOAD_BYTES: usize = 16 * 1024 * 1024;
const MAX_PIXELS: u64 = 40_000_000;
const MAX_DIMENSION: u32 = 16_384;
const MAX_PREVIEW_DIMENSION: u32 = 2_048;

/// Local files must be selected explicitly; a remote URL cannot become a file
/// path or invoke another protocol.
#[derive(Debug, Clone)]
pub enum Source {
    Remote(String),
    Local(PathBuf),
}

#[derive(Debug)]
pub struct OriginalImage {
    /// Exact encoded bytes, suitable for writing to a native-viewer temp file.
    bytes: Vec<u8>,
    /// A decoded preview, limited to 2048 pixels per side, for terminal use.
    image: DynamicImage,
    /// Extension inferred from validated content, never a remote filename.
    extension: &'static str,
}

impl OriginalImage {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn extension(&self) -> &'static str {
        self.extension
    }

    pub fn preview(&self) -> &DynamicImage {
        &self.image
    }

    pub fn into_preview(self) -> DynamicImage {
        self.image
    }
}

/// Load and validate an explicitly requested image without account credentials.
/// Decoding and local filesystem reads run outside the async UI worker.
pub async fn load_original(source: Source) -> Result<OriginalImage> {
    match source {
        Source::Local(path) => tokio::task::spawn_blocking(move || {
            let metadata = path
                .metadata()
                .context("Could not inspect the local image")?;
            if !metadata.is_file() {
                bail!("Select a regular image file");
            }
            if metadata.len() > MAX_DOWNLOAD_BYTES as u64 {
                bail!("Image exceeds the 16 MiB preview limit");
            }
            let mut bytes = Vec::new();
            std::fs::File::open(&path)
                .context("Could not open the local image")?
                .take((MAX_DOWNLOAD_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .context("Could not read the local image")?;
            decode_original(bytes)
        })
        .await
        .context("Local image decoding worker failed")?,
        Source::Remote(value) => {
            let bytes = match value.as_str() {
                "demo://desktop" => include_bytes!("../assets/demo-desktop.png").to_vec(),
                "demo://trail" => include_bytes!("../assets/demo-trail.png").to_vec(),
                _ => download(&value).await?,
            };
            tokio::task::spawn_blocking(move || decode_original(bytes))
                .await
                .context("Image decoding worker failed")?
        }
    }
}

async fn download(value: &str) -> Result<Vec<u8>> {
    let url = media_url(value)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(25))
        .connect_timeout(Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() > 3 || media_url(attempt.url().as_str()).is_err() {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .user_agent(concat!("tootles/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let mut response = client
        .get(url)
        .send()
        .await
        .context("Could not download image")?;
    if !response.status().is_success() {
        bail!("Image server returned HTTP {}", response.status());
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_DOWNLOAD_BYTES as u64)
    {
        bail!("Image exceeds the 16 MiB preview limit; open it in your browser");
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .context("Image download was interrupted")?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_DOWNLOAD_BYTES {
            bail!("Image exceeds the 16 MiB preview limit; open it in your browser");
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// Validate original bytes and retain them beside a decoded terminal preview.
/// This performs CPU work; async callers should prefer `load_original`, which
/// dispatches decoding to a blocking worker.
pub fn decode_original(bytes: Vec<u8>) -> Result<OriginalImage> {
    let (image, extension) = decode(&bytes)?;
    Ok(OriginalImage {
        bytes,
        image,
        extension,
    })
}

/// Decode a bounded thumbnail without cloning the encoded image. Kept public
/// for the existing media preview API and deterministic decode tests.
pub fn decode_preview(bytes: &[u8]) -> Result<DynamicImage> {
    decode(bytes).map(|(image, _)| image)
}

fn decode(bytes: &[u8]) -> Result<(DynamicImage, &'static str)> {
    if bytes.len() > MAX_DOWNLOAD_BYTES {
        bail!("Image exceeds the 16 MiB preview limit");
    }
    let probe = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .context("Unrecognized image format")?;
    let format = probe.format().context("Unrecognized image format")?;
    let (width, height) = probe
        .into_dimensions()
        .context("Could not read image dimensions")?;
    if u64::from(width) * u64::from(height) > MAX_PIXELS
        || width > MAX_DIMENSION
        || height > MAX_DIMENSION
    {
        bail!("Image exceeds the 40 megapixel preview limit");
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(256 * 1024 * 1024);
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    reader.limits(limits);
    let image = reader.decode().context("Could not decode image")?;
    let extension = match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Gif => "gif",
        ImageFormat::WebP => "webp",
        // Only these codecs are enabled. A newly enabled format needs an
        // explicit extension before it can be handed to an external viewer.
        _ => bail!("This image format is not supported"),
    };
    let preview = if image.width() > MAX_PREVIEW_DIMENSION || image.height() > MAX_PREVIEW_DIMENSION
    {
        image.thumbnail(MAX_PREVIEW_DIMENSION, MAX_PREVIEW_DIMENSION)
    } else {
        image
    };
    Ok((preview, extension))
}

fn media_url(value: &str) -> Result<Url> {
    if value.chars().any(char::is_control) {
        bail!("Invalid image URL");
    }
    let url = Url::parse(value).context("Invalid image URL")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        bail!("Images must use an HTTP or HTTPS URL without embedded credentials");
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Frame, RgbaImage, codecs::gif::GifEncoder};
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::new_rgb8(width, height)
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }

    #[tokio::test]
    async fn original_resolution_and_encoded_bytes_survive_preview_downsampling() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory
            .path()
            .join("a photo with spaces.untrusted-extension");
        let bytes = png(3000, 100);
        std::fs::write(&file, &bytes).unwrap();
        let original = load_original(Source::Local(file)).await.unwrap();
        assert_eq!(original.bytes, bytes);
        assert_eq!(original.extension, "png");
        assert_eq!(original.image.width(), MAX_PREVIEW_DIMENSION);
        assert_eq!(
            ImageReader::new(Cursor::new(&original.bytes))
                .with_guessed_format()
                .unwrap()
                .into_dimensions()
                .unwrap(),
            (3000, 100)
        );
    }

    #[test]
    fn animation_bytes_are_retained_while_preview_uses_one_frame() {
        let mut bytes = Vec::new();
        {
            let mut encoder = GifEncoder::new(&mut bytes);
            encoder
                .encode_frame(Frame::new(RgbaImage::from_pixel(
                    3,
                    2,
                    image::Rgba([255, 0, 0, 255]),
                )))
                .unwrap();
            encoder
                .encode_frame(Frame::new(RgbaImage::from_pixel(
                    3,
                    2,
                    image::Rgba([0, 0, 255, 255]),
                )))
                .unwrap();
        }
        let original = decode_original(bytes.clone()).unwrap();
        assert_eq!(original.bytes, bytes);
        assert_eq!(original.extension, "gif");
        assert_eq!((original.image.width(), original.image.height()), (3, 2));
        assert_eq!(
            original.image.to_rgba8().get_pixel(0, 0).0,
            [255, 0, 0, 255]
        );
    }

    #[tokio::test]
    async fn redirect_download_preserves_original_bytes_without_auth_or_cookies() {
        let origin = MockServer::start().await;
        let destination = MockServer::start().await;
        let bytes = png(3, 2);
        Mock::given(path("/start"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("Location", format!("{}/original.png", destination.uri()))
                    .insert_header("Set-Cookie", "session=do-not-forward"),
            )
            .expect(1)
            .mount(&origin)
            .await;
        Mock::given(path("/original.png"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes.clone()))
            .expect(1)
            .mount(&destination)
            .await;
        let original = load_original(Source::Remote(format!("{}/start", origin.uri())))
            .await
            .unwrap();
        assert_eq!(original.bytes, bytes);
        assert_eq!(original.extension, "png");
        for server in [&origin, &destination] {
            let requests = server.received_requests().await.unwrap();
            assert_eq!(requests.len(), 1);
            assert!(!requests[0].headers.contains_key("authorization"));
            assert!(!requests[0].headers.contains_key("cookie"));
        }
    }

    #[tokio::test]
    async fn original_loading_rejects_large_invalid_and_arbitrary_protocol_sources() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("oversized.png");
        std::fs::File::create(&file)
            .unwrap()
            .set_len((MAX_DOWNLOAD_BYTES + 1) as u64)
            .unwrap();
        assert!(
            load_original(Source::Local(file))
                .await
                .unwrap_err()
                .to_string()
                .contains("16 MiB")
        );
        assert!(
            load_original(Source::Local(directory.path().into()))
                .await
                .is_err()
        );
        for url in [
            "file:///etc/passwd",
            "data:image/png;base64,AAAA",
            "https://user:secret@example.org/image.png",
            "demo://unknown",
            "https://example.org/\n",
        ] {
            assert!(load_original(Source::Remote(url.into())).await.is_err());
        }
        assert!(decode_preview(&png(16_385, 1)).is_err());
        assert!(decode_preview(b"not an image").is_err());
    }

    #[tokio::test]
    async fn demo_originals_are_exact_bundled_bytes() {
        for (url, expected) in [
            (
                "demo://desktop",
                include_bytes!("../assets/demo-desktop.png").as_slice(),
            ),
            (
                "demo://trail",
                include_bytes!("../assets/demo-trail.png").as_slice(),
            ),
        ] {
            let original = load_original(Source::Remote(url.into())).await.unwrap();
            assert_eq!(original.bytes, expected);
            assert_eq!(original.extension, "png");
        }
    }
}
