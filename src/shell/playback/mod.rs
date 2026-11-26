//! The video that's open: starting one (from the library, or a file named
//! on the command line), what its player says while it plays, and ending
//! it. `NowPlaying` owns the player; the shell shows its view and puts the
//! library back when it says the player closed (`Event`).

use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{App, AppContext as _, Context, Entity, EventEmitter, Subscription, Window};

use super::preferences::Preferences;
use crate::jellyfin::{self, Client, PlaySession, PlaybackInfo, PlaybackReport};
use crate::library::{
    AfterReport, Catalog, Item, Library, Profile, Report, Source, SubtitleTuning, TrackPrefs,
};
use crate::media_session::MediaSession;
use crate::player::{self, Player, PlayerEvent};

mod pause;
mod rules;
mod stream;

use rules::{End, Record};

/// What the open video says to the shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Event {
    /// The player closed and no other video follows: back to the library.
    Closed,
}

/// Where in an item to start playing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StartAt {
    /// Where it was left off.
    ResumePoint,
    Beginning,
}

/// The open video.
struct Active {
    view: Entity<Player>,
    /// None when playing a file passed on the command line.
    source: Option<Source>,
    position: f64,
    duration: f64,
    /// The show being watched, whose track choices are remembered.
    show: Option<String>,
    /// Where subtitle delay and size are kept (`rules::tuning_scope`), for a library item.
    tuning_scope: Option<String>,
    /// Jellyfin playback: the session reports and transcoding belong to.
    session: Option<PlaySession>,
    /// The item's streams: a new quality needs its subtitle list made again.
    playback_info: Option<PlaybackInfo>,
    /// Set while paused (Jellyfin playback).
    pause: Option<pause::Pause>,
    _subscription: Subscription,
}

/// What a video is opened with.
struct Opening {
    url: String,
    http_header: Option<String>,
    playback: player::Playback,
    start: f64,
    /// None for a file passed on the command line.
    source: Option<Source>,
    /// The Jellyfin play session, for a video from a server.
    session: Option<PlaySession>,
}

/// The video that's open, if one is, and what outlives each video.
pub(super) struct NowPlaying {
    profile: Profile,
    library: Entity<Library>,
    active: Option<Active>,
    /// The item whose resume point is being asked from the server.
    starting: Option<String>,
    /// The system's media controls, set up with the first video.
    media: Option<Rc<MediaSession>>,
}

impl EventEmitter<Event> for NowPlaying {}

impl NowPlaying {
    pub(super) fn new(profile: Profile, library: Entity<Library>) -> Self {
        Self {
            profile,
            library,
            active: None,
            starting: None,
            media: None,
        }
    }

    /// The open video's player, to draw.
    pub(super) fn view(&self) -> Option<&Entity<Player>> {
        self.active.as_ref().map(|active| &active.view)
    }

    pub(super) fn is_open(&self) -> bool {
        self.active.is_some()
    }

    /// Shows or hides the skip buttons in the system's media controls.
    pub(super) fn set_media_skip_buttons(&self, on: bool) {
        if let Some(media) = &self.media {
            media.set_skip_buttons(on);
        }
    }

    /// Test runs: saves the video's picture, if a video is open.
    pub(super) fn screenshot(&self, path: &Path, cx: &App) {
        if let Some(active) = &self.active {
            active.view.read(cx).screenshot(path);
        }
    }

    /// What the library holds now.
    fn catalog(&self, cx: &App) -> Arc<Catalog> {
        self.library.read(cx).catalog()
    }

    fn client(&self, server: i64, cx: &App) -> Option<Client> {
        self.catalog(cx).client(server, &self.profile.device_id)
    }

    fn user_id(&self, server: i64, cx: &App) -> Option<String> {
        self.catalog(cx).user_id(server)
    }

