//! Data-only documentation bundles. See architecture/documentation-bundles.md for the format.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::Path;

use anyhow::{Context, ensure};
use async_compression::tokio::bufread::GzipDecoder;
use axum::body::Body;
use axum::response::{IntoResponse, Redirect, Response};
use bytes::Bytes;
use http::{HeaderMap, HeaderValue, Method, Request, StatusCode, header};
use serde::Deserialize;
use tokio::io::AsyncReadExt;
use tokio_util::io::ReaderStream;

const MAGIC: &[u8; 8] = b"AGWDOCS1";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
	version: String,
	entrypoint: String,
	assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Asset {
	path: String,
	content_type: String,
	encoding: Encoding,
	offset: usize,
	length: usize,
	original_length: u64,
	sha256: String,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Encoding {
	Identity,
	Gzip,
}

#[derive(Debug)]
pub(crate) struct Bundle {
	version: String,
	data: Bytes,
	assets: HashMap<String, Asset>,
	entrypoint: String,
}

impl Bundle {
	pub(crate) async fn load_if_present(path: &Path) -> anyhow::Result<Option<Self>> {
		let data = match fs_err::tokio::read(path).await {
			Ok(data) => data,
			Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
				tracing::warn!(path = %path.display(), "documentation bundle not found; Docs navigation is disabled");
				return Ok(None);
			},
			Err(error) => return Err(error.into()),
		};
		let bundle = Self::parse(Bytes::from(data))
			.with_context(|| format!("invalid documentation bundle {}", path.display()))?;
		// Verify gzip streams and declared lengths without retaining decompressed files.
		for asset in bundle
			.assets
			.values()
			.filter(|asset| asset.encoding == Encoding::Gzip)
		{
			let contents = bundle.data.slice(asset.offset..asset.offset + asset.length);
			let limit = asset
				.original_length
				.checked_add(1)
				.context("original length overflow")?;
			let mut reader = GzipDecoder::new(Cursor::new(contents)).take(limit);
			let length = tokio::io::copy(&mut reader, &mut tokio::io::sink())
				.await
				.with_context(|| format!("invalid compressed documentation asset {}", asset.path))?;
			ensure!(
				length == asset.original_length,
				"incorrect original length: {}",
				asset.path
			);
		}
		tracing::info!(
			version = bundle.version,
			files = bundle.assets.len(),
			bytes = bundle.data.len(),
			"loaded documentation bundle"
		);
		Ok(Some(bundle))
	}

	fn parse(data: Bytes) -> anyhow::Result<Self> {
		ensure!(
			data.get(..8) == Some(MAGIC),
			"unsupported documentation bundle format"
		);
		let length = data.get(8..16).context("truncated bundle header")?;
		let length = usize::try_from(u64::from_le_bytes(length.try_into()?))?;
		let start = 16usize
			.checked_add(length)
			.context("manifest length overflow")?;
		let manifest: Manifest =
			serde_json::from_slice(data.get(16..start).context("truncated bundle manifest")?)?;
		ensure!(
			!manifest.version.is_empty(),
			"missing documentation version"
		);
		ensure!(valid_path(&manifest.entrypoint, true), "invalid entrypoint");
		let data = data.slice(start..);
		let mut assets = HashMap::new();
		let mut end = 0;
		for asset in manifest.assets {
			ensure!(
				valid_path(&asset.path, false),
				"invalid asset path: {}",
				asset.path
			);
			ensure!(
				asset.offset == end,
				"asset data must be contiguous and ordered"
			);
			end = asset
				.offset
				.checked_add(asset.length)
				.context("asset length overflow")?;
			let contents = data.get(asset.offset..end).context("truncated asset")?;
			ensure!(
				hex::encode(crate::crypto::digest::sha256(contents)) == asset.sha256,
				"checksum mismatch: {}",
				asset.path
			);
			ensure!(
				asset.encoding != Encoding::Identity || asset.original_length == asset.length as u64,
				"incorrect identity length"
			);
			HeaderValue::from_str(&asset.content_type).context("invalid content type header")?;
			mediatype::MediaType::parse(&asset.content_type).context("invalid content type")?;
			ensure!(
				assets.insert(asset.path.clone(), asset).is_none(),
				"duplicate asset path"
			);
		}
		ensure!(end == data.len(), "unindexed bundle data");
		let entry = index_path(&manifest.entrypoint);
		ensure!(
			assets.contains_key(&entry),
			"entrypoint is missing: {entry}"
		);
		Ok(Self {
			version: manifest.version,
			data,
			assets,
			entrypoint: manifest.entrypoint,
		})
	}

	/// Relative to the API base, which can differ from the UI origin in development.
	pub(crate) fn entrypoint(&self) -> String {
		let path = self
			.entrypoint
			.split('/')
			.map(|part| {
				percent_encoding::utf8_percent_encode(part, percent_encoding::NON_ALPHANUMERIC).to_string()
			})
			.collect::<Vec<_>>()
			.join("/");
		format!("/ui/docs/{path}")
	}

	/// Called by the UI service after `/ui` has been stripped from the request path.
	pub(crate) fn serve(&self, req: Request<Body>) -> Response {
		if req.method() != Method::GET && req.method() != Method::HEAD {
			return (
				StatusCode::METHOD_NOT_ALLOWED,
				[(header::ALLOW, "GET, HEAD")],
			)
				.into_response();
		}
		let raw_path = req.uri().path();
		let path = raw_path.strip_prefix("/docs/").unwrap_or("");
		let Ok(path) = percent_encoding::percent_decode_str(path).decode_utf8() else {
			return StatusCode::BAD_REQUEST.into_response();
		};
		if !valid_path(&path, true) {
			return StatusCode::BAD_REQUEST.into_response();
		}
		let candidate = index_path(&path);
		let asset = if let Some(asset) = self.assets.get(&candidate) {
			asset
		} else if !path.ends_with('/') && self.assets.contains_key(&format!("{path}/index.html")) {
			return directory_redirect(&req);
		} else {
			return StatusCode::NOT_FOUND.into_response();
		};
		if raw_path == "/docs" {
			return directory_redirect(&req);
		}
		let (gzip_q, identity_q) = encoding_quality(req.headers());
		let gzip = asset.encoding == Encoding::Gzip && gzip_q > 0.0 && gzip_q >= identity_q;
		if !gzip && identity_q == 0.0 {
			return (
				StatusCode::NOT_ACCEPTABLE,
				[(header::VARY, "Accept-Encoding")],
			)
				.into_response();
		}
		let etag = format!(
			"\"{}-{}\"",
			asset.sha256,
			if gzip { "gzip" } else { "identity" }
		);
		let unchanged = req
			.headers()
			.get_all(header::IF_NONE_MATCH)
			.iter()
			.filter_map(|v| v.to_str().ok())
			.flat_map(|v| v.split(','))
			.any(|v| v.trim() == "*" || v.trim().trim_start_matches("W/") == etag);
		let body = if unchanged || req.method() == Method::HEAD {
			Body::empty()
		} else {
			let contents = self.data.slice(asset.offset..asset.offset + asset.length);
			if asset.encoding == Encoding::Gzip && !gzip {
				Body::from_stream(ReaderStream::new(GzipDecoder::new(Cursor::new(contents))))
			} else {
				Body::from(contents)
			}
		};
		let mut response = body.into_response();
		if unchanged {
			*response.status_mut() = StatusCode::NOT_MODIFIED;
		} else {
			let length = if gzip {
				asset.length as u64
			} else {
				asset.original_length
			};
			response
				.headers_mut()
				.insert(header::CONTENT_LENGTH, length.into());
		}
		let headers = response.headers_mut();
		headers.insert(
			header::CONTENT_TYPE,
			HeaderValue::from_str(&asset.content_type).expect("validated on load"),
		);
		headers.insert(
			header::ETAG,
			HeaderValue::from_str(&etag).expect("hex digest"),
		);
		headers.insert(
			header::CACHE_CONTROL,
			HeaderValue::from_static("private, no-cache"),
		);
		headers.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
		headers.insert(
			header::X_CONTENT_TYPE_OPTIONS,
			HeaderValue::from_static("nosniff"),
		);
		if gzip {
			headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
		}
		response
	}
}

