//! Owner questions: a fixed `+++ Owner questions` section in a description.
//!
//! Agents ask, the owner answers, and both live in the description, so it
//! stays the record. The format:
//!
//! ```text
//! +++ Owner questions
//! ### Q1. Should the badge slide in or use a fixed column?
//! Optional context, any Markdown.
//!
//! - a) Slide in and push the row
//! - b) Fixed column
//!
//! **Answer:** b) Fixed column
//!
//! ### Q2. What should the binary be called?
//!
//! **Answer:**
//! +++
//! ```
//!
//! - A question starts with a `### Q<n>. ` heading. `Q<n>:` works too.
//! - Lines `- a) text` before the answer line are options. A question with
//!   options is a choice; one without is free text.
//! - `**Answer:**` (or `Answer:`) holds the answer, which runs to the next
//!   question or the end of the section. An empty answer means open.

use serde::{Deserialize, Serialize};

use crate::markdown::{PatchError, Section, SectionKind, sections};

/// The title of the questions section.
pub const QUESTIONS_SECTION: &str = "Owner questions";

const ANSWER_MARK: &str = "**Answer:**";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct QuestionOption {
    /// One lowercase letter: `a`, `b`, `c` ...
    pub letter: char,
    pub text: String,
}

/// One parsed question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Question {
    /// `Q1`, `Q2` ...
    pub id: String,
    pub number: u32,
    pub text: String,
    /// Markdown between the heading and the options or the answer.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub context: String,
    /// Empty for a free-text question.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<QuestionOption>,
    /// `None` while the question is open.
    pub answer: Option<String>,
}

impl Question {
    pub fn is_open(&self) -> bool {
        self.answer.is_none()
    }

    pub fn is_choice(&self) -> bool {
        !self.options.is_empty()
    }

    /// The text an answer stores: `b` on a choice becomes `b) Its option`,
    /// and anything else stays as typed.
    pub fn answer_text(&self, answer: &str) -> String {
        let a = answer.trim();
        match self.options.iter().find(|o| {
            a.len() == 1 && a.chars().next().map(|c| c.to_ascii_lowercase()) == Some(o.letter)
        }) {
            Some(o) => format!("{}) {}", o.letter, o.text),
            None => a.to_string(),
        }
    }

    /// The option the answer picks, when it starts with an option letter.
    pub fn chosen(&self) -> Option<char> {
        let a = self.answer.as_deref()?;
        let letter = option_letter(a).map(|(l, _)| l).or_else(|| {
            let t = a.trim();
            (t.len() == 1).then(|| t.chars().next().unwrap().to_ascii_lowercase())
        })?;
        self.options
            .iter()
            .any(|o| o.letter == letter)
            .then_some(letter)
    }
}

/// `Q12. text` or `Q12: text` → (12, text).
fn question_heading(line: &str) -> Option<(u32, String)> {
    let t = line.trim_start();
    if line.len() - t.len() > 3 {
        return None;
    }
    let rest = t.strip_prefix("###")?;
    if rest.starts_with('#') {
        return None;
    }
    let rest = rest.trim_start();
    let rest = rest.strip_prefix(['Q', 'q'])?;
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    let number: u32 = rest[..digits].parse().ok()?;
    let rest = rest[digits..].strip_prefix(['.', ':', ')'])?;
    Some((number, rest.trim().to_string()))
}

/// `a) text` or `a. text` → ('a', text). The letter must be lowercase.
fn option_letter(s: &str) -> Option<(char, &str)> {
    let t = s.trim_start();
    let mut chars = t.chars();
    let c = chars.next()?;
    if !c.is_ascii_lowercase() {
        return None;
    }
    let rest = chars.as_str().strip_prefix([')', '.'])?;
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None;
    }
    Some((c, rest.trim()))
}

fn option_line(line: &str) -> Option<(char, &str)> {
    let t = line.trim_start();
    let rest = t.strip_prefix("- ").or_else(|| t.strip_prefix("* "))?;
    option_letter(rest)
}

fn answer_line(line: &str) -> Option<&str> {
    let t = line.trim_start();
    t.strip_prefix(ANSWER_MARK)
        .or_else(|| t.strip_prefix("Answer:"))
        .map(str::trim)
}

fn questions_section(src: &str) -> Result<Option<Section>, PatchError> {
    let mut hits: Vec<Section> = sections(src)
        .into_iter()
        .filter(|s| s.kind == SectionKind::Collapsible)
        .filter(|s| s.title.eq_ignore_ascii_case(QUESTIONS_SECTION))
        .collect();
    match hits.len() {
        0 => Ok(None),
        1 => Ok(Some(hits.remove(0))),
        n => Err(PatchError::MatchCount {
            target: QUESTIONS_SECTION.into(),
            count: n,
        }),
    }
}

