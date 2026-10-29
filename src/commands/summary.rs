//! Shared flags and rendering for the cursor-paginated summary routes:
//! `usage credits|rpc|streams|webhooks`, `metrics traffic|errors`,
//! `endpoint errors`, and `transaction list`.
//!
//! The API wants either a window (`from` + `to`) or a `cursor` that continues
//! the previous query; a cursor must travel alone.

use std::io::Write;

use clap::Args as ClapArgs;
use comfy_table::Cell;

use crate::errors::CliError;
use crate::output::{new_table, opt_cell, set_header_bold, write_table, OutputCtx};
use crate::time_arg;

#[derive(Debug, ClapArgs)]
pub struct WindowArgs {
    /// Start of the window (RFC-3339, a relative duration like `1h`, or `now`).
    /// Required unless --cursor is given.
    #[arg(long)]
    pub from: Option<String>,
    /// End of the window. Defaults to now.
    #[arg(long)]
    pub to: Option<String>,
    /// Page size (1-1000; the API defaults to 100).
    #[arg(long, value_parser = clap::value_parser!(i32).range(1..=1000))]
    pub limit: Option<i32>,
    /// Continue from a previous page's `next_cursor`. The cursor carries the
    /// original query, so no other flag may accompany it.
    #[arg(long)]
    pub cursor: Option<String>,
}

/// A resolved window: a cursor alone, or an explicit RFC-3339 `from`/`to` pair.
#[derive(Debug, Default)]
pub struct Window {
    pub from_time: Option<String>,
    pub to_time: Option<String>,
    pub limit: Option<i32>,
    pub cursor: Option<String>,
}

impl WindowArgs {
    /// `filters_given` is whether any command-specific grouping or filter flag
    /// was passed; those are rejected alongside a cursor too.
    pub fn resolve(self, filters_given: bool) -> Result<Window, CliError> {
        if let Some(cursor) = self.cursor {
            if self.from.is_some() || self.to.is_some() || self.limit.is_some() || filters_given {
                return Err(CliError::Arg(
                    "--cursor continues the previous query; do not combine it with \
                     --from, --to, --limit, --group-by, or filters"
                        .into(),
                ));
            }
            return Ok(Window {
                cursor: Some(cursor),
                ..Default::default()
            });
        }
        let from = self.from.ok_or_else(|| {
            CliError::Arg(
                "supply --from (e.g. --from 1h), or --cursor to continue a previous page".into(),
            )
        })?;
        Ok(Window {
            from_time: Some(time_arg::parse_rfc3339(&from)?),
            to_time: Some(time_arg::parse_rfc3339(
                self.to.as_deref().unwrap_or("now"),
            )?),
            limit: self.limit,
            cursor: None,
        })
    }
}

