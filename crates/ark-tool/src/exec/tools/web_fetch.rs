//! Builtin `web_fetch` tool: fetch one public web page through the governed
//! egress (requirements/web-fetch-tool.md).
//!
//! The tool owns no HTTP client — it only calls the injected
//! [`HttpCapability`] under the `ark.builtin.web` egress identity, so timeouts,
//! size caps, per-hop redirect checks, redacted logs and denial diagnostics all
//! stay in `ark-http`. Its own responsibilities are the SSRF guard (reject
//! targets resolving to non-global addresses) and turning the response body
//! into model-readable text.

use std::net::IpAddr;
use std::sync::Arc;

use ark_common::Result;
use ark_ports::{
    HttpCapability, OutboundRequest, Tool, ToolArgs, ToolConcurrency, ToolContext, ToolOutput,
};
use url::Url;

use crate::domain::{args, schema};
use crate::policy::truncate::{cap_bytes, DEFAULT_LIMITS};

/// Egress identity of the runtime's own fetch capability.
const EGRESS_ID: &str = "ark.builtin.web";

pub struct WebFetchTool {
    http: Arc<dyn HttpCapability>,
}

impl WebFetchTool {
    pub fn new(http: Arc<dyn HttpCapability>) -> Self {
        Self { http }
    }
}

impl std::fmt::Debug for WebFetchTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebFetchTool").finish_non_exhaustive()
    }
}

#[async_trait::async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &'static str {
        "web_fetch"
    }

    fn schema(&self) -> &'static serde_json::Value {
        schema::web_fetch()
    }
    /// A fetch mutates nothing in the workspace; the egress bus rate-limits
    /// and traces each request independently.
    fn concurrency(&self) -> ToolConcurrency {
        ToolConcurrency::ReadOnly
    }

    async fn execute(&self, _ctx: &ToolContext, raw: ToolArgs) -> Result<ToolOutput> {
        // A malformed argument is a tool-level error, not a turn failure: the
        // model can fix its arguments and try again (design ark-tool.md:29).
        let parsed: args::WebFetchArgs = match args::parse("web_fetch", &raw) {
            Ok(parsed) => parsed,
            Err(err) => return Ok(ToolOutput::err(err.message)),
        };
        Ok(self.fetch(parsed.url.trim()).await)
    }
}

impl WebFetchTool {
    /// Fetch one URL, following 3xx hops ourselves.
    ///
    /// The bus is asked with `follow_redirects: false` on purpose: its own hop
    /// walk re-checks only scheme and allowlist, and this tool's egress
    /// identity is `allow_all()`, so a page redirecting to `169.254.169.254`
    /// would be followed. Walking the hops here is what lets every hop go back
    /// through the SSRF guard. Returning the raw 3xx instead (the previous
    /// behaviour) made ordinary `http`→`https` and trailing-slash redirects
    /// look like failures: twelve `web_fetch` calls in one day's sessions died
    /// on `request returned status 301`, and the model spent a whole turn
    /// re-deriving the final URL by hand.
    async fn fetch(&self, raw_url: &str) -> ToolOutput {
        let mut url = match Url::parse(raw_url) {
            Ok(url) => url,
            Err(err) => return ToolOutput::err(format!("invalid url: {err}")),
        };
        let mut hops = 0_u8;
        loop {
            if let Err(reason) = ensure_public_host_for(&url).await {
                // Structured refusal, not a turn failure: the model can react.
                // The guard refusal is not a dead-URL problem, but the same
                // re-derive hint applies (the guessed name may be wrong), and
                // naming the browser escape hatch keeps one consistent
                // recovery contract for every fetch failure.
                return ToolOutput::err(format!(
                    "target refused by the SSRF guard: {reason}; {FAILURE_GUIDANCE}"
                ));
            }
            let response = self
                .http
                .request(
                    EGRESS_ID,
                    OutboundRequest {
                        method: "GET".into(),
                        url: url.to_string(),
                        follow_redirects: Some(false),
                        ..Default::default()
                    },
                )
                .await;
            let response = match response {
                Ok(response) => response,
                Err(err) => return ToolOutput::err(err.message),
            };
            let Some(target) = redirect_target(&url, &response) else {
                return render(response.status, &response.headers, &response.body);
            };
            hops += 1;
            if hops > MAX_REDIRECTS {
                return ToolOutput::err(format!(
                    "stopped after {MAX_REDIRECTS} redirects; the last target was `{target}`; \
                     {FAILURE_GUIDANCE}"
                ));
            }
            url = target;
        }
    }
}

