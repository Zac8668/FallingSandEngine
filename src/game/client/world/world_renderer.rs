use std::{iter, ptr::slice_from_raw_parts};

use rapier2d::prelude::Shape;
use sdl2::{pixels::Color, rect::Rect};
use sdl_gpu::{shaders::Shader, GPUFilter, GPUFormat, GPUImage, GPURect, GPUSubsystem, GPUTarget};
use specs::{
    prelude::ParallelIterator,
    rayon::{
        iter::{IndexedParallelIterator, IntoParallelRefIterator},
        slice::ParallelSlice,
    },
    Join, ReadStorage, WorldExt, WriteStorage,
};

use crate::game::{
    client::{
        render::{Fonts, RenderCanvas, Renderable, Sdl2Context, Shaders, TransformStack},
        Client,
    },
    common::{
        world::{
            entity::{
                GameEntity, Hitbox, PhysicsEntity, Player, PlayerGrappleState, PlayerMovementMode,
            },
            gen::WorldGenerator,
            particle::{Particle, ParticleSystem},
            physics::PHYSICS_SCALE,
            AutoTarget, Camera, ChunkHandlerGeneric, ChunkState, Position, Velocity, World,
            CHUNK_SIZE,
        },
        Settings,
    },
};

use super::{ClientChunk, ClientWorld};

pub struct WorldRenderer {
    pub liquid_image: GPUImage,
    pub liquid_image2: GPUImage,
    physics_dirty: bool,
}

impl WorldRenderer {
    pub fn new() -> Self {
        let mut liquid_image =
            GPUSubsystem::create_image(1920 / 2, 1080 / 2, GPUFormat::GPU_FORMAT_RGBA);
        liquid_image.set_image_filter(GPUFilter::GPU_FILTER_NEAREST);

        let mut liquid_image2 =
            GPUSubsystem::create_image(1920 / 2, 1080 / 2, GPUFormat::GPU_FORMAT_RGBA);
        liquid_image2.set_image_filter(GPUFilter::GPU_FILTER_NEAREST);

        Self { liquid_image, liquid_image2, physics_dirty: false }
    }

    pub fn init(&self, world: &mut World<ClientChunk>) {}

