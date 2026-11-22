//! Original image loading shared by terminal previews and native viewers.
//!
//! Downloads never use the account HTTP client. Encoded bytes are preserved
//! exactly, while a bounded decode validates them and prepares a small terminal
//! preview. Opening a viewer can therefore retain resolution, metadata, and GIF
//! animation without bypassing the existing image resource limits.
use std::{
    io::{Cursor, Read},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use image::{DynamicImage, ImageFormat, ImageReader, Limits};
use reqwest::header::LOCATION;
use url::{Host, Url};

pub const MAX_DOWNLOAD_BYTES: usize = 16 * 1024 * 1024;
const MAX_PIXELS: u64 = 40_000_000;
const MAX_DIMENSION: u32 = 16_384;
const MAX_PREVIEW_DIMENSION: u32 = 2_048;
const MAX_REDIRECTS: usize = 4;

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
    let url = public_media_url(value)?;
    tokio::time::timeout(
        Duration::from_secs(25),
        download_with_resolver(url, &PublicResolver),
    )
    .await
    .context("Image download timed out")?
}

#[derive(Debug)]
struct ResolvedTarget {
    host: String,
    addresses: Vec<SocketAddr>,
}

#[async_trait]
trait MediaResolver: Sync {
    async fn resolve(&self, url: &Url) -> Result<ResolvedTarget>;
}

struct PublicResolver;

#[async_trait]
impl MediaResolver for PublicResolver {
    async fn resolve(&self, url: &Url) -> Result<ResolvedTarget> {
        let host = match url.host().context("Image URL has no host")? {
            Host::Domain(host) => host.to_owned(),
            Host::Ipv4(address) => address.to_string(),
            Host::Ipv6(address) => address.to_string(),
        };
        let port = url
            .port_or_known_default()
            .context("Image URL has no network port")?;
        let addresses: Vec<_> = tokio::net::lookup_host((host.as_str(), port))
            .await
            .context("Could not resolve image server")?
            .collect();
        resolved_public_target(host, addresses)
    }
}

fn resolved_public_target(
    host: String,
    addresses: impl IntoIterator<Item = SocketAddr>,
) -> Result<ResolvedTarget> {
    let mut addresses: Vec<_> = addresses
        .into_iter()
        .filter(|address| public_network_address(address.ip()))
        .collect();
    addresses.sort_unstable();
    addresses.dedup();
    if addresses.is_empty() {
        bail!("Image URL must resolve to a public network address");
    }
    Ok(ResolvedTarget { host, addresses })
}

async fn download_with_resolver(mut url: Url, resolver: &impl MediaResolver) -> Result<Vec<u8>> {
    for redirect_count in 0..=MAX_REDIRECTS {
        let target = resolver.resolve(&url).await?;
        let mut response = request(&url, &target).await?;
        if let Some(location) = redirect_location(&response, &url)? {
            if redirect_count == MAX_REDIRECTS {
                bail!("Image server redirected too many times");
            }
            url = public_media_url(location.as_str())?;
            continue;
        }
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
        return Ok(bytes);
    }
    unreachable!("the redirect loop returns or fails at its explicit limit")
}