/// Redirect budget for one `web_fetch` call, matching the bus policy default
/// (`HttpPolicy::max_redirects`) so a fetch that the bus would have followed
/// is not refused here.
const MAX_REDIRECTS: u8 = 5;

/// The next hop for a 3xx response, or `None` when this is a final response.
///
/// Mirrors `ark_http::domain::redirect::next_hop` for the GET-only case this
/// tool uses: `Location` is resolved against the current URL, so a relative
/// redirect works. A 3xx without a usable `Location` is final, and `render`
/// reports its status — guessing a target would hide the server's answer.
fn redirect_target(current: &Url, response: &ark_ports::OutboundResponse) -> Option<Url> {
    if !matches!(response.status, 301 | 302 | 303 | 307 | 308) {
        return None;
    }
    let location = response
        .headers
        .get("location")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())?;
    current.join(location).ok()
}

/// SSRF guard for one URL, naming the offending host.
async fn ensure_public_host_for(url: &Url) -> std::result::Result<(), String> {
    let Some(host) = url.host_str() else {
        return Err(format!("`{url}` has no host"));
    };
    ensure_public_host(host).await
}

/// Recovery guidance appended to fetch failures.
///
/// A bare `request returned status 404` reads like a network outage, so the
/// model retries the same dead URL or abandons the thread (a harness-analysis
/// session logged 36 child + 6 parent 404s that were guessed URLs, with zero
/// attempts to re-discover from a listing page or a browser). The text states
/// the two escape hatches the runtime actually offers: fetch a real index
/// page and follow links, or hand the URL to a browser tool when the page is
/// script-rendered or bot-gated. It must stay a *recovery* hint, not an
/// instruction to retry the identical URL.
const FAILURE_GUIDANCE: &str = "the URL may be wrong (not a network outage): re-derive it from \
a real page (site home, docs index, sitemap, search results) instead of retrying; if the page is \
script-rendered or bot-gated, navigate to it with a browser tool (e.g. playwright.* MCP tools) \
instead of web_fetch.";

/// Render one completed response as model-visible output.
fn render(status: u16, headers: &ark_ports::HeaderMap, body: &str) -> ToolOutput {
    if !(200..300).contains(&status) {
        return ToolOutput::err(format!(
            "request returned status {status}; {FAILURE_GUIDANCE}"
        ));
    }
    let content_type = headers
        .get("content-type")
        .map(String::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    let text = if content_type.starts_with("text/html")
        || content_type.starts_with("application/xhtml")
    {
        extract_text(body)
    } else if content_type.starts_with("text/") || content_type.starts_with("application/json") {
        body.to_string()
    } else {
        return ToolOutput::err(format!(
            "unsupported content type `{content_type}`; only text and JSON bodies are fetched"
        ));
    };
    let capped = cap_bytes(&text, DEFAULT_LIMITS.max_bash_bytes);
    let detail = format!(
        "status={status} type={}",
        content_type.split(';').next().unwrap_or("")
    );
    ToolOutput {
        ok: true,
        content: capped,
        detail: Some(detail),
    }
}

/// Reject hosts that are IP literals or resolve into non-global ranges.
///
/// The check runs at the tool because the bus allowlist is string-based; the
/// known TOCTOU gap between this resolution and the actual connection is
/// accepted for the builtin path (design: requirements/web-fetch-tool.md §3).
async fn ensure_public_host(host: &str) -> std::result::Result<(), String> {
    // An IP literal needs no DNS: resolving e.g. `fe80::1` through
    // `lookup_host` is platform-dependent (some stacks fail, others return a
    // scoped address), which made the guard flaky. Judge literals directly so
    // link-local/loopback/private addresses are always refused.
    let literal = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = literal.parse::<IpAddr>() {
        return if is_public_ip(ip) {
            Ok(())
        } else {
            Err(format!("host `{host}` resolves to a non-public address"))
        };
    }
    let candidates = match tokio::net::lookup_host((host, 443)).await {
        Ok(candidates) => candidates,
        Err(err) => return Err(format!("host `{host}` does not resolve: {err}")),
    };
    for candidate in candidates {
        if !is_public_ip(candidate.ip()) {
            return Err(format!("host `{host}` resolves to a non-public address"));
        }
    }
    Ok(())
}

/// Whether an address is in a globally routable range.
fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            // `is_private` misses 0.0.0.0/8 and 100.64/10 (carrier NAT); both
            // must stay internal.
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_multicast()
                || v4.octets()[0] == 0
                || (v4.octets()[0] == 100 && v4.octets()[1] & 0b1100_0000 == 64))
        }
        IpAddr::V6(v6) => {
            if let Some(mapped) = mapped_v4(v6) {
                return is_public_ip(IpAddr::V4(mapped));
            }
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || unique_local_v6(v6)
                || link_local_v6(v6))
        }
    }
}

