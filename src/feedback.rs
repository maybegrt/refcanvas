//! Short status-line messages (a failed download, a copied URL).

use bevy::prelude::*;

#[derive(Resource, Default)]
pub struct Notices {
    text: Option<String>,
    until: f32,
}

impl Notices {
    pub fn push(&mut self, text: impl Into<String>, now: f32) {
        self.text = Some(text.into());
        self.until = now + 6.5;
    }

    pub fn current(&self, now: f32) -> Option<&str> {
        self.text.as_deref().filter(|_| now < self.until)
    }
}
