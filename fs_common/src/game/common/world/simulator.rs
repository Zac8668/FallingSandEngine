use std::sync::Arc;

use fastrand::Rng;
use rapier2d::na::Isometry2;

use crate::game::common::world::material::{MaterialInstance, PhysicsType};
use crate::game::common::world::{rigidbody, CHUNK_SIZE};
use crate::game::common::{Rect, Registries};

use super::material::color::Color;
use super::particle::Particle;
use super::rigidbody::FSRigidBody;
use super::{material, pixel_to_chunk_pos};
use super::{
    physics::{Physics, PHYSICS_SCALE},
    Chunk, ChunkHandler, ChunkHandlerGeneric, Position, Velocity,
};

pub struct Simulator {}

trait SimulationHelper {
    fn get_pixel_local(&self, x: i32, y: i32) -> MaterialInstance;
    fn borrow_pixel_local(&self, x: i32, y: i32) -> &MaterialInstance;
    fn set_pixel_local(&mut self, x: i32, y: i32, mat: MaterialInstance);
    fn get_color_local(&self, x: i32, y: i32) -> Color;
    fn set_color_local(&mut self, x: i32, y: i32, col: Color);
    fn get_light_local(&self, x: i32, y: i32) -> [f32; 3];
    fn set_light_local(&mut self, x: i32, y: i32, light: [f32; 3]);
    fn add_particle(&mut self, material: MaterialInstance, pos: Position, vel: Velocity);
}

struct SimulationHelperChunk<'a, 'b> {
    chunk_data: &'a mut [SimulatorChunkContext<'b>; 9],
    min_x: [u16; 9],
    min_y: [u16; 9],
    max_x: [u16; 9],
    max_y: [u16; 9],
    particles: &'a mut Vec<Particle>,
    chunk_x: i32,
    chunk_y: i32,
}

#[allow(unused)]
impl SimulationHelperChunk<'_, '_> {
    #[inline]
    fn get_pixel_from_index(&self, (ch, px, ..): (usize, usize, u16, u16)) -> MaterialInstance {
        self.chunk_data[ch].pixels[px].clone()
    }

    #[inline]
    fn borrow_pixel_from_index(&self, (ch, px, ..): (usize, usize, u16, u16)) -> &MaterialInstance {
        &self.chunk_data[ch].pixels[px]
    }

    #[inline]
    unsafe fn get_pixel_from_index_unchecked(
        &self,
        (ch, px, ..): (usize, usize, u16, u16),
    ) -> MaterialInstance {
        self.chunk_data
            .get_unchecked(ch)
            .pixels
            .get_unchecked(px)
            .clone()
    }

    #[inline]
    unsafe fn borrow_pixel_from_index_unchecked(
        &self,
        (ch, px, ..): (usize, usize, u16, u16),
    ) -> &MaterialInstance {
        self.chunk_data.get_unchecked(ch).pixels.get_unchecked(px)
    }

    #[inline(always)]
    unsafe fn get_pixel_local_unchecked(&self, x: i32, y: i32) -> MaterialInstance {
        self.get_pixel_from_index_unchecked(Self::local_to_indices(x, y))
    }

    #[inline]
    unsafe fn borrow_pixel_local_unchecked(&self, x: i32, y: i32) -> &MaterialInstance {
        self.borrow_pixel_from_index_unchecked(Self::local_to_indices(x, y))
    }

    #[inline]
    fn set_pixel_from_index(
        &mut self,
        (ch, px, ch_x, ch_y): (usize, usize, u16, u16),
        mat: MaterialInstance,
    ) {
        self.chunk_data[ch].pixels[px] = mat;

        self.min_x[ch] = self.min_x[ch].min(ch_x);
        self.min_y[ch] = self.min_y[ch].min(ch_y);
        self.max_x[ch] = self.max_x[ch].max(ch_x);
        self.max_y[ch] = self.max_y[ch].max(ch_y);
    }

    #[inline]
    unsafe fn set_pixel_from_index_unchecked(
        &mut self,
        (ch, px, ch_x, ch_y): (usize, usize, u16, u16),
        mat: MaterialInstance,
    ) {
        *self
            .chunk_data
            .get_unchecked_mut(ch)
            .pixels
            .get_unchecked_mut(px) = mat;

        *self.min_x.get_unchecked_mut(ch) = (*self.min_x.get_unchecked_mut(ch)).min(ch_x);
        *self.min_y.get_unchecked_mut(ch) = (*self.min_y.get_unchecked_mut(ch)).min(ch_y);
        *self.max_x.get_unchecked_mut(ch) = (*self.max_x.get_unchecked_mut(ch)).max(ch_x);
        *self.max_y.get_unchecked_mut(ch) = (*self.max_y.get_unchecked_mut(ch)).max(ch_y);
    }

