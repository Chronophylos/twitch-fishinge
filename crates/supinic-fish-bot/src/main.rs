use bot_framework::runner::{start_bot, Client};
use futures::future::FutureExt;
use miette::{Result, WrapErr};
use sea_orm::DatabaseConnection;
use twitch_irc::message::ServerMessage;

#[tokio::main]
async fn main() -> Result<()> {
    pretty_env_logger::init_timed();
    dotenvy::dotenv().ok();

    let closure = |conn: DatabaseConnection, client: Client, message: ServerMessage| {
        handle_server_message(conn, client, message).boxed()
    };
    start_bot(closure).await.wrap_err("failed to run bot")
}

async fn handle_server_message(
    _conn: DatabaseConnection,
    _client: Client,
    _message: ServerMessage,
) -> Result<()> {
    Ok(())
}
