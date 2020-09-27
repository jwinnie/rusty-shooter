#![deny(unsafe_code)]
#![deny(unused_must_use)]

extern crate rand;
extern crate rg3d;
extern crate serde;
extern crate serde_json;

mod actor;
mod bot;
mod character;
mod control_scheme;
mod effects;
mod gui;
mod hud;
mod item;
mod jump_pad;
mod leader_board;
mod level;
mod match_menu;
mod menu;
mod message;
mod options_menu;
mod player;
mod projectile;
mod settings;
mod weapon;

use crate::{
    actor::Actor, control_scheme::ControlScheme, hud::Hud, level::Level, menu::Menu,
    message::Message, settings::Settings,
};
use rg3d::engine::resource_manager::ResourceManager;
use rg3d::gui::message::MessageDirection;
use rg3d::{
    core::{
        color::Color,
        pool::Handle,
        visitor::{Visit, VisitResult, Visitor},
    },
    engine::Engine,
    event::{DeviceEvent, ElementState, Event, VirtualKeyCode, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    gui::{
        message::TextMessage,
        message::UiMessage,
        node::{StubNode, UINode},
        text::TextBuilder,
        widget::WidgetBuilder,
        UserInterface,
    },
    sound::{
        context::Context,
        effects::{BaseEffect, Effect, EffectInput},
        source::{
            generic::GenericSourceBuilder, spatial::SpatialSourceBuilder, SoundSource, Status,
        },
    },
    utils::translate_event,
};
use std::sync::{Arc, Mutex};
use std::{
    cell::RefCell,
    fs::File,
    io::Write,
    path::Path,
    rc::Rc,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{self, Duration, Instant},
};

const FIXED_FPS: f32 = 60.0;
const SETTINGS_FILE: &'static str = "settings.json";

// Define type aliases for engine structs.
pub type UiNode = UINode<(), StubNode>;
pub type UINodeHandle = Handle<UiNode>;
pub type GameEngine = Engine<(), StubNode>;
pub type Gui = UserInterface<(), StubNode>;
pub type GuiMessage = UiMessage<(), StubNode>;
pub type BuildContext<'a> = rg3d::gui::BuildContext<'a, (), StubNode>;

pub struct Game {
    menu: Menu,
    hud: Hud,
    engine: GameEngine,
    level: Option<Level>,
    debug_text: UINodeHandle,
    debug_string: String,
    last_tick_time: time::Instant,
    running: bool,
    control_scheme: Rc<RefCell<ControlScheme>>,
    time: GameTime,
    events_receiver: Receiver<Message>,
    events_sender: Sender<Message>,
    sound_manager: SoundManager,
}

#[derive(Copy, Clone)]
pub struct GameTime {
    clock: time::Instant,
    elapsed: f64,
    delta: f32,
}

// Disable false-positive lint, isize *is* portable.
#[allow(clippy::enum_clike_unportable_variant)]
pub enum CollisionGroups {
    Generic = 1,
    Projectile = 1 << 1,
    Actor = 1 << 2,
    All = std::isize::MAX,
}

#[derive(Copy, Clone, Debug)]
pub struct DeathMatch {
    pub time_limit_secs: f32,
    pub frag_limit: u32,
}

impl Default for DeathMatch {
    fn default() -> Self {
        Self {
            time_limit_secs: Default::default(),
            frag_limit: 0,
        }
    }
}

impl Visit for DeathMatch {
    fn visit(&mut self, name: &str, visitor: &mut Visitor) -> VisitResult {
        visitor.enter_region(name)?;

        self.time_limit_secs.visit("TimeLimit", visitor)?;
        self.frag_limit.visit("FragLimit", visitor)?;

        visitor.leave_region()
    }
}

#[derive(Copy, Clone, Debug)]
pub struct TeamDeathMatch {
    pub time_limit_secs: f32,
    pub team_frag_limit: u32,
}

impl Default for TeamDeathMatch {
    fn default() -> Self {
        Self {
            time_limit_secs: Default::default(),
            team_frag_limit: 0,
        }
    }
}

impl Visit for TeamDeathMatch {
    fn visit(&mut self, name: &str, visitor: &mut Visitor) -> VisitResult {
        visitor.enter_region(name)?;

        self.time_limit_secs.visit("TimeLimit", visitor)?;
        self.team_frag_limit.visit("TeamFragLimit", visitor)?;

        visitor.leave_region()
    }
}

#[derive(Copy, Clone, Debug)]
pub struct CaptureTheFlag {
    pub time_limit_secs: f32,
    pub flag_limit: u32,
}

impl Default for CaptureTheFlag {
    fn default() -> Self {
        Self {
            time_limit_secs: Default::default(),
            flag_limit: 0,
        }
    }
}

impl Visit for CaptureTheFlag {
    fn visit(&mut self, name: &str, visitor: &mut Visitor) -> VisitResult {
        visitor.enter_region(name)?;

        self.time_limit_secs.visit("TimeLimit", visitor)?;
        self.flag_limit.visit("FlagLimit", visitor)?;

        visitor.leave_region()
    }
}

#[derive(Copy, Clone, Debug)]
pub enum MatchOptions {
    DeathMatch(DeathMatch),
    TeamDeathMatch(TeamDeathMatch),
    CaptureTheFlag(CaptureTheFlag),
}

impl MatchOptions {
    fn from_id(id: u32) -> Result<Self, String> {
        match id {
            0 => Ok(MatchOptions::DeathMatch(Default::default())),
            1 => Ok(MatchOptions::TeamDeathMatch(Default::default())),
            2 => Ok(MatchOptions::CaptureTheFlag(Default::default())),
            _ => Err(format!("Invalid match options {}", id)),
        }
    }

    fn id(&self) -> u32 {
        match self {
            MatchOptions::DeathMatch(_) => 0,
            MatchOptions::TeamDeathMatch(_) => 1,
            MatchOptions::CaptureTheFlag(_) => 2,
        }
    }
}

impl Default for MatchOptions {
    fn default() -> Self {
        MatchOptions::DeathMatch(Default::default())
    }
}

impl Visit for MatchOptions {
    fn visit(&mut self, name: &str, visitor: &mut Visitor) -> VisitResult {
        visitor.enter_region(name)?;

        let mut id = self.id();
        id.visit("Id", visitor)?;
        if visitor.is_reading() {
            *self = Self::from_id(id)?;
        }
        match self {
            MatchOptions::DeathMatch(o) => o.visit("Data", visitor)?,
            MatchOptions::TeamDeathMatch(o) => o.visit("Data", visitor)?,
            MatchOptions::CaptureTheFlag(o) => o.visit("Data", visitor)?,
        }

        visitor.leave_region()
    }
}

pub struct SoundManager {
    context: Arc<Mutex<Context>>,
    music: Handle<SoundSource>,
    reverb: Handle<Effect>,
}

impl SoundManager {
    pub fn new(context: Arc<Mutex<Context>>, resource_manager: &mut ResourceManager) -> Self {
        let buffer = resource_manager
            .request_sound_buffer("data/sounds/Antonio_Bizarro_Berzerker.ogg", true)
            .unwrap();
        let music = context.lock().unwrap().add_source(
            GenericSourceBuilder::new(buffer)
                .with_looping(true)
                .with_status(Status::Playing)
                .with_gain(0.25)
                .build_source()
                .unwrap(),
        );

        let mut base_effect = BaseEffect::default();
        base_effect.set_gain(0.7);
        let mut reverb = rg3d::sound::effects::reverb::Reverb::new(base_effect);
        reverb.set_decay_time(Duration::from_secs_f32(3.0));
        let reverb = context
            .lock()
            .unwrap()
            .add_effect(rg3d::sound::effects::Effect::Reverb(reverb));

        Self {
            context,
            music,
            reverb,
        }
    }

    pub fn handle_message(&mut self, resource_manager: &mut ResourceManager, message: &Message) {
        let mut context = self.context.lock().unwrap();

        match message {
            Message::PlaySound {
                path,
                position,
                gain,
                rolloff_factor,
                radius,
            } => {
                let shot_buffer = resource_manager.request_sound_buffer(path, false).unwrap();
                let shot_sound = SpatialSourceBuilder::new(
                    GenericSourceBuilder::new(shot_buffer)
                        .with_status(Status::Playing)
                        .with_play_once(true)
                        .with_gain(*gain)
                        .build()
                        .unwrap(),
                )
                .with_position(*position)
                .with_radius(*radius)
                .with_rolloff_factor(*rolloff_factor)
                .build_source();
                let source = context.add_source(shot_sound);
                context
                    .effect_mut(self.reverb)
                    .add_input(EffectInput::direct(source));
            }
            Message::SetMusicVolume { volume } => {
                context.source_mut(self.music).set_gain(*volume);
            }
            _ => {}
        }
    }
}

impl Visit for SoundManager {
    fn visit(&mut self, name: &str, visitor: &mut Visitor) -> VisitResult {
        visitor.enter_region(name)?;

        self.reverb.visit("Reverb", visitor)?;
        self.music.visit("Music", visitor)?;

        visitor.leave_region()
    }
}

impl Game {
    pub fn run() {
        let events_loop = EventLoop::<()>::new();

        let primary_monitor = events_loop.primary_monitor();
        let mut monitor_dimensions = primary_monitor.size();
        monitor_dimensions.height = (monitor_dimensions.height as f32 * 0.7) as u32;
        monitor_dimensions.width = (monitor_dimensions.width as f32 * 0.7) as u32;
        let inner_size = monitor_dimensions.to_logical::<f32>(primary_monitor.scale_factor());

        let window_builder = rg3d::window::WindowBuilder::new()
            .with_title("Rusty Shooter")
            .with_inner_size(inner_size)
            .with_resizable(true);

        let settings = settings::Settings::load_from_file(SETTINGS_FILE);

        let mut engine = GameEngine::new(window_builder, &events_loop, settings.renderer).unwrap();
        let hrtf_sphere = rg3d::sound::hrtf::HrtfSphere::new("data/sounds/IRC_1040_C.bin").unwrap();
        engine.sound_context.lock().unwrap().set_renderer(
            rg3d::sound::renderer::Renderer::HrtfRenderer(rg3d::sound::hrtf::HrtfRenderer::new(
                hrtf_sphere,
            )),
        );

        effects::register_custom_emitter_factory();

        engine.renderer.set_ambient_color(Color::opaque(60, 60, 60));

        let control_scheme = Rc::new(RefCell::new(settings.controls));

        let fixed_timestep = 1.0 / FIXED_FPS;

        let time = GameTime {
            clock: Instant::now(),
            elapsed: 0.0,
            delta: fixed_timestep,
        };

        let (tx, rx) = mpsc::channel();

        let sound_manager = SoundManager::new(
            engine.sound_context.clone(),
            &mut engine.resource_manager.lock().unwrap(),
        );

        let mut game = Game {
            sound_manager,
            hud: Hud::new(&mut engine),
            running: true,
            menu: Menu::new(&mut engine, control_scheme.clone(), tx.clone()),
            control_scheme,
            debug_text: Handle::NONE,
            engine,
            level: None,
            debug_string: String::new(),
            last_tick_time: time::Instant::now(),
            time,
            events_receiver: rx,
            events_sender: tx,
        };

        game.create_debug_ui();

        events_loop.run(move |event, _, control_flow| {
            game.process_input_event(&event);

            match event {
                Event::MainEventsCleared => {
                    let mut dt = game.time.clock.elapsed().as_secs_f64() - game.time.elapsed;
                    while dt >= fixed_timestep as f64 {
                        dt -= fixed_timestep as f64;
                        game.time.elapsed += fixed_timestep as f64;

                        game.update(game.time);

                        while let Some(ui_event) = game.engine.user_interface.poll_message() {
                            game.menu.handle_ui_event(&mut game.engine, &ui_event);
                        }
                    }
                    if !game.running {
                        game.exit_game(control_flow);
                    }
                    game.engine.get_window().request_redraw();
                }
                Event::RedrawRequested(_) => {
                    game.update_statistics(game.time.elapsed);

                    // <<<<< ENABLE THIS TO SHOW DEBUG GEOMETRY >>>>>
                    if false {
                        game.debug_render();
                    }

                    // Render at max speed
                    game.engine.render(fixed_timestep).unwrap();
                    // Make sure to cap update rate to 60 FPS.
                    game.limit_fps(FIXED_FPS as f64);
                }
                Event::WindowEvent { event, .. } => match event {
                    WindowEvent::CloseRequested => {
                        game.destroy_level();
                        game.exit_game(control_flow);
                    }
                    WindowEvent::Resized(new_size) => {
                        game.engine.renderer.set_frame_size(new_size.into());
                    }
                    _ => (),
                },
                Event::LoopDestroyed => {
                    rg3d::core::profiler::print();
                }
                _ => *control_flow = ControlFlow::Poll,
            }
        });
    }

    fn debug_render(&mut self) {
        self.engine.renderer.debug_renderer.clear_lines();

        if let Some(level) = self.level.as_mut() {
            level.debug_draw(&mut self.engine);
        }
    }

    pub fn create_debug_ui(&mut self) {
        self.debug_text = TextBuilder::new(WidgetBuilder::new().with_width(400.0))
            .build(&mut self.engine.user_interface.build_ctx());
    }

    pub fn save_game(&mut self) -> VisitResult {
        let mut visitor = Visitor::new();

        // Visit engine state first.
        self.engine.visit("GameEngine", &mut visitor)?;

        self.level.visit("Level", &mut visitor)?;

        self.sound_manager.visit("SoundManager", &mut visitor)?;

        // Debug output
        if let Ok(mut file) = File::create(Path::new("save.txt")) {
            file.write_all(visitor.save_text().as_bytes()).unwrap();
        }

        visitor.save_binary(Path::new("save.bin"))
    }

    pub fn load_game(&mut self) -> VisitResult {
        println!("Attempting load a save...");

        let mut visitor = Visitor::load_binary(Path::new("save.bin"))?;

        // Clean up.
        self.destroy_level();

        // Load engine state first
        println!("Trying to load engine state...");
        self.engine.visit("GameEngine", &mut visitor)?;

        println!("GameEngine state successfully loaded!");

        // Then load game state.
        self.level.visit("Level", &mut visitor)?;

        println!("Game state successfully loaded!");

        self.sound_manager.visit("SoundManager", &mut visitor)?;
        self.sound_manager.context = self.engine.sound_context.clone();

        // Hide menu only of we successfully loaded a save.
        self.set_menu_visible(false);

        // Set control scheme for player.
        if let Some(level) = &mut self.level {
            level.set_message_sender(self.events_sender.clone());
            level.build_navmesh(&mut self.engine);
            level.control_scheme = Some(self.control_scheme.clone());
            let player = level.get_player();
            if let Actor::Player(player) = level.actors_mut().get_mut(player) {
                player.set_control_scheme(self.control_scheme.clone());
            }
        }

        self.time.elapsed = self.time.clock.elapsed().as_secs_f64();

        Ok(())
    }

    fn destroy_level(&mut self) {
        if let Some(ref mut level) = self.level.take() {
            level.destroy(&mut self.engine);
            println!("Current level destroyed!");
        }
    }

    fn exit_game(&self, control_flow: &mut rg3d::event_loop::ControlFlow) {
        let settings = Settings {
            controls: self.control_scheme.borrow().clone(),
            renderer: self.engine.renderer.get_quality_settings(),
        };
        settings.write_to_file(SETTINGS_FILE);
        *control_flow = ControlFlow::Exit;
    }

    pub fn start_new_game(&mut self, options: MatchOptions) {
        self.destroy_level();
        self.level = Some(Level::new(
            &mut self.engine,
            self.control_scheme.clone(),
            self.events_sender.clone(),
            options,
        ));
        self.set_menu_visible(false);
    }

    pub fn set_menu_visible(&mut self, visible: bool) {
        let ui = &mut self.engine.user_interface;
        self.menu.set_visible(ui, visible);
        self.hud.set_visible(ui, !visible);
    }

    pub fn is_menu_visible(&self) -> bool {
        self.menu.is_visible(&self.engine.user_interface)
    }

    pub fn update(&mut self, time: GameTime) {
        let window = self.engine.get_window();
        window.set_cursor_visible(self.is_menu_visible());
        let _ = window.set_cursor_grab(!self.is_menu_visible());

        self.engine.update(time.delta);

        if let Some(ref mut level) = self.level {
            level.update(&mut self.engine, time);
            let ui = &mut self.engine.user_interface;
            self.hud.set_time(ui, level.time());
            let player = level.get_player();
            if player.is_some() {
                // Sync hud with player state.
                let player = level.actors().get(player);
                self.hud.set_health(ui, player.get_health());
                self.hud.set_armor(ui, player.get_armor());
                let current_weapon = player.current_weapon();
                if current_weapon.is_some() {
                    self.hud
                        .set_ammo(ui, level.weapons()[current_weapon].ammo());
                }
                self.hud.set_is_died(ui, false);
            } else {
                self.hud.set_is_died(ui, true);
            }
        }

        self.handle_messages(time);

        self.hud.update(&mut self.engine.user_interface, &self.time);
    }

    fn handle_messages(&mut self, time: GameTime) {
        while let Ok(message) = self.events_receiver.try_recv() {
            match &message {
                Message::StartNewGame { options } => {
                    self.start_new_game(*options);
                }
                Message::SaveGame => match self.save_game() {
                    Ok(_) => println!("successfully saved"),
                    Err(e) => println!("failed to make a save, reason: {}", e),
                },
                Message::LoadGame => {
                    if let Err(e) = self.load_game() {
                        println!("Failed to load saved game. Reason: {:?}", e);
                    }
                }
                Message::QuitGame => {
                    self.destroy_level();
                    self.running = false;
                }
                Message::EndMatch => {
                    self.destroy_level();
                    self.hud
                        .leader_board()
                        .set_visible(true, &mut self.engine.user_interface);
                }
                _ => (),
            }

            self.sound_manager
                .handle_message(&mut self.engine.resource_manager.lock().unwrap(), &message);

            if let Some(ref mut level) = self.level {
                level.handle_message(&mut self.engine, &message, time);

                self.hud.handle_message(
                    &message,
                    &mut self.engine.user_interface,
                    &level.leader_board,
                    &level.options,
                );
            }
        }
    }

    pub fn update_statistics(&mut self, elapsed: f64) {
        self.debug_string.clear();
        use std::fmt::Write;
        let statistics = self.engine.renderer.get_statistics();
        write!(
            self.debug_string,
            "Pure frame time: {:.2} ms\n\
               Capped frame time: {:.2} ms\n\
               FPS: {}\n\
               Triangles: {}\n\
               Draw calls: {}\n\
               Up time: {:.2} s\n\
               Sound render time: {:?}\n\
               UI Time: {:?}",
            statistics.pure_frame_time * 1000.0,
            statistics.capped_frame_time * 1000.0,
            statistics.frames_per_second,
            statistics.geometry.triangles_rendered,
            statistics.geometry.draw_calls,
            elapsed,
            self.engine
                .sound_context
                .lock()
                .unwrap()
                .full_render_duration(),
            self.engine.ui_time
        )
        .unwrap();

        self.engine.user_interface.send_message(TextMessage::text(
            self.debug_text,
            MessageDirection::ToWidget,
            self.debug_string.clone(),
        ));
    }

    pub fn limit_fps(&mut self, value: f64) {
        let current_time = time::Instant::now();
        let render_call_duration = current_time
            .duration_since(self.last_tick_time)
            .as_secs_f64();
        self.last_tick_time = current_time;
        let desired_frame_time = 1.0 / value;
        if render_call_duration < desired_frame_time {
            thread::sleep(Duration::from_secs_f64(
                desired_frame_time - render_call_duration,
            ));
        }
    }

    fn process_dispatched_event(&mut self, event: &Event<()>) {
        if let Event::WindowEvent { event, .. } = event {
            if let Some(event) = translate_event(event) {
                self.engine.user_interface.process_os_event(&event);
            }
        }

        if !self.is_menu_visible() {
            if let Some(ref mut level) = self.level {
                level.process_input_event(event);
            }
        }
    }

    pub fn process_input_event(&mut self, event: &Event<()>) {
        self.process_dispatched_event(event);

        if let Event::DeviceEvent { event, .. } = event {
            if let DeviceEvent::Key(input) = event {
                if let ElementState::Pressed = input.state {
                    if let Some(key) = input.virtual_keycode {
                        if key == VirtualKeyCode::Escape {
                            self.set_menu_visible(!self.is_menu_visible());
                        }
                    }
                }
            }
        }

        self.menu.process_input_event(&mut self.engine, &event);
        self.hud.process_event(&mut self.engine, &event);
    }
}

fn main() {
    Game::run();
}
