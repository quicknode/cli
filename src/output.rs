//! Output rendering.
//!
//! Five formats, selected by the global `--format/-o` flag. When the flag and
//! the config file both leave the format unset, the default is TTY-aware:
//! `table` when stdout is a terminal (interactive use), `toon` otherwise
//! (piped / agent invocations). See [`crate::context::GlobalArgs::resolve_output`].
//!
//! - `table`: comfy-table with UTF-8 borders for humans on a TTY.
//! - `json`:  pretty-printed JSON via serde_json.
//! - `yaml`:  YAML via serde_yml — same shape as JSON.
//! - `md`:    GitHub-flavored markdown tables (same data, markdown borders).
//! - `toon`:  Token-Oriented Object Notation (toon-format crate, default opts).
//!
//! The `Render` trait is only used for `table` and `md`. The other three
//! formats serialize directly off `Serialize`.
//!
//! Color is suppressed when any of: `--no-color`, `NO_COLOR` env, `TERM=dumb`,
//! stdout is not a TTY, or the format is anything other than `table`.
//!
//! State-change confirmations go to stderr through [`OutputCtx::note`]; only
//! `--quiet` suppresses them.

use std::io::{IsTerminal, Write};

use clap::ValueEnum;
use comfy_table::{Attribute, Cell, CellAlignment, ContentArrangement, Table};
use serde::Serialize;

use crate::errors::CliError;

/// Output format selected by `--format/-o`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default, Serialize, serde::Deserialize)]
#[value(rename_all = "lower")]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// Pretty UTF-8 tables for humans.
    #[default]
    Table,
    /// Pretty-printed JSON.
    Json,
    /// YAML (same shape as JSON).
    Yaml,
    /// GitHub-flavored markdown tables.
    Md,
    /// Token-Oriented Object Notation.
    Toon,
}

impl Format {
    /// True when the format is a structured/serialized one (json/yaml/toon),
    /// as opposed to a human-rendered table or markdown.
    ///
    /// Used by single-value commands (`stream enabled-count`, etc.) to decide
    /// between emitting the structured response and printing the bare value.
    pub fn is_structured(self) -> bool {
        matches!(self, Self::Json | Self::Yaml | Self::Toon)
    }
}

/// Carries the user's output preferences and TTY state.
#[derive(Debug, Clone, Copy)]
pub struct OutputCtx {
    pub format: Format,
    pub color: bool,
    pub quiet: bool,
    pub verbose: bool,
    /// `--wide` was passed; list-style table/md renderers should show extra
    /// columns. Has no effect on json/yaml/toon (which always include
    /// everything from the SDK response).
    pub wide: bool,
    pub stdout_is_tty: bool,
}

impl OutputCtx {
    /// Detect from environment + CLI flags.
    pub fn detect(format: Format, no_color: bool, quiet: bool, verbose: bool, wide: bool) -> Self {
        Self::detect_with(
            format,
            no_color,
            quiet,
            verbose,
            wide,
            std::io::stdout().is_terminal(),
            std::env::var_os("NO_COLOR"),
            std::env::var("TERM").ok(),
        )
    }

    /// Pure version of [`detect`] for testing.
    #[allow(clippy::too_many_arguments)] // test injection seam; pass env values in
    pub fn detect_with(
        format: Format,
        no_color: bool,
        quiet: bool,
        verbose: bool,
        wide: bool,
        stdout_is_tty: bool,
        no_color_env: Option<std::ffi::OsString>,
        term_env: Option<String>,
    ) -> Self {
        let color = !no_color
            && format == Format::Table
            && stdout_is_tty
            && no_color_env.map_or(true, |v| v.is_empty())
            && term_env.map_or(true, |t| t != "dumb");
        Self {
            format,
            color,
            quiet,
            verbose,
            wide,
            stdout_is_tty,
        }
    }

