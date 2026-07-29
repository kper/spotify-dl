use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use librespot::core::Session;
use librespot::playback::config::{Bitrate, PlayerConfig};
use librespot::playback::mixer::NoOpVolume;
use librespot::playback::player::{Player, PlayerEvent};
use tokio::sync::mpsc::UnboundedSender;

use crate::stream::channel_sink::{ChannelSink, SinkEvent};
use crate::stream::{StreamError, StreamEvent, StreamEventChannel};
use crate::track::Track;

const TRACK_LOAD_RETRIES: u32 = 6;
const TRACK_LOAD_BASE_BACKOFF_SECS: u64 = 10;
const TRACK_LOAD_MAX_BACKOFF_SECS: u64 = 120;

pub struct Stream {
    player_config: PlayerConfig,
    session: Session,
}

impl Stream {
    pub fn new(session: Session) -> Self {
        let config = PlayerConfig {
            bitrate: Bitrate::Bitrate320,
            ..Default::default()
        };
        Stream {
            player_config: config,
            session,
        }
    }

    pub async fn stream(&self, track: Track) -> Result<StreamEventChannel> {
        let metadata = track.metadata(&self.session).await?;
        let track_id = track.id.clone();
        let (sink, mut channel) = ChannelSink::new(metadata);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

        let player = Player::new(
            self.player_config.clone(),
            self.session.clone(),
            Box::new(NoOpVolume),
            move || Box::new(sink),
        );

        tokio::spawn(async move {
            match tryhard::retry_fn(|| async { Self::load(player.clone(), &track).await })
                .retries(TRACK_LOAD_RETRIES)
                .on_retry(|attempt, _, e| {
                    let error = format!("{}", e);
                    let tx = tx.clone();
                    let track_id = track_id.clone();
                    async move {
                        tracing::warn!(
                            "Attempt {} to load track {:?} failed: {}",
                            attempt,
                            track_id,
                            error
                        );
                        Self::send_event(&tx, StreamEvent::Retry {
                            attempt: attempt as usize,
                            max_attempts: TRACK_LOAD_RETRIES as usize,
                        }).await;
                    }
                })
                .exponential_backoff(Duration::from_secs(TRACK_LOAD_BASE_BACKOFF_SECS))
                .max_delay(Duration::from_secs(TRACK_LOAD_MAX_BACKOFF_SECS))
                .await
            {
                Ok(_) => tracing::info!("Track loaded successfully: {:?}", track_id),
                Err(e) => {
                    tracing::error!("Failed to load track: {:?}, error: {:?}", track_id, e);
                    Self::send_event(
                        &tx,
                        StreamEvent::Error(StreamError::LoadError(format!(
                            "Failed to load track {:?}: {}",
                            track_id, e
                        ))),
                    )
                    .await;
                    return;
                }
            }

            tracing::info!("Streaming track: {:?}", track_id);

            while let Some(event) = channel.recv().await {
                match event {
                    SinkEvent::Write {
                        bytes,
                        total,
                        content,
                    } => {
                        Self::send_event(
                            &tx,
                            StreamEvent::Write {
                                bytes,
                                total,
                                content,
                            },
                        )
                        .await
                    }
                    SinkEvent::Finished => {
                        Self::send_event(&tx, StreamEvent::Finished).await;
                        break;
                    }
                }
            }
        });

        Ok(rx)
    }

    async fn load(player: Arc<Player>, track: &Track) -> Result<()> {
        player.load(track.id.clone(), true, 0);

        tracing::info!("Loading track: {:?}", track.id);
        loop {
            match player.get_player_event_channel().recv().await {
                Some(PlayerEvent::Playing { .. })
                | Some(PlayerEvent::TrackChanged { .. })
                | Some(PlayerEvent::EndOfTrack { .. }) => {
                    tracing::info!("Player started playing track: {:?}", track.id);
                    break;
                }
                Some(PlayerEvent::Unavailable { .. }) => {
                    tracing::info!("Track is unavailable: {:?}", track.id);
                    return Err(anyhow::anyhow!("Could not load track: {:?}", track.id));
                }
                _ => {
                    // Ignore other events
                }
            }
        }

        tokio::spawn(async move {
            player.await_end_of_track().await;
            player.stop();
        });

        Ok(())
    }

    async fn send_event(tx: &UnboundedSender<StreamEvent>, event: StreamEvent) {
        tx.send(event).unwrap_or_else(|e| {
            tracing::error!("Failed to send event: {:?}", e);
        });
    }
}
