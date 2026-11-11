//! Theme: the Goko dark palette (`tokens.css` `:root` + `index.css` `@theme`
//! name→token mappings), the Tailwind type scale the components use, the lucide
//! + agent-logo icon bodies, and the small builders shared by the
//!   chrome regions. Ported from the verified rune-spike `fidelity.rs`.

use rune::{Color, El, Icon, Live, Spring, TextStyle, el, icon, row, text};

pub fn c(hex: &str) -> Color {
    rune::color::hex(hex).expect("palette hex")
}

/// Palette — Goko default dark theme.
#[derive(Clone, Copy)]
pub struct Ink {
    /// Whether the resolved palette is dark — mode-sensitive surfaces
    /// (syntax tokens, subtle alphas) follow it rather than the theme id.
    pub dark: bool,
    pub bg: Color,      // --bg #1e1e1e
    pub panel: Color,   // --panel #252526 (also --secondary)
    pub text: Color,    // --text #cccccc (also --foreground)
    pub muted: Color,   // --muted #9d9d9d
    pub accent: Color,  // Tailwind `accent` = --accent-text #3794ff
    pub primary: Color, // Tailwind `primary` = --accent #007acc
    pub border: Color,  // --border #303031
    pub success: Color, // #6dd3a4
    pub warning: Color, // #e8b86a
    pub danger: Color,  // #e87575
    pub claude: Color,  // agent mark orange, hardcoded #D97757 in AgentIcon.tsx
    pub white: Color,   // --primary-foreground #ffffff
    pub dim: Color,     // fallback for rare ANSI named colours
}

/// The appearance choice in Settings — the desktop's `Theme` (`goko.theme`).
/// `System` resolves through the OS appearance at render time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Theme {
    System,
    Dark,
    Light,
    OneDark,
    CatppuccinMocha,
    Dracula,
}

impl Theme {
    /// `use-theme.ts` `THEME_OPTIONS`, in the Settings picker's order.
    pub const OPTIONS: [Theme; 6] = [
        Theme::System,
        Theme::Dark,
        Theme::Light,
        Theme::OneDark,
        Theme::CatppuccinMocha,
        Theme::Dracula,
    ];

    /// The option's label in the picker ("Dark+" is the dark default).
    pub fn label(self) -> &'static str {
        match self {
            Theme::System => "System",
            Theme::Dark => "Dark+",
            Theme::Light => "Light",
            Theme::OneDark => "One Dark",
            Theme::CatppuccinMocha => "Catppuccin Mocha",
            Theme::Dracula => "Dracula",
        }
    }

    /// The value stored under `goko.theme`.
    pub fn id(self) -> &'static str {
        match self {
            Theme::System => "system",
            Theme::Dark => "dark",
            Theme::Light => "light",
            Theme::OneDark => "one-dark",
            Theme::CatppuccinMocha => "catppuccin-mocha",
            Theme::Dracula => "dracula",
        }
    }

    /// `storedTheme()`: a stored value, or `dark` when missing/unknown.
    pub fn parse(value: &str) -> Theme {
        Theme::OPTIONS
            .iter()
            .copied()
            .find(|theme| theme.id() == value)
            .unwrap_or(Theme::Dark)
    }

    /// `resolveTheme()`: `system` becomes the OS appearance; anything else
    /// is already concrete.
    pub fn resolve(self, os_dark: bool) -> Theme {
        if self == Theme::System {
            if os_dark { Theme::Dark } else { Theme::Light }
        } else {
            self
        }
    }

    /// Whether the resolved palette is a dark one — the kit palettes and
    /// light-on-dark details (submit ink, halo alphas) follow it.
    pub fn dark(self) -> bool {
        !matches!(self, Theme::Light)
    }
}

/// The settings key the theme choice is stored under (`goko.theme`, the
/// same name the desktop persists in local storage).
pub const THEME_KEY: &str = "goko.theme";

impl Ink {
    /// The Dark+ palette — the shell's home theme.
    pub fn new() -> Self {
        Ink::for_theme(Theme::Dark)
    }

