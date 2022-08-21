#![forbid(unsafe_code)]

mod config;

use std::{
    fmt::Display, fs::create_dir_all, ops::Range, path::PathBuf, time::Duration as StdDuration,
};

use chrono::{Duration, NaiveDateTime, Utc};
use log::{debug, error, info, trace, warn};
use once_cell::sync::{Lazy, OnceCell};
use rand::{seq::SliceRandom, Rng};
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

    #[error("Could not create data dir")]
    CreateDataDir(#[source] std::io::Error),

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

const SLOTS: u32 = 1000;
static FISHES: Lazy<Vec<Fish>> = Lazy::new(|| {
    let vec = vec![
        Fish::new("👞", 400, 0, None),
        Fish::new("💣", 200, 0, None),
        Fish::new("🦆", 150, 10, Some(2.0..5.0)),
        Fish::new("🐟", 100, 20, Some(0.2..5.0)),
        Fish::new("💀", 50, 50, None),
        Fish::new("FishMoley", 90, 100, Some(3.5..10.0)),
        Fish::new("Hhhehehe", 10, 200, None),
    ];

    assert_eq!(
        vec.iter().map(|f| f.weight).sum::<u32>(),
        SLOTS,
        "Weights do not add up to 1000"
    );

    vec
});

#[derive(Debug, Clone)]
struct Fish {
    name: String,
    weight: u32,
    max_value: u32,
    weight_range: Option<Range<f32>>,
}

impl Fish {
    pub fn new(name: &str, weight: u32, value: u32, weight_range: Option<Range<f32>>) -> Self {
        Self {
            name: name.to_string(),
            max_value: value,
            weight,
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

impl Display for Fish {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({:.1}%)",
            self.name,
            self.weight as f32 / SLOTS as f32 * 100.0
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
        write!(f, " for ${:.2}", self.value())?;

        Ok(())
    }
}

static DATABASE_PATH: OnceCell<PathBuf> = OnceCell::new();

async fn connect_to_database() -> Result<SqliteConnection, Error> {
    let path = DATABASE_PATH.get_or_try_init(|| {
        let settings = Config::load()?;

        let data_dir = settings.project_dirs().data_dir();

        trace!("Creating data dir {data_dir:?}");
        create_dir_all(data_dir).map_err(Error::CreateDataDir)?;

        Result::<PathBuf, Error>::Ok(data_dir.join("fish.db"))
    })?;

    debug!("Connecting to database");
    SqliteConnectOptions::new()
        .filename(path)
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
                        if let Err(err) = handle_privmsg(&client, &msg).await {
                            error!("Error handling privmsg: {}", err);
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
}

async fn handle_privmsg(client: &Client, msg: &PrivmsgMessage) -> Result<(), Error> {
    // TODO: add response to 🐱 Fishinge responding with "No catfishing!"

    match msg.message_text.replace("  ", " ").trim() {
        "🐱 Fishinge" => {
            client
                .say_in_reply_to(msg, "No catfishing!".to_string())
                .await
                .map_err(Error::ReplyToMessage)?;

            return Ok(());
        }
        "🔍 Fishinge" => {
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

            return Ok(());
        }
        "🏆 Fishinge" => {
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

            return Ok(());
        }
        _ => {}
    }

    if !msg.message_text.starts_with("Fishinge") {
        return Ok(());
    }

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
        let cooled_off = user.last_fished + Duration::hours(6);
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
        .choose_weighted(&mut rand::thread_rng(), |fish| fish.weight)
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
