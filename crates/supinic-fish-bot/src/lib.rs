mod parser;

use std::time::Duration;

use bot_framework::runner::{Client, IrcError};
use exponential_backoff::Backoff;
use log::{debug, error, info, trace};
use miette::{Diagnostic, IntoDiagnostic, Result, WrapErr};
use sea_orm::DatabaseConnection;
use tokio::{
    sync::mpsc::{Receiver, Sender},
    time::timeout,
};
use twitch_irc::message::ServerMessage;

use crate::parser::fish_response::{FishResponse, FishResponseKind};

const BOT_LOGIN: &str = "supibot";

#[derive(Debug, thiserror::Error, Diagnostic)]
pub enum Error {
    #[error("could not send message")]
    #[diagnostic(code(supinic_fish_bot::send_message))]
    SendMessage(#[source] IrcError),

    #[error("channel closed")]
    #[diagnostic(code(supinic_fish_bot::channel_closed))]
    ChannelClosed,

    #[error("timed out waiting for response")]
    #[diagnostic(code(supinic_fish_bot::receive_message_timeout))]
    ReceiveMessageTimeout,
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
    username: String,
    tx: Sender<Message>,
) -> Result<()> {
    trace!("handling message: {:?}", server_message);

    let message = match server_message {
        ServerMessage::GlobalUserState(_) => Message::Ready,
        ServerMessage::Privmsg(msg)
            if msg.sender.login == BOT_LOGIN && msg.message_text.starts_with(&username) =>
        {
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
        let message = send_command(
            &client,
            &mut rx,
            channel.clone(),
            "$fish skipStory:true".to_string(),
        )
        .await?;

        debug!("parsing response");
        let response = match FishResponse::parse(&message) {
            Ok(response) => response,
            Err(err) => {
                error!("failed to parse fish response from {message}: {err}");
                tokio::time::sleep(Duration::from_secs_f32(5.2)).await;
                continue;
            }
        };

        debug!("fish response: {:?}", response);

        match response.kind {
            FishResponseKind::Success { catch, length } => {
                trace!("caught fish: {catch} @ {length} cm");

                tokio::time::sleep(Duration::from_secs_f32(5.2)).await;
                sell(&client, &mut rx, channel.clone(), &catch).await?;
            }
            FishResponseKind::Failure {
                junk: Some(junk), ..
            } => {
                trace!("caught junk: {junk}");

                tokio::time::sleep(Duration::from_secs_f32(5.2)).await;
                sell(&client, &mut rx, channel.clone(), &junk).await?;
            }
            FishResponseKind::Failure { .. } => {
                trace!("no junk caught");
            }
            FishResponseKind::Cooldown => {
                trace!("command is on cooldown");
            }
        }

        let cooldown = response
            .cooldown
            .clamp(Duration::from_secs(5), Duration::from_secs(60 * 60 * 24))
            + Duration::from_secs_f32(0.3);

        info!("sleeping for {cooldown:?}");
        tokio::time::sleep(cooldown).await;
    }
}

async fn send_command(
    client: &Client,
    rx: &mut Receiver<Message>,
    channel: String,
    command: String,
) -> Result<String, Error> {
    debug!("sending command: {command}");

    let backoff = Backoff::new(3, Duration::from_secs_f32(5.2), Duration::from_secs(30));

    for duration in &backoff {
        client
            .say(channel.clone(), command.clone())
            .await
            .map_err(Error::SendMessage)?;

        // wait for response
        match timeout(Duration::from_secs(3), rx.recv()).await {
            Ok(Some(Message::Bot(message))) => return Ok(message),
            Ok(None) => return Err(Error::ChannelClosed),
            _ => {}
        }

        tokio::time::sleep(duration).await;
    }

    Err(Error::ReceiveMessageTimeout)
}

async fn sell(
    client: &Client,
    rx: &mut Receiver<Message>,
    channel: String,
    what: &str,
) -> Result<(), Error> {
    let message = send_command(client, rx, channel, format!("$fish sell {what}")).await?;

    // TODO: parse sell response
    dbg!(message);

    Ok(())
}
