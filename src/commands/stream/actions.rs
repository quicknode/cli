//! Command bodies for `qn stream …`.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use quicknode_sdk::streams::{
    CreateStreamParams, DestinationAttributes, ListStreamsParams, TestFilterParams,
    UpdateStreamParams, WebhookAttributes,
};

use super::render::{StreamView, StreamsListView, TestFilterView};
use super::{CreateArgs, ListArgs, TestFilterArgs, UpdateArgs};
use crate::confirm::{decide_without_prompt, prompt_yes_no, ConfirmCfg, Severity};
use crate::context::Ctx;
use crate::errors::CliError;
use crate::retry::retrying;

pub(super) async fn list(a: ListArgs, ctx: Ctx) -> Result<(), CliError> {
    let params = ListStreamsParams {
        stream_type: a.stream_type,
        offset: a.offset,
        limit: a.limit,
        order_by: a.order_by,
        order_direction: a.order_direction,
    };
    let resp = retrying(ctx.global.retries, || ctx.sdk.streams.list_streams(&params)).await?;
    crate::output::emit(&ctx.out, &StreamsListView(resp))
}

pub(super) async fn create(a: CreateArgs, ctx: Ctx) -> Result<(), CliError> {
    let params = if let Some(path) = a.stream_config_file {
        let text = std::fs::read_to_string(&path)?;
        serde_json::from_str::<CreateStreamParams>(&text)?
    } else {
        build_create_params(a)?
    };
    let stream = ctx.sdk.streams.create_stream(&params).await?;
    ctx.out.note(&format!("✓ Created stream {}", stream.id));
    crate::output::emit(&ctx.out, &StreamView(stream))
}

fn build_create_params(a: CreateArgs) -> Result<CreateStreamParams, CliError> {
    // These flags are `required_unless_present = "stream_config_file"` in
    // clap, and this function is only reached when --stream-config-file was
    // NOT supplied.
    const ENFORCED: &str = "enforced by clap unless --stream-config-file is present";
    let name = a.name.expect(ENFORCED);
    let network = a.network.expect(ENFORCED);
    let dataset = a.dataset.expect(ENFORCED);
    let start = a.start.expect(ENFORCED);
    let end = a.end.expect(ENFORCED);
    let region = a.region.expect(ENFORCED);
    let url = a.webhook.expect(ENFORCED);

    let filter_function = match (a.filter, a.filter_file) {
        (Some(s), None) => Some(STANDARD.encode(s)),
        (None, Some(p)) => Some(STANDARD.encode(std::fs::read(&p)?)),
        (None, None) => None,
        (Some(_), Some(_)) => {
            return Err(CliError::Arg(
                "supply only one of --filter or --filter-file".into(),
            ));
        }
    };

    Ok(CreateStreamParams {
        name,
        region: region.into(),
        network,
        dataset: dataset.into(),
        start_range: start,
        end_range: end,
        destination_attributes: DestinationAttributes::Webhook(WebhookAttributes {
            url,
            max_retry: 3,
            retry_interval_sec: 1,
            post_timeout_sec: 10,
            compression: Some(a.compression),
            security_token: a.webhook_security_token,
        }),
        plan: a.plan,
        threshold_fetch_buffer: a.threshold_fetch_buffer,
        dataset_batch_size: a.batch_size.unwrap_or(1),
        max_batch_size: None,
        max_buffer_range_size: None,
        max_buffer_processing_workers: None,
        keep_distance_from_tip: a.keep_distance_from_tip,
        filter_function,
        filter_language: a.filter_language.map(Into::into),
        address_book_config: None,
        include_stream_metadata: None,
        product_type: None,
        status: a.status.map(Into::into),
        notification_email: a.notification_email,
        charge_min_cap: None,
        // The API models this as 0/1.
        fix_block_reorgs: a.fix_block_reorgs.map(i32::from),
        elastic_batch_enabled: a.elastic_batch_enabled.unwrap_or(false),
        extra_destinations: None,
    })
}

pub(super) async fn show(id: &str, ctx: Ctx) -> Result<(), CliError> {
    let s = retrying(ctx.global.retries, || ctx.sdk.streams.get_stream(id)).await?;
    crate::output::emit(&ctx.out, &StreamView(s))
}

pub(super) async fn update(a: UpdateArgs, ctx: Ctx) -> Result<(), CliError> {
    if a.name.is_none() && a.status.is_none() && a.notification_email.is_none() {
        return Err(CliError::Arg(
            "supply at least one of --name, --status, --notification-email".into(),
        ));
    }
    let params = UpdateStreamParams {
        name: a.name,
        status: a.status.map(Into::into),
        notification_email: a.notification_email,
        ..Default::default()
    };
    let s = ctx.sdk.streams.update_stream(&a.id, &params).await?;
    ctx.out.note(&format!("✓ Updated stream {}", a.id));
    crate::output::emit(&ctx.out, &StreamView(s))
}

pub(super) async fn delete(id: &str, ctx: Ctx) -> Result<(), CliError> {
    let cfg = ConfirmCfg::new(
        ctx.global.yes_count,
        ctx.global.no_input,
        ctx.out.stdout_is_tty,
    );
    let proceed = match decide_without_prompt(Severity::Mild, cfg)? {
        true => true,
        false => prompt_yes_no(&format!("Delete stream {id}?"))?,
    };
    if !proceed {
        return Err(CliError::Cancelled);
    }
    ctx.sdk.streams.delete_stream(id).await?;
    ctx.out.note(&format!("✓ Deleted stream {id}"));
    Ok(())
}

// There is intentionally no `stream delete-all` command. Account-wide wipes
// are out of scope for the CLI; use the API directly if you really need one.

pub(super) async fn activate(id: &str, ctx: Ctx) -> Result<(), CliError> {
    ctx.sdk.streams.activate_stream(id).await?;
    ctx.out.note(&format!("✓ Activated stream {id}"));
    Ok(())
}

pub(super) async fn pause(id: &str, ctx: Ctx) -> Result<(), CliError> {
    ctx.sdk.streams.pause_stream(id).await?;
    ctx.out.note(&format!("✓ Paused stream {id}"));
    Ok(())
}

pub(super) async fn test_filter(a: TestFilterArgs, ctx: Ctx) -> Result<(), CliError> {
    let filter_function = match (a.filter, a.filter_file) {
        (Some(s), None) => STANDARD.encode(s),
        (None, Some(p)) => STANDARD.encode(std::fs::read(&p)?),
        (None, None) => {
            return Err(CliError::Arg("supply --filter or --filter-file".into()));
        }
        (Some(_), Some(_)) => {
            return Err(CliError::Arg(
                "supply only one of --filter or --filter-file".into(),
            ));
        }
    };
    let params = TestFilterParams {
        network: a.network,
        dataset: a.dataset.into(),
        block: a.block,
        filter_function,
        filter_language: a.filter_language.map(Into::into),
        address_book_config: None,
    };
    // POST, but read-only: it evaluates a filter against historical data and
    // changes nothing, so it's safe to retry.
    let resp = retrying(ctx.global.retries, || ctx.sdk.streams.test_filter(&params)).await?;
    crate::output::emit(&ctx.out, &TestFilterView(resp))
}

pub(super) async fn enabled_count(stream_type: Option<String>, ctx: Ctx) -> Result<(), CliError> {
    let resp = retrying(ctx.global.retries, || {
        ctx.sdk.streams.get_enabled_count(stream_type.as_deref())
    })
    .await?;
    if ctx.out.format.is_structured() {
        crate::output::emit(&ctx.out, &resp)
    } else {
        println!("{}", resp.total);
        Ok(())
    }
}
