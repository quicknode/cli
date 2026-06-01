//! Renderers for `qn kv …`.

use comfy_table::Cell;
use serde::Serialize;

use crate::output::{new_table, set_header_bold, write_table, OutputCtx, Render};

#[derive(Serialize)]
pub(super) struct SetsView(pub quicknode_sdk::GetSetsResponse);

impl Render for SetsView {
    fn render_table(&self, w: &mut dyn std::io::Write, ctx: &OutputCtx) -> std::io::Result<()> {
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["KEY", "VALUE"]);
        for e in &self.0.data {
            t.add_row(vec![Cell::new(&e.key), Cell::new(&e.value)]);
        }
        write_table(w, &t)?;
        if !self.0.cursor.is_empty() {
            writeln!(w, "next: --cursor {}", self.0.cursor)?;
        }
        Ok(())
    }
}

#[derive(Serialize)]
pub(super) struct ListsView(pub quicknode_sdk::GetListsResponse);

impl Render for ListsView {
    fn render_table(&self, w: &mut dyn std::io::Write, ctx: &OutputCtx) -> std::io::Result<()> {
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["KEY"]);
        for k in &self.0.data.keys {
            t.add_row(vec![Cell::new(k)]);
        }
        write_table(w, &t)?;
        if !self.0.cursor.is_empty() {
            writeln!(w, "next: --cursor {}", self.0.cursor)?;
        }
        Ok(())
    }
}

#[derive(Serialize)]
pub(super) struct ListView(pub quicknode_sdk::GetListResponse);

impl Render for ListView {
    fn render_table(&self, w: &mut dyn std::io::Write, _ctx: &OutputCtx) -> std::io::Result<()> {
        for item in &self.0.data.items {
            writeln!(w, "{item}")?;
        }
        if !self.0.cursor.is_empty() {
            writeln!(w, "next: --cursor {}", self.0.cursor)?;
        }
        Ok(())
    }
}

impl Render for quicknode_sdk::GetSetResponse {
    fn render_table(&self, w: &mut dyn std::io::Write, _ctx: &OutputCtx) -> std::io::Result<()> {
        writeln!(w, "{}", self.value)
    }
}

impl Render for quicknode_sdk::ListContainsItemResponse {
    fn render_table(&self, w: &mut dyn std::io::Write, _ctx: &OutputCtx) -> std::io::Result<()> {
        writeln!(w, "{}", self.exists)
    }
}
