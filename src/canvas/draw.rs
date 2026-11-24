//! Brush and eraser. Strokes are vectors: the pointer samples are smoothed,
//! simplified, then stored. A stroke that starts on an image is stored in
//! that image's texel space; anything else is its own node.

use std::collections::HashMap;

use bevy::prelude::*;
use bevy_egui::egui;

use super::egui_pointer_busy;
use super::hit::point_inside;
use super::ink::{self, ShowStrokes};
use super::manipulate::{ZCounter, current_pose};
use super::node::{CanvasNode, ImageStroke, NodeKind, Pose, StrokeNode, TabOwner};
use super::palette;
use super::selection::{self, NodeQ};
use super::stroke::{self, ImageSpace};
use super::touch::Pointer;
use super::undo::{InkSnapshot, NodeData, NodeRecord, UndoEntry, UndoStack};
use crate::project::local;
use crate::project::tabs::Tabs;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Select,
    Brush,
    Eraser,
}

#[derive(Resource)]
pub struct DrawTool {
    pub tool: Tool,
    pub color: u8,
    pub width_px: f32,
}

impl Default for DrawTool {
    fn default() -> Self {
        Self {
            tool: Tool::Select,
            color: 15,
            width_px: 4.0,
        }
    }
}

struct Gesture {
    kind: Tool,
    samples: Vec<Vec2>,
    anchor: Option<Entity>,
    width_world: f32,
    color: u8,
    image_before: HashMap<Entity, Vec<ImageStroke>>,
    removed: Vec<NodeRecord>,
}

#[derive(Resource, Default)]
struct ActiveGesture(Option<Gesture>);

pub struct DrawPlugin;

impl Plugin for DrawPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DrawTool>()
            .init_resource::<ActiveGesture>()
            .add_systems(
                Update,
                draw_input
                    .before(super::selection::pointer_events)
                    .in_set(super::CanvasSet),
            )
            .add_systems(bevy_egui::EguiPrimaryContextPass, tool_ui);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_input(
    pointer: Res<Pointer>,
    keys: Res<ButtonInput<KeyCode>>,
    camera: Single<(&Camera, &GlobalTransform)>,
    mut contexts: bevy_egui::EguiContexts,
    mut tool: ResMut<DrawTool>,
    mut gesture: ResMut<ActiveGesture>,
    mut undo: ResMut<UndoStack>,
    mut commands: Commands,
    mut zc: ResMut<ZCounter>,
    tabs: Res<Tabs>,
    show: Res<ShowStrokes>,
    q_own: Query<(Entity, &TabOwner), Without<super::node::InactiveTab>>,
    mut q: NodeQ,
) {
    if keys.just_pressed(KeyCode::BracketLeft) {
        tool.width_px = (tool.width_px - 1.0).max(1.0);
    }
    if keys.just_pressed(KeyCode::BracketRight) {
        tool.width_px = (tool.width_px + 1.0).min(64.0);
    }

    if keys.pressed(KeyCode::Space) {
        if gesture.0.as_ref().is_some_and(|g| g.kind == Tool::Brush) {
            gesture.0 = None;
        }
        return;
    }

    let drawing = matches!(tool.tool, Tool::Brush | Tool::Eraser);
    if gesture.0.is_some() && !drawing {
        finish(
            &mut gesture,
            &mut undo,
            &mut commands,
            &mut zc,
            &tabs,
            &mut q,
        );
    }
    if !drawing {
        return;
    }
    if pointer.is_touch || egui_pointer_busy(&mut contexts) {
        return;
    }

    let (camera, cam_tf) = *camera;
    let Some(cursor) = pointer.screen else {
        return;
    };
    let Ok(world) = camera.viewport_to_world_2d(cam_tf, cursor) else {
        return;
    };
    let wpp = super::camera::world_per_logical_px(camera, cam_tf).unwrap_or(1.0) as f32;
    let keep = selection::owner_filter(tabs.active_id(), &q_own);

    if pointer.just_pressed && gesture.0.is_none() {
        let anchor = if tool.tool == Tool::Brush {
            top_image(world, &q, &keep)
        } else {
            None
        };
        gesture.0 = Some(Gesture {
            kind: tool.tool,
            samples: vec![world],
            anchor,
            width_world: tool.width_px * wpp,
            color: tool.color,
            image_before: HashMap::new(),
            removed: Vec::new(),
        });
        if tool.tool == Tool::Eraser {
            erase_at(
                world,
                wpp,
                show.0,
                tabs.active_id().unwrap_or(0),
                &keep,
                &mut gesture,
                &mut commands,
                &mut q,
            );
        }
    } else if pointer.pressed {
        if let Some(g) = gesture.0.as_mut() {
            if g.kind == Tool::Brush {
                let last = g.samples.last().copied().unwrap_or(world);
                if last.distance(world) >= 1.25 * wpp {
                    g.samples.push(world);
                }
            }
        }
        if tool.tool == Tool::Eraser {
            erase_at(
                world,
                wpp,
                show.0,
                tabs.active_id().unwrap_or(0),
                &keep,
                &mut gesture,
                &mut commands,
                &mut q,
            );
        }
    }

    if pointer.just_released {
        finish(
            &mut gesture,
            &mut undo,
            &mut commands,
            &mut zc,
            &tabs,
            &mut q,
        );
    }
}

