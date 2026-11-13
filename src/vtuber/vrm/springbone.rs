use glam::{Mat4, Quat, Vec3};

#[derive(Clone, Copy, Debug)]
pub enum SpringColliderShape {
    Sphere {
        offset: Vec3,
        radius: f32,
    },
    Capsule {
        offset: Vec3,
        tail: Vec3,
        radius: f32,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct SpringColliderDef {
    pub node_idx: usize,
    pub shape: SpringColliderShape,
}

#[derive(Clone, Copy, Debug)]
struct WorldCollider {
    p0: Vec3,
    p1: Vec3,
    radius: f32,
    is_capsule: bool,
}

#[derive(Clone, Debug)]
pub struct SpringJointLink {
    pub node_idx: usize,
    pub parent_node_idx: Option<usize>,
    pub local_rest_pos: Vec3,
    pub local_rest_rot: Quat,
    pub local_rest_scale: Vec3,
    pub local_bone_axis: Vec3,
    pub bone_length: f32,
    pub stiffness: f32,
    pub gravity_power: f32,
    pub gravity_dir: Vec3,
    pub drag_force: f32,
    pub hit_radius: f32,
    pub collider_indices: Vec<usize>,
}

#[derive(Clone, Copy, Debug)]
struct JointRuntimeState {
    prev_tail: Vec3,
    current_tail: Vec3,
}

pub struct SpringBoneWorld {
    pub colliders: Vec<SpringColliderDef>,
    pub links: Vec<SpringJointLink>,
    states: Vec<JointRuntimeState>,
    world_colliders_scratch: Vec<WorldCollider>,
    initialized: bool,
    accum_dt: f32,
}

impl SpringBoneWorld {
    pub fn new(colliders: Vec<SpringColliderDef>, links: Vec<SpringJointLink>) -> Self {
        let n = links.len();
        let c = colliders.len();
        Self {
            colliders,
            links,
            states: vec![
                JointRuntimeState {
                    prev_tail: Vec3::ZERO,
                    current_tail: Vec3::ZERO,
                };
                n
            ],
            world_colliders_scratch: Vec::with_capacity(c),
            initialized: false,
            accum_dt: 0.0,
        }
    }

    /// Steps Verlet secondary physics and updates `local_rotations` and `world_matrices` in-place.
    /// `root_transform` is the model root transform (e.g. `R_y(PI)` for VRM 0.x, identity for VRM 1.0).
    pub fn step(
        &mut self,
        dt: f32,
        root_transform: Mat4,
        local_rotations: &mut [Quat],
        world_matrices: &mut [Mat4],
    ) {
        if self.links.is_empty() {
            return;
        }

        if !self.initialized {
            for (i, link) in self.links.iter().enumerate() {
                let node_world = world_matrices
                    .get(link.node_idx)
                    .copied()
                    .unwrap_or(root_transform);
                let tail = node_world.transform_point3(link.local_bone_axis * link.bone_length);
                self.states[i] = JointRuntimeState {
                    prev_tail: tail,
                    current_tail: tail,
                };
            }
            self.initialized = true;
            return;
        }

        const FIXED_STEP: f32 = 1.0 / 90.0;
        self.accum_dt = (self.accum_dt + dt.clamp(0.0, 0.08)).min(FIXED_STEP * 4.0);
        let mut steps = 0;
        while self.accum_dt >= FIXED_STEP && steps < 4 {
            self.accum_dt -= FIXED_STEP;
            self.substep(FIXED_STEP, root_transform, local_rotations, world_matrices);
            steps += 1;
        }
        // If monitor runs faster than 90Hz and no substep fired this tick, still re-project rotations
        // from `current_tail` against the freshly animated parent bones so hair stays glued to the head.
        if steps == 0 {
            self.apply_current_tails(root_transform, local_rotations, world_matrices);
        }
    }

    fn refresh_world_colliders(&mut self, world_matrices: &[Mat4]) {
        self.world_colliders_scratch.clear();
        for def in &self.colliders {
            let m = world_matrices
                .get(def.node_idx)
                .copied()
                .unwrap_or(Mat4::IDENTITY);
            let (_, _, _) = m.to_scale_rotation_translation();
            match def.shape {
                SpringColliderShape::Sphere { offset, radius } => {
                    let p0 = m.transform_point3(offset);
                    self.world_colliders_scratch.push(WorldCollider {
                        p0,
                        p1: p0,
                        radius,
                        is_capsule: false,
                    });
                }
                SpringColliderShape::Capsule {
                    offset,
                    tail,
                    radius,
                } => {
                    let p0 = m.transform_point3(offset);
                    let p1 = m.transform_point3(tail);
                    self.world_colliders_scratch.push(WorldCollider {
                        p0,
                        p1,
                        radius,
                        is_capsule: true,
                    });
                }
            }
        }
    }

    fn substep(
        &mut self,
        dt: f32,
        root_transform: Mat4,
        local_rotations: &mut [Quat],
        world_matrices: &mut [Mat4],
    ) {
        self.refresh_world_colliders(world_matrices);

        for (i, link) in self.links.iter().enumerate() {
            let parent_world = link
                .parent_node_idx
                .and_then(|p| world_matrices.get(p).copied())
                .unwrap_or(root_transform);
            let (_, parent_rot, _) = parent_world.to_scale_rotation_translation();
            let head_pos = parent_world.transform_point3(link.local_rest_pos);

            // Prevent extreme teleports if model was reset or moved sharply
            let max_stretch = (link.bone_length * 3.5).max(0.25);
            if (self.states[i].current_tail - head_pos).length_squared() > max_stretch * max_stretch
            {
                let reset_tail = head_pos
                    + (parent_rot * link.local_rest_rot * link.local_bone_axis) * link.bone_length;
                self.states[i].current_tail = reset_tail;
                self.states[i].prev_tail = reset_tail;
            }

            let state = &mut self.states[i];
            let inertia =
                (state.current_tail - state.prev_tail) * (1.0 - link.drag_force.clamp(0.0, 1.0));
            let rest_world_rot = parent_rot * link.local_rest_rot;
            let stiffness_vec = (rest_world_rot * link.local_bone_axis) * (link.stiffness * dt);
            let gravity_vec = link.gravity_dir * (link.gravity_power * dt);

            let mut next_tail = state.current_tail + inertia + stiffness_vec + gravity_vec;
            let dir = (next_tail - head_pos).normalize_or_zero();
            if dir.length_squared() > 1e-8 {
                next_tail = head_pos + dir * link.bone_length;
            } else {
                next_tail = head_pos + (rest_world_rot * link.local_bone_axis) * link.bone_length;
            }

            // Collider resolution (spheres & capsules)
            for &col_idx in &link.collider_indices {
                let Some(col) = self.world_colliders_scratch.get(col_idx) else {
                    continue;
                };
                let closest = if col.is_capsule {
                    closest_point_on_segment(col.p0, col.p1, next_tail)
                } else {
                    col.p0
                };
                let r_sum = link.hit_radius + col.radius;
                let diff = next_tail - closest;
                let dist_sq = diff.length_squared();
                if dist_sq < r_sum * r_sum {
                    let push_dir = if dist_sq > 1e-10 {
                        diff / dist_sq.sqrt()
                    } else {
                        Vec3::Z
                    };
                    next_tail = closest + push_dir * r_sum;
                    let clamped_dir = (next_tail - head_pos).normalize_or_zero();
                    if clamped_dir.length_squared() > 1e-8 {
                        next_tail = head_pos + clamped_dir * link.bone_length;
                    }
                }
            }

            state.prev_tail = state.current_tail;
            state.current_tail = next_tail;

            let new_dir_world = (next_tail - head_pos).normalize_or_zero();
            let rest_dir_world = (rest_world_rot * link.local_bone_axis).normalize_or_zero();
            if new_dir_world.length_squared() > 1e-8 && rest_dir_world.length_squared() > 1e-8 {
                let delta_world = Quat::from_rotation_arc(rest_dir_world, new_dir_world);
                let new_local_rot =
                    (parent_rot.inverse() * delta_world * rest_world_rot).normalize();
                if let Some(slot) = local_rotations.get_mut(link.node_idx) {
                    *slot = new_local_rot;
                }
                if let Some(wm) = world_matrices.get_mut(link.node_idx) {
                    *wm = parent_world
                        * Mat4::from_scale_rotation_translation(
                            link.local_rest_scale,
                            new_local_rot,
                            link.local_rest_pos,
                        );
                }
            }
        }
    }

    fn apply_current_tails(
        &self,
        root_transform: Mat4,
        local_rotations: &mut [Quat],
        world_matrices: &mut [Mat4],
    ) {
        for (i, link) in self.links.iter().enumerate() {
            let parent_world = link
                .parent_node_idx
                .and_then(|p| world_matrices.get(p).copied())
                .unwrap_or(root_transform);
            let (_, parent_rot, _) = parent_world.to_scale_rotation_translation();
            let head_pos = parent_world.transform_point3(link.local_rest_pos);
            let rest_world_rot = parent_rot * link.local_rest_rot;
            let new_dir_world = (self.states[i].current_tail - head_pos).normalize_or_zero();
            let rest_dir_world = (rest_world_rot * link.local_bone_axis).normalize_or_zero();
            if new_dir_world.length_squared() > 1e-8 && rest_dir_world.length_squared() > 1e-8 {
                let delta_world = Quat::from_rotation_arc(rest_dir_world, new_dir_world);
                let new_local_rot =
                    (parent_rot.inverse() * delta_world * rest_world_rot).normalize();
                if let Some(slot) = local_rotations.get_mut(link.node_idx) {
                    *slot = new_local_rot;
                }
                if let Some(wm) = world_matrices.get_mut(link.node_idx) {
                    *wm = parent_world
                        * Mat4::from_scale_rotation_translation(
                            link.local_rest_scale,
                            new_local_rot,
                            link.local_rest_pos,
                        );
                }
            }
        }
    }
}

fn closest_point_on_segment(a: Vec3, b: Vec3, p: Vec3) -> Vec3 {
    let ab = b - a;
    let len_sq = ab.length_squared();
    if len_sq <= 1e-10 {
        return a;
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    a + ab * t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_springbone_preserves_length_and_pushes_out_of_sphere_collider() {
        let colliders = vec![SpringColliderDef {
            node_idx: 0,
            shape: SpringColliderShape::Sphere {
                offset: Vec3::new(0.0, -0.10, 0.0),
                radius: 0.05,
            },
        }];
        let links = vec![SpringJointLink {
            node_idx: 1,
            parent_node_idx: Some(0),
            local_rest_pos: Vec3::ZERO,
            local_rest_rot: Quat::IDENTITY,
            local_rest_scale: Vec3::ONE,
            local_bone_axis: Vec3::NEG_Y,
            bone_length: 0.10,
            stiffness: 0.5,
            gravity_power: 0.2,
            gravity_dir: Vec3::NEG_Y,
            drag_force: 0.4,
            hit_radius: 0.02,
            collider_indices: vec![0],
        }];

        let mut world = SpringBoneWorld::new(colliders, links);
        let mut local_rots = vec![Quat::IDENTITY, Quat::IDENTITY];
        let mut world_mats = vec![Mat4::IDENTITY, Mat4::IDENTITY];

        // Frame 0 initializes tail
        world.step(1.0 / 60.0, Mat4::IDENTITY, &mut local_rots, &mut world_mats);
        // Step several frames; sphere at (0, -0.10, 0) must push tail away while keeping length = 0.10
        for _ in 0..10 {
            world.step(1.0 / 60.0, Mat4::IDENTITY, &mut local_rots, &mut world_mats);
        }

        let tail = world.states[0].current_tail;
        assert!((tail.length() - 0.10).abs() < 1e-4);
        let dist_to_sphere = (tail - Vec3::new(0.0, -0.10, 0.0)).length();
        assert!(
            dist_to_sphere >= 0.069,
            "tail must be pushed outside collider + hit_radius"
        );
    }
}
