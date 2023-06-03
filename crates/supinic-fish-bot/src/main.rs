use miette::{Result, WrapErr};
use supinic_fish_bot::bot_runner::BotRunner;

#[tokio::main]
async fn main() -> Result<()> {
    pretty_env_logger::init_timed();
    dotenvy::dotenv().ok();

    start().await.wrap_err("failed to run bot")
}

async fn start() -> Result<()> {
    BotRunner::new().await?;

    Ok(())
}
