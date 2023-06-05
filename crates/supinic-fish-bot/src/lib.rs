use std::time::Duration;

use bot_framework::runner::{Client, IrcError};
use log::{debug, info, trace, warn};
use miette::{Diagnostic, IntoDiagnostic, Result, WrapErr};
use once_cell::sync::Lazy;
use regex::Regex;
use sea_orm::DatabaseConnection;
use tokio::sync::mpsc::{Receiver, Sender};
use twitch_irc::message::ServerMessage;

const FISH_RESPONSE_COOLDOWN_PREFIX: &str = "Hol' up partner! You can go fishing again in ";
static FISH_RESPONSE_COOLDOWN_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"Hol' up partner! You can go fishing again in (?P<cooldown>[0-9\.]+)s!"#).unwrap()
});
const FISH_RESPONSE_SUCCESS_PREFIX: &str = "You caught a ✨ ";
static FISH_RESPONSE_SUCCESS_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"You caught a ✨ (?P<catch>.) ✨ It is (?P<length>\d+) cm in length. (?P<is_record>This is a new record! )?\w+ Now, go do something productive! \((?P<cooldown>\d+) minute fishing cooldown after a successful catch\)"#).unwrap()
});
const FISH_RESPONSE_FAILURE_PREFIX: &str = "No luck..";
static FISH_RESPONSE_FAILURE_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"No luck\.{3} \D+ (You reel out a (?P<junk>.)|(?P<distance>\d+) cm away\.) \(((?P<minutes>\d+)m, )?((?P<seconds>\d+)s )cooldown\)( This is your attempt #(?P<attempt>\d+) since your last catch\.)?"#).unwrap()
});
const BOT_LOGIN: &str = "supibot";

