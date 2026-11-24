//! Stroke meshes. One mesh entity per stroke, parented to its owner so a
//! move or rotate is just the parent's transform. The mesh is rebuilt only
//! when the points, crop or flip change; scaling updates the child's scale.

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, Mesh, PrimitiveTopology};
use bevy::prelude::*;
use bevy::sprite_render::{ColorMaterial, MeshMaterial2d};

use super::node::{CanvasNode, Pose, StrokeNode, TabOwner};
use super::palette;
use super::stroke::{self, ImageSpace};
use crate::project::local;
use crate::project::oplog::NodeId;

/// Global "show strokes" switch. The corner button writes it; per-image
/// hiding is stored on the image and survives this.
#[derive(Resource)]
pub struct ShowStrokes(pub bool);

impl Default for ShowStrokes {
    fn default() -> Self {
        Self(true)
    }
}

#[derive(Resource)]
pub struct InkMaterials(pub [Handle<ColorMaterial>; 16]);

#[derive(Component)]
struct InkMesh {
    owner: Entity,
    index: u32,
    stamp: u64,
    scale: Vec2,
}

pub struct InkPlugin;

impl Plugin for InkPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ShowStrokes>()
            .add_systems(Startup, (load_show_pref, setup_materials))
            .add_systems(Update, sync_ink.in_set(super::CanvasLateSet));
    }
}

fn load_show_pref(mut show: ResMut<ShowStrokes>) {
    show.0 = local::load_show_strokes();
}

fn setup_materials(mut commands: Commands, mut materials: ResMut<Assets<ColorMaterial>>) {
    let handles =
        std::array::from_fn(|i| materials.add(ColorMaterial::from(palette::color(i as u8))));
    commands.insert_resource(InkMaterials(handles));
}

