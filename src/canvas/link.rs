//! Optional links on image, frame and text nodes. A small mark sits on any
//! node that has one. Opening goes through the OS default handler so
//! `notion://` works when the desktop has registered it.

use bevy::prelude::*;
use bevy_egui::egui;

use super::camera::world_per_logical_px;
use super::hit::{self, corner_world};
use super::manipulate;
use super::node::{CanvasNode, TabOwner};
use super::selection::NodeQ;
use super::undo::{UndoEntry, UndoStack};
use crate::feedback::Notices;
use crate::project::tabs::Tabs;

#[derive(Resource, Default)]
pub struct LinkEditor {
    pub entity: Option<Entity>,
    pub text: String,
    pub before: Option<String>,
    pub pos: Vec2,
    pub focus_me: bool,
    pub had_focus: bool,
}

pub fn open_link(url: &str) {
    let url = url.trim();
    if !usable_link(url) {
        warn!("refusing to open link");
        return;
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;
        let mut file: Vec<u16> = std::ffi::OsStr::new(url).encode_wide().collect();
        file.push(0);
        let op: Vec<u16> = std::ffi::OsStr::new("open")
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // `SW_SHOWNORMAL`. ShellExecute is the documented way to hand a URL,
        // including a custom scheme, to the registered handler. A return
        // value above 32 means the shell accepted it.
        let rc = unsafe {
            windows_sys::Win32::UI::Shell::ShellExecuteW(
                std::ptr::null_mut(),
                op.as_ptr(),
                file.as_ptr(),
                std::ptr::null::<u16>(),
                std::ptr::null::<u16>(),
                1,
            )
        };
        if (rc as isize) <= 32 {
            warn!("could not open link (ShellExecute {rc:?})");
        }
        return;
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
        return;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}

/// A single scheme URL with no whitespace. `https://…` and `notion://…` both
/// pass; shell metacharacters never reach a shell because the URL is one
/// argument, and this still rejects pasted junk.
pub fn usable_link(url: &str) -> bool {
    let url = url.trim();
    if url.len() < 3 || url.len() > 4096 {
        return false;
    }
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return false;
    }
    let Some((scheme, rest)) = url.split_once(':') else {
        return false;
    };
    if scheme.len() < 2
        || !scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
    {
        return false;
    }
    !rest.is_empty()
}

pub fn draw_link_markers(
    mut gizmos: Gizmos,
    camera: Single<(&Camera, &GlobalTransform)>,
    tabs: Res<Tabs>,
    q: Query<(&CanvasNode, &Transform, &TabOwner), Without<super::node::InactiveTab>>,
) {
    let Some(active) = tabs.active_id() else {
        return;
    };
    let (camera, cam_tf) = *camera;
    let Some(wpp) = world_per_logical_px(camera, cam_tf) else {
        return;
    };
    let radius = (4.5 * wpp as f32).max(0.5);
    let mark = Color::srgb(0.95, 0.95, 0.95);
    for (node, tf, owner) in &q {
        if owner.0 != active || node.link.is_none() {
            continue;
        }
        let pose = manipulate::current_pose(tf, node);
        let corner = corner_world(&pose, 1);
        let inward = hit::rot2(pose.rot) * Vec2::new(-1.0, -1.0).normalize();
        let pos = corner + inward * radius * 2.4;
        gizmos.circle_2d(Isometry2d::from_translation(pos), radius, mark);
    }
}

pub fn edit_ui(
    mut contexts: bevy_egui::EguiContexts,
    mut editor: ResMut<LinkEditor>,
    mut undo: ResMut<UndoStack>,
    mut notices: ResMut<Notices>,
    time: Res<Time>,
    mut q: NodeQ,
) -> Result {
    let Some(entity) = editor.entity else {
        return Ok(());
    };
    let Ok((_, _, _, mut node)) = q.get_mut(entity) else {
        editor.entity = None;
        return Ok(());
    };
    let ctx = contexts.ctx_mut()?;
    let pos = editor.pos;
    let mut commit = false;
    let mut cancel = false;
    egui::Area::new(egui::Id::new("refcanvas_link_edit"))
        .fixed_pos(egui::pos2(pos.x, pos.y))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(280.0);
                ui.label("Link");
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut editor.text)
                        .desired_width(280.0)
                        .hint_text("https://… or notion://…"),
                );
                if editor.focus_me {
                    resp.request_focus();
                    editor.focus_me = false;
                }
                if resp.has_focus() {
                    editor.had_focus = true;
                }
                if editor.had_focus {
                    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        cancel = true;
                    } else if resp.lost_focus() {
                        commit = true;
                    }
                }
            });
        });

    if cancel {
        editor.entity = None;
        return Ok(());
    }
    if commit {
        let next = editor.text.trim().to_string();
        if next.is_empty() {
            set_link(&mut node, entity, None, &editor.before, &mut undo);
            editor.entity = None;
        } else if usable_link(&next) {
            set_link(&mut node, entity, Some(next), &editor.before, &mut undo);
            editor.entity = None;
        } else {
            notices.push(
                "Link needs a scheme, like https:// or notion://",
                time.elapsed_secs(),
            );
            editor.focus_me = true;
            editor.had_focus = false;
        }
    }
    Ok(())
}

fn set_link(
    node: &mut CanvasNode,
    entity: Entity,
    after: Option<String>,
    before: &Option<String>,
    undo: &mut UndoStack,
) {
    if &node.link == &after || node.link == after {
        return;
    }
    let before = before.clone().or_else(|| node.link.clone());
    node.link = after.clone();
    if before != after {
        undo.push(UndoEntry::Link {
            entity,
            before,
            after,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::usable_link;

    #[test]
    fn links_allow_https_and_custom_schemes() {
        assert!(usable_link("https://www.notion.so/page"));
        assert!(usable_link("notion://abc"));
        assert!(usable_link("http://example.com/a.png"));
        assert!(!usable_link("notaurl"));
        assert!(!usable_link("https://has space"));
        assert!(!usable_link("://missing"));
    }
}