    #[warn(clippy::too_many_arguments)]
    #[warn(clippy::too_many_lines)]
    #[profiling::function]
    pub fn render(
        &mut self,
        world: &mut World<ClientChunk>,
        target: &mut GPUTarget,
        transform: &mut TransformStack,
        _delta_time: f64,
        sdl: &Sdl2Context,
        fonts: &Fonts,
        settings: &Settings,
        shaders: &Shaders,
        client: &mut Option<Client>,
        partial_ticks: f64,
    ) {
        // TODO
        // if world.lqf_world.get_debug_draw().is_none() {
        //     self.init(world);
        // }

        // draw world

        let (position_storage, velocity_storage, camera_storage) = world.ecs.system_data::<(
            ReadStorage<Position>,
            ReadStorage<Velocity>,
            ReadStorage<Camera>,
        )>();

        let camera_pos = (&position_storage, velocity_storage.maybe(), &camera_storage)
            .join()
            .find_map(|(p, v, _c)| {
                Some(Position {
                    x: p.x + v.map_or(0.0, |v| v.x) * partial_ticks,
                    y: p.y + v.map_or(0.0, |v| v.y) * partial_ticks,
                })
            })
            .expect("No Camera in world!");

        let loader_pos = match client {
            Some(Client { world: Some(ClientWorld { local_entity }), .. }) => local_entity
                .and_then(|local| position_storage.get(local))
                .or(Some(&camera_pos))
                .map(|pos| (pos.x, pos.y))
                .unwrap(),
            _ => (camera_pos.x, camera_pos.y),
        };

        drop(position_storage);
        drop(velocity_storage);
        drop(camera_storage);

        let camera_scale = client.as_ref().unwrap().camera_scale;

        transform.push();
        transform.translate(
            f64::from(target.width()) / 2.0,
            f64::from(target.height()) / 2.0,
        );
        transform.scale(camera_scale, camera_scale);
        transform.translate(-camera_pos.x, -camera_pos.y);

        let screen_zone = world
            .chunk_handler
            .get_screen_zone((camera_pos.x, camera_pos.y)); // note we always use the camera for the screen zone
        let active_zone = world.chunk_handler.get_active_zone(loader_pos);
        let load_zone = world.chunk_handler.get_load_zone(loader_pos);
        let unload_zone = world.chunk_handler.get_unload_zone(loader_pos);

        // let clip = canvas.clip_rect();
        // if game.settings.cull_chunks {
        //     canvas.set_clip_rect(transform.transform_rect(screen_zone));
        // }

        {
            profiling::scope!("chunks");
            world
                .chunk_handler
                .loaded_chunks
                .iter()
                .for_each(|(_i, ch)| {
                    let rc = Rect::new(
                        ch.chunk_x * i32::from(CHUNK_SIZE),
                        ch.chunk_y * i32::from(CHUNK_SIZE),
                        u32::from(CHUNK_SIZE),
                        u32::from(CHUNK_SIZE),
                    );
                    if (settings.debug && !settings.cull_chunks) || rc.has_intersection(screen_zone)
                    {
                        transform.push();
                        transform.translate(
                            ch.chunk_x * i32::from(CHUNK_SIZE),
                            ch.chunk_y * i32::from(CHUNK_SIZE),
                        );
                        ch.render(target, transform, sdl, fonts, settings);

                        if settings.debug && settings.draw_chunk_dirty_rects {
                            if let Some(dr) = ch.dirty_rect {
                                let rect = transform.transform_rect(dr);
                                target.rectangle_filled2(rect, Color::RGBA(255, 64, 64, 127));
                                target.rectangle2(rect, Color::RGBA(255, 64, 64, 127));
                            }
                            if ch.graphics.was_dirty {
                                let rect = transform.transform_rect(Rect::new(
                                    0,
                                    0,
                                    u32::from(CHUNK_SIZE),
                                    u32::from(CHUNK_SIZE),
                                ));
                                target.rectangle_filled2(rect, Color::RGBA(255, 255, 64, 127));
                                target.rectangle2(rect, Color::RGBA(255, 255, 64, 127));
                            }
                        }

                        transform.pop();
                    }

                    if settings.debug && settings.draw_chunk_state_overlay {
                        let rect = transform.transform_rect(rc);

                        let alpha: u8 = (settings.draw_chunk_state_overlay_alpha * 255.0) as u8;
                        let color = match ch.state {
                            ChunkState::NotGenerated => Color::RGBA(127, 127, 127, alpha),
                            ChunkState::Generating(stage) => Color::RGBA(
                                64,
                                (f32::from(stage)
                                    / f32::from(world.chunk_handler.generator.max_gen_stage())
                                    * 255.0) as u8,
                                255,
                                alpha,
                            ),
                            ChunkState::Cached => Color::RGBA(255, 127, 64, alpha),
                            ChunkState::Active => Color::RGBA(64, 255, 64, alpha),
                        };
                        target.rectangle_filled2(rect, color);
                        target.rectangle2(rect, color);

                        // let ind = world.chunk_handler.chunk_index(ch.chunk_x, ch.chunk_y);
                        // let ind = world.chunk_handler.chunk_update_order(ch.chunk_x, ch.chunk_y);
                        // let tex = canvas.texture_creator();
                        // let txt_sf = fonts.pixel_operator
                        //     .render(format!("{}", ind).as_str())
                        //     .solid(Color::RGB(255, 255, 255)).unwrap();
                        // let txt_tex = tex.create_texture_from_surface(&txt_sf).unwrap();

                        // let aspect = txt_sf.width() as f32 / txt_sf.height() as f32;
                        // let mut txt_height = rect.height() as f32 * 0.75;
                        // let mut txt_width = (aspect * txt_height as f32) as u32;

                        // let max_width = (rect.w as f32 * 0.9) as u32;

                        // if txt_width > max_width as u32 {
                        //     txt_width = max_width as u32;
                        //     txt_height = 1.0 / aspect * txt_width as f32;
                        // }

                        // let txt_rec = Rect::new(rect.x + rect.w/2 - (txt_width as i32)/2, rect.y, txt_width, txt_height as u32);
                        // canvas.copy(&txt_tex, None, Some(txt_rec)).unwrap();
                    }
                });
        }

        // draw liquids

        if self.physics_dirty {
            self.physics_dirty = false;

            let mut liquid_target = self.liquid_image.get_target();
            liquid_target.clear();

            for (handle, fluid) in world.physics.fluid_pipeline.liquid_world.fluids().iter() {
                for (idx, particle) in fluid.positions.iter().enumerate() {
                    let (x, y) = transform.transform((
                        particle.coords[0] * PHYSICS_SCALE,
                        particle.coords[1] * PHYSICS_SCALE,
                    ));
                    target.circle_filled(x as f32, y as f32, 2.0, Color::CYAN);
                }
            }

            // if let Some(particle_system) = world.lqf_world.get_particle_system_list() {
            //     let particle_count = particle_system.get_particle_count();
            //     let particle_colors: &[b2ParticleColor] = particle_system.get_color_buffer();
            //     let particle_positions: &[Vec2] = particle_system.get_position_buffer();

            //     for i in 0..particle_count as usize {
            //         let pos = particle_positions[i];
            //         let color = particle_colors[i];
            //         let cam_x = camera_pos.x.floor();
            //         let cam_y = camera_pos.y.floor();
            //         GPUSubsystem::set_shape_blend_mode(
            //             sdl_gpu::sys::GPU_BlendPresetEnum::GPU_BLEND_SET,
            //         );
            //         let color = Color::RGBA(color.r, color.g, color.b, color.a);
            //         // let color = Color::RGBA(64, 90, 255, 191);
            //         liquid_target.pixel(
            //             pos.x * PHYSICS_SCALE - cam_x as f32 + 1920.0 / 4.0 - 1.0,
            //             pos.y * PHYSICS_SCALE - cam_y as f32 + 1080.0 / 4.0 - 1.0,
            //             color,
            //         );
            //         // liquid_target.circle_filled(pos.x * 2.0 - camera_pos.x as f32 + 1920.0/4.0, pos.y * 2.0 - camera_pos.y as f32 + 1080.0/4.0, 2.0, Color::RGB(100, 100, 255));
            //     }

            //     GPUSubsystem::set_shape_blend_mode(
            //         sdl_gpu::sys::GPU_BlendPresetEnum::GPU_BLEND_NORMAL,
            //     );

            //     let mut liquid_target2 = self.liquid_image2.get_target();
            //     liquid_target2.clear();

            //     self.liquid_image
            //         .set_blend_mode(sdl_gpu::sys::GPU_BlendPresetEnum::GPU_BLEND_SET);

            //     shaders.liquid_shader.activate();
            //     self.liquid_image
            //         .blit_rect(None::<GPURect>, &mut liquid_target2, None);
            //     Shader::deactivate();

            //     self.liquid_image
            //         .set_blend_mode(sdl_gpu::sys::GPU_BlendPresetEnum::GPU_BLEND_NORMAL);
            // };
        }

        // TODO: transforming screen zone here is not the right way to do this, it causes some jumping when x or y switch between + and -
        self.liquid_image2
            .blit_rect(None, target, Some(transform.transform_rect(screen_zone)));

        // draw solids

        {
            profiling::scope!("rigidbodies");
            transform.push();
            transform.scale(PHYSICS_SCALE, PHYSICS_SCALE);
            for rb in &mut world.rigidbodies {
                if rb.image.is_none() {
                    rb.update_image();
                }

                if let Some(body) = rb.get_body(&world.physics) {
                    if let Some(img) = &rb.image {
                        let pos = body.translation();

                        let (width, height) = (
                            f32::from(rb.width) / PHYSICS_SCALE,
                            f32::from(rb.height) / PHYSICS_SCALE,
                        );

                        let mut rect = GPURect::new(pos.x, pos.y, width, height);

                        let (x1, y1) = transform.transform((rect.x, rect.y));
                        let (x2, y2) = transform.transform((rect.x + rect.w, rect.y + rect.h));

                        rect = GPURect::new2(x1 as f32, y1 as f32, x2 as f32, y2 as f32);

                        img.blit_rect_x(
                            None,
                            target,
                            Some(rect),
                            body.rotation().angle().to_degrees(),
                            0.0,
                            0.0,
                            0,
                        );
                    }
                }
            }
            transform.pop();
        }

        // physics debug draw

        if settings.debug && settings.physics_dbg_draw {
            profiling::scope!("physics debug");
            transform.push();
            transform.scale(PHYSICS_SCALE, PHYSICS_SCALE);

            fn draw_shape(
                shape: &dyn Shape,
                x: f32,
                y: f32,
                angle: f32,
                transform: &mut TransformStack,
                target: &mut GPUTarget,
                color: Color,
            ) {
                transform.push();
                transform.translate(x, y);
                if let Some(comp) = shape.as_compound() {
                    for (iso, shape) in comp.shapes() {
                        draw_shape(&**shape, 0.0, 0.0, 0.0, transform, target, color);
                    }
                } else if let Some(cuboid) = shape.as_cuboid() {
                    let (x1, y1) =
                        transform.transform((-cuboid.half_extents[0], -cuboid.half_extents[1]));
                    let (x2, y2) =
                        transform.transform((cuboid.half_extents[0], cuboid.half_extents[1]));
                    target.rectangle(x1 as f32, y1 as f32, x2 as f32, y2 as f32, color);
                } else if let Some(polyline) = shape.as_polyline() {
                    for seg in polyline.segments() {
                        let (x1, y1) = transform.transform((seg.a[0], seg.a[1]));
                        let (x2, y2) = transform.transform((seg.b[0], seg.b[1]));
                        target.line(x1 as f32, y1 as f32, x2 as f32, y2 as f32, color);
                    }
                } else if let Some(poly) = shape.as_convex_polygon() {
                    target.polygon(
                        poly.points()
                            .iter()
                            .flat_map(|v| {
                                let (x, y) = transform.transform((v[0], v[1]));
                                [x as f32, y as f32]
                            })
                            .collect(),
                        color,
                    );
                } else if let Some(trimesh) = shape.as_trimesh() {
                    for tri in trimesh.triangles() {
                        let (x1, y1) = transform.transform((tri.a[0], tri.a[1]));
                        let (x2, y2) = transform.transform((tri.b[0], tri.b[1]));
                        let (x3, y3) = transform.transform((tri.c[0], tri.c[1]));
                        target.polygon(
                            vec![
                                x1 as f32, y1 as f32, x2 as f32, y2 as f32, x3 as f32, y3 as f32,
                            ],
                            color,
                        );
                    }
                } else if let Some(tri) = shape.as_triangle() {
                    let (x1, y1) = transform.transform((x + tri.a[0], y + tri.a[1]));
                    let (x2, y2) = transform.transform((x + tri.b[0], y + tri.b[1]));
                    let (x3, y3) = transform.transform((x + tri.c[0], y + tri.c[1]));
                    target.polygon(
                        vec![
                            x1 as f32, y1 as f32, x2 as f32, y2 as f32, x3 as f32, y3 as f32,
                        ],
                        color,
                    );
                }
                transform.pop();
            }

            // TODO: physics_dbg_draw_joint
            // TODO: physics_dbg_draw_pair
            // TODO: physics_dbg_draw_particle

            for (a, b) in world.physics.bodies.iter() {
                let (rx, ry) = (
                    b.position().translation.vector[0],
                    b.position().translation.vector[1],
                );

                let (x, y) = transform.transform((rx, ry));
                target.circle(x as f32, y as f32, 3.0, Color::GREEN);

                if settings.physics_dbg_draw_center_of_mass {
                    let com = b.mass_properties().world_com(b.position());
                    let (x, y) = transform.transform((com.x, com.y));
                    target.circle(x as f32, y as f32, 2.0, Color::RED);
                }

                for c in b.colliders() {
                    let col = world.physics.colliders.get(*c).unwrap();

                    if settings.physics_dbg_draw_shape {
                        let shape = col.shape();
                        draw_shape(
                            shape,
                            rx,
                            ry,
                            b.rotation().angle(),
                            transform,
                            target,
                            Color::RGBA(
                                0x00,
                                0xff,
                                0x00,
                                if b.is_sleeping() { 0x64 } else { 0xff },
                            ),
                        );
                    }

                    if settings.physics_dbg_draw_aabb {
                        let aabb = col.compute_aabb();

                        transform.push();
                        transform.translate(aabb.center().x, aabb.center().y);

                        let (x1, y1) =
                            transform.transform((-aabb.half_extents()[0], -aabb.half_extents()[1]));
                        let (x2, y2) =
                            transform.transform((aabb.half_extents()[0], aabb.half_extents()[1]));
                        target.rectangle(
                            x1 as f32,
                            y1 as f32,
                            x2 as f32,
                            y2 as f32,
                            Color::RGBA(0xff, 0, 0xff, if b.is_sleeping() { 0x64 } else { 0xff }),
                        );

                        transform.pop();
                    }
                }
            }

            transform.pop();
        }

        {
            profiling::scope!("particles");
            let particle_system = world.ecs.read_resource::<ParticleSystem>();

            // TODO: magic number, works well on my machine but probably different on others
            let mut batches: Vec<Vec<f32>> = particle_system
                .active
                .par_chunks(2000)
                .map(|chunk| {
                    let mut batch = Vec::new();
                    for part in chunk {
                        #[allow(clippy::cast_lossless)]
                        if screen_zone.contains_point(sdl2::rect::Point::new(
                            part.pos.x as i32,
                            part.pos.y as i32,
                        )) || !settings.cull_chunks
                        {
                            let lerp_x = part.pos.x + part.vel.x * partial_ticks;
                            let lerp_y = part.pos.y + part.vel.y * partial_ticks;
                            let (x1, y1) = transform.transform((lerp_x - 0.5, lerp_y - 0.5));
                            let (x2, y2) = transform.transform((lerp_x + 0.5, lerp_y + 0.5));
                            let col = f32::from_le_bytes([
                                part.material.color.r,
                                part.material.color.g,
                                part.material.color.b,
                                part.material.color.a,
                            ]);

                            batch.extend([
                                x1 as f32, y1 as f32, col, x2 as f32, y1 as f32, col, x2 as f32,
                                y2 as f32, col, x1 as f32, y1 as f32, col, x2 as f32, y2 as f32,
                                col, x1 as f32, y2 as f32, col,
                            ]);
                            // target.rectangle_filled(
                            //     x1 as f32,
                            //     y1 as f32,
                            //     x2 as f32,
                            //     y2 as f32,
                            //     part.material.color,
                            // );
                        }
                    }
                    batch
                })
                .collect();
            for mut batch in &mut batches {
                // profiling::scope!("triangle_batch_raw_u8", format!("#verts = {}", batch.len() / 3).as_str());
                target.triangle_batch_raw_u8(batch);
            }
        }

        {
            profiling::scope!("ecs debug");

            let (game_entity_storage, position_storage, velocity_storage, physics_storage) =
                world.ecs.system_data::<(
                    ReadStorage<GameEntity>,
                    ReadStorage<Position>,
                    ReadStorage<Velocity>,
                    ReadStorage<PhysicsEntity>,
                )>();

            (
                &game_entity_storage,
                &position_storage,
                velocity_storage.maybe(),
                physics_storage.maybe(),
            )
                .join()
                .for_each(
                    |(_ge, pos, vel, _phys): (
                        &GameEntity,
                        &Position,
                        Option<&Velocity>,
                        Option<&PhysicsEntity>,
                    )| {
                        let mut draw = |x: f64, y: f64, alpha: u8| {
                            transform.push();
                            transform.translate(x, y);

                            let (x1, y1) = transform.transform((-1.0, -1.0));
                            let (x2, y2) = transform.transform((1.0, 1.0));

                            target.rectangle(
                                x1 as f32,
                                y1 as f32,
                                x2 as f32,
                                y2 as f32,
                                Color::RGBA(64, 255, 64, alpha),
                            );

                            if let Some(vel) = vel {
                                let (vel_x1, vel_y1) = transform.transform((0.0, 0.0));
                                let (vel_x2, vel_y2) = transform.transform((vel.x, vel.y));

                                target.line(
                                    vel_x1 as f32,
                                    vel_y1 as f32,
                                    vel_x2 as f32,
                                    vel_y2 as f32,
                                    Color::RGBA(64, 255, 64, alpha),
                                );
                            }

                            transform.pop();
                        };

                        let lerp_x = pos.x + vel.map_or(0.0, |v| v.x) * partial_ticks;
                        let lerp_y = pos.y + vel.map_or(0.0, |v| v.y) * partial_ticks;
                        draw(lerp_x, lerp_y, 255);
                        draw(pos.x, pos.y, 80);
                    },
                );

            let (position_storage, hitbox_storage, velocity_storage) = world.ecs.system_data::<(
                ReadStorage<Position>,
                ReadStorage<Hitbox>,
                ReadStorage<Velocity>,
            )>();

            (&position_storage, &hitbox_storage, velocity_storage.maybe())
                .join()
                .for_each(|(pos, hit, vel)| {
                    let mut draw = |x: f64, y: f64, alpha: u8| {
                        transform.push();
                        transform.translate(x, y);

                        let (x1, y1) = transform.transform((f64::from(hit.x1), f64::from(hit.y1)));
                        let (x2, y2) = transform.transform((f64::from(hit.x2), f64::from(hit.y2)));

                        target.rectangle(
                            x1 as f32,
                            y1 as f32,
                            x2 as f32,
                            y2 as f32,
                            Color::RGBA(255, 64, 64, alpha),
                        );

                        transform.pop();
                    };

                    let lerp_x = pos.x + vel.map_or(0.0, |v| v.x) * partial_ticks;
                    let lerp_y = pos.y + vel.map_or(0.0, |v| v.y) * partial_ticks;
                    draw(lerp_x, lerp_y, 255);
                    draw(pos.x, pos.y, 80);
                });

            let (position_storage, velocity_storage, target_storage) = world.ecs.system_data::<(
                ReadStorage<Position>,
                ReadStorage<Velocity>,
                ReadStorage<AutoTarget>,
            )>();

            (&position_storage, velocity_storage.maybe(), &target_storage)
                .join()
                .for_each(|(pos, vel, at)| {
                    let mut draw = |x: f64, y: f64, alpha: u8| {
                        transform.push();
                        transform.translate(x, y);

                        let (x1, y1) = transform.transform((-1.0, -1.0));
                        let (x2, y2) = transform.transform((1.0, 1.0));

                        target.rectangle(
                            x1 as f32,
                            y1 as f32,
                            x2 as f32,
                            y2 as f32,
                            Color::RGBA(64, 255, 64, alpha),
                        );

                        let target_pos = at.get_target_pos(&position_storage);
                        if let Some(target_pos) = target_pos {
                            let (line_x1, line_y1) = (0.0, 0.0);
                            let (line_x2, line_y2) = (target_pos.x - x, target_pos.y - y);

                            target.line(
                                line_x1 as f32,
                                line_y1 as f32,
                                line_x2 as f32,
                                line_y2 as f32,
                                Color::RGBA(255, 255, 64, alpha / 2),
                            );
                        }

                        transform.pop();
                    };

                    let lerp_x = pos.x + vel.map_or(0.0, |v| v.x) * partial_ticks;
                    let lerp_y = pos.y + vel.map_or(0.0, |v| v.y) * partial_ticks;
                    draw(lerp_x, lerp_y, 255);
                    draw(pos.x, pos.y, 80);
                });

            let (entities, position_storage, velocity_storage, player_storage) =
                world.ecs.system_data::<(
                    specs::Entities,
                    ReadStorage<Position>,
                    ReadStorage<Velocity>,
                    ReadStorage<Player>,
                )>();

            (&entities, &player_storage)
                .join()
                .for_each(|(ent, player)| match &player.movement {
                    PlayerMovementMode::Normal { grapple_state, .. } => {
                        let mut draw_grapple = |grapple: &specs::Entity, pivots: &Vec<Position>| {
                            let player_pos = position_storage
                                .get(ent)
                                .expect("Missing Position on Player");
                            let grapple_pos = position_storage
                                .get(*grapple)
                                .expect("Missing Position on grapple");
                            let player_vel = velocity_storage
                                .get(ent)
                                .expect("Missing Velocity on Player");
                            let grapple_vel = velocity_storage
                                .get(*grapple)
                                .expect("Missing Velocity on grapple");

                            target.set_line_thickness(2.0);
                            if pivots.is_empty() {
                                let (x1, y1) = transform.transform((
                                    player_pos.x + player_vel.x * partial_ticks,
                                    player_pos.y + player_vel.y * partial_ticks,
                                ));
                                let (x2, y2) = transform.transform((
                                    grapple_pos.x + grapple_vel.x * partial_ticks,
                                    grapple_pos.y + grapple_vel.y * partial_ticks,
                                ));

                                target.line(
                                    x1 as f32,
                                    y1 as f32,
                                    x2 as f32,
                                    y2 as f32,
                                    Color::RGBA(191, 191, 191, 255),
                                );
                            } else {
                                {
                                    let (x1, y1) = transform.transform((
                                        grapple_pos.x + grapple_vel.x * partial_ticks,
                                        grapple_pos.y + grapple_vel.y * partial_ticks,
                                    ));
                                    let (x2, y2) = transform.transform((pivots[0].x, pivots[0].y));
                                    target.line(
                                        x1 as f32,
                                        y1 as f32,
                                        x2 as f32,
                                        y2 as f32,
                                        Color::RGBA(191, 191, 191, 255),
                                    );
                                }

                                if pivots.len() > 1 {
                                    for i in 1..pivots.len() {
                                        let p1 = &pivots[i - 1];
                                        let p2 = &pivots[i];
                                        let (x1, y1) = transform.transform((p1.x, p1.y));
                                        let (x2, y2) = transform.transform((p2.x, p2.y));

                                        target.line(
                                            x1 as f32,
                                            y1 as f32,
                                            x2 as f32,
                                            y2 as f32,
                                            Color::RGBA(191, 191, 191, 255),
                                        );
                                    }
                                }

                                {
                                    let (x1, y1) = transform.transform((
                                        pivots[pivots.len() - 1].x,
                                        pivots[pivots.len() - 1].y,
                                    ));
                                    let (x2, y2) = transform.transform((
                                        player_pos.x + player_vel.x * partial_ticks,
                                        player_pos.y + player_vel.y * partial_ticks,
                                    ));
                                    target.line(
                                        x1 as f32,
                                        y1 as f32,
                                        x2 as f32,
                                        y2 as f32,
                                        Color::RGBA(191, 191, 191, 255),
                                    );
                                }
                            }
                            target.set_line_thickness(1.0);
                        };

                        match grapple_state {
                            PlayerGrappleState::Out { entity, pivots, .. } => {
                                draw_grapple(entity, pivots);
                            },
                            PlayerGrappleState::Cancelled { entity } => {
                                draw_grapple(entity, &vec![]);
                            },
                            PlayerGrappleState::Ready | PlayerGrappleState::Used => (),
                        }
                    },
                    PlayerMovementMode::Free => (),
                });
        }
        // canvas.set_clip_rect(clip);

        if settings.debug && settings.draw_chunk_grid {
            for x in -10..10 {
                for y in -10..10 {
                    let rc_x = x + (camera_pos.x / f64::from(CHUNK_SIZE)) as i32;
                    let rc_y = y + (camera_pos.y / f64::from(CHUNK_SIZE)) as i32;
                    let rc = Rect::new(
                        rc_x * i32::from(CHUNK_SIZE),
                        rc_y * i32::from(CHUNK_SIZE),
                        u32::from(CHUNK_SIZE),
                        u32::from(CHUNK_SIZE),
                    );
                    target.rectangle2(transform.transform_rect(rc), Color::RGBA(64, 64, 64, 127));
                }
            }
        }

        if settings.debug && settings.draw_origin {
            let len: f32 = 16.0;
            let origin = transform.transform((0, 0));
            target.rectangle_filled2(
                GPURect::new(
                    origin.0 as f32 - len - 2.0,
                    origin.1 as f32 - 1.0,
                    (len * 2.0 + 4.0) as f32,
                    3.0,
                ),
                Color::RGBA(0, 0, 0, 127),
            );
            target.rectangle_filled2(
                GPURect::new(
                    origin.0 as f32 - 1.0,
                    origin.1 as f32 - len - 2.0,
                    3.0,
                    (len * 2.0 + 4.0) as f32,
                ),
                Color::RGBA(0, 0, 0, 127),
            );

            target.line(
                origin.0 as f32 - len,
                origin.1 as f32,
                origin.0 as f32 + len,
                origin.1 as f32,
                Color::RGBA(255, 0, 0, 255),
            );
            target.line(
                origin.0 as f32,
                origin.1 as f32 - len,
                origin.0 as f32,
                origin.1 as f32 + len,
                Color::RGBA(0, 255, 0, 255),
            );
        }

        if settings.debug && settings.draw_load_zones {
            target.rectangle2(
                transform.transform_rect(unload_zone),
                Color::RGBA(255, 0, 0, 127),
            );
            target.rectangle2(
                transform.transform_rect(load_zone),
                Color::RGBA(255, 127, 0, 127),
            );
            target.rectangle2(
                transform.transform_rect(active_zone),
                Color::RGBA(255, 255, 0, 127),
            );
            target.rectangle2(
                transform.transform_rect(screen_zone),
                Color::RGBA(0, 255, 0, 127),
            );
        }

        transform.pop();

        // draw overlay
    }

    pub fn mark_liquid_dirty(&mut self) {
        self.physics_dirty = true;
    }
}
