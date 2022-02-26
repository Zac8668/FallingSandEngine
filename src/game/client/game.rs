use std::{
    io::{BufReader, Read},
    net::TcpStream,
    time::{Duration, Instant},
};

use clap::ArgMatches;
use liquidfun::box2d::{
    collision::shapes::polygon_shape::PolygonShape,
    common::math::Vec2,
    dynamics::{
        body::{BodyDef, BodyType},
        fixture::FixtureDef,
    },
};
use log::{debug, error, info, warn};
use sdl2::{
    event::{Event, WindowEvent},
    keyboard::Keycode,
    sys::SDL_WindowFlags,
    video::{FullscreenType, SwapInterval},
};
use sdl_gpu::GPUSubsystem;
use specs::{Builder, Join, ReadStorage, WorldExt, WriteStorage};
use sysinfo::{Pid, ProcessExt, SystemExt};

use crate::game::{
    client::world::ClientWorld,
    common::{
        networking::{Packet, PacketType},
        world::{
            entity::{GameEntity, Hitbox, Persistent, PhysicsEntity, Player, PlayerMovementMode},
            material::MaterialInstance,
            B2BodyComponent, Camera, ChunkHandlerGeneric, CollisionFlags, Loader, Position,
            Velocity, World, WorldNetworkMode, LIQUIDFUN_SCALE,
        },
        Settings,
    },
    Game,
};

use super::{
    render::{Renderer, Sdl2Context},
    world::ClientChunk,
};