    /// The palette for a resolved theme — `tokens.css` mapped onto the
    /// fields the app reads (`accent` is `--accent-text`, `primary` is
    /// `--accent`, `white` is `--primary-foreground`). `claude` keeps its
    /// hardcoded brand orange and `dim` the ANSI fallback on every theme.
    pub fn for_theme(theme: Theme) -> Self {
        let (bg, panel, text, muted, accent, primary, border, success, warning, danger, fg) =
            match theme {
                Theme::Light => (
                    "#f5f7fa", "#ffffff", "#1a2430", "#6c7a89", "#116c6b", "#116c6b", "#dce2e8",
                    "#29784a", "#976a13", "#b84245", "#f5f7fa",
                ),
                Theme::OneDark => (
                    "#282c34", "#21252b", "#abb2bf", "#7f848e", "#61afef", "#61afef", "#3e4451",
                    "#98c379", "#e5c07b", "#e06c75", "#ffffff",
                ),
                Theme::CatppuccinMocha => (
                    "#1e1e2e", "#181825", "#cdd6f4", "#a6adc8", "#89b4fa", "#cba6f7", "#45475a",
                    "#a6e3a1", "#f9e2af", "#f38ba8", "#11111b",
                ),
                Theme::Dracula => (
                    "#282a36", "#21222c", "#f8f8f2", "#6272a4", "#8be9fd", "#bd93f9", "#44475a",
                    "#50fa7b", "#f1fa8c", "#ff5555", "#282a36",
                ),
                // Dark+ (and System before the OS answer is in).
                _ => (
                    "#1e1e1e", "#252526", "#cccccc", "#9d9d9d", "#3794ff", "#007acc", "#303031",
                    "#6dd3a4", "#e8b86a", "#e87575", "#ffffff",
                ),
            };
        Ink {
            dark: theme.dark(),
            bg: c(bg),
            panel: c(panel),
            text: c(text),
            muted: c(muted),
            accent: c(accent),
            primary: c(primary),
            border: c(border),
            success: c(success),
            warning: c(warning),
            danger: c(danger),
            claude: c("#D97757"),
            white: c(fg),
            dim: c("#9d9d9d"),
        }
    }
}

// ---------------------------------------------------------------------------
// Type scale — Tailwind sizes used by the real components, in points.
// `tracking` mirrors tracking-tight (-0.025em) / tracking-wider (0.05em).
// ---------------------------------------------------------------------------

pub const T11: TextStyle = TextStyle::new(11.0, 400.0);
pub const T11M: TextStyle = TextStyle::new(11.0, 500.0);
/// Chip and pill labels on a 16 pt line box, so text sits on whole device pixels
/// inside a 20 pt badge (the sidebar's compact scale does the same).
pub const T11M_LH: TextStyle = T11M.lh(16.0);
pub const T11MN: TextStyle = TextStyle::new(11.0, 500.0).features("'tnum' 1");
pub const T11MN_BADGE: TextStyle = T11MN.lh(16.0);
pub const T11ST: TextStyle = TextStyle::new(11.0, 600.0).tracking(0.55); // Projects/Unassigned headers
pub const T12: TextStyle = TextStyle::new(12.0, 400.0);
pub const T12TN: TextStyle = TextStyle::new(12.0, 400.0).features("'tnum' 1"); // text-xs tabular-nums
pub const T12M: TextStyle = TextStyle::new(12.0, 500.0);
pub const T128: TextStyle = TextStyle::new(12.8, 500.0); // button sm text-[0.8rem]
pub const T14: TextStyle = TextStyle::new(14.0, 400.0);
pub const T14M: TextStyle = TextStyle::new(14.0, 500.0);
pub const T14S: TextStyle = TextStyle::new(14.0, 600.0).tracking(-0.35); // "goko" tracking-tight

// ---------------------------------------------------------------------------
// Motion — one colour spring for state changes (active, selected, open) shared
// by the sidebar rows and the header controls, so both read as one app.
// ---------------------------------------------------------------------------

/// Critically damped, ~0.11 s to 95 %. The default colour spring takes ~0.22 s,
/// which trails the instant press wash and makes a state change read as a second,
/// late event.
pub const STATE_SPRING: Spring = Spring::new(0.15, 0.0);

// ---------------------------------------------------------------------------
// Icons — lucide bodies (viewBox 24) via @lobehub path data + agent logos from
// agent-logos.ts (fill, evenodd).
// ---------------------------------------------------------------------------

