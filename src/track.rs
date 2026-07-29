use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use bytes::Bytes;
use librespot::core::error::{Error as LibrespotError, ErrorKind as LibrespotErrorKind};
use lazy_static::lazy_static;
use librespot::core::session::Session;
use librespot::core::SpotifyUri;
use librespot::metadata::Metadata;
use librespot::metadata::image::Image;
use regex::Regex;

use crate::encoder::tags::Tags;
use crate::utils::clean_invalid_characters;

pub type AsyncFn<T> =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Option<T>> + Send>> + Send + Sync>;

const METADATA_MAX_RETRIES: usize = 5;
const METADATA_RETRY_BASE_DELAY_MS: u64 = 750;

#[async_trait::async_trait]
trait TrackCollection {
    async fn get_tracks(&self, session: &Session) -> Result<Vec<Track>>;
}

#[tracing::instrument(name = "get_tracks", skip(session), level = "debug")]
pub async fn get_tracks(spotify_ids: Vec<String>, session: &Session) -> Result<Vec<Track>> {
    let mut tracks: Vec<Track> = Vec::new();
    for id in spotify_ids {
        tracing::debug!("Getting tracks for: {}", id);
        let id = parse_uri_or_url(&id).ok_or(anyhow::anyhow!("Invalid track"))?;
        let new_tracks = match &id {
            SpotifyUri::Track { .. } | SpotifyUri::Episode { .. } => vec![Track::from_id(id)],
            SpotifyUri::Album { .. } => Album::from_id(id).get_tracks(session).await?,
            SpotifyUri::Playlist { .. } => Playlist::from_id(id).get_tracks(session).await?,
            _ => {
                tracing::warn!("Unsupported item type: {}", id.item_type());
                vec![]
            }
        };
        tracks.extend(new_tracks);
    }
    tracing::debug!("Got tracks: {:?}", tracks);
    Ok(tracks)
}

fn parse_uri_or_url(track: &str) -> Option<SpotifyUri> {
    parse_uri(track).or_else(|| parse_url(track))
}

fn parse_uri(track_uri: &str) -> Option<SpotifyUri> {
    let res = SpotifyUri::from_uri(track_uri);
    tracing::info!("Parsed URI: {:?}", res);
    res.ok()
}

fn parse_url(track_url: &str) -> Option<SpotifyUri> {
    let results = SPOTIFY_URL_REGEX.captures(track_url)?;
    let uri = format!(
        "spotify:{}:{}",
        results.get(1)?.as_str(),
        results.get(2)?.as_str()
    );
    SpotifyUri::from_uri(&uri).ok()
}

#[derive(Clone, Debug)]
pub struct Track {
    pub id: SpotifyUri,
}

lazy_static! {
    static ref SPOTIFY_URL_REGEX: Regex =
        Regex::new(r"https://open\.spotify\.com(?:/intl-[a-z]{2})?/(\w+)/([a-zA-Z0-9]+)").unwrap();
}

impl Track {
    pub fn new(track: &str) -> Result<Self> {
        let id = parse_uri_or_url(track).ok_or(anyhow::anyhow!("Invalid track"))?;
        Ok(Track { id })
    }

    pub fn from_id(id: SpotifyUri) -> Self {
        Track { id }
    }

    pub async fn metadata(&self, session: &Session) -> Result<TrackMetadata> {
        match &self.id {
            SpotifyUri::Track { .. } => {
                let metadata = retry_metadata_request(
                    || librespot::metadata::Track::get(session, &self.id),
                    format!("track metadata for {}", self.id),
                )
                .await
                    .with_context(|| format!("failed to get track metadata for {}", self.id))?;

                let mut artists = Vec::new();
                for artist in metadata.artists.iter() {
                    artists.push(
                        retry_metadata_request(
                            || librespot::metadata::Artist::get(session, &artist.id),
                            format!("artist metadata for {} while resolving track {}", artist.id, self.id),
                        )
                        .await
                            .with_context(|| {
                                format!(
                                    "failed to get artist metadata for {} while resolving track {}",
                                    artist.id, self.id
                                )
                            })?,
                    );
                }

                let album = retry_metadata_request(
                    || librespot::metadata::Album::get(session, &metadata.album.id),
                    format!(
                        "album metadata for {} while resolving track {}",
                        metadata.album.id, self.id
                    ),
                )
                .await
                    .with_context(|| {
                        format!(
                            "failed to get album metadata for {} while resolving track {}",
                            metadata.album.id, self.id
                        )
                    })?;

                Ok(TrackMetadata::from_track(metadata, artists, album, session))
            }
            SpotifyUri::Episode { .. } => {
                let metadata = retry_metadata_request(
                    || librespot::metadata::Episode::get(session, &self.id),
                    format!("episode metadata for {}", self.id),
                )
                .await
                    .with_context(|| format!("failed to get episode metadata for {}", self.id))?;
                Ok(TrackMetadata::from_episode(metadata, session))
            }
            _ => Err(anyhow::anyhow!(
                "Unsupported playable item type: {}",
                self.id.item_type()
            )),
        }
    }
}

