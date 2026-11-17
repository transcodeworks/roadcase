//! How bytes arrive.

use std::io::Write;
use url::Url;

/// What a transport reports when it fails.
pub type TransportError = Box<dyn std::error::Error + Send + Sync>;

/// Fetches a URL. The updater checks everything that arrives, so a transport
/// only has to be honest about failures.
pub trait Transport: Send + Sync {
    /// Writes the body of `url` to `sink`, calling `progress` with the bytes
    /// received so far and the total, when the server says it.
    ///
    /// # Errors
    ///
    /// When the server cannot be reached, answers with an error status, or
    /// `sink` refuses the bytes.
    fn get(
        &self,
        url: &Url,
        sink: &mut dyn Write,
        progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<(), TransportError>;
}

/// Whether updates may come from `url`: HTTPS, or this computer for tests.
pub(crate) fn allowed(url: &Url) -> bool {
    match url.scheme() {
        "https" => true,
        "http" => matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")),
        _ => false,
    }
}

/// HTTPS, with reqwest and rustls.
#[cfg(feature = "http")]
#[derive(Debug, Clone)]
pub struct Http(reqwest::blocking::Client);

#[cfg(feature = "http")]
impl Http {
    /// A client that names `agent` to the server, such as `Example/0.1.0`.
    ///
    /// # Errors
    ///
    /// When the TLS stack does not start.
    pub fn new(agent: &str) -> Result<Self, TransportError> {
        let redirects = reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() > 5 {
                attempt.error("too many redirects")
            } else if allowed(attempt.url()) {
                attempt.follow()
            } else {
                attempt.error("redirected away from HTTPS")
            }
        });
        let client = reqwest::blocking::Client::builder()
            .user_agent(agent)
            .redirect(redirects)
            .connect_timeout(std::time::Duration::from_secs(30))
            // A download takes as long as it takes.
            .timeout(None)
            .build()?;
        Ok(Self(client))
    }
}

#[cfg(feature = "http")]
impl Transport for Http {
    fn get(
        &self,
        url: &Url,
        sink: &mut dyn Write,
        progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<(), TransportError> {
        use std::io::Read;
        let mut response = self.0.get(url.clone()).send()?.error_for_status()?;
        let total = response.content_length();
        let mut received = 0u64;
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let read = response.read(&mut buffer)?;
            if read == 0 {
                return Ok(());
            }
            sink.write_all(&buffer[..read])?;
            received += read as u64;
            progress(received, total);
        }
    }
}