macro_rules! stroke_icon {
    ($name:ident, $icon:literal, $($path:literal),+ $(,)?) => {
        pub static $name: Icon = Icon {
            name: $icon,
            view: 24.0,
            body: concat!(
                "<g fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\">",
                $($path),+,
                "</g>"
            ),
        };
    };
}

stroke_icon!(CHEVRON_DOWN, "chevron-down", "<path d=\"m6 9 6 6 6-6\"/>");
stroke_icon!(
    CHEVRON_RIGHT,
    "chevron-right",
    "<path d=\"m9 18 6-6-6-6\"/>"
);
stroke_icon!(
    X,
    "x",
    "<path d=\"M18 6 6 18\"/>",
    "<path d=\"m6 6 12 12\"/>"
);
stroke_icon!(
    PLUS,
    "plus",
    "<path d=\"M5 12h14\"/>",
    "<path d=\"M12 5v14\"/>"
);
stroke_icon!(
    SETTINGS,
    "settings",
    "<path d=\"M9.671 4.136a2.34 2.34 0 0 1 4.659 0 2.34 2.34 0 0 0 3.319 1.915 2.34 2.34 0 0 1 2.33 4.033 2.34 2.34 0 0 0 0 3.831 2.34 2.34 0 0 1-2.33 4.033 2.34 2.34 0 0 0-3.319 1.915 2.34 2.34 0 0 1-4.659 0 2.34 2.34 0 0 0-3.32-1.915 2.34 2.34 0 0 1-2.33-4.033 2.34 2.34 0 0 0 0-3.831A2.34 2.34 0 0 1 6.35 6.051a2.34 2.34 0 0 0 3.319-1.915\"/>",
    "<circle cx=\"12\" cy=\"12\" r=\"3\"/>"
);
stroke_icon!(
    FOLDER_GIT2,
    "folder-git-2",
    "<path d=\"M18 19a5 5 0 0 1-5-5v8\"/>",
    "<path d=\"M9 20H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2a2 2 0 0 0 1.67.9H20a2 2 0 0 1 2 2v5\"/>",
    "<circle cx=\"13\" cy=\"12\" r=\"2\"/>",
    "<circle cx=\"20\" cy=\"19\" r=\"2\"/>"
);
stroke_icon!(
    FOLDER,
    "folder",
    "<path d=\"M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z\"/>"
);
stroke_icon!(
    FOLDER_TREE,
    "folder-tree",
    "<path d=\"M20 10a1 1 0 0 0 1-1V6a1 1 0 0 0-1-1h-2.5a1 1 0 0 1-.8-.4l-.9-1.2A1 1 0 0 0 15 3h-2a1 1 0 0 0-1 1v5a1 1 0 0 0 1 1Z\"/>",
    "<path d=\"M20 21a1 1 0 0 0 1-1v-3a1 1 0 0 0-1-1h-2.9a1 1 0 0 1-.88-.55l-.42-.85a1 1 0 0 0-.92-.6H13a1 1 0 0 0-1 1v5a1 1 0 0 0 1 1Z\"/>",
    "<path d=\"M3 5a2 2 0 0 0 2 2h3\"/>",
    "<path d=\"M3 3v13a2 2 0 0 0 2 2h3\"/>"
);
stroke_icon!(
    FOLDER_PLUS,
    "folder-plus",
    "<path d=\"M12 10v6\"/>",
    "<path d=\"M9 13h6\"/>",
    "<path d=\"M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z\"/>"
);
stroke_icon!(
    GIT_BRANCH,
    "git-branch",
    "<path d=\"M15 6a9 9 0 0 0-9 9V3\"/>",
    "<circle cx=\"18\" cy=\"6\" r=\"3\"/>",
    "<circle cx=\"6\" cy=\"18\" r=\"3\"/>"
);
stroke_icon!(
    SQUARE_TERMINAL,
    "square-terminal",
    "<path d=\"m7 11 2-2-2-2\"/>",
    "<path d=\"M11 13h4\"/>",
    "<rect width=\"18\" height=\"18\" x=\"3\" y=\"3\" rx=\"2\" ry=\"2\"/>"
);
// AgentIcon.tsx: shell uses SquareTerminal; custom/unknown agents use Terminal.
stroke_icon!(
    TERMINAL,
    "terminal",
    "<path d=\"M12 19h8\"/>",
    "<path d=\"m4 17 6-6-6-6\"/>"
);
// Split-pane glyphs for the tab strip's icon-only split buttons (lucide
// columns-2 / rows-2: an 18 px frame with one divider).
stroke_icon!(
    COLUMNS_2,
    "columns-2",
    "<rect width=\"18\" height=\"18\" x=\"3\" y=\"3\" rx=\"2\" ry=\"2\"/>",
    "<path d=\"M12 3v18\"/>"
);
stroke_icon!(
    ROWS_2,
    "rows-2",
    "<rect width=\"18\" height=\"18\" x=\"3\" y=\"3\" rx=\"2\" ry=\"2\"/>",
    "<path d=\"M3 12h18\"/>"
);
stroke_icon!(
    BOOK_OPEN,
    "book-open",
    "<path d=\"M2 3h6a4 4 0 0 1 4 4v14a3 3 0 0 0-3-3H2z\"/>",
    "<path d=\"M22 3h-6a4 4 0 0 0-4 4v14a3 3 0 0 1 3-3h7z\"/>"
);
// Badge variant="warning" icon (CheckoutStatus "missing" badge).
stroke_icon!(
    TRIANGLE_ALERT,
    "triangle-alert",
    "<path d=\"m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3\"/>",
    "<path d=\"M12 9v4\"/>",
    "<path d=\"M12 17h.01\"/>"
);
// The spinner a working session shows instead of the plain state dot
// (lucide "loader-circle"): one open arc, turned by the app's clock.
stroke_icon!(
    LOADER,
    "loader-circle",
    "<path d=\"M21 12a9 9 0 1 1-6.219-8.56\"/>"
);
// A row's "more actions" button (lucide "ellipsis").
stroke_icon!(
    MORE,
    "ellipsis",
    "<circle cx=\"12\" cy=\"12\" r=\"1\"/>",
    "<circle cx=\"19\" cy=\"12\" r=\"1\"/>",
    "<circle cx=\"5\" cy=\"12\" r=\"1\"/>"
);
// The stop button of a session row (lucide "square").
stroke_icon!(
    SQUARE,
    "square",
    "<rect width=\"18\" height=\"18\" x=\"3\" y=\"3\" rx=\"2\"/>"
);

