//! Bibliography view model for the library pane (feature `rag`).
//!
//! The pane shows one row per BibTeX/BibLaTeX entry as **Au–Yr–Ti–Source**
//! (decision: *the right bar reads the ragrig-provided `.bib` file and shows
//! the entries*). [`grobid_bibtex::Collection`] is the parser, but its
//! [`CiteEntry`](grobid_bibtex::Collection::cite_entries) carries only key
//! and file, so the rows are built from the raw [`biblatex::Entry`] values
//! [`Collection::iter`] yields (upstream note: US-12).
//!
//! Everything here is pure and unit-testable: [`LibraryState`] describes
//! what the pane should render (including the missing, ambiguous and corrupt
//! cases), [`LibraryView::filter`] ranks rows for the search box, and the
//! author/year/source fallback chains live in small, testable functions.

use std::path::{Path, PathBuf};

use biblatex::{ChunksExt, Entry, Person};
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use grobid_bibtex::Collection;

use super::LibrarySettings;

/// One library row: authors, year, title and source of a bibliography entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryEntry {
    /// Canonical citation key (what gets inserted into the document).
    pub key: String,
    /// Short author string, e.g. `Caspar`, `Caspar & Möller`, `Caspar et al.`.
    pub authors: String,
    /// Publication year; empty when the entry has none.
    pub year: String,
    /// Title; empty when the entry has none.
    pub title: String,
    /// Venue: journal, book title, series or publisher; empty when unknown.
    pub source: String,
    /// The document recorded in the entry (`file`/`pdf`), when it points
    /// somewhere.
    pub file: Option<PathBuf>,
    /// Pre-computed, lower-cased search text (kept private to the filter).
    search: String,
}

impl LibraryEntry {
    /// Fields of one entry, with `search` derived.
    fn new(
        key: String,
        authors: String,
        year: String,
        title: String,
        source: String,
        file: Option<PathBuf>,
    ) -> Self {
        let mut entry = Self {
            key,
            authors,
            year,
            title,
            source,
            file,
            search: String::new(),
        };
        entry.search = entry.build_search();
        entry
    }

    fn build_search(&self) -> String {
        format!(
            "{} {} {} {} {}",
            self.authors, self.year, self.title, self.source, self.key
        )
        .to_lowercase()
    }

    /// One-line citation for tooltips, e.g.
    /// `Caspar et al. (2021). Title. Source.` — parts that are unknown are
    /// left out, never rendered as `().`.
    pub fn citation_line(&self) -> String {
        let mut line = String::new();
        if !self.authors.is_empty() {
            line.push_str(&self.authors);
        }
        if !self.year.is_empty() {
            if !line.is_empty() {
                line.push(' ');
            }
            line.push('(');
            line.push_str(&self.year);
            line.push(')');
        }
        if !self.title.is_empty() {
            if !line.is_empty() {
                line.push_str(". ");
            }
            line.push_str(&self.title);
        }
        if !self.source.is_empty() {
            if !line.is_empty() {
                line.push_str(". ");
            }
            line.push_str(&self.source);
        }
        line
    }
}

/// The parsed bibliography, ready for the list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LibraryView {
    /// Rows, sorted by author → year → title.
    pub entries: Vec<LibraryEntry>,
}

impl LibraryView {
    /// Row indices matching `query`, best match first.
    ///
    /// An empty query keeps the bibliography order. Matching is fuzzy over
    /// authors, year, title, source and key (case-insensitive) — the same
    /// matcher the command palette uses.
    pub fn filter(&self, query: &str) -> Vec<usize> {
        let query = query.trim();
        if query.is_empty() {
            return (0..self.entries.len()).collect();
        }
        let matcher = SkimMatcherV2::default().ignore_case();
        let mut scored: Vec<(i64, usize)> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                matcher
                    .fuzzy_match(&entry.search, query)
                    .map(|score| (score, index))
            })
            .collect();
        // Stable tie-break on the row order keeps results predictable.
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        scored.into_iter().map(|(_, index)| index).collect()
    }
}