/// One question's span inside the section body, as byte offsets into the
/// whole document.
struct Span {
    q: Question,
    /// Start of the answer line, or `None` when the question has none.
    answer_start: Option<usize>,
    /// End of the question's text: the next question or the section end.
    end: usize,
}

fn parse_spans(src: &str, s: &Section) -> Vec<Span> {
    let body = &src[s.body_start..s.body_end];
    let mut out: Vec<Span> = Vec::new();
    let mut pos = s.body_start;
    let mut in_fence = false;
    let mut context: Vec<&str> = Vec::new();
    let mut answer: Option<Vec<&str>> = None;
    let finish = |out: &mut Vec<Span>, context: &mut Vec<&str>, answer: &mut Option<Vec<&str>>| {
        if let Some(last) = out.last_mut() {
            last.q.context = context.join("\n").trim().to_string();
            last.q.answer = answer
                .take()
                .map(|a| a.join("\n").trim().to_string())
                .filter(|a| !a.is_empty());
        }
        context.clear();
    };
    for raw in body.split_inclusive('\n') {
        let line = raw.trim_end_matches(['\n', '\r']);
        let start = pos;
        pos += raw.len();
        let fence = line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~");
        if !in_fence && let Some((number, text)) = question_heading(line) {
            finish(&mut out, &mut context, &mut answer);
            if let Some(last) = out.last_mut() {
                last.end = start;
            }
            out.push(Span {
                q: Question {
                    id: format!("Q{number}"),
                    number,
                    text,
                    context: String::new(),
                    options: Vec::new(),
                    answer: None,
                },
                answer_start: None,
                end: s.body_end,
            });
            continue;
        }
        let Some(cur) = out.last_mut() else {
            continue;
        };
        if let Some(a) = &mut answer {
            a.push(line);
        } else if !in_fence && let Some(a) = answer_line(line) {
            cur.answer_start = Some(start);
            answer = Some(vec![a]);
        } else if !in_fence && let Some((letter, text)) = option_line(line) {
            cur.q.options.push(QuestionOption {
                letter,
                text: text.to_string(),
            });
        } else if cur.q.options.is_empty() {
            context.push(line);
        }
        if fence {
            in_fence = !in_fence;
        }
    }
    finish(&mut out, &mut context, &mut answer);
    out
}

/// Every question in the description's `+++ Owner questions` section, in
/// order. A description without that section has none.
pub fn questions(src: &str) -> Vec<Question> {
    match questions_section(src) {
        Ok(Some(s)) => parse_spans(src, &s).into_iter().map(|s| s.q).collect(),
        _ => Vec::new(),
    }
}

/// Questions that have no answer yet.
pub fn open_questions(src: &str) -> Vec<Question> {
    questions(src)
        .into_iter()
        .filter(Question::is_open)
        .collect()
}

fn question_block(number: u32, text: &str, context: &str, options: &[QuestionOption]) -> String {
    let mut b = format!("### Q{number}. {}\n", text.trim());
    if !context.trim().is_empty() {
        b.push_str(context.trim());
        b.push('\n');
    }
    if !options.is_empty() {
        b.push('\n');
        for o in options {
            b.push_str(&format!("- {}) {}\n", o.letter, o.text.trim()));
        }
    }
    b.push('\n');
    b.push_str(ANSWER_MARK);
    b.push('\n');
    b
}

/// Checks the options of a new question: one to 26, unique lowercase
/// letters, no empty text.
pub fn validate_options(options: &[QuestionOption]) -> Result<(), PatchError> {
    if options.len() == 1 {
        return Err(PatchError::Question(
            "a choice needs at least two options".into(),
        ));
    }
    let mut seen = Vec::new();
    for o in options {
        if !o.letter.is_ascii_lowercase() {
            return Err(PatchError::Question(format!(
                "option letter {:?} must be a lowercase letter",
                o.letter
            )));
        }
        if seen.contains(&o.letter) {
            return Err(PatchError::Question(format!(
                "option letter {:?} is used twice",
                o.letter
            )));
        }
        if o.text.trim().is_empty() || o.text.contains('\n') {
            return Err(PatchError::Question(format!(
                "option {:?} needs one line of text",
                o.letter
            )));
        }
        seen.push(o.letter);
    }
    Ok(())
}