// Agent marks — fill icons from agent-logos.ts, tinted via .color() like
// AgentIcon.tsx does (claude keeps its brand orange, the rest currentColor).
pub static CLAUDE_MARK: Icon = Icon {
    name: "claude",
    view: 24.0,
    body: "<g fill=\"currentColor\" fill-rule=\"evenodd\"><path d=\"M20.998 10.949H24v3.102h-3v3.028h-1.487V20H18v-2.921h-1.487V20H15v-2.921H9V20H7.488v-2.921H6V20H4.487v-2.921H3V14.05H0V10.95h3V5h17.998v5.949zM6 10.949h1.488V8.102H6v2.847zm10.51 0H18V8.102h-1.49v2.847z\"/></g>",
};
pub static CODEX_MARK: Icon = Icon {
    name: "codex",
    view: 24.0,
    body: "<g fill=\"currentColor\" fill-rule=\"evenodd\"><path d=\"M8.086.457a6.105 6.105 0 013.046-.415c1.333.153 2.521.72 3.564 1.7a.117.117 0 00.107.029c1.408-.346 2.762-.224 4.061.366l.063.03.154.076c1.357.703 2.33 1.77 2.918 3.198.278.679.418 1.388.421 2.126a5.655 5.655 0 01-.18 1.631.167.167 0 00.04.155 5.982 5.982 0 011.578 2.891c.385 1.901-.01 3.615-1.183 5.14l-.182.22a6.063 6.063 0 01-2.934 1.851.162.162 0 00-.108.102c-.255.736-.511 1.364-.987 1.992-1.199 1.582-2.962 2.462-4.948 2.451-1.583-.008-2.986-.587-4.21-1.736a.145.145 0 00-.14-.032c-.518.167-1.04.191-1.604.185a5.924 5.924 0 01-2.595-.622 6.058 6.058 0 01-2.146-1.781c-.203-.269-.404-.522-.551-.821a7.74 7.74 0 01-.495-1.283 6.11 6.11 0 01-.017-3.064.166.166 0 00.008-.074.115.115 0 00-.037-.064 5.958 5.958 0 01-1.38-2.202 5.196 5.196 0 01-.333-1.589 6.915 6.915 0 01.188-2.132c.45-1.484 1.309-2.648 2.577-3.493.282-.188.55-.334.802-.438.286-.12.573-.22.861-.304a.129.129 0 00.087-.087A6.016 6.016 0 015.635 2.31C6.315 1.464 7.132.846 8.086.457zm-.804 7.85a.848.848 0 00-1.473.842l1.694 2.965-1.688 2.848a.849.849 0 001.46.864l1.94-3.272a.849.849 0 00.007-.854l-1.94-3.393zm5.446 6.24a.849.849 0 000 1.695h4.848a.849.849 0 000-1.696h-4.848z\"/></g>",
};
pub static OPENCODE_MARK: Icon = Icon {
    name: "opencode",
    view: 24.0,
    body: "<g fill=\"currentColor\" fill-rule=\"evenodd\"><path d=\"M16 6H8v12h8V6zm4 16H4V2h16v20z\"/></g>",
};
pub static DEVIN_MARK: Icon = Icon {
    name: "devin",
    view: 24.0,
    body: "<g fill=\"currentColor\" fill-rule=\"evenodd\"><path d=\"M2.033 9.867l2.554 1.483a.589.589 0 00.592 0l2.554-1.483.01-.008a.608.608 0 00.11-.084l.013-.015a.631.631 0 00.076-.1c.003-.005.008-.01.01-.016a.558.558 0 00.052-.125l.007-.028a.611.611 0 00.019-.14V7.868c0-.572.307-1.105.8-1.392a1.595 1.595 0 011.598 0l1.277.742a.54.54 0 00.129.053l.028.01c.044.01.088.015.133.016h.006l.013-.002a.587.587 0 00.27-.074l.011-.004 2.554-1.483a.596.596 0 00.297-.516V2.253a.595.595 0 00-.297-.516L12.293.257a.587.587 0 00-.591 0L9.148 1.737l-.01.01a.609.609 0 00-.109.083l-.014.015a.632.632 0 00-.076.1c-.003.005-.008.01-.01.016a.57.57 0 00-.052.124l-.007.028a.612.612 0 00-.018.14v1.483c0 .572-.307 1.105-.8 1.393a1.597 1.597 0 01-1.599 0l-1.276-.742a.603.603 0 00-.13-.053l-.028-.008a.658.658 0 00-.133-.018h-.02a.57.57 0 00-.269.074c-.003.002-.008.002-.012.005L2.033 5.872a.596.596 0 00-.297.515v2.966c0 .213.113.41.297.515z\"/><path d=\"M15.943 10.607a1.596 1.596 0 011.599 0l1.276.74c.041.025.085.04.13.055l.028.008c.043.01.088.016.133.018h.005c.005 0 .01-.002.014-.003a.474.474 0 00.122-.016l.021-.005a.616.616 0 00.126-.052c.004-.002.009-.002.013-.005l2.554-1.482a.597.597 0 00.297-.516V6.383a.596.596 0 00-.297-.515l-2.552-1.483a.587.587 0 00-.592 0l-2.553 1.482-.011.008a.61.61 0 00-.108.084l-.014.016a.637.637 0 00-.076.1c-.003.005-.008.01-.01.016a.57.57 0 00-.052.124l-.007.029a.612.612 0 00-.018.14v1.482c0 .572-.307 1.105-.8 1.393a1.597 1.597 0 01-1.599 0l-1.276-.742a.584.584 0 00-.13-.053l-.028-.008a.62.62 0 00-.133-.018h-.02a.587.587 0 00-.269.074l-.012.004L9.15 10a.596.596 0 00-.296.516v2.966c0 .212.112.409.296.515l2.554 1.483s.008.002.012.005c.04.022.082.04.126.052l.02.004a.57.57 0 00.123.017l.014.002h.006c.054 0 .108-.01.16-.025a.587.587 0 00.13-.054l1.277-.741a1.597 1.597 0 012.398 1.392v1.482c0 .049.007.095.019.14l.007.028a.619.619 0 00.051.125c.013.036.03.07.051.125l.01.016c.022.036.047.07.076.1l.014.015c.033.032.069.06.108.084.004.002.006.006.011.008l2.554 1.483a.59.59 0 00.593 0l2.554-1.483a.597.597 0 00.296-.516v-2.965a.595.595 0 00-.296-.516l-2.554-1.483s-.008-.002-.012-.005a.54.54 0 00-.126-.051c-.007-.003-.013-.003-.02-.005a.635.635 0 00-.125-.017h-.018a.557.557 0 00-.16.026.588.588 0 00-.13.053l-1.276.742a1.595 1.595 0 01-1.598 0 1.615 1.615 0 010-2.785l-.005-.001z\"/><path d=\"M14.848 18.265l-2.554-1.482-.012-.005a.526.526 0 00-.126-.052c-.007-.002-.014-.002-.02-.005a.64.64 0 00-.124-.017h-.02a.56.56 0 00-.16.026.588.588 0 00-.13.053l-1.276.742a1.594 1.594 0 01-1.598 0c-.493-.286-.8-.82-.8-1.393V14.65a.563.563 0 00-.018-.14l-.008-.028a.604.604 0 00-.051-.124l-.01-.017a.603.603 0 00-.076-.1l-.014-.015a.596.596 0 00-.109-.084c-.003-.002-.005-.006-.01-.008L5.178 12.65a.587.587 0 00-.591 0l-2.554 1.483a.596.596 0 00-.297.516v2.965c0 .213.113.41.297.516l2.554 1.483.012.004a.618.618 0 00.267.074l.016.002h.007a.55.55 0 00.16-.026.584.584 0 00.129-.053l1.277-.742a1.597 1.597 0 012.398 1.393v1.482c0 .05.007.095.019.14l.007.028c.013.044.03.085.051.125l.01.016c.022.036.047.07.076.1l.014.015c.032.032.069.06.109.084l.01.008 2.554 1.483a.587.587 0 00.593 0l2.554-1.483a.596.596 0 00.296-.515v-2.966a.596.596 0 00-.296-.516h-.002z\"/></g>",
};