#[async_trait::async_trait]
impl TrackCollection for Track {
    async fn get_tracks(&self, _session: &Session) -> Result<Vec<Track>> {
        Ok(vec![self.clone()])
    }
}

pub struct Album {
    id: SpotifyUri,
}

impl Album {
    pub fn new(album: &str) -> Result<Self> {
        let id = parse_uri_or_url(album).ok_or(anyhow::anyhow!("Invalid album"))?;
        Ok(Album { id })
    }

    pub fn from_id(id: SpotifyUri) -> Self {
        Album { id }
    }

    pub async fn is_album(id: SpotifyUri, session: &Session) -> bool {
        retry_metadata_request(
            || librespot::metadata::Album::get(session, &id),
            format!("album metadata for {}", id),
        )
        .await
        .is_ok()
    }
}

#[async_trait::async_trait]
impl TrackCollection for Album {
    async fn get_tracks(&self, session: &Session) -> Result<Vec<Track>> {
        let album = retry_metadata_request(
            || librespot::metadata::Album::get(session, &self.id),
            format!("album metadata for {}", self.id),
        )
        .await
            .with_context(|| format!("failed to get album metadata for {}", self.id))?;
        Ok(album
            .tracks()
            .cloned()
            .map(Track::from_id)
            .collect())
    }
}

pub struct Playlist {
    id: SpotifyUri,
}

impl Playlist {
    pub fn new(playlist: &str) -> Result<Self> {
        let id = parse_uri_or_url(playlist).ok_or(anyhow::anyhow!("Invalid playlist"))?;
        Ok(Playlist { id })
    }

    pub fn from_id(id: SpotifyUri) -> Self {
        Playlist { id }
    }

    pub async fn is_playlist(id: SpotifyUri, session: &Session) -> bool {
        retry_metadata_request(
            || librespot::metadata::Playlist::get(session, &id),
            format!("playlist metadata for {}", id),
        )
        .await
        .is_ok()
    }
}

#[async_trait::async_trait]
impl TrackCollection for Playlist {
    async fn get_tracks(&self, session: &Session) -> Result<Vec<Track>> {
        let playlist = retry_metadata_request(
            || librespot::metadata::Playlist::get(session, &self.id),
            format!("playlist metadata for {}", self.id),
        )
        .await
            .with_context(|| format!("failed to get playlist metadata for {}", self.id))?;
        Ok(playlist
            .tracks()
            .cloned()
            .map(Track::from_id)
            .collect())
    }
}

#[derive(Clone)]
pub struct TrackMetadata {
    pub artists: Vec<ArtistMetadata>,
    pub track_name: String,
    pub album: AlbumMetadata,
    pub duration: i32,
    image_retriever: AsyncFn<Bytes>,
}

impl TrackMetadata {
    pub fn from_track(
        track: librespot::metadata::Track,
        artists: Vec<librespot::metadata::Artist>,
        album: librespot::metadata::Album,
        session: &Session,
    ) -> Self {
        let covers = album.covers.to_vec();
        let image_retriever = build_image_retriever(covers, session);
        let artists = artists
            .iter()
            .map(|artist| ArtistMetadata::from(artist.clone()))
            .collect();
        let album = AlbumMetadata::from(album);

        TrackMetadata {
            artists,
            track_name: track.name.clone(),
            album,
            duration: track.duration,
            image_retriever,
        }
    }

