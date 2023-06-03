use std::{collections::HashSet, sync::Arc};

use database::connection;
use dotenvy::dotenv;
use log::{debug, error, info};
use miette::{Diagnostic, Result, WrapErr};
use sea_orm::DatabaseConnection;
use signal_hook::consts::signal::{SIGINT, SIGQUIT, SIGTERM};
use signal_hook_tokio::Signals;
use tokio::{select, sync::Notify};
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
}

#[async_trait::async_trait]
pub trait Bot {
    fn run(&self, db: &DatabaseConnection, quit_signal: Arc<Notify>) -> Result<()>;

    async fn handle_server_message(
        &self,
        db: &DatabaseConnection,
        client: &Client,
        message: ServerMessage,
    ) -> Result<()>;
}

pub struct BotRunner<B>
where
    B: Bot,
{
    signals: Signals,
    quit_signal: Arc<Notify>,
    database_connection: DatabaseConnection,
    signals_task: tokio::task::JoinHandle<()>,
    bot_task: tokio::task::JoinHandle<()>,
    bot: B,
}

impl<B> BotRunner<B>
where
    B: Bot + Send + Sync + Clone + 'static,
{
    pub async fn new(bot: B) -> Result<Self, Error>
    where
        B: Bot,
    {
        let signals = Signals::new(&[SIGINT, SIGTERM, SIGQUIT]).map_err(Error::RegisterSignals)?;
        let quit_signal = Arc::new(Notify::new());

        info!("connecting to database");
        let db = connection().await.map_err(Error::ConnectDatabase)?;
        let config = Self::create_client_config(&db).await?;

        info!("creating twitch client");
        let (mut incoming_messages, client) = Client::new(config);

        let twitch_task = tokio::spawn({
            let client = client.clone();
            let bot = bot.clone();

            async move {
                loop {
                    select! {
                        maybe_message = incoming_messages.recv() => {
                            if let Some(message) = maybe_message {
                                if let Err(err) = bot.handle_server_message(&db, &client, message).await {
                                    error!("Error handling message: {err}");
                                }

                            } else {
                                break;
                            }
                        }
                        _ = quit_signal.notified() => {
                            debug!("Received quitting twitch task");
                            break;
                        }
                    }
                }
            }
        });

        let wanted_channels = env_var("CHANNELS")?
            .split(',')
            .map(|channel| channel.trim().to_string())
            .collect::<HashSet<_>>();

        debug!(
            "Wanting to join channels {}",
            wanted_channels
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );

        client.set_wanted_channels(wanted_channels)?;

        todo!()
    }

    async fn create_client_config(
        db: &DatabaseConnection,
    ) -> Result<ClientConfig<RefreshingLoginCredentials<Account>>, Error> {
        let username = env_var("USERNAME")?;
        let client_id = env_var("CLIENT_ID")?;
        let client_secret = env_var("CLIENT_SECRET")?;

        info!("creating client config for {username}");

        let account = Account::new(db.clone(), &username)
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
}

#[inline]
fn env_var(name: &'static str) -> Result<String, Error> {
    std::env::var(name).map_err(|source| Error::EnvVarNotSet { source, name })
}
