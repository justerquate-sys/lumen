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

/// Largest body read (the client bundle is a few MB; an APK about 15).
const LIMIT: u64 = 96 * 1024 * 1024;

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

    pub fn post_json(&self, url: &str, body: &serde_json::Value) -> Result<Answer, String> {
        Self::read(
            self.agent
                .post(url)
                .header("Content-Type", "application/json")
                .send(body.to_string().as_bytes()),
        )
    }
}
