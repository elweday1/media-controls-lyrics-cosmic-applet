use crate::{
    artwork,
    config::Config,
    lyrics::{self, FetchResult, Line},
    mpris::{self, PlayerCommand, Track},
    visualizer::{self, BARS},
};
use cosmic::{
    Element,
    cosmic_config::{self, CosmicConfigEntry},
    iced::{
        Alignment, Length, Limits, Subscription,
        futures::{SinkExt, StreamExt},
        platform_specific::shell::wayland::commands::popup::destroy_popup,
        window::Id,
    },
    surface,
    widget,
};
use cosmic::iced::widget::scrollable::{scroll_to, AbsoluteOffset, Direction, Scrollbar};
use std::time::{Duration, Instant};

const ART_CACHE_SIZE: usize = 12;
const BAR_WIDTH: f32 = 3.0;
const BAR_SPACING: f32 = 2.0;
const BAR_MIN_HEIGHT: f32 = 2.0;
const BAR_MAX_HEIGHT: f32 = 16.0;

#[derive(Default)]
pub struct AppModel {
    core: cosmic::Core,
    popup: Option<Id>,
    track: Option<Track>,
    players: Vec<Track>,
    selected_player: String,
    player_override: bool,
    track_key: String,
    config: Config,
    album_art: Option<widget::image::Handle>,
    art_cache: Vec<(String, widget::image::Handle)>,
    lyrics: Vec<Line>,
    lyric_source: Option<String>,
    anchored_at: Option<Instant>,
    seek_preview: f32,
    seeking: bool,
    control_error: Option<String>,
    show_lyrics: bool,
    marquee_tick: u32,
    bars: Vec<u8>,
}

#[derive(Debug, Clone)]
pub enum Message {
    TogglePopup,
    ToggleLyrics,
    PopupClosed(Id),
    Surface(surface::Action<Message>),
    PlayersUpdated(Vec<Track>),
    SelectPlayer(String),
    Control(PlayerCommand),
    CommandFinished(Result<(), String>),
    ArtworkFetched(String, Result<Vec<u8>, String>),
    LyricsFetched(String, Result<FetchResult, String>),
    SeekChanged(f32),
    SeekReleased,
    Spectrum(Vec<u8>),
    Tick,
}

impl cosmic::Application for AppModel {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;

    const APP_ID: &'static str = "io.github.nasser.CosmicExtMusicLyrics";

    fn core(&self) -> &cosmic::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::Core {
        &mut self.core
    }

    fn init(core: cosmic::Core, _flags: Self::Flags) -> (Self, cosmic::Task<cosmic::Action<Message>>) {
        let config = cosmic_config::Config::new(Self::APP_ID, Config::VERSION)
            .ok()
            .and_then(|context| Config::get_entry(&context).ok())
            .unwrap_or_default();
        (
            Self {
                core,
                config,
                bars: vec![0; BARS],
                ..Self::default()
            },
            cosmic::Task::none(),
        )
    }

    fn on_close_requested(&self, id: Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    fn view(&self) -> Element<'_, Message> {
        let Some(track) = &self.track else {
            return self.core.applet.autosize_window(widget::Space::new()).into();
        };
        let toggle = if self.is_playing() {
            PlayerCommand::Pause
        } else {
            PlayerCommand::Play
        };
        let visualizer = widget::button::custom(self.bars_view())
            .class(cosmic::theme::Button::Text)
            .padding(0)
            .on_press(Message::Control(toggle));
        let label = widget::text::caption(compact(&self.panel_label(track), 24));
        let content = widget::row([])
            .spacing(6)
            .align_y(Alignment::Center)
            .push(visualizer)
            .push(label);
        let item = widget::button::custom(content)
            .class(cosmic::theme::Button::AppletIcon)
            .padding([0, self.core.applet.suggested_padding(true).0])
            .on_press(Message::TogglePopup);
        let item = self.core.applet.applet_tooltip(
            item,
            self.panel_tooltip(track),
            self.popup.is_some(),
            Message::Surface,
            None,
        );
        self.core.applet.autosize_window(item).into()
    }