fn valid_path(path: &str, directory: bool) -> bool {
	let path = if directory {
		path.strip_suffix('/').unwrap_or(path)
	} else {
		path
	};
	if path.is_empty() {
		return directory;
	}
	!path.starts_with('/')
		&& !path.contains(['\\', '?', '#', '%'])
		&& !path.chars().any(char::is_control)
		&& path.split('/').all(|part| !matches!(part, "" | "." | ".."))
}

fn index_path(path: &str) -> String {
	if path.is_empty() || path.ends_with('/') {
		format!("{path}index.html")
	} else {
		path.to_string()
	}
}

fn directory_redirect(req: &Request<Body>) -> Response {
	let query = req
		.uri()
		.query()
		.map(|q| format!("?{q}"))
		.unwrap_or_default();
	Redirect::permanent(&format!(
		"{}/ui{}/{query}",
		super::ui_base_path(req.extensions().get()),
		req.uri().path()
	))
	.into_response()
}

/// An explicit zero overrides `*`. Identity is acceptable unless explicitly excluded.
fn encoding_quality(headers: &HeaderMap) -> (f32, f32) {
	let (mut gzip, mut identity, mut wildcard) = (None, None, None);
	for value in headers
		.get_all(header::ACCEPT_ENCODING)
		.iter()
		.filter_map(|v| v.to_str().ok())
	{
		for item in value.split(',') {
			let mut parts = item.split(';');
			let token = parts.next().unwrap_or_default().trim();
			let mut q = 1.0;
			for parameter in parts {
				if let Some((key, value)) = parameter.trim().split_once('=')
					&& key.trim().eq_ignore_ascii_case("q")
				{
					q = value
						.trim()
						.parse::<f32>()
						.ok()
						.filter(|v| (0.0..=1.0).contains(v))
						.unwrap_or(0.0);
				}
			}
			if token.eq_ignore_ascii_case("gzip") {
				gzip = Some(q);
			} else if token.eq_ignore_ascii_case("identity") {
				identity = Some(q);
			} else if token == "*" {
				wildcard = Some(q);
			}
		}
	}
	(
		gzip.or(wildcard).unwrap_or(0.0),
		identity.unwrap_or(if wildcard == Some(0.0) { 0.0 } else { 1.0 }),
	)
}