    #[inline]
    unsafe fn set_pixel_local_unchecked(&mut self, x: i32, y: i32, mat: MaterialInstance) {
        self.set_pixel_from_index_unchecked(Self::local_to_indices(x, y), mat);
    }

    #[inline]
    fn get_color_from_index(&self, (ch, px, ..): (usize, usize, u16, u16)) -> Color {
        Color::rgba(
            self.chunk_data[ch].colors[px * 4],
            self.chunk_data[ch].colors[px * 4 + 1],
            self.chunk_data[ch].colors[px * 4 + 2],
            self.chunk_data[ch].colors[px * 4 + 3],
        )
    }

    #[inline]
    #[allow(dead_code)]
    unsafe fn get_color_from_index_unchecked(
        &self,
        (ch, px, ..): (usize, usize, u16, u16),
    ) -> Color {
        Color::rgba(
            *self
                .chunk_data
                .get_unchecked(ch)
                .colors
                .get_unchecked(px * 4),
            *self
                .chunk_data
                .get_unchecked(ch)
                .colors
                .get_unchecked(px * 4 + 1),
            *self
                .chunk_data
                .get_unchecked(ch)
                .colors
                .get_unchecked(px * 4 + 2),
            *self
                .chunk_data
                .get_unchecked(ch)
                .colors
                .get_unchecked(px * 4 + 3),
        )
    }

    #[inline]
    #[allow(dead_code)]
    unsafe fn get_color_local_unchecked(&self, x: i32, y: i32) -> Color {
        self.get_color_from_index_unchecked(Self::local_to_indices(x, y))
    }

    #[inline]
    fn set_color_from_index(&mut self, (ch, px, ..): (usize, usize, u16, u16), color: Color) {
        self.chunk_data[ch].colors[px * 4] = color.r;
        self.chunk_data[ch].colors[px * 4 + 1] = color.g;
        self.chunk_data[ch].colors[px * 4 + 2] = color.b;
        self.chunk_data[ch].colors[px * 4 + 3] = color.a;

        self.chunk_data[ch].dirty = true;
    }

    #[inline]
    unsafe fn set_color_from_index_unchecked(
        &mut self,
        (ch, px, ..): (usize, usize, u16, u16),
        color: Color,
    ) {
        *self
            .chunk_data
            .get_unchecked_mut(ch)
            .colors
            .get_unchecked_mut(px * 4) = color.r;
        *self
            .chunk_data
            .get_unchecked_mut(ch)
            .colors
            .get_unchecked_mut(px * 4 + 1) = color.g;
        *self
            .chunk_data
            .get_unchecked_mut(ch)
            .colors
            .get_unchecked_mut(px * 4 + 2) = color.b;
        *self
            .chunk_data
            .get_unchecked_mut(ch)
            .colors
            .get_unchecked_mut(px * 4 + 3) = color.a;

        self.chunk_data[ch].dirty = true;
    }

    #[inline]
    unsafe fn set_color_local_unchecked(&mut self, x: i32, y: i32, col: Color) {
        self.set_color_from_index_unchecked(Self::local_to_indices(x, y), col);
    }

    #[inline]
    fn get_light_from_index(&self, (ch, px, ..): (usize, usize, u16, u16)) -> [f32; 3] {
        [
            self.chunk_data[ch].lights[px][0],
            self.chunk_data[ch].lights[px][1],
            self.chunk_data[ch].lights[px][2],
        ]
    }

    #[inline]
    unsafe fn set_light_local_unchecked(&mut self, x: i32, y: i32, light: [f32; 3]) {
        self.set_light_from_index_unchecked(Self::local_to_indices(x, y), light);
    }

    #[inline]
    unsafe fn get_light_from_index_unchecked(
        &self,
        (ch, px, ..): (usize, usize, u16, u16),
    ) -> [f32; 3] {
        [
            *self
                .chunk_data
                .get_unchecked(ch)
                .lights
                .get_unchecked(px)
                .get_unchecked(0),
            *self
                .chunk_data
                .get_unchecked(ch)
                .lights
                .get_unchecked(px)
                .get_unchecked(1),
            *self
                .chunk_data
                .get_unchecked(ch)
                .lights
                .get_unchecked(px)
                .get_unchecked(2),
        ]
    }

