//! Media preview for the binary files the editor cannot read as text: images
//! show in the pane, video and audio play in it with one play/pause button.
//! Rune decodes PNG and JPEG and plays what the platform player does
//! (mp4/mov, mp3/m4a/wav/flac/ogg); every other media type opens in the
//! system's default app instead.

use std::path::Path;

use rune::{El, Image, ImageCache, Player, PlayerState, SurfaceFit, Waker, col, el, row};

use crate::State;
use crate::theme::{self, Ink, T12};

/// The image cache and the app waker a preview needs, set in `setup`.
pub type Host = (ImageCache, Waker);

pub enum Media {
    Image(Image),
    /// A video (`true`, drawn in the pane) or an audio clip.
    Player(Player, bool),
}

fn ext(path: &str) -> String {
    path.rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Whether the pane previews `path` itself.
pub fn previews(path: &str) -> bool {
    matches!(
        ext(path).as_str(),
        "png" | "jpg" | "jpeg" | "mp4" | "mov" | "m4v" | "mp3" | "m4a" | "wav" | "flac" | "ogg"
    )
}

/// Whether `path` is media the pane cannot preview: it opens in the system's
/// default app.
// ponytail: webp/gif/pdf/webm… go to the system app; add a decoder or a
// viewer here if an in-app preview of them matters.
pub fn opens_externally(path: &str) -> bool {
    !previews(path)
        && matches!(
            ext(path).as_str(),
            "gif"
                | "webp"
                | "bmp"
                | "ico"
                | "tif"
                | "tiff"
                | "heic"
                | "avif"
                | "webm"
                | "mkv"
                | "avi"
                | "aac"
                | "pdf"
        )
}

impl Media {
    /// The preview for the file at `file`, when it is one the pane shows.
    pub fn open(path: &str, file: &Path, host: &Host) -> Option<Media> {
        if !previews(path) {
            return None;
        }
        let kind = ext(path);
        Some(match kind.as_str() {
            "png" | "jpg" | "jpeg" => Media::Image(host.0.file(file, 64.0, 2.0)),
            "mp4" | "mov" | "m4v" => {
                let player = Player::file(file);
                player.set_muted(false);
                player.set_waker(host.1.clone());
                Media::Player(player, true)
            }
            _ => {
                let player = Player::audio(file.to_path_buf());
                player.set_waker(host.1.clone());
                Media::Player(player, false)
            }
        })
    }
}

fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// The pane's content: the image, or the player with its play/pause bar.
pub fn view(media: &Media, ink: &Ink) -> El {
    match media {
        Media::Image(image) => {
            if let Some(error) = image.error() {
                return note(&format!("cannot show this image: {error}"), ink);
            }
            el().grow(1.0)
                .min_h(0.0)
                .w_full()
                .pixels_fit(image.pixels(), SurfaceFit::Fit)
        }
        Media::Player(player, video) => {
            let state = player.state();
            if let PlayerState::Failed(reason) = &state {
                return note(&format!("cannot play this file: {reason}"), ink);
            }
            let playing = player.is_playing();
            let toggle = player.clone();
            let label = format!(
                "{} / {}",
                clock(player.time()),
                player
                    .duration()
                    .map(clock)
                    .unwrap_or_else(|| "–:––".into())
            );
            let bar = row()
                .w_full()
                .h(40.0)
                .px(10.0)
                .gap(10.0)
                .items_center()
                .border_sides([1.0, 0.0, 0.0, 0.0], ink.border)
                .child(
                    theme::btn_sm(if playing { "Pause" } else { "Play" }, None, true, ink)
                        .on_press(move |_: &mut State| {
                            if toggle.is_playing() {
                                toggle.pause();
                            } else {
                                toggle.play();
                            }
                        }),
                )
                .child(theme::txt(&label, T12, ink.muted));
            let surface = if *video {
                el().grow(1.0)
                    .min_h(0.0)
                    .w_full()
                    .video(player, SurfaceFit::Fit)
            } else {
                el().grow(1.0).min_h(0.0).w_full()
            };
            col()
                .grow(1.0)
                .min_h(0.0)
                .w_full()
                .child(surface)
                .child(bar)
        }
    }
}

fn note(message: &str, ink: &Ink) -> El {
    el().grow(1.0)
        .w_full()
        .items_center()
        .justify_center()
        .child(theme::txt(message, T12, ink.muted))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_media_type_has_exactly_one_home() {
        for path in ["a.png", "b/c.JPG", "v.mp4", "m.mov", "s.mp3", "s.flac"] {
            assert!(previews(path) && !opens_externally(path), "{path}");
        }
        for path in ["a.gif", "d.pdf", "v.webm", "x.WEBP", "s.aac"] {
            assert!(!previews(path) && opens_externally(path), "{path}");
        }
        for path in ["a.rs", "icon.svg", "README", "dir.png/readme.md", "mp4"] {
            assert!(!previews(path) && !opens_externally(path), "{path}");
        }
    }

    #[test]
    fn the_clock_reads_minutes_and_seconds() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(75.9), "1:15");
    }
}
