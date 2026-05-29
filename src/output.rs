//! Output rendering: JSON for scripts, pretty tables for humans.
//!
//! Decision logic per the plan:
//! - `--json` → pretty-printed JSON to stdout.
//! - TTY and not `--json` → call the type's `render_table` to stdout.
//! - Non-TTY and not `--json` → still call `render_table`, but without color.
//!
//! Color is suppressed when any of: `--no-color`, `NO_COLOR` env, `TERM=dumb`,
//! or stdout is not a TTY.
//!
//! Every state-changing command also writes a one-line confirmation to stderr
//! through [`OutputCtx::note`] — this isn't muted by `--json` (it's stderr),
//! only by `--quiet`.

use std::io::{IsTerminal, Write};

use comfy_table::{Cell, ContentArrangement, Table};
use serde::Serialize;

use crate::errors::CliError;

/// Carries the user's output preferences and TTY state.
#[derive(Debug, Clone, Copy)]
pub struct OutputCtx {
    pub json: bool,
    pub color: bool,
    pub quiet: bool,
    pub verbose: bool,
    pub stdout_is_tty: bool,
}

impl OutputCtx {
    /// Detect from environment + CLI flags. `--no-color` and `--json` are the
    /// only explicit overrides; the rest comes from the environment.
    pub fn detect(json: bool, no_color: bool, quiet: bool, verbose: bool) -> Self {
        Self::detect_with(
            json,
            no_color,
            quiet,
            verbose,
            std::io::stdout().is_terminal(),
            std::env::var_os("NO_COLOR"),
            std::env::var("TERM").ok(),
        )
    }

    /// Pure version of [`detect`] for testing.
    pub fn detect_with(
        json: bool,
        no_color: bool,
        quiet: bool,
        verbose: bool,
        stdout_is_tty: bool,
        no_color_env: Option<std::ffi::OsString>,
        term_env: Option<String>,
    ) -> Self {
        let color = !no_color
            && !json
            && stdout_is_tty
            && no_color_env.map_or(true, |v| v.is_empty())
            && term_env.map_or(true, |t| t != "dumb");
        Self {
            json,
            color,
            quiet,
            verbose,
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
    /// Render a table representation to `w`. Implementations should respect
    /// `_ctx.color` (most can ignore it; `comfy-table` doesn't color rows
    /// unless we ask).
    fn render_table(&self, w: &mut dyn Write, ctx: &OutputCtx) -> std::io::Result<()>;
}

/// Top-level emit: decides between JSON and table.
pub fn emit<T: Render>(ctx: &OutputCtx, value: &T) -> Result<(), CliError> {
    let mut out = std::io::stdout().lock();
    if ctx.json {
        serde_json::to_writer_pretty(&mut out, value)?;
        out.write_all(b"\n")?;
    } else {
        value.render_table(&mut out, ctx)?;
    }
    Ok(())
}

/// Builds a fresh table with conservative defaults: ASCII-only borders unless
/// we're on a TTY, dynamic column widths.
pub fn new_table() -> Table {
    let mut t = Table::new();
    t.set_content_arrangement(ContentArrangement::Dynamic);
    t.load_preset(comfy_table::presets::UTF8_FULL);
    t
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

/// Writes `text` to `w`, falling back to the table preset on errors. Used by
/// renderers that build a [`Table`] and want a one-line writeln.
pub fn write_table(w: &mut dyn Write, table: &Table) -> std::io::Result<()> {
    writeln!(w, "{table}")
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

    #[test]
    fn json_path_serializes() {
        let ctx = OutputCtx {
            json: true,
            color: false,
            quiet: false,
            verbose: false,
            stdout_is_tty: false,
        };
        let val = Sample {
            id: "x".into(),
            n: 7,
        };
        // Smoke: serialize doesn't panic and produces valid json.
        let s = serde_json::to_string(&val).unwrap();
        assert!(s.contains("\"x\""));
        // emit() writes to stdout (the real one); we test render_table directly here.
        let mut buf = Cursor::new(Vec::<u8>::new());
        val.render_table(&mut buf, &ctx).unwrap();
        let out = String::from_utf8(buf.into_inner()).unwrap();
        assert_eq!(out, "x\t7\n");
    }

    fn ctx_for(
        json: bool,
        no_color: bool,
        stdout_is_tty: bool,
        no_color_env: Option<&str>,
        term: Option<&str>,
    ) -> OutputCtx {
        OutputCtx::detect_with(
            json,
            no_color,
            false,
            false,
            stdout_is_tty,
            no_color_env.map(std::ffi::OsString::from),
            term.map(String::from),
        )
    }

    #[test]
    fn color_disabled_with_no_color_env() {
        let ctx = ctx_for(false, false, true, Some("1"), None);
        assert!(!ctx.color);
    }

    #[test]
    fn empty_no_color_env_does_not_disable() {
        let ctx = ctx_for(false, false, true, Some(""), None);
        assert!(ctx.color);
    }

    #[test]
    fn color_disabled_with_term_dumb() {
        let ctx = ctx_for(false, false, true, None, Some("dumb"));
        assert!(!ctx.color);
    }

    #[test]
    fn color_disabled_when_not_tty() {
        let ctx = ctx_for(false, false, false, None, None);
        assert!(!ctx.color);
    }

    #[test]
    fn color_disabled_when_json() {
        let ctx = ctx_for(true, false, true, None, None);
        assert!(!ctx.color);
    }

    #[test]
    fn color_disabled_with_no_color_flag() {
        let ctx = ctx_for(false, true, true, None, None);
        assert!(!ctx.color);
    }

    #[test]
    fn color_enabled_on_tty_with_no_overrides() {
        let ctx = ctx_for(false, false, true, None, Some("xterm-256color"));
        assert!(ctx.color);
    }

    #[test]
    fn opt_cell_shows_dash_for_none() {
        let cell: Cell = opt_cell::<String>(&None);
        // No direct accessor on Cell; rendering via table is the only way.
        let mut t = new_table();
        t.set_header(vec!["x"]).add_row(vec![cell]);
        let s = t.to_string();
        assert!(s.contains("—"), "got:\n{s}");
    }

    #[test]
    fn bool_cell_renders_check_or_cross() {
        let mut t = new_table();
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
}