fn mapped_v4(v6: std::net::Ipv6Addr) -> Option<std::net::Ipv4Addr> {
    let segments = v6.segments();
    if segments[0..5] == [0, 0, 0, 0, 0] && segments[5] == 0xffff {
        Some(std::net::Ipv4Addr::new(
            (segments[6] >> 8) as u8,
            segments[6] as u8,
            (segments[7] >> 8) as u8,
            segments[7] as u8,
        ))
    } else {
        None
    }
}

/// fc00::/7 unique-local addresses.
fn unique_local_v6(v6: std::net::Ipv6Addr) -> bool {
    v6.segments()[0] & 0xfe00 == 0xfc00
}

/// fe80::/10 link-local addresses.
fn link_local_v6(v6: std::net::Ipv6Addr) -> bool {
    v6.segments()[0] & 0xffc0 == 0xfe80
}

/// Reduce an HTML document to readable text.
///
/// A hand-rolled scanner instead of a HTML crate: the model needs the words,
/// not a DOM, and the whole extractor must stay dependency-free and testable.
/// `script`/`style` bodies are dropped, block boundaries become line breaks
/// (the whitespace that follows them is swallowed), remaining tags are
/// stripped and entities are decoded.
fn extract_text(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut kept = String::with_capacity(html.len());
    let mut cursor = 0;
    // Set after a block boundary: the next text chunk's leading whitespace is
    // dropped so `</p>\n<p>` does not become a blank line.
    let mut swallow_leading_ws = false;
    'scan: while let Some(start) = lower[cursor..].find('<').map(|offset| cursor + offset) {
        let mut chunk = &html[cursor..start];
        if swallow_leading_ws {
            chunk = chunk.trim_start();
        }
        kept.push_str(chunk);
        swallow_leading_ws = false;
        let rest = &lower[start..];
        for tag in ["<script", "<style"] {
            if rest.starts_with(tag) {
                // Drop the element body wholesale; a missing closer eats the
                // remainder, which is the safe direction for a broken page.
                cursor = match lower[start..].find(&format!("</{}", &tag[1..])) {
                    // `close` is relative to `start`; the closer's own `>` ends
                    // the element.
                    Some(close) => lower[start + close..]
                        .find('>')
                        .map_or(html.len(), |t| start + close + t + 1),
                    None => html.len(),
                };
                continue 'scan;
            }
        }
        // Copy the tag's structural meaning: a block boundary becomes a newline.
        cursor = match html[start..].find('>') {
            Some(end) => {
                let tag = &lower[start..=start + end];
                if is_block_boundary(tag) {
                    kept.push('\n');
                    swallow_leading_ws = true;
                }
                start + end + 1
            }
            None => html.len(),
        };
    }
    let mut tail = &html[cursor.min(html.len())..];
    if swallow_leading_ws {
        tail = tail.trim_start();
    }
    kept.push_str(tail);
    decode_entities(&collapse_whitespace(&kept))
}

fn is_block_boundary(tag: &str) -> bool {
    const BLOCKS: [&str; 12] = [
        "</p>", "</div>", "</li>", "</tr>", "</h1>", "</h2>", "</h3>", "</h4>", "</h5>", "</h6>",
        "<br", "</table>",
    ];
    BLOCKS.iter().any(|needle| tag.starts_with(needle))
}

