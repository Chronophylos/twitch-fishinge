#![forbid(unsafe_code)]

use std::{
    collections::HashSet,
    env,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration as StdDuration,
};

use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use database::{
    connection,
    entities::{catches, fishes, messages, prelude::*, sea_orm_active_enums::MessageType, users},
    migrate,
};
use dotenvy::dotenv;
use eyre::{eyre, Result, WrapErr};
use fishinge_bot::{get_active_season, get_fishes, Account, Catch};
use futures_lite::stream::StreamExt;
use log::{debug, error, info, trace, warn};
use once_cell::sync::Lazy;
use rand::{rngs::StdRng, seq::SliceRandom, thread_rng, SeedableRng};
use regex::Regex;
use sea_orm::{
    sea_query::OnConflict, ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection,
    DeriveColumn, EntityTrait, EnumIter, QueryFilter, QueryOrder, QuerySelect,
};
use signal_hook::consts::*;
use signal_hook_tokio::Signals;
use tokio::{select, sync::Notify};
use twitch_irc::{
    login::RefreshingLoginCredentials,
    message::{PrivmsgMessage, ServerMessage},
    ClientConfig, SecureTCPTransport, TwitchIRCClient,
};

#[derive(Debug, thiserror::Error)]
enum Error {
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
async fn main() -> Result<()> {
    pretty_env_logger::init_timed();
    dotenv().ok();

    run().await.wrap_err("failed to run bot")
}

#[inline]
fn env_var(name: &'static str) -> Result<String, Error> {
    env::var(name).map_err(|source| Error::EnvarNotSet { source, name })
}

async fn run() -> Result<()> {
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
) -> Result<()> {
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

async fn handle_privmsg(
    db: &DatabaseConnection,
    client: &Client,
    msg: &PrivmsgMessage,
) -> Result<()> {
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
            Some("💰") => {
                #[derive(Copy, Clone, Debug, EnumIter, DeriveColumn)]
                enum QueryAs {
                    Score,
                }

                let query: Option<f32> = Catches::find()
                    .inner_join(Users)
                    .filter(users::Column::Name.eq(msg.sender.login.to_lowercase()))
                    .select_only()
                    .column_as(catches::Column::Value.sum(), "score")
                    .into_values::<_, QueryAs>()
                    .one(db)
                    .await?
                    .flatten();

                if let Some(score) = query {
                    client
                        .say_in_reply_to(msg, format!("your current score is ${score:.2}"))
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

pub static COOLDOWN: Lazy<Duration> = Lazy::new(|| Duration::hours(4));

async fn handle_fishinge(
    db: &DatabaseConnection,
    client: &Client,
    msg: &PrivmsgMessage,
) -> Result<()> {
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
                return Err(eyre!("no cooldown messages found in database"));
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

    let season = get_active_season(db).await?;
    let fishes = get_fishes(db, &season).await?;

    if fishes.is_empty() {
        return Err(eyre!("no fishes found in database"));
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
        season_id: ActiveValue::set(season.id),
        ..Default::default()
    }
    .insert(db)
    .await?;

    client
        .say_in_reply_to(msg, format!("caught a {catch}!"))
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::ops::Range;

    use approx::assert_ulps_eq;
    use fishinge_bot::Fish;
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

    #[test_case(Catch{ fish_name: "fish".to_string(), weight: None, value: 0.0 }, "fish worth nothing" ; "without weight worth nothing")]
    #[test_case(Catch{ fish_name: "fish".to_string(), weight: None, value: -50.0 }, "fish worth $-50.00" ; "without weight with negative worth")]
    #[test_case(Catch{ fish_name: "fish".to_string(), weight: None, value: 50.0 }, "fish worth $50.00" ; "without weight with positive worth")]
    #[test_case(Catch{ fish_name: "fish".to_string(), weight: Some(1.23), value: 0.0 }, "fish (1.2kg) worth nothing" ; "with weight worth nothing")]
    #[test_case(Catch{ fish_name: "fish".to_string(), weight: Some(1.23), value: -50.0 }, "fish (1.2kg) worth $-50.00" ; "with weight with negative worth")]
    #[test_case(Catch{ fish_name: "fish".to_string(), weight: Some(1.23), value: 50.0 }, "fish (1.2kg) worth $50.00" ; "with weight with positive worth")]
    fn catch_format(catch: Catch, expected: &str) {
        assert_eq!(catch.to_string(), expected);
    }
}
