//! Prepares a description for the Markdown preview, and routes clicks on
//! its links.
//!
//! - Stored files, `/v1/files/<hash>`, get the running server's address,
//!   so images load and file links open.
//! - Bare issue keys such as `LLM-12` become links that open the issue in
//!   the app. Keys inside code, inside a link or inside a URL stay text.
//! - A click on an issue link opens the issue. Any other link opens in the
//!   browser.

use llmtodo_core::find_keys;

/// The scheme of the links this module writes for issue keys.
const ISSUE_LINK: &str = "llmtodo://issue/";

/// The path of a stored file on the server.
const FILES_PATH: &str = "/v1/files/";

/// Where a link in the preview goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    /// An issue in the app, by key.
    Issue(String),
    /// A page for the browser.
    Browser(String),
}

/// The target of a clicked link. A link written by [`prepare`] names an
/// issue. So does a link whose whole destination is a key, as in
/// `[the map](LLM-1)`. A server path such as `/v1/files/…` opens on
/// `server`.
pub fn link_target(url: &str, server: Option<&str>) -> LinkTarget {
    let url = url.trim();
    if let Some(key) = url.strip_prefix(ISSUE_LINK) {
        return LinkTarget::Issue(key.to_string());
    }
    if find_keys(url)
        .first()
        .is_some_and(|m| m.start == 0 && m.end == url.len())
    {
        return LinkTarget::Issue(url.to_string());
    }
    match server {
        Some(server) if url.starts_with('/') => {
            LinkTarget::Browser(format!("{}{url}", server.trim_end_matches('/')))
        }
        _ => LinkTarget::Browser(url.to_string()),
    }
}

/// The Markdown the preview renders for `src`. `server` is the running
/// server's base URL, such as `http://127.0.0.1:7878`. `is_issue` says
/// whether a key names an issue the app knows, so unknown keys stay text.
pub fn prepare(src: &str, server: Option<&str>, is_issue: impl Fn(&str) -> bool) -> String {
    let mut out = String::with_capacity(src.len() + 64);
    let mut fence: Option<(u8, usize)> = None;
    for line in src.split_inclusive('\n') {
        let trimmed = line.trim_start_matches(' ');
        let indent = line.len() - trimmed.len();
        let marker = trimmed.bytes().next().filter(|b| matches!(b, b'`' | b'~'));
        let run = marker.map_or(0, |m| trimmed.bytes().take_while(|&b| b == m).count());
        match fence {
            // The closing fence: the same character, at least as long.
            Some((m, len)) => {
                if indent < 4 && marker == Some(m) && run >= len && trimmed[run..].trim().is_empty()
                {
                    fence = None;
                }
                out.push_str(line);
            }
            None if indent < 4 && run >= 3 => {
                fence = marker.map(|m| (m, run));
                out.push_str(line);
            }
            None => prepare_line(line, server, &is_issue, &mut out),
        }
    }
    out
}

/// One line outside a fenced code block. Inline code spans stay as they
/// are; the text between them is rewritten.
fn prepare_line(
    line: &str,
    server: Option<&str>,
    is_issue: &impl Fn(&str) -> bool,
    out: &mut String,
) {
    let b = line.as_bytes();
    let mut text_start = 0;
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'`' {
            i += 1;
            continue;
        }
        let run = b[i..].iter().take_while(|&&c| c == b'`').count();
        // The span ends at the next run of exactly the same length.
        let mut j = i + run;
        let mut close = None;
        while j < b.len() {
            if b[j] == b'`' {
                let r = b[j..].iter().take_while(|&&c| c == b'`').count();
                if r == run {
                    close = Some(j + r);
                    break;
                }
                j += r;
            } else {
                j += 1;
            }
        }
        match close {
            Some(end) => {
                prepare_text(&line[text_start..i], server, is_issue, out);
                out.push_str(&line[i..end]);
                text_start = end;
                i = end;
            }
            // An unclosed run is plain text.
            None => i += run,
        }
    }
    prepare_text(&line[text_start..], server, is_issue, out);
}

/// Text outside code: adds the server to file paths and links bare keys.
fn prepare_text(
    text: &str,
    server: Option<&str>,
    is_issue: &impl Fn(&str) -> bool,
    out: &mut String,
) {
    let protected = protected_ranges(text);
    let inside = |at: usize| protected.iter().any(|r| r.contains(&at));
    let keys: Vec<_> = find_keys(text)
        .into_iter()
        .filter(|m| !inside(m.start) && is_issue(&text[m.start..m.end]))
        .collect();
    let server = server.map(|s| s.trim_end_matches('/'));
    let mut keys = keys.into_iter().peekable();
    let mut i = 0;
    while i < text.len() {
        if let Some(m) = keys.next_if(|m| m.start == i) {
            let key = &text[m.start..m.end];
            out.push_str(&format!("[{key}]({ISSUE_LINK}{key})"));
            i = m.end;
            continue;
        }
        if let Some(server) = server
            && text[i..].starts_with(FILES_PATH)
            && starts_a_destination(text, i)
        {
            out.push_str(server);
        }
        let c = text[i..].chars().next().expect("i is on a char boundary");
        out.push(c);
        i += c.len_utf8();
    }
}

