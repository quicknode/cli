//! Renderers for `qn webhook …`.

use comfy_table::Cell;
use serde::Serialize;

use crate::output::{new_table, opt_cell, set_header_bold, write_table, OutputCtx, Render};

#[derive(Serialize)]
pub(super) struct WebhooksListView(pub quicknode_sdk::webhooks::ListWebhooksResponse);

impl Render for WebhooksListView {
    fn render_table(&self, w: &mut dyn std::io::Write, ctx: &OutputCtx) -> std::io::Result<()> {
        let mut t = new_table(ctx);
        set_header_bold(
            &mut t,
            ctx,
            vec!["ID", "NAME", "STATUS", "NETWORK", "TEMPLATE"],
        );
        for h in &self.0.data {
            t.add_row(vec![
                Cell::new(&h.id),
                Cell::new(&h.name),
                Cell::new(&h.status),
                Cell::new(&h.network),
                opt_cell(&h.template_id),
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
pub(super) struct WebhookView(pub quicknode_sdk::webhooks::Webhook);

impl Render for WebhookView {
    fn render_table(&self, w: &mut dyn std::io::Write, ctx: &OutputCtx) -> std::io::Result<()> {
        let h = &self.0;
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["FIELD", "VALUE"]);
        t.add_row(vec![Cell::new("id"), Cell::new(&h.id)]);
        t.add_row(vec![Cell::new("name"), Cell::new(&h.name)]);
        t.add_row(vec![Cell::new("status"), Cell::new(&h.status)]);
        t.add_row(vec![Cell::new("network"), Cell::new(&h.network)]);
        t.add_row(vec![Cell::new("template_id"), opt_cell(&h.template_id)]);
        t.add_row(vec![Cell::new("created_at"), Cell::new(&h.created_at)]);
        t.add_row(vec![Cell::new("updated_at"), opt_cell(&h.updated_at)]);
        t.add_row(vec![
            Cell::new("notification_email"),
            opt_cell(&h.notification_email),
        ]);
        if let Some(d) = &h.destination_attributes {
            t.add_row(vec![Cell::new("destination_attributes"), Cell::new(d)]);
        }
        write_table(w, &t)
    }
}

impl Render for quicknode_sdk::webhooks::WebhookEnabledCountResponse {
    fn render_table(&self, w: &mut dyn std::io::Write, _ctx: &OutputCtx) -> std::io::Result<()> {
        writeln!(w, "{}", self.total)
    }
}
