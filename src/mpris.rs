use crate::api::Station;
use futures::SinkExt;
use mpris_server::{Metadata, PlaybackStatus, Player, TrackId};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// Commands from D-Bus clients to the app
#[derive(Debug, Clone)]
pub enum MprisCommand {
    Play,
    Pause,
    PlayPause,
    Stop,
    SetVolume(f64),
    Raise,
    Quit,
}

/// State updates from the app to the MPRIS server
#[derive(Debug, Clone)]
pub enum MprisStateUpdate {
    Playing { station: Box<Station> },
    Stopped,
    Volume(u8),
}

/// Events yielded by the MPRIS subscription
#[derive(Debug, Clone)]
pub enum MprisEvent {
    Ready(mpsc::UnboundedSender<MprisStateUpdate>),
    Command(MprisCommand),
}

/// Convert app volume (0-100 u8) to MPRIS volume (0.0-1.0 f64)
#[must_use]
pub fn volume_to_mpris(vol: u8) -> f64 {
    f64::from(vol.min(100)) / 100.0
}

/// Convert MPRIS volume (0.0-1.0 f64) to app volume (0-100 u8)
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn volume_from_mpris(vol: f64) -> u8 {
    (vol.clamp(0.0, 1.0) * 100.0).round() as u8
}

/// Build MPRIS metadata from a Station
pub fn build_metadata(station: &Station) -> Metadata {
    let mut builder = Metadata::builder().title(&station.name);

    if !station.stationuuid.is_empty() {
        let sanitized = station.stationuuid.replace('-', "_");
        let path = format!("/org/mpris/MediaPlayer2/Track/{sanitized}");
        if let Ok(track_id) = TrackId::try_from(path) {
            builder = builder.trackid(track_id);
        }
    }

    if !station.favicon.is_empty() {
        builder = builder.art_url(&station.favicon);
    }

    if !station.homepage.is_empty() {
        builder = builder.url(&station.homepage);
    }

    if !station.tags.is_empty() {
        let genres: Vec<&str> = station.tags.split(',').map(str::trim).collect();
        builder = builder.genre(genres);
    }

    builder.build()
}

/// Spawn the MPRIS server on a dedicated OS thread.
///
/// Returns a sender for pushing state updates to the MPRIS server.
/// Commands from D-Bus clients are forwarded via `cmd_tx`.
fn spawn_mpris_thread(
    cmd_tx: mpsc::UnboundedSender<MprisCommand>,
) -> mpsc::UnboundedSender<MprisStateUpdate> {
    let (state_tx, state_rx) = mpsc::unbounded_channel();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create tokio runtime for MPRIS");

        let local = tokio::task::LocalSet::new();

        local.block_on(&rt, async move {
            match run_mpris_server(cmd_tx, state_rx).await {
                Ok(()) => info!("MPRIS server shut down"),
                Err(e) => error!("MPRIS server error: {}", e),
            }
        });
    });

    state_tx
}

/// Run the MPRIS server event loop (must be called on a `LocalSet`)
async fn run_mpris_server(
    cmd_tx: mpsc::UnboundedSender<MprisCommand>,
    mut state_rx: mpsc::UnboundedReceiver<MprisStateUpdate>,
) -> Result<(), Box<dyn std::error::Error>> {
    let player = Player::builder("cosmic_radio_applet")
        .identity("COSMIC Radio")
        .desktop_entry("com.marcos.RadioApplet")
        .can_play(true)
        .can_pause(true)
        .can_control(true)
        .can_seek(false)
        .can_go_next(false)
        .can_go_previous(false)
        .build()
        .await?;

    // Wire D-Bus method calls to MprisCommand channel
    {
        let tx = cmd_tx.clone();
        player.connect_play(move |_| {
            let _ = tx.send(MprisCommand::Play);
        });
    }
    {
        let tx = cmd_tx.clone();
        player.connect_pause(move |_| {
            let _ = tx.send(MprisCommand::Pause);
        });
    }
    {
        let tx = cmd_tx.clone();
        player.connect_play_pause(move |_| {
            let _ = tx.send(MprisCommand::PlayPause);
        });
    }
    {
        let tx = cmd_tx.clone();
        player.connect_stop(move |_| {
            let _ = tx.send(MprisCommand::Stop);
        });
    }
    {
        let tx = cmd_tx.clone();
        player.connect_set_volume(move |_, vol| {
            let _ = tx.send(MprisCommand::SetVolume(vol));
        });
    }
    {
        let tx = cmd_tx.clone();
        player.connect_raise(move |_| {
            let _ = tx.send(MprisCommand::Raise);
        });
    }
    {
        let tx = cmd_tx;
        player.connect_quit(move |_| {
            let _ = tx.send(MprisCommand::Quit);
        });
    }

    debug!("MPRIS server started on D-Bus");

    // Run the D-Bus event loop as a background local task
    tokio::task::spawn_local(player.run());

    // Process state updates from the app
    while let Some(update) = state_rx.recv().await {
        match update {
            MprisStateUpdate::Playing { station } => {
                let metadata = build_metadata(station.as_ref());
                if let Err(e) = player.set_metadata(metadata).await {
                    warn!("Failed to set MPRIS metadata: {}", e);
                }
                if let Err(e) = player
                    .set_playback_status(PlaybackStatus::Playing)
                    .await
                {
                    warn!("Failed to set MPRIS playback status: {}", e);
                }
            }
            MprisStateUpdate::Stopped => {
                if let Err(e) = player
                    .set_playback_status(PlaybackStatus::Stopped)
                    .await
                {
                    warn!("Failed to set MPRIS playback status: {}", e);
                }
            }
            MprisStateUpdate::Volume(vol) => {
                if let Err(e) = player.set_volume(volume_to_mpris(vol)).await {
                    warn!("Failed to set MPRIS volume: {}", e);
                }
            }
        }
    }

    Ok(())
}

