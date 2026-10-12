use std::collections::HashMap;

use zbus::{
    Connection, MessageStream,
    match_rule::MatchRule,
    message::Type,
    zvariant::{OwnedObjectPath, OwnedValue},
};

const MPRIS_PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER_IFACE: &str = "org.mpris.MediaPlayer2.Player";
const ROOT_IFACE: &str = "org.mpris.MediaPlayer2";
const PROPERTIES_IFACE: &str = "org.freedesktop.DBus.Properties";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub player: String,
    pub identity: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub art_url: String,
    pub track_path: String,
    pub playback_status: String,
    pub position: f64,
    pub duration: f64,
}

impl Track {
    pub fn key(&self) -> String {
        format!(
            "{}\0{}\0{}\0{}\0{}",
            self.player, self.title, self.artist, self.art_url, self.track_path
        )
    }
}

#[derive(Clone, Debug)]
pub enum PlayerCommand {
    Previous,
    Play,
    Pause,
    Next,
    SeekTo(f64),
}

pub async fn connect() -> Option<Connection> {
    Connection::session().await.ok()
}

/// Fires whenever any MPRIS player emits a signal (property or seek changes).
pub async fn change_stream(connection: &Connection) -> Option<MessageStream> {
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .path(MPRIS_PATH)
        .ok()?
        .build();
    MessageStream::for_match_rule(rule, connection, None).await.ok()
}

pub async fn control(
    player: String,
    track_path: String,
    current_position: f64,
    command: PlayerCommand,
) -> Result<(), String> {
    let connection = Connection::session()
        .await
        .map_err(|error| error.to_string())?;
    let proxy = zbus::Proxy::new(&connection, player, MPRIS_PATH, PLAYER_IFACE)
        .await
        .map_err(|error| error.to_string())?;

    let result = match command {
        PlayerCommand::Previous => proxy.call_method("Previous", &()).await,
        PlayerCommand::Play => proxy.call_method("Play", &()).await,
        PlayerCommand::Pause => proxy.call_method("Pause", &()).await,
        PlayerCommand::Next => proxy.call_method("Next", &()).await,
        PlayerCommand::SeekTo(seconds) => {
            let micros = (seconds.max(0.0) * 1_000_000.0) as i64;
            if track_path.is_empty() {
                let offset = ((seconds - current_position) * 1_000_000.0) as i64;
                proxy.call_method("Seek", &(offset,)).await
            } else {
                let path = OwnedObjectPath::try_from(track_path.as_str())
                    .map_err(|error| error.to_string())?;
                proxy.call_method("SetPosition", &(path, micros)).await
            }
        }
    };
    result.map(|_| ()).map_err(|error| error.to_string())
}

pub async fn available_tracks(connection: &Connection) -> Vec<Track> {
    let Ok(dbus) = zbus::fdo::DBusProxy::new(connection).await else {
        return Vec::new();
    };
    let mut players = dbus
        .list_names()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|name| name.to_string())
        .filter(|name| name.starts_with("org.mpris.MediaPlayer2.") && !name.contains("playerctld"))
        .collect::<Vec<_>>();
    players.sort();

    let mut candidates = Vec::new();
    for player in players {
        if let Some(track) = read_player(connection, &player).await {
            if !track.title.is_empty() {
                candidates.push(track);
            }
        }
    }
    candidates
}

pub fn choose_active(tracks: &[Track]) -> Option<&Track> {
    tracks
        .iter()
        .find(|track| track.playback_status == "Playing")
        .or_else(|| tracks.first())
}

fn string_prop(props: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    props
        .get(key)
        .and_then(|value| String::try_from(value.clone()).ok())
}

async fn all_properties(
    connection: &Connection,
    player: &str,
    interface: &str,
) -> Option<HashMap<String, OwnedValue>> {
    let proxy = zbus::Proxy::new(connection, player, MPRIS_PATH, PROPERTIES_IFACE)
        .await
        .ok()?;
    proxy.call("GetAll", &(interface,)).await.ok()
}

async fn read_player(connection: &Connection, player: &str) -> Option<Track> {
    let props = all_properties(connection, player, PLAYER_IFACE).await?;
    let root = all_properties(connection, player, ROOT_IFACE)
        .await
        .unwrap_or_default();

    let metadata: HashMap<String, OwnedValue> = props
        .get("Metadata")
        .and_then(|value| HashMap::try_from(value.try_clone().ok()?).ok())
        .unwrap_or_default();
    let artist = metadata
        .get("xesam:artist")
        .and_then(|value| Vec::<String>::try_from(value.clone()).ok())
        .unwrap_or_default()
        .join(", ");
    let track_path = metadata
        .get("mpris:trackid")
        .and_then(|value| OwnedObjectPath::try_from(value.clone()).ok())
        .map(|value| value.to_string())
        .unwrap_or_default();
    let duration = metadata
        .get("mpris:length")
        .and_then(|value| i64::try_from(value.clone()).ok())
        .unwrap_or_default() as f64
        / 1_000_000.0;
    let position = props
        .get("Position")
        .and_then(|value| i64::try_from(value.clone()).ok())
        .unwrap_or_default() as f64
        / 1_000_000.0;

    Some(Track {
        player: player.to_owned(),
        identity: string_prop(&root, "Identity").unwrap_or_else(|| player.to_owned()),
        title: metadata
            .get("xesam:title")
            .and_then(|value| String::try_from(value.clone()).ok())
            .unwrap_or_default(),
        artist,
        album: metadata
            .get("xesam:album")
            .and_then(|value| String::try_from(value.clone()).ok())
            .unwrap_or_default(),
        art_url: metadata
            .get("mpris:artUrl")
            .and_then(|value| String::try_from(value.clone()).ok())
            .unwrap_or_default(),
        track_path,
        playback_status: string_prop(&props, "PlaybackStatus").unwrap_or_default(),
        position,
        duration,
    })
}
