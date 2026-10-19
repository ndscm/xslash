//! http(s) transport: the optional cache server and credential helper.
//!
//! This layer knows nothing about the on-disk cache layout or entry data model;
//! it just moves bytes between URLs and caller-provided paths, applying the
//! credential helper.
//!
//! http without https is allowed anywhere for convenience. One common case is
//! an on-prem cache server on a trusted network.
//!
//! It's the credential helper's responsibility to avoid credential leaks over a
//! cleartext hop: it sees the full request URL and can withhold credentials for
//! endpoints it doesn't trust.

use ::anyhow::Context;
use ::anyhow::Result;
use ::anyhow::bail;
use ::serde::Deserialize;
use ::std::collections::BTreeMap;
use ::std::fs::File;
use ::std::path::Path;
use ::std::path::PathBuf;
use ::std::process::Command;

/// Environment variable holding the optional cache server base URL.
pub const XSLASH_CACHE_SERVER: &str = "XSLASH_CACHE_SERVER";
/// Environment variable holding the optional credential helper path.
pub const XSLASH_CREDENTIAL_HELPER: &str = "XSLASH_CREDENTIAL_HELPER";

/// Follow at most this many redirects, re-consulting the credential helper at
/// each hop.
const MAX_REDIRECTS: usize = 10;

/// Resolve a redirect `location` against the `base` URL it came from.
fn resolve_redirect(base: &str, location: &str) -> Result<String> {
    // An absolute URL (one with its own scheme) is taken as-is. A scheme is a
    // leading `name:` before any `/`, so a `:` later in a path or query isn't
    // mistaken for one.
    let has_scheme = match location.find('/') {
        Some(slash) => location[..slash].contains(':'),
        None => location.contains(':'),
    };
    if has_scheme {
        return Ok(location.to_string());
    }

    let base: ureq::http::Uri = base
        .parse()
        .with_context(|| format!("parsing redirect base {base}"))?;
    let scheme = base.scheme_str().unwrap_or("https");

    // Protocol-relative: //host/path inherits only the scheme.
    if let Some(rest) = location.strip_prefix("//") {
        return Ok(format!("{scheme}://{rest}"));
    }

    let authority = base
        .authority()
        .map(|a| a.as_str())
        .context("redirect base has no host")?;

    // Absolute path: replace the whole path.
    if location.starts_with('/') {
        return Ok(format!("{scheme}://{authority}{location}"));
    }

    // Relative path: resolve against the base path's directory.
    let base_path = base.path();
    let dir_end = base_path.rfind('/').map(|i| i + 1).unwrap_or(0);
    Ok(format!(
        "{scheme}://{authority}{}{location}",
        &base_path[..dir_end]
    ))
}

/// The redirect target of `response` resolved against `base`, or `None` if it
/// isn't a redirect.
fn redirect(base: &str, response: &ureq::http::Response<ureq::Body>) -> Result<Option<String>> {
    if !response.status().is_redirection() {
        return Ok(None);
    }
    let Some(location) = response.headers().get(ureq::http::header::LOCATION) else {
        return Ok(None);
    };
    let location = location
        .to_str()
        .context("redirect Location is not valid text")?;
    Ok(Some(resolve_redirect(base, location)?))
}

/// An http(s) client wired to the configured cache server and credential
/// helper.
pub struct Http {
    /// Suppress xslash's messages.
    silent: bool,

    /// The ureq agent used for every request; follows no redirects itself.
    agent: ureq::Agent,

    /// Base URL of the optional cache server, trailing slash trimmed; `None`
    /// when [`XSLASH_CACHE_SERVER`] is unset.
    cache_server: Option<String>,

    /// Path to the optional credential helper; `None` when
    /// [`XSLASH_CREDENTIAL_HELPER`] is unset.
    credential_helper: Option<PathBuf>,
}

/// The JSON a credential helper prints on stdout (same shape as bazel's).
#[derive(Deserialize)]
struct Credentials {
    #[serde(default)]
    headers: BTreeMap<String, Vec<String>>,
}

