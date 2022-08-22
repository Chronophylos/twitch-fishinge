#![forbid(unsafe_code)]

mod config;

use std::{fmt::Display, ops::Range, time::Duration as StdDuration};

use chrono::{Duration, NaiveDateTime, Utc};
use log::{debug, error, info, warn};
use once_cell::sync::Lazy;
use rand::{seq::SliceRandom, Rng};
use regex::Regex;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode},
    ConnectOptions, Connection, SqliteConnection,
};
use twitch_irc::{
    login::RefreshingLoginCredentials,
    message::{PrivmsgMessage, ServerMessage},
    SecureTCPTransport, TwitchIRCClient,
};

use crate::config::Config;

type Client = TwitchIRCClient<SecureTCPTransport, RefreshingLoginCredentials<Config>>;

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("Could not use settings")]
    Settings(#[from] settings::Error),

    #[error("Could not validate channel name")]
    ValidateChannelName(#[from] twitch_irc::validate::Error),

    #[error("Could not connect to database")]
    ConnectToDatabase(#[source] sqlx::Error),

    #[error("Could not close database connection")]
    CloseDatabseConnection(#[source] sqlx::Error),

    #[error("Could not query user")]
    QueryUser(#[source] sqlx::Error),

    #[error("Could not create user")]
    CreateUser(#[source] sqlx::Error),

    #[error("Could not update user")]
    UpdateUser(#[source] sqlx::Error),

    #[error("Could not migrate database")]
    MigrateDatabase(#[from] sqlx::migrate::MigrateError),

    #[error("Could not reply to message")]
    ReplyToMessage(
        #[from] twitch_irc::Error<SecureTCPTransport, RefreshingLoginCredentials<Config>>,
    ),
}

static FISHES: [Fish; 19] = [
    Fish::new("👞", 500, 0, None),
    Fish::new("💣", 25, 0, None),
    Fish::new("🦆", 50, 0, Some(2.0..5.0)),
    Fish::new("🐸", 50, 0, None),
    Fish::new("🐚", 50, 10, None),
    Fish::new("🐢", 50, 30, Some(10.0..500.0)),
    Fish::new("🐟", 150, 20, Some(0.2..5.0)),
    Fish::new("🐠", 90, 30, Some(0.2..5.0)),
    Fish::new("🐡", 80, 40, Some(0.2..5.0)),
    Fish::new("🦑", 50, 50, None),
    Fish::new("🦈", 50, 150, Some(522.0..771.0)),
    Fish::new("🐯🦈", 10, 200, Some(175.0..635.0)),
    Fish::new("💀", 50, 50, None),
    Fish::new("💰", 10, 1000, Some(1.0..10.0)),
    Fish::new("🦀", 10, 400, Some(10.0..14.0)),
    Fish::new("🐳", 10, 800, Some(88_000.0..130_000.0)),
    Fish::new("FishMoley", 90, 100, Some(3.5..10.0)),
    Fish::new("Hhhehehe", 10, 200, None),
    Fish::new("FLOPPA", 1, 2000, None),
];
static TOTAL_FISHES: Lazy<u32> = Lazy::new(|| FISHES.iter().map(|f| f.count).sum());

static COOLDOWN: Lazy<Duration> = Lazy::new(|| Duration::hours(6));

// struct FishModel {
//     name: String,
//     count: u32,
//     max_value: u32,
//     min_weight: f32,
//     max_weight: f32,
//     is_trash: bool,
// }

#[derive(Debug, Clone)]
struct Fish {
    name: &'static str,
    count: u32,
    max_value: u32,
    weight_range: Option<Range<f32>>,
}

impl Fish {
    pub const fn new(
        name: &'static str,
        count: u32,
        value: u32,
        weight_range: Option<Range<f32>>,
    ) -> Self {
        Self {
            name,
            max_value: value,
            count,
            weight_range,
        }
    }

    pub fn catch(&self) -> Catch {
        let mut rng = rand::thread_rng();

        let weight = self
            .weight_range
            .clone()
            .map(|weight| rng.gen_range(weight));

        Catch::new(self, weight)
    }
}

//impl From<FishModel> for Fish {
//    fn from(fish: FishModel) -> Self {
//        Self {
//            name: &fish.name,
//            count: fish.count,
//            max_value: fish.max_value,
//            weight_range: Some(fish.min_weight..fish.max_weight),
//        }
//    }
//}

impl Display for Fish {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({:.1}%)",
            self.name,
            self.count as f32 / *TOTAL_FISHES as f32 * 100.0
        )?;

        if let Some(weight) = &self.weight_range {
            write!(f, " ({:.1}kg - {:.1}kg)", weight.start, weight.end)?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone)]
struct Catch<'a> {
    fish: &'a Fish,
    weight: Option<f32>,
}

impl<'a> Catch<'a> {
    pub fn new(fish: &'a Fish, weight: Option<f32>) -> Self {
        Self { fish, weight }
    }

    pub fn value(&self) -> f32 {
        let weight_multiplier = self
            .fish
            .weight_range
            .as_ref()
            .and_then(|range| {
                self.weight
                    .map(|weight| (weight - range.start) / (range.end - range.start))
            })
            .unwrap_or(1.0)
            * 2.0;

        self.fish.max_value as f32 * weight_multiplier
    }
}

impl Display for Catch<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.fish.name)?;
        if let Some(weight) = self.weight {
            write!(f, " ({:.1}kg)", weight)?;
        }
        write!(f, " worth ${:.2}", self.value())?;

        Ok(())
    }
}

async fn connect_to_database() -> Result<SqliteConnection, Error> {
    debug!("Connecting to database");
    SqliteConnectOptions::new()
        .filename("fish.db")
        .journal_mode(SqliteJournalMode::Wal)
        .create_if_missing(true)
        .connect()
        .await
        .map_err(Error::ConnectToDatabase)
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    pretty_env_logger::init();

    let mut conn = connect_to_database().await?;

    info!("Running Migrations");
    sqlx::migrate!().run(&mut conn).await?;

    conn.close().await.map_err(Error::CloseDatabseConnection)?;

    let settings = Config::load()?;
    let config = settings.client_config();

    info!("Creating client");
    let (mut incoming_messages, client) = Client::new(config);

    // consume the incoming messages stream
    let twitch_handle = tokio::spawn({
        let client = client.clone();

        async move {
            while let Some(message) = incoming_messages.recv().await {
                match message {
                    ServerMessage::Privmsg(msg) => {
                        if let Err(err) = handle_privmsg(&client, dbg!(&msg)).await {
                            error!("Error handling privmsg: {:#?}", err);
                        }
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
            }
        }
    });

    client.set_wanted_channels(settings.channels.clone())?;

    // keep the tokio executor alive.
    // If you return instead of waiting the background task will exit.
    twitch_handle.await.unwrap();

    Ok(())
}

#[allow(dead_code)]
#[derive(Debug)]
struct UserModel {
    id: i64,
    name: String,
    last_fished: NaiveDateTime,
    score: f64,
    is_bot: bool,
}

static COMMAND_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^((?P<emote>\S+)\s+)?Fishinge( (?P<args>.*))?$").unwrap());

async fn handle_privmsg(client: &Client, msg: &PrivmsgMessage) -> Result<(), Error> {
    if let Some(captures) = dbg!(COMMAND_REGEX.captures(dbg!(&msg.message_text))) {
        match dbg!(captures.name("emote").map(|m| m.as_str())) {
            Some("🐱") => {
                client
                    .say_in_reply_to(msg, "No catfishing!".to_string())
                    .await
                    .map_err(Error::ReplyToMessage)?;

                Ok(())
            }
            Some("🔍") => {
                client
                    .say_in_reply_to(
                        msg,
                        format!(
                            "you can catch the following fish: {}",
                            FISHES
                                .iter()
                                .map(Fish::to_string)
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    )
                    .await
                    .map_err(Error::ReplyToMessage)?;

                Ok(())
            }
            Some("🏆") => {
                let mut conn = connect_to_database().await?;
                let users = sqlx::query_as!(UserModel, "SELECT * FROM users ORDER BY score DESC")
                    .fetch_all(&mut conn)
                    .await
                    .map_err(Error::QueryUser)?;

                conn.close().await.map_err(Error::CloseDatabseConnection)?;

                let users = users
                    .iter()
                    .take(10)
                    .filter(|user| user.score > 0.0)
                    .filter(|user| !user.is_bot)
                    .enumerate()
                    .map(|(id, user)| format!("{}. {} - ${:.2}", id + 1, user.name, user.score))
                    .collect::<Vec<_>>();

                client
                    .say_in_reply_to(
                        msg,
                        format!("the top 10 fishers are: {}", users.join(" · ")),
                    )
                    .await
                    .map_err(Error::ReplyToMessage)?;

                Ok(())
            }
            Some("🤖") => {
                if dbg!(&msg.sender.login) != "chronophylos" {
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

                    let mut conn = connect_to_database().await?;
                    let epoch = NaiveDateTime::from_timestamp(0, 0);

                    sqlx::query!(
                    r#"
                    INSERT OR IGNORE INTO users (name, last_fished, is_bot, score) VALUES (?, ?, true, 0);
                    UPDATE users SET is_bot = true WHERE name = ?;
                    "#,
                    target,
                    epoch,
                    target
                ).execute(&mut conn).await.map_err(Error::UpdateUser)?;

                    client
                        .say_in_reply_to(msg, format!("designated {} as bot", target))
                        .await
                        .map_err(Error::ReplyToMessage)?;
                }

                Ok(())
            }
            None => handle_fishinge(client, msg).await,
            _ => Ok(()),
        }
    } else {
        Ok(())
    }
}

async fn handle_fishinge(client: &Client, msg: &PrivmsgMessage) -> Result<(), Error> {
    let now = Utc::now().naive_utc();

    let mut conn = connect_to_database().await?;

    // get user from database
    let id = if let Some(user) = sqlx::query_as!(
        UserModel,
        "SELECT * FROM users WHERE name = ?",
        msg.sender.login
    )
    .fetch_optional(&mut conn)
    .await
    .map_err(Error::QueryUser)?
    {
        // cooldown
        let cooled_off = user.last_fished + *COOLDOWN;
        if cooled_off > now {
            let cooldown = humantime::format_duration(StdDuration::from_secs(
                (cooled_off - now).num_seconds() as u64,
            ));
            client
                .say_in_reply_to(msg, format!("you just fished! Try again in {cooldown}."))
                .await
                .map_err(Error::ReplyToMessage)?;
            return Ok(());
        }
        user.id
    } else {
        // create user
        let id = sqlx::query!(
            "INSERT INTO users (name, last_fished, score) VALUES (?, ?, ?)",
            msg.sender.login,
            now,
            0
        )
        .execute(&mut conn)
        .await
        .map_err(Error::CreateUser)?;
        id.last_insert_rowid()
    };

    let fish = FISHES
        .choose_weighted(&mut rand::thread_rng(), |fish| fish.count)
        .unwrap();

    info!("{} is fishing for {fish}", msg.sender.name);

    let catch = fish.catch();

    info!("{} caught {catch}", msg.sender.name);

    let score = catch.value();
    sqlx::query!(
        "UPDATE users SET score = score + ?, last_fished = ? WHERE id = ?",
        score,
        now,
        id
    )
    .execute(&mut conn)
    .await
    .map_err(Error::UpdateUser)?;

    client
        .say_in_reply_to(msg, format!("caught a {catch}!"))
        .await
        .map_err(Error::ReplyToMessage)?;

    Ok(())
}
