//! Text label nodes. A label is an ordinary canvas node: it moves, scales,
//! rotates and deletes with the same gizmos as an image. Scaling changes
//! `CanvasNode::size`; the glyph mesh is a child whose scale maps the laid-out
//! text onto that box. The typeface is IBM Plex Sans (OFL), bundled in the binary.

use bevy::prelude::*;
use bevy::sprite::Anchor;
use bevy::text::{Font, FontSource, TextLayoutInfo};

use super::camera::View;
use super::hit;
use super::manipulate::{self, ZCounter};
use super::node::{CanvasNode, NodeKind, Pose, TabOwner, TextNode};
use super::palette;
use super::undo::{UndoEntry, UndoStack};
use crate::project::oplog::NodeId;

/// Parent entity of a label's [`Text2d`].
#[derive(Component)]
pub struct TextBody(pub Entity);

#[derive(Resource)]
pub struct LabelFont(pub Handle<Font>);

#[derive(Resource, Default)]
pub struct TextEdit {
    pub target: Option<Entity>,
    pub text: String,
    pub focus_me: bool,
    pub had_focus: bool,
    /// Last primary press, shared with link double-click detection.
    pub last_press: Option<(std::time::Instant, Entity)>,
}

const REF_FONT_PX: f32 = 64.0;
/// World height of a label the first time it is measured.
const DEFAULT_HEIGHT: f32 = 36.0;

pub struct TextPlugin;

impl Plugin for TextPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TextEdit>()
            .add_systems(Startup, load_font)
            .add_systems(Update, sync_labels.in_set(super::CanvasLateSet))
            .add_systems(bevy_egui::EguiPrimaryContextPass, edit_ui);
    }
}

fn load_font(mut commands: Commands, mut fonts: ResMut<Assets<Font>>) {
    let bytes = include_bytes!("../../assets/fonts/IBMPlexSans-Regular.ttf").to_vec();
    commands.insert_resource(LabelFont(fonts.add(Font::from_bytes(bytes))));
}

pub fn spawn_text(
    commands: &mut Commands,
    tab_id: u64,
    visible: bool,
    node_id: Option<NodeId>,
    body: String,
    color: u8,
    pose: Pose,
    z: f32,
    link: Option<String>,
    fitted: bool,
) -> Entity {
    let revision = 1;
    let node = commands
        .spawn((
            CanvasNode {
                node_id,
                kind: NodeKind::Text(TextNode {
                    body: body.clone(),
                    color,
                    revision,
                    fitted_revision: if fitted { revision } else { 0 },
                    fit_wait: if fitted { 0 } else { 2 },
                }),
                size: pose.size,
                rotation: pose.rot,
                link,
            },
            Sprite::from_color(Color::NONE, pose.size.max(Vec2::splat(1.0))),
            Transform {
                translation: Vec3::new(pose.center.x, pose.center.y, z),
                rotation: Quat::from_rotation_z(pose.rot),
                ..default()
            },
            TabOwner(tab_id),
            if visible {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            },
        ))
        .id();
    commands.spawn((
        Text2d::new(body),
        TextFont {
            font_size: bevy::text::FontSize::Px(REF_FONT_PX),
            ..default()
        },
        TextColor(palette::color(color)),
        Anchor::CENTER,
        Transform::from_xyz(0.0, 0.0, 0.1),
        TextBody(node),
        TabOwner(tab_id),
        ChildOf(node),
    ));
    if !visible {
        commands.entity(node).insert(super::node::InactiveTab);
    }
    node
}

/// Create a label at `center` and open the in-place editor.
pub fn create_text(
    commands: &mut Commands,
    zc: &mut ZCounter,
    undo: &mut UndoStack,
    tab_id: u64,
    center: Vec2,
    color: u8,
    edit: &mut TextEdit,
) -> Entity {
    let pose = Pose {
        center,
        size: Vec2::new(120.0, DEFAULT_HEIGHT),
        rot: 0.0,
    };
    let z = zc.next();
    let body = "Label".to_string();
    let entity = spawn_text(
        commands,
        tab_id,
        true,
        None,
        body.clone(),
        color,
        pose,
        z,
        None,
        false,
    );
    undo.push(UndoEntry::Added {
        items: vec![super::undo::NodeRecord {
            entity,
            tab_id,
            link: None,
            data: super::undo::NodeData::Text {
                body: body.clone(),
                color,
            },
            snap: super::node::NodeSnapshot {
                pose,
                z,
                flip_x: false,
                flip_y: false,
                crop: Rect::from_corners(Vec2::ZERO, Vec2::ONE),
                px_scale: 1.0,
            },
        }],
    });
    edit.target = Some(entity);
    edit.text = body;
    edit.focus_me = true;
    edit.had_focus = false;
    entity
}

