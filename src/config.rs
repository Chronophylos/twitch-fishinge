use std::{
    collections::HashSet,
    ops::{Deref, DerefMut},
};

use async_trait::async_trait;
use log::debug;
use once_cell::sync::OnceCell;
use secrecy::{CloneableSecret, DebugSecret, ExposeSecret, Secret, SerializableSecret, Zeroize};
use serde::{Deserialize, Serialize};
use settings::Settings;
use twitch_irc::{
    login::{GetAccessTokenResponse, RefreshingLoginCredentials, TokenStorage, UserAccessToken},
    ClientConfig,
};

/// Configuration for this crate.
///
/// This ugly wrapper is requiered due to [`TokenStorage`].
#[derive(Debug, Clone)]
pub struct Config(Settings<ConfigData>);

static SETTINGS: OnceCell<Config> = OnceCell::new();

impl Config {
    pub fn load<'a>() -> Result<&'a Self, settings::Error> {
        SETTINGS.get_or_try_init(|| Settings::load("com", "Chronophylos", "Fishinge").map(Self))
    }

    pub fn client_config(&self) -> ClientConfig<RefreshingLoginCredentials<Config>> {
        debug!("Creating client config");

        let credentials = RefreshingLoginCredentials::init_with_username(
            self.username.clone(),
            self.client.id.clone(),
            self.client.secret.expose_secret().to_string(),
            self.clone(),
        );
        ClientConfig::new_simple(credentials)
    }
}

impl Deref for Config {
    type Target = Settings<ConfigData>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Config {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigData {
    pub username: Option<String>,
    pub token: Option<SecretUserAccessToken>,
    pub client: ClientSettings,
    pub channels: HashSet<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientSettings {
    pub id: String,
    pub secret: SecretClientSecret,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClientSecret(String);

impl Zeroize for ClientSecret {
    fn zeroize(&mut self) {
        self.0.zeroize();
    }
}

impl Deref for ClientSecret {
    type Target = String;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl CloneableSecret for ClientSecret {}
impl DebugSecret for ClientSecret {}
impl SerializableSecret for ClientSecret {}
type SecretClientSecret = Secret<ClientSecret>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserAccessTokenWrapper(UserAccessToken);

impl Zeroize for UserAccessTokenWrapper {
    fn zeroize(&mut self) {
        self.0.access_token.zeroize();
        self.0.refresh_token.zeroize();
    }
}

impl Deref for UserAccessTokenWrapper {
    type Target = UserAccessToken;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl CloneableSecret for UserAccessTokenWrapper {}
impl DebugSecret for UserAccessTokenWrapper {}
impl SerializableSecret for UserAccessTokenWrapper {}
type SecretUserAccessToken = Secret<UserAccessTokenWrapper>;

#[derive(Debug, thiserror::Error)]
pub enum LoadTokenError {
    #[error("Could not read token")]
    ReadInput(#[source] std::io::Error),

    #[error("Could not parse response url")]
    ParseUrl(#[from] url::ParseError),

    #[error("Missing parameter code in response url")]
    MissingCodeInResponse,

    #[error("Missing parameter state in response url")]
    MissingStateInResponse,

    #[error("Parameter state returned by twitch does not match state sent by client")]
    WrongStateReturned,

    #[error("Could not send access token request")]
    RequestAccessToken(#[source] reqwest::Error),

    #[error("Could not deserialize access token response")]
    DeserializeAccessToken(#[source] reqwest::Error),

    #[error("Could not save settings")]
    SaveSettings(#[from] settings::Error),
}

#[async_trait]
impl TokenStorage for Config {
    type LoadError = LoadTokenError;
    type UpdateError = settings::Error;

    async fn load_token(&mut self) -> Result<UserAccessToken, Self::LoadError> {
        if let Some(token) = self.0.token.as_ref() {
            Ok(token.expose_secret().deref().clone())
        } else {
            let state = format!("{:x}", rand::random::<u64>());
            let url = format!("https://id.twitch.tv/oauth2/authorize?response_type=code&force_verify=true&client_id={}&redirect_uri=https://localhost&scope=chat%3Aedit+chat%3Aread&state={state}", self.client.id);

            println!("No token stored: Open {url} in your browser and login. Then paste the final URL here!");
            let mut input = String::new();
            std::io::stdin()
                .read_line(&mut input)
                .map_err(LoadTokenError::ReadInput)?;

            let url = input.trim().parse::<url::Url>()?;

            let returned_state = url
                .query_pairs()
                .find(|(k, _)| k == "state")
                .map(|(_, v)| v)
                .ok_or(LoadTokenError::MissingStateInResponse)?;

            if returned_state != state {
                return Err(LoadTokenError::WrongStateReturned);
            }

            let code = url
                .query_pairs()
                .find(|(k, _)| k == "code")
                .map(|(_, v)| v)
                .ok_or(LoadTokenError::MissingCodeInResponse)?;

            debug!("Getting token from twitch");

            let response = reqwest::Client::new()
                .post("https://id.twitch.tv/oauth2/token")
                .form(&[
                    ("client_id", self.client.id.as_str()),
                    ("client_secret", self.client.secret.expose_secret().as_str()),
                    ("code", code.into_owned().as_str()),
                    ("grant_type", "authorization_code"),
                    ("redirect_uri", "https://localhost"),
                ])
                .send()
                .await
                .map_err(LoadTokenError::RequestAccessToken)?
                .json::<GetAccessTokenResponse>()
                .await
                .map_err(LoadTokenError::DeserializeAccessToken)?;

            let token = response.into();
            self.update_token(&token).await?;

            Ok(token)
        }
    }

    async fn update_token(&mut self, token: &UserAccessToken) -> Result<(), Self::UpdateError> {
        debug!("Updating token");

        self.token = Some(SecretUserAccessToken::new(UserAccessTokenWrapper(
            token.clone(),
        )));
        self.save()
    }
}

#[test]
fn can_decode_current_settings() {
    dbg!(Settings::<ConfigData>::load("com", "Chronophylos", "Fishinge").unwrap());
}
