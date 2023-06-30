use std::{collections::HashSet, future::Future, pin::Pin, sync::Arc};

use database::connection;
use log::{debug, error, info, trace};
use miette::{Diagnostic, Result};
use sea_orm::DatabaseConnection;
use signal_hook::consts::signal::{SIGINT, SIGQUIT, SIGTERM};
use signal_hook_tokio::Signals;
use tokio::{select, sync::Notify, task::JoinHandle};
use tokio_stream::StreamExt;
use twitch_irc::{
    login::RefreshingLoginCredentials, message::ServerMessage, ClientConfig, SecureTCPTransport,
    TwitchIRCClient,
};

use crate::account::{self, Account};

pub type Client = TwitchIRCClient<SecureTCPTransport, RefreshingLoginCredentials<Account>>;
pub type IrcError = twitch_irc::Error<SecureTCPTransport, RefreshingLoginCredentials<Account>>;

#[derive(Debug, thiserror::Error, Diagnostic)]
pub enum Error {
    #[error("could not register signals")]
    #[diagnostic(code(bot_runner::register_signals))]
    RegisterSignals(#[source] std::io::Error),

    #[error("could not connect to database")]
    #[diagnostic(code(bot_runner::connect_database))]
    ConnectDatabase(#[source] database::Error),

    #[error("could not get account")]
    #[diagnostic(code(bot_runner::get_account))]
    GetAccount(#[source] account::Error),

    #[error("could not set wanted channels")]
    #[diagnostic(code(bot_runner::set_wanted_channels))]
    SetWantedChannels(#[source] twitch_irc::validate::Error),

    #[error("failed to run twitch task")]
    #[diagnostic(code(bot_runner::twitch_task))]
    TwitchTask(#[source] tokio::task::JoinError),

    #[error("failed to run init task")]
    #[diagnostic(code(bot_runner::init_task))]
    InitTask(#[source] tokio::task::JoinError),

    #[error("failed to run signals task")]
    #[diagnostic(code(bot_runner::signals_task))]
    SignalsTask(#[source] tokio::task::JoinError),
}

#[derive(Debug, Clone)]
pub struct Config {
    pub wanted_channels: HashSet<String>,
    pub username: String,
    pub client_id: String,
    pub client_secret: String,
}

pub async fn start_bot<I, H>(config: Config, init: I, handle_server_message: H) -> Result<()>
where
    I: FnOnce(
            DatabaseConnection,
            Client,
        ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'static>>
        + Send
        + 'static,
    H: Fn(
            DatabaseConnection,
            Client,
            ServerMessage,
        ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'static>>
        + Send
        + Sync
        + 'static,
{
    let (quit, quit_handle, quit_task) = register_signals()?;

    info!("Connecting to database");
    let conn = connection().await.map_err(Error::ConnectDatabase)?;

    start_twitch_bot(conn.clone(), config, quit, init, handle_server_message).await?;

    // Terminate the signal stream.
    quit_handle.close();
    quit_task.await.map_err(Error::SignalsTask)?;

    Ok(())
}

fn register_signals() -> Result<(Arc<Notify>, signal_hook_tokio::Handle, JoinHandle<()>), Error> {
    info!("Registering signals");

    let signals = Signals::new([SIGINT, SIGTERM, SIGQUIT]).map_err(Error::RegisterSignals)?;
    let notify = Arc::new(Notify::new());

    let handle = signals.handle();
    let task = tokio::spawn(handle_signals(signals, notify.clone()));

    Ok((notify, handle, task))
}

async fn start_twitch_bot<I, H>(
    conn: DatabaseConnection,
    bot_config: Config,
    quit: Arc<Notify>,
    init: I,
    handle_server_message: H,
) -> Result<(), Error>
where
    I: FnOnce(
            DatabaseConnection,
            Client,
        ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'static>>
        + Send
        + 'static,
    H: Fn(
            DatabaseConnection,
            Client,
            ServerMessage,
        ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'static>>
        + Send
        + Sync
        + 'static,
{
    info!("Creating twitch client");

    let Config {
        wanted_channels,
        username,
        client_id,
        client_secret,
    } = bot_config;

    let client_config = create_client_config(&conn, username, client_id, client_secret).await?;
    let (mut incoming_messages, client) = Client::new(client_config);

    info!("Spawning init task");
    let init_task = tokio::spawn({
        let conn = conn.clone();
        let client = client.clone();

        async move {
            debug!("Running init task");
            if let Err(err) = init(conn, client).await {
                error!("Error initializing bot: {err}");
            }
        }
    });

    info!("Spawning twitch task");
    let twitch_task = tokio::spawn({
        let client = client.clone();

        async move {
            debug!("Starting message handler loop");
            loop {
                select! {
                    channel_value = incoming_messages.recv() => {
                        let Some(message) = channel_value else {
                            break;
                        };
                        if let Err(err) = handle_server_message(conn.clone(), client.clone(), message).await {
                            error!("Error handling message: {err}");
                        }
                    }
                    _ = quit.notified() => {
                        debug!("Received quitting twitch task");
                        break;
                    }
                }
            }
        }
    });

    debug!(
        "Setting wanted channels: {}",
        wanted_channels
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    client
        .set_wanted_channels(wanted_channels)
        .map_err(Error::SetWantedChannels)?;

    trace!("Waiting for twitch task and init task to finish");
    twitch_task.await.map_err(Error::TwitchTask)?;
    init_task.await.map_err(Error::InitTask)?;

    Ok(())
}

async fn create_client_config(
    conn: &DatabaseConnection,
    username: String,
    client_id: String,
    client_secret: String,
) -> Result<ClientConfig<RefreshingLoginCredentials<Account>>, Error> {
    info!("Creating client config for {username}");

    let account = Account::new(conn.clone(), &username)
        .await
        .map_err(Error::GetAccount)?;
    let credentials = RefreshingLoginCredentials::init_with_username(
        Some(username),
        client_id,
        client_secret,
        account,
    );
    let config = ClientConfig::new_simple(credentials);

    Ok(config)
}

async fn handle_signals(mut signals: Signals, quit_signal: Arc<Notify>) {
    info!("Starting signal handler");
    while let Some(signal) = signals.next().await {
        match signal {
            SIGTERM | SIGINT | SIGQUIT => {
                // Shutdown the system
                quit_signal.notify_waiters();
                break;
            }
            _ => unreachable!(),
        }
    }
}