    /// Writes a state-change note to stderr (e.g. "✓ Paused endpoint ep-123").
    /// Suppressed under `--quiet`.
    pub fn note(&self, message: &str) {
        if self.quiet {
            return;
        }
        let _ = writeln!(std::io::stderr(), "{message}");
    }
}

/// Trait every printable response implements.
pub trait Render: Serialize {
    /// Render a human-facing representation to `w`. Implementations should use
    /// [`new_table`] (which picks the right preset for the current `ctx.format`)
    /// for tabular data so markdown and table formats share one code path.
    fn render_table(&self, w: &mut dyn Write, ctx: &OutputCtx) -> std::io::Result<()>;

    /// Override only when the default `Serialize` shape produces TOON output
    /// that can't tabularize — typically a `Vec<struct>` field on each row of a
    /// list response. The returned [`serde_json::Value`] is used **only** for
    /// TOON encoding (JSON/YAML stay lossless via the default `Serialize` impl).
    fn toon_projection(&self) -> Option<serde_json::Value> {
        None
    }
}

/// Top-level emit: serializes through the chosen format.
pub fn emit<T: Render>(ctx: &OutputCtx, value: &T) -> Result<(), CliError> {
    let mut out = std::io::stdout().lock();
    match ctx.format {
        Format::Json => {
            serde_json::to_writer_pretty(&mut out, value)?;
            out.write_all(b"\n")?;
        }
        Format::Yaml => {
            serde_yml::to_writer(&mut out, value).map_err(|e| CliError::Format(e.to_string()))?;
        }
        Format::Toon => {
            // TOON tabularizes uniform arrays of primitives (one CSV row per
            // record) but bails to a verbose per-object form as soon as a row
            // has an Array or Object field. Two interventions, TOON-only:
            //   1. Render::toon_projection lets a view project Vec<struct>
            //      fields down to primitives (lossless JSON/YAML preserved).
            //   2. flatten_primitive_arrays joins primitive-only arrays inside
            //      array-of-objects so the tabular check passes.
            let mut json = match value.toon_projection() {
                Some(v) => v,
                None => serde_json::to_value(value).map_err(|e| CliError::Format(e.to_string()))?,
            };
            flatten_primitive_arrays(&mut json);
            let s =
                toon_format::encode_default(&json).map_err(|e| CliError::Format(e.to_string()))?;
            out.write_all(s.as_bytes())?;
            if !s.ends_with('\n') {
                out.write_all(b"\n")?;
            }
        }
        Format::Table | Format::Md => {
            value.render_table(&mut out, ctx)?;
        }
    }
    Ok(())
}

/// Walks `value` and, for every object that lives inside an array, replaces
/// any field whose value is a primitive-only array with a single string of the
/// comma-joined elements. This unlocks TOON's tabular form for the common case
/// where a row has e.g. `tags: ["prod","staging"]`.
///
/// Scope is deliberately narrow: only fields *inside array elements* are
/// joined. A top-level `Value::Array` of primitives is left alone — TOON
/// already renders that form compactly via its own primitive-array rule.
/// Non-primitive arrays (arrays of objects, arrays of arrays) are also left
/// alone; those need a [`Render::toon_projection`] to summarize.
pub(crate) fn flatten_primitive_arrays(value: &mut serde_json::Value) {
    use serde_json::Value;
    match value {
        Value::Array(arr) => {
            for el in arr.iter_mut() {
                if let Value::Object(obj) = el {
                    for v in obj.values_mut() {
                        if let Value::Array(inner) = v {
                            // Empty arrays count: an empty `Vec<EndpointTag>`
                            // still blocks tabular until we collapse it.
                            if inner.iter().all(is_json_primitive) {
                                *v = Value::String(join_primitives(inner));
                                continue;
                            }
                        }
                        flatten_primitive_arrays(v);
                    }
                } else {
                    flatten_primitive_arrays(el);
                }
            }
        }
        Value::Object(obj) => {
            for v in obj.values_mut() {
                flatten_primitive_arrays(v);
            }
        }
        _ => {}
    }
}