    #[inline]
    fn set_light_from_index(&mut self, (ch, px, ..): (usize, usize, u16, u16), light: [f32; 3]) {
        self.chunk_data[ch].lights[px] = [light[0], light[1], light[2], 1.0];
    }

    #[inline]
    unsafe fn set_light_from_index_unchecked(
        &mut self,
        (ch, px, ..): (usize, usize, u16, u16),
        light: [f32; 3],
    ) {
        *self
            .chunk_data
            .get_unchecked_mut(ch)
            .lights
            .get_unchecked_mut(px) = [light[0], light[1], light[2], 1.0];
    }

    // (chunk index, pixel index, pixel x in chunk, pixel y in chunk)
    #[inline(always)]
    fn local_to_indices(x: i32, y: i32) -> (usize, usize, u16, u16) {
        let size = i32::from(CHUNK_SIZE);
        // div_euclid is the same as div_floor in this case (div_floor is currenlty unstable)
        let rel_chunk_x = x.div_euclid(i32::from(CHUNK_SIZE)) as i8;
        let rel_chunk_y = y.div_euclid(i32::from(CHUNK_SIZE)) as i8;

        let chunk_px_x = x.rem_euclid(size) as u16;
        let chunk_px_y = y.rem_euclid(size) as u16;

        (
            (rel_chunk_x + 1) as usize + (rel_chunk_y + 1) as usize * 3,
            (chunk_px_x + chunk_px_y * CHUNK_SIZE) as usize,
            chunk_px_x,
            chunk_px_y,
        )
    }

    fn finish_dirty_rects(&mut self) {
        for i in 0..9 {
            if self.min_x[i] == CHUNK_SIZE + 1 {
                self.chunk_data[i].dirty_rect = None;
            } else {
                self.chunk_data[i].dirty_rect = Some(Rect::new_wh(
                    i32::from(self.min_x[i]),
                    i32::from(self.min_y[i]),
                    self.max_x[i] - self.min_x[i] + 1,
                    self.max_y[i] - self.min_y[i] + 1,
                ));
            }
        }
    }
}

impl SimulationHelper for SimulationHelperChunk<'_, '_> {
    #[inline]
    fn get_pixel_local(&self, x: i32, y: i32) -> MaterialInstance {
        self.get_pixel_from_index(Self::local_to_indices(x, y))
    }

    #[inline]
    fn borrow_pixel_local(&self, x: i32, y: i32) -> &MaterialInstance {
        self.borrow_pixel_from_index(Self::local_to_indices(x, y))
    }

    #[inline]
    fn set_pixel_local(&mut self, x: i32, y: i32, mat: MaterialInstance) {
        self.set_pixel_from_index(Self::local_to_indices(x, y), mat);
    }

    #[inline]
    fn get_color_local(&self, x: i32, y: i32) -> Color {
        self.get_color_from_index(Self::local_to_indices(x, y))
    }

    #[inline]
    fn set_color_local(&mut self, x: i32, y: i32, col: Color) {
        self.set_color_from_index(Self::local_to_indices(x, y), col);
    }

    #[inline]
    fn add_particle(&mut self, material: MaterialInstance, pos: Position, vel: Velocity) {
        self.particles.push(Particle::new(
            material,
            Position {
                x: pos.x + f64::from(self.chunk_x) * f64::from(CHUNK_SIZE),
                y: pos.y + f64::from(self.chunk_y) * f64::from(CHUNK_SIZE),
            },
            vel,
        ));
    }

    fn get_light_local(&self, x: i32, y: i32) -> [f32; 3] {
        self.get_light_from_index(Self::local_to_indices(x, y))
    }

    fn set_light_local(&mut self, x: i32, y: i32, light: [f32; 3]) {
        self.set_light_from_index(Self::local_to_indices(x, y), light);
    }
}

struct SimulationHelperRigidBody<'a, C: Chunk> {
    air: MaterialInstance,
    chunk_handler: &'a mut ChunkHandler<C>,
    rigidbodies: &'a mut Vec<FSRigidBody>,
    particles: &'a mut Vec<Particle>,
    physics: &'a mut Physics,
}