fn finish(
    gesture: &mut ActiveGesture,
    undo: &mut UndoStack,
    commands: &mut Commands,
    zc: &mut ZCounter,
    tabs: &Tabs,
    q: &mut NodeQ,
) {
    let Some(g) = gesture.0.take() else {
        return;
    };
    let tab_id = tabs.active_id().unwrap_or(0);
    match g.kind {
        Tool::Brush => commit_brush(g, undo, commands, zc, tabs, q),
        Tool::Eraser => commit_eraser(g, undo, q, tab_id),
        Tool::Select => {}
    }
}

fn commit_brush(
    g: Gesture,
    undo: &mut UndoStack,
    commands: &mut Commands,
    zc: &mut ZCounter,
    tabs: &Tabs,
    q: &mut NodeQ,
) {
    let Some(anchor) = g.anchor else {
        commit_free(g, undo, commands, zc, tabs);
        return;
    };
    let Ok((_, tf, sprite, mut node)) = q.get_mut(anchor) else {
        commit_free(g, undo, commands, zc, tabs);
        return;
    };
    let Some(space) = image_space(&tf, &node, &sprite) else {
        drop((tf, sprite, node));
        commit_free(g, undo, commands, zc, tabs);
        return;
    };
    let texels: Vec<Vec2> = g.samples.iter().map(|p| space.world_to_texel(*p)).collect();
    let width = (g.width_world / space.px_scale.max(1e-6)).max(0.4);
    let points = stroke::smooth_stroke(&texels, width * 0.35);
    if points.is_empty() {
        return;
    }
    let Some(img) = node.image_mut() else {
        return;
    };
    let before = img.data.strokes.clone();
    img.data.strokes.push(ImageStroke {
        points,
        width,
        color: g.color,
    });
    let after = img.data.strokes.clone();
    undo.push(UndoEntry::Ink {
        images: vec![InkSnapshot {
            entity: anchor,
            before,
            after,
        }],
        removed: Vec::new(),
    });
}

