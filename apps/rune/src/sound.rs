//! The done sound: one chime when a session's work finishes, played only
//! while the app window is unfocused. macOS uses the system "Blow" sound
//! (`Breeze.aiff` is the same sound under Apple's newer name); other
//! platforms get a synthesized tone through the first available player.

/// The minimum gap between chimes: finishes landing in the same poll
/// still make one sound.
const DEBOUNCE_MS: i64 = 1000;

impl crate::State {
    /// Chime once when a session finishes: the window unfocused, the
    /// `goko.sound_done` setting on, and the debounce elapsed.
    pub(crate) fn note_done_sound(&mut self) {
        if self.window_focused || !self.sound_done {
            return;
        }
        if self.now_ms - self.sound_last_ms < DEBOUNCE_MS {
            return;
        }
        self.sound_last_ms = self.now_ms;
        play();
    }
}

/// Play the done sound. Tests count plays instead of spawning players.
pub(crate) fn play() {
    imp::play();
}

/// A soft bell — C6 with two quieter partials, ~8 ms attack, 0.5 s decay —
/// as 16-bit mono PCM at 22050 Hz, wrapped in a WAV header.
fn done_wav() -> Vec<u8> {
    const SR: usize = 22050;
    const F0: f64 = 1046.5;
    let n = SR / 2;
    let mut samples = Vec::with_capacity(n);
    let mut peak = 0.0f64;
    for i in 0..n {
        let t = i as f64 / SR as f64;
        let env = (-7.0 * t).exp() * (t / 0.008).min(1.0);
        let s = ((2.0 * std::f64::consts::PI * F0 * t).sin()
            + 0.35 * (2.0 * std::f64::consts::PI * 2.0 * F0 * t).sin()
            + 0.12 * (2.0 * std::f64::consts::PI * 3.0 * F0 * t).sin())
            * env;
        peak = peak.max(s.abs());
        samples.push(s);
    }
    let gain = 0.7 / peak;
    let data_len = (n * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&(SR as u32).to_le_bytes());
    out.extend_from_slice(&(SR as u32 * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&((s * gain * 32767.0).round() as i16).to_le_bytes());
    }
    out
}

#[cfg(not(test))]
mod imp {
    use super::done_wav;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    /// macOS system sounds tried in order: Blow, then Breeze — Apple's
    /// rename of the same sound on newer releases.
    const SYSTEM_SOUNDS: [&str; 2] = ["Blow.aiff", "Breeze.aiff"];
    /// Fallback players for the synthesized tone, in preference order;
    /// each entry is the program plus the flags that make it exit after
    /// playback. `afplay` plays WAV too — it is the last resort on macOS,
    /// where the other players do not exist.
    const PLAYERS: [(&str, &[&str]); 5] = [
        ("paplay", &[]),
        ("aplay", &[]),
        ("ffplay", &["-nodisp", "-autoexit", "-loglevel", "quiet"]),
        ("mpv", &["--no-video", "--really-quiet", "--idle=no"]),
        ("afplay", &[]),
    ];

    pub fn play() {
        if !system_sound() {
            synth_sound();
        }
    }

    /// `afplay` the first system sound that exists; `false` when `afplay`
    /// or every candidate is missing.
    fn system_sound() -> bool {
        if !on_path("afplay") {
            return false;
        }
        SYSTEM_SOUNDS.iter().any(|name| {
            let path = format!("/System/Library/Sounds/{name}");
            Path::new(&path).is_file() && spawn("afplay", &[path.as_str()])
        })
    }

    /// Write the synthesized tone once and hand it to the first player
    /// that starts. No player at all means silence, not an error.
    fn synth_sound() {
        let path = temp_wav();
        let file = path.to_string_lossy().to_string();
        for (player, flags) in PLAYERS {
            let mut args: Vec<&str> = flags.to_vec();
            args.push(file.as_str());
            if spawn(player, &args) {
                return;
            }
        }
    }

    fn temp_wav() -> PathBuf {
        // Per-process name: two Goko instances chiming at once never share
        // a file, and the debounce (1 s) outlasts the tone (0.5 s), so a
        // chime never overwrites a file its own player still reads.
        let path = std::env::temp_dir().join(format!("goko-done-{}.wav", std::process::id()));
        let _ = std::fs::write(&path, done_wav());
        path
    }

    /// Start `program` detached; `false` when it cannot be spawned. The
    /// child is reaped on a thread so chimes leave no zombies.
    fn spawn(program: &str, args: &[&str]) -> bool {
        let Ok(mut child) = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            return false;
        };
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        true
    }

    fn on_path(program: &str) -> bool {
        let Some(paths) = std::env::var_os("PATH") else {
            return false;
        };
        std::env::split_paths(&paths).any(|dir| {
            let candidate = dir.join(program);
            candidate.is_file() && executable(&candidate)
        })
    }

    #[cfg(unix)]
    fn executable(path: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0)
    }

    #[cfg(not(unix))]
    fn executable(path: &Path) -> bool {
        path.is_file()
    }
}

#[cfg(test)]
pub(crate) fn test_plays() -> usize {
    imp::plays()
}

#[cfg(test)]
mod imp {
    use std::cell::Cell;

    // Thread-local: libtest runs each test on its own thread, so parallel
    // tests never share the count.
    thread_local! {
        static PLAYS: Cell<usize> = const { Cell::new(0) };
    }

    pub fn play() {
        PLAYS.with(|plays| plays.set(plays.get() + 1));
    }

    pub fn plays() -> usize {
        PLAYS.with(Cell::get)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn done_wav_is_a_valid_pcm_wav() {
        let wav = done_wav();
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(u32::from_le_bytes(wav[16..20].try_into().unwrap()), 16);
        assert_eq!(u16::from_le_bytes(wav[20..22].try_into().unwrap()), 1);
        assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 22050);
        assert_eq!(u32::from_le_bytes(wav[28..32].try_into().unwrap()), 44100);
        assert_eq!(u16::from_le_bytes(wav[32..34].try_into().unwrap()), 2);
        assert_eq!(u16::from_le_bytes(wav[34..36].try_into().unwrap()), 16);
        assert_eq!(&wav[36..40], b"data");
        let data_len = u32::from_le_bytes(wav[40..44].try_into().unwrap()) as usize;
        assert_eq!(wav.len(), 44 + data_len);
        assert!(data_len > 0);
    }
}
