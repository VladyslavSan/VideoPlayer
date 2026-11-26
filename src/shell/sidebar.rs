//! The sidebar beside the library's pages: Home, Downloads, then each
//! source with its libraries and its Sonarr and Radarr. A click picks the
//! tab; a show's or a movie's page counts as the tab it was opened from.

use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, SharedString, div, prelude::*, px, rgb, rgba,
};

use super::browse::{self, Grid, SIDEBAR_WIDTH};
use super::header::HEADER_HEIGHT;
use super::{ACCENT, MUTED, Screen, Shell, TEXT, arr};
use crate::arr::connections::Id;

/// The background of the tab that's open, and of one under the pointer.
const SELECTED: u32 = 0xffffff1a;
const HOVERED: u32 = 0xffffff0d;

/// Where a line stands in the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    /// Home, or a source: bold.
    Heading,
    /// A library of the source above it: indented.
    Library,
}

/// A line of the sidebar that opens a tab.
struct Line {
    id: SharedString,
    label: String,
    screen: Screen,
    level: Level,
}

impl Shell {
    /// Shows `screen` as the tab to come back to.
    pub(super) fn set_tab(&mut self, screen: Screen, cx: &mut Context<Self>) {
        match screen {
            Screen::Downloads => {
                let downloads = self.downloads.clone();
                downloads.update(cx, |downloads, cx| downloads.refresh(cx));
            }
            Screen::Arr(server, kind) => {
                let app = Id { server, kind };
                self.arr.update(cx, |arr, cx| arr.opened(app, cx));
            }
            _ => {}
        }
        self.navigation.open_tab(screen);
        cx.notify();
    }

    /// A line that opens a tab, lit when that tab is open.
    fn tab_line(&self, line: Line, cx: &mut Context<Self>) -> AnyElement {
        let Line {
            id,
            label,
            screen,
            level,
        } = line;
        let active = *self.navigation.tab() == screen;
        let heading = level == Level::Heading;
        div()
            .id(id)
            .flex_none()
            .px_3()
            .py_1p5()
            .when(level == Level::Library, |d| d.pl_6())
            .rounded_md()
            .truncate()
            .cursor_pointer()
            .when(heading, |d| d.font_weight(FontWeight::SEMIBOLD))
            .text_color(rgb(if active || heading { TEXT } else { MUTED }))
            .when(active, |d| d.bg(rgba(SELECTED)))
            .when(!active, |d| d.hover(|s| s.bg(rgba(HOVERED))))
            .child(label)
            .on_click(
                cx.listener(move |this, _: &ClickEvent, _, cx| this.set_tab(screen.clone(), cx)),
            )
            .into_any_element()
    }

    /// The Downloads line, with how many downloads are under way.
    fn downloads_line(&self, cx: &mut Context<Self>) -> AnyElement {
        let active = self.downloads.read(cx).active();
        let selected = *self.navigation.tab() == Screen::Downloads;
        div()
            .id("side-downloads")
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .px_3()
            .py_1p5()
            .rounded_md()
            .cursor_pointer()
            .font_weight(FontWeight::SEMIBOLD)
            .when(selected, |d| d.bg(rgba(SELECTED)))
            .when(!selected, |d| d.hover(|s| s.bg(rgba(HOVERED))))
            .child(div().flex_1().min_w(px(0.0)).truncate().child("Downloads"))
            .when(active > 0, |d| {
                d.child(
                    div()
                        .flex_none()
                        .px_2()
                        .rounded_full()
                        .text_xs()
                        .bg(rgb(ACCENT))
                        .child(active.to_string()),
                )
            })
            .on_click(
                cx.listener(move |this, _: &ClickEvent, _, cx| this.set_tab(Screen::Downloads, cx)),
            )
            .into_any_element()
    }

    /// A server's Sonarr or Radarr in the sidebar.
    fn arr_line(&self, app: Id, cx: &mut Context<Self>) -> AnyElement {
        let screen = Screen::Arr(app.server, app.kind);
        let entry = arr::sidebar::Entry {
            app,
            selected: *self.navigation.tab() == screen,
            active: self.downloads.read(cx).active_for(app),
        };
        arr::sidebar::entry(entry, cx, move |this, cx| this.set_tab(screen.clone(), cx))
    }

    /// A source's lines: its own page, its libraries, its Sonarr and Radarr.
    fn source_lines(&self, source: browse::SourceEntry, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let key = source.origin.map_or("local".to_string(), |s| s.to_string());
        let mut lines = vec![div().h(px(14.0)).flex_none().into_any_element()];
        let heading = Line {
            id: format!("side-source-{key}").into(),
            label: source.name.clone(),
            screen: Screen::Source(source.origin),
            level: Level::Heading,
        };
        lines.push(self.tab_line(heading, cx));
        for (label, screen) in source.libraries {
            let id = match Grid::of(&screen) {
                Some(grid) => format!("side-{}", grid.id()),
                None => format!("side-{key}-{label}"),
            };
            let line = Line {
                id: id.into(),
                label,
                screen,
                level: Level::Library,
            };
            lines.push(self.tab_line(line, cx));
        }
        if let Some(server) = source.origin {
            let apps = self.apps.connected_ids().into_iter();
            for app in apps.filter(|app| app.server == server) {
                lines.push(self.arr_line(app, cx));
            }
        }
        lines
    }

    /// The left sidebar: Home, then each source with its libraries.
    pub(super) fn sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let home = Line {
            id: "side-home".into(),
            label: "Home".into(),
            screen: Screen::Home,
            level: Level::Heading,
        };
        let mut lines = vec![self.tab_line(home, cx)];
        if !self.apps.connected_ids().is_empty() {
            lines.push(self.downloads_line(cx));
        }
        let catalog = self.library.read(cx).catalog();
        for source in browse::sources(&catalog) {
            lines.extend(self.source_lines(source, cx));
        }
        let mut sidebar = div()
            .id("sidebar")
            .flex()
            .flex_col()
            .flex_none()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .gap_0p5()
            .pl_6()
            .pr_2()
            .pt(px(HEADER_HEIGHT + 8.0))
            .pb_6()
            .overflow_y_scroll()
            .children(lines);
        sidebar.style().restrict_scroll_to_axis = Some(true);
        sidebar.into_any_element()
    }
}