/// Create an iced Subscription that runs the MPRIS server and forwards events
pub fn mpris_subscription() -> cosmic::iced::Subscription<MprisEvent> {
    cosmic::iced::Subscription::run(|| {
        cosmic::iced::stream::channel(100, |mut output| async move {
            let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel();
            let state_tx = spawn_mpris_thread(cmd_tx);

            if output.send(MprisEvent::Ready(state_tx)).await.is_err() {
                return;
            }

            while let Some(cmd) = cmd_rx.recv().await {
                if output.send(MprisEvent::Command(cmd)).await.is_err() {
                    break;
                }
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_volume_to_mpris() {
        assert!((volume_to_mpris(0) - 0.0).abs() < f64::EPSILON);
        assert!((volume_to_mpris(50) - 0.5).abs() < f64::EPSILON);
        assert!((volume_to_mpris(100) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_volume_to_mpris_clamps_above_100() {
        assert!((volume_to_mpris(255) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_volume_from_mpris() {
        assert_eq!(volume_from_mpris(0.0), 0);
        assert_eq!(volume_from_mpris(0.5), 50);
        assert_eq!(volume_from_mpris(1.0), 100);
    }

    #[test]
    fn test_volume_from_mpris_clamps() {
        assert_eq!(volume_from_mpris(-0.5), 0);
        assert_eq!(volume_from_mpris(1.5), 100);
    }

    #[test]
    fn test_volume_roundtrip() {
        for vol in 0..=100u8 {
            assert_eq!(volume_from_mpris(volume_to_mpris(vol)), vol);
        }
    }

    #[test]
    fn test_build_metadata_full_station() {
        let station = Station {
            stationuuid: "96202c39-0601-11e8-ae97-52543be04c81".to_string(),
            name: "SomaFM - Groove Salad".to_string(),
            url: "https://somafm.com/groovesalad/".to_string(),
            url_resolved: "https://ice1.somafm.com/groovesalad-128-mp3".to_string(),
            homepage: "https://somafm.com".to_string(),
            favicon: "https://somafm.com/favicon.ico".to_string(),
            tags: "ambient,electronic,chillout".to_string(),
            country: "USA".to_string(),
            language: "English".to_string(),
        };

        let metadata = build_metadata(&station);
        assert!(format!("{metadata:?}").contains("SomaFM"));
    }

    #[test]
    fn test_build_metadata_empty_station() {
        let station = Station::default();
        let _metadata = build_metadata(&station);
    }

    #[test]
    fn test_build_metadata_missing_optional_fields() {
        let station = Station {
            name: "Minimal Station".to_string(),
            ..Default::default()
        };
        let _metadata = build_metadata(&station);
    }

    #[test]
    fn test_mpris_command_debug() {
        let cmd = MprisCommand::Play;
        assert_eq!(format!("{cmd:?}"), "Play");

        let cmd = MprisCommand::SetVolume(0.75);
        assert!(format!("{cmd:?}").contains("0.75"));
    }

    #[test]
    fn test_mpris_state_update_debug() {
        let update = MprisStateUpdate::Stopped;
        assert_eq!(format!("{update:?}"), "Stopped");

        let update = MprisStateUpdate::Volume(50);
        assert!(format!("{update:?}").contains("50"));
    }
}