    fn view_window(&self, _id: Id) -> Element<'_, Message> {
        let content = match &self.track {
            Some(track) => self.now_playing(track),
            None => widget::text::body("Nothing playing").into(),
        };
        self.core
            .applet
            .popup_container(widget::container(content).padding([8, 0]))
            .into()
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subscriptions = vec![Subscription::run(watch_players)];
        if self.is_playing() {
            subscriptions.push(Subscription::run(visualizer::spectrum).map(Message::Spectrum));
            if self.popup.is_some() {
                subscriptions.push(
                    cosmic::iced::time::every(Duration::from_millis(100)).map(|_| Message::Tick),
                );
            }
        }
        Subscription::batch(subscriptions)
    }

    fn update(&mut self, message: Message) -> cosmic::Task<cosmic::Action<Message>> {
        match message {
            Message::TogglePopup => {
                return if let Some(popup) = self.popup.take() {
                    cosmic::surface::surface_task(cosmic::surface::action::destroy_popup(popup))
                } else {
                    cosmic::surface::surface_task(cosmic::surface::action::app_popup(
                        |_| Default::default(),
                        |app: &mut Self| {
                            let id = Id::unique();
                            app.popup = Some(id);
                            let mut settings = app.core.applet.get_popup_settings(
                                app.core.main_window_id().unwrap(),
                                id,
                                None,
                                None,
                                None,
                            );
                            settings.positioner.size_limits = Limits::NONE
                                .min_width(340.0)
                                .max_width(440.0)
                                .min_height(120.0)
                                .max_height(560.0);
                            settings
                        },
                        None,
                    ))
                };
            }
            Message::ToggleLyrics => self.show_lyrics = !self.show_lyrics,
            Message::Surface(action) => {
                return cosmic::task::message(cosmic::Action::Surface(action));
            }
            Message::PopupClosed(id) => {
                if self.popup == Some(id) {
                    self.popup = None;
                }
            }
            Message::PlayersUpdated(players) => {
                if players == self.players {
                    return cosmic::Task::none();
                }
                self.players = players;
                let selected = self
                    .players
                    .iter()
                    .find(|track| track.player == self.selected_player);
                if self.player_override && selected.is_none() {
                    self.player_override = false;
                }
                let chosen = if self.player_override {
                    selected
                } else {
                    mpris::choose_active(&self.players)
                }
                .cloned();
                if let Some(track) = chosen {
                    self.selected_player.clone_from(&track.player);
                    return self.activate_track(track);
                }
                self.track = None;
                self.track_key.clear();
                self.album_art = None;
                self.lyrics.clear();
                self.lyric_source = None;
                self.anchored_at = None;
                self.control_error = None;
                self.bars.iter_mut().for_each(|bar| *bar = 0);
                if let Some(popup) = self.popup.take() {
                    return destroy_popup(popup);
                }
            }
            Message::SelectPlayer(player) => {
                self.selected_player.clone_from(&player);
                self.player_override = true;
                if let Some(track) = self.players.iter().find(|track| track.player == player).cloned() {
                    return self.activate_track(track);
                }
            }
            Message::Control(command) => {
                let position = self.position();
                let task = self.control_task(command.clone());
                if let Some(track) = self.track.as_mut() {
                    match command {
                        PlayerCommand::Play => track.playback_status = "Playing".to_owned(),
                        PlayerCommand::Pause => track.playback_status = "Paused".to_owned(),
                        _ => {}
                    }
                    if matches!(command, PlayerCommand::Play | PlayerCommand::Pause) {
                        track.position = position;
                        self.anchored_at = Some(Instant::now());
                    }
                }
                return task;
            }
            Message::CommandFinished(result) => {
                self.control_error = result.err().map(|_| "This player rejected the control request.".to_owned());
            }
            Message::ArtworkFetched(url, result) => {
                if let Ok(bytes) = result {
                    let Some((width, height, pixels)) = artwork::thumbnail(&bytes) else {
                        return cosmic::Task::none();
                    };
                    let handle = widget::image::Handle::from_rgba(width, height, pixels);
                    self.cache_art(url.clone(), handle.clone());
                    if self.track.as_ref().is_some_and(|track| track.art_url == url) {
                        self.album_art = Some(handle);
                    }
                }
            }
            Message::LyricsFetched(key, result) => {
                if key == self.track_key {
                    match result {
                        Ok(result) => {
                            self.lyrics = result.lines;
                            self.lyric_source = Some(result.source);
                        }
                        Err(_) => {
                            self.lyrics.clear();
                            self.lyric_source = None;
                        }
                    }
                }
            }
            Message::SeekChanged(position) => {
                self.seek_preview = position;
                self.seeking = true;
            }
            Message::SeekReleased => {
                self.seeking = false;
                let target = self.seek_preview as f64;
                let task = self.control_task(PlayerCommand::SeekTo(target));
                if let Some(track) = self.track.as_mut() {
                    track.position = target;
                    self.anchored_at = Some(Instant::now());
                }
                return task;
            }
            Message::Spectrum(frame) => self.bars = frame,
            Message::Tick => {
                self.marquee_tick = self.marquee_tick.wrapping_add(1);
                return self.marquee_tasks();
            }
        }
        cosmic::Task::none()
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }
}