fn sync_labels(
    font: Option<Res<LabelFont>>,
    mut undo: ResMut<UndoStack>,
    mut nodes: Query<(&mut CanvasNode, &mut Sprite), Without<TextBody>>,
    mut labels: Query<
        (
            &TextBody,
            &mut Transform,
            &mut Text2d,
            &mut TextColor,
            &mut TextFont,
            &TextLayoutInfo,
        ),
        Without<CanvasNode>,
    >,
) {
    let Some(font) = font else {
        return;
    };
    for (body, mut tf, mut text, mut color, mut text_font, layout) in &mut labels {
        let Ok((mut node, mut sprite)) = nodes.get_mut(body.0) else {
            continue;
        };
        let height_now = node.size.y;
        let logical = layout.size / layout.scale_factor.max(1e-4);
        let mut fitted_size = None;
        {
            let Some(label) = node.kind.text_mut() else {
                continue;
            };
            let handle = FontSource::Handle(font.0.clone());
            if text_font.font != handle {
                text_font.font = handle;
            }
            let want = palette::color(label.color);
            if color.0 != want {
                color.0 = want;
            }
            if text.0 != label.body {
                text.0 = label.body.clone();
                label.fit_wait = label.fit_wait.max(2);
                continue;
            }
            if label.fit_wait > 0 {
                label.fit_wait -= 1;
            }
            if logical.x >= 1.0
                && logical.y >= 1.0
                && label.fitted_revision != label.revision
                && label.fit_wait == 0
            {
                let height = if label.fitted_revision == 0 {
                    DEFAULT_HEIGHT
                } else {
                    height_now.max(8.0)
                };
                let aspect = logical.x / logical.y.max(1.0);
                fitted_size = Some(Vec2::new((height * aspect).max(8.0), height));
                label.fitted_revision = label.revision;
            }
        }
        if let Some(size) = fitted_size {
            node.size = size;
            sprite.custom_size = Some(size);
            note_text_size(&mut undo, body.0, size);
        }
        if logical.x < 1.0 || logical.y < 1.0 {
            continue;
        }
        let scale = Vec2::new(
            node.size.x / logical.x.max(1.0),
            node.size.y / logical.y.max(1.0),
        );
        let want_scale = Vec3::new(scale.x, scale.y, 1.0);
        if tf.scale != want_scale {
            tf.scale = want_scale;
        }
    }
}

fn note_text_size(undo: &mut UndoStack, entity: Entity, size: Vec2) {
    for entry in undo.past.iter_mut().rev() {
        match entry {
            UndoEntry::Added { items } => {
                for item in items.iter_mut() {
                    if item.entity == entity {
                        item.snap.pose.size = size;
                        return;
                    }
                }
            }
            UndoEntry::TextBody {
                entity: id,
                after_size,
                ..
            } if *id == entity => {
                *after_size = size;
                return;
            }
            _ => {}
        }
    }
}

fn edit_ui(
    mut contexts: bevy_egui::EguiContexts,
    mut edit: ResMut<TextEdit>,
    mut undo: ResMut<UndoStack>,
    camera: Single<(&Camera, &GlobalTransform)>,
    view: Res<View>,
    mut q: super::selection::NodeQ,
) -> Result {
    let Some(target) = edit.target else {
        return Ok(());
    };
    let Ok((_, tf, _sprite, mut node)) = q.get_mut(target) else {
        edit.target = None;
        return Ok(());
    };
    if !node.kind.is_text() {
        edit.target = None;
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    let wpp = (view.scale() as f32).max(1e-4);
    let pose = manipulate::current_pose(&tf, &node);
    let anchor = hit::corner_world(&pose, 0);
    let (camera, cam_tf) = *camera;
    let Ok(pos) = camera.world_to_viewport(cam_tf, anchor.extend(0.0)) else {
        return Ok(());
    };
    let width = (node.size.x / wpp).clamp(140.0, 420.0);

    let mut commit = false;
    let mut cancel = false;
    bevy_egui::egui::Area::new(bevy_egui::egui::Id::new("refcanvas_text_edit"))
        .fixed_pos(bevy_egui::egui::pos2(pos.x, pos.y))
        .order(bevy_egui::egui::Order::Foreground)
        .show(ctx, |ui| {
            bevy_egui::egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(width);
                let resp = ui.add(
                    bevy_egui::egui::TextEdit::multiline(&mut edit.text)
                        .desired_width(width)
                        .desired_rows(3)
                        .hint_text("Label"),
                );
                if edit.focus_me {
                    resp.request_focus();
                    edit.focus_me = false;
                }
                if resp.has_focus() {
                    edit.had_focus = true;
                }
                if edit.had_focus {
                    let (escape, commit_key) = ui.input(|i| {
                        (
                            i.key_pressed(bevy_egui::egui::Key::Escape),
                            i.key_pressed(bevy_egui::egui::Key::Enter) && i.modifiers.command,
                        )
                    });
                    if escape {
                        cancel = true;
                    } else if commit_key || resp.lost_focus() {
                        commit = true;
                    }
                }
            });
        });

    if cancel {
        edit.target = None;
    } else if commit {
        let next = edit.text.trim().chars().take(500).collect::<String>();
        let before_size = node.size;
        if let Some(label) = node.kind.text_mut()
            && !next.is_empty()
            && next != label.body
        {
            let before = std::mem::replace(&mut label.body, next);
            let color = label.color;
            label.revision = label.revision.wrapping_add(1);
            label.fit_wait = 2;
            let after = label.body.clone();
            undo.push(UndoEntry::TextBody {
                entity: target,
                before,
                after,
                before_color: color,
                after_color: color,
                before_size,
                after_size: before_size,
            });
        }
        edit.target = None;
    }
    Ok(())
}