async fn request(url: &Url, target: &ResolvedTarget) -> Result<reqwest::Response> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::none())
        // A proxy could resolve the origin again and bypass the vetted, pinned
        // address set. Media previews therefore always connect directly.
        .no_proxy()
        .resolve_to_addrs(&target.host, &target.addresses)
        .user_agent(concat!("tootles/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let response = client
        .get(url.clone())
        .send()
        .await
        .context("Could not download image")?;
    let remote = response
        .remote_addr()
        .context("Could not verify the image server's network address")?;
    if !target
        .addresses
        .iter()
        .any(|address| same_endpoint(*address, remote))
    {
        bail!("Image server connected through an unverified network address");
    }
    Ok(response)
}

fn redirect_location(response: &reqwest::Response, base: &Url) -> Result<Option<Url>> {
    if !matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
        return Ok(None);
    }
    let location = response
        .headers()
        .get(LOCATION)
        .context("Image redirect did not include a location")?
        .to_str()
        .context("Image redirect location is invalid")?;
    Ok(Some(
        base.join(location)
            .context("Image redirect location is invalid")?,
    ))
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

pub(crate) fn public_media_url(value: &str) -> Result<Url> {
    if value.chars().any(char::is_control) {
        bail!("Invalid media URL");
    }
    let url = Url::parse(value).context("Invalid media URL")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        bail!("Media must use an HTTP or HTTPS URL without embedded credentials");
    }
    let literal_address = match url.host().context("Media URL has no host")? {
        Host::Ipv4(address) => Some(IpAddr::V4(address)),
        Host::Ipv6(address) => Some(IpAddr::V6(address)),
        Host::Domain(_) => None,
    };
    if literal_address.is_some_and(|address| !public_network_address(address)) {
        bail!("Media URL must use a public network address");
    }
    Ok(url)
}

fn same_endpoint(expected: SocketAddr, actual: SocketAddr) -> bool {
    expected.port() == actual.port() && normalize_ip(expected.ip()) == normalize_ip(actual.ip())
}

fn normalize_ip(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(address) => address
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(address)),
        address => address,
    }
}

/// Accept ordinary globally routable unicast addresses and reject every local,
/// private, link-local, multicast, documentation, transition, and reserved
/// range. DNS names are pinned to the filtered result before connecting.
fn public_network_address(address: IpAddr) -> bool {
    match normalize_ip(address) {
        IpAddr::V4(address) => public_ipv4(address),
        IpAddr::V6(address) => {
            let segments = address.segments();
            // Global IPv6 unicast is 2000::/3. Exclude special-use ranges
            // within it as well, including IETF assignments, documentation,
            // 6to4, and the 3fff::/20 documentation block.
            (0x2000..=0x3fff).contains(&segments[0])
                && !(segments[0] == 0x2001 && segments[1] <= 0x01ff)
                && !(segments[0] == 0x2001 && segments[1] == 0x0db8)
                && segments[0] != 0x2002
                && !(segments[0] == 0x3fff && segments[1] < 0x1000)
        }
    }
}

