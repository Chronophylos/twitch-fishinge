#![forbid(unsafe_code)]

use std::{
    collections::HashSet,
    env,
    fmt::Display,
    ops::Range,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
    time::Duration as StdDuration,
};

use async_trait::async_trait;
use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use database::{
    connection,
    entities::{
        accounts, catches, fishes, messages, prelude::*, sea_orm_active_enums::MessageType, users,
    },
    migrate,
};
use dotenvy::dotenv;
use eyre::WrapErr;
use futures_lite::stream::StreamExt;
use log::{debug, error, info, trace, warn};
use once_cell::sync::Lazy;
use rand::{rngs::StdRng, seq::SliceRandom, thread_rng, Rng, SeedableRng};
use regex::Regex;
use sea_orm::{
    sea_query::OnConflict, ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection,
    DeriveColumn, EntityTrait, EnumIter, FromQueryResult, IdenStatic, QueryFilter, QueryOrder,
    QuerySelect,
};
use signal_hook::consts::*;
use signal_hook_tokio::Signals;
use tokio::{select, sync::Notify};
use twitch_irc::{
    login::{RefreshingLoginCredentials, TokenStorage, UserAccessToken},
    message::{PrivmsgMessage, ServerMessage},
    ClientConfig, SecureTCPTransport, TwitchIRCClient,
};

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("Could not use settings")]
    Settings(#[from] settings::Error),

    #[error("Could not validate channel name")]
    ValidateChannelName(#[from] twitch_irc::validate::Error),

    #[error("Could not open database connection")]
    OpenDatabase(#[from] database::Error),

    #[error("Database error")]
    Database(#[from] sea_orm::DbErr),

    #[error("Could not reply to message")]
    ReplyToMessage(
        #[from] twitch_irc::Error<SecureTCPTransport, RefreshingLoginCredentials<Account>>,
    ),

    #[error("No fishes found in database")]
    NoFishesInDatabase,

    #[error("No cooldown messages found in database")]
    NoCooldownMessages,

    #[error("Account `{0}` not found in database")]
    AccountNotFound(String),

    #[error("Could not join thread")]
    JoinThread(#[from] tokio::task::JoinError),

    #[error("Environent variable {name} not set")]
    EnvarNotSet {
        source: std::env::VarError,
        name: &'static str,
    },

    #[error("Signal hooking error")]
    Signals(#[source] std::io::Error),
}

static FISH_POPULATION: RwLock<i32> = RwLock::new(0);
static COOLDOWN: Lazy<Duration> = Lazy::new(|| Duration::hours(4));

#[derive(Debug, Clone)]
struct Fish {
    id: i32,
    name: String,
    count: u32,
    base_value: i32,
    weight_range: Option<Range<f32>>,
}

impl Fish {
    pub fn catch(&self) -> Catch {
        let mut rng = rand::thread_rng();

        let weight = self
            .weight_range
            .clone()
            .map(|weight| rng.gen_range(weight));

        Catch::new(self, weight)
    }
}

impl From<database::entities::fishes::Model> for Fish {
    fn from(fish: database::entities::fishes::Model) -> Self {
        Self {
            id: fish.id,
            name: fish.name,
            count: fish.count as u32,
            base_value: fish.base_value as i32,
            weight_range: if fish.min_weight > f32::EPSILON && fish.max_weight > f32::EPSILON {
                Some(fish.min_weight..fish.max_weight)
            } else {
                None
            },
        }
    }
}

impl Display for Fish {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({:.1}%)",
            self.name,
            self.count as f32 / *FISH_POPULATION.read().unwrap() as f32 * 100.0
        )?;

        if let Some(weight) = &self.weight_range {
            write!(f, " ({:.1}kg - {:.1}kg)", weight.start, weight.end)?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone)]
struct Catch {
    fish_name: String,
    weight: Option<f32>,
    value: f32,
}

impl Catch {
    pub fn new(fish: &Fish, weight: Option<f32>) -> Self {
        let multiplier = fish
            .weight_range
            .as_ref()
            .and_then(|range| {
                weight.map(|weight| (weight - range.start) / (range.end - range.start))
            })
            .map_or(1.0, |x| (x * 1.36 - 0.48).powi(3) + 1.01 + x * 0.11);

        Self {
            fish_name: fish.name.clone(),
            weight,
            value: fish.base_value as f32 * multiplier,
        }
    }
}

#[cfg(test)]
mod tests {
    use approx::assert_ulps_eq;
    use test_case::test_case;

    use super::*;

    #[test_case(Some(0.0..1.0), 100, 0.0, 89.940796 ; "range 0.0 to 1.0 with base value 100 and weight 0.0")]
    #[test_case(Some(0.0..1.0), 100, 0.5, 107.299995 ; "range 0.0 to 1.0 with base value 100 and weight 0.5")]
    #[test_case(Some(0.0..1.0), 100, 1.0, 180.1472 ; "range 0.0 to 1.0 with base value 100 and weight 1.0")]
    #[test_case(Some(0.0..1.0), 100, 1.1, 217.97722 ; "range 0.0 to 1.0 with base value 100 and over weight 1.1")]
    #[test_case(Some(0.0..1.0), -100, 0.0, -89.940796 ; "range 0.0 to 1.0 with negative base value -100 and weight 0.0")]
    #[test_case(Some(0.0..1.0), -100, 0.5, -107.299995 ; "range 0.0 to 1.0 with negative base value -100 and weight 0.5")]
    #[test_case(Some(0.0..1.0), -100, 1.0, -180.1472 ; "range 0.0 to 1.0 with negative base value -100 and weight 1.0")]
    #[test_case(Some(5.3..12.6), 123, 5.3, 110.62718 ; "range 5.3 to 12.6 with base value 123 and weight 5.3")]
    #[test_case(Some(5.3..12.6), 123, 8.95, 131.97899 ; "range 5.3 to 12.6 with base value 123 and weight 8.95")]
    #[test_case(Some(5.3..12.6), 123, 12.6, 221.58107 ; "range 5.3 to 12.6 with base value 123 and weight 12.6")]
    #[test_case(Some(88000.0..130000.0), 800, 91961.3 , 781.4889 ; "range 88000.0 to 130000.0 with base value 800 and weight 91961.3")]
    #[test_case(None, -50, 0.0, -50.0 ; "without range with base value -50 and weight 0.0")]
    #[test_case(None, -50, 100.0, -50.0 ; "without range with base value -50 and weight 100.0")]
    fn catch_value(
        weight_range: Option<Range<f32>>,
        base_value: i32,
        weight: f32,
        expected_value: f32,
    ) {
        let fish = Fish {
            id: 0,
            name: String::new(),
            count: 0,
            base_value,
            weight_range,
        };
        let catch = Catch::new(&fish, Some(weight));
        assert_ulps_eq!(catch.value, expected_value, max_ulps = 4);
    }
}

impl Display for Catch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.fish_name)?;
        if let Some(weight) = self.weight {
            write!(f, " ({:.1}kg)", weight)?;
        }
        if self.value < f32::EPSILON {
            write!(f, " worth nothing")?;
        } else {
            write!(f, " worth ${:.2}", self.value)?;
        }

        Ok(())
    }
}

#[derive(Debug)]
struct Account {
    id: i32,
    db: DatabaseConnection,
}

impl Account {
    pub async fn new(db: DatabaseConnection, username: &str) -> Result<Self, Error> {
        #[derive(FromQueryResult)]
        struct AccountId {
            id: i32,
        }

        let id = Accounts::find()
            .filter(accounts::Column::Username.eq(username))
            .select_only()
            .column(accounts::Column::Id)
            .into_model::<AccountId>()
            .one(&db)
            .await?
            .ok_or_else(|| Error::AccountNotFound(username.to_string()))?
            .id;

        Ok(Self { id, db })
    }
}

#[async_trait]
impl TokenStorage for Account {
    type LoadError = eyre::Error;
    type UpdateError = eyre::Error;

    async fn load_token(&mut self) -> Result<UserAccessToken, Self::LoadError> {
        let account = Accounts::find_by_id(self.id)
            .one(&self.db)
            .await
            .wrap_err("Could not query account")?
            .ok_or_else(|| eyre::eyre!("Account not found"))?;

        Ok(UserAccessToken {
            access_token: account.access_token,
            refresh_token: account.refresh_token,
            created_at: account.created_at.into(),
            expires_at: account.expires_at.map(Into::into),
        })
    }

    async fn update_token(&mut self, token: &UserAccessToken) -> Result<(), Self::UpdateError> {
        let account = accounts::ActiveModel {
            id: ActiveValue::unchanged(self.id),
            access_token: ActiveValue::set(token.access_token.clone()),
            refresh_token: ActiveValue::set(token.refresh_token.clone()),
            created_at: ActiveValue::set(token.created_at.into()),
            expires_at: ActiveValue::set(token.expires_at.map(Into::into)),
            ..Default::default()
        };

        account
            .update(&self.db)
            .await
            .wrap_err("Could not update account")?;

        Ok(())
    }
}

type Client = TwitchIRCClient<SecureTCPTransport, RefreshingLoginCredentials<Account>>;

static QUITTING: AtomicBool = AtomicBool::new(false);

async fn handle_signals(mut signals: Signals, quit_signal: Arc<Notify>) {
    info!("Starting signal handler");
    while let Some(signal) = signals.next().await {
        match signal {
            SIGTERM | SIGINT | SIGQUIT => {
                // Shutdown the system
                QUITTING.store(true, Ordering::Relaxed);
                quit_signal.notify_waiters();
                break;
            }
            _ => unreachable!(),
        }
    }
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    pretty_env_logger::init_timed();
    dotenv().ok();

    Ok(run().await?)
}

#[inline]
fn env_var(name: &'static str) -> Result<String, Error> {
    env::var(name).map_err(|source| Error::EnvarNotSet { source, name })
}

async fn run() -> Result<(), Error> {
    let signals = Signals::new(&[SIGTERM, SIGINT, SIGQUIT]).map_err(Error::Signals)?;
    let quit_signal = Arc::new(Notify::new());

    let db = connection().await?;

    info!("Running Migrations");
    migrate(&db).await?;

    let username = env_var("USERNAME")?;
    let client_id = env_var("CLIENT_ID")?;
    let client_secret = env_var("CLIENT_SECRET")?;
    let account = Account::new((&db).clone(), &username).await?;
    let credentials = RefreshingLoginCredentials::init_with_username(
        Some(username),
        client_id,
        client_secret,
        account,
    );
    let config = ClientConfig::new_simple(credentials);

    info!("Creating client");
    let (mut incoming_messages, client) = Client::new(config);

    let handle = signals.handle();
    let signals_task = tokio::spawn(handle_signals(signals, quit_signal.clone()));

    // consume the incoming messages stream
    let twitch_task = tokio::spawn({
        let client = client.clone();

        async move {
            while !QUITTING.load(Ordering::Relaxed) {
                select! {
                    maybe_message = incoming_messages.recv() => {
                        if let Some(message) = maybe_message {
                            if let Err(err)=handle_server_message(&db, &client, message).await {
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

    // keep the tokio executor alive.
    // If you return instead of waiting the background task will exit.
    twitch_task.await?;

    // Terminate the signal stream.
    handle.close();
    signals_task.await?;

    Ok(())
}

async fn handle_server_message(
    db: &DatabaseConnection,
    client: &Client,
    message: ServerMessage,
) -> Result<(), Error> {
    trace!("Received message: {:?}", &message);

    match message {
        ServerMessage::Privmsg(msg) => {
            handle_privmsg(db, client, &msg).await?;
        }
        ServerMessage::Notice(msg) => {
            warn!(
                "Notice: {} {}",
                msg.channel_login.unwrap_or_else(|| "Server".to_string()),
                msg.message_text
            );
        }
        ServerMessage::Reconnect(_) => {
            info!("Twitch Server requested a reconnect");
        }
        _ => {}
    }
    Ok(())
}

static COMMAND_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^((?P<emote>\S+)\s+)?Fishinge( (?P<args>.*))?$").unwrap());
const WEB_URL: &str = "https://fishinge.chronophylos.com";

async fn get_fishes(db: &DatabaseConnection) -> Result<Vec<Fish>, Error> {
    let fishes = Fishes::find().all(db).await?;

    let population = fishes.iter().map(|fish| fish.count).sum();

    *FISH_POPULATION.write().unwrap() = population;

    Ok(fishes.into_iter().map(Fish::from).collect())
}

async fn handle_privmsg(
    db: &DatabaseConnection,
    client: &Client,
    msg: &PrivmsgMessage,
) -> Result<(), Error> {
    if msg.message_text.starts_with("!bot") {
        client
            .say_in_reply_to(
                msg,
                "this micro bot allows you to fish. Type `❓ Fishinge` for help.".to_string(),
            )
            .await
            .map_err(Error::ReplyToMessage)?;

        return Ok(());
    }

    if let Some(captures) = COMMAND_REGEX.captures(&msg.message_text) {
        match captures.name("emote").map(|m| m.as_str()) {
            Some("🐱") => {
                client
                    .say_in_reply_to(msg, "No catfishing!".to_string())
                    .await
                    .map_err(Error::ReplyToMessage)?;

                Ok(())
            }
            Some("🔍") | Some("🔎") => {
                client
                    .say_in_reply_to(msg, format!("fishes are here {WEB_URL}/fishes"))
                    .await
                    .map_err(Error::ReplyToMessage)?;

                Ok(())
            }
            Some("🏆") => {
                client
                    .say_in_reply_to(
                        msg,
                        format!("check out the leaderboard at {WEB_URL}/leaderboard"),
                    )
                    .await
                    .map_err(Error::ReplyToMessage)?;

                Ok(())
            }
            Some("🤖") => {
                if &msg.sender.login != "chronophylos" {
                    return Ok(());
                }

                if let Some(args) = captures.name("args") {
                    let target = args
                        .as_str()
                        .split_whitespace()
                        .next()
                        .unwrap()
                        .trim_start_matches('@')
                        .to_lowercase();

                    let epoch =
                        DateTime::<Utc>::from_utc(NaiveDateTime::from_timestamp(61, 0), Utc).into();

                    let user = users::ActiveModel {
                        name: ActiveValue::set(target.to_string()),
                        is_bot: ActiveValue::set(true),
                        last_fished: ActiveValue::set(epoch),
                        ..Default::default()
                    };

                    users::Entity::insert(user)
                        .on_conflict(
                            // on conflict do update
                            OnConflict::column(users::Column::Name)
                                .update_column(users::Column::IsBot)
                                .to_owned(),
                        )
                        .exec(db)
                        .await?;

                    client
                        .say_in_reply_to(msg, format!("designated {} as bot", target))
                        .await
                        .map_err(Error::ReplyToMessage)?;
                }

                Ok(())
            }
            Some("❓") => {
                client
                    .say_in_reply_to(msg, format!("the list of commands is here {WEB_URL}"))
                    .await
                    .map_err(Error::ReplyToMessage)?;

                Ok(())
            }
            Some("💎") => {
                let query: Option<(catches::Model, Option<fishes::Model>)> = Catches::find()
                    .inner_join(Users)
                    .filter(users::Column::Name.eq(msg.sender.login.to_lowercase()))
                    .order_by_desc(catches::Column::Value)
                    .find_also_related(Fishes)
                    .one(db)
                    .await?;

                if let Some((catch_model, Some(fish_model))) = query {
                    let catch = Catch {
                        fish_name: fish_model.name,
                        weight: catch_model.weight,
                        value: catch_model.value,
                    };

                    client
                        .say_in_reply_to(msg, format!("your most valuable catch is {}", catch))
                        .await
                        .map_err(Error::ReplyToMessage)?;
                } else {
                    client
                        .say_in_reply_to(msg, "you did not catch any fish yet".to_string())
                        .await
                        .map_err(Error::ReplyToMessage)?;
                };

                Ok(())
            }
            None => handle_fishinge(db, client, msg).await,
            _ => Ok(()),
        }
    } else {
        Ok(())
    }
}

async fn handle_fishinge(
    db: &DatabaseConnection,
    client: &Client,
    msg: &PrivmsgMessage,
) -> Result<(), Error> {
    let now = Utc::now().into();
    // TODO: remove unwrap
    let mut rng = StdRng::from_rng(thread_rng()).unwrap();

    // get user from database
    let user = if let Some(user) = Users::find()
        .filter(users::Column::Name.eq(msg.sender.login.to_lowercase()))
        .one(db)
        .await?
    {
        // cooldown
        let cooled_off = user.last_fished + *COOLDOWN;
        if cooled_off > now {
            let cooldown = humantime::format_duration(StdDuration::from_secs(
                (cooled_off - now).num_seconds() as u64,
            ));

            let mut biased_rng = StdRng::seed_from_u64(user.last_fished.timestamp() as u64);

            #[derive(Copy, Clone, Debug, EnumIter, DeriveColumn)]
            enum QueryAs {
                Text,
            }

            let messages: Vec<String> = Messages::find()
                .filter(messages::Column::Type.eq(MessageType::Cooldown))
                .into_values::<_, QueryAs>()
                .all(db)
                .await?;

            if messages.is_empty() {
                return Err(Error::NoCooldownMessages);
            }

            let message = messages
                .choose(&mut biased_rng)
                .unwrap()
                .replace("{cooldown}", &cooldown.to_string());

            client
                .say_in_reply_to(msg, message)
                .await
                .map_err(Error::ReplyToMessage)?;

            return Ok(());
        }
        users::ActiveModel {
            last_fished: ActiveValue::set(now),
            ..user.into()
        }
        .update(db)
        .await?
    } else {
        // create user
        let user = users::ActiveModel {
            name: ActiveValue::set(msg.sender.login.to_lowercase()),
            last_fished: ActiveValue::set(now),
            is_bot: ActiveValue::set(false),
            ..Default::default()
        };
        user.insert(db).await?
    };

    let fishes = get_fishes(db).await?;

    if fishes.is_empty() {
        return Err(Error::NoFishesInDatabase);
    }

    let fish = fishes.choose_weighted(&mut rng, |fish| fish.count).unwrap();

    info!("{} is fishing for {fish}", msg.sender.name);

    let catch = fish.catch();

    info!("{} caught {catch}", msg.sender.name);

    catches::ActiveModel {
        user_id: ActiveValue::set(user.id),
        fish_id: ActiveValue::set(fish.id),
        weight: ActiveValue::set(catch.weight),
        caught_at: ActiveValue::set(now),
        value: ActiveValue::set(catch.value),
        ..Default::default()
    }
    .insert(db)
    .await?;

    client
        .say_in_reply_to(msg, format!("caught a {catch}!"))
        .await
        .map_err(Error::ReplyToMessage)?;

    Ok(())
}
