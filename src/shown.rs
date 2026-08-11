//! One answer, at whatever density the surface has.
//!
//! A route plan is a table on the big panel in VR, one line on the mini panel,
//! a sentence spoken aloud, and a caption. Those are four presentations of the
//! same answer, and the way this goes wrong is that they become four pieces of
//! code that drift until the panel and the voice disagree about what Ward just
//! said.
//!
//! So a capability produces a [`Shown`] and never a string per surface. The
//! surface decides how much room it has.
//!
//! **Three shapes only.** Text, a list, and named values. The temptation is to
//! design the full taxonomy now, before there is anything to project — which is
//! precisely how two earlier attempts spent themselves on a display layer with
//! nothing behind it. These three come from a capability that exists; the
//! fourth arrives with the capability that needs it.
//!
//! What the application uses today is `List`, [`Shown::spoken`] and
//! [`Shown::title`] — the checklist, spoken aloud and drawn on the desktop
//! panel. The rest is marked rather than left as a silent warning:
//! [`Shown::glance`] is the mini panel's projection and gets its consumer with
//! the panel in the headset (#13), and `Text` and `Pairs` are the shapes the
//! next two capabilities will return. All of them are exercised by tests
//! meanwhile, because the projections are easier to agree on once than to
//! reconcile later across four surfaces that already disagree.

/// One line of a list, which may be something already done.
///
/// `done` is here because the first real list is a checklist. It is not a
/// prediction that every list will want it — a list with nothing completable in
/// it simply leaves it false, and if a second list type ever makes that awkward
/// it will be an argument grown from two real capabilities rather than from
/// imagining them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub text: String,
    pub done: bool,
}

/// What a capability has to show.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shown {
    /// A sentence or two, with no structure worth keeping.
    Text(String),
    /// Things in order.
    List { title: String, rows: Vec<Row> },
    /// Named values.
    Pairs {
        title: String,
        pairs: Vec<(String, String)>,
    },
}

/// The longest a single glanced line may be before it is cut.
///
/// The mini panel is read in peripheral vision while flying. A line that wraps
/// there has already failed at its job.
#[allow(dead_code)]
const GLANCE: usize = 48;

#[allow(dead_code)]
fn clipped(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default().trim();

    if line.chars().count() <= GLANCE {
        return line.to_string();
    }

    // Cut on a word so the remnant is still readable, and only fall back to
    // cutting mid-word when there is no space to cut on.
    let mut out: String = line.chars().take(GLANCE).collect();

    if let Some(space) = out.rfind(' ') {
        out.truncate(space);
    }

    format!("{}…", out.trim_end())
}

impl Shown {
    /// A sentence, for speaking aloud and for the caption that follows it.
    ///
    /// Spoken, so it reads as something a person would say rather than as a
    /// table read out loud. A list becomes how far along you are and what is
    /// left, because that is what somebody asking out loud wanted to know.
    pub fn spoken(&self) -> String {
        match self {
            Shown::Text(text) => text.clone(),

            Shown::List { title, rows } => {
                let done = rows.iter().filter(|r| r.done).count();

                match (rows.len(), done) {
                    (0, _) => format!("{title} is empty."),
                    (all, finished) if all == finished => {
                        format!("{title}: all {all} done.")
                    }
                    (all, finished) => {
                        let left: Vec<&str> = rows
                            .iter()
                            .filter(|r| !r.done)
                            .map(|r| r.text.as_str())
                            .collect();

                        format!(
                            "{title}: {finished} of {all} done. Still to do: {}.",
                            left.join(", ")
                        )
                    }
                }
            }

            Shown::Pairs { title, pairs } if pairs.is_empty() => format!("{title} is empty."),

            Shown::Pairs { title, pairs } => {
                let said: Vec<String> = pairs
                    .iter()
                    .map(|(name, value)| format!("{name} {value}"))
                    .collect();

                format!("{title}: {}.", said.join(", "))
            }
        }
    }

    /// One line, for a panel read at a glance while flying.
    ///
    /// Its consumer is the mini panel in the headset (#13). Marked rather than
    /// deleted because the whole argument for one display model is that all
    /// four projections are decided together; agreeing them one surface at a
    /// time is how they end up disagreeing.
    #[allow(dead_code)]
    pub fn glance(&self) -> String {
        match self {
            Shown::Text(text) => clipped(text),

            Shown::List { title, rows } => match rows.len() {
                0 => format!("{title}: empty"),
                all => format!(
                    "{title}: {} of {all}",
                    rows.iter().filter(|r| r.done).count()
                ),
            },

            Shown::Pairs { title, pairs } => match pairs.first() {
                None => format!("{title}: empty"),
                Some((name, value)) => clipped(&format!("{title}: {name} {value}")),
            },
        }
    }

