#![forbid(unsafe_code)]

mod config;

use std::{fmt::Display, ops::Range};

use log::{error, info, warn};
use once_cell::sync::Lazy;
use rand::{seq::SliceRandom, Rng};
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

async fn handle_privmsg(client: &Client, msg: &PrivmsgMessage) {
    if !msg.message_text.starts_with("Fishinge") {
        return;
    }

    let fish = FISHES
        .choose_weighted(&mut rand::thread_rng(), |fish| fish.weight)
        .unwrap();

    info!("{} is fishing for {fish}", msg.sender.name);

    let catch = fish.catch();

    info!("{} caught {catch}", msg.sender.name);

    if let Err(err) = client
        .say_in_reply_to(msg, format!("caught a {catch} !"))
        .await
    {
        error!("Could not send message: {}", err);
    }
}
