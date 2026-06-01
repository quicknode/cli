//! Command bodies for `qn stream …`.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use quicknode_sdk::streams::{
    CreateStreamParams, DestinationAttributes, ListStreamsParams, TestFilterParams,
    UpdateStreamParams, WebhookAttributes,
};

use super::render::{StreamView, StreamsListView, TestFilterView};
use super::{CreateArgs, ListArgs, TestFilterArgs, UpdateArgs};
use crate::confirm::{decide_without_prompt, prompt_typed, prompt_yes_no, ConfirmCfg, Severity};
use crate::context::Ctx;
use crate::errors::CliError;

pub(super) async fn list(a: ListArgs, ctx: Ctx) -> Result<(), CliError> {
    let params = ListStreamsParams {
        stream_type: a.stream_type,
        offset: a.offset,
        limit: a.limit,
        order_by: a.order_by,
        order_direction: a.order_direction,
    };
    let resp = ctx.sdk.streams.list_streams(&params).await?;
    crate::output::emit(&ctx.out, &StreamsListView(resp))
}

pub(super) async fn create(a: CreateArgs, ctx: Ctx) -> Result<(), CliError> {
    let params = if let Some(path) = a.config_file {
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
    let name = a
        .name
        .ok_or_else(|| CliError::Arg("--name is required".into()))?;
    let network = a
        .network
        .ok_or_else(|| CliError::Arg("--network is required".into()))?;
    let dataset = a
        .dataset
        .ok_or_else(|| CliError::Arg("--dataset is required".into()))?;
    let start = a
        .start
        .ok_or_else(|| CliError::Arg("--start is required".into()))?;
    let end = a
        .end
        .ok_or_else(|| CliError::Arg("--end is required (-1 for continuous)".into()))?;
    let region = a
        .region
        .ok_or_else(|| CliError::Arg("--region is required".into()))?;
    let url = a.webhook.ok_or_else(|| {
        CliError::Arg("--webhook is required (or use --config-file for other destinations)".into())
    })?;

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
        fix_block_reorgs: a.fix_block_reorgs,
        elastic_batch_enabled: a.elastic_batch_enabled.unwrap_or(false),
        extra_destinations: None,
    })
}

pub(super) async fn show(id: &str, ctx: Ctx) -> Result<(), CliError> {
    let s = ctx.sdk.streams.get_stream(id).await?;
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

pub(super) async fn delete_all(ctx: Ctx) -> Result<(), CliError> {
    let cfg = ConfirmCfg::new(
        ctx.global.yes_count,
        ctx.global.no_input,
        ctx.out.stdout_is_tty,
    );
    let proceed = match decide_without_prompt(Severity::Severe, cfg)? {
        true => true,
        false => prompt_typed(
            "Type 'delete-all' to delete EVERY stream on the account",
            "delete-all",
        )?,
    };
    if !proceed {
        return Err(CliError::Cancelled);
    }
    ctx.sdk.streams.delete_all_streams().await?;
    ctx.out.note("✓ Deleted all streams");
    Ok(())
}

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
    let resp = ctx.sdk.streams.test_filter(&params).await?;
    crate::output::emit(&ctx.out, &TestFilterView(resp))
}

pub(super) async fn enabled_count(stream_type: Option<String>, ctx: Ctx) -> Result<(), CliError> {
    let resp = ctx
        .sdk
        .streams
        .get_enabled_count(stream_type.as_deref())
        .await?;
    if ctx.out.format.is_structured() {
        crate::output::emit(&ctx.out, &resp)
    } else {
        println!("{}", resp.total);
        Ok(())
    }
}
