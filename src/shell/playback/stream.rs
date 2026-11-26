//! The stream of a Jellyfin video: its address at a quality, switching
//! quality while it plays, and what the server and the intro databases
//! know beyond the file (subtitle files, intro and credits markers).

use gpui::Context;

use super::NowPlaying;
use crate::introdb;
use crate::jellyfin::{Client, PlaySession, PlaybackReport, Quality};
use crate::library::{AfterReport, Report, Source};
use crate::player::SegmentSource;

/// Where to stream an item from at `quality`.
pub(super) fn url(
    client: &Client,
    item_id: &str,
    session: &PlaySession,
    quality: Quality,
) -> String {
    match quality {
        Quality::Original => client.stream_url(item_id),
        Quality::Bitrate(bitrate) => client.transcode_url(item_id, session, bitrate),
    }
}

impl NowPlaying {
    /// Switches the playing Jellyfin video to another quality, keeping the
    /// position: a new play session replaces the old one on the server.
    pub(super) fn change_quality(&mut self, quality: Quality, cx: &mut Context<Self>) {
        let Some(active) = &self.active else {
            return;
        };
        let Some(Source::Jellyfin { server, id }) = active.source.clone() else {
            return;
        };
        self.preferences().set_quality(server, quality);
        let Some(client) = self.client(server, cx) else {
            return;
        };
        let Some(active) = &mut self.active else {
            return;
        };
        let old = active.session.clone();
        let session = PlaySession {
            id: PlaySession::new_id(),
            transcoding: quality.transcodes(),
        };
        let url = url(&client, &id, &session, quality);
        let position = active.view.read(cx).position();
        let subtitles = active
            .playback_info
            .as_ref()
            .map(|i| i.subtitle_files(client.base(), &id, session.transcoding))
            .unwrap_or_default();
        active.view.update(cx, |player, cx| {
            player.reload(&url, position, subtitles, quality, cx)
        });
        active.session = Some(session.clone());
        eprintln!("[jellyfin] quality {} ({})", quality.label(), session.id);
        // The old session stops (ending its transcode) and the new one starts.
        let source = Source::Jellyfin { server, id };
        let paused = active.pause.is_some();
        if let Some(old) = old {
            let stopped = Report {
                kind: PlaybackReport::Stopped,
                position,
                paused,
                session: old,
            };
            self.report(&source, stopped, AfterReport::Nothing, cx);
        }
        let started = Report {
            kind: PlaybackReport::Started,
            position,
            paused,
            session,
        };
        self.report(&source, started, AfterReport::Nothing, cx);
        self.restart_pause(cx);
    }

    /// Asks the server for what it knows beyond the file: subtitle files
    /// beside the video, and intro and credits markers, for the player.
    pub(super) fn load_server_extras(
        &mut self,
        client: Client,
        server: i64,
        item_id: String,
        cx: &mut Context<Self>,
    ) {
        let Some(user_id) = self.user_id(server, cx) else {
            return;
        };
        let source = Source::Jellyfin {
            server,
            id: item_id.clone(),
        };
        let base = client.base().to_string();
        let (client_for_lookup, user_id_for_lookup, source_for_lookup) =
            (client.clone(), user_id.clone(), source.clone());
        cx.spawn(async move |this, cx| {
            let fetch_id = item_id.clone();
            let (info, segments) = cx
                .background_executor()
                .spawn(async move {
                    (
                        client.playback_info(&user_id, &fetch_id),
                        client.media_segments(&fetch_id),
                    )
                })
                .await;
            let info = info
                .map_err(|e| eprintln!("[jellyfin] listing subtitles: {e:#}"))
                .ok();
            // Older servers have no segments API; that's fine.
            let segments = segments.unwrap_or_default();
            this.update(cx, |this, cx| {
                // Only for the video they were fetched for.
                if let Some(active) = &mut this.active
                    && active.source.as_ref() == Some(&source)
                {
                    let transcoding = active.session.as_ref().is_some_and(|s| s.transcoding);
                    let subtitles = info
                        .as_ref()
                        .map(|i| i.subtitle_files(&base, &item_id, transcoding))
                        .unwrap_or_default();
                    eprintln!(
                        "[jellyfin] {} subtitle file(s), {} segment(s)",
                        subtitles.len(),
                        segments.len()
                    );
                    active.playback_info = info;
                    active.view.update(cx, |player, _| {
                        player.add_external_subtitles(subtitles);
                        player.add_segments(SegmentSource::Server, segments);
                    });
                }
            })
            .ok();
        })
        .detach();
        self.load_online_segments(client_for_lookup, user_id_for_lookup, source_for_lookup, cx);
    }

    /// Intro, recap and credits times from the intro databases, for the
    /// kinds the server itself has no markers for.
    fn load_online_segments(
        &mut self,
        client: Client,
        user_id: String,
        source: Source,
        cx: &mut Context<Self>,
    ) {
        let Source::Jellyfin { id, .. } = &source else {
            return;
        };
        let id = id.clone();
        cx.spawn(async move |this, cx| {
            let segments = cx
                .background_executor()
                .spawn(async move {
                    let lookup = client.intro_lookup(&user_id, &id)?;
                    let segments = introdb::fetch(&lookup);
                    eprintln!("[segments] {} from the intro databases", segments.len());
                    anyhow::Ok(segments)
                })
                .await;
            let segments = match segments {
                Ok(s) => s,
                Err(e) => return eprintln!("[segments] looking up ids: {e:#}"),
            };
            this.update(cx, |this, cx| {
                if let Some(active) = &mut this.active
                    && active.source.as_ref() == Some(&source)
                {
                    active.view.update(cx, |player, _| {
                        player.add_segments(SegmentSource::Online, segments);
                    });
                }
            })
            .ok();
        })
        .detach();
    }
}