// ---------------------------------------------------------------------------
// AgentIcon.tsx: the mark + tint for a session's agent id. Logo agents get
// their mark in text colour (claude keeps brand orange); shell, custom
// commands, unknown ids and missing meta all get a muted terminal glyph.
// ---------------------------------------------------------------------------

pub fn agent_glyph(agent: Option<&str>, ink: &Ink) -> (&'static Icon, Color) {
    match agent {
        Some("claude") => (&CLAUDE_MARK, ink.claude),
        Some("codex") => (&CODEX_MARK, ink.text),
        Some("opencode") => (&OPENCODE_MARK, ink.text),
        Some("devin") => (&DEVIN_MARK, ink.text),
        Some("shell") => (&SQUARE_TERMINAL, ink.muted),
        _ => (&TERMINAL, ink.muted),
    }
}

// ---------------------------------------------------------------------------
// Small builders shared by the chrome regions.
// ---------------------------------------------------------------------------

pub fn txt(s: &str, style: TextStyle, color: Color) -> El {
    text(s).font(style).color(color)
}

pub fn ic(i: &'static Icon, size: f32, color: Color) -> El {
    icon(i, size).color(color)
}

/// The state dot a session row carries, told by the child process's
/// activity:
///
/// - a working child shows the loading ring (the loader arc, turned by
///   `angle`; under Reduce Motion it stands still),
/// - a waiting child blinks amber (the pulse drives its opacity),
/// - a finished child — and a session with nothing running — shows a
///   green dot.
pub fn state_dot(
    state: crate::data::SessionState,
    activity: Option<app_core::activity::Activity>,
    angle: f32,
    pulse: &Live<f32>,
    ink: &Ink,
) -> El {
    if state.ended() {
        return el().size(6.0, 6.0).radius(3.0).fill(state.dot_color(ink));
    }
    match activity {
        Some(app_core::activity::Activity::Working) => el()
            .size(10.0, 10.0)
            .center()
            .child(ic(&LOADER, 10.0, ink.accent).icon_rotation(angle)),
        Some(app_core::activity::Activity::Waiting) => el()
            .size(6.0, 6.0)
            .radius(3.0)
            .fill(ink.warning)
            .live_opacity(pulse),
        _ => el().size(6.0, 6.0).radius(3.0).fill(ink.success),
    }
}