/// What the library pane renders.
#[derive(Clone, Debug, PartialEq)]
pub enum LibraryState {
    /// Nothing to read; `reason` is user-presentable and points at the fix.
    Missing {
        /// Why there is no bibliography (no workspace, nothing found, …).
        reason: String,
    },
    /// Several bibliographies were found; the user picks one (Corpus menu).
    Ambiguous {
        /// The candidates found by [`LibrarySettings::resolve`].
        candidates: Vec<PathBuf>,
    },
    /// The bibliography exists but does not parse.
    Corrupt {
        /// The file that failed.
        path: PathBuf,
        /// Parser error message.
        error: String,
    },
    /// Loaded and ready.
    Ready {
        /// The file that was read.
        path: PathBuf,
        /// Whether the path was discovered rather than configured.
        discovered: bool,
        /// The rows.
        view: LibraryView,
    },
}

/// Resolve and load the bibliography for `workspace`.
///
/// This is the single place that decides between the pane's states, so the
/// UI never has to re-derive them.
pub fn load_state(workspace: Option<&Path>, settings: &LibrarySettings) -> LibraryState {
    let Some(workspace) = workspace else {
        return LibraryState::Missing {
            reason: "no RAG workspace — choose a folder in the component bar".to_string(),
        };
    };
    let paths = settings.resolve(workspace);
    let Some(bib) = paths.bibtex else {
        if !paths.candidates.is_empty() {
            return LibraryState::Ambiguous {
                candidates: paths.candidates,
            };
        }
        return LibraryState::Missing {
            reason: format!(
                "no bibliography found in {} — set one in the Corpus menu",
                workspace.display()
            ),
        };
    };
    if !bib.is_file() {
        return LibraryState::Missing {
            reason: format!(
                "no bibliography at {} — set one in the Corpus menu",
                bib.display()
            ),
        };
    }
    match load(&bib) {
        Ok(view) => LibraryState::Ready {
            path: bib,
            discovered: paths.discovered,
            view,
        },
        Err(error) => LibraryState::Corrupt { path: bib, error },
    }
}

/// Parse a bibliography file into the row view.
pub fn load(path: &Path) -> Result<LibraryView, String> {
    let collection = Collection::load(path).map_err(|error| error.to_string())?;
    Ok(view_of(&collection))
}

fn view_of(collection: &Collection) -> LibraryView {
    let mut entries: Vec<LibraryEntry> = collection.iter().map(entry_of).collect();
    // Bibliography order is file order (merges append); author → year → title
    // is stable, findable and independent of how the file was assembled.
    // Entries without an author go last — they are the exception, and a list
    // that starts with them reads like it is unsorted.
    entries.sort_by_cached_key(|entry| {
        (
            entry.authors.is_empty(),
            entry.authors.to_lowercase(),
            entry.year.clone(),
            entry.title.to_lowercase(),
        )
    });
    LibraryView { entries }
}

/// Build one row from a bibliography entry.
fn entry_of(entry: &Entry) -> LibraryEntry {
    LibraryEntry::new(
        entry.key.clone(),
        authors_of(entry),
        year_of(entry),
        field_text(entry, &["title"]),
        source_of(entry),
        file_of(entry),
    )
}

/// Short author string; falls back to editors (collections have no authors).
fn authors_of(entry: &Entry) -> String {
    let people = entry
        .get_as::<Vec<Person>>("author")
        .or_else(|_| entry.get_as::<Vec<Person>>("editor"))
        .unwrap_or_default();
    match people.as_slice() {
        [] => String::new(),
        [one] => one.name.clone(),
        [first, second] => format!("{} & {}", first.name, second.name),
        [first, ..] => format!("{} et al.", first.name),
    }
}

/// Publication year: BibTeX `year`, else the year part of BibLaTeX `date`.
fn year_of(entry: &Entry) -> String {
    let year = field_text(entry, &["year"]);
    if !year.is_empty() {
        return year;
    }
    let date = field_text(entry, &["date"]);
    let digits: String = date.chars().take_while(char::is_ascii_digit).collect();
    digits
}

/// Venue chain: journal → book title → series → publisher → institution.
fn source_of(entry: &Entry) -> String {
    field_text(
        entry,
        &[
            "journaltitle",
            "journal",
            "booktitle",
            "series",
            "publisher",
            "institution",
        ],
    )
}