fn public_ipv4(address: Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();
    !matches!(a, 0 | 10 | 127)
        && !(a == 100 && (64..=127).contains(&b))
        && !(a == 169 && b == 254)
        && !(a == 172 && (16..=31).contains(&b))
        && !(a == 192 && b == 0 && c == 0)
        && !(a == 192 && b == 0 && c == 2)
        && !(a == 192 && b == 88 && c == 99)
        && !(a == 192 && b == 168)
        && !(a == 198 && (b == 18 || b == 19))
        && !(a == 198 && b == 51 && c == 100)
        && !(a == 203 && b == 0 && c == 113)
        && a < 224
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use image::{Frame, RgbaImage, codecs::gif::GifEncoder};
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};

    #[derive(Default)]
    struct FixedResolver(HashMap<String, SocketAddr>);

    impl FixedResolver {
        fn route(mut self, host: &str, address: SocketAddr) -> Self {
            self.0.insert(host.to_owned(), address);
            self
        }
    }

    #[async_trait]
    impl MediaResolver for FixedResolver {
        async fn resolve(&self, url: &Url) -> Result<ResolvedTarget> {
            let host = url.host_str().context("test URL has no host")?.to_owned();
            let address = *self.0.get(&host).context("test host was not routed")?;
            Ok(ResolvedTarget {
                host,
                addresses: vec![address],
            })
        }
    }

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
        let origin_url = format!("http://origin.test:{}", origin.address().port());
        let destination_url = format!("http://destination.test:{}", destination.address().port());
        let resolver = FixedResolver::default()
            .route("origin.test", *origin.address())
            .route("destination.test", *destination.address());
        let bytes = png(3, 2);
        Mock::given(path("/start"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("Location", format!("{destination_url}/original.png"))
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
        let downloaded = download_with_resolver(
            public_media_url(&format!("{origin_url}/start")).unwrap(),
            &resolver,
        )
        .await
        .unwrap();
        let original = decode_original(downloaded).unwrap();
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
    async fn redirect_to_private_address_is_rejected_before_connecting() {
        let origin = MockServer::start().await;
        let origin_url = format!("http://origin.test:{}", origin.address().port());
        Mock::given(path("/start"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("Location", "http://127.0.0.1:9/private.png"),
            )
            .expect(1)
            .mount(&origin)
            .await;
        let resolver = FixedResolver::default().route("origin.test", *origin.address());
        let error = download_with_resolver(
            public_media_url(&format!("{origin_url}/start")).unwrap(),
            &resolver,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("public network address"));
    }

    #[tokio::test]
    async fn chunked_images_remain_bounded_without_a_content_length() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let byte = stream.read_u8().await.unwrap();
                request.push(byte);
                if request.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            let chunk = vec![0_u8; 64 * 1024];
            for _ in 0..(MAX_DOWNLOAD_BYTES / chunk.len() + 1) {
                if stream.write_all(b"10000\r\n").await.is_err()
                    || stream.write_all(&chunk).await.is_err()
                    || stream.write_all(b"\r\n").await.is_err()
                {
                    return;
                }
            }
            let _ = stream.write_all(b"0\r\n\r\n").await;
        });
        let resolver = FixedResolver::default().route("chunked.test", address);
        let url =
            public_media_url(&format!("http://chunked.test:{}/image.png", address.port())).unwrap();
        let result = download_with_resolver(url, &resolver).await;
        server.abort();
        let error = result.unwrap_err();
        assert!(error.to_string().contains("16 MiB"), "{error:#}");
    }

    #[test]
    fn private_and_special_use_network_addresses_are_rejected() {
        for address in [
            "0.0.0.0",
            "2130706433",
            "0x7f000001",
            "0177.0.0.1",
            "10.0.0.1",
            "100.64.0.1",
            "127.0.0.1",
            "169.254.169.254",
            "172.16.0.1",
            "192.0.0.1",
            "192.0.2.1",
            "192.88.99.1",
            "192.168.0.1",
            "198.18.0.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "240.0.0.1",
            "[::]",
            "[::1]",
            "[::ffff:127.0.0.1]",
            "[64:ff9b::1]",
            "[fc00::1]",
            "[fe80::1]",
            "[ff00::1]",
            "[2001:db8::1]",
            "[2002::1]",
            "[3fff::1]",
        ] {
            assert!(
                public_media_url(&format!("http://{address}/media")).is_err(),
                "accepted {address}"
            );
        }
        for address in ["8.8.8.8", "1.1.1.1", "[2606:4700:4700::1111]"] {
            assert!(
                public_media_url(&format!("https://{address}/media")).is_ok(),
                "rejected {address}"
            );
        }
    }

    #[test]
    fn resolved_hostnames_keep_only_public_addresses() {
        let private_only = resolved_public_target(
            "private.test".into(),
            [
                SocketAddr::from(([127, 0, 0, 1], 443)),
                SocketAddr::from(([169, 254, 169, 254], 443)),
            ],
        )
        .unwrap_err();
        assert!(
            private_only.to_string().contains("public network address"),
            "{private_only:#}"
        );

        let public = SocketAddr::from(([8, 8, 8, 8], 443));
        let target = resolved_public_target(
            "mixed.test".into(),
            [SocketAddr::from(([10, 0, 0, 1], 443)), public, public],
        )
        .unwrap();
        assert_eq!(target.addresses, vec![public]);
    }

    #[test]
    fn connected_peer_must_match_a_vetted_endpoint() {
        let public = SocketAddr::from(([8, 8, 8, 8], 443));
        assert!(same_endpoint(
            public,
            "[::ffff:8.8.8.8]:443".parse().unwrap()
        ));
        assert!(!same_endpoint(
            public,
            SocketAddr::from(([1, 1, 1, 1], 443))
        ));
        assert!(!same_endpoint(public, SocketAddr::from(([8, 8, 8, 8], 80))));
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
