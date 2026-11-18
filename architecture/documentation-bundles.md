# Local documentation bundles

An optional data bundle lets the gateway serve a static documentation site from
memory. Documentation updates replace the bundle and restart the process; they do
not require recompiling the gateway or rebuilding its UI. The feature is independent
of the site's language, branding, and generator.

## Build and configure

1. Build your static site for `/ui/docs/`. If you use the YAML setting
   `ui.basePath: /gateway`, build the site for `/gateway/ui/docs/` instead.
   The site must use that prefix consistently for links, scripts, styles, images,
   and search indexes, or use relative URLs. The gateway does not rewrite content.
   With Hugo, for example: `hugo --baseURL /ui/docs/ --destination public-bundle`.
   Verify theme-specific navigation and search: some themes assume a root mount
   and need changes in addition to the base URL setting.
2. Package the generated directory using Python 3.9 or newer (standard library only):

   ```sh
   python3 tools/build-docs-bundle.py /path/to/public-bundle /tmp/docs.pack \
     --version 1.7.0 --entrypoint docs/standalone/latest/
   ```

   The `--version` CLI argument identifies the documentation release. The optional
   `--entrypoint` CLI argument selects the sidebar link's destination, relative to
   the site root; its default is the site's root `index.html`. Directory entrypoints
   must end with `/`. The output must be outside the input directory. The builder
   rejects symlinks and publishes the output with an atomic rename.
3. Install the bundle where the gateway process can read it:

   ```sh
   install -m 0644 /tmp/docs.pack /opt/agentgateway/docs.pack
   ```

4. Set the **YAML setting** `config.documentationBundle` and restart the gateway:

   ```yaml
   config:
     documentationBundle: /opt/agentgateway/docs.pack
   ```

   Relative file paths resolve against the process working directory. The bundle is
   loaded once at startup, with checks for the format, index, paths, file boundaries,
   stored-content checksums, gzip streams, and uncompressed lengths. A missing
   bundle logs a warning and disables local documentation. An invalid or unreadable
   existing bundle prevents startup. Configuration reload does not reload this static setting or its file.

The top-level **Docs** navigation item opens the local landing page in a new tab.
It appears in desktop and mobile navigation only when a bundle loaded successfully
at startup. If the YAML setting `config.documentationBundle` is unset or the file
is missing, the UI shows no documentation link. It stays hidden while the UI waits
for runtime information; there is no external website fallback.
Local documentation is served under `/ui/docs/` through the existing management
service, with the same configured UI gateway authentication and base path. The
admin listener retains its existing access rules; configure its bind address and
network access as you would for the rest of the management API.

## Update and deployment

Build a new bundle, install it atomically at the configured path, then restart the
gateway. Active processes retain the old in-memory copy until restart. For replicas,
distribute the same bundle and perform a rolling restart. There is no file watcher,
remote download, or dependency on a separate web server.

The bundle contains executable browser content (HTML and JavaScript), so install
only trusted documentation, just as with the UI assets. The checksums detect
corruption; they are not signatures. A bundle is portable across gateway platforms
that support format version 1. Site URLs still need to match the deployment prefix.
External site dependencies, such as CDN fonts, remain external; package those in the
site if fully offline operation is required.

The gateway retains compressed asset bytes plus the index in memory. Browsers that
accept gzip receive a shared slice of those bytes. For clients requesting identity
encoding, gzip assets are decompressed as a stream without retaining an expanded
copy of the site. GET, HEAD, conditional requests with ETags, MIME types, and directory
redirects are supported. Missing paths return 404, without the management UI's SPA
fallback. Responses use `Cache-Control: private, no-cache` so browsers revalidate
across documentation updates. Byte ranges are not implemented; a range request gets
the complete representation.

## Format version 1

A bundle is data, not a native library. It has no executable loader hooks or Rust ABI.
All offsets and lengths below count bytes.

| Section | Encoding |
| --- | --- |
| Magic | Eight bytes: `AGWDOCS1` |
| Manifest length | Unsigned 64-bit little-endian integer |
| Manifest | UTF-8 JSON of the stated length |
| Payload | Concatenated asset representations |

The manifest contains `version` (nonempty documentation release string), `entrypoint`
(relative file or directory path), and `assets` (ordered array). Each asset contains:

| Field | Meaning |
| --- | --- |
| `path` | Relative, decoded site path with `/` separators |
| `contentType` | HTTP MIME type |
| `encoding` | `gzip` or `identity` |
| `offset` | Offset relative to the start of the payload |
| `length` | Stored representation length |
| `originalLength` | Uncompressed file length |
| `sha256` | Lowercase hexadecimal digest of the stored representation |

Entries cover the payload exactly, in order, without gaps, duplicates, or overlapping
ranges. Absolute paths, empty interior segments, `.` and `..`, backslashes, control
characters, and `%`, `?`, `#` in file names are rejected. The builder sorts paths and
uses gzip with a zero timestamp, storing identity when gzip would be larger.
Unsupported format versions fail explicitly instead of being interpreted as version 1.

The tiny fixture under `crates/agentgateway/src/ui/testdata/` is produced by the same
builder. Production documentation artifacts should be distributed separately and
should not be committed to the gateway source repository.
