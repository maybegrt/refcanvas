//! Vector stroke geometry: smoothing, simplification, image-local space, and
//! the triangle ribbon used to draw a stroke.
//!
//! Attached strokes are stored in full-image texel space (origin top-left,
//! y down, the same space as a crop rect) so they survive move, scale,
//! rotation, flip and crop without rewriting the points. Free-standing
//! strokes are stored relative to their own center, in a fixed "space" that
//! the node's current size scales at draw time.

use bevy::prelude::*;

use super::node::{ImageStroke, StrokeNode};

/// How an image is placed. Texel coordinates are y-down, origin at the
/// top-left of the uncropped image.
#[derive(Clone, Copy, Debug)]
pub struct ImageSpace {
    pub center: Vec2,
    pub rot: f32,
    pub px_scale: f32,
    pub crop: Rect,
    pub flip_x: bool,
    pub flip_y: bool,
}

impl ImageSpace {
    /// World point → full-image texel (y down).
    pub fn world_to_texel(self, world: Vec2) -> Vec2 {
        let local = Rot2::radians(-self.rot) * (world - self.center);
        let mut lx = local.x;
        let mut ly = local.y;
        if self.flip_x {
            lx = -lx;
        }
        if self.flip_y {
            ly = -ly;
        }
        let px = self.px_scale.max(1e-8);
        Vec2::new(
            self.crop.center().x + lx / px,
            self.crop.center().y - ly / px,
        )
    }

    /// Full-image texel (y down) → world.
    pub fn texel_to_world(self, texel: Vec2) -> Vec2 {
        let px = self.px_scale.max(1e-8);
        let mut lx = (texel.x - self.crop.center().x) * px;
        let mut ly = -(texel.y - self.crop.center().y) * px;
        if self.flip_x {
            lx = -lx;
        }
        if self.flip_y {
            ly = -ly;
        }
        self.center + Rot2::radians(self.rot) * Vec2::new(lx, ly)
    }

    /// Texel point → mesh space: y-up, origin at the crop center, flip baked
    /// in, still in texel units (the mesh entity is scaled by `px_scale`).
    pub fn texel_to_mesh(self, texel: Vec2) -> Vec2 {
        let mut x = texel.x - self.crop.center().x;
        let mut y = -(texel.y - self.crop.center().y);
        if self.flip_x {
            x = -x;
        }
        if self.flip_y {
            y = -y;
        }
        Vec2::new(x, y)
    }
}

/// Free-stroke local point (y up, origin at the node center, in `space`
/// units) → world. `size` is the node's current size, so scaling the node
/// scales the stroke without rewriting points.
pub fn free_local_to_world(local: Vec2, center: Vec2, rot: f32, size: Vec2, space: Vec2) -> Vec2 {
    let scaled = Vec2::new(
        local.x * size.x / space.x.max(1e-6),
        local.y * size.y / space.y.max(1e-6),
    );
    center + Rot2::radians(rot) * scaled
}

/// Inverse of [`free_local_to_world`]. The round-trip tests use it; the
/// canvas itself only needs the forward mapping.
#[cfg_attr(not(test), allow(dead_code))]
pub fn world_to_free_local(world: Vec2, center: Vec2, rot: f32, size: Vec2, space: Vec2) -> Vec2 {
    let local = Rot2::radians(-rot) * (world - center);
    Vec2::new(
        local.x * space.x / size.x.max(1e-6),
        local.y * space.y / size.y.max(1e-6),
    )
}

/// Chaikin corner cutting. Endpoints stay put so the stroke doesn't shrink
/// away from where the pointer went down and came up. Fewer than three
/// points is already a line (or a dot) and is returned unchanged.
pub fn chaikin(points: &[Vec2], iterations: usize) -> Vec<Vec2> {
    if points.len() < 3 || iterations == 0 {
        return points.to_vec();
    }
    let mut pts = points.to_vec();
    for _ in 0..iterations {
        let mut next = Vec::with_capacity(pts.len() * 2);
        next.push(pts[0]);
        for pair in pts.windows(2) {
            let p = pair[0];
            let q = pair[1];
            next.push(p * 0.75 + q * 0.25);
            next.push(p * 0.25 + q * 0.75);
        }
        next.push(*pts.last().unwrap());
        pts = next;
    }
    pts
}