impl AppModel {
    fn is_playing(&self) -> bool {
        self.track
            .as_ref()
            .is_some_and(|track| track.playback_status == "Playing")
    }

    /// Playback position interpolated between player reads, so the UI moves smoothly
    /// without polling the player every frame.
    fn position(&self) -> f64 {
        let Some(track) = &self.track else {
            return 0.0;
        };
        let elapsed = match (self.is_playing(), self.anchored_at) {
            (true, Some(anchor)) => anchor.elapsed().as_secs_f64(),
            _ => 0.0,
        };
        let position = track.position + elapsed;
        if track.duration > 0.0 {
            position.min(track.duration)
        } else {
            position
        }
        .max(0.0)
    }

    fn displayed_position(&self) -> f64 {
        if self.seeking {
            self.seek_preview as f64
        } else {
            self.position()
        }
    }

    fn current_line_index(&self) -> Option<usize> {
        lyrics::current_line(&self.lyrics, self.position())
    }

    fn current_lyric(&self) -> Option<&str> {
        let index = self.current_line_index()?;
        self.lyrics.get(index).map(|line| line.text.as_str())
    }

    fn panel_label(&self, track: &Track) -> String {
        if self.show_lyrics {
            if let Some(line) = self.current_lyric() {
                return line.to_owned();
            }
        }
        track.title.clone()
    }

    fn panel_tooltip(&self, track: &Track) -> String {
        if self.show_lyrics {
            if track.artist.is_empty() {
                track.title.clone()
            } else {
                format!("{} — {}", track.title, track.artist)
            }
        } else {
            self.current_lyric().unwrap_or("No lyrics").to_owned()
        }
    }

