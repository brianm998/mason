// SPDX-License-Identifier: GPL-3.0-or-later

//! How another pipeline's request is described to the scheduler. Pure: names
//! and numbers in, a rank's inputs out. The ranking itself is the scheduler's.

/// Steps a request is from landing when the caller says nothing of progress.
pub const MAX_PROGRESS: u32 = 6;

/// An outside pipeline's request, ready to rank.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outside {
    pub pipeline: String,
    /// Steps remaining: `6 - progress`.
    pub steps: u32,
    /// Task priority: urgent 0, high 1, normal 2, low 3.
    pub priority: u8,
}

/// Validate the fields of a request. `progress` is how near landing, 0 to 6.
pub fn parse_request(
    pipeline: &str,
    priority: Option<&str>,
    progress: Option<u64>,
) -> Result<Outside, String> {
    let pipeline = pipeline.trim();
    if pipeline.is_empty() || pipeline.len() > 64 || pipeline.chars().any(char::is_control) {
        return Err("`pipeline` must be a short name".into());
    }
    let priority = match priority.map(str::trim) {
        None | Some("") | Some("normal") => 2,
        Some("urgent") => 0,
        Some("high") => 1,
        Some("low") => 3,
        Some(p) => return Err(format!("priority `{p}` is not urgent, high, normal or low")),
    };
    let progress = progress.unwrap_or(0);
    if progress > MAX_PROGRESS as u64 {
        return Err(format!(
            "progress {progress} is not between 0 and {MAX_PROGRESS}"
        ));
    }
    Ok(Outside {
        pipeline: pipeline.to_string(),
        steps: MAX_PROGRESS - progress as u32,
        priority,
    })
}

/// The label a monitor shows: the pipeline's name, then its own label.
pub fn display_label(pipeline: &str, label: &str) -> String {
    format!("{pipeline} {label}")
}

/// Why an outside request waits, in the words `mason resources` prints.
pub fn outside_explain(resource: &str, ahead: usize) -> String {
    format!("waiting for {resource} ({ahead} ahead)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priorities_and_progress_map() {
        let o = parse_request("starpipe", None, None).unwrap();
        assert_eq!((o.steps, o.priority), (6, 2));
        for (name, p) in [("urgent", 0), ("high", 1), ("normal", 2), ("low", 3)] {
            assert_eq!(parse_request("x", Some(name), None).unwrap().priority, p);
        }
        assert_eq!(parse_request("x", None, Some(6)).unwrap().steps, 0);
        assert_eq!(parse_request("x", None, Some(2)).unwrap().steps, 4);
    }

    #[test]
    fn bad_fields_are_refused() {
        assert!(parse_request("", None, None).is_err());
        assert!(parse_request("x", Some("asap"), None).is_err());
        assert!(parse_request("x", None, Some(7)).is_err());
    }

    #[test]
    fn labels() {
        assert_eq!(
            display_label("starpipe", "#483 develop"),
            "starpipe #483 develop"
        );
        assert_eq!(
            format!(
                "{} {}",
                "starpipe #483 develop:",
                outside_explain("llm:claude", 2)
            ),
            "starpipe #483 develop: waiting for llm:claude (2 ahead)"
        );
    }
}