pub fn spawn_stroke_node(
    commands: &mut Commands,
    tab_id: u64,
    visible: bool,
    node_id: Option<NodeId>,
    stroke: StrokeNode,
    pose: Pose,
    z: f32,
    link: Option<String>,
) -> Entity {
    let entity = commands
        .spawn((
            CanvasNode {
                node_id,
                kind: super::node::NodeKind::Stroke(stroke),
                size: pose.size,
                rotation: pose.rot,
                link,
            },
            // Invisible hit target. Picking uses the polyline, not this quad;
            // the sprite exists so the node participates in the shared query.
            Sprite::from_color(Color::NONE, pose.size),
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
    if !visible {
        commands.entity(entity).insert(super::node::InactiveTab);
    }
    entity
}

#[derive(Component)]
struct InkDraws(bool);

fn sync_ink(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    materials: Option<Res<InkMaterials>>,
    show: Res<ShowStrokes>,
    nodes: Query<(Entity, &CanvasNode, &Sprite)>,
    mut inks: Query<(
        Entity,
        &mut InkMesh,
        &Mesh2d,
        &mut Transform,
        &mut Visibility,
        &mut MeshMaterial2d<ColorMaterial>,
        &mut InkDraws,
    )>,
) {
    let Some(materials) = materials else {
        return;
    };
    let mut by_owner: HashMap<Entity, Vec<(u32, Entity)>> = HashMap::new();
    for (entity, ink, _, _, _, _, _) in inks.iter() {
        by_owner
            .entry(ink.owner)
            .or_default()
            .push((ink.index, entity));
    }

    for (owner, node, sprite) in &nodes {
        let count = stroke_count(node);
        let have = by_owner.remove(&owner).unwrap_or_default();
        for index in 0..count {
            let stamp = stamp_of(node, sprite, index);
            let scale = scale_of(node, index);
            let color = color_of(node, index);
            let allow = stroke_allowed(node, show.0);
            if let Some((_, entity)) = have.iter().find(|(i, _)| *i == index as u32) {
                let Ok((_, mut ink, mesh, mut tf, mut vis, mut material, mut draws)) =
                    inks.get_mut(*entity)
                else {
                    continue;
                };
                if ink.scale != scale {
                    ink.scale = scale;
                    tf.scale = Vec3::new(scale.x, scale.y, 1.0);
                }
                if ink.stamp != stamp {
                    ink.stamp = stamp;
                    let (positions, indices) = tessellate(node, sprite, index);
                    draws.0 = !indices.is_empty();
                    if let Some(mut existing) = meshes.get_mut(&mesh.0) {
                        *existing = triangle_mesh(&positions, &indices);
                    }
                    *material =
                        MeshMaterial2d(materials.0[palette::clamp_index(color) as usize].clone());
                }
                let want = if allow && draws.0 {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                };
                if *vis != want {
                    *vis = want;
                }
            } else {
                let (positions, indices) = tessellate(node, sprite, index);
                let draws = !indices.is_empty();
                let mesh = meshes.add(triangle_mesh(&positions, &indices));
                let handle = materials.0[palette::clamp_index(color) as usize].clone();
                commands.spawn((
                    InkMesh {
                        owner,
                        index: index as u32,
                        stamp,
                        scale,
                    },
                    InkDraws(draws),
                    Mesh2d(mesh),
                    MeshMaterial2d(handle),
                    Transform {
                        translation: Vec3::new(0.0, 0.0, 0.2),
                        scale: Vec3::new(scale.x, scale.y, 1.0),
                        ..default()
                    },
                    ChildOf(owner),
                    if allow && draws {
                        Visibility::Inherited
                    } else {
                        Visibility::Hidden
                    },
                ));
            }
        }
        for (index, entity) in have {
            if (index as usize) >= count {
                commands.entity(entity).despawn();
            }
        }
    }
    for (_, leftover) in by_owner {
        for (_, entity) in leftover {
            commands.entity(entity).despawn();
        }
    }
}

fn stroke_count(node: &CanvasNode) -> usize {
    match &node.kind {
        super::node::NodeKind::Image(img) => img.data.strokes.len(),
        super::node::NodeKind::Stroke(_) => 1,
        _ => 0,
    }
}

fn stroke_allowed(node: &CanvasNode, show_all: bool) -> bool {
    if !show_all {
        return false;
    }
    match &node.kind {
        super::node::NodeKind::Image(img) => !img.data.strokes_hidden,
        super::node::NodeKind::Stroke(_) => true,
        _ => false,
    }
}

fn color_of(node: &CanvasNode, index: usize) -> u8 {
    match &node.kind {
        super::node::NodeKind::Image(img) => {
            img.data.strokes.get(index).map(|s| s.color).unwrap_or(15)
        }
        super::node::NodeKind::Stroke(stroke) => stroke.color,
        _ => 15,
    }
}

fn scale_of(node: &CanvasNode, _index: usize) -> Vec2 {
    match &node.kind {
        super::node::NodeKind::Image(img) => Vec2::splat(img.data.px_scale.max(1e-6)),
        super::node::NodeKind::Stroke(stroke) => Vec2::new(
            node.size.x / stroke.space.x.max(1e-6),
            node.size.y / stroke.space.y.max(1e-6),
        ),
        _ => Vec2::ONE,
    }
}

fn stamp_of(node: &CanvasNode, sprite: &Sprite, index: usize) -> u64 {
    match &node.kind {
        super::node::NodeKind::Image(img) => {
            let Some(stroke) = img.data.strokes.get(index) else {
                return 0;
            };
            let mut stamp = fnv(&stroke.width.to_le_bytes());
            stamp = fnv_byte(stamp, stroke.color);
            stamp = fnv_byte(stamp, u8::from(sprite.flip_x));
            stamp = fnv_byte(stamp, u8::from(sprite.flip_y));
            let crop = img.data.crop;
            stamp = fnv_f32(stamp, crop.min.x);
            stamp = fnv_f32(stamp, crop.min.y);
            stamp = fnv_f32(stamp, crop.max.x);
            stamp = fnv_f32(stamp, crop.max.y);
            for p in &stroke.points {
                stamp = fnv_f32(stamp, p.x);
                stamp = fnv_f32(stamp, p.y);
            }
            stamp
        }
        super::node::NodeKind::Stroke(stroke) => {
            let mut stamp = fnv(&stroke.width.to_le_bytes());
            stamp = fnv_byte(stamp, stroke.color);
            for p in &stroke.points {
                stamp = fnv_f32(stamp, p.x);
                stamp = fnv_f32(stamp, p.y);
            }
            stamp
        }
        _ => 0,
    }
}

fn tessellate(node: &CanvasNode, sprite: &Sprite, index: usize) -> (Vec<[f32; 3]>, Vec<u32>) {
    match &node.kind {
        super::node::NodeKind::Image(img) => {
            let Some(stroke) = img.data.strokes.get(index) else {
                return dummy();
            };
            let space = ImageSpace {
                center: Vec2::ZERO,
                rot: 0.0,
                px_scale: img.data.px_scale,
                crop: img.data.crop,
                flip_x: sprite.flip_x,
                flip_y: sprite.flip_y,
            };
            let parts = stroke::clip_polyline(&stroke.points, space.crop);
            let mut positions = Vec::new();
            let mut indices = Vec::new();
            for part in parts {
                let local: Vec<Vec2> = part.into_iter().map(|p| space.texel_to_mesh(p)).collect();
                let (pos, idx) = stroke::stroke_triangles(&local, stroke.width.max(0.05));
                let base = positions.len() as u32;
                positions.extend(pos);
                indices.extend(idx.into_iter().map(|i| i + base));
            }
            if indices.is_empty() {
                dummy()
            } else {
                (positions, indices)
            }
        }
        super::node::NodeKind::Stroke(stroke) => {
            let (positions, indices) =
                stroke::stroke_triangles(&stroke.points, stroke.width.max(0.05));
            if indices.is_empty() {
                dummy()
            } else {
                (positions, indices)
            }
        }
        _ => dummy(),
    }
}

fn dummy() -> (Vec<[f32; 3]>, Vec<u32>) {
    (vec![[0.0, 0.0, 0.0]; 3], Vec::new())
}

fn triangle_mesh(positions: &[[f32; 3]], indices: &[u32]) -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    let n = positions.len();
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions.to_vec());
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 0.0, 1.0]; n]);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0, 0.0]; n]);
    mesh.insert_indices(Indices::U32(indices.to_vec()));
    mesh
}

fn fnv(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn fnv_byte(mut h: u64, b: u8) -> u64 {
    h ^= b as u64;
    h.wrapping_mul(0x100000001b3)
}

fn fnv_f32(h: u64, v: f32) -> u64 {
    fnv_continue(h, &v.to_le_bytes())
}

fn fnv_continue(mut h: u64, bytes: &[u8]) -> u64 {
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}