    fn bars_view(&self) -> Element<'_, Message> {
        let playing = self.is_playing();
        let mut row = widget::row([])
            .spacing(BAR_SPACING)
            .height(Length::Fixed(BAR_MAX_HEIGHT))
            .align_y(Alignment::End);
        for index in 0..BARS {
            let level = if playing {
                self.bars.get(index).copied().unwrap_or(0) as f32
            } else {
                0.0
            };
            let height = (BAR_MIN_HEIGHT + level / 100.0 * (BAR_MAX_HEIGHT - BAR_MIN_HEIGHT))
                .clamp(BAR_MIN_HEIGHT, BAR_MAX_HEIGHT);
            row = row.push(
                widget::container(widget::Space::new())
                    .width(Length::Fixed(BAR_WIDTH))
                    .height(Length::Fixed(height))
                    .class(cosmic::theme::Container::custom(bar_style)),
            );
        }
        row.into()
    }

    fn activate_track(&mut self, track: Track) -> cosmic::Task<cosmic::Action<Message>> {
        let key = track.key();
        let changed = key != self.track_key;
        let art_url = track.art_url.clone();
        self.track = Some(track.clone());
        self.anchored_at = Some(Instant::now());
        if !changed {
            return cosmic::Task::none();
        }

        self.track_key.clone_from(&key);
        self.album_art = self.cached_art(&art_url);
        self.lyrics.clear();
        self.lyric_source = None;
        self.control_error = None;
        self.seeking = false;

        let mut tasks = vec![self.lyrics_task(track)];
        if !art_url.is_empty() && self.album_art.is_none() {
            let url = art_url.clone();
            tasks.push(cosmic::Task::perform(artwork::fetch(art_url), move |result| {
                cosmic::Action::App(Message::ArtworkFetched(url, result))
            }));
        }
        cosmic::Task::batch(tasks)
    }

    fn cached_art(&self, url: &str) -> Option<widget::image::Handle> {
        if url.is_empty() {
            return None;
        }
        self.art_cache
            .iter()
            .find(|(cached, _)| cached == url)
            .map(|(_, handle)| handle.clone())
    }

    fn cache_art(&mut self, url: String, handle: widget::image::Handle) {
        self.art_cache.retain(|(cached, _)| *cached != url);
        self.art_cache.push((url, handle));
        if self.art_cache.len() > ART_CACHE_SIZE {
            let _ = self.art_cache.remove(0);
        }
    }

    fn lyrics_task(&self, track: Track) -> cosmic::Task<cosmic::Action<Message>> {
        let key = track.key();
        cosmic::Task::perform(
            lyrics::fetch(
                track.title,
                track.artist,
                track.album,
                track.duration,
                self.config.clone(),
            ),
            move |result| cosmic::Action::App(Message::LyricsFetched(key, result)),
        )
    }

    fn control_task(&self, command: PlayerCommand) -> cosmic::Task<cosmic::Action<Message>> {
        let Some(track) = &self.track else {
            return cosmic::Task::none();
        };
        cosmic::Task::perform(
            mpris::control(
                track.player.clone(),
                track.track_path.clone(),
                track.position,
                command,
            ),
            |result| cosmic::Action::App(Message::CommandFinished(result)),
        )
    }

    fn art(&self, size: f32) -> Element<'_, Message> {
        match &self.album_art {
            Some(handle) => widget::image(handle.clone())
                .width(Length::Fixed(size))
                .height(Length::Fixed(size))
                .border_radius([14.0; 4])
                .into(),
            None => widget::container(
                widget::icon::from_name("multimedia-audio-player-symbolic").size((size * 0.5) as u16),
            )
            .width(Length::Fixed(size))
            .height(Length::Fixed(size))
            .center(Length::Fill)
            .class(cosmic::theme::Container::Card)
            .into(),
        }
    }

    fn now_playing(&self, track: &Track) -> Element<'_, Message> {
        let playing = self.is_playing();
        let position = self.displayed_position();
        let duration = track.duration.max(0.0);
        let artist = if track.artist.is_empty() {
            "Unknown artist".to_owned()
        } else {
            track.artist.clone()
        };

        let transport = widget::row([])
            .spacing(14)
            .align_y(Alignment::Center)
            .push(transport_button("media-skip-backward-symbolic", PlayerCommand::Previous, false))
            .push(transport_button(
                if playing { "media-playback-pause-symbolic" } else { "media-playback-start-symbolic" },
                if playing { PlayerCommand::Pause } else { PlayerCommand::Play },
                true,
            ))
            .push(transport_button("media-skip-forward-symbolic", PlayerCommand::Next, false));

        let info = widget::column([])
            .width(Length::Fixed(INFO_WIDTH))
            .spacing(0)
            .push(scrolling_text(TITLE_ID, &track.title, title_size(&track.title), true))
            .push(scrolling_text(ARTIST_ID, &artist, 14.0, false))
            .push(scrolling_text(ALBUM_ID, &track.album, 12.0, false))
            .push(
                widget::container(transport).width(Length::Fixed(INFO_WIDTH)),
            );

        let header = widget::row([])
            .spacing(12)
            .align_y(Alignment::Center)
            .push(self.art(96.0))
            .push(info);

        let progress = widget::row([])
            .spacing(8)
            .align_y(Alignment::Center)
            .push(widget::text::caption(format_time(position)))
            .push(
                widget::slider(0.0..=(duration as f32).max(1.0), position as f32, Message::SeekChanged)
                    .step(0.5_f32)
                    .on_release(Message::SeekReleased)
                    .width(Length::Fill),
            )
            .push(widget::text::caption(format_time(duration)));

        let has_lyrics = !self.lyrics.is_empty();
        let mut footer = widget::row([]).spacing(6).align_y(Alignment::Center);
        for player in &self.players {
            let selected = player.player == self.selected_player;
            footer = footer.push(
                widget::button::custom(widget::text::caption(player.identity.clone()))
                    .padding([2, 8])
                    .class(if selected {
                        cosmic::theme::Button::Suggested
                    } else {
                        cosmic::theme::Button::Standard
                    })
                    .on_press(Message::SelectPlayer(player.player.clone())),
            );
        }
        if has_lyrics {
            footer = footer.push(widget::space::horizontal()).push(
                widget::button::icon(widget::icon::from_name("format-justify-left-symbolic").size(16))
                    .class(if self.show_lyrics {
                        cosmic::theme::Button::Suggested
                    } else {
                        cosmic::theme::Button::Icon
                    })
                    .on_press(Message::ToggleLyrics),
            );
        }

        let mut content = widget::column([])
            .spacing(10)
            .push(padded(header))
            .push(padded(progress));

        if has_lyrics && self.show_lyrics {
            content = content
                .push(widget::divider::horizontal::default())
                .push(padded(self.lyrics_block()));
        }
        if self.players.len() > 1 || has_lyrics {
            content = content
                .push(widget::divider::horizontal::default())
                .push(padded(footer));
        }

        if let Some(error) = &self.control_error {
            content = content.push(padded(widget::text::caption(error.clone())));
        }

        content.into()
    }

    fn marquee_tasks(&self) -> cosmic::Task<cosmic::Action<Message>> {
        let Some(track) = &self.track else {
            return cosmic::Task::none();
        };
        let artist = if track.artist.is_empty() {
            "Unknown artist".to_owned()
        } else {
            track.artist.clone()
        };
        let tick = self.marquee_tick as f32;
        let tasks = [
            (TITLE_ID, &track.title, title_size(&track.title)),
            (ARTIST_ID, &artist, 14.0),
            (ALBUM_ID, &track.album, 12.0),
        ]
        .into_iter()
        .map(|(id, text, size)| {
            let overflow = overflow_px(text, size);
            let cycle = overflow + MARQUEE_PAUSE;
            let x = if overflow <= 0.0 { 0.0 } else { (tick * MARQUEE_SPEED) % cycle };
            scroll_to(
                widget::Id::new(id),
                AbsoluteOffset { x: Some(x.min(overflow)), y: None },
            )
        });
        cosmic::Task::batch(tasks)
    }

    fn lyrics_block(&self) -> Element<'_, Message> {
        let selected = self.current_line_index().unwrap_or(0);
        let mut lines = widget::column([])
            .spacing(4)
            .width(Length::Fill)
            .align_x(Alignment::Center);
        for index in lyrics::visible_window(self.lyrics.len(), selected) {
            let text = self.lyrics[index].text.clone();
            let line: Element<'_, Message> = if index == selected {
                widget::text::heading(text).width(Length::Fill).align_x(Alignment::Center).into()
            } else {
                widget::text::caption(text).width(Length::Fill).align_x(Alignment::Center).into()
            };
            lines = lines.push(line);
        }
        lines.into()
    }
}

