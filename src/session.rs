use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use librespot::core::cache::Cache;
use librespot::core::config::SessionConfig;
use librespot::core::session::Session;
use librespot::discovery::Credentials;
use librespot::oauth::OAuthClientBuilder;
use tracing::warn;

const SPOTIFY_REDIRECT_URI: &str = "http://127.0.0.1:8898/login";
const SPOTIFY_SCOPES: [&str; 1] = ["streaming"];
const OAUTH_REFRESH_TOKEN_FILE: &str = "oauth.refresh";

pub async fn create_session() -> Result<Session> {
    let session_config = SessionConfig::default();
    let credentials_store = credentials_store()?;

    if let Some(cached_credentials) = load_cached_credentials(&credentials_store)? {
        match connect_session(
            &session_config,
            &credentials_store,
            cached_credentials,
            true,
        )
        .await
        {
            Ok(session) => return Ok(session),
            Err(err) => warn!("Cached Spotify session could not be reused: {err}"),
        }
    }

    let oauth_client = OAuthClientBuilder::new(
        &session_config.client_id,
        SPOTIFY_REDIRECT_URI,
        SPOTIFY_SCOPES.to_vec(),
    )
    .open_in_browser()
    .build()
    .context("failed to initialize Spotify OAuth client")?;

    let refresh_token_path = credentials_store.join(OAUTH_REFRESH_TOKEN_FILE);
    if let Some(refresh_token) = load_refresh_token(&refresh_token_path)? {
        match oauth_client.refresh_token(&refresh_token) {
            Ok(token) => {
                save_refresh_token(&refresh_token_path, &token.refresh_token)?;
                match connect_session(
                    &session_config,
                    &credentials_store,
                    Credentials::with_access_token(token.access_token),
                    true,
                )
                .await
                {
                    Ok(session) => return Ok(session),
                    Err(err) => warn!("Refreshed Spotify OAuth token was rejected: {err}"),
                }
            }
            Err(err) => warn!("Saved Spotify refresh token is no longer valid: {err}"),
        }
    }

    let oauth_token = oauth_client
        .get_access_token()
        .context("failed to complete Spotify OAuth login")?;
    save_refresh_token(&refresh_token_path, &oauth_token.refresh_token)?;

    connect_session(
        &session_config,
        &credentials_store,
        Credentials::with_access_token(oauth_token.access_token),
        true,
    )
    .await
}

fn credentials_store() -> Result<PathBuf> {
    dirs::home_dir()
        .map(|path| path.join(".spotify-dl"))
        .context("could not resolve the home directory for Spotify auth cache")
}

fn load_cached_credentials(credentials_store: &Path) -> Result<Option<Credentials>> {
    Ok(Cache::new(Some(credentials_store), None::<&Path>, None::<&Path>, None)?.credentials())
}

async fn connect_session(
    session_config: &SessionConfig,
    credentials_store: &Path,
    credentials: Credentials,
    store_credentials: bool,
) -> Result<Session> {
    let cache = Cache::new(Some(credentials_store), None::<&Path>, None::<&Path>, None)?;
    let session = Session::new(session_config.clone(), Some(cache));
    session.connect(credentials, store_credentials).await?;
    Ok(session)
}

fn load_refresh_token(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(token) => {
            let token = token.trim().to_owned();
            if token.is_empty() {
                Ok(None)
            } else {
                Ok(Some(token))
            }
        }
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| {
            format!(
                "failed to read Spotify OAuth refresh token from {}",
                path.display()
            )
        }),
    }
}

fn save_refresh_token(path: &Path, refresh_token: &str) -> Result<()> {
    fs::write(path, refresh_token).with_context(|| {
        format!(
            "failed to save Spotify OAuth refresh token to {}",
            path.display()
        )
    })
}
