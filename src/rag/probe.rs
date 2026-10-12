//! Connectivity probes for the RAG backends (feature `rag`).
//!
//! The agent bar can only show honest Ready/Error states if it knows whether
//! the configured backends answer. This module performs that check with the
//! same endpoints the chat/embedding stacks use:
//!
//! - **Ollama** — `GET {base}/api/tags`. The base URL honours rig's
//!   `OLLAMA_API_BASE_URL` environment variable (default
//!   `http://localhost:11434`), so the probe hits exactly the server ragrig
//!   would talk to.
//! - **DeepSeek** — `GET https://api.deepseek.com/models` with a bearer
//!   token (the endpoint rig's own client tests use). `401`/`403` mean the
//!   key was rejected; any other HTTP answer means the API is reachable.
//!
//! Probes are blocking calls (the editor's `ureq` client) with short
//! timeouts; the worker runs them on a blocking thread. The classification
//! is a pure function of the outcome, which keeps the interesting cases
//! (`401` vs. timeout vs. `200`) unit-testable without a network.

use std::time::Duration;

use super::status::Reachability;

/// Default Ollama base URL (mirrors rig's `OLLAMA_API_BASE_URL` default).
pub const DEFAULT_OLLAMA_BASE_URL: &str = "http://localhost:11434";

/// DeepSeek API base URL (mirrors rig's `DEEPSEEK_API_BASE_URL`).
pub const DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com";

/// Timeout for one probe request. Ollama is local (fast), DeepSeek is not;
/// the value covers both without stalling the worker noticeably.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// The Ollama base URL the stacks connect to (`OLLAMA_API_BASE_URL`, as rig
/// reads it), falling back to [`DEFAULT_OLLAMA_BASE_URL`].
pub fn ollama_base_url() -> String {
    std::env::var("OLLAMA_API_BASE_URL")
        .ok()
        .map(|url| url.trim().trim_end_matches('/').to_string())
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| DEFAULT_OLLAMA_BASE_URL.to_string())
}

/// Probe the Ollama server behind `base_url`.
pub fn probe_ollama(base_url: &str) -> Reachability {
    let url = format!("{}/api/tags", base_url.trim_end_matches('/'));
    classify_ollama(http_get(&url, None))
}

/// Probe the DeepSeek API with `api_key`.
pub fn probe_deepseek(api_key: &str) -> Reachability {
    let url = format!("{DEEPSEEK_BASE_URL}/models");
    classify_deepseek(http_get(&url, Some(api_key)))
}

/// Raw outcome of one HTTP GET, split out so classification is pure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HttpOutcome {
    /// The server answered with a success status.
    Success,
    /// The server answered with this status code.
    Status(u16),
    /// No HTTP answer at all (connection refused, timeout, TLS, DNS).
    Transport,
}

fn http_get(url: &str, bearer: Option<&str>) -> HttpOutcome {
    let agent = ureq::AgentBuilder::new().timeout(PROBE_TIMEOUT).build();
    let mut request = agent.get(url);
    if let Some(token) = bearer {
        request = request.set("Authorization", &format!("Bearer {token}"));
    }
    match request.call() {
        Ok(_) => HttpOutcome::Success,
        // Any HTTP answer proves the endpoint is reachable.
        Err(ureq::Error::Status(code, _)) => HttpOutcome::Status(code),
        Err(ureq::Error::Transport(_)) => HttpOutcome::Transport,
    }
}

/// An HTTP answer from Ollama — even an error status — proves it is up.
fn classify_ollama(outcome: HttpOutcome) -> Reachability {
    match outcome {
        HttpOutcome::Success | HttpOutcome::Status(_) => Reachability::Reachable,
        HttpOutcome::Transport => Reachability::Unreachable,
    }
}

/// DeepSeek rejects bad credentials with `401`/`403`; everything else that
/// answers (including `429`, `5xx`) means the API itself is reachable.
fn classify_deepseek(outcome: HttpOutcome) -> Reachability {
    match outcome {
        HttpOutcome::Success => Reachability::Reachable,
        HttpOutcome::Status(401 | 403) => Reachability::Unreachable,
        HttpOutcome::Status(_) => Reachability::Reachable,
        HttpOutcome::Transport => Reachability::Unreachable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ollama_serves_reachability_on_any_http_answer() {
        assert_eq!(
            classify_ollama(HttpOutcome::Success),
            Reachability::Reachable
        );
        assert_eq!(
            classify_ollama(HttpOutcome::Status(404)),
            Reachability::Reachable
        );
        assert_eq!(
            classify_ollama(HttpOutcome::Transport),
            Reachability::Unreachable
        );
    }

    #[test]
    fn deepseek_distinguishes_rejection_from_transport() {
        assert_eq!(
            classify_deepseek(HttpOutcome::Success),
            Reachability::Reachable
        );
        assert_eq!(
            classify_deepseek(HttpOutcome::Status(401)),
            Reachability::Unreachable,
            "a rejected key is a real failure, not a reachability miss"
        );
        assert_eq!(
            classify_deepseek(HttpOutcome::Status(403)),
            Reachability::Unreachable
        );
        assert_eq!(
            classify_deepseek(HttpOutcome::Status(429)),
            Reachability::Reachable,
            "throttling still proves the API is there"
        );
        assert_eq!(
            classify_deepseek(HttpOutcome::Transport),
            Reachability::Unreachable
        );
    }

    #[test]
    fn ollama_base_url_reads_the_rig_variable() {
        // The environment is shared between tests; only assert the default
        // when the variable is unset (CI usually leaves it unset).
        if std::env::var("OLLAMA_API_BASE_URL").is_err() {
            assert_eq!(ollama_base_url(), DEFAULT_OLLAMA_BASE_URL);
        }
    }

    #[test]
    fn refused_connection_is_unreachable() {
        // Port 1 on localhost refuses immediately on every mainstream OS.
        assert_eq!(
            probe_ollama("http://127.0.0.1:1"),
            Reachability::Unreachable
        );
    }

    /// Serve exactly one HTTP 200 answer, then stop.
    fn one_shot_server() -> (String, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                use std::io::{Read, Write};
                let mut buffer = [0u8; 1024];
                let _ = stream.read(&mut buffer);
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
                );
            }
        });
        (format!("http://{addr}"), handle)
    }

    #[test]
    fn a_listening_server_is_reachable() {
        let (base, handle) = one_shot_server();
        assert_eq!(probe_ollama(&base), Reachability::Reachable);
        let _ = handle.join();
    }

    #[test]
    fn a_rejecting_endpoint_is_unreachable_for_deepseek() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                use std::io::{Read, Write};
                let mut buffer = [0u8; 1024];
                let _ = stream.read(&mut buffer);
                let _ = stream.write_all(
                    b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
        });
        let outcome = http_get(&format!("http://{addr}/models"), Some("bad-key"));
        assert_eq!(classify_deepseek(outcome), Reachability::Unreachable);
        let _ = handle.join();
    }
}