#[derive(Debug, thiserror::Error, Diagnostic)]
pub enum Error {
    #[error("bot response malformed")]
    #[diagnostic(code(supinic_fish_bot::malformed_response))]
    MalformedResponse { reason: &'static str, text: String },

    #[error("unknown bot response: {0:?}")]
    #[diagnostic(code(supinic_fish_bot::unknown_response))]
    UnknownResponse(String),

    #[error("could not send message")]
    #[diagnostic(code(supinic_fish_bot::send_message))]
    SendMessage(#[source] IrcError),

    #[error("channel closed")]
    #[diagnostic(code(supinic_fish_bot::channel_closed))]
    ChannelClosed,
}

#[derive(Debug)]
pub enum Message {
    Bot(String),
    Ready,
}

pub async fn handle_server_message(
    _conn: DatabaseConnection,
    _client: Client,
    server_message: ServerMessage,
    tx: Sender<Message>,
) -> Result<()> {
    trace!("handling message: {:?}", server_message);

    let message = match server_message {
        ServerMessage::GlobalUserState(_) => Message::Ready,
        ServerMessage::Privmsg(msg) if msg.sender.login == BOT_LOGIN => {
            Message::Bot(msg.message_text.to_string())
        }
        _ => return Ok(()),
    };

    trace!("passing message to main task: {message:?}");
    tx.send(message)
        .await
        .into_diagnostic()
        .wrap_err("failed to pass message to main task")?;

    Ok(())
}

pub async fn run_wrapper(
    _conn: DatabaseConnection,
    client: Client,
    channel: String,
    rx: Receiver<Message>,
) -> Result<()> {
    tokio::spawn(async move {
        if let Err(e) = run(client, channel, rx).await {
            log::error!("error in main task: {}", e);
        }
    });

    Ok(())
}

async fn run(client: Client, channel: String, mut rx: Receiver<Message>) -> Result<(), Error> {
    info!("Starting fish bot");

    // wait for ready message
    debug!("waiting for twitch to be ready");
    loop {
        match rx.recv().await {
            Some(Message::Ready) => break,
            Some(_) => {}
            None => {
                return Err(Error::ChannelClosed);
            }
        }
    }

    loop {
        debug!("sending $fish");
        client
            .say(channel.clone(), "$fish".to_string())
            .await
            .map_err(Error::SendMessage)?;

        let message = wait_for_bot_message(&mut rx).await?;

        debug!("parsing response");
        let response = match FishResponse::parse(&message) {
            Ok(response) => response,
            Err(e) => {
                warn!("failed to parse fish response: {}", e);
                continue;
            }
        };

        debug!("fish response: {:?}", response);

        match response.kind {
            FishResponseKind::Success { catch, length } => {
                trace!("caught fish: {catch} @ {length} cm");

                sell_all(client.clone(), &mut rx, channel.clone()).await?;
            }
            FishResponseKind::Failure {
                junk: Some(junk), ..
            } => {
                trace!("caught junk: {}", junk);

                sell_all(client.clone(), &mut rx, channel.clone()).await?;
            }
            FishResponseKind::Failure { .. } => {
                trace!("no junk caught");
            }
            FishResponseKind::Cooldown => {
                trace!("command is on cooldown");
            }
        }

        info!("sleeping for {:?}", response.cooldown);
        tokio::time::sleep(response.cooldown).await;
    }
}

async fn wait_for_bot_message(rx: &mut Receiver<Message>) -> Result<String, Error> {
    // TODO: handle timeout
    debug!("waiting for response");
    loop {
        match rx.recv().await {
            Some(Message::Bot(message)) => return Ok(message),
            Some(_) => {}
            None => {
                return Err(Error::ChannelClosed);
            }
        }
    }
}

async fn sell_all(
    client: Client,
    rx: &mut Receiver<Message>,
    channel: String,
) -> Result<(), Error> {
    debug!("sending $fish sell all");

    client
        .say(channel, "$fish sell all".to_string())
        .await
        .map_err(Error::SendMessage)?;

    let message = wait_for_bot_message(rx).await?;

    // TODO: parse sell response
    dbg!(message);

    Ok(())
}

#[derive(Debug, PartialEq)]
pub struct FishResponse {
    name: String,
    kind: FishResponseKind,
    cooldown: Duration,
}

impl FishResponse {
    /// Parse response to $fish from message text
    pub fn parse(text: &str) -> Result<Self, Error> {
        let Some((name, rest)) = text.trim().split_once(',') else {
        return Err(Error::MalformedResponse{reason: "no comma found", text: text.to_string()});
    };
        let rest = rest.trim();

        // sorted by most common first
        if rest.starts_with(FISH_RESPONSE_FAILURE_PREFIX) {
            Self::parse_falure(name.to_string(), rest)
        } else if rest.starts_with(FISH_RESPONSE_SUCCESS_PREFIX) {
            Self::parse_success(name.to_string(), rest)
        } else if rest.starts_with(FISH_RESPONSE_COOLDOWN_PREFIX) {
            Self::parse_cooldown(name.to_string(), rest)
        } else {
            return Err(Error::UnknownResponse(rest.to_string()));
        }
    }

    fn parse_success(name: String, text: &str) -> Result<Self, Error> {
        FISH_RESPONSE_SUCCESS_REGEX.captures(text).map_or_else(
            || {
                Err(Error::MalformedResponse {
                    reason: "success regex did not match",
                    text: text.to_string(),
                })
            },
            |captures| {
                let cooldown = captures
                    .name("cooldown")
                    .unwrap()
                    .as_str()
                    .parse::<u64>()
                    .unwrap();
                let catch = captures.name("catch").unwrap().as_str().to_string();
                let length = captures
                    .name("length")
                    .unwrap()
                    .as_str()
                    .parse::<u32>()
                    .unwrap();

                Ok(Self {
                    name,
                    kind: FishResponseKind::Success { catch, length },
                    cooldown: Duration::from_secs(cooldown * 60),
                })
            },
        )
    }

    fn parse_falure(name: String, text: &str) -> Result<Self, Error> {
        FISH_RESPONSE_FAILURE_REGEX.captures(text).map_or_else(
            || {
                Err(Error::MalformedResponse {
                    reason: "failure regex did not match",
                    text: text.to_string(),
                })
            },
            |captures| {
                let attempt = captures
                    .name("attempt")
                    .map(|m| m.as_str().parse::<u32>().unwrap());
                let distance = captures
                    .name("distance")
                    .map(|m| m.as_str().parse::<u32>().unwrap());
                let junk = captures.name("junk").map(|m| m.as_str().to_string());
                let minutes = captures
                    .name("minutes")
                    .map(|m| m.as_str().parse::<u64>().unwrap())
                    .unwrap_or(0);
                let seconds = captures
                    .name("seconds")
                    .unwrap()
                    .as_str()
                    .parse::<u64>()
                    .unwrap();

                Ok(Self {
                    name,
                    kind: FishResponseKind::Failure {
                        attempt,
                        distance,
                        junk,
                    },
                    cooldown: Duration::from_secs(60 * minutes + seconds),
                })
            },
        )
    }

    fn parse_cooldown(name: String, text: &str) -> Result<Self, Error> {
        FISH_RESPONSE_COOLDOWN_REGEX.captures(text).map_or_else(
            || {
                Err(Error::MalformedResponse {
                    reason: "cooldown regex did not match",
                    text: text.to_string(),
                })
            },
            |captures| {
                let cooldown = captures
                    .name("cooldown")
                    .unwrap()
                    .as_str()
                    .parse::<f64>()
                    .unwrap();

                Ok(Self {
                    name,
                    kind: FishResponseKind::Cooldown,
                    cooldown: Duration::from_secs_f64(cooldown),
                })
            },
        )
    }
}

#[derive(Debug, PartialEq)]
pub enum FishResponseKind {
    Failure {
        attempt: Option<u32>,
        distance: Option<u32>,
        junk: Option<String>,
    },
    Success {
        catch: String,
        length: u32,
    },
    Cooldown,
}

#[cfg(test)]
mod tests {
    mod response {
        mod parse {
            use crate::{Error, FishResponse, FishResponseKind};

            #[test]
            fn returns_malformed_response_when_missing_comma() {
                let result = FishResponse::parse("test").unwrap_err();

                assert!(matches!(result, Error::MalformedResponse { .. }));
            }

            #[test]
            fn returns_unknown_response() {
                let result = FishResponse::parse("test, test").unwrap_err();

                assert!(matches!(result, Error::UnknownResponse { .. }));
            }

            #[test]
            fn cooldown_response() {
                let result = FishResponse::parse(
                    "chronophylos, Hol' up partner! You can go fishing again in 34.67s!",
                )
                .unwrap();
                let expected = FishResponse {
                    name: "chronophylos".to_string(),
                    kind: FishResponseKind::Cooldown,
                    cooldown: std::time::Duration::from_secs_f64(34.67),
                };

                assert_eq!(result, expected);
            }

            #[test]
            fn success_reponse() {
                let input = r#"gargoyletec, You caught a ✨ 🦀 ✨ It is 10 cm in length. PagChomp Now, go do something productive! (30 minute fishing cooldown after a successful catch)"#;
                let result = FishResponse::parse(input).unwrap();
                let expected = FishResponse {
                    name: "gargoyletec".to_string(),
                    kind: FishResponseKind::Success {
                        catch: "🦀".to_string(),
                        length: 10,
                    },
                    cooldown: std::time::Duration::from_secs(30 * 60),
                };

                assert_eq!(result, expected);
            }

            #[test]
            fn failure_response_with_junk() {
                let input = r#"gargoyletec, No luck... FailFish It seems luck wasn't on your side this time. You caught a piece of junk. You reel out a 🌿 (1m, 18s cooldown) This is your attempt #17 since your last catch."#;
                let result = FishResponse::parse(input).unwrap();
                let expected = FishResponse {
                    name: "gargoyletec".to_string(),
                    kind: FishResponseKind::Failure {
                        attempt: Some(17),
                        distance: None,
                        junk: Some("🌿".to_string()),
                    },
                    cooldown: std::time::Duration::from_secs(60 + 18),
                };

                assert_eq!(result, expected);
            }

            #[test]
            fn failure_response_without_junk_and_attempts() {
                let input = r#"gargoyletec, No luck... SadgeCry Your fishing line landed 77 cm away. (45s cooldown)"#;
                let result = FishResponse::parse(input).unwrap();
                let expected = FishResponse {
                    name: "gargoyletec".to_string(),
                    kind: FishResponseKind::Failure {
                        attempt: None,
                        distance: Some(77),
                        junk: None,
                    },
                    cooldown: std::time::Duration::from_secs(45),
                };

                assert_eq!(result, expected);
            }

            #[test]
            fn failure_response_without_junk() {
                let input = r#"gargoyletec, No luck... Sadge Your fishing line landed 150 cm away. (59s cooldown) This is your attempt #8 since your last catch."#;
                let result = FishResponse::parse(input).unwrap();
                let expected = FishResponse {
                    name: "gargoyletec".to_string(),
                    kind: FishResponseKind::Failure {
                        attempt: Some(8),
                        distance: Some(150),
                        junk: None,
                    },
                    cooldown: std::time::Duration::from_secs(59),
                };

                assert_eq!(result, expected);
            }
        }
    }
}
