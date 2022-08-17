use std::{
    collections::HashSet,
    ops::{Deref, DerefMut},
};

use async_trait::async_trait;
use secrecy::{CloneableSecret, DebugSecret, ExposeSecret, Secret, SerializableSecret, Zeroize};
use serde::{Deserialize, Serialize};
use settings::Settings;
use twitch_irc::{
    login::{RefreshingLoginCredentials, TokenStorage, UserAccessToken},
    ClientConfig,
};

#[derive(Debug, thiserror::Error)]
#[error("No token stored")]
pub struct NoTokenError;

/// Configuration for this crate.
///
/// This ugly wrapper is requiered due to [`TokenStorage`].
#[derive(Debug, Clone)]
pub struct Config(Settings<ConfigData>);

impl Config {
    pub fn load() -> Result<Self, settings::Error> {
        Settings::load("com", "Chronophylos", "Fishinge").map(Self)
    }

    pub fn client_config(&self) -> ClientConfig<RefreshingLoginCredentials<Config>> {
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

#[async_trait]
impl TokenStorage for Config {
    type LoadError = NoTokenError;
    type UpdateError = settings::Error;

    async fn load_token(&mut self) -> Result<UserAccessToken, Self::LoadError> {
        self.0
            .token
            .as_ref()
            .map(|token| token.expose_secret().deref().clone())
            .ok_or(NoTokenError)
    }

    async fn update_token(&mut self, token: &UserAccessToken) -> Result<(), Self::UpdateError> {
        self.0.token = Some(SecretUserAccessToken::new(UserAccessTokenWrapper(
            token.clone(),
        )));
        self.0.save()
    }
}

#[test]
fn can_decode_current_settings() {
    dbg!(Settings::<ConfigData>::load("com", "Chronophylos", "Fishinge").unwrap());
}
