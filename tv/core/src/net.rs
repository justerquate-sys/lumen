//! One HTTP agent for the shell: rustls with bundled web roots, a global timeout per request, and
//! HTTP error statuses returned as values (the OAuth endpoints answer 400/428 with JSON bodies).

use std::time::Duration;

pub struct Http {
    agent: ureq::Agent,
}

/// An HTTP answer: status and body.
pub struct Answer {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Answer {
    pub fn json(&self) -> Option<serde_json::Value> {
        serde_json::from_slice(&self.body).ok()
    }
}

/// Largest body read (the client bundle is a few MB; a release APK about 18).
const LIMIT: u64 = 96 * 1024 * 1024;
/// Largest download with progress (APKs: debug builds of the app run to a few hundred MB).
const BIG: u64 = 1024 * 1024 * 1024;

impl Http {
    pub fn new(timeout: Duration) -> Http {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .user_agent("Lumen-TV/1 (+https://github.com)")
            .build()
            .into();
        Http { agent }
    }

    fn read(r: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<Answer, String> {
        let mut r = r.map_err(|e| e.to_string())?;
        let status = r.status().as_u16();
        let body = r.body_mut().with_config().limit(LIMIT).read_to_vec().map_err(|e| e.to_string())?;
        Ok(Answer { status, body })
    }

    pub fn get(&self, url: &str, headers: &[(&str, &str)]) -> Result<Answer, String> {
        let mut req = self.agent.get(url);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        Self::read(req.call())
    }

    /// A GET whose body is read in pieces, `on(got, total)` after each (total 0 = unknown).
    pub fn get_progress(&self, url: &str, on: &mut dyn FnMut(u64, u64)) -> Result<Answer, String> {
        use std::io::Read;
        let mut r = self.agent.get(url).call().map_err(|e| e.to_string())?;
        let status = r.status().as_u16();
        let total = r.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        let mut body = Vec::with_capacity(total.min(BIG) as usize);
        let mut rd = r.body_mut().with_config().limit(BIG).reader();
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            let n = rd.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&buf[..n]);
            on(body.len() as u64, total);
        }
        Ok(Answer { status, body })
    }

    /// A POST of `body` as `content_type` (telemetry reports).
    pub fn post_body(&self, url: &str, content_type: &str, body: &[u8]) -> Result<Answer, String> {
        Self::read(self.agent.post(url).header("Content-Type", content_type).send(body))
    }

    pub fn post_json(&self, url: &str, body: &serde_json::Value) -> Result<Answer, String> {
        Self::read(
            self.agent
                .post(url)
                .header("Content-Type", "application/json")
                .send(body.to_string().as_bytes()),
        )
    }
}