fn file_of(entry: &Entry) -> Option<PathBuf> {
    let text = field_text(entry, &["file", "pdf"]);
    if text.is_empty() {
        None
    } else {
        Some(PathBuf::from(text))
    }
}

/// Resolve the document of a bibliography entry within the corpus `folder`.
///
/// This is the same cascade the citation attach uses (decision D5): the
/// entry's recorded `file`/`pdf` field first — resolved against the corpus
/// folder when it is relative — then a corpus file named after the key
/// (`<key> - …`, the ragrig rename convention). `scan` caches the folder
/// listing across calls, so opening many rows pays for one `read_dir`, not one
/// per row.
///
/// Errors carry a user-presentable reason and never guess: a missing document
/// and an ambiguous keyed name are reported, not silently resolved — opening
/// (or citing) the wrong paper would be worse than a miss.
pub fn resolve_document(
    key: &str,
    recorded: Option<&Path>,
    folder: &Path,
    scan: &mut Option<Vec<(String, PathBuf)>>,
) -> Result<PathBuf, String> {
    // 1. The document recorded in the entry's `file` field.
    if let Some(recorded) = recorded {
        let in_corpus = if recorded.is_absolute() {
            recorded.to_path_buf()
        } else {
            folder.join(recorded)
        };
        if in_corpus.is_file() {
            return Ok(in_corpus);
        }
        if recorded.is_file() {
            return Ok(recorded.to_path_buf());
        }
    }

    // 2. A corpus file named after the key (`<key> - …`).
    let files = scan.get_or_insert_with(|| corpus_files(folder));
    let mut matches = candidates(key, files);
    match matches.len() {
        0 => Err(if recorded.is_some() {
            "linked document not found".to_string()
        } else {
            "no document linked".to_string()
        }),
        1 => Ok(matches.pop().expect("one candidate")),
        count => Err(format!("{count} candidate files (ambiguous)")),
    }
}

/// Corpus files as `(lowercase stem, path)` pairs.
fn corpus_files(folder: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| {
            entry
                .file_type()
                .map(|kind| kind.is_file())
                .unwrap_or(false)
        })
        .filter_map(|entry| {
            let path = entry.path();
            let stem = path.file_stem()?.to_str()?.to_lowercase();
            Some((stem, path))
        })
        .collect()
}

/// Files whose name matches the key exactly or starts with `<key> - `.
fn candidates(key: &str, files: &[(String, PathBuf)]) -> Vec<PathBuf> {
    let lowercase = key.to_lowercase();
    let prefix = format!("{lowercase} - ");
    let mut matches: Vec<PathBuf> = files
        .iter()
        .filter(|(stem, _)| *stem == lowercase || stem.starts_with(&prefix))
        .map(|(_, path)| path.clone())
        .collect();
    matches.sort();
    matches.dedup();
    matches
}

/// Short file name for labels and user-facing messages.
pub(crate) fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// First non-empty field among `names`, as display text.
fn field_text(entry: &Entry, names: &[&str]) -> String {
    for name in names {
        if let Some(chunks) = entry.get(name) {
            let text = normalize_text(&chunks.format_verbatim());
            if !text.is_empty() {
                return text;
            }
        }
    }
    String::new()
}