impl<C: Chunk + Send> SimulationHelper for SimulationHelperRigidBody<'_, C> {
    fn get_pixel_local(&self, x: i32, y: i32) -> MaterialInstance {
        let world_mat = self.chunk_handler.get(i64::from(x), i64::from(y)); // TODO: consider changing the args to i64
        if let Ok(m) = world_mat {
            if m.material_id != *material::AIR {
                return m.clone();
            }
        }

        for i in 0..self.rigidbodies.len() {
            let cur = &self.rigidbodies[i];
            if let Some(body) = cur.get_body(self.physics) {
                let s = (-body.rotation().angle()).sin();
                let c = (-body.rotation().angle()).cos();

                let tx = x as f32 - body.translation().x * PHYSICS_SCALE;
                let ty = y as f32 - body.translation().y * PHYSICS_SCALE;

                let nt_x = (tx * c - ty * s) as i32;
                let nt_y = (tx * s + ty * c) as i32;

                if nt_x >= 0 && nt_y >= 0 && nt_x < cur.width.into() && nt_y < cur.width.into() {
                    let px = cur.pixels[(nt_x + nt_y * i32::from(cur.width)) as usize].clone();

                    if px.material_id != *material::AIR {
                        return px;
                    }
                }
            }
        }

        MaterialInstance::air()
    }

    fn borrow_pixel_local(&self, x: i32, y: i32) -> &MaterialInstance {
        let world_mat = self.chunk_handler.get(i64::from(x), i64::from(y)); // TODO: consider changing the args to i64
        if let Ok(m) = world_mat {
            if m.material_id != *material::AIR {
                return m;
            }
        }

        for i in 0..self.rigidbodies.len() {
            let cur = &self.rigidbodies[i];
            if let Some(body) = cur.get_body(self.physics) {
                let s = (-body.rotation().angle()).sin();
                let c = (-body.rotation().angle()).cos();

                let tx = x as f32 - body.translation().x * PHYSICS_SCALE;
                let ty = y as f32 - body.translation().y * PHYSICS_SCALE;

                let nt_x = (tx * c - ty * s) as i32;
                let nt_y = (tx * s + ty * c) as i32;

                if nt_x >= 0 && nt_y >= 0 && nt_x < cur.width.into() && nt_y < cur.width.into() {
                    let px = &cur.pixels[(nt_x + nt_y * i32::from(cur.width)) as usize];

                    if px.material_id != *material::AIR {
                        return px;
                    }
                }
            }
        }

        &self.air
    }

    fn set_pixel_local(&mut self, x: i32, y: i32, mat: MaterialInstance) {
        let _ignore = self.chunk_handler.set(i64::from(x), i64::from(y), mat); // TODO: consider changing the args to i64
    }

    fn get_color_local(&self, x: i32, y: i32) -> Color {
        let (chunk_x, chunk_y) = pixel_to_chunk_pos(i64::from(x), i64::from(y));
        let chunk = self.chunk_handler.get_chunk(chunk_x, chunk_y);

        if let Some(ch) = chunk {
            let col_r = ch.get_color(
                (i64::from(x) - i64::from(chunk_x) * i64::from(CHUNK_SIZE)) as u16,
                (i64::from(y) - i64::from(chunk_y) * i64::from(CHUNK_SIZE)) as u16,
            );
            if let Ok(col) = col_r {
                if col.a > 0 {
                    return col;
                }
            }
        }

        for i in 0..self.rigidbodies.len() {
            let cur = &self.rigidbodies[i];
            if let Some(body) = cur.get_body(self.physics) {
                let s = (-body.rotation().angle()).sin();
                let c = (-body.rotation().angle()).cos();

                let tx = x as f32 - body.translation().x * PHYSICS_SCALE;
                let ty = y as f32 - body.translation().y * PHYSICS_SCALE;

                let nt_x = (tx * c - ty * s) as i32;
                let nt_y = (tx * s + ty * c) as i32;

                if nt_x >= 0 && nt_y >= 0 && nt_x < cur.width.into() && nt_y < cur.width.into() {
                    let px = cur.pixels[(nt_x + nt_y * i32::from(cur.width)) as usize].clone();

                    if px.material_id != *material::AIR {
                        return px.color;
                    }
                }
            }
        }

        Color::rgba(0, 0, 0, 0)
    }

    fn set_color_local(&mut self, x: i32, y: i32, col: Color) {
        let (chunk_x, chunk_y) = pixel_to_chunk_pos(i64::from(x), i64::from(y));
        let chunk = self.chunk_handler.get_chunk_mut(chunk_x, chunk_y);

        if let Some(ch) = chunk {
            let _ignore = ch.set_color(
                (i64::from(x) - i64::from(chunk_x) * i64::from(CHUNK_SIZE)) as u16,
                (i64::from(y) - i64::from(chunk_y) * i64::from(CHUNK_SIZE)) as u16,
                col,
            );
        }
    }

    #[inline]
    fn add_particle(&mut self, material: MaterialInstance, pos: Position, vel: Velocity) {
        self.particles.push(Particle::new(material, pos, vel));
    }

    fn get_light_local(&self, _x: i32, _y: i32) -> [f32; 3] {
        // TODO
        [0.0; 3]
    }

    fn set_light_local(&mut self, _x: i32, _y: i32, _light: [f32; 3]) {
        // TODO
    }
}

