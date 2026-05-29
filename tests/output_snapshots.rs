//! Snapshot tests for human-readable table output.
//!
//! These lock in the table layout so future changes that affect rendering
//! show up as obvious diffs in `cargo insta review`.

use qn::output::{Format, OutputCtx, Render};
use quicknode_sdk::admin::{
    Endpoint, EndpointTag, GetEndpointsResponse, ListChainsResponse, Pagination,
};
use serde::Serialize;
use std::io::Cursor;

fn no_color_ctx() -> OutputCtx {
    OutputCtx::detect_with(Format::Table, true, false, false, false, false, None, None)
}

fn render_string<R: Render>(r: &R) -> String {
    let mut buf = Cursor::new(Vec::<u8>::new());
    r.render_table(&mut buf, &no_color_ctx()).unwrap();
    String::from_utf8(buf.into_inner()).unwrap()
}

// We re-declare the renderers here as newtypes so we don't have to make every
// internal renderer pub. Each snapshot represents the "shape" of the output
// for a concrete response.
#[derive(Serialize)]
struct EndpointsView(GetEndpointsResponse);
impl Render for EndpointsView {
    fn render_table(&self, w: &mut dyn std::io::Write, ctx: &OutputCtx) -> std::io::Result<()> {
        use comfy_table::Cell;
        use qn::output::{new_table, opt_cell, set_header_bold, write_table};
        let mut t = new_table(ctx);
        set_header_bold(
            &mut t,
            ctx,
            vec!["ID", "LABEL", "STATUS", "CHAIN/NETWORK", "TYPE", "MULTI"],
        );
        for e in &self.0.data {
            t.add_row(vec![
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
            ]);
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
struct ChainsView(ListChainsResponse);
impl Render for ChainsView {
    fn render_table(&self, w: &mut dyn std::io::Write, ctx: &OutputCtx) -> std::io::Result<()> {
        use comfy_table::Cell;
        use qn::output::{new_table, set_header_bold, write_table};
        let mut t = new_table(ctx);
        set_header_bold(&mut t, ctx, vec!["CHAIN", "NETWORKS"]);
        for c in &self.0.data {
            let nets = c
                .networks
                .iter()
                .map(|n| n.slug.clone())
                .collect::<Vec<_>>()
                .join(", ");
            t.add_row(vec![Cell::new(&c.slug), Cell::new(nets)]);
        }
        write_table(w, &t)
    }
}

#[test]
fn endpoints_list_with_pagination_snapshot() {
    let resp = GetEndpointsResponse {
        data: vec![
            Endpoint {
                id: "ep-1".to_string(),
                name: "ep-1".to_string(),
                label: Some("production".to_string()),
                status: "active".to_string(),
                chain: "ethereum".to_string(),
                network: "mainnet".to_string(),
                is_dedicated: false,
                is_flat_rate: false,
                http_url: "https://ep-1.example".to_string(),
                wss_url: None,
                tags: vec![EndpointTag {
                    tag_id: 1,
                    label: "prod".to_string(),
                }],
                is_multichain: true,
            },
            Endpoint {
                id: "ep-2".to_string(),
                name: "ep-2".to_string(),
                label: None,
                status: "paused".to_string(),
                chain: "solana".to_string(),
                network: "mainnet".to_string(),
                is_dedicated: true,
                is_flat_rate: false,
                http_url: "https://ep-2.example".to_string(),
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
    };
    let out = render_string(&EndpointsView(resp));
    insta::assert_snapshot!(out);
}

#[test]
fn empty_endpoints_list_renders_header_only() {
    let resp = GetEndpointsResponse {
        data: vec![],
        pagination: None,
        error: None,
    };
    let out = render_string(&EndpointsView(resp));
    insta::assert_snapshot!(out);
}

#[test]
fn chains_list_snapshot() {
    use quicknode_sdk::admin::{Chain, ChainNetwork};
    let resp = ListChainsResponse {
        data: vec![
            Chain {
                slug: "ethereum".to_string(),
                networks: vec![
                    ChainNetwork {
                        slug: "mainnet".to_string(),
                        name: "Mainnet".to_string(),
                        chain_id: Some(1),
                    },
                    ChainNetwork {
                        slug: "sepolia".to_string(),
                        name: "Sepolia".to_string(),
                        chain_id: Some(11155111),
                    },
                ],
                is_select_chain: None,
            },
            Chain {
                slug: "solana".to_string(),
                networks: vec![ChainNetwork {
                    slug: "mainnet".to_string(),
                    name: "Mainnet Beta".to_string(),
                    chain_id: None,
                }],
                is_select_chain: None,
            },
        ],
        error: None,
    };
    let out = render_string(&ChainsView(resp));
    insta::assert_snapshot!(out);
}