    /// The settings saved in the library database.
    fn preferences(&self) -> Preferences<'_> {
        Preferences::of(&self.profile.db)
    }

    /// Shows what went wrong in the header.
    fn show_error(&mut self, error: String, cx: &mut Context<Self>) {
        self.library
            .update(cx, |library, cx| library.set_error(error, cx));
        cx.notify();
    }

    /// Tells the server about the video playing (see `Library::report`).
    fn report(
        &mut self,
        source: &Source,
        report: Report,
        after: AfterReport,
        cx: &mut Context<Self>,
    ) {
        self.library
            .update(cx, |library, cx| library.report(source, report, after, cx));
    }

    /// Plays a file that isn't in the library (named on the command line).
    pub(super) fn play_file(&mut self, file: String, window: &mut Window, cx: &mut Context<Self>) {
        let title = Path::new(&file)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| file.clone());
        let playback = player::Playback {
            title: title.into(),
            subtitle: None,
            episodes: Vec::new(),
            current: None,
            prefs: self.track_prefs(None),
            tuning: SubtitleTuning::default(),
            quality: None,
            thumb: None,
        };
        let opening = Opening {
            url: file,
            http_header: None,
            playback,
            start: 0.0,
            source: None,
            session: None,
        };
        self.open(opening, window, cx);
    }

    /// Plays an item from where it was left off. A Jellyfin item's resume
    /// point is asked from its server first: the one here may be stale.
    pub(super) fn play(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.catalog(cx).item(key).cloned() else {
            return;
        };
        let Source::Jellyfin { server, id } = &item.source else {
            return self.play_at(item, StartAt::ResumePoint, window, cx);
        };
        let (Some(client), Some(user_id)) = (self.client(*server, cx), self.user_id(*server, cx))
        else {
            return self.play_at(item, StartAt::ResumePoint, window, cx);
        };
        if self.starting.is_some() {
            return;
        }
        self.starting = Some(item.key());
        let id = id.clone();
        cx.spawn_in(window, async move |this, cx| {
            let fresh = cx
                .background_executor()
                .spawn(async move { client.user_data(&user_id, &id) })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.starting = None;
                let mut item = item;
                match fresh {
                    Ok(data) => {
                        item.position = jellyfin::ticks_to_seconds(data.playback_position_ticks);
                        item.watched = data.played;
                        let (key, position, watched) = (item.key(), item.position, item.watched);
                        this.library.update(cx, |library, cx| {
                            library.set_play_state(&key, position, watched, cx)
                        });
                    }
                    Err(e) => eprintln!("[jellyfin] resume point: {e:#}; using the saved one"),
                }
                if this.active.is_none() {
                    this.play_at(item, StartAt::ResumePoint, window, cx);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Plays an item at once, from the resume point saved here or from
    /// its beginning.
    pub(super) fn play_saved(
        &mut self,
        key: &str,
        start_at: StartAt,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let item = self.catalog(cx).item(key).cloned();
        if let Some(item) = item {
            self.play_at(item, start_at, window, cx)
        }
    }

    fn play_at(
        &mut self,
        item: Item,
        start_at: StartAt,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rules::Titles { title, subtitle } = rules::titles(&item);
        let (episodes, current) = rules::episodes_around(self.catalog(cx).items(), &item);
        let scope = rules::tuning_scope(&item);
        let tuning = self.profile.db.subtitle_tuning(&scope).unwrap_or_else(|e| {
            eprintln!("[library] reading subtitle tuning: {e}");
            SubtitleTuning::default()
        });
        let playback = player::Playback {
            title,
            subtitle,
            episodes,
            current,
            prefs: self.track_prefs(item.show.as_deref()),
            tuning,
            quality: None,
            thumb: item.thumb.clone(),
        };
        let start = match start_at {
            StartAt::Beginning => 0.0,
            StartAt::ResumePoint => item.resume_position(),
        };
        let key = item.key();
        eprintln!("[play] {} ({key}) from {start:.1}s", item.display_title());
        match &item.source {
            Source::Local { .. } => self.open_file(&item, playback, start, window, cx),
            Source::Jellyfin { .. } => self.open_stream(&item, playback, start, window, cx),
        }
        if let Some(active) = &mut self.active {
            active.show = item.show.clone();
            active.tuning_scope = Some(scope);
        }
    }

    /// Opens a library item that is a file on this computer.
    fn open_file(
        &mut self,
        item: &Item,
        playback: player::Playback,
        start: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Source::Local { path, .. } = &item.source else {
            return;
        };
        if !path.exists() {
            return self.show_error(format!("{} is not available", path.display()), cx);
        }
        let opening = Opening {
            url: path.to_string_lossy().into_owned(),
            http_header: None,
            playback,
            start,
            source: Some(item.source.clone()),
            session: None,
        };
        self.open(opening, window, cx);
    }

    /// Opens a Jellyfin item at the quality chosen for its server, and
    /// tells the server it started.
    fn open_stream(
        &mut self,
        item: &Item,
        playback: player::Playback,
        start: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Source::Jellyfin { server, id } = &item.source else {
            return;
        };
        let Some(client) = self.client(*server, cx) else {
            return self.show_error("That server is no longer signed in.".into(), cx);
        };
        let quality = self.preferences().quality(*server);
        let session = PlaySession {
            id: PlaySession::new_id(),
            transcoding: quality.transcodes(),
        };
        let opening = Opening {
            url: stream::url(&client, id, &session, quality),
            http_header: client.stream_header(),
            playback: player::Playback {
                quality: Some(quality),
                ..playback
            },
            start,
            source: Some(item.source.clone()),
            session: Some(session.clone()),
        };
        if !self.open(opening, window, cx) {
            return;
        }
        let report = Report {
            kind: PlaybackReport::Started,
            position: start,
            paused: false,
            session,
        };
        self.report(&item.source, report, AfterReport::Nothing, cx);
        self.load_server_extras(client, *server, id.clone(), cx);
    }

    /// Remembered track choices: the show's own, then the general ones.
    fn track_prefs(&self, show: Option<&str>) -> TrackPrefs {
        let load = |scope: &str| self.profile.db.track_prefs(scope).unwrap_or_default();
        let general = load(rules::GENERAL_PREFS);
        match show {
            Some(show) => load(&rules::show_scope(show)).or(general),
            None => general,
        }
    }

    /// The system's media controls, made with the first video.
    fn media_controls(&mut self, window: &mut Window) -> Option<Rc<MediaSession>> {
        let skip_buttons = self.preferences().media_skip_buttons();
        if self.media.is_none() && crate::test_window::media_controls() {
            match MediaSession::new(window) {
                Ok(media) => {
                    media.set_skip_buttons(skip_buttons);
                    self.media = Some(Rc::new(media));
                }
                Err(e) => eprintln!("[media] system media controls unavailable: {e:#}"),
            }
        }
        self.media.clone()
    }

    /// Opens the video and its player. False if it couldn't be opened
    /// (the header says why).
    fn open(&mut self, opening: Opening, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Opening {
            url,
            http_header,
            playback,
            start,
            source,
            session,
        } = opening;
        let video = match player::open_video(window, &url, start, http_header.as_deref()) {
            Ok(video) => video,
            Err(e) => {
                self.show_error(format!("Couldn't play {}: {e}", playback.title), cx);
                return false;
            }
        };
        self.library
            .update(cx, |library, cx| library.clear_error(cx));
        let preferences = self.preferences();
        let volume = preferences.volume();
        let sound_on = preferences.sound_on();
        let sound = preferences.sound_settings();
        let controls = preferences.controls();
        let media = self.media_controls(window);
        // Previews open the video a second time: not for transcoded streams,
        // which are for connections too slow for that.
        let previews = playback.quality.is_none_or(|q| !q.transcodes()).then(|| {
            player::Previews::start(&url, http_header.as_deref(), self.profile.paths.previews())
        });
        let view = cx.new(|cx| {
            let mut player = Player::new(video, playback, volume, media, window, cx);
            player.set_controls(controls);
            player.start_sound(sound_on, sound);
            if let Some(previews) = previews {
                player.set_previews(previews);
            }
            player
        });
        let subscription = cx.subscribe_in(&view, window, |this, _, event, window, cx| {
            this.on_player_event(event, window, cx)
        });
        self.active = Some(Active {
            view,
            source,
            position: start,
            duration: 0.0,
            show: None,
            tuning_scope: None,
            session,
            playback_info: None,
            pause: None,
            _subscription: subscription,
        });
        self.library
            .update(cx, |library, _| library.playback_started());
        cx.notify();
        true
    }

    fn on_player_event(
        &mut self,
        event: &PlayerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            PlayerEvent::Progress {
                position,
                duration,
                paused,
            } => self.progress(*position, *duration, *paused, cx),
            PlayerEvent::Exit => {
                if window.is_fullscreen() {
                    window.toggle_fullscreen();
                }
                self.finish(cx);
                cx.emit(Event::Closed);
                cx.notify();
            }
            PlayerEvent::QualityChosen(quality) => self.change_quality(*quality, cx),
            PlayerEvent::SoundChosen { on, profile } => {
                self.preferences().sound_chosen(*on, *profile)
            }
            PlayerEvent::AudioChosen(language) => {
                for scope in self.pref_scopes() {
                    if let Err(e) = self.profile.db.save_audio_pref(&scope, language) {
                        eprintln!("[library] saving audio choice: {e}");
                    }
                }
            }
            PlayerEvent::SubtitleChosen(pref) => {
                for scope in self.pref_scopes() {
                    if let Err(e) = self.profile.db.save_subtitle_pref(&scope, pref) {
                        eprintln!("[library] saving subtitle choice: {e}");
                    }
                }
            }
            PlayerEvent::SubtitleTuned(tuning) => {
                if let Some(scope) = self.active.as_ref().and_then(|a| a.tuning_scope.as_ref())
                    && let Err(e) = self.profile.db.save_subtitle_tuning(scope, *tuning)
                {
                    eprintln!("[library] saving subtitle tuning: {e}");
                }
            }
            PlayerEvent::Play(key) => self.play_next(key, window, cx),
        }
    }

    /// The player's position, every few seconds: kept for a local file,
    /// passed on to the server for a Jellyfin item.
    fn progress(&mut self, position: f64, duration: f64, paused: bool, cx: &mut Context<Self>) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        active.position = position;
        active.duration = duration;
        match active.source.clone() {
            Some(Source::Local { id, .. }) => {
                if let Err(e) = self.profile.db.save_progress(id, position, duration) {
                    eprintln!("[library] saving progress: {e}");
                }
            }
            Some(Source::Jellyfin { .. }) => self.playback_progress(position, paused, cx),
            None => {}
        }
    }

    /// Straight to another video (the next episode), from the resume point
    /// saved here; fullscreen stays as it is unless it couldn't be opened.
    fn play_next(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.finish(cx);
        self.play_saved(key, StartAt::ResumePoint, window, cx);
        if self.active.is_none() {
            if window.is_fullscreen() {
                window.toggle_fullscreen();
            }
            cx.emit(Event::Closed);
        }
        cx.notify();
    }

    /// Where a track choice made now is remembered.
    fn pref_scopes(&self) -> Vec<String> {
        let show = self.active.as_ref().and_then(|a| a.show.as_deref());
        rules::pref_scopes(show)
    }

    /// Ends the open video: saves the volume, records progress, tells the
    /// server, and has the library read again so watched state shows up.
    fn finish(&mut self, cx: &mut Context<Self>) {
        // Paused (perhaps for long, perhaps while watching on the TV): the
        // paused position mustn't overwrite what happened elsewhere.
        let paused = self.finish_paused(cx);
        // Dropping the view shuts mpv down and destroys its window.
        let Some(active) = self.active.take() else {
            return;
        };
        if let Some(media) = &self.media {
            media.hide();
        }
        // Remember the volume for next time, but never start silent.
        let volume = active.view.read(cx).volume();
        self.preferences().remember_volume(volume);
        let player = active.view.read(cx);
        let said = End {
            position: player.position(),
            duration: player.duration(),
            finished: player.finished(),
        };
        let end = End::freshest(said, active.position, active.duration);
        let record = Record::of(active.source.as_ref(), paused, end);
        if let Record::Local {
            id,
            position,
            duration,
        } = record
            && let Err(e) = self.profile.db.save_progress(id, position, duration)
        {
            eprintln!("[library] saving progress: {e}");
        }
        if let (Some(stop), Some(source)) = (record.stop(), &active.source) {
            let session = active.session.clone();
            self.library.update(cx, |library, cx| {
                library.playback_stopped(source, stop, session, cx)
            });
        }
        self.library
            .update(cx, |library, cx| library.playback_ended(cx));
    }

    /// The window's close button: ends the video. False if the window must
    /// stay until the server has been told; the library closes it then.
    pub(super) fn close_requested(&mut self, cx: &mut Context<Self>) -> bool {
        if self.library.read(cx).is_closing() {
            // Asked again while waiting: don't keep the user waiting.
            return true;
        }
        self.finish(cx);
        self.library
            .update(cx, |library, cx| library.close_when_sent(cx))
    }
}
