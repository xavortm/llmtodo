//! LLM-592: a preview build of a ticket's branch. `scripts/preview-build`
//! records it, and whatever launches it sets `LLMTODO_PREVIEW_TICKET` to the
//! ticket key. The app then marks itself as that ticket's preview: a yellow
//! title bar, a gray Dock icon with the key, and no Restart to update.

/// The variable that names the ticket a preview build shows.
pub const TICKET_VAR: &str = "LLMTODO_PREVIEW_TICKET";

/// The ticket key this process previews, or None for any other run.
pub fn ticket_from_env() -> Option<String> {
    parse(std::env::var(TICKET_VAR).ok().as_deref())
}

/// A ticket key such as `LLM-123`, in capitals. Anything else is no
/// preview, so a stray value never paints the owner's app yellow.
pub fn parse(raw: Option<&str>) -> Option<String> {
    let key = raw?.trim();
    let (team, number) = key.split_once('-')?;
    let team_ok = !team.is_empty() && team.chars().all(|c| c.is_ascii_alphanumeric());
    let number_ok = !number.is_empty() && number.chars().all(|c| c.is_ascii_digit());
    (team_ok && number_ok).then(|| key.to_ascii_uppercase())
}

/// The text the title bar shows for a preview of `key`.
pub fn title(key: &str) -> String {
    format!("Preview build · {key}")
}

#[cfg(test)]
mod tests {
    use super::{parse, title};

    #[test]
    fn a_ticket_key_is_a_preview() {
        assert_eq!(parse(Some("LLM-123")).as_deref(), Some("LLM-123"));
        assert_eq!(parse(Some(" llm-7 ")).as_deref(), Some("LLM-7"));
        assert_eq!(title("LLM-123"), "Preview build · LLM-123");
    }

    #[test]
    fn anything_else_is_no_preview() {
        for raw in [
            None,
            Some(""),
            Some("LLM"),
            Some("LLM-"),
            Some("-12"),
            Some("LLM-1a"),
            Some("L M-1"),
        ] {
            assert_eq!(parse(raw), None, "{raw:?}");
        }
    }
}