/// Collapse intra-line whitespace and redundant blank lines.
fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut spaces = 0usize;
    let mut newlines = 0usize;
    for ch in text.chars() {
        match ch {
            '\n' => {
                spaces = 0;
                newlines += 1;
                if newlines <= 2 {
                    out.push('\n');
                }
            }
            ' ' | '\t' | '\r' if newlines == 0 => {
                spaces += 1;
                if spaces == 1 {
                    out.push(' ');
                }
            }
            other => {
                spaces = 0;
                newlines = 0;
                out.push(other);
            }
        }
    }
    out.trim().to_string()
}

/// Decode the entities a fetched page plausibly carries.
fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        if ch != '&' {
            out.push(ch);
            continue;
        }
        let rest = &text[index..];
        let decoded = if let Some(semi) = rest.find(';') {
            let entity = &rest[1..semi];
            match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                numeric if numeric.starts_with("#x") || numeric.starts_with("#X") => {
                    u32::from_str_radix(&numeric[2..], 16)
                        .ok()
                        .and_then(char::from_u32)
                }
                numeric if numeric.starts_with('#') => {
                    numeric[1..].parse::<u32>().ok().and_then(char::from_u32)
                }
                _ => None,
            }
        } else {
            None
        };
        match decoded {
            Some(decoded) => {
                out.push(decoded);
                // Skip the raw entity text (the '&' we already consumed plus
                // the remainder up to and including the semicolon).
                let consumed = rest.find(';').unwrap_or(0) + 1;
                while let Some((next_index, _)) = chars.peek() {
                    if *next_index >= index + consumed {
                        break;
                    }
                    chars.next();
                }
            }
            None => out.push('&'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_common::Result;
    use ark_ports::OutboundResponse;
    use std::collections::BTreeMap;

    struct FakeHttp {
        status: u16,
        content_type: String,
        body: String,
    }

    #[async_trait::async_trait]
    impl HttpCapability for FakeHttp {
        async fn request(
            &self,
            plugin_id: &str,
            _req: OutboundRequest,
        ) -> Result<OutboundResponse> {
            assert_eq!(plugin_id, EGRESS_ID);
            let mut headers = BTreeMap::new();
            headers.insert("content-type".to_string(), self.content_type.clone());
            Ok(OutboundResponse {
                status: self.status,
                headers,
                body: self.body.clone(),
            })
        }
    }

    /// Captures the last request so the SSRF redirect override is testable.
    static LAST_FETCH_REQUEST: std::sync::OnceLock<std::sync::Mutex<Option<OutboundRequest>>> =
        std::sync::OnceLock::new();

    #[derive(Default)]
    struct RecordingHttp;

    impl RecordingHttp {
        fn last() -> OutboundRequest {
            LAST_FETCH_REQUEST
                .get_or_init(|| std::sync::Mutex::new(None))
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
                .expect("web_fetch must issue one request")
        }
    }

    #[async_trait::async_trait]
    impl HttpCapability for RecordingHttp {
        async fn request(
            &self,
            _plugin_id: &str,
            req: OutboundRequest,
        ) -> Result<OutboundResponse> {
            *LAST_FETCH_REQUEST
                .get_or_init(|| std::sync::Mutex::new(None))
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(req);
            Ok(OutboundResponse {
                status: 200,
                headers: BTreeMap::new(),
                body: String::new(),
            })
        }
    }

    fn tool(body: &str, content_type: &str) -> WebFetchTool {
        WebFetchTool::new(Arc::new(FakeHttp {
            status: 200,
            content_type: content_type.into(),
            body: body.into(),
        }))
    }

    fn ctx() -> ToolContext {
        ToolContext {
            thread_id: ark_common::ThreadId::new(),
            turn_id: ark_common::TurnId::new(),
            workspace: std::env::temp_dir(),
            sandbox: ark_ports::SandboxDecision::ReadOnly,
            env: Default::default(),
        }
    }

    async fn fetch(url: &str) -> ToolOutput {
        let tool = tool("<html></html>", "text/html");
        tool.execute(&ctx(), ToolArgs(serde_json::json!({ "url": url })))
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn malformed_arguments_are_a_tool_error_not_a_turn_failure() {
        let tool = tool("<html></html>", "text/html");
        for raw in [
            serde_json::json!({}),
            serde_json::json!({"url": 123}),
            serde_json::json!({"url": "not a url"}),
        ] {
            let out = tool.execute(&ctx(), ToolArgs(raw)).await.unwrap();
            assert!(!out.ok, "a bad argument must be a structured tool failure");
        }
    }

    #[tokio::test]
    async fn fetch_disables_redirect_following_for_the_ssrf_guard() {
        // A 3xx to a private host must never become a second connection: the
        // guard only resolved the first hop. Assert the per-request override.
        let tool = WebFetchTool::new(Arc::new(RecordingHttp));
        let _ = tool
            .execute(
                &ctx(),
                ToolArgs(serde_json::json!({"url": "https://1.1.1.1/x"})),
            )
            .await
            .unwrap();
        assert_eq!(
            RecordingHttp::last().follow_redirects,
            Some(false),
            "web_fetch must opt out of bus-level redirects"
        );
    }

    /// Replays a fixed script of responses and records every requested URL, so
    /// a redirect chain is testable end to end without a network.
    struct ScriptedHttp {
        responses: std::sync::Mutex<Vec<OutboundResponse>>,
        urls: std::sync::Mutex<Vec<String>>,
    }

    impl ScriptedHttp {
        fn new(responses: Vec<(u16, Option<&str>, &str)>) -> Self {
            let responses = responses
                .into_iter()
                .map(|(status, location, body)| {
                    let mut headers = BTreeMap::new();
                    headers.insert("content-type".to_string(), "text/html".to_string());
                    if let Some(location) = location {
                        headers.insert("location".to_string(), location.to_string());
                    }
                    OutboundResponse {
                        status,
                        headers,
                        body: body.to_string(),
                    }
                })
                .collect();
            Self {
                responses: std::sync::Mutex::new(responses),
                urls: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn urls(&self) -> Vec<String> {
            self.urls.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl HttpCapability for ScriptedHttp {
        async fn request(
            &self,
            _plugin_id: &str,
            req: OutboundRequest,
        ) -> Result<OutboundResponse> {
            self.urls.lock().unwrap().push(req.url.clone());
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                return Err(ark_common::ArkError::new(
                    ark_common::ArkErrorCode::ToolExecutionFailed,
                    "scripted http ran out of responses",
                ));
            }
            Ok(responses.remove(0))
        }
    }

    #[tokio::test]
    async fn a_permanent_redirect_is_followed_and_re_guarded() {
        // Ordinary https migrations answer 301. Returning it raw made every
        // such fetch a failure; the tool now walks the hop itself so the guard
        // runs again on the new URL.
        let scripted = Arc::new(ScriptedHttp::new(vec![
            (301, Some("/moved"), ""),
            (200, None, "<html><body>final page</body></html>"),
        ]));
        let tool = WebFetchTool::new(Arc::clone(&scripted) as Arc<dyn HttpCapability>);
        let out = tool
            .execute(
                &ctx(),
                ToolArgs(serde_json::json!({"url": "https://1.1.1.1/start"})),
            )
            .await
            .unwrap();
        assert!(out.ok, "{}", out.content);
        assert!(out.content.contains("final page"), "{}", out.content);
        assert_eq!(
            scripted.urls(),
            vec![
                "https://1.1.1.1/start".to_string(),
                "https://1.1.1.1/moved".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn a_redirect_to_a_private_host_is_refused_after_the_hop() {
        // The bus would follow this silently under `allow_all()`; the tool must
        // resolve the new host and refuse it instead of opening the connection.
        let scripted = Arc::new(ScriptedHttp::new(vec![(
            302,
            Some("https://169.254.169.254/latest/meta-data/"),
            "",
        )]));
        let tool = WebFetchTool::new(Arc::clone(&scripted) as Arc<dyn HttpCapability>);
        let out = tool
            .execute(
                &ctx(),
                ToolArgs(serde_json::json!({"url": "https://1.1.1.1/start"})),
            )
            .await
            .unwrap();
        assert!(!out.ok, "{}", out.content);
        assert!(out.content.contains("SSRF"), "{}", out.content);
        assert_eq!(
            scripted.urls().len(),
            1,
            "the private hop must never be requested"
        );
    }

    #[tokio::test]
    async fn a_redirect_loop_stops_at_the_budget() {
        let responses: Vec<(u16, Option<&str>, &str)> = (0..(MAX_REDIRECTS as usize + 2))
            .map(|_| (302, Some("/loop"), ""))
            .collect();
        let tool = WebFetchTool::new(Arc::new(ScriptedHttp::new(responses)));
        let out = tool
            .execute(
                &ctx(),
                ToolArgs(serde_json::json!({"url": "https://1.1.1.1/start"})),
            )
            .await
            .unwrap();
        assert!(!out.ok, "{}", out.content);
        assert!(out.content.contains("redirects"), "{}", out.content);
    }

    #[tokio::test]
    async fn a_redirect_without_a_usable_location_reports_its_status() {
        let tool = WebFetchTool::new(Arc::new(ScriptedHttp::new(vec![(302, None, "")])));
        let out = tool
            .execute(
                &ctx(),
                ToolArgs(serde_json::json!({"url": "https://1.1.1.1/start"})),
            )
            .await
            .unwrap();
        assert!(!out.ok, "{}", out.content);
        assert!(out.content.contains("302"), "{}", out.content);
    }

    #[tokio::test]
    async fn private_and_loopback_targets_are_refused() {
        for url in [
            "https://127.0.0.1/x",
            "https://10.1.2.3/x",
            "https://192.168.1.4/x",
            "https://172.16.0.9/x",
            "https://169.254.1.1/x",
            "https://[fe80::1]/x",
            "https://[fc00::1]/x",
            "https://0.0.0.0/x",
        ] {
            let out = fetch(url).await;
            assert!(!out.ok, "{url} must be refused");
            assert!(out.content.contains("SSRF"), "got {}", out.content);
        }
    }

    #[tokio::test]
    async fn a_non_2xx_status_is_a_structured_failure_with_recovery_guidance() {
        let tool = WebFetchTool::new(Arc::new(FakeHttp {
            status: 404,
            content_type: "text/html".into(),
            body: "gone".into(),
        }));
        let out = tool
            .execute(
                &ctx(),
                ToolArgs(serde_json::json!({"url": "https://1.1.1.1/x"})),
            )
            .await
            .unwrap();
        assert!(!out.ok);
        assert!(out.content.contains("404"));
        // The failure must teach the escape hatches, not just report: a bare
        // status reads like an outage and the model retries the same URL.
        assert!(
            out.content.contains("re-derive"),
            "failure must carry re-derive guidance, got: {}",
            out.content
        );
        assert!(
            out.content.contains("browser tool"),
            "failure must name the browser escape hatch, got: {}",
            out.content
        );
    }

    #[tokio::test]
    async fn html_bodies_are_reduced_to_text() {
        let tool = tool(
            "<html><head><style>body{}</style></head><body><h1>Title</h1><p>a&amp;b &#65; <span>c</span></p></body></html>",
            "text/html; charset=utf-8",
        );
        let out = tool
            .execute(
                &ctx(),
                ToolArgs(serde_json::json!({"url": "https://1.1.1.1"})),
            )
            .await
            .unwrap();
        assert!(out.ok);
        assert!(out.content.contains("Title"));
        assert!(out.content.contains("a&b A c"));
        assert!(out.detail.as_deref().unwrap_or("").contains("status=200"));
    }

    #[tokio::test]
    async fn binary_content_types_are_refused() {
        let tool = tool("\x00\x00\x01", "image/png");
        let out = tool
            .execute(
                &ctx(),
                ToolArgs(serde_json::json!({"url": "https://1.1.1.1/a.png"})),
            )
            .await
            .unwrap();
        assert!(!out.ok);
        assert!(out.content.contains("image/png"));
    }

    #[test]
    fn oversized_output_is_capped_at_the_bash_budget() {
        let huge = "x".repeat(DEFAULT_LIMITS.max_bash_bytes + 100);
        let out = render(200, &header("text/plain"), &huge);
        assert!(out.content.contains("[truncated:"));
    }

    fn header(value: &str) -> ark_ports::HeaderMap {
        let mut headers = BTreeMap::new();
        headers.insert("content-type".to_string(), value.to_string());
        headers
    }

    #[test]
    fn entities_and_whitespace_are_normalized() {
        assert_eq!(decode_entities("a&amp;b&#x41;&#66;&nbsp;c"), "a&bAB c");
        assert_eq!(collapse_whitespace("a  b\n\n\n\nc"), "a b\n\nc");
        assert_eq!(extract_text("<p>a</p>\n<p>b</p><script>x</script>"), "a\nb");
    }
}
