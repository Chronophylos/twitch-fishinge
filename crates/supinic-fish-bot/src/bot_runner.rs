use std::{collections::HashSet, future::Future, pin::Pin, sync::Arc};

use database::connection;
use log::{debug, error, info};
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

#[derive(Debug, thiserror::Error, Diagnostic)]
pub enum Error {
    #[error("could not register signals")]
    #[diagnostic(code(bot_runner::register_signals))]
    RegisterSignals(#[source] std::io::Error),

    #[error("could not connect to database")]
    #[diagnostic(code(bot_runner::connect_database))]
    ConnectDatabase(#[source] database::Error),

    #[error("env var {name} is not set")]
    #[diagnostic(code(bot_runner::env_var_not_set))]
    EnvVarNotSet {
        source: std::env::VarError,
        name: &'static str,
    },

    #[error("could not get account")]
    #[diagnostic(code(bot_runner::get_account))]
    GetAccount(#[source] account::Error),

    #[error("could not set wanted channels")]
    #[diagnostic(code(bot_runner::set_wanted_channels))]
    SetWantedChannels(#[source] twitch_irc::validate::Error),

    #[error("failed to run twitch task")]
    #[diagnostic(code(bot_runner::twitch_task))]
    TwitchTask(#[source] tokio::task::JoinError),

    #[error("failed to run signals task")]
    #[diagnostic(code(bot_runner::signals_task))]
    SignalsTask(#[source] tokio::task::JoinError),
}

pub async fn start_bot<F>(handle_server_message: F) -> Result<()>
where
    F: Fn(
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

    let wanted_channels = env_var("CHANNELS")?
        .split(',')
        .map(|channel| channel.trim().to_string())
        .collect::<HashSet<_>>();

    let twitch_task =
        start_twitch_bot(conn.clone(), wanted_channels, quit, handle_server_message).await?;

    // keep the tokio executor alive.
    // If you return instead of waiting the background task will exit.
    twitch_task.await.map_err(Error::TwitchTask)?;

    // Terminate the signal stream.
    quit_handle.close();
    quit_task.await.map_err(Error::SignalsTask)?;

    Ok(())
}

fn register_signals() -> Result<(Arc<Notify>, signal_hook_tokio::Handle, JoinHandle<()>), Error> {
    info!("Registering signals");

    let signals = Signals::new(&[SIGINT, SIGTERM, SIGQUIT]).map_err(Error::RegisterSignals)?;
    let notify = Arc::new(Notify::new());

    let handle = signals.handle();
    let task = tokio::spawn(handle_signals(signals, notify.clone()));

    Ok((notify, handle, task))
}

async fn start_twitch_bot<F>(
    conn: DatabaseConnection,
    wanted_channels: HashSet<String>,
    quit: Arc<Notify>,
    handle_server_message: F,
) -> Result<JoinHandle<()>, Error>
where
    F: Fn(
            DatabaseConnection,
            Client,
            ServerMessage,
        ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'static>>
        + Send
        + Sync
        + 'static,
{
    info!("Creating twitch client");

    let config = create_client_config(&conn).await?;
    let (mut incoming_messages, client) = Client::new(config);

    let twitch_task = tokio::spawn({
        let client = client.clone();

        async move {
            loop {
                select! {
                    maybe_message = incoming_messages.recv() => {
                        if let Some(message) = maybe_message {
                            if let Err(err) = handle_server_message(conn.clone(), client.clone(), message).await {
                                error!("Error handling message: {err}");
                            }

                        } else {
                            break;
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

    Ok(twitch_task)
}

async fn create_client_config(
    conn: &DatabaseConnection,
) -> Result<ClientConfig<RefreshingLoginCredentials<Account>>, Error> {
    let username = env_var("USERNAME")?;
    let client_id = env_var("CLIENT_ID")?;
    let client_secret = env_var("CLIENT_SECRET")?;

    info!("creating client config for {username}");

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

#[inline]
fn env_var(name: &'static str) -> Result<String, Error> {
    std::env::var(name).map_err(|source| Error::EnvVarNotSet { source, name })
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