    /// What a surface with room calls this.
    pub fn title(&self) -> Option<&str> {
        match self {
            Shown::Text(_) => None,
            Shown::List { title, .. } | Shown::Pairs { title, .. } => Some(title),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(text: &str, done: bool) -> Row {
        Row {
            text: text.to_string(),
            done,
        }
    }

    fn list(rows: Vec<Row>) -> Shown {
        Shown::List {
            title: "Checklist".to_string(),
            rows,
        }
    }

    #[test]
    fn a_spoken_list_says_how_far_along_and_what_is_left() {
        // What somebody asking out loud wanted to know. Reading every item
        // including the finished ones answers a question nobody asked.
        let shown = list(vec![
            row("buy tritium", true),
            row("refuel at Jameson", false),
            row("swap to the Krait", false),
        ]);

        assert_eq!(
            shown.spoken(),
            "Checklist: 1 of 3 done. Still to do: refuel at Jameson, swap to the Krait."
        );
    }

    #[test]
    fn a_finished_list_does_not_read_out_an_empty_remainder() {
        let shown = list(vec![row("buy tritium", true), row("refuel", true)]);
        assert_eq!(shown.spoken(), "Checklist: all 2 done.");
    }

    #[test]
    fn an_empty_list_says_so_rather_than_counting_to_zero() {
        assert_eq!(list(vec![]).spoken(), "Checklist is empty.");
        assert_eq!(list(vec![]).glance(), "Checklist: empty");
    }

    #[test]
    fn the_same_answer_is_a_sentence_aloud_and_a_count_at_a_glance() {
        // The whole point of one display model. Two renderers would eventually
        // disagree about what Ward just said.
        let shown = list(vec![row("buy tritium", true), row("refuel", false)]);

        assert_eq!(
            shown.spoken(),
            "Checklist: 1 of 2 done. Still to do: refuel."
        );
        assert_eq!(shown.glance(), "Checklist: 1 of 2");
    }

    #[test]
    fn a_glanced_line_does_not_wrap() {
        // The mini panel is read in peripheral vision. A line that wraps there
        // has already failed.
        let long = Shown::Text(
            "Deciat is thirty four light years away and you will need to refuel \
             at least once on the way there."
                .to_string(),
        );

        let glanced = long.glance();

        assert!(
            glanced.chars().count() <= GLANCE + 1,
            "{} characters: {glanced}",
            glanced.chars().count()
        );
        assert!(glanced.ends_with('…'), "no sign it was cut: {glanced}");
        assert!(!glanced.contains("refuel"), "not cut at all: {glanced}");
    }

    #[test]
    fn a_short_line_is_left_whole_and_unmarked() {
        let shown = Shown::Text("Ward 0.1.0".to_string());
        assert_eq!(shown.glance(), "Ward 0.1.0");
        assert_eq!(shown.spoken(), "Ward 0.1.0");
    }

    #[test]
    fn a_glanced_line_is_cut_between_words() {
        // Cutting mid-word leaves a fragment that reads as a different, shorter
        // word, which at a glance is worse than simply showing less.
        let text = "alpha bravo charlie delta echo foxtrot golf hotel india juliet";
        let glanced = Shown::Text(text.to_string()).glance();

        let stem = glanced.trim_end_matches('…');

        assert!(text.starts_with(stem), "not the start of it: {stem:?}");
        assert!(
            text[stem.len()..].starts_with(' '),
            "cut mid-word: {stem:?}"
        );
    }

    #[test]
    fn only_a_titled_shape_has_a_title() {
        assert_eq!(list(vec![]).title(), Some("Checklist"));
        assert_eq!(Shown::Text("hello".into()).title(), None);
    }

    #[test]
    fn named_values_read_as_a_sentence() {
        let shown = Shown::Pairs {
            title: "Your ship".to_string(),
            pairs: vec![
                ("jump range".to_string(), "is 42 light years".to_string()),
                ("fuel".to_string(), "is at 60 percent".to_string()),
            ],
        };

        assert_eq!(
            shown.spoken(),
            "Your ship: jump range is 42 light years, fuel is at 60 percent."
        );
        assert_eq!(shown.glance(), "Your ship: jump range is 42 light years");
    }
}
