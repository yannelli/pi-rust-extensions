//! Builtin status line presets.

use crate::chips::Chip;
use crate::icons::Icons;
use crate::segments::Segment::{self, *};

pub struct Preset {
    pub name: &'static str,
    pub chip: Chip,
    pub theme: &'static str,
    pub icons: Icons,
    pub lines: &'static [&'static [Segment]],
    pub separator: &'static str,
}

const CHIPS: &[Segment] = &[Dir, Branch, Fill, Model, Thinking, ContextBar, Cost];

pub const PRESETS: &[Preset] = &[
    Preset {
        name: "compact",
        chip: Chip::Plain,
        theme: "terminal",
        icons: Icons::Unicode,
        lines: &[&[
            Dir,
            Branch,
            Model,
            Thinking,
            ContextRemaining,
            ContextHeadroom,
        ]],
        separator: " · ",
    },
    Preset {
        name: "stacked",
        chip: Chip::Block,
        theme: "pi-rust",
        icons: Icons::Unicode,
        lines: &[
            &[Dir, Branch, Session],
            &[Model, Thinking, ContextRemaining, ContextHeadroom, Cost],
        ],
        separator: " ",
    },
    Preset {
        name: "minimal",
        chip: Chip::Plain,
        theme: "terminal",
        icons: Icons::Unicode,
        lines: &[&[Model, Thinking, Context, Cost, Fill, Dir, Branch]],
        separator: " • ",
    },
    Preset {
        name: "pi-rust",
        chip: Chip::Block,
        theme: "pi-rust",
        icons: Icons::Unicode,
        lines: &[&[Logo, Dir, Branch, Fill, Model, Thinking, ContextBar, Cost]],
        separator: " ",
    },
    Preset {
        name: "pi",
        chip: Chip::Block,
        theme: "pi",
        icons: Icons::Unicode,
        lines: &[&[Logo, Dir, Branch, Fill, Model, Thinking, ContextBar, Cost]],
        separator: " ",
    },
    Preset {
        name: "pills",
        chip: Chip::Block,
        theme: "pi-rust",
        icons: Icons::Unicode,
        lines: &[CHIPS],
        separator: " ",
    },
    Preset {
        name: "powerline",
        chip: Chip::Powerline,
        theme: "pi-rust",
        icons: Icons::Unicode,
        lines: &[CHIPS],
        separator: " ",
    },
    Preset {
        name: "claude",
        chip: Chip::Block,
        theme: "claude",
        icons: Icons::Unicode,
        lines: &[&[Model, Thinking, Dir, Branch, Fill, ContextBar, Cost]],
        separator: " ",
    },
    Preset {
        name: "codex",
        chip: Chip::Plain,
        theme: "codex",
        icons: Icons::None,
        lines: &[&[ModelWithReasoning, ContextRemaining, Cwd, Branch]],
        separator: " · ",
    },
    Preset {
        name: "cursor",
        chip: Chip::Plain,
        theme: "cursor",
        icons: Icons::Unicode,
        lines: &[&[Model, ContextUsed, Fill, Cwd, Branch]],
        separator: " · ",
    },
    Preset {
        name: "rounded",
        chip: Chip::Rounded,
        theme: "catppuccin",
        icons: Icons::Nerd,
        lines: &[CHIPS],
        separator: " ",
    },
    Preset {
        name: "slanted",
        chip: Chip::Slanted,
        theme: "tokyo-night",
        icons: Icons::Nerd,
        lines: &[CHIPS],
        separator: " ",
    },
    Preset {
        name: "retro",
        chip: Chip::Brackets,
        theme: "gruvbox",
        icons: Icons::Unicode,
        lines: &[&[Dir, Branch, Fill, Model, Thinking, Context, Cost, Time]],
        separator: " ",
    },
    Preset {
        name: "dots",
        chip: Chip::Dot,
        theme: "nord",
        icons: Icons::Unicode,
        lines: &[CHIPS],
        separator: "  ",
    },
    Preset {
        name: "emoji",
        chip: Chip::Block,
        theme: "catppuccin",
        icons: Icons::Emoji,
        lines: &[&[Dir, Branch, Fill, Model, Thinking, ContextBar, Cost, Time]],
        separator: " ",
    },
    Preset {
        name: "mono",
        chip: Chip::Block,
        theme: "mono",
        icons: Icons::Unicode,
        lines: &[CHIPS],
        separator: " ",
    },
    Preset {
        name: "full",
        chip: Chip::Block,
        theme: "pi-rust",
        icons: Icons::Unicode,
        lines: &[
            &[Logo, Cwd, Branch, Session, Fill, Version, Time],
            &[
                ModelWithReasoning,
                Provider,
                Tokens,
                Cache,
                Cost,
                Fill,
                RunState,
                ContextBar,
            ],
        ],
        separator: " ",
    },
];

/// The preset names, for `/statusline` and docs.
pub fn preset_names() -> impl Iterator<Item = &'static str> {
    PRESETS.iter().map(|preset| preset.name)
}