/// Adds a question at the end of the `+++ Owner questions` section, and
/// creates the section at the end of the description when it is missing.
/// Returns the new text and the new question's ID.
pub fn ask(
    src: &str,
    text: &str,
    context: &str,
    options: &[QuestionOption],
) -> Result<(String, String), PatchError> {
    let text = text.trim();
    if text.is_empty() || text.contains('\n') {
        return Err(PatchError::Question(
            "the question needs one line of text".into(),
        ));
    }
    validate_options(options)?;
    match questions_section(src)? {
        None => {
            let block = question_block(1, text, context, options);
            let mut out = src.trim_end().to_string();
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(&format!("+++ {QUESTIONS_SECTION}\n{block}+++\n"));
            Ok((out, "Q1".into()))
        }
        Some(s) => {
            let n = parse_spans(src, &s)
                .iter()
                .map(|s| s.q.number)
                .max()
                .unwrap_or(0)
                + 1;
            let block = question_block(n, text, context, options);
            let before = src[s.body_start..s.body_end].trim_end();
            let mut body = before.to_string();
            if !body.is_empty() {
                body.push_str("\n\n");
            }
            body.push_str(&block);
            let tail = if s.body_end == src.len() {
                "+++\n"
            } else {
                &src[s.body_end..]
            };
            Ok((
                format!("{}{body}{tail}", &src[..s.body_start]),
                format!("Q{n}"),
            ))
        }
    }
}

/// Parses `Q3`, `q3` or `3`.
fn parse_id(id: &str) -> Option<u32> {
    let t = id.trim();
    let t = t.strip_prefix(['Q', 'q']).unwrap_or(t);
    t.parse().ok()
}

/// The question with this ID, such as `Q1`, `q1` or `1`.
pub fn find(src: &str, id: &str) -> Result<Question, PatchError> {
    let Some(number) = parse_id(id) else {
        return Err(PatchError::Question(format!(
            "{id:?} is not a question ID such as Q1"
        )));
    };
    questions(src)
        .into_iter()
        .find(|q| q.number == number)
        .ok_or_else(|| PatchError::Question(format!("no question Q{number}")))
}