/// Ramer–Douglas–Peucker. Endpoints are kept.
pub fn simplify(points: &[Vec2], epsilon: f32) -> Vec<Vec2> {
    if points.len() < 3 || epsilon <= 0.0 {
        return points.to_vec();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    simplify_range(points, epsilon, &mut keep, 0, points.len() - 1);
    points
        .iter()
        .enumerate()
        .filter(|(i, _)| keep[*i])
        .map(|(_, p)| *p)
        .collect()
}

fn simplify_range(points: &[Vec2], epsilon: f32, keep: &mut [bool], first: usize, last: usize) {
    if last <= first + 1 {
        return;
    }
    let (mut far, mut far_d) = (first, 0.0f32);
    for i in first + 1..last {
        let d = dist_to_segment(points[i], points[first], points[last]);
        if d > far_d {
            far_d = d;
            far = i;
        }
    }
    if far_d > epsilon {
        keep[far] = true;
        simplify_range(points, epsilon, keep, first, far);
        simplify_range(points, epsilon, keep, far, last);
    }
}

fn dist_to_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_squared();
    if len2 <= 1e-12 {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Drop samples that sit on top of the previous one, then smooth and simplify.
pub fn smooth_stroke(raw: &[Vec2], epsilon: f32) -> Vec<Vec2> {
    let mut points = Vec::with_capacity(raw.len());
    for p in raw {
        if points
            .last()
            .is_none_or(|prev: &Vec2| prev.distance(*p) > 1e-3)
        {
            points.push(*p);
        }
    }
    if points.len() < 2 {
        return points;
    }
    let smoothed = chaikin(&points, 2);
    let mut simple = simplify(&smoothed, epsilon.max(0.0));
    if simple.len() > 2000 {
        simple = simplify(&simple, epsilon.max(0.05) * 2.0);
        simple.truncate(2000);
    }
    simple
}

/// Clip a polyline to an axis-aligned rect. Pieces that leave and re-enter
/// come back as separate polylines. A single point inside the rect is kept
/// so a dot still draws.
pub fn clip_polyline(points: &[Vec2], rect: Rect) -> Vec<Vec<Vec2>> {
    fn inside(p: Vec2, rect: Rect) -> bool {
        p.x >= rect.min.x && p.x <= rect.max.x && p.y >= rect.min.y && p.y <= rect.max.y
    }

    if points.is_empty() {
        return Vec::new();
    }
    if points.len() == 1 {
        return if inside(points[0], rect) {
            vec![vec![points[0]]]
        } else {
            Vec::new()
        };
    }

    let mut out = Vec::new();
    let mut cur: Vec<Vec2> = Vec::new();
    for pair in points.windows(2) {
        if let Some((a, b)) = clip_segment(pair[0], pair[1], rect) {
            let fresh = cur.last().is_none_or(|p| p.distance(a) > 1e-3);
            if fresh {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                cur.push(a);
            }
            cur.push(b);
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Cohen–Sutherland. Returns the clipped segment when any of it is visible.
fn clip_segment(mut a: Vec2, mut b: Vec2, rect: Rect) -> Option<(Vec2, Vec2)> {
    let code = |p: Vec2| -> u8 {
        let mut c = 0u8;
        if p.x < rect.min.x {
            c |= 1;
        } else if p.x > rect.max.x {
            c |= 2;
        }
        if p.y < rect.min.y {
            c |= 4;
        } else if p.y > rect.max.y {
            c |= 8;
        }
        c
    };
    let mut ca = code(a);
    let mut cb = code(b);
    for _ in 0..8 {
        if ca == 0 && cb == 0 {
            return Some((a, b));
        }
        if ca & cb != 0 {
            return None;
        }
        let c = if ca != 0 { ca } else { cb };
        let (x, y) = if c & 1 != 0 {
            (
                rect.min.x,
                a.y + (b.y - a.y) * (rect.min.x - a.x) / (b.x - a.x),
            )
        } else if c & 2 != 0 {
            (
                rect.max.x,
                a.y + (b.y - a.y) * (rect.max.x - a.x) / (b.x - a.x),
            )
        } else if c & 4 != 0 {
            (
                a.x + (b.x - a.x) * (rect.min.y - a.y) / (b.y - a.y),
                rect.min.y,
            )
        } else {
            (
                a.x + (b.x - a.x) * (rect.max.y - a.y) / (b.y - a.y),
                rect.max.y,
            )
        };
        let p = Vec2::new(x, y);
        if c == ca {
            a = p;
            ca = code(a);
        } else {
            b = p;
            cb = code(b);
        }
    }
    None
}

/// Triangle list for a uniform-width stroke in the same space as `points`.
/// One point (or a run of identical points) becomes a disc, so a click still
/// leaves a mark. Returns `(positions, indices)`.
pub fn stroke_triangles(points: &[Vec2], width: f32) -> (Vec<[f32; 3]>, Vec<u32>) {
    let half = (width * 0.5).max(0.05);
    let distinct = points
        .iter()
        .copied()
        .fold(Vec::<Vec2>::new(), |mut acc, p| {
            if acc.last().is_none_or(|prev| prev.distance(p) > 1e-4) {
                acc.push(p);
            }
            acc
        });
    if distinct.is_empty() {
        return (Vec::new(), Vec::new());
    }
    if distinct.len() == 1 {
        return disc(distinct[0], half, 10);
    }

    let mut pos = Vec::new();
    let mut idx = Vec::new();
    for pair in distinct.windows(2) {
        push_quad(&mut pos, &mut idx, pair[0], pair[1], half);
    }
    // Discs fill the gaps the quads leave on the outside of a bend, and
    // round off the ends. A smoothed stroke has few sharp corners, so this
    // stays cheap.
    for p in &distinct {
        let (dpos, didx) = disc(*p, half, 8);
        let base = pos.len() as u32;
        pos.extend(dpos);
        idx.extend(didx.into_iter().map(|i| i + base));
    }
    (pos, idx)
}

fn push_quad(pos: &mut Vec<[f32; 3]>, idx: &mut Vec<u32>, a: Vec2, b: Vec2, half: f32) {
    let d = b - a;
    let len = d.length();
    if len <= 1e-5 {
        return;
    }
    let n = Vec2::new(-d.y, d.x) / len * half;
    let base = pos.len() as u32;
    pos.push([a.x + n.x, a.y + n.y, 0.0]);
    pos.push([a.x - n.x, a.y - n.y, 0.0]);
    pos.push([b.x + n.x, b.y + n.y, 0.0]);
    pos.push([b.x - n.x, b.y - n.y, 0.0]);
    idx.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 1, base + 3]);
}

fn disc(center: Vec2, radius: f32, segments: u32) -> (Vec<[f32; 3]>, Vec<u32>) {
    let segments = segments.max(3);
    let mut pos = Vec::with_capacity(segments as usize + 1);
    let mut idx = Vec::with_capacity(segments as usize * 3);
    pos.push([center.x, center.y, 0.0]);
    for i in 0..segments {
        let a = i as f32 / segments as f32 * std::f32::consts::TAU;
        pos.push([
            center.x + a.cos() * radius,
            center.y + a.sin() * radius,
            0.0,
        ]);
    }
    for i in 0..segments {
        let next = if i + 1 == segments { 1 } else { i + 2 };
        idx.extend_from_slice(&[0, i + 1, next]);
    }
    (pos, idx)
}

pub fn hit_free_stroke(
    node: &StrokeNode,
    center: Vec2,
    rot: f32,
    size: Vec2,
    cursor: Vec2,
    extra: f32,
) -> bool {
    if node.points.is_empty() {
        return false;
    }
    let sx = size.x / node.space.x.max(1e-6);
    let sy = size.y / node.space.y.max(1e-6);
    let reach = node.width * sx.max(sy) * 0.5 + extra;
    if node.points.len() == 1 {
        let p = free_local_to_world(node.points[0], center, rot, size, node.space);
        return cursor.distance(p) <= reach;
    }
    node.points.windows(2).any(|pair| {
        let a = free_local_to_world(pair[0], center, rot, size, node.space);
        let b = free_local_to_world(pair[1], center, rot, size, node.space);
        dist_to_segment(cursor, a, b) <= reach
    })
}

pub fn hit_image_stroke(stroke: &ImageStroke, space: ImageSpace, cursor: Vec2, extra: f32) -> bool {
    if stroke.points.is_empty() {
        return false;
    }
    let reach = stroke.width * space.px_scale.max(1e-8) * 0.5 + extra;
    if stroke.points.len() == 1 {
        let p = space.texel_to_world(stroke.points[0]);
        return cursor.distance(p) <= reach;
    }
    stroke.points.windows(2).any(|pair| {
        let a = space.texel_to_world(pair[0]);
        let b = space.texel_to_world(pair[1]);
        dist_to_segment(cursor, a, b) <= reach
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn space_at(
        center: Vec2,
        rot: f32,
        scale: f32,
        flip_x: bool,
        flip_y: bool,
        crop: Rect,
    ) -> ImageSpace {
        ImageSpace {
            center,
            rot,
            px_scale: scale,
            crop,
            flip_x,
            flip_y,
        }
    }

    #[test]
    fn texel_round_trips_through_move_rotate_scale_flip_and_crop() {
        let crop = Rect::from_corners(Vec2::new(10.0, 5.0), Vec2::new(70.0, 60.0));
        let placed = space_at(
            Vec2::new(-40.0, 25.0),
            std::f32::consts::FRAC_PI_2,
            2.5,
            true,
            false,
            crop,
        );
        let texel = Vec2::new(33.0, 18.0);
        let world = placed.texel_to_world(texel);
        let back = placed.world_to_texel(world);
        assert!(
            (back - texel).length() < 1e-3,
            "back={back:?} texel={texel:?}"
        );

        // The same texel stays glued to the image when only the center moves.
        let moved = ImageSpace {
            center: placed.center + Vec2::new(80.0, -15.0),
            ..placed
        };
        let world_moved = moved.texel_to_world(texel);
        assert!((world_moved - world - Vec2::new(80.0, -15.0)).length() < 1e-3);
        assert!((moved.world_to_texel(world_moved) - texel).length() < 1e-3);
    }

    #[test]
    fn crop_changes_where_a_texel_sits_without_changing_its_identity() {
        let full = Rect::from_corners(Vec2::ZERO, Vec2::new(100.0, 80.0));
        let left = Rect::from_corners(Vec2::ZERO, Vec2::new(40.0, 80.0));
        let texel = Vec2::new(20.0, 40.0);
        let a = space_at(Vec2::ZERO, 0.0, 1.0, false, false, full);
        let b = space_at(Vec2::ZERO, 0.0, 1.0, false, false, left);
        // Crop center moved, so the same texel is no longer at the node center,
        // but it still converts back to itself.
        assert!(
            a.texel_to_world(texel).distance(Vec2::ZERO) > 1.0
                || b.texel_to_world(texel).distance(Vec2::ZERO) > 1.0
                || a.texel_to_world(texel).distance(b.texel_to_world(texel)) > 1.0
        );
        assert!((b.world_to_texel(b.texel_to_world(texel)) - texel).length() < 1e-3);
    }

    #[test]
    fn free_stroke_local_space_follows_scale_and_rotation() {
        let local = Vec2::new(4.0, -2.0);
        let space = Vec2::new(10.0, 8.0);
        let size = Vec2::new(30.0, 16.0);
        let center = Vec2::new(5.0, 6.0);
        let rot = 0.4;
        let world = free_local_to_world(local, center, rot, size, space);
        let back = world_to_free_local(world, center, rot, size, space);
        assert!((back - local).length() < 1e-3);
        // Doubling the node size doubles the offset from the center.
        let bigger = free_local_to_world(local, center, rot, size * 2.0, space);
        let offset = Rot2::radians(-rot) * (world - center);
        let offset_big = Rot2::radians(-rot) * (bigger - center);
        assert!((offset_big - offset * 2.0).length() < 1e-3);
    }

    #[test]
    fn smoothing_keeps_endpoints_and_drops_colinear_noise() {
        let mut raw = vec![Vec2::new(0.0, 0.0)];
        for i in 1..20 {
            let wobble = if i % 2 == 0 { 0.05 } else { -0.05 };
            raw.push(Vec2::new(i as f32, wobble));
        }
        let smooth = smooth_stroke(&raw, 0.2);
        assert!(smooth.len() >= 2);
        assert!(smooth.len() < raw.len());
        assert!(smooth[0].distance(raw[0]) < 1e-4);
        assert!(smooth.last().unwrap().distance(*raw.last().unwrap()) < 1e-4);
    }

    #[test]
    fn clip_splits_a_polyline_that_leaves_the_rect() {
        let rect = Rect::from_corners(Vec2::ZERO, Vec2::new(10.0, 10.0));
        let gap = [
            Vec2::new(1.0, 1.0),
            Vec2::new(2.0, 1.0),
            Vec2::new(20.0, 1.0),
            Vec2::new(30.0, 1.0),
            Vec2::new(8.0, 8.0),
            Vec2::new(9.0, 8.0),
        ];
        let parts = clip_polyline(&gap, rect);
        assert!(
            parts.len() >= 2,
            "expected the outside run to split the stroke, got {parts:?}"
        );
        assert!(parts.iter().all(|part| part.iter().all(|p| {
            p.x >= -1e-3 && p.x <= 10.0 + 1e-3 && p.y >= -1e-3 && p.y <= 10.0 + 1e-3
        })));
    }

    #[test]
    fn a_click_is_a_disc_and_a_line_has_triangles() {
        let (pos, idx) = stroke_triangles(&[Vec2::ZERO], 4.0);
        assert!(pos.len() > 3);
        assert_eq!(idx.len() % 3, 0);
        let (pos, idx) = stroke_triangles(
            &[Vec2::ZERO, Vec2::new(10.0, 0.0), Vec2::new(10.0, 8.0)],
            2.0,
        );
        assert!(idx.len() >= 6);
        assert!(pos.len() >= 4);
    }
}
