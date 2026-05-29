//! Table renderers for endpoint command responses.
//!
//! Each renderer is a `serde::Serialize` newtype wrapper around an SDK
//! response type. JSON output uses the inner type's serialization directly;
//! table output uses the [`Render`] impl.

use std::io::Write;

use comfy_table::Cell;
use quicknode_sdk::admin::{
    EndpointMetric, GetEndpointLogsResponse, GetEndpointMetricsResponse, GetEndpointUrlsData,
    GetEndpointsResponse, GetLogDetailsResponse, SingleEndpoint,
};
use serde::Serialize;

use crate::output::{new_table, opt_cell, set_header_bold, write_table, Render};

#[derive(Serialize)]
pub struct EndpointsView(pub GetEndpointsResponse);

impl Render for EndpointsView {
    fn render_table(
        &self,
        w: &mut dyn Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let mut t = new_table(ctx);
        let mut headers = vec!["ID", "LABEL", "STATUS", "CHAIN/NETWORK", "TYPE", "MULTI"];
        if ctx.wide {
            headers.extend(["HTTP", "WSS"]);
        }
        set_header_bold(&mut t, ctx, headers);
        for e in &self.0.data {
            let mut row = vec![
                Cell::new(&e.id),
                opt_cell(&e.label),
                Cell::new(&e.status),
                Cell::new(format!("{}/{}", e.chain, e.network)),
                Cell::new(if e.is_dedicated {
                    "dedicated"
                } else {
                    "shared"
                }),
                Cell::new(if e.is_multichain { "yes" } else { "no" }),
            ];
            if ctx.wide {
                row.push(Cell::new(&e.http_url));
                row.push(opt_cell(&e.wss_url));
            }
            t.add_row(row);
        }
        write_table(w, &t)?;
        if let Some(p) = &self.0.pagination {
            writeln!(
                w,
                "showing {}–{} of {}",
                p.offset + 1,
                (p.offset as i64 + self.0.data.len() as i64).min(p.total),
                p.total
            )?;
        }
        Ok(())
    }
}

#[derive(Serialize)]
pub struct SingleEndpointView(pub SingleEndpoint);

impl Render for SingleEndpointView {
    fn render_table(
        &self,
        w: &mut dyn Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let e = &self.0;
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["FIELD", "VALUE"]);
        t.add_row(vec![Cell::new("id"), Cell::new(&e.id)]);
        t.add_row(vec![Cell::new("label"), opt_cell(&e.label)]);
        t.add_row(vec![Cell::new("status"), opt_cell(&e.status)]);
        t.add_row(vec![
            Cell::new("chain/network"),
            Cell::new(format!("{}/{}", e.chain, e.network)),
        ]);
        t.add_row(vec![Cell::new("http_url"), Cell::new(&e.http_url)]);
        t.add_row(vec![Cell::new("wss_url"), opt_cell(&e.wss_url)]);
        if !e.tags.is_empty() {
            let tags = e
                .tags
                .iter()
                .map(|t| t.label.clone())
                .collect::<Vec<_>>()
                .join(", ");
            t.add_row(vec![Cell::new("tags"), Cell::new(tags)]);
        }
        write_table(w, &t)
    }
}

#[derive(Serialize)]
pub struct EndpointUrlsView(pub GetEndpointUrlsData);

impl Render for EndpointUrlsView {
    fn render_table(
        &self,
        w: &mut dyn Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["NETWORK", "HTTP", "WSS"]);
        t.add_row(vec![
            Cell::new("default"),
            Cell::new(&self.0.http_url),
            opt_cell(&self.0.wss_url),
        ]);
        if let Some(mc) = &self.0.multichain_urls {
            let mut keys: Vec<_> = mc.keys().collect();
            keys.sort();
            for k in keys {
                let u = &mc[k];
                t.add_row(vec![
                    Cell::new(k),
                    Cell::new(&u.http_url),
                    opt_cell(&u.wss_url),
                ]);
            }
        }
        write_table(w, &t)
    }
}

#[derive(Serialize)]
pub struct EndpointLogsView(pub GetEndpointLogsResponse);

impl Render for EndpointLogsView {
    fn render_table(
        &self,
        w: &mut dyn Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let mut t = new_table(ctx);
        set_header_bold(
            &mut t,
            ctx,
            vec!["TIME", "METHOD", "STATUS", "NETWORK", "REQUEST_ID"],
        );
        for l in &self.0.data {
            t.add_row(vec![
                Cell::new(&l.timestamp),
                opt_cell(&l.method),
                opt_cell(&l.status),
                opt_cell(&l.network),
                opt_cell(&l.request_id),
            ]);
        }
        write_table(w, &t)?;
        if let Some(c) = &self.0.next_at {
            writeln!(w, "next page: --next-at {c}")?;
        }
        Ok(())
    }
}

#[derive(Serialize)]
pub struct LogDetailsView(pub GetLogDetailsResponse);

impl Render for LogDetailsView {
    fn render_table(
        &self,
        w: &mut dyn Write,
        _ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        match &self.0.data {
            Some(d) => {
                writeln!(w, "== request ==")?;
                writeln!(w, "{}", d.request.as_deref().unwrap_or("(none)"))?;
                writeln!(w, "== response ==")?;
                writeln!(w, "{}", d.response.as_deref().unwrap_or("(none)"))?;
            }
            None => writeln!(w, "(no log details)")?,
        }
        Ok(())
    }
}

#[derive(Serialize)]
pub struct EndpointMetricsView(pub GetEndpointMetricsResponse);

impl Render for EndpointMetricsView {
    fn render_table(
        &self,
        w: &mut dyn Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        for m in &self.0.data {
            metric_series(w, m, ctx)?;
        }
        Ok(())
    }
}

pub(crate) fn metric_series(
    w: &mut dyn Write,
    m: &EndpointMetric,
    ctx: &crate::output::OutputCtx,
) -> std::io::Result<()> {
    let label = m.tag.join("/");
    writeln!(
        w,
        "== {} ==",
        if label.is_empty() { "series" } else { &label }
    )?;
    let mut t = new_table(ctx);
    set_header_bold(&mut t, ctx, vec!["TIMESTAMP", "VALUE"]);
    for pair in &m.data {
        let ts = pair.first().copied().unwrap_or_default();
        let v = pair.get(1).copied().unwrap_or_default();
        t.add_row(vec![Cell::new(ts), Cell::new(v)]);
    }
    write_table(w, &t)
}