#[cfg(test)]
mod tests {
	use super::*;

	const FIXTURE: &[u8] = include_bytes!("testdata/docs.pack");

	pub(super) fn fixture() -> Bundle {
		Bundle::parse(Bytes::from_static(FIXTURE)).unwrap()
	}

	fn request(path: &str, method: &str, encoding: Option<&str>) -> Request<Body> {
		let mut builder = Request::builder().uri(path).method(method);
		if let Some(value) = encoding {
			builder = builder.header(header::ACCEPT_ENCODING, value);
		}
		builder.body(Body::empty()).unwrap()
	}

	#[tokio::test]
	async fn serves_compressed_and_identity_representations() {
		let bundle = fixture();
		for (encoding, compressed) in [
			(None, false),
			(Some("gzip"), true),
			(Some("br, gzip;q=1"), true),
			(Some("gzip;q=0, *;q=1"), false),
			(Some("gzip;q=0.5, identity;q=1"), false),
			(Some("*;q=1"), true),
			(Some("gzip;q=invalid"), false),
		] {
			let response = bundle.serve(request("/docs/", "GET", encoding));
			assert_eq!(response.status(), StatusCode::OK);
			assert_eq!(
				response.headers().contains_key(header::CONTENT_ENCODING),
				compressed
			);
			assert_eq!(
				response.headers()[header::CONTENT_TYPE],
				"text/html; charset=utf-8"
			);
			assert_eq!(response.headers()[header::VARY], "Accept-Encoding");
			let length = response.headers()[header::CONTENT_LENGTH]
				.to_str()
				.unwrap()
				.parse::<usize>()
				.unwrap();
			let body = axum::body::to_bytes(response.into_body(), usize::MAX)
				.await
				.unwrap();
			assert_eq!(body.len(), length);
			if compressed {
				assert_eq!(&body[..2], &[0x1f, 0x8b]);
			} else {
				assert_eq!(&body[..], include_bytes!("testdata/site/index.html"));
			}
		}
		// Already-small files are stored without compression.
		let response = bundle.serve(request("/docs/style.css", "GET", Some("gzip")));
		assert!(!response.headers().contains_key(header::CONTENT_ENCODING));
		assert_eq!(
			axum::body::to_bytes(response.into_body(), usize::MAX)
				.await
				.unwrap(),
			include_bytes!("testdata/site/style.css").as_slice()
		);
	}

	#[tokio::test]
	async fn head_and_revalidation_do_not_send_content() {
		let bundle = fixture();
		let get = bundle.serve(request("/docs/", "GET", Some("gzip")));
		let head = bundle.serve(request("/docs/", "HEAD", Some("gzip")));
		assert_eq!(get.headers(), head.headers());
		assert!(
			axum::body::to_bytes(head.into_body(), usize::MAX)
				.await
				.unwrap()
				.is_empty()
		);
		let etag = get.headers()[header::ETAG].to_str().unwrap();
		for validator in [
			etag.to_string(),
			format!("\"old\", W/{etag}"),
			"*".to_string(),
		] {
			let mut req = request("/docs/", "GET", Some("gzip"));
			req
				.headers_mut()
				.insert(header::IF_NONE_MATCH, validator.parse().unwrap());
			let response = bundle.serve(req);
			assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
			assert!(!response.headers().contains_key(header::CONTENT_LENGTH));
			assert!(
				axum::body::to_bytes(response.into_body(), usize::MAX)
					.await
					.unwrap()
					.is_empty()
			);
		}
		let identity = bundle.serve(request("/docs/", "GET", None));
		assert_ne!(
			identity.headers()[header::ETAG],
			get.headers()[header::ETAG]
		);
	}