/// Collapse whitespace and drop BibTeX protection braces for display
/// (`{T}itle` renders as `Title`, multi-line values as one line).
fn normalize_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_was_space = false;
    for c in text.chars() {
        match c {
            '{' | '}' => {}
            c if c.is_whitespace() => {
                if !last_was_space && !out.is_empty() {
                    out.push(' ');
                    last_was_space = true;
                }
            }
            c => {
                out.push(c);
                last_was_space = false;
            }
        }
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    fn settings_for(bib: &Path) -> LibrarySettings {
        LibrarySettings {
            folder: None,
            bibtex: Some(bib.to_path_buf()),
        }
    }

    const SAMPLE: &str = r#"
@article{Caspar2021,
  author = {Caspar, Martin and M{\"o}ller, Sebastian},
  title = {The {C}ell model for continuous variables},
  journaltitle = {Ecological Economics},
  date = {2021-05-01},
  file = {Caspar2021 - Möller - The Cell model.pdf}
}

@inproceedings{Kahle2000,
  author = {Kahle, Brewster},
  title = {A dataset},
  booktitle = {Proceedings of the Fake Conference},
  year = {2000}
}

@book{Anonymous,
  title = {Unattributed book},
  publisher = {Nonexistent Press}
}
"#;

    fn sample_view() -> LibraryView {
        let dir = tempfile::tempdir().unwrap();
        let bib = write(dir.path(), "library.bib", SAMPLE);
        load(&bib).unwrap()
    }

    #[test]
    fn rows_carry_author_year_title_and_source() {
        let view = sample_view();
        assert_eq!(view.entries.len(), 3);
        // Sorted by author, author-less entries last: Caspar, Kahle, Anonymous.
        let caspar = &view.entries[0];
        assert_eq!(caspar.key, "Caspar2021");
        assert_eq!(caspar.authors, "Caspar & Möller");
        assert_eq!(caspar.year, "2021");
        assert_eq!(caspar.title, "The Cell model for continuous variables");
        assert_eq!(caspar.source, "Ecological Economics");
        assert!(caspar.file.is_some(), "the file field is kept");

        let kahle = &view.entries[1];
        assert_eq!(kahle.authors, "Kahle");
        assert_eq!(kahle.year, "2000");
        assert_eq!(kahle.source, "Proceedings of the Fake Conference");

        let anonymous = &view.entries[2];
        assert_eq!(anonymous.authors, "");
        assert_eq!(anonymous.year, "");
        assert_eq!(anonymous.source, "Nonexistent Press");
    }

    #[test]
    fn author_lists_fall_back_to_the_first_author_and_editors() {
        let mut view = sample_view();
        let three = LibraryEntry::new(
            "K".into(),
            "A & B".into(),
            String::new(),
            String::new(),
            String::new(),
            None,
        );
        assert_eq!(three.authors, "A & B");
        view.entries.push(three);

        let dir = tempfile::tempdir().unwrap();
        let bib = write(
            dir.path(),
            "edited.bib",
            "@collection{Ed2020,\n author = {One, A. and Two, B. and Three, C.},\n title = {T},\n year = {2020}\n}\n",
        );
        let view = load(&bib).unwrap();
        assert_eq!(view.entries[0].authors, "One et al.");
    }

    #[test]
    fn citation_lines_skip_unknown_parts() {
        let view = sample_view();
        assert_eq!(
            view.entries[0].citation_line(),
            "Caspar & Möller (2021). The Cell model for continuous variables. Ecological Economics"
        );
        assert_eq!(
            view.entries[2].citation_line(),
            "Unattributed book. Nonexistent Press"
        );
    }

    #[test]
    fn filter_matches_fields_and_ranks_best_first() {
        let view = sample_view();
        assert_eq!(
            view.filter(""),
            vec![0, 1, 2],
            "an empty query keeps the row order"
        );
        assert_eq!(view.filter("kahle"), vec![1]);
        assert_eq!(view.filter("2000"), vec![1]);
        assert_eq!(view.filter("ecological"), vec![0]);
        assert_eq!(
            view.filter("Caspar2021"),
            vec![0],
            "the key is searchable too, case-insensitively"
        );
        assert!(view.filter("zzz").is_empty());
    }

    #[test]
    fn load_state_reports_missing_ambiguous_and_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path();

        // Nothing found at all.
        let state = load_state(Some(workspace), &LibrarySettings::default());
        assert!(matches!(state, LibraryState::Missing { .. }));

        // No workspace.
        let state = load_state(None, &LibrarySettings::default());
        let LibraryState::Missing { reason } = state else {
            panic!("expected missing");
        };
        assert!(reason.contains("workspace"), "unexpected reason: {reason}");

        // Two bibliographies: the pane must ask, not guess.
        write(workspace, "CEH.bib", SAMPLE);
        write(workspace, "CEH_2.bib", SAMPLE);
        let state = load_state(Some(workspace), &LibrarySettings::default());
        let LibraryState::Ambiguous { candidates } = state else {
            panic!("expected ambiguity");
        };
        assert_eq!(candidates.len(), 2);

        // A configured but missing file names the path.
        let state = load_state(
            Some(workspace),
            &settings_for(&workspace.join("nowhere.bib")),
        );
        let LibraryState::Missing { reason } = state else {
            panic!("expected missing");
        };
        assert!(
            reason.contains("nowhere.bib"),
            "unexpected reason: {reason}"
        );
    }

    #[test]
    fn load_state_reports_a_corrupt_bibliography() {
        let dir = tempfile::tempdir().unwrap();
        let bib = write(
            dir.path(),
            "broken.bib",
            "@article{key,\n  title = {unterminated\n",
        );
        let state = load_state(Some(dir.path()), &settings_for(&bib));
        let LibraryState::Corrupt { path, error } = state else {
            panic!("expected corrupt, got {state:?}");
        };
        assert_eq!(path, bib);
        assert!(!error.is_empty(), "the parser error is surfaced");
    }

    #[test]
    fn load_state_reports_a_ready_view_with_the_discovery_flag() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "CEH.bib", SAMPLE);
        let state = load_state(Some(dir.path()), &LibrarySettings::default());
        let LibraryState::Ready {
            path,
            discovered,
            view,
        } = state
        else {
            panic!("expected ready, got {state:?}");
        };
        assert!(path.ends_with("CEH.bib"));
        assert!(discovered, "a lone workspace bibliography is discovered");
        assert_eq!(view.entries.len(), 3);
    }

    #[test]
    fn normalize_text_strips_protection_and_collapses_whitespace() {
        assert_eq!(normalize_text("The {C}ell model"), "The Cell model");
        assert_eq!(normalize_text("A\n   title\n"), "A title");
        assert_eq!(normalize_text("  "), "");
        assert_eq!(normalize_text("C++ {R}ocks"), "C++ Rocks");
    }

    #[test]
    fn resolve_document_prefers_the_recorded_file_field() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path();
        let paper = write(folder, "paper.pdf", "%PDF-1.4");
        let mut scan = None;

        // A relative link resolves against the corpus folder.
        let resolved =
            resolve_document("Smith2024", Some(Path::new("paper.pdf")), folder, &mut scan);
        assert_eq!(resolved.as_deref(), Ok(paper.as_path()));

        // An absolute link is taken as it is (the folder is irrelevant).
        let resolved = resolve_document("Smith2024", Some(&paper), folder, &mut scan);
        assert_eq!(resolved.as_deref(), Ok(paper.as_path()));
    }

    #[test]
    fn resolve_document_falls_back_to_a_file_named_after_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path();
        // The ragrig rename convention: `<key> - …`.
        let keyed = write(
            folder,
            "Smith2024 - Jane Smith - A Paper - 2024.pdf",
            "%PDF",
        );
        // A near miss (a different key with the same prefix) must not match.
        write(folder, "Smith2024x - other.pdf", "%PDF");
        let mut scan = None;

        // No recorded file: only the keyed name can answer.
        assert_eq!(
            resolve_document("Smith2024", None, folder, &mut scan).as_deref(),
            Ok(keyed.as_path())
        );
        // Case-insensitive, and a recorded file whose link is dead does not
        // shadow the keyed name.
        assert_eq!(
            resolve_document("smith2024", Some(Path::new("gone.pdf")), folder, &mut scan,)
                .as_deref(),
            Ok(keyed.as_path())
        );
    }

    #[test]
    fn resolve_document_reports_missing_and_ambiguous_without_guessing() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path();
        write(folder, "A2024 - one.pdf", "%PDF");
        write(folder, "A2024 - two.pdf", "%PDF");
        let mut scan = None;

        // Two keyed candidates: never pick one.
        let ambiguous = resolve_document("A2024", None, folder, &mut scan).unwrap_err();
        assert!(ambiguous.contains("ambiguous"), "{ambiguous}");

        // An entry with no link and no keyed file.
        let none = resolve_document("B2024", None, folder, &mut scan).unwrap_err();
        assert!(none.contains("no document linked"), "{none}");

        // A recorded link that resolves nowhere.
        let broken =
            resolve_document("C2024", Some(Path::new("gone.pdf")), folder, &mut scan).unwrap_err();
        assert!(broken.contains("linked document not found"), "{broken}");
    }
}