/// Badge variant="secondary" + Count's min-w-6 justify-center px-1.5 tnum.
pub fn count_badge(n: usize, ink: &Ink) -> El {
    row()
        .h(16.0)
        .min_w(20.0)
        .px(5.0)
        .radius(8.0)
        .border(1.0, ink.border)
        .fill(ink.panel)
        .items_center()
        .justify_center()
        .child(txt(&n.to_string(), T11MN_BADGE, ink.text))
}

/// One window's read on a usage chip: the label (`5h`, `7d`, `wk`), the
/// value text (`42%`, `reset`), and how much of the quota is spent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UsageTone {
    Muted,
    Warning,
    Danger,
}

/// One rendered quota window on a chip.
#[derive(Clone, Debug)]
pub struct ChipLimit {
    pub label: String,
    pub value: String,
    pub tone: UsageTone,
    /// The reset time as the tooltip reads it ("in 2h", "just now").
    pub reset_at: String,
}

/// `usageTone()` from usage/tone.ts: >=90 danger, >=80 warning, else muted.
pub fn usage_tone(pct: f64) -> UsageTone {
    if pct >= 90.0 {
        UsageTone::Danger
    } else if pct >= 80.0 {
        UsageTone::Warning
    } else {
        UsageTone::Muted
    }
}

