#![forbid(unsafe_code)]

use std::{fmt::Display, ops::Range, time::Duration as StdDuration};

use chrono::{Duration, NaiveDateTime, Utc};
use log::{debug, error, info, trace, warn};
use once_cell::sync::{Lazy, OnceCell};
use rand::{rngs::OsRng, seq::SliceRandom, Rng};
use regex::Regex;
use sqlx::{Connection, SqliteConnection};
use tokio::sync::OnceCell as AsyncOnceCell;
use twitch_fishinge::{
    db_conn,
    models::{Fish as FishModel, User as UserModel},
    Config,
};
use twitch_irc::{
    login::RefreshingLoginCredentials,
    message::{PrivmsgMessage, ServerMessage},
    SecureTCPTransport, TwitchIRCClient,
};

type Client = TwitchIRCClient<SecureTCPTransport, RefreshingLoginCredentials<Config>>;

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("Could not use settings")]
    Settings(#[from] settings::Error),

    #[error("Could not validate channel name")]
    ValidateChannelName(#[from] twitch_irc::validate::Error),

    #[error("Could not open database connection")]
    OpenDatabase(#[from] twitch_fishinge::OpenDatabaseError),

    #[error("Could not close database connection")]
    CloseDatabase(#[source] sqlx::Error),

    #[error("Could not query user")]
    QueryUser(#[source] sqlx::Error),

    #[error("Could not create user")]
    CreateUser(#[source] sqlx::Error),

    #[error("Could not update user")]
    UpdateUser(#[source] sqlx::Error),

    #[error("Could not query fishes")]
    QueryFishes(#[source] sqlx::Error),

    #[error("Could not migrate database")]
    MigrateDatabase(#[from] sqlx::migrate::MigrateError),

    #[error("Could not reply to message")]
    ReplyToMessage(
        #[from] twitch_irc::Error<SecureTCPTransport, RefreshingLoginCredentials<Config>>,
    ),

    #[error("No fishes found in database")]
    NoFishesInDatabase,
}

static FISHES: AsyncOnceCell<Vec<Fish>> = AsyncOnceCell::const_new();
static FISH_POPULATION: OnceCell<u32> = OnceCell::new();

static COOLDOWN: Lazy<Duration> = Lazy::new(|| Duration::hours(6));

#[derive(Debug, Clone)]
struct Fish {
    name: String,
    count: u32,
    base_value: u32,
    weight_range: Option<Range<f32>>,
}

impl Fish {
    pub const fn new(
        name: String,
        count: u32,
        value: u32,
        weight_range: Option<Range<f32>>,
    ) -> Self {
        Self {
            name,
            base_value: value,
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

impl From<FishModel> for Fish {
    fn from(fish: FishModel) -> Self {
        Self::new(
            fish.name,
            fish.count as u32,
            fish.base_value as u32,
            if fish.min_weight > f64::EPSILON && fish.max_weight > f64::EPSILON {
                Some(fish.min_weight as f32..fish.max_weight as f32)
            } else {
                None
            },
        )
    }
}

impl Display for Fish {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({:.1}%)",
            self.name,
            self.count as f32 / *FISH_POPULATION.get().unwrap() as f32 * 100.0
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

        self.fish.base_value as f32 * weight_multiplier
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

#[tokio::main]
async fn main() -> Result<(), Error> {
    pretty_env_logger::init();

    let mut conn = db_conn().await?;

    info!("Running Migrations");
    sqlx::migrate!().run(&mut conn).await?;

    FISHES
        .get_or_try_init(|| async {
            let fishes: Vec<_> = sqlx::query_as!(FishModel, "SELECT * FROM fishes")
                .fetch_all(&mut conn)
                .await
                .map_err(Error::QueryFishes)?
                .into_iter()
                .map(Fish::from)
                .collect();

            let population = fishes.iter().map(|fish| fish.count).sum();

            FISH_POPULATION.get_or_init(|| population);

            info!(
                "Loaded {} fish species with a total population of {population}",
                fishes.len()
            );

            Result::<_, Error>::Ok(fishes)
        })
        .await?;

    conn.close().await.map_err(Error::CloseDatabase)?;

    let settings = Config::load()?;
    let config = settings.client_config();

    info!("Creating client");
    let (mut incoming_messages, client) = Client::new(config);

    // consume the incoming messages stream
    let twitch_handle = tokio::spawn({
        let client = client.clone();

        async move {
            while let Some(message) = incoming_messages.recv().await {
                trace!("Received message: {:?}", &message);
                match message {
                    ServerMessage::Privmsg(msg) => {
                        if let Err(err) = handle_privmsg(&client, &msg).await {
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

    debug!(
        "Wanting to join channels {}",
        settings
            .channels
            .iter()
            .map(String::from)
            .collect::<Vec<_>>()
            .join(", ")
    );

    client.set_wanted_channels(settings.channels.clone())?;

    // keep the tokio executor alive.
    // If you return instead of waiting the background task will exit.
    twitch_handle.await.unwrap();

    Ok(())
}

static COMMAND_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^((?P<emote>\S+)\s+)?Fishinge( (?P<args>.*))?$").unwrap());
const WEB_URL: &str = "https://fishinge.chronophylos.com";

async fn get_fishes(conn: &mut SqliteConnection) -> Result<Vec<Fish>, Error> {
    let fishes: Vec<_> = sqlx::query_as!(FishModel, "SELECT * FROM fishes")
        .fetch_all(conn)
        .await
        .map_err(Error::QueryFishes)?
        .into_iter()
        .map(Fish::from)
        .collect();

    let population = fishes.iter().map(|fish| fish.count).sum();

    FISH_POPULATION.get_or_init(|| population);
    Ok(fishes)
}

async fn handle_privmsg(client: &Client, msg: &PrivmsgMessage) -> Result<(), Error> {
    if let Some(captures) = COMMAND_REGEX.captures(&msg.message_text) {
        match captures.name("emote").map(|m| m.as_str()) {
            Some("🐱") => {
                client
                    .say_in_reply_to(msg, "No catfishing!".to_string())
                    .await
                    .map_err(Error::ReplyToMessage)?;

                Ok(())
            }
            Some("🔍") => {
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

                    let mut conn = db_conn().await?;
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
            Some("❓") => {
                client
                    .say_in_reply_to(msg, format!("the list of commands is here {WEB_URL}"))
                    .await
                    .map_err(Error::ReplyToMessage)?;

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
    let mut rng = OsRng;

    let mut conn = db_conn().await?;

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

            const MESSAGES: [&str; 7] = [
                "you can't fish yet.",
                "you just fished!",
                "you lost your fishing pole!",
                "you have no bobbers.",
                "not yet!",
                "pirates stole your boat R) !",
                "Oh snap! Your line broke.",
            ];

            client
                .say_in_reply_to(
                    msg,
                    format!(
                        "{} Try again in {cooldown}.",
                        MESSAGES.choose(&mut rng).unwrap()
                    ),
                )
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

    let fishes = get_fishes(&mut conn).await?;

    if fishes.is_empty() {
        return Err(Error::NoFishesInDatabase);
    }

    let fish = fishes.choose_weighted(&mut rng, |fish| fish.count).unwrap();

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