	#[test]
	fn routes_directories_and_rejects_invalid_requests() {
		let bundle = fixture();
		for base in ["", "/team/gateway"] {
			for path in ["/docs", "/docs/guide"] {
				let mut req = request(&format!("{path}?language=en"), "GET", None);
				req
					.extensions_mut()
					.insert(super::super::UiBasePath(base.into()));
				let response = bundle.serve(req);
				assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
				assert_eq!(
					response.headers()[header::LOCATION],
					format!("{base}/ui{path}/?language=en")
				);
			}
		}
		for path in [
			"/docs/../config",
			"/docs/%2e%2e/config",
			"/docs/a%5cb",
			"/docs/%FF",
			"/docs/%00",
			"/docs//guide",
			"/docs/a/./b",
		] {
			assert_eq!(
				bundle.serve(request(path, "GET", None)).status(),
				StatusCode::BAD_REQUEST,
				"{path}"
			);
		}
		assert_eq!(
			bundle.serve(request("/docs/missing", "GET", None)).status(),
			StatusCode::NOT_FOUND
		);
		assert_eq!(
			bundle.serve(request("/docs/", "POST", None)).status(),
			StatusCode::METHOD_NOT_ALLOWED
		);
		for path in ["/docs/", "/docs/style.css"] {
			assert_eq!(
				bundle
					.serve(request(path, "GET", Some("identity;q=0, gzip;q=0")))
					.status(),
				StatusCode::NOT_ACCEPTABLE
			);
		}
	}

	fn mutate_manifest(change: impl FnOnce(&mut serde_json::Value)) -> Bytes {
		let length = u64::from_le_bytes(FIXTURE[8..16].try_into().unwrap()) as usize;
		let mut manifest: serde_json::Value =
			serde_json::from_slice(&FIXTURE[16..16 + length]).unwrap();
		change(&mut manifest);
		let manifest = serde_json::to_vec(&manifest).unwrap();
		let mut result = MAGIC.to_vec();
		result.extend_from_slice(&(manifest.len() as u64).to_le_bytes());
		result.extend(manifest);
		result.extend_from_slice(&FIXTURE[16 + length..]);
		result.into()
	}

	#[test]
	fn rejects_corrupt_or_unsupported_bundles() {
		for length in [0, 7, 8, 15, 16, FIXTURE.len() - 1] {
			assert!(Bundle::parse(Bytes::copy_from_slice(&FIXTURE[..length])).is_err());
		}
		let mut corrupt = FIXTURE.to_vec();
		*corrupt.last_mut().unwrap() ^= 1;
		assert!(Bundle::parse(corrupt.into()).is_err());
		for (field, value) in [
			("path", serde_json::json!("../index.html")),
			("offset", serde_json::json!(u64::MAX)),
			("length", serde_json::json!(u64::MAX)),
			("encoding", serde_json::json!("brotli")),
			(
				"contentType",
				serde_json::json!("text/html\r\nx-header: value"),
			),
		] {
			assert!(
				Bundle::parse(mutate_manifest(|m| m["assets"][0][field] = value)).is_err(),
				"{field}"
			);
		}
		assert!(Bundle::parse(mutate_manifest(|m| m["entrypoint"] = "missing/".into())).is_err());
		assert!(
			Bundle::parse(mutate_manifest(
				|m| m["assets"][1]["path"] = m["assets"][0]["path"].clone()
			))
			.is_err()
		);
	}

	#[tokio::test]
	async fn loaded_bundle_survives_file_removal_and_missing_bundle_disables_docs() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("docs.pack");
		fs_err::write(&path, FIXTURE).unwrap();
		let bundle = Bundle::load_if_present(&path).await.unwrap().unwrap();
		fs_err::write(
			&path,
			mutate_manifest(|m| m["assets"][0]["originalLength"] = 1.into()),
		)
		.unwrap();
		assert!(Bundle::load_if_present(&path).await.is_err());
		fs_err::remove_file(&path).unwrap();
		assert!(Bundle::load_if_present(&path).await.unwrap().is_none());
		let response = bundle.serve(request("/docs/", "GET", None));
		assert_eq!(
			axum::body::to_bytes(response.into_body(), usize::MAX)
				.await
				.unwrap(),
			include_bytes!("testdata/site/index.html").as_slice()
		);
	}
}