impl Game<ClientChunk> {
    #[profiling::function]
    pub fn run(
        &mut self,
        sdl: &Sdl2Context,
        mut renderer: Option<&mut Renderer>,
        args: &ArgMatches,
    ) {
        self.settings.debug = args.is_present("debug");
        if args.is_present("no-tick") {
            self.settings.simulate_chunks = false;
            self.settings.simulate_particles = false;
            self.settings.tick_lqf = false;
        }

        let mut network = None;

        if let Some(addr) = args.value_of("connect") {
            info!("Connecting to {}...", addr);
            match TcpStream::connect(addr).map(BufReader::new) {
                Ok(mut r) => {
                    info!("[CLIENT] Connected to server");

                    r.get_mut().set_nonblocking(true).unwrap();
                    self.world.as_mut().unwrap().net_mode = WorldNetworkMode::Remote;

                    network = Some(r);
                }
                Err(e) => {
                    error!("[CLIENT] Failed to connect to server: {}", e);
                }
            }
        }

        let mut prev_tick_time = std::time::Instant::now();
        let mut prev_tick_lqf_time = std::time::Instant::now();

        let mut event_pump = sdl.sdl.event_pump().unwrap();

        let mut shift_key = false;

        let mut last_frame = Instant::now();

        let mut counter_last_frame = Instant::now();

        let mut sys = sysinfo::System::new();

        let mut do_tick_next = false;
        let mut do_tick_lqf_next = false;

        let mut bytes_to_read: Option<u32> = None;
        let mut read_buffer: Option<Vec<u8>> = None;

        'mainLoop: loop {
            for event in event_pump.poll_iter() {
                if let Some(r) = &mut renderer {
                    r.imgui_sdl2.handle_event(&mut r.imgui, &event);
                    // missing from official support
                    // if r.imgui_sdl2.ignore_event(&event) {
                    //     continue;
                    // }
                }

                let client_consumed_event = match &mut self.client {
                    Some(c) => c.on_event(&event),
                    None => false,
                };

                if !client_consumed_event {
                    match event {
                        Event::Quit { .. }
                        | Event::KeyDown { keycode: Some(Keycode::Escape), .. } => break 'mainLoop,
                        Event::KeyDown { keycode: Some(Keycode::F11), .. } => {
                            self.settings.fullscreen = !self.settings.fullscreen;
                        }
                        Event::KeyDown {
                            keycode: Some(Keycode::RShift | Keycode::LShift), ..
                        } => {
                            shift_key = true;
                        }
                        Event::KeyUp {
                            keycode: Some(Keycode::RShift | Keycode::LShift), ..
                        } => {
                            shift_key = false;
                        }
                        Event::MouseWheel { y, .. } => {
                            if let Some(c) = &mut self.client {
                                if shift_key {
                                    let mut v = c.camera_scale + 0.1 * f64::from(y);
                                    if y > 0 {
                                        v = v.ceil();
                                    } else {
                                        v = v.floor();
                                    }

                                    v = v.clamp(1.0, 10.0);
                                    c.camera_scale = v;
                                } else {
                                    c.camera_scale = (c.camera_scale * (1.0 + 0.1 * f64::from(y)))
                                        .clamp(0.01, 10.0);
                                }
                            }
                        }
                        Event::MouseButtonDown {
                            mouse_btn: sdl2::mouse::MouseButton::Right,
                            x,
                            y,
                            ..
                        } => {
                            if let Some(w) = &mut self.world {
                                if let Some(ref r) = renderer {
                                    if let Some(ref mut c) = &mut self.client {
                                        let (
                                            position_storage,
                                            camera_storage,
                                        ) = w.ecs.system_data::<(
                                            ReadStorage<Position>,
                                            ReadStorage<Camera>,
                                        )>();

                                        let camera_pos = (&position_storage, &camera_storage)
                                            .join()
                                            .find_map(|(p, _c)| Some(p));

                                        if let Some(camera_pos) = camera_pos {
                                            let world_x = camera_pos.x
                                                + (f64::from(x)
                                                    - f64::from(r.window.size().0) / 2.0)
                                                    / c.camera_scale;
                                            let world_y = camera_pos.y
                                                + (f64::from(y)
                                                    - f64::from(r.window.size().1) / 2.0)
                                                    / c.camera_scale;
                                            // let (chunk_x, chunk_y) = w.chunk_handler.pixel_to_chunk_pos(world_x as i64, world_y as i64);
                                            // w.chunk_handler.force_update_chunk(chunk_x, chunk_y);

                                            if let Some(mj) =
                                                w.lqf_world.mouse_joint_begin(Vec2::new(
                                                    world_x as f32 / LIQUIDFUN_SCALE,
                                                    world_y as f32 / LIQUIDFUN_SCALE,
                                                ))
                                            {
                                                let mj: liquidfun::box2d::dynamics::joints::mouse_joint::MouseJoint = mj;
                                                c.mouse_joint = Some(mj);
                                                debug!("made mouse joint");
                                            } else {
                                                c.mouse_joint = None;
                                                debug!("failed to make mouse joint");
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Event::MouseButtonUp {
                            mouse_btn: sdl2::mouse::MouseButton::Right, ..
                        } => {
                            if let Some(w) = &mut self.world {
                                if let Some(ref mut c) = &mut self.client {
                                    if let Some(mj) = &c.mouse_joint {
                                        w.lqf_world.destroy_mouse_joint(mj);
                                    }
                                    c.mouse_joint = None;
                                }
                            }
                        }
                        Event::MouseMotion { xrel, yrel, mousestate, x, y, .. } => {
                            if mousestate.left() {
                                if let Some(w) = &mut self.world {
                                    if let Some(c) = &mut self.client {
                                        let (
                                            mut position_storage,
                                            camera_storage,
                                        ) = w.ecs.system_data::<(
                                            WriteStorage<Position>,
                                            ReadStorage<Camera>,
                                        )>();

                                        let camera_pos = (&mut position_storage, &camera_storage)
                                            .join()
                                            .find_map(|(p, _c)| Some(p));

                                        if let Some(camera_pos) = camera_pos {
                                            // this doesn't do anything if game.client_entity_id exists
                                            //     since the renderer will snap the camera to the client entity
                                            camera_pos.x -= f64::from(xrel) / c.camera_scale;
                                            camera_pos.y -= f64::from(yrel) / c.camera_scale;
                                        }
                                    }
                                }
                            } else if mousestate.middle() {
                                if let Some(w) = &mut self.world {
                                    if let Some(ref c) = &mut self.client {
                                        if let Some(ref r) = renderer {
                                            let (
                                                position_storage,
                                                camera_storage,
                                            ) = w.ecs.system_data::<(
                                                ReadStorage<Position>,
                                                ReadStorage<Camera>,
                                            )>();

                                            let camera_pos = (&position_storage, &camera_storage)
                                                .join()
                                                .find_map(|(p, _c)| Some(p));

                                            if let Some(camera_pos) = camera_pos {
                                                let world_x = camera_pos.x
                                                    + (f64::from(x)
                                                        - f64::from(r.window.size().0) / 2.0)
                                                        / c.camera_scale;
                                                let world_y = camera_pos.y
                                                    + (f64::from(y)
                                                        - f64::from(r.window.size().1) / 2.0)
                                                        / c.camera_scale;

                                                for xx in -3..=3 {
                                                    for yy in -3..=3 {
                                                        let _ = w.chunk_handler.set(
                                                            world_x as i64 + xx,
                                                            world_y as i64 + yy,
                                                            MaterialInstance::air(),
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            } else if mousestate.right() {
                                if let Some(w) = &mut self.world {
                                    if let Some(ref r) = renderer {
                                        if let Some(ref mut c) = &mut self.client {
                                            let (
                                                position_storage,
                                                camera_storage,
                                            ) = w.ecs.system_data::<(
                                                ReadStorage<Position>,
                                                ReadStorage<Camera>,
                                            )>();

                                            let camera_pos = (&position_storage, &camera_storage)
                                                .join()
                                                .find_map(|(p, _c)| Some(p));

                                            if let Some(camera_pos) = camera_pos {
                                                let world_x = camera_pos.x
                                                    + (f64::from(x)
                                                        - f64::from(r.window.size().0) / 2.0)
                                                        / c.camera_scale;
                                                let world_y = camera_pos.y
                                                    + (f64::from(y)
                                                        - f64::from(r.window.size().1) / 2.0)
                                                        / c.camera_scale;
                                                if let Some(mj) = &mut c.mouse_joint {
                                                    mj.set_target(Vec2::new(
                                                        world_x as f32 / LIQUIDFUN_SCALE,
                                                        world_y as f32 / LIQUIDFUN_SCALE,
                                                    ));
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Event::Window { win_event: WindowEvent::Resized(w, h), .. } => {
                            #[allow(clippy::cast_sign_loss)]
                            GPUSubsystem::set_window_resolution(w as u16, h as u16);
                        }
                        _ => {}
                    }
                }
            }

            let now = std::time::Instant::now();
            let delta = now.saturating_duration_since(last_frame);
            last_frame = now;
            if let Some(r) = &mut renderer {
                r.imgui_sdl2
                    .prepare_frame(&mut r.imgui, &r.window, &event_pump);

                let delta_s =
                    delta.as_secs() as f32 + delta.subsec_nanos() as f32 / 1_000_000_000.0;
                r.imgui.io_mut().delta_time = delta_s;
            }

            if let Some(r) = &mut renderer {
                let fs = r.window.fullscreen_state();

                let des_fs = match self.settings {
                    Settings { fullscreen, fullscreen_type, .. }
                        if fullscreen && fullscreen_type == 0 =>
                    {
                        FullscreenType::Desktop
                    }
                    Settings { fullscreen, fullscreen_type, .. }
                        if fullscreen && fullscreen_type != 0 =>
                    {
                        FullscreenType::True
                    }
                    _ => FullscreenType::Off,
                };

                if fs != des_fs {
                    debug!("{:?}", des_fs);

                    if des_fs == FullscreenType::True {
                        r.window.set_fullscreen(FullscreenType::Off).unwrap();
                        r.window.maximize();
                    } else if des_fs == FullscreenType::Desktop {
                        r.window.restore();
                    }

                    r.window.set_fullscreen(des_fs).unwrap();

                    if des_fs == FullscreenType::Off {
                        r.window.restore();
                        GPUSubsystem::set_window_resolution(
                            r.window.size().0 as u16,
                            r.window.size().1 as u16,
                        );
                    }
                }

                if sdl2::hint::get_video_minimize_on_focus_loss()
                    != self.settings.minimize_on_lost_focus
                {
                    sdl2::hint::set_video_minimize_on_focus_loss(
                        self.settings.minimize_on_lost_focus,
                    );
                }

                let si_des = if self.settings.vsync {
                    SwapInterval::VSync
                } else {
                    SwapInterval::Immediate
                };
                if sdl.sdl_video.gl_get_swap_interval() != si_des {
                    sdl.sdl_video.gl_set_swap_interval(si_des).unwrap();
                }
            }

            // tick

            let mut can_tick = self.settings.tick;

            if let Some(r) = &renderer {
                let flags = r.window.window_flags();
                can_tick = can_tick
                    && !(self.settings.pause_on_lost_focus
                        && renderer.is_some()
                        && !(flags & SDL_WindowFlags::SDL_WINDOW_INPUT_FOCUS as u32
                            == SDL_WindowFlags::SDL_WINDOW_INPUT_FOCUS as u32));
            }

            if do_tick_next && can_tick {
                prev_tick_time = now;
                let st = Instant::now();
                self.tick();

                if let Some(client) = &mut self.client {
                    for act in client.main_menu.action_queue.drain(..) {
                        match act {
                            crate::game::client::ui::MainMenuAction::Quit => {
                                break 'mainLoop;
                            }
                            crate::game::client::ui::MainMenuAction::LoadWorld(path) => {
                                let world_meta = crate::game::common::world::World::<ClientChunk>::parse_file_meta(path.clone()).expect("Failed to parse file meta");
                                if let Some(w) = &mut self.world {
                                    info!("Unload current world...");
                                    w.save().expect("World save failed");
                                    w.close().expect("World unload failed");
                                }

                                info!("Load world \"{}\"...", world_meta.name);
                                self.world = Some(World::create(Some(
                                    path.parent()
                                        .expect("World meta file has no parent directory ??")
                                        .to_path_buf(),
                                )));

                                let body_def = BodyDef {
                                    body_type: BodyType::DynamicBody,
                                    fixed_rotation: true,
                                    gravity_scale: 0.0,
                                    bullet: true,
                                    ..BodyDef::default()
                                };
                                let body = self
                                    .world
                                    .as_mut()
                                    .unwrap()
                                    .lqf_world
                                    .create_body(&body_def);
                                let mut dynamic_box = PolygonShape::new();
                                dynamic_box.set_as_box(
                                    12.0 / LIQUIDFUN_SCALE / 2.0,
                                    20.0 / LIQUIDFUN_SCALE / 2.0,
                                );
                                let mut fixture_def = FixtureDef::new(&dynamic_box);
                                fixture_def.density = 1.5;
                                fixture_def.friction = 0.3;
                                fixture_def.filter.category_bits = CollisionFlags::PLAYER.bits();
                                fixture_def.filter.mask_bits =
                                    (CollisionFlags::RIGIDBODY | CollisionFlags::ENTITY).bits();
                                body.create_fixture(&fixture_def);

                                if let Some(w) = &mut self.world {
                                    let player = w
                                        .ecs
                                        .create_entity()
                                        .with(Player { movement: PlayerMovementMode::Free })
                                        .with(GameEntity)
                                        .with(PhysicsEntity {
                                            on_ground: false,
                                            gravity: 0.1,
                                            edge_clip_distance: 2.0,
                                            collision: true,
                                            collide_with_sand: true,
                                        })
                                        .with(Persistent)
                                        .with(Position { x: 0.0, y: -20.0 })
                                        .with(Velocity { x: 0.0, y: 0.0 })
                                        .with(Hitbox { x1: -6.0, y1: -10.0, x2: 6.0, y2: 10.0 })
                                        .with(Loader)
                                        .with(B2BodyComponent::of(body))
                                        .build();

                                    client.world = Some(ClientWorld { local_entity: Some(player) });
                                };
                            }
                        }
                    }
                }

                if let Some(stream) = &mut network {
                    let start = Instant::now();

                    // let mut n = 0;
                    while Instant::now().saturating_duration_since(start).as_nanos() < 5_000_000 {
                        if bytes_to_read.is_none() {
                            let mut buf = [0; 4];
                            if stream.read_exact(&mut buf).is_ok() {
                                let size: u32 = bincode::deserialize(&buf).unwrap();
                                // println!("[CLIENT] Incoming packet, size = {}.", size);

                                bytes_to_read = Some(size);
                                read_buffer = Some(Vec::with_capacity(size as usize));
                            }
                        }

                        if let (Some(size), Some(buf)) = (bytes_to_read, &mut read_buffer) {
                            // println!("[CLIENT] size = {}", size);
                            if size == 0 {
                                // trying_to_read = None;
                                panic!("[CLIENT] Zero length packet.");
                            } else {
                                assert!(size <= 2_000_000, "[CLIENT] Almost tried to read packet that is too big ({} bytes)", size);

                                // let mut buf = vec![0; size as usize];

                                // println!("[CLIENT] read_to_end...");
                                let prev_size = buf.len();
                                match std::io::Read::by_ref(stream)
                                    .take(u64::from(size))
                                    .read_to_end(buf)
                                {
                                    // match stream.read_exact(&mut buf) {
                                    Ok(read) => {
                                        if read != size as usize {
                                            warn!(
                                                "[CLIENT] Couldn't read enough bytes! Read {}/{}.",
                                                read, size
                                            );
                                        }

                                        // println!("[CLIENT] Read {}/{} bytes", read, buf.len());

                                        bytes_to_read = None;

                                        // println!("[CLIENT] Read {} bytes.", buf.len());
                                        match bincode::deserialize::<Packet>(buf) {
                                            // match serde_json::from_slice::<Packet>(&buf) {
                                            Ok(p) => {
                                                // n += 1;
                                                #[allow(unreachable_patterns)]
                                                match p.packet_type {
                                                    PacketType::SyncChunkPacket {
                                                        chunk_x,
                                                        chunk_y,
                                                        pixels,
                                                        colors,
                                                    } => {
                                                        if let Some(w) = &mut self.world {
                                                            if let Err(e) = w.sync_chunk(
                                                                chunk_x, chunk_y, pixels, colors,
                                                            ) {
                                                                warn!("[CLIENT] sync_chunk failed: {}", e);
                                                            }
                                                        }
                                                    }
                                                    PacketType::SyncLiquidFunPacket {
                                                        positions,
                                                        velocities,
                                                    } => {
                                                        // println!("[CLIENT] Got SyncLiquidFunPacket");
                                                        if let Some(w) = &mut self.world {
                                                            let mut particle_system = w
                                                                .lqf_world
                                                                .get_particle_system_list()
                                                                .unwrap();

                                                            let particle_count = particle_system
                                                                .get_particle_count()
                                                                as usize;
                                                            // let particle_colors: &[b2ParticleColor] = particle_system.get_color_buffer();
                                                            let particle_positions: &mut [Vec2] =
                                                                particle_system
                                                                    .get_position_buffer_mut();
                                                            for i in 0..particle_count
                                                                .min(positions.len())
                                                            {
                                                                let dx = positions[i].x
                                                                    - particle_positions[i].x;
                                                                let dy = positions[i].y
                                                                    - particle_positions[i].y;

                                                                if dx.abs() > 1.0 || dy.abs() > 1.0
                                                                {
                                                                    particle_positions[i].x += dx;
                                                                    particle_positions[i].y += dy;
                                                                } else {
                                                                    particle_positions[i].x +=
                                                                        dx / 2.0;
                                                                    particle_positions[i].y +=
                                                                        dy / 2.0;
                                                                }
                                                            }

                                                            let particle_velocities: &mut [Vec2] =
                                                                particle_system
                                                                    .get_velocity_buffer_mut();
                                                            for i in 0..particle_count
                                                                .min(positions.len())
                                                            {
                                                                particle_velocities[i].x =
                                                                    velocities[i].x;
                                                                particle_velocities[i].y =
                                                                    velocities[i].y;
                                                            }
                                                        }
                                                    }
                                                    _ => {}
                                                }
                                            }
                                            Err(e) => {
                                                warn!(
                                                    "[CLIENT] Failed to deserialize packet: {}",
                                                    e
                                                );
                                                // println!("[CLIENT]     Raw: {:?}", buf);
                                                // let s = String::from_utf8(buf);
                                                // match s {
                                                //     Ok(st) => {
                                                //         // println!("[CLIENT]     Raw: {} <- raw", st)
                                                //         let mut file = std::fs::File::create("data.dat").expect("create failed");
                                                //         file.write_all(&st.into_bytes()).expect("write failed");
                                                //         panic!("[CLIENT] See data.dat");
                                                //     },
                                                //     Err(e) => {
                                                //         let index = e.utf8_error().valid_up_to();
                                                //         let len = e.utf8_error().error_len().unwrap();
                                                //         let sl = &e.as_bytes()[index .. index + len];

                                                //         let mut file = std::fs::File::create("data.dat").expect("create failed");
                                                //         file.write_all(e.as_bytes()).expect("write failed");

                                                //         panic!("[CLIENT] See data.dat: {} : {:?}", e, sl);
                                                //     },
                                                // }
                                            }
                                        };
                                        // println!("[CLIENT] Recieved packet : {:?}", match p.packet_type {
                                        //     PacketType::SyncChunkPacket{..} => "SyncChunkPacket",
                                        //     _ => "???",
                                        // });
                                    }
                                    Err(_e) => {
                                        let read = buf.len() - prev_size;
                                        // println!("[CLIENT] read_to_end failed (but read {} bytes): {}", read, e);
                                        bytes_to_read = Some(size - read as u32);
                                    }
                                }
                            }
                        }
                    }
                    // println!("[CLIENT] Handled {} packets.", n);
                }

                self.fps_counter.tick_times.rotate_left(1);
                self.fps_counter.tick_times[self.fps_counter.tick_times.len() - 1] =
                    Instant::now().saturating_duration_since(st).as_nanos() as f32;
            }
            do_tick_next = can_tick
                && now.saturating_duration_since(prev_tick_time).as_nanos()
                    > 1_000_000_000 / u128::from(self.settings.tick_speed); // intended is 30 ticks per second

            // tick liquidfun

            let mut can_tick = self.settings.tick_lqf;

            if let Some(r) = &renderer {
                let flags = r.window.window_flags();
                can_tick = can_tick
                    && !(self.settings.pause_on_lost_focus
                        && renderer.is_some()
                        && !(flags & SDL_WindowFlags::SDL_WINDOW_INPUT_FOCUS as u32
                            == SDL_WindowFlags::SDL_WINDOW_INPUT_FOCUS as u32));
            }

            if do_tick_lqf_next && can_tick {
                prev_tick_lqf_time = now;
                if let Some(w) = &mut self.world {
                    let st = Instant::now();
                    w.tick_lqf(&self.settings);
                    self.fps_counter.tick_lqf_times.rotate_left(1);
                    self.fps_counter.tick_lqf_times[self.fps_counter.tick_lqf_times.len() - 1] =
                        Instant::now().saturating_duration_since(st).as_nanos() as f32;
                    if let Some(r) = &mut renderer {
                        r.world_renderer.mark_liquid_dirty();
                    }
                }
            }
            do_tick_lqf_next = can_tick
                && now.saturating_duration_since(prev_tick_lqf_time).as_nanos()
                    > 1_000_000_000 / u128::from(self.settings.tick_lqf_speed); // intended is 60 ticks per second

            // render

            if let Some(w) = &mut self.world {
                w.frame(delta); // this delta is more accurate than the one based on counter_last_frame
            }

            if let Some(r) = &mut renderer {
                profiling::scope!("rendering");

                let partial_ticks = (now.saturating_duration_since(prev_tick_time).as_secs_f64()
                    / (1.0 / f64::from(self.settings.tick_speed)))
                .clamp(0.0, 1.0);
                let delta_time = Instant::now().saturating_duration_since(counter_last_frame);
                self.render(r, sdl, delta_time.as_secs_f64(), partial_ticks);

                self.frame_count += 1;
                self.fps_counter.frames += 1;
                if now
                    .saturating_duration_since(self.fps_counter.last_update)
                    .as_millis()
                    >= 1000
                {
                    self.fps_counter.display_value = self.fps_counter.frames;
                    self.fps_counter.frames = 0;
                    self.fps_counter.last_update = now;
                    let set = r.window.set_title(
                        format!(
                            "FallingSandRust ({} FPS) ({})",
                            self.fps_counter.display_value,
                            self.world.as_ref().map_or_else(
                                || "unknown".to_owned(),
                                |w| format!("{:?}", w.net_mode)
                            )
                        )
                        .as_str(),
                    );
                    if set.is_err() {
                        error!("Failed to set window title.");
                    }

                    sys.refresh_process(std::process::id() as Pid);
                    if let Some(pc) = sys.process(std::process::id() as Pid) {
                        self.process_stats.cpu_usage =
                            Some(pc.cpu_usage() / sys.processors().len() as f32);
                        self.process_stats.memory = Some(pc.memory());
                    }
                }
            }

            let time_nano = Instant::now()
                .saturating_duration_since(counter_last_frame)
                .as_nanos();
            self.fps_counter.frame_times.rotate_left(1);
            self.fps_counter.frame_times[self.fps_counter.frame_times.len() - 1] = time_nano as f32;

            profiling::finish_frame!();
            // sleep
            if !do_tick_next {
                ::std::thread::sleep(Duration::new(0, 1_000_000)); // 1ms sleep so the computer doesn't explode
            }
            counter_last_frame = Instant::now();
        }

        if let Some(w) = &mut self.world {
            info!("Unload current world...");
            w.save().expect("World save failed");
            w.close().expect("World unload failed");
        }

        info!("Closing...");
    }

    pub fn render(
        &mut self,
        renderer: &mut Renderer,
        sdl: &Sdl2Context,
        delta_time: f64,
        partial_ticks: f64,
    ) {
        renderer.render(sdl, self, delta_time, partial_ticks);
    }

    #[profiling::function]
    fn tick(&mut self) {
        self.tick_time += 1;

        if let Some(w) = &mut self.world {
            w.tick(self.tick_time, &self.settings);
            if let Some(cw) = &mut self.client {
                cw.tick(w);
            }
        }
    }
}
