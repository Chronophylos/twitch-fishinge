use std::collections::HashSet;

use bot_framework::runner::{start_bot, Client, Config};
use futures::future::FutureExt;
use miette::{IntoDiagnostic, Result, WrapErr};
use sea_orm::DatabaseConnection;
use supinic_fish_bot::{handle_server_message, run};
use twitch_irc::message::ServerMessage;

#[inline]
fn env_var(name: &'static str) -> Result<String> {
    std::env::var(name)
        .into_diagnostic()
        .wrap_err_with(|| format!("env var {name} is not set"))
}

#[tokio::main]
async fn main() -> Result<()> {
    pretty_env_logger::init_timed();
    dotenvy::dotenv().ok();

    let (tx, rx) = tokio::sync::mpsc::channel(1);

    let wanted_channel = env_var("CHANNEL")?;
    let username = env_var("USERNAME")?;
    let client_id = env_var("CLIENT_ID")?;
    let client_secret = env_var("CLIENT_SECRET")?;
    let config = Config {
        wanted_channels: vec![wanted_channel].into_iter().collect::<HashSet<_>>(),
        username,
        client_id,
        client_secret,
    };

    start_bot(
        config,
        move |conn: DatabaseConnection, client: Client| run(conn, client, rx).boxed(),
        move |conn: DatabaseConnection, client: Client, message: ServerMessage| {
            handle_server_message(conn, client, message, tx.clone()).boxed()
        },
    )
    .await
    .wrap_err("failed to run bot")
}
