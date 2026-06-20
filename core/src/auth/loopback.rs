use std::time::{Duration, Instant};

use tiny_http::{Header, Request, Response, Server};

use crate::auth::oauth::{constant_time_eq, parse_redirect, RedirectParams};
use crate::error::AuthError;

/// Cap on the request-target length we'll parse; a real OAuth callback is well
/// under this, so anything larger is junk traffic we ignore.
const MAX_TARGET_LEN: usize = 8192;

const SUCCESS_HTML: &str = "<html><body style=\"font-family:sans-serif\">\
    <h3>Signed in to Outlook Tasks</h3>\
    <p>You can close this tab and return to the panel.</p></body></html>";
const NEUTRAL_HTML: &str = "<html><body style=\"font-family:sans-serif\">\
    <p>Waiting for sign-in...</p></body></html>";

/// A loopback HTTP listener on `127.0.0.1:<ephemeral>` that captures the OAuth
/// redirect. Advertises its redirect as `http://localhost:<port>/` (Entra ignores
/// the loopback port). Note: this binds IPv4 only; on the rare host whose resolver
/// returns `::1` for `localhost` first, register/use `http://127.0.0.1` instead
/// (see README) - that is the documented fallback.
pub struct LoopbackServer {
    server: Server,
    port: u16,
}

impl LoopbackServer {
    pub fn bind() -> Result<Self, AuthError> {
        let server = Server::http("127.0.0.1:0").map_err(|e| AuthError::Protocol(e.to_string()))?;
        let port = server
            .server_addr()
            .to_ip()
            .ok_or_else(|| AuthError::Protocol("loopback bound to non-ip address".into()))?
            .port();
        Ok(Self { server, port })
    }

    pub fn redirect_url(&self) -> String {
        format!("http://localhost:{}/", self.port)
    }

    /// Waits up to `timeout` for a redirect carrying a `code` whose `state`
    /// matches `expected_state`. Ignores unrelated local requests (replying with
    /// a neutral page) so another local process can't end sign-in early or force
    /// a forged code. Returns a timeout error if no valid redirect arrives.
    pub async fn wait_for_code(
        self,
        expected_state: String,
        timeout: Duration,
    ) -> Result<RedirectParams, AuthError> {
        tokio::task::spawn_blocking(move || {
            let deadline = Instant::now() + timeout;
            loop {
                let remaining = deadline
                    .checked_duration_since(Instant::now())
                    .ok_or_else(|| AuthError::Protocol("sign-in timed out".into()))?;
                let request = match self
                    .server
                    .recv_timeout(remaining)
                    .map_err(|e| AuthError::Protocol(e.to_string()))?
                {
                    Some(r) => r,
                    None => return Err(AuthError::Protocol("sign-in timed out".into())),
                };
                // Only a GET to the exact redirect path can be our callback; other
                // methods/paths/oversized targets are unrelated local traffic.
                if !is_callback_request(&request) {
                    respond(request, NEUTRAL_HTML);
                    continue;
                }
                match parse_redirect(request.url()) {
                    // Constant-time state check: don't leak the expected state via
                    // comparison timing to a local process probing the port.
                    Ok(params) if constant_time_eq(&params.state, &expected_state) => {
                        respond(request, SUCCESS_HTML);
                        return Ok(params);
                    }
                    // Unrelated request, missing code, or wrong/forged state:
                    // never signal success; keep waiting until valid or timeout.
                    _ => respond(request, NEUTRAL_HTML),
                }
            }
        })
        .await
        .map_err(|e| AuthError::Protocol(e.to_string()))?
    }
}

/// True only for a GET to the exact redirect path (`/...`) within the length
/// cap - the shape a browser's OAuth callback takes.
fn is_callback_request(request: &Request) -> bool {
    if !matches!(request.method(), tiny_http::Method::Get) {
        return false;
    }
    let target = request.url();
    if target.len() > MAX_TARGET_LEN {
        return false;
    }
    let path = target.split('?').next().unwrap_or(target);
    path == "/"
}

fn respond(request: Request, html: &str) {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..])
        .expect("valid header");
    let _ = request.respond(Response::from_string(html).with_header(header));
}