impl Http {
    /// Build a client from the environment.
    pub fn from_env(silent: bool) -> Http {
        let cache_server = ::std::env::var(XSLASH_CACHE_SERVER)
            .ok()
            .filter(|s| !s.is_empty())
            .map(|s| s.trim_end_matches('/').to_string());
        let credential_helper = ::std::env::var_os(XSLASH_CREDENTIAL_HELPER)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        // max_redirects(0) hands 3xx back instead of chasing it, so `request`
        // follows redirects itself and re-runs the helper per hop.
        let agent = ::ureq::Agent::new_with_config(
            ::ureq::Agent::config_builder().max_redirects(0).build(),
        );
        Http {
            silent,
            agent,
            cache_server,
            credential_helper,
        }
    }

    /// Headers from the credential helper for `method url`; empty when no helper
    /// is configured or it prints nothing, an error (non-zero exit) skips the
    /// request.
    fn credentials(&self, method: &str, url: &str) -> Result<Vec<(String, String)>> {
        let Some(helper) = &self.credential_helper else {
            return Ok(Vec::new());
        };
        let output = Command::new(helper)
            .arg(method)
            .arg(url)
            .output()
            .with_context(|| format!("running credential helper {}", helper.display()))?;
        if !output.status.success() {
            bail!("credential helper exited non-zero for {method} {url}");
        }
        if output.stdout.iter().all(u8::is_ascii_whitespace) {
            return Ok(Vec::new());
        }
        let creds: Credentials =
            ::serde_json::from_slice(&output.stdout).context("parsing credential helper output")?;
        Ok(creds
            .headers
            .into_iter()
            .map(|(name, values)| (name, values.join(", ")))
            .collect())
    }

    /// Send `method url` (`body` is the PUT payload, `None` for GET), following
    /// redirects by hand so each hop's credentials come fresh from the helper and
    /// never ride along to the redirect target.
    fn request(
        &self,
        method: &str,
        url: &str,
        body: Option<&[u8]>,
    ) -> Result<::ureq::http::Response<::ureq::Body>> {
        let mut url = url.to_string();
        for _ in 0..=MAX_REDIRECTS {
            if !self.silent {
                eprintln!("xslash: {} {url}", method.to_ascii_lowercase());
            }
            let headers = self.credentials(method, &url)?;
            // get and put are distinct ureq builder types, so dispatch per hop.
            let response = match body {
                None => {
                    let mut req = self.agent.get(&url);
                    for (name, value) in &headers {
                        req = req.header(name, value);
                    }
                    req.call()
                }
                Some(bytes) => {
                    let mut req = self.agent.put(&url);
                    for (name, value) in &headers {
                        req = req.header(name, value);
                    }
                    req.send(bytes)
                }
            }
            .with_context(|| format!("{method} {url}"))?;

            match redirect(&url, &response)? {
                Some(next) => url = next,
                None => return Ok(response),
            }
        }
        bail!("too many redirects following {method} {url}")
    }

    /// GET `url`, streaming the body into `dest`. The transport isn't trusted for
    /// integrity - the caller verifies each download against its hash and digest.
    pub fn get(&self, url: &str, dest: &Path) -> Result<()> {
        let response = self.request("GET", url, None)?;
        let mut reader = response.into_body().into_reader();
        let mut file =
            File::create(dest).with_context(|| format!("creating {}", dest.display()))?;
        ::std::io::copy(&mut reader, &mut file)
            .with_context(|| format!("writing {}", dest.display()))?;
        Ok(())
    }

    /// PUT the contents of `file` to `url`.
    fn put(&self, url: &str, file: &Path) -> Result<()> {
        let bytes = ::std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
        self.request("PUT", url, Some(&bytes))?;
        Ok(())
    }

    /// GET the artifact `<hash>/<digest>` from the cache server into `dest`.
    /// Returns whether a cache server is configured and the GET succeeded. The
    /// result is *not* verified; the caller must check its digest.
    pub fn cache_server_get(&self, hash: &str, digest: &str, dest: &Path) -> bool {
        let Some(server) = &self.cache_server else {
            return false;
        };
        self.get(&format!("{server}/{hash}/{digest}"), dest).is_ok()
    }

    /// Best-effort PUT of `file` back to the cache server as `<hash>/<digest>`.
    /// A no-op when no cache server is configured; failures are ignored.
    pub fn cache_server_put(&self, hash: &str, digest: &str, file: &Path) {
        if let Some(server) = &self.cache_server {
            let _ = self.put(&format!("{server}/{hash}/{digest}"), file);
        }
    }
}