#[derive(Debug)]
pub struct SimulatorChunkContext<'a> {
    pub pixels: &'a mut [MaterialInstance; (CHUNK_SIZE * CHUNK_SIZE) as usize],
    pub colors: &'a mut [u8; (CHUNK_SIZE * CHUNK_SIZE) as usize * 4],
    pub lights: &'a mut [[f32; 4]; CHUNK_SIZE as usize * CHUNK_SIZE as usize],
    pub dirty: bool,
    pub dirty_rect: Option<Rect<i32>>,
}

impl Simulator {
    #[warn(clippy::too_many_arguments)]
    #[profiling::function]
    pub fn simulate_chunk(
        chunk_x: i32,
        chunk_y: i32,
        chunk_data: &mut [SimulatorChunkContext; 9],
        particles: &mut Vec<Particle>,
        registries: Arc<Registries>,
    ) {
        const CENTER_CHUNK: usize = 4;

        let my_dirty_rect_o = chunk_data[CENTER_CHUNK].dirty_rect;
        if my_dirty_rect_o.is_none() {
            for d in chunk_data {
                d.dirty_rect = None;
            }
            return;
        }
        let my_dirty_rect = my_dirty_rect_o.unwrap();

        let mut helper = SimulationHelperChunk {
            chunk_data,
            min_x: [CHUNK_SIZE + 1; 9],
            min_y: [CHUNK_SIZE + 1; 9],
            max_x: [0; 9],
            max_y: [0; 9],
            particles,
            chunk_x,
            chunk_y,
        };

        let rng = fastrand::Rng::new();
        {
            // this being inlined is important for performance
            #[inline(always)]
            fn process(
                x: i32,
                y: i32,
                helper: &mut SimulationHelperChunk,
                rng: &Rng,
                _registries: &Registries,
            ) {
                // Safety: dirty rects are always within the chunk
                let cur = unsafe { helper.get_pixel_local_unchecked(x, y) };

                if let Some(mat) = Simulator::simulate_pixel(x, y, &cur, helper, rng) {
                    unsafe {
                        helper.set_color_local_unchecked(x, y, mat.color);
                        helper.set_light_local_unchecked(x, y, mat.light);
                        helper.set_pixel_local_unchecked(x, y, mat);
                    }
                }
            }

            profiling::scope!("loop");
            if rng.bool() {
                for y in my_dirty_rect.range_tb().rev() {
                    for x in my_dirty_rect.range_lr() {
                        process(x, y, &mut helper, &rng, &registries);
                    }
                }
            } else {
                for y in my_dirty_rect.range_tb().rev() {
                    for x in my_dirty_rect.range_lr().rev() {
                        process(x, y, &mut helper, &rng, &registries);
                    }
                }
            }
        }

        helper.finish_dirty_rects();
    }