const TITLE_ID: &str = "title";
const ARTIST_ID: &str = "artist";
const ALBUM_ID: &str = "album";
const INFO_WIDTH: f32 = 280.0;
const MARQUEE_SPEED: f32 = 1.2;
const MARQUEE_PAUSE: f32 = 60.0;

fn title_size(text: &str) -> f32 {
    match text.chars().count() {
        0..=14 => 22.0,
        15..=22 => 18.0,
        _ => 16.0,
    }
}

fn overflow_px(text: &str, size: f32) -> f32 {
    (text.chars().count() as f32 * size * 0.55 - INFO_WIDTH).max(0.0)
}

fn scrolling_text<'a>(id: &'static str, text: &str, size: f32, bold: bool) -> Element<'a, Message> {
    let mut label = widget::text(text.to_owned()).size(size);
    if bold {
        label = label.font(cosmic::font::bold());
    }
    widget::scrollable(label)
        .id(widget::Id::new(id))
        .direction(Direction::Horizontal(Scrollbar::hidden()))
        .width(Length::Fixed(INFO_WIDTH))
        .into()
}

fn padded<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    widget::container(content).padding([0, 14]).width(Length::Fill).into()
}

fn watch_players() -> impl futures::Stream<Item = Message> {
    cosmic::iced::stream::channel(4, |mut sender: futures::channel::mpsc::Sender<Message>| async move {
        loop {
            let Some(connection) = mpris::connect().await else {
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            };
            let mut changes = mpris::change_stream(&connection).await;
            loop {
                let players = mpris::available_tracks(&connection).await;
                if sender.send(Message::PlayersUpdated(players)).await.is_err() {
                    return;
                }
                let timeout = Box::pin(tokio::time::sleep(Duration::from_secs(1)));
                let Some(stream) = changes.as_mut() else {
                    timeout.await;
                    continue;
                };
                match futures::future::select(timeout, stream.next()).await {
                    futures::future::Either::Left(_) => {}
                    futures::future::Either::Right((signal, _)) => {
                        if signal.is_none() {
                            break;
                        }
                        // Coalesce bursts of property changes into one refresh.
                        tokio::time::sleep(Duration::from_millis(30)).await;
                    }
                }
            }
        }
    })
}

fn transport_button<'a>(
    icon: &'static str,
    command: PlayerCommand,
    prominent: bool,
) -> Element<'a, Message> {
    let button = widget::button::icon(widget::icon::from_name(icon).size(if prominent { 20 } else { 16 }))
        .on_press(Message::Control(command));
    if prominent {
        button.class(cosmic::theme::Button::Suggested).into()
    } else {
        button.into()
    }
}

fn bar_style(theme: &cosmic::Theme) -> cosmic::iced::widget::container::Style {
    let cosmic = theme.cosmic();
    cosmic::iced::widget::container::Style {
        background: Some(cosmic::iced::Background::Color(cosmic::iced::Color::from(
            cosmic.accent.base,
        ))),
        border: cosmic::iced::Border {
            radius: 1.5.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn compact(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let shortened = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{shortened}…")
    } else {
        shortened
    }
}

fn format_time(seconds: f64) -> String {
    let seconds = seconds.max(0.0) as u64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}