/// Writes an answer into a question, replacing an earlier one. For a choice,
/// a single option letter expands to `b) Option text`. Any other text is
/// kept as written, so the owner can answer outside the options. An empty
/// answer opens the question again.
pub fn answer(src: &str, id: &str, answer: &str) -> Result<String, PatchError> {
    let Some(number) = parse_id(id) else {
        return Err(PatchError::Question(format!(
            "{id:?} is not a question ID such as Q1"
        )));
    };
    let Some(s) = questions_section(src)? else {
        return Err(PatchError::Question(format!(
            "the description has no \"+++ {QUESTIONS_SECTION}\" section"
        )));
    };
    let spans = parse_spans(src, &s);
    let hits: Vec<&Span> = spans.iter().filter(|sp| sp.q.number == number).collect();
    let span = match hits.as_slice() {
        [one] => *one,
        [] => {
            return Err(PatchError::Question(format!("no question Q{number}")));
        }
        _ => {
            return Err(PatchError::Question(format!(
                "Q{number} is in the section twice"
            )));
        }
    };
    let text = span.q.answer_text(answer);
    let line = if text.is_empty() {
        format!("{ANSWER_MARK}\n")
    } else {
        format!("{ANSWER_MARK} {text}\n")
    };
    // The answer runs to the next question, so keep one blank line before
    // that heading.
    let sep = if span.end < s.body_end { "\n" } else { "" };
    let (start, end) = match span.answer_start {
        Some(start) => (start, span.end),
        None => {
            let body = &src[..span.end];
            let trimmed = body.trim_end().len();
            (trimmed, span.end)
        }
    };
    let lead = if span.answer_start.is_none() {
        "\n\n"
    } else {
        ""
    };
    let mut out = String::with_capacity(src.len() + line.len());
    out.push_str(&src[..start]);
    out.push_str(lead);
    out.push_str(&line);
    out.push_str(sep);
    if end == src.len() && s.body_end == src.len() {
        out.push_str("+++\n");
    }
    out.push_str(&src[end..]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(list: &[(char, &str)]) -> Vec<QuestionOption> {
        list.iter()
            .map(|(l, t)| QuestionOption {
                letter: *l,
                text: (*t).into(),
            })
            .collect()
    }

    const DOC: &str = "Intro.\n\n+++ Owner questions\n### Q1. Slide or fixed?\nThe badge pushes rows now.\n\n- a) Slide in\n- b) Fixed column\n\n**Answer:**\n\n### Q2. Binary name?\n\n**Answer:** llmtodo\n+++\n\n+++ Agent notes\nnotes\n+++\n";

    #[test]
    fn parses_both_kinds() {
        let qs = questions(DOC);
        assert_eq!(qs.len(), 2);
        assert_eq!(qs[0].id, "Q1");
        assert_eq!(qs[0].text, "Slide or fixed?");
        assert_eq!(qs[0].context, "The badge pushes rows now.");
        assert_eq!(
            qs[0].options,
            opts(&[('a', "Slide in"), ('b', "Fixed column")])
        );
        assert_eq!(qs[0].answer, None);
        assert!(qs[0].is_choice());
        assert_eq!(qs[1].answer.as_deref(), Some("llmtodo"));
        assert!(!qs[1].is_choice());
        assert_eq!(open_questions(DOC).len(), 1);
    }

    #[test]
    fn no_section_means_no_questions() {
        assert!(questions("### Q1. Not in a section\n**Answer:**\n").is_empty());
    }

    #[test]
    fn ask_creates_the_section() {
        let (out, id) = ask("Body.", "Which?", "", &opts(&[('a', "Fork"), ('b', "PR")])).unwrap();
        assert_eq!(id, "Q1");
        assert_eq!(
            out,
            "Body.\n\n+++ Owner questions\n### Q1. Which?\n\n- a) Fork\n- b) PR\n\n**Answer:**\n+++\n"
        );
        let (out, id) = ask(&out, "Name?", "Short, please.", &[]).unwrap();
        assert_eq!(id, "Q2");
        assert!(
            out.ends_with("**Answer:**\n\n### Q2. Name?\nShort, please.\n\n**Answer:**\n+++\n"),
            "{out}"
        );
        assert_eq!(questions(&out).len(), 2);
    }

    #[test]
    fn ask_keeps_later_sections() {
        let (out, id) = ask(DOC, "Third?", "", &[]).unwrap();
        assert_eq!(id, "Q3");
        assert!(
            out.contains("**Answer:** llmtodo\n\n### Q3. Third?\n\n**Answer:**\n+++\n\n+++ Agent notes\nnotes\n+++\n"),
            "{out}"
        );
    }

    #[test]
    fn answer_expands_a_letter() {
        let out = answer(DOC, "q1", "B").unwrap();
        let q = &questions(&out)[0];
        assert_eq!(q.answer.as_deref(), Some("b) Fixed column"));
        assert_eq!(q.chosen(), Some('b'));
        assert!(
            out.contains("**Answer:** b) Fixed column\n\n### Q2."),
            "{out}"
        );
        assert!(open_questions(&out).is_empty());
        // Everything after the questions section stays.
        assert!(out.ends_with("+++\n\n+++ Agent notes\nnotes\n+++\n"));
    }

    #[test]
    fn answer_replaces_and_reopens() {
        let out = answer(DOC, "Q2", "lt\nsecond line").unwrap();
        assert_eq!(
            questions(&out)[1].answer.as_deref(),
            Some("lt\nsecond line")
        );
        let out = answer(&out, "2", "").unwrap();
        assert_eq!(questions(&out)[1].answer, None);
        assert_eq!(questions(&out).len(), 2);
    }

    #[test]
    fn free_text_answer_on_a_choice() {
        let out = answer(DOC, "Q1", "Neither, hide it").unwrap();
        let q = &questions(&out)[0];
        assert_eq!(q.answer.as_deref(), Some("Neither, hide it"));
        assert_eq!(q.chosen(), None);
    }

    #[test]
    fn answer_adds_a_missing_answer_line() {
        let src = "+++ Owner questions\n### Q1. Hand written?\n+++\n";
        let out = answer(src, "Q1", "yes").unwrap();
        assert_eq!(
            out,
            "+++ Owner questions\n### Q1. Hand written?\n\n**Answer:** yes\n+++\n"
        );
    }

    #[test]
    fn answer_in_an_unclosed_section() {
        let src = "+++ Owner questions\n### Q1. Open?\n**Answer:**";
        let out = answer(src, "Q1", "x").unwrap();
        assert_eq!(
            out,
            "+++ Owner questions\n### Q1. Open?\n**Answer:** x\n+++\n"
        );
    }

    #[test]
    fn bad_input_fails() {
        assert!(answer(DOC, "Q9", "x").is_err());
        assert!(answer(DOC, "nope", "x").is_err());
        assert!(answer("no section", "Q1", "x").is_err());
        assert!(ask("", "", "", &[]).is_err());
        assert!(ask("", "Two\nlines", "", &[]).is_err());
        assert!(ask("", "One?", "", &opts(&[('a', "only")])).is_err());
        assert!(ask("", "Dup?", "", &opts(&[('a', "x"), ('a', "y")])).is_err());
        assert!(ask("", "Upper?", "", &opts(&[('A', "x"), ('b', "y")])).is_err());
    }

    #[test]
    fn fenced_lines_are_context() {
        let src = "+++ Owner questions\n### Q1. Which?\n```\n- a) not an option\n**Answer:** no\n```\n- a) Yes\n- b) No\n\n**Answer:**\n+++\n";
        let q = &questions(src)[0];
        assert_eq!(q.options.len(), 2);
        assert!(q.context.contains("not an option"));
        assert_eq!(q.answer, None);
    }
}