    #[allow(clippy::unnecessary_unwrap)]
    #[allow(clippy::needless_range_loop)]
    #[profiling::function]
    pub fn simulate_rigidbodies<C: Chunk + Send>(
        chunk_handler: &mut ChunkHandler<C>,
        rigidbodies: &mut Vec<FSRigidBody>,
        physics: &mut Physics,
        particles: &mut Vec<Particle>,
    ) {
        let mut dirty = vec![false; rigidbodies.len()];
        let mut needs_remesh = vec![false; rigidbodies.len()];
        for i in 0..rigidbodies.len() {
            let rb_w = rigidbodies[i].width;
            let rb_h = rigidbodies[i].height;
            let body_opt = rigidbodies[i].get_body(physics);

            if body_opt.is_some() {
                let s = body_opt.unwrap().rotation().angle().sin();
                let c = body_opt.unwrap().rotation().angle().cos();
                let pos_x = body_opt.unwrap().translation().x * PHYSICS_SCALE;
                let pos_y = body_opt.unwrap().translation().y * PHYSICS_SCALE;

                let mut helper = SimulationHelperRigidBody {
                    air: MaterialInstance::air(),
                    chunk_handler,
                    rigidbodies,
                    particles,
                    physics,
                };

                let rng = fastrand::Rng::new();
                for rb_y in 0..rb_w {
                    for rb_x in 0..rb_h {
                        let tx = f32::from(rb_x) * c - f32::from(rb_y) * s + pos_x;
                        let ty = f32::from(rb_x) * s + f32::from(rb_y) * c + pos_y;

                        // let cur = helper.get_pixel_local(tx as i32, ty as i32);
                        let cur =
                            helper.rigidbodies[i].pixels[(rb_x + rb_y * rb_w) as usize].clone();

                        let res =
                            Self::simulate_pixel(tx as i32, ty as i32, &cur, &mut helper, &rng);

                        // if cur.material_id != material::AIR {
                        //     // helper.set_pixel_local(tx as i32, ty as i32, MaterialInstance {
                        //     //     material_id: material::TEST,
                        //     //     physics: PhysicsType::Sand,
                        //     //     color: Color::RGB(64, 255, 64),
                        //     // });
                        //     // helper.set_pixel_local(tx as i32, ty as i32, cur);

                        // }

                        if let Some(mat) = res {
                            helper.rigidbodies[i].pixels[(rb_x + rb_y * rb_w) as usize] =
                                mat.clone();
                            dirty[i] = true;
                            if (cur.physics == PhysicsType::Solid
                                && mat.physics != PhysicsType::Solid)
                                || (cur.physics != PhysicsType::Solid
                                    && mat.physics == PhysicsType::Solid)
                            {
                                needs_remesh[i] = true;
                            }
                        }

                        // helper.rigidbodies[i].height = 5;
                    }
                }
            }
        }

        for i in 0..rigidbodies.len() {
            if dirty[i] && !needs_remesh[i] {
                // don't bother updating the image if it's going to be destroyed anyway
                rigidbodies[i].image_dirty = true;
            }
        }

        let mut new_rb: Vec<FSRigidBody> = rigidbodies
            .drain(..)
            .enumerate()
            .flat_map(|(i, mut rb): (usize, FSRigidBody)| {
                if needs_remesh[i] {
                    let pos = (
                        rb.get_body(physics).unwrap().translation().x,
                        rb.get_body(physics).unwrap().translation().y,
                    );

                    let rb_pos = *rb.get_body(physics).unwrap().translation();
                    let rb_angle = rb.get_body(physics).unwrap().rotation().angle();
                    let rb_linear_velocity = *rb.get_body(physics).unwrap().linvel();
                    let rb_angular_velocity = rb.get_body(physics).unwrap().angvel();

                    physics.bodies.remove(
                        rb.body.take().unwrap(),
                        &mut physics.islands,
                        &mut physics.colliders,
                        &mut physics.impulse_joints,
                        &mut physics.multibody_joints,
                        true,
                    );
                    let mut r = rigidbody::FSRigidBody::make_bodies(
                        &rb.pixels, rb.width, rb.height, physics, pos,
                    )
                    .unwrap_or_default();

                    for rb in &mut r {
                        rb.get_body_mut(physics)
                            .unwrap()
                            .set_position(Isometry2::new(rb_pos, rb_angle), true);
                        rb.get_body_mut(physics)
                            .unwrap()
                            .set_linvel(rb_linear_velocity, true);
                        rb.get_body_mut(physics)
                            .unwrap()
                            .set_angvel(rb_angular_velocity, true);
                    }

                    r
                } else {
                    vec![rb]
                }
            })
            .collect();

        rigidbodies.append(&mut new_rb);
    }

