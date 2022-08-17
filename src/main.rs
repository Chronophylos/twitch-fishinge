#![forbid(unsafe_code)]

mod config;

use twitch_irc::{
    login::RefreshingLoginCredentials, message::ServerMessage, SecureTCPTransport, TwitchIRCClient,
};

use crate::config::Config;

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("Could not use settings")]
    Settings(#[from] settings::Error),

    #[error("Could not validate channel name")]
    ValidateChannelName(#[from] twitch_irc::validate::Error),
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    pretty_env_logger::init();

    let settings = Config::load()?;
    let config = settings.client_config();

    let (mut incoming_messages, client) =
        TwitchIRCClient::<SecureTCPTransport, RefreshingLoginCredentials<Config>>::new(config);

    // consume the incoming messages stream
    let twitch_handle = tokio::spawn(async move {
        while let Some(message) = incoming_messages.recv().await {
            match message {
                ServerMessage::Privmsg(msg) => {
                    println!(
                        "(#{}) {}: {}",
                        msg.channel_login, msg.sender.name, msg.message_text
                    );
                }
                ServerMessage::Whisper(msg) => {
                    println!("(w) {}: {}", msg.sender.name, msg.message_text);
                }
                _ => {}
            }
        }
    });

    client.set_wanted_channels(settings.channels.clone())?;

    // keep the tokio executor alive.
    // If you return instead of waiting the background task will exit.
    twitch_handle.await.unwrap();

    Ok(())
}