fn is_json_primitive(v: &serde_json::Value) -> bool {
    use serde_json::Value;
    matches!(
        v,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
    )
}

fn join_primitives(arr: &[serde_json::Value]) -> String {
    use serde_json::Value;
    arr.iter()
        .map(|v| match v {
            Value::Null => String::new(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => n.to_string(),
            Value::String(s) => s.clone(),
            _ => unreachable!("guarded by is_json_primitive"),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Builds a fresh table.
///
/// - For `Format::Md` we use the ASCII markdown preset (pipes + dashes) so
///   the output can be pasted into a doc.
/// - For `Format::Table` we use a borderless, docker-/kubectl-style layout:
///   no row separators, no outer frame, columns separated by two spaces.
///   Headers get [`set_header_bold`] applied at the call site.
pub fn new_table(ctx: &OutputCtx) -> Table {
    let mut t = Table::new();
    t.set_content_arrangement(ContentArrangement::Dynamic);
    if ctx.format == Format::Md {
        t.load_preset(comfy_table::presets::ASCII_MARKDOWN);
        return t;
    }
    t.load_preset(comfy_table::presets::NOTHING);
    t
}

/// Sets the table header docker/kubectl-style: ALL-CAPS bold cells (bold only
/// when colors are active — otherwise we'd dump raw ANSI escapes into piped
/// output). Callers should pass already-uppercased strings.
///
/// Also configures two-space right padding on every column; with the
/// borderless preset that gap is the only thing separating columns.
pub fn set_header_bold<I, T>(table: &mut Table, ctx: &OutputCtx, columns: I)
where
    I: IntoIterator<Item = T>,
    T: Into<String>,
{
    let cells = columns.into_iter().map(|c| {
        let mut cell = Cell::new(c.into());
        if ctx.color {
            cell = cell.add_attribute(Attribute::Bold);
        }
        cell
    });
    table.set_header(cells);
    if ctx.format != Format::Md {
        for col in table.column_iter_mut() {
            col.set_padding((0, 2));
            col.set_cell_alignment(CellAlignment::Left);
        }
    }
}

/// Helper: a Cell whose text is `value.map_or("—", |v| &v.to_string())`.
pub fn opt_cell<T: ToString>(v: &Option<T>) -> Cell {
    match v {
        Some(x) => Cell::new(x.to_string()),
        None => Cell::new("—"),
    }
}

/// Helper for boolean cells: ✓ / ✗ / —.
pub fn bool_cell(v: Option<bool>) -> Cell {
    match v {
        Some(true) => Cell::new("✓"),
        Some(false) => Cell::new("✗"),
        None => Cell::new("—"),
    }
}

/// Writes `table` to `w`.
pub fn write_table(w: &mut dyn Write, table: &Table) -> std::io::Result<()> {
    writeln!(w, "{table}")
}

/// Writes a `"showing X–Y of Z"` footer below a list-style table. Handles the
/// empty-page case (`page_len == 0`) without underflowing to `"1-0 of N"`.
pub fn write_pagination_footer(
    w: &mut dyn Write,
    offset: i64,
    page_len: usize,
    total: i64,
) -> std::io::Result<()> {
    if page_len == 0 {
        writeln!(w, "showing 0 of {total}")
    } else {
        let end = (offset + page_len as i64).min(total);
        writeln!(w, "showing {}–{} of {}", offset + 1, end, total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[derive(Serialize)]
    struct Sample {
        id: String,
        n: i64,
    }

    impl Render for Sample {
        fn render_table(&self, w: &mut dyn Write, _: &OutputCtx) -> std::io::Result<()> {
            writeln!(w, "{}\t{}", self.id, self.n)
        }
    }

    fn ctx(format: Format) -> OutputCtx {
        OutputCtx {
            format,
            color: false,
            quiet: false,
            verbose: false,
            wide: false,
            stdout_is_tty: false,
        }
    }

    #[test]
    fn json_path_serializes() {
        let val = Sample {
            id: "x".into(),
            n: 7,
        };
        let s = serde_json::to_string(&val).unwrap();
        assert!(s.contains("\"x\""));
        let mut buf = Cursor::new(Vec::<u8>::new());
        val.render_table(&mut buf, &ctx(Format::Table)).unwrap();
        assert_eq!(String::from_utf8(buf.into_inner()).unwrap(), "x\t7\n");
    }

    #[test]
    fn yaml_serializes_via_serde_yml() {
        let val = Sample {
            id: "x".into(),
            n: 7,
        };
        let s = serde_yml::to_string(&val).unwrap();
        assert!(s.contains("id"), "got:\n{s}");
        assert!(s.contains('x'), "got:\n{s}");
        assert!(s.contains('7'), "got:\n{s}");
    }

    #[test]
    fn toon_serializes_directly_from_serialize() {
        let val = Sample {
            id: "x".into(),
            n: 7,
        };
        let s = toon_format::encode_default(&val).expect("toon encode");
        assert!(s.contains("id:") && s.contains('x'), "got:\n{s}");
        assert!(s.contains("n:") && s.contains('7'), "got:\n{s}");
    }

    #[test]
    fn markdown_table_uses_pipe_borders() {
        let mut t = new_table(&ctx(Format::Md));
        t.set_header(vec!["a", "b"]).add_row(vec!["1", "2"]);
        let s = t.to_string();
        assert!(s.contains('|'), "expected pipe-bordered table, got:\n{s}");
        // ASCII_MARKDOWN doesn't use box-drawing chars.
        assert!(!s.contains('╞'), "unexpected utf8 border in md table:\n{s}");
    }

    #[test]
    fn table_format_is_borderless_docker_style() {
        let mut t = new_table(&ctx(Format::Table));
        set_header_bold(&mut t, &ctx(Format::Table), vec!["A", "B"]);
        t.add_row(vec!["1", "2"]);
        let s = t.to_string();
        // No box-drawing characters from the UTF8_FULL preset.
        assert!(!s.contains('╞'), "unexpected utf8 border:\n{s}");
        assert!(!s.contains('│'), "unexpected utf8 border:\n{s}");
        // Columns separated by spaces (the borderless preset has none).
        assert!(s.contains("A") && s.contains("B"));
        assert!(s.contains("1") && s.contains("2"));
    }

    fn ctx_for(
        format: Format,
        no_color: bool,
        stdout_is_tty: bool,
        no_color_env: Option<&str>,
        term: Option<&str>,
    ) -> OutputCtx {
        OutputCtx::detect_with(
            format,
            no_color,
            false,
            false,
            false,
            stdout_is_tty,
            no_color_env.map(std::ffi::OsString::from),
            term.map(String::from),
        )
    }

    #[test]
    fn color_disabled_with_no_color_env() {
        let ctx = ctx_for(Format::Table, false, true, Some("1"), None);
        assert!(!ctx.color);
    }

    #[test]
    fn empty_no_color_env_does_not_disable() {
        let ctx = ctx_for(Format::Table, false, true, Some(""), None);
        assert!(ctx.color);
    }

    #[test]
    fn color_disabled_with_term_dumb() {
        let ctx = ctx_for(Format::Table, false, true, None, Some("dumb"));
        assert!(!ctx.color);
    }

    #[test]
    fn color_disabled_when_not_tty() {
        let ctx = ctx_for(Format::Table, false, false, None, None);
        assert!(!ctx.color);
    }

    #[test]
    fn color_disabled_for_non_table_formats() {
        for f in [Format::Json, Format::Yaml, Format::Md, Format::Toon] {
            let ctx = ctx_for(f, false, true, None, None);
            assert!(!ctx.color, "color should be off for {f:?}");
        }
    }

    #[test]
    fn color_disabled_with_no_color_flag() {
        let ctx = ctx_for(Format::Table, true, true, None, None);
        assert!(!ctx.color);
    }

    #[test]
    fn color_enabled_on_tty_with_no_overrides() {
        let ctx = ctx_for(Format::Table, false, true, None, Some("xterm-256color"));
        assert!(ctx.color);
    }

    #[test]
    fn opt_cell_shows_dash_for_none() {
        let cell: Cell = opt_cell::<String>(&None);
        let mut t = new_table(&ctx(Format::Table));
        t.set_header(vec!["x"]).add_row(vec![cell]);
        let s = t.to_string();
        assert!(s.contains("—"), "got:\n{s}");
    }

    #[test]
    fn bool_cell_renders_check_or_cross() {
        let mut t = new_table(&ctx(Format::Table));
        t.set_header(vec!["y", "n", "u"]).add_row(vec![
            bool_cell(Some(true)),
            bool_cell(Some(false)),
            bool_cell(None),
        ]);
        let s = t.to_string();
        assert!(
            s.contains("✓") && s.contains("✗") && s.contains("—"),
            "got:\n{s}"
        );
    }

    #[test]
    fn is_structured_classification() {
        assert!(Format::Json.is_structured());
        assert!(Format::Yaml.is_structured());
        assert!(Format::Toon.is_structured());
        assert!(!Format::Table.is_structured());
        assert!(!Format::Md.is_structured());
    }

    #[test]
    fn flatten_joins_primitive_array_inside_array_element() {
        let mut v = serde_json::json!({"data": [{"id": 1, "tags": ["a", "b", "c"]}]});
        flatten_primitive_arrays(&mut v);
        assert_eq!(
            v,
            serde_json::json!({"data": [{"id": 1, "tags": "a, b, c"}]})
        );
    }

    #[test]
    fn flatten_collapses_empty_primitive_array_to_empty_string() {
        let mut v = serde_json::json!({"data": [{"tags": []}]});
        flatten_primitive_arrays(&mut v);
        assert_eq!(v, serde_json::json!({"data": [{"tags": ""}]}));
    }

    #[test]
    fn flatten_leaves_top_level_primitive_array_alone() {
        // Top-level primitive arrays are TOON-friendly already (`tags[2]: a,b`),
        // and joining them would change the semantics observed by callers.
        let mut v = serde_json::json!({"tags": ["a", "b"]});
        flatten_primitive_arrays(&mut v);
        assert_eq!(v, serde_json::json!({"tags": ["a", "b"]}));
    }

    #[test]
    fn flatten_leaves_array_of_objects_alone() {
        // The generic walker doesn't know how to summarize an array of
        // structs — that's `Render::toon_projection`'s job.
        let mut v = serde_json::json!({"data": [{"tags": [{"tag_id": 1, "label": "x"}]}]});
        flatten_primitive_arrays(&mut v);
        assert_eq!(
            v,
            serde_json::json!({"data": [{"tags": [{"tag_id": 1, "label": "x"}]}]})
        );
    }

    #[test]
    fn flatten_preserves_sibling_pagination_object() {
        let mut v = serde_json::json!({
            "data": [{"id": 1, "tags": ["x"]}],
            "pagination": {"total": 1, "limit": 100, "offset": 0}
        });
        flatten_primitive_arrays(&mut v);
        assert_eq!(
            v,
            serde_json::json!({
                "data": [{"id": 1, "tags": "x"}],
                "pagination": {"total": 1, "limit": 100, "offset": 0}
            })
        );
    }

    #[test]
    fn flatten_then_toon_emits_tabular_header() {
        let mut v = serde_json::json!({
            "data": [
                {"id": 1, "name": "a", "tags": ["prod"]},
                {"id": 2, "name": "b", "tags": []}
            ]
        });
        flatten_primitive_arrays(&mut v);
        let s = toon_format::encode_default(&v).unwrap();
        assert!(
            s.contains("data[2]{") && s.contains("}:"),
            "expected tabular header, got:\n{s}"
        );
        assert!(s.contains("prod"), "got:\n{s}");
    }
}