/// Whether a path at `at` begins a link or image destination: after `(`,
/// a quote or `<`, or after `]:` in a link reference definition.
fn starts_a_destination(text: &str, at: usize) -> bool {
    let before = text[..at].trim_end_matches(' ');
    before.ends_with(['(', '"', '\'', '<', '='])
        || (before.ends_with("]:") && before.trim_start().starts_with('['))
}

/// Byte ranges where a key must stay text: link text and destinations,
/// autolinks and HTML tags, and words that hold a URL.
fn protected_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'[' => {
                // `[text](dest)`, `[text][ref]` and `[ref]: dest`.
                if let Some(close) = text[i..].find(']').map(|c| i + c) {
                    let after = b.get(close + 1).copied();
                    if matches!(after, Some(b'(' | b'[' | b':')) {
                        out.push(i..close + 1);
                        if let Some(end) = matching_close(text, close + 1) {
                            out.push(close + 1..end);
                            i = end;
                            continue;
                        }
                        if after == Some(b':') {
                            out.push(close + 1..b.len());
                            break;
                        }
                    }
                }
                i += 1;
            }
            b'<' => match text[i..].find('>') {
                Some(end) => {
                    out.push(i..i + end + 1);
                    i += end + 1;
                }
                None => i += 1,
            },
            _ => i += 1,
        }
    }
    // Words with a URL in them, such as `https://github.com/o/r/LLM-12`.
    let mut start = 0;
    for word in text.split_inclusive(char::is_whitespace) {
        if word.contains("://") || word.starts_with("www.") {
            out.push(start..start + word.len());
        }
        start += word.len();
    }
    out
}

/// The end of the bracket group that opens at `open`, `(…)` or `[…]`.
fn matching_close(text: &str, open: usize) -> Option<usize> {
    let (o, c) = match text.as_bytes().get(open)? {
        b'(' => (b'(', b')'),
        b'[' => (b'[', b']'),
        _ => return None,
    };
    let mut depth = 0;
    for (ix, &ch) in text.as_bytes().iter().enumerate().skip(open) {
        if ch == o {
            depth += 1;
        } else if ch == c {
            depth -= 1;
            if depth == 0 {
                return Some(ix + 1);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERVER: Option<&str> = Some("http://127.0.0.1:7878");

    fn known(key: &str) -> bool {
        key.starts_with("LLM-")
    }

    fn run(src: &str) -> String {
        prepare(src, SERVER, known)
    }

    #[test]
    fn stored_images_get_the_server_address() {
        assert_eq!(
            run("![icon](/v1/files/abc)\n"),
            "![icon](http://127.0.0.1:7878/v1/files/abc)\n"
        );
        assert_eq!(
            run("<img src=\"/v1/files/abc\">"),
            "<img src=\"http://127.0.0.1:7878/v1/files/abc\">"
        );
        assert_eq!(
            run("[icon]: /v1/files/abc"),
            "[icon]: http://127.0.0.1:7878/v1/files/abc"
        );
        // Prose and code that name the path stay as they are.
        assert_eq!(
            run("Files live at /v1/files/<hash>."),
            "Files live at /v1/files/<hash>."
        );
        assert_eq!(run("`![x](/v1/files/a)`"), "`![x](/v1/files/a)`");
        // Remote images stay as they are.
        let remote = "![r](https://placehold.co/320x120/png)";
        assert_eq!(run(remote), remote);
        // With no server, nothing changes.
        assert_eq!(
            prepare("![i](/v1/files/a)", None, known),
            "![i](/v1/files/a)"
        );
    }

    #[test]
    fn bare_keys_become_issue_links() {
        assert_eq!(
            run("See LLM-12, then LLM-3."),
            "See [LLM-12](llmtodo://issue/LLM-12), then [LLM-3](llmtodo://issue/LLM-3)."
        );
        // A key the app does not know stays text.
        assert_eq!(run("ALX-9 is elsewhere"), "ALX-9 is elsewhere");
    }

    #[test]
    fn keys_in_code_links_and_urls_stay_text() {
        let cases = [
            "`LLM-12`",
            "``a `LLM-12` b``",
            "[LLM-12 fix](https://example.com)",
            "[docs](https://example.com/LLM-12)",
            "https://github.com/o/r/pull/LLM-12",
            "<https://example.com/LLM-12>",
            "[ref]: https://example.com/LLM-12",
        ];
        for case in cases {
            assert_eq!(run(case), case, "{case}");
        }
        let fenced = "```\nLLM-12\n```\nLLM-13\n";
        assert_eq!(
            run(fenced),
            "```\nLLM-12\n```\n[LLM-13](llmtodo://issue/LLM-13)\n"
        );
        let tilde = "~~~~\n```\nLLM-12\n~~~~\n";
        assert_eq!(run(tilde), tilde);
    }

    #[test]
    fn clicks_route_issues_to_the_app() {
        assert_eq!(
            link_target("llmtodo://issue/LLM-12", SERVER),
            LinkTarget::Issue("LLM-12".into())
        );
        assert_eq!(
            link_target("LLM-1", SERVER),
            LinkTarget::Issue("LLM-1".into())
        );
        assert_eq!(
            link_target("https://zed.dev", SERVER),
            LinkTarget::Browser("https://zed.dev".into())
        );
        assert_eq!(
            link_target("/v1/files/abc", SERVER),
            LinkTarget::Browser("http://127.0.0.1:7878/v1/files/abc".into())
        );
    }
}