/// `None` for an empty flag list: the SDK sends an empty `Vec` as a filter
/// value, not as "unfiltered".
pub fn non_empty<T>(v: Vec<T>) -> Option<Vec<T>> {
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

/// A grouping dimension cell. The outer `None` means the dimension was not
/// selected (no column); the inner `None` is an explicit null from the API.
pub fn dim<T: ToString>(v: &Option<Option<T>>) -> Option<Cell> {
    v.as_ref().map(opt_cell)
}

/// Reads one grouping dimension from a breakdown row; `None` when the
/// dimension was not selected.
pub type DimGetter<R> = fn(&R) -> Option<Cell>;

/// Builds the breakdown table: one column per dimension the API returned
/// (selected with --group-by), followed by the metric columns.
pub fn breakdown_table<R>(
    rows: &[R],
    dims: &[(&'static str, DimGetter<R>)],
    metrics: &[&'static str],
    metric_cells: impl Fn(&R) -> Vec<Cell>,
) -> (Vec<&'static str>, Vec<Vec<Cell>>) {
    let present: Vec<&(&'static str, DimGetter<R>)> = dims
        .iter()
        .filter(|(_, get)| rows.iter().any(|r| get(r).is_some()))
        .collect();
    let mut columns: Vec<&'static str> = present.iter().map(|(h, _)| *h).collect();
    columns.extend(metrics);
    let table_rows = rows
        .iter()
        .map(|r| {
            let mut cells: Vec<Cell> = present
                .iter()
                .map(|(_, get)| get(r).unwrap_or_else(|| Cell::new("—")))
                .collect();
            cells.extend(metric_cells(r));
            cells
        })
        .collect();
    (columns, table_rows)
}

/// One summary page: the resolved window, full-window totals, the optional
/// grouped rows, and the continuation cursor.
pub struct SummaryPage<'a> {
    pub from: &'a str,
    pub to: &'a str,
    pub totals: Vec<(&'static str, Cell)>,
    pub breakdown: Option<(Vec<&'static str>, Vec<Vec<Cell>>)>,
    pub next_cursor: Option<&'a str>,
}

pub fn write_summary(
    w: &mut dyn Write,
    ctx: &OutputCtx,
    page: SummaryPage<'_>,
) -> std::io::Result<()> {
    writeln!(w, "window: {} → {}", page.from, page.to)?;
    let mut t = new_table(ctx);
    set_header_bold(&mut t, ctx, vec!["FIELD", "VALUE"]);
    for (field, value) in page.totals {
        t.add_row(vec![Cell::new(field), value]);
    }
    write_table(w, &t)?;
    if let Some((columns, rows)) = page.breakdown {
        writeln!(w)?;
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, columns);
        for r in rows {
            t.add_row(r);
        }
        write_table(w, &t)?;
    }
    write_cursor_footer(w, page.next_cursor)
}

/// Prints the continuation hint when the API returned a `next_cursor`.
pub fn write_cursor_footer(w: &mut dyn Write, next_cursor: Option<&str>) -> std::io::Result<()> {
    if let Some(c) = next_cursor {
        writeln!(w, "next page: --cursor {c}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(from: Option<&str>, to: Option<&str>, cursor: Option<&str>) -> WindowArgs {
        WindowArgs {
            from: from.map(String::from),
            to: to.map(String::from),
            limit: None,
            cursor: cursor.map(String::from),
        }
    }

    #[test]
    fn cursor_alone_resolves_to_cursor_only() {
        let w = args(None, None, Some("abc")).resolve(false).unwrap();
        assert_eq!(w.cursor.as_deref(), Some("abc"));
        assert!(w.from_time.is_none() && w.to_time.is_none());
    }

    #[test]
    fn cursor_rejects_window_and_filters() {
        assert!(args(Some("1h"), None, Some("abc")).resolve(false).is_err());
        assert!(args(None, None, Some("abc")).resolve(true).is_err());
    }

    #[test]
    fn window_requires_from_and_defaults_to_now() {
        assert!(args(None, None, None).resolve(false).is_err());
        let w = args(Some("2026-01-01T00:00:00Z"), None, None)
            .resolve(false)
            .unwrap();
        assert_eq!(w.from_time.as_deref(), Some("2026-01-01T00:00:00Z"));
        assert!(w.to_time.is_some());
    }

    #[test]
    fn breakdown_omits_unselected_dimensions() {
        struct Row {
            method: Option<Option<String>>,
            endpoint: Option<Option<i64>>,
            n: f64,
        }
        let rows = vec![
            Row {
                method: Some(Some("eth_call".into())),
                endpoint: None,
                n: 1.0,
            },
            Row {
                method: Some(None),
                endpoint: None,
                n: 2.5,
            },
        ];
        let (columns, table) = breakdown_table(
            &rows,
            &[
                ("METHOD", |r: &Row| dim(&r.method)),
                ("ENDPOINT", |r: &Row| dim(&r.endpoint)),
            ],
            &["N"],
            |r| vec![Cell::new(r.n)],
        );
        assert_eq!(columns, vec!["METHOD", "N"]);
        assert_eq!(table.len(), 2);
        assert_eq!(table[1][0].content(), "—");
        assert_eq!(table[1][1].content(), "2.5");
        assert_eq!(table[0][1].content(), "1");
    }
}