/// UsageChips' outline badge: name + per-window `label value` groups. Every
/// child inherits the badge's 11 px / 500, so window labels and separators are
/// medium too — only the percentage carries tnum. The `·` separators are chip
/// children, not part of a window group, so they sit the same 6 pt from the value
/// before them and the label after them.
pub fn usage_chip(name: &str, limits: &[ChipLimit], ink: &Ink) -> El {
    let mut chip = row()
        .h(20.0)
        .px(8.0)
        .gap(6.0)
        .items_center()
        .radius(10.0)
        .border(1.0, ink.border)
        .child(txt(name, T11M_LH, ink.text));
    for (i, limit) in limits.iter().enumerate() {
        let tone = match limit.tone {
            UsageTone::Danger => ink.danger,
            UsageTone::Warning => ink.warning,
            UsageTone::Muted => ink.muted,
        };
        if i > 0 {
            chip = chip.child(txt("·", T11M_LH, ink.muted));
        }
        chip = chip.child(
            row()
                .gap(4.0)
                .items_center()
                .child(txt(&limit.label, T11M_LH, ink.muted))
                .child(txt(&limit.value, T11MN_BADGE, tone)),
        );
    }
    chip
}

/// Badge variant="warning" with the AlertTriangle icon — CheckoutStatus's
/// "missing" badge (border-warning/30 bg-warning/10 text-warning).
pub fn missing_badge(ink: &Ink) -> El {
    row()
        .h(20.0)
        .px(8.0)
        .gap(4.0)
        .items_center()
        .radius(10.0)
        .border(1.0, ink.warning.alpha(0.3))
        .fill(ink.warning.alpha(0.1))
        .child(ic(&TRIANGLE_ALERT, 12.0, ink.warning))
        .child(txt("missing", T11M, ink.warning))
        .help("This folder no longer exists on disk")
}

/// Button size="sm": h-7, gap-1, text-[0.8rem] font-medium, radius 8.
/// `outline` adds border-border; icon-start buttons get pl-2 pr-2.5.
pub fn btn_sm(label: &str, icon: Option<(&'static Icon, f32)>, outline: bool, ink: &Ink) -> El {
    let mut b = row().h(28.0).gap(4.0).items_center().radius(8.0);
    b = if icon.is_some() {
        b.pl(8.0).pr(10.0)
    } else {
        b.px(10.0)
    };
    if outline {
        b = b.border(1.0, ink.border);
    }
    if let Some((i, size)) = icon {
        b = b.child(ic(i, size, ink.text));
    }
    // Hover and press are text-colour washes, not `ink.bg`: `bg` is the active tab's
    // fill and a darker-than-panel well, so a `bg` hover is invisible on a `bg`
    // surface and reads as "selected" next to one. The press shrinks it 3 % so the
    // button visibly hears the click; the ring shows for keyboard focus only.
    b.child(txt(label, T128, ink.text))
        .hover(ink.text.alpha(0.08))
        .press(ink.text.alpha(0.12))
        .press_scale(0.97)
        .role("button")
        .label(label)
        .focusable(true)
        .focus_ring(2.0, 1.0, ink.accent)
}
