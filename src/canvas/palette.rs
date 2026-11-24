//! The 16 classic VGA colors, shared by strokes and text labels.
//!
//! Indices are part of the on-disk format. Do not reorder them.

use bevy::prelude::*;

/// Black, the dark primaries, the bright primaries, then white. Index 15 is
/// white, the default for labels on the dark canvas.
pub const PALETTE_SRGB: [[u8; 3]; 16] = [
    [0x00, 0x00, 0x00],
    [0x80, 0x00, 0x00],
    [0x00, 0x80, 0x00],
    [0x80, 0x80, 0x00],
    [0x00, 0x00, 0x80],
    [0x80, 0x00, 0x80],
    [0x00, 0x80, 0x80],
    [0xC0, 0xC0, 0xC0],
    [0x80, 0x80, 0x80],
    [0xFF, 0x00, 0x00],
    [0x00, 0xFF, 0x00],
    [0xFF, 0xFF, 0x00],
    [0x00, 0x00, 0xFF],
    [0xFF, 0x00, 0xFF],
    [0x00, 0xFF, 0xFF],
    [0xFF, 0xFF, 0xFF],
];

pub const PALETTE_LEN: u8 = 16;

pub fn clamp_index(index: u8) -> u8 {
    index.min(PALETTE_LEN - 1)
}

pub fn color(index: u8) -> Color {
    let [r, g, b] = PALETTE_SRGB[clamp_index(index) as usize];
    Color::srgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
}
