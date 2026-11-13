//! LLM-209: `:step` moves tasks between the steps of their parent's order.
//! The app works out the parent's whole new order here and sends it in one
//! write, so several marked tasks land in one step together.

use gpui_shared_string::SharedString;

use crate::model::data::Issue;

/// Where `:step` puts the tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepTarget {
    /// Into this step, 1 first.
    Step(u32),
    /// Into a new last step.
    New,
    /// Out of the order.
    None,
}

impl StepTarget {
    /// Reads the argument of `:step`: a number, `new` or `none`.
    pub fn parse(arg: &str) -> Option<StepTarget> {
        match arg.trim().to_lowercase().as_str() {
            "new" | "last" => Some(StepTarget::New),
            "none" | "off" | "clear" => Some(StepTarget::None),
            n => n.parse().ok().filter(|n| *n > 0).map(StepTarget::Step),
        }
    }
}

/// The steps of `parent` from its live sub-issues, first step first. Each
/// step lists its tasks' keys in key order.
pub fn steps_of<'i>(
    parent: &str,
    issues: impl Iterator<Item = &'i Issue>,
) -> Vec<Vec<SharedString>> {
    let mut ordered: Vec<(u32, &Issue)> = issues
        .filter(|i| !i.deleted && i.parent.as_deref() == Some(parent))
        .filter_map(|i| i.order_step.map(|s| (s, i)))
        .collect();
    ordered.sort_by(|a, b| (a.0, key_order(&a.1.key)).cmp(&(b.0, key_order(&b.1.key))));
    let mut steps: Vec<Vec<SharedString>> = Vec::new();
    let mut last = None;
    for (step, issue) in ordered {
        if last != Some(step) {
            steps.push(Vec::new());
            last = Some(step);
        }
        if let Some(current) = steps.last_mut() {
            current.push(issue.key.clone());
        }
    }
    steps
}

fn key_order(key: &str) -> (&str, u64) {
    match key.rsplit_once('-') {
        Some((team, n)) => (team, n.parse().unwrap_or(u64::MAX)),
        None => (key, 0),
    }
}

/// The order after `moved` go to `target`. Step numbers mean the steps as
/// they are now, so `:step 3` joins the tasks the list shows in step 3,
/// also when a moved task leaves an earlier step empty. Empty steps then
/// drop out, and the server numbers the rest 1..n again.
///
/// The error is the message to show. Nothing changes then.
pub fn move_to(
    steps: &[Vec<SharedString>],
    moved: &[SharedString],
    target: StepTarget,
    parent: &str,
) -> Result<Vec<Vec<SharedString>>, String> {
    let mut steps: Vec<Vec<SharedString>> = steps
        .iter()
        .map(|step| {
            step.iter()
                .filter(|k| !moved.contains(k))
                .cloned()
                .collect()
        })
        .collect();
    match target {
        StepTarget::Step(n) if (n as usize) <= steps.len() => {
            steps[n as usize - 1].extend(moved.iter().cloned());
        }
        StepTarget::Step(n) if n as usize == steps.len() + 1 => steps.push(moved.to_vec()),
        StepTarget::Step(_) => {
            let count = steps.len();
            return Err(match count {
                0 => format!("{parent} has no order yet. Use :step 1 or :step new"),
                1 => format!("{parent} has 1 step. Use :step 1, :step 2 or :step new"),
                _ => format!(
                    "{parent} has {count} steps. Use :step 1 to :step {}, or :step new",
                    count + 1
                ),
            });
        }
        StepTarget::New => steps.push(moved.to_vec()),
        StepTarget::None => {}
    }
    steps.retain(|step| !step.is_empty());
    Ok(steps)
}

/// The step of `key` in `steps`, 1 first.
pub fn step_of(steps: &[Vec<SharedString>], key: &str) -> Option<u32> {
    steps
        .iter()
        .position(|step| step.iter().any(|k| k == key))
        .map(|ix| ix as u32 + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(steps: &[&[&str]]) -> Vec<Vec<SharedString>> {
        steps
            .iter()
            .map(|s| {
                s.iter()
                    .map(|k| SharedString::from(k.to_string()))
                    .collect()
            })
            .collect()
    }

    fn moved(list: &[&str]) -> Vec<SharedString> {
        list.iter()
            .map(|k| SharedString::from(k.to_string()))
            .collect()
    }

    #[test]
    fn parses_a_number_new_and_none() {
        assert_eq!(StepTarget::parse("3"), Some(StepTarget::Step(3)));
        assert_eq!(StepTarget::parse(" NEW "), Some(StepTarget::New));
        assert_eq!(StepTarget::parse("none"), Some(StepTarget::None));
        assert_eq!(StepTarget::parse("0"), None);
        assert_eq!(StepTarget::parse(""), None);
        assert_eq!(StepTarget::parse("two"), None);
    }

    #[test]
    fn tasks_join_a_step_as_the_list_shows_it() {
        let now = keys(&[&["A-1"], &["A-2"], &["A-3", "A-4"]]);
        // A-2 leaves step 2 empty, and still lands in what is step 3 now.
        assert_eq!(
            move_to(&now, &moved(&["A-2"]), StepTarget::Step(3), "A-9").unwrap(),
            keys(&[&["A-1"], &["A-3", "A-4", "A-2"]])
        );
        // Several tasks land in one step together.
        assert_eq!(
            move_to(&now, &moved(&["A-1", "A-5"]), StepTarget::Step(2), "A-9").unwrap(),
            keys(&[&["A-2", "A-1", "A-5"], &["A-3", "A-4"]])
        );
    }

    #[test]
    fn new_adds_a_last_step_and_none_takes_tasks_out() {
        let now = keys(&[&["A-1"], &["A-2", "A-3"]]);
        assert_eq!(
            move_to(&now, &moved(&["A-1", "A-2"]), StepTarget::New, "A-9").unwrap(),
            keys(&[&["A-3"], &["A-1", "A-2"]])
        );
        assert_eq!(
            move_to(&now, &moved(&["A-3"]), StepTarget::Step(3), "A-9").unwrap(),
            keys(&[&["A-1"], &["A-2"], &["A-3"]])
        );
        assert_eq!(
            move_to(&now, &moved(&["A-1"]), StepTarget::None, "A-9").unwrap(),
            keys(&[&["A-2", "A-3"]])
        );
    }

    #[test]
    fn a_step_past_the_new_last_one_is_refused() {
        let now = keys(&[&["A-1"], &["A-2"]]);
        assert_eq!(
            move_to(&now, &moved(&["A-3"]), StepTarget::Step(4), "A-9").unwrap_err(),
            "A-9 has 2 steps. Use :step 1 to :step 3, or :step new"
        );
        assert_eq!(
            move_to(&[], &moved(&["A-3"]), StepTarget::Step(2), "A-9").unwrap_err(),
            "A-9 has no order yet. Use :step 1 or :step new"
        );
        assert_eq!(
            move_to(&[], &moved(&["A-3"]), StepTarget::Step(1), "A-9").unwrap(),
            keys(&[&["A-3"]])
        );
    }
}
