//! Ray/panel maths for laser-free pointer control.
//!
//! The wrist panel is a tracked-device-relative overlay. We cast a ray from the
//! opposite controller and intersect it with the panel plane, so the cursor
//! follows the hand without reading the thumbstick (which the game also reads).

/// Row-major 3x4 affine transform (same layout as `openvr::pose::Matrix3x4`).
pub(super) type Mat3x4 = [[f32; 4]; 3];

pub(super) fn translation(m: &Mat3x4) -> [f32; 3] {
    [m[0][3], m[1][3], m[2][3]]
}

fn axis(m: &Mat3x4, column: usize) -> [f32; 3] {
    [m[0][column], m[1][column], m[2][column]]
}

fn normalize(v: [f32; 3]) -> Option<[f32; 3]> {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len < 1e-6 {
        return None;
    }
    Some([v[0] / len, v[1] / len, v[2] / len])
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(v: [f32; 3], k: f32) -> [f32; 3] {
    [v[0] * k, v[1] * k, v[2] * k]
}

/// `parent * child` for two affine 3x4 transforms.
pub(super) fn compose(parent: &Mat3x4, child: &Mat3x4) -> Mat3x4 {
    let mut out = [[0.0f32; 4]; 3];
    for row in 0..3 {
        for col in 0..4 {
            let mut value = 0.0f32;
            for k in 0..3 {
                value += parent[row][k] * child[k][col];
            }
            if col == 3 {
                value += parent[row][3];
            }
            out[row][col] = value;
        }
    }
    out
}

/// Intersect the controller ray with the panel plane.
///
/// Returns the hit position in normalized panel coordinates: `(0, 0)` is the
/// top-left corner and `(1, 1)` the bottom-right one. Values outside `0..1`
/// mean the ray misses the panel.
pub(super) fn ray_to_panel(
    controller: &Mat3x4,
    panel: &Mat3x4,
    width_meters: f32,
    height_meters: f32,
) -> Option<(f32, f32)> {
    let origin = translation(controller);
    // OpenVR controllers point along their local -Z axis.
    let forward = normalize(scale(axis(controller, 2), -1.0))?;
    let center = translation(panel);
    let normal = normalize(axis(panel, 2))?;
    let right = normalize(axis(panel, 0))?;
    let up = normalize(axis(panel, 1))?;

    let denom = dot(forward, normal);
    if denom.abs() < 1e-4 {
        return None;
    }
    let t = dot(sub(center, origin), normal) / denom;
    if t <= 0.02 {
        return None;
    }
    let hit = add(origin, scale(forward, t));
    let local = sub(hit, center);
    let half_w = (width_meters * 0.5).max(1e-4);
    let half_h = (height_meters * 0.5).max(1e-4);
    let u = dot(local, right) / half_w;
    let v = dot(local, up) / half_h;
    // u/v are -1..1 across the panel; map to 0..1 with y pointing down.
    Some(((u + 1.0) * 0.5, (1.0 - v) * 0.5))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> Mat3x4 {
        [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
        ]
    }

    fn controller_at(x: f32, y: f32, z: f32) -> Mat3x4 {
        [[1.0, 0.0, 0.0, x], [0.0, 1.0, 0.0, y], [0.0, 0.0, 1.0, z]]
    }

    #[test]
    fn ray_through_center_hits_panel_center() {
        // Controller one metre in front of the panel, looking along -Z.
        let controller = controller_at(0.0, 0.0, 1.0);
        let panel = identity();
        let (u, v) = ray_to_panel(&controller, &panel, 0.2, 0.2).expect("hit");
        assert!((u - 0.5).abs() < 1e-3, "{u}");
        assert!((v - 0.5).abs() < 1e-3, "{v}");
    }

    #[test]
    fn ray_offset_maps_to_quadrant() {
        let controller = controller_at(0.05, 0.05, 1.0);
        let panel = identity();
        let (u, v) = ray_to_panel(&controller, &panel, 0.2, 0.2).expect("hit");
        assert!(u > 0.5, "expected right half, got {u}");
        assert!(v < 0.5, "expected upper half, got {v}");
    }

    #[test]
    fn ray_away_from_panel_misses() {
        let controller = controller_at(0.0, 0.0, 1.0);
        let mut behind = controller_at(0.0, 0.0, 1.0);
        behind[2][2] = -1.0; // looking away (+Z)
        assert!(ray_to_panel(&behind, &identity(), 0.2, 0.2).is_none());
        // Sanity: the forward one still hits.
        assert!(ray_to_panel(&controller, &identity(), 0.2, 0.2).is_some());
    }

    #[test]
    fn compose_applies_child_then_parent() {
        let parent = controller_at(0.0, 0.0, 2.0);
        let child = controller_at(0.0, 0.0, -0.5);
        let world = compose(&parent, &child);
        assert!((world[2][3] - 1.5).abs() < 1e-5, "{:?}", world[2][3]);
    }
}