    pub fn from_episode(track: librespot::metadata::Episode, session: &Session) -> Self {
        let image_retriever = build_image_retriever(track.covers.to_vec(), session);

        TrackMetadata {
            artists: vec![ArtistMetadata {
                name: track.show_name.clone(),
            }],
            track_name: track.name.clone(),
            album: AlbumMetadata {
                name: track.show_name.clone(),
                cover: track.covers.first().cloned(),
            },
            duration: track.duration,
            image_retriever,
        }
    }

    pub fn approx_size(&self) -> usize {
        let duration = self.duration / 1000;
        let sample_rate = 44100;
        let channels = 2;
        let bits_per_sample = 32;
        let bytes_per_sample = bits_per_sample / 8;
        (duration as usize) * sample_rate * channels * bytes_per_sample
    }

    pub async fn tags(&self) -> Result<Tags> {
        let tags = Tags {
            title: self.track_name.clone(),
            artists: self.artists.iter().map(|a| a.name.clone()).collect(),
            album_title: self.album.name.clone(),
            album_cover: (self.image_retriever)().await,
        };
        Ok(tags)
    }
}

impl ToString for TrackMetadata {
    fn to_string(&self) -> String {
        if self.artists.len() > 3 {
            let artists_name = self
                .artists
                .iter()
                .take(3)
                .map(|artist| artist.name.clone())
                .collect::<Vec<String>>()
                .join(", ");
            return clean_invalid_characters(format!(
                "{}, ... - {}",
                artists_name, self.track_name
            ));
        }

        let artists_name = self
            .artists
            .iter()
            .map(|artist| artist.name.clone())
            .collect::<Vec<String>>()
            .join(", ");
        clean_invalid_characters(format!("{} - {}", artists_name, self.track_name))
    }
}

#[derive(Clone, Debug)]
pub struct ArtistMetadata {
    pub name: String,
}

impl From<librespot::metadata::Artist> for ArtistMetadata {
    fn from(artist: librespot::metadata::Artist) -> Self {
        ArtistMetadata {
            name: artist.name.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AlbumMetadata {
    pub name: String,
    pub cover: Option<Image>,
}

impl From<librespot::metadata::Album> for AlbumMetadata {
    fn from(album: librespot::metadata::Album) -> Self {
        AlbumMetadata {
            name: album.name.clone(),
            cover: album.covers.first().cloned(),
        }
    }
}

fn build_image_retriever(covers: Vec<Image>, session: &Session) -> AsyncFn<Bytes> {
    let session = session.clone();

    Arc::new(move || {
        let covers = covers.clone();
        let session = session.clone();

        Box::pin(async move {
            let cover = covers.first()?;
            session.spclient().get_image(&cover.id).await.ok()
        })
    })
}

async fn retry_metadata_request<T, F, Fut>(
    mut request: F,
    operation: String,
) -> std::result::Result<T, LibrespotError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = std::result::Result<T, LibrespotError>>,
{
    let mut attempt = 0;

    loop {
        match request().await {
            Ok(value) => return Ok(value),
            Err(err) if should_retry_metadata_error(&err) && attempt < METADATA_MAX_RETRIES => {
                attempt += 1;
                let delay_ms = METADATA_RETRY_BASE_DELAY_MS * (1_u64 << (attempt - 1));
                tracing::warn!(
                    "Retrying {} after {} (attempt {}/{}) in {}ms",
                    operation,
                    describe_librespot_error(&err),
                    attempt,
                    METADATA_MAX_RETRIES,
                    delay_ms
                );
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
            Err(err) => {
                if should_retry_metadata_error(&err) {
                    tracing::error!(
                        "Exhausted metadata retries for {} after {} attempts: {}",
                        operation,
                        METADATA_MAX_RETRIES,
                        describe_librespot_error(&err)
                    );
                }
                return Err(err);
            }
        }
    }
}

fn should_retry_metadata_error(err: &LibrespotError) -> bool {
    matches!(
        err.kind,
        LibrespotErrorKind::ResourceExhausted | LibrespotErrorKind::Unavailable
    )
}

fn describe_librespot_error(err: &LibrespotError) -> String {
    format!("{} {{ {} }}", err.kind, err.error)
}
