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
    // SDK ships `tags: Vec<EndpointTag>` on every Endpoint. That nested struct
    // array blocks TOON's compact tabular form. For TOON only, project tags
    // down to their labels — `tag_id` isn't user-facing here. JSON/YAML keep
    // the full struct via the derived `Serialize`.
    fn toon_projection(&self) -> Option<serde_json::Value> {
        let mut v = serde_json::to_value(&self.0).ok()?;
        if let Some(data) = v.get_mut("data").and_then(|d| d.as_array_mut()) {
            for ep in data {
                if let Some(tags) = ep.get_mut("tags").and_then(|t| t.as_array_mut()) {
                    let labels: Vec<serde_json::Value> = tags
                        .iter()
                        .filter_map(|t| t.get("label").cloned())
                        .collect();
                    *ep.get_mut("tags").unwrap() = serde_json::Value::Array(labels);
                }
            }
        }
        Some(v)
    }

    fn render_table(
        &self,
        w: &mut dyn Write,
        ctx: &crate::output::OutputCtx,
    ) -> std::io::Result<()> {
        let mut t = new_table(ctx);
        let mut headers = vec!["ID", "NAME", "LABEL", "STATUS", "CHAIN/NETWORK", "TYPE", "MULTI"];
        if ctx.wide {
            headers.extend(["HTTP", "WSS"]);
        }
        set_header_bold(&mut t, ctx, headers);
        for e in &self.0.data {
            let mut row = vec![
                Cell::new(&e.id),
                Cell::new(&e.name),
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

#[cfg(test)]
mod tests {
    use super::*;
    use quicknode_sdk::admin::{Endpoint, EndpointTag, Pagination};

    fn fixture() -> GetEndpointsResponse {
        GetEndpointsResponse {
            data: vec![
                Endpoint {
                    id: "ep-1".into(),
                    name: "ep-1".into(),
                    label: Some("prod-1".into()),
                    status: "active".into(),
                    chain: "ethereum".into(),
                    network: "mainnet".into(),
                    is_dedicated: false,
                    is_flat_rate: false,
                    http_url: "https://ep-1.example".into(),
                    wss_url: None,
                    tags: vec![
                        EndpointTag {
                            tag_id: 1,
                            label: "prod".into(),
                        },
                        EndpointTag {
                            tag_id: 2,
                            label: "eu".into(),
                        },
                    ],
                    is_multichain: false,
                },
                Endpoint {
                    id: "ep-2".into(),
                    name: "ep-2".into(),
                    label: None,
                    status: "paused".into(),
                    chain: "solana".into(),
                    network: "mainnet".into(),
                    is_dedicated: true,
                    is_flat_rate: false,
                    http_url: "https://ep-2.example".into(),
                    wss_url: None,
                    tags: vec![],
                    is_multichain: false,
                },
            ],
            pagination: Some(Pagination {
                total: 2,
                limit: 20,
                offset: 0,
            }),
            error: None,
        }
    }

    #[test]
    fn endpoints_view_json_keeps_full_endpoint_tag_struct() {
        // Layer 2 must be TOON-only: JSON consumers still see `tag_id` + `label`.
        let view = EndpointsView(fixture());
        let json = serde_json::to_value(&view).unwrap();
        let tags = &json["data"][0]["tags"];
        assert!(
            tags.is_array(),
            "tags should still be an array, got: {tags}"
        );
        assert_eq!(tags[0]["tag_id"], 1);
        assert_eq!(tags[0]["label"], "prod");
    }

    #[test]
    fn endpoints_view_toon_projection_reduces_tags_to_labels() {
        let view = EndpointsView(fixture());
        let projected = view.toon_projection().expect("projection runs");
        let tags = &projected["data"][0]["tags"];
        assert_eq!(tags, &serde_json::json!(["prod", "eu"]));
        // Second endpoint has no tags → still an empty array.
        assert_eq!(projected["data"][1]["tags"], serde_json::json!([]));
    }

    #[test]
    fn endpoints_view_toon_snapshot() {
        // Locks the compact tabular shape so any regression to verbose
        // per-object output is loud.
        let view = EndpointsView(fixture());
        let mut json = view.toon_projection().expect("projection runs");
        crate::output::flatten_primitive_arrays(&mut json);
        let s = toon_format::encode_default(&json).unwrap();
        insta::assert_snapshot!(s);
    }
}
