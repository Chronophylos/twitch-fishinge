#![forbid(unsafe_code)]

mod config;

use std::{collections::HashMap, fmt::Display, ops::Range};

use log::{error, info, warn};
use once_cell::sync::Lazy;
use rand::{seq::IteratorRandom, Rng};
use regex::Regex;
use twitch_irc::{
    login::RefreshingLoginCredentials,
    message::{PrivmsgMessage, ServerMessage},
    SecureTCPTransport, TwitchIRCClient,
};

use crate::config::Config;

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("Could not use settings")]
    Settings(#[from] settings::Error),

    #[error("Could not validate channel name")]
    ValidateChannelName(#[from] twitch_irc::validate::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
enum Difficulty {
    Trivial,
    Easy,
    Medium,
    Hard,
    Custom(f64),
}

impl Difficulty {
    fn probability(&self) -> f64 {
        match self {
            Difficulty::Trivial => 0.5,
            Difficulty::Easy => 0.2,
            Difficulty::Medium => 0.1,
            Difficulty::Hard => 0.01,
            Difficulty::Custom(custom) => *custom,
        }
    }
}

impl Default for Difficulty {
    fn default() -> Self {
        Difficulty::Medium
    }
}

static FISHES: Lazy<HashMap<String, Fish>> = Lazy::new(|| {
    [
        Fish::new("👞", Difficulty::Trivial, None),
        Fish::new("🦆", Difficulty::Easy, Some(2.0..5.0)),
        Fish::new("🐟", Difficulty::Medium, Some(0.2..5.0)),
        Fish::new("💀", Difficulty::Hard, None),
        Fish::new("FishMoley", Difficulty::Hard, Some(3.5..10.0)),
        Fish::new("Hhhehehe", Difficulty::Custom(0.01), None),
    ]
    .into_iter()
    .map(|fish| (fish.name.clone(), fish))
    .collect()
});

#[derive(Debug, Clone)]
struct Fish {
    name: String,
    difficulty: Difficulty,
    weight: Option<Range<f32>>,
}

impl Fish {
    pub fn new(name: &str, difficulty: Difficulty, weight: Option<Range<f32>>) -> Self {
        Self {
            name: name.to_string(),
            difficulty,
            weight,
        }
    }

    pub fn catch(&self) -> Option<Catch> {
        let mut rng = rand::thread_rng();

        let probability = self.difficulty.probability();

        if !rng.gen_bool(probability) {
            return None;
        }

        let weight = self.weight.clone().map(|weight| rng.gen_range(weight));

        Some(Catch::new(self, weight))
    }
}

impl Display for Fish {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
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
}

impl Display for Catch<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.fish.name)?;
        if let Some(weight) = self.weight {
            write!(f, " ({:.1} kg)", weight)?;
        }

        Ok(())
    }
}

type Client = TwitchIRCClient<SecureTCPTransport, RefreshingLoginCredentials<Config>>;

#[tokio::main]
async fn main() -> Result<(), Error> {
    pretty_env_logger::init();

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
                        handle_privmsg(&client, &msg).await;
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

static FISH_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(r"^((?P<fish>.+)\W+)?Fishinge").unwrap());

async fn handle_privmsg(client: &Client, msg: &PrivmsgMessage) {
    let fish = match FISH_REGEX.captures(&msg.message_text) {
        Some(captures) => captures
            .name("fish")
            .map(|m| m.as_str().trim())
            .map(|name| {
                FISHES
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| Fish::new(name, Difficulty::default(), None))
            })
            .unwrap_or_else(|| {
                FISHES
                    .values()
                    .choose(&mut rand::thread_rng())
                    .unwrap()
                    .clone()
            }),
        None => return,
    };

    info!("{} is fishing for {fish}", msg.sender.name);

    if let Some(catch) = fish.catch() {
        info!("{} caught {catch}", msg.sender.name);

        if let Err(err) = client
            .say_in_reply_to(msg, format!("caught a {catch} !"))
            .await
        {
            error!("Could not send message: {}", err);
        }
    }
}