fn commit_free(
    g: Gesture,
    undo: &mut UndoStack,
    commands: &mut Commands,
    zc: &mut ZCounter,
    tabs: &Tabs,
) {
    let points = stroke::smooth_stroke(&g.samples, g.width_world * 0.35);
    if points.is_empty() {
        return;
    }
    let (center, local, size) = bounds_of(&points, g.width_world);
    let z = zc.next();
    let tab_id = tabs.active_id().unwrap_or(0);
    let pose = Pose {
        center,
        size,
        rot: 0.0,
    };
    let data = StrokeNode {
        points: local,
        width: g.width_world,
        color: g.color,
        space: size,
    };
    let entity = ink::spawn_stroke_node(commands, tab_id, true, None, data.clone(), pose, z, None);
    undo.push(UndoEntry::Added {
        items: vec![NodeRecord {
            entity,
            tab_id,
            link: None,
            data: NodeData::Stroke(data),
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
}

fn bounds_of(points: &[Vec2], width: f32) -> (Vec2, Vec<Vec2>, Vec2) {
    if points.len() == 1 {
        let size = Vec2::splat(width.max(1.0));
        return (points[0], vec![Vec2::ZERO], size);
    }
    let mut min = Vec2::splat(f32::INFINITY);
    let mut max = Vec2::splat(f32::NEG_INFINITY);
    for p in points {
        min = min.min(*p);
        max = max.max(*p);
    }
    let center = (min + max) * 0.5;
    let local = points.iter().map(|p| *p - center).collect();
    let size = (max - min) + Vec2::splat(width.max(1.0));
    (center, local, size.max(Vec2::splat(1.0)))
}

fn commit_eraser(g: Gesture, undo: &mut UndoStack, q: &NodeQ, tab_id: u64) {
    let mut images = Vec::new();
    for (entity, before) in g.image_before {
        let after = q
            .get(entity)
            .ok()
            .and_then(|(_, _, _, node)| node.image().map(|img| img.data.strokes.clone()))
            .unwrap_or_default();
        if before != after {
            images.push(InkSnapshot {
                entity,
                before,
                after,
            });
        }
    }
    let mut removed = g.removed;
    for record in &mut removed {
        record.tab_id = tab_id;
    }
    if images.is_empty() && removed.is_empty() {
        return;
    }
    undo.push(UndoEntry::Ink { images, removed });
}

fn erase_at(
    world: Vec2,
    wpp: f32,
    show_all: bool,
    tab_id: u64,
    keep: &impl Fn(Entity) -> bool,
    gesture: &mut ActiveGesture,
    commands: &mut Commands,
    q: &mut NodeQ,
) {
    let Some(g) = gesture.0.as_mut() else {
        return;
    };
    let extra = 8.0 * wpp;
    let mut doomed = Vec::new();
    for (entity, tf, sprite, node) in q.iter() {
        if !keep(entity) {
            continue;
        }
        match &node.kind {
            NodeKind::Image(img) => {
                if !show_all || img.data.strokes_hidden || img.data.strokes.is_empty() {
                    continue;
                }
                let Some(space) = image_space(tf, node, sprite) else {
                    continue;
                };
                let hit = img
                    .data
                    .strokes
                    .iter()
                    .any(|stroke| stroke::hit_image_stroke(stroke, space, world, extra));
                if hit {
                    doomed.push(entity);
                }
            }
            NodeKind::Stroke(stroke) => {
                if !show_all {
                    continue;
                }
                let pose = current_pose(tf, node);
                if stroke::hit_free_stroke(stroke, pose.center, pose.rot, pose.size, world, extra) {
                    doomed.push(entity);
                }
            }
            _ => {}
        }
    }
    for entity in doomed {
        let is_stroke = q
            .get(entity)
            .ok()
            .is_some_and(|(_, _, _, node)| node.kind.is_stroke());
        if is_stroke {
            if let Ok((_, tf, sprite, node)) = q.get(entity) {
                let record = record_of(entity, tab_id, tf, sprite, node);
                commands.entity(entity).despawn();
                g.removed.push(record);
            }
            continue;
        }
        let space = q
            .get(entity)
            .ok()
            .and_then(|(_, tf, sprite, node)| image_space(tf, node, sprite));
        let Some(space) = space else {
            continue;
        };
        let Ok((_, _, _, mut node)) = q.get_mut(entity) else {
            continue;
        };
        let Some(img) = node.image_mut() else {
            continue;
        };
        g.image_before
            .entry(entity)
            .or_insert_with(|| img.data.strokes.clone());
        img.data
            .strokes
            .retain(|stroke| !stroke::hit_image_stroke(stroke, space, world, extra));
    }
}

fn record_of(
    entity: Entity,
    tab_id: u64,
    tf: &Transform,
    sprite: &Sprite,
    node: &CanvasNode,
) -> NodeRecord {
    let data = match &node.kind {
        NodeKind::Stroke(stroke) => NodeData::Stroke(stroke.clone()),
        NodeKind::Text(text) => NodeData::Text {
            body: text.body.clone(),
            color: text.color,
        },
        NodeKind::Image(img) => NodeData::Image(img.data.clone()),
        NodeKind::Frame(frame) => NodeData::Frame {
            label: frame.label.clone(),
        },
    };
    NodeRecord {
        entity,
        tab_id,
        link: node.link.clone(),
        data,
        snap: super::undo::snapshot_of(tf, node, sprite),
    }
}

fn image_space(tf: &Transform, node: &CanvasNode, sprite: &Sprite) -> Option<ImageSpace> {
    let img = node.image()?;
    Some(ImageSpace {
        center: tf.translation.truncate(),
        rot: node.rotation,
        px_scale: img.data.px_scale,
        crop: img.data.crop,
        flip_x: sprite.flip_x,
        flip_y: sprite.flip_y,
    })
}

fn top_image(world: Vec2, q: &NodeQ, keep: &impl Fn(Entity) -> bool) -> Option<Entity> {
    let mut items: Vec<(Entity, f32)> = q
        .iter()
        .filter(|(e, _, _, node)| keep(*e) && node.image().is_some())
        .map(|(e, tf, _, _)| (e, tf.translation.z))
        .collect();
    items.sort_by(|a, b| b.1.total_cmp(&a.1));
    for (entity, _) in items {
        let Ok((_, tf, _, node)) = q.get(entity) else {
            continue;
        };
        if point_inside(world, &current_pose(tf, node)) {
            return Some(entity);
        }
    }
    None
}

fn tool_ui(
    mut contexts: bevy_egui::EguiContexts,
    mut tool: ResMut<DrawTool>,
    mut show: ResMut<ShowStrokes>,
    mut selection: ResMut<super::selection::Selection>,
    mut undo: ResMut<UndoStack>,
    mut q: NodeQ,
) -> Result {
    let ctx = contexts.ctx_mut()?;

    egui::Area::new(egui::Id::new("refcanvas_stroke_toggle"))
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-10.0, -40.0))
        .show(ctx, |ui| {
            let label = if show.0 {
                "Hide strokes"
            } else {
                "Show strokes"
            };
            if ui.button(label).clicked() {
                show.0 = !show.0;
                local::save_show_strokes(show.0);
            }
        });

    egui::Area::new(egui::Id::new("refcanvas_tools"))
        .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -8.0))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .inner_margin(egui::Margin::symmetric(6, 4))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        if ui
                            .selectable_label(tool.tool == Tool::Select, "Select")
                            .clicked()
                        {
                            tool.tool = Tool::Select;
                        }
                        if ui
                            .selectable_label(tool.tool == Tool::Brush, "Brush")
                            .clicked()
                        {
                            tool.tool = Tool::Brush;
                        }
                        if ui
                            .selectable_label(tool.tool == Tool::Eraser, "Eraser")
                            .clicked()
                        {
                            tool.tool = Tool::Eraser;
                        }
                        ui.label(format!("{:.0}px", tool.width_px));
                        for i in 0..palette::PALETTE_LEN {
                            let [r, g, b] = palette::PALETTE_SRGB[i as usize];
                            let fill = egui::Color32::from_rgb(r, g, b);
                            let mut button = egui::Button::new("")
                                .fill(fill)
                                .min_size(egui::vec2(16.0, 16.0));
                            if tool.color == i {
                                button =
                                    button.stroke(egui::Stroke::new(2.0, egui::Color32::WHITE));
                            }
                            if ui.add(button).clicked() {
                                tool.color = i;
                                recolor_selected_text(i, &mut selection, &mut undo, &mut q);
                            }
                        }
                    });
                });
        });
    Ok(())
}

fn recolor_selected_text(
    color: u8,
    selection: &mut super::selection::Selection,
    undo: &mut UndoStack,
    q: &mut NodeQ,
) {
    if selection.nodes.len() != 1 {
        return;
    }
    let entity = selection.nodes[0];
    let Ok((_, _, _, mut node)) = q.get_mut(entity) else {
        return;
    };
    let size = node.size;
    let Some(text) = node.kind.text_mut() else {
        return;
    };
    if text.color == color {
        return;
    }
    let before_color = text.color;
    let body = text.body.clone();
    text.color = color;
    undo.push(UndoEntry::TextBody {
        entity,
        before: body.clone(),
        after: body,
        before_color,
        after_color: color,
        before_size: size,
        after_size: size,
    });
}