    #[allow(clippy::inline_always)]
    #[inline(always)] // speeds up simulate_chunk by ~35%
    fn simulate_pixel(
        x: i32,
        y: i32,
        cur: &MaterialInstance,
        helper: &mut impl SimulationHelper,
        rng: &fastrand::Rng,
    ) -> Option<MaterialInstance> {
        let mut new_mat = None;

        #[allow(clippy::single_match)]
        match cur.physics {
            PhysicsType::Sand => {
                let below = helper.borrow_pixel_local(x, y + 1);
                let below_can = below.physics == PhysicsType::Air;

                let bl = helper.borrow_pixel_local(x - 1, y + 1);
                let bl_can = bl.physics == PhysicsType::Air;

                let br = helper.borrow_pixel_local(x + 1, y + 1);
                let br_can = br.physics == PhysicsType::Air;

                if below_can && (!(br_can || bl_can) || rng.f32() > 0.1) {
                    // let below2_i = index_helper(x, y + 2);
                    // let below2 = (*pixels[below_i.0])[below_i.1];
                    // if below2.physics == PhysicsType::Air {
                    //     set_color(x, y + 2, cur.color, true);
                    //     (*pixels[below2_i.0])[below2_i.1] = cur;
                    //     new_mat = Some(MaterialInstance::air());
                    // }else {

                    let empty_below = (0..4).all(|i| {
                        let pix = helper.borrow_pixel_local(x, y + i + 2); // don't include myself or one below
                        pix.physics == PhysicsType::Air
                    });

                    if empty_below {
                        helper.add_particle(
                            cur.clone(),
                            Position { x: f64::from(x), y: f64::from(y) },
                            Velocity { x: (rng.f64() - 0.5) * 0.5, y: 1.0 + rng.f64() },
                        );
                    } else if rng.bool()
                        && helper.borrow_pixel_local(x, y + 2).physics == PhysicsType::Air
                    {
                        helper.set_color_local(x, y + 2, cur.color);
                        helper.set_light_local(x, y + 2, cur.light);
                        helper.set_pixel_local(x, y + 2, cur.clone());
                    } else {
                        helper.set_color_local(x, y + 1, cur.color);
                        helper.set_light_local(x, y + 1, cur.light);
                        helper.set_pixel_local(x, y + 1, cur.clone());
                    }

                    new_mat = Some(MaterialInstance::air());

                    // }
                } else {
                    let above = helper.borrow_pixel_local(x, y - 1);
                    let above_air = above.physics == PhysicsType::Air;
                    if above_air || rng.f32() > 0.5 {
                        if bl_can && br_can {
                            if rng.bool() {
                                helper.set_color_local(x + 1, y + 1, cur.color);
                                helper.set_light_local(x + 1, y + 1, cur.light);
                                helper.set_pixel_local(x + 1, y + 1, cur.clone());
                            } else {
                                helper.set_color_local(x - 1, y + 1, cur.color);
                                helper.set_light_local(x - 1, y + 1, cur.light);
                                helper.set_pixel_local(x - 1, y + 1, cur.clone());
                            }
                            new_mat = Some(MaterialInstance::air());
                        } else if bl_can {
                            if rng.bool()
                                && helper.borrow_pixel_local(x - 2, y + 1).physics
                                    == PhysicsType::Air
                                && helper.borrow_pixel_local(x - 2, y + 2).physics
                                    != PhysicsType::Air
                            {
                                helper.set_color_local(x - 2, y + 1, cur.color);
                                helper.set_light_local(x - 2, y + 1, cur.light);
                                helper.set_pixel_local(x - 2, y + 1, cur.clone());
                                new_mat = Some(MaterialInstance::air());
                            } else {
                                helper.set_color_local(x - 1, y + 1, cur.color);
                                helper.set_light_local(x - 1, y + 1, cur.light);
                                helper.set_pixel_local(x - 1, y + 1, cur.clone());
                                new_mat = Some(MaterialInstance::air());
                            }
                        } else if br_can {
                            if rng.bool()
                                && helper.borrow_pixel_local(x + 2, y + 1).physics
                                    == PhysicsType::Air
                                && helper.borrow_pixel_local(x + 2, y + 2).physics
                                    != PhysicsType::Air
                            {
                                helper.set_color_local(x + 2, y + 1, cur.color);
                                helper.set_light_local(x + 2, y + 1, cur.light);
                                helper.set_pixel_local(x + 2, y + 1, cur.clone());
                                new_mat = Some(MaterialInstance::air());
                            } else {
                                helper.set_color_local(x + 1, y + 1, cur.color);
                                helper.set_light_local(x + 1, y + 1, cur.light);
                                helper.set_pixel_local(x + 1, y + 1, cur.clone());
                                new_mat = Some(MaterialInstance::air());
                            }
                        }
                    }
                }
            },
            _ => {},
        }

        new_mat
    }
}
