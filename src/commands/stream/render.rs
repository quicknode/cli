//! Renderers for `qn stream …`.

use comfy_table::Cell;
use serde::Serialize;

use crate::output::{new_table, opt_cell, set_header_bold, write_table, OutputCtx, Render};

#[derive(Serialize)]
pub(super) struct StreamsListView(pub quicknode_sdk::streams::ListStreamsResponse);

impl Render for StreamsListView {
    fn render_table(&self, w: &mut dyn std::io::Write, ctx: &OutputCtx) -> std::io::Result<()> {
        let mut t = new_table(ctx);
        set_header_bold(
            &mut t,
            ctx,
            vec!["ID", "NAME", "STATUS", "NETWORK", "DATASET", "REGION"],
        );
        for s in &self.0.data {
            t.add_row(vec![
                Cell::new(&s.id),
                Cell::new(&s.name),
                Cell::new(&s.status),
                Cell::new(&s.network),
                Cell::new(&s.dataset),
                Cell::new(&s.region),
            ]);
        }
        write_table(w, &t)?;
        crate::output::write_pagination_footer(
            w,
            self.0.page_info.offset,
            self.0.data.len(),
            self.0.page_info.total,
        )
    }
}

#[derive(Serialize)]
pub(super) struct StreamView(pub quicknode_sdk::streams::Stream);

impl Render for StreamView {
    fn render_table(&self, w: &mut dyn std::io::Write, ctx: &OutputCtx) -> std::io::Result<()> {
        let s = &self.0;
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["FIELD", "VALUE"]);
        t.add_row(vec![Cell::new("id"), Cell::new(&s.id)]);
        t.add_row(vec![Cell::new("name"), Cell::new(&s.name)]);
        t.add_row(vec![Cell::new("status"), Cell::new(&s.status)]);
        t.add_row(vec![Cell::new("network"), Cell::new(&s.network)]);
        t.add_row(vec![Cell::new("dataset"), Cell::new(&s.dataset)]);
        t.add_row(vec![Cell::new("region"), Cell::new(&s.region)]);
        t.add_row(vec![Cell::new("start_range"), Cell::new(s.start_range)]);
        t.add_row(vec![Cell::new("end_range"), Cell::new(s.end_range)]);
        t.add_row(vec![Cell::new("created_at"), Cell::new(&s.created_at)]);
        t.add_row(vec![Cell::new("updated_at"), Cell::new(&s.updated_at)]);
        t.add_row(vec![Cell::new("plan"), opt_cell(&s.plan)]);
        write_table(w, &t)
    }
}

#[derive(Serialize)]
pub(super) struct TestFilterView(pub quicknode_sdk::streams::TestFilterResponse);

impl Render for TestFilterView {
    fn render_table(&self, w: &mut dyn std::io::Write, _ctx: &OutputCtx) -> std::io::Result<()> {
        if !self.0.logs.is_empty() {
            writeln!(w, "== logs ==")?;
            for line in &self.0.logs {
                writeln!(w, "{line}")?;
            }
        }
        writeln!(w, "== result ==")?;
        writeln!(w, "{}", self.0.result)
    }
}

impl Render for quicknode_sdk::streams::EnabledCountResponse {
    fn render_table(&self, w: &mut dyn std::io::Write, _ctx: &OutputCtx) -> std::io::Result<()> {
        writeln!(w, "{}", self.total)
    }
}
