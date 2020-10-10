use crate::{
    actor::{Actor, TargetDescriptor},
    assets,
    character::Character,
    item::ItemContainer,
    level::UpdateContext,
    message::Message,
    weapon::WeaponContainer,
    GameTime,
};
use rand::Rng;
use rg3d::scene::SceneDrawingContext;
use rg3d::{
    animation::AnimationSignal,
    animation::{
        machine::{self, Machine, PoseNode, State},
        Animation,
    },
    core::{
        color::Color,
        math::{frustum::Frustum, mat4::Mat4, quat::Quat, ray::Ray, vec3::Vec3, SmoothAngle},
        pool::Handle,
        visitor::{Visit, VisitResult, Visitor},
    },
    engine::resource_manager::ResourceManager,
    physics::{
        convex_shape::{Axis, CapsuleShape, ConvexShape},
        rigid_body::RigidBody,
        HitKind, RayCastOptions,
    },
    scene,
    scene::{base::BaseBuilder, graph::Graph, node::Node, transform::TransformBuilder, Scene},
    utils::navmesh::Navmesh,
};
use std::ops::{Deref, DerefMut};
use std::{path::Path, sync::mpsc::Sender};

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum BotKind {
    // Beasts
    Mutant,
    Parasite,
    Maw,
    // Humans
}

impl BotKind {
    pub fn from_id(id: i32) -> Result<Self, String> {
        match id {
            0 => Ok(BotKind::Mutant),
            1 => Ok(BotKind::Parasite),
            2 => Ok(BotKind::Maw),
            _ => Err(format!("Invalid bot kind {}", id)),
        }
    }

    pub fn id(self) -> i32 {
        match self {
            BotKind::Mutant => 0,
            BotKind::Parasite => 1,
            BotKind::Maw => 2,
        }
    }
}

pub struct Target {
    position: Vec3,
    handle: Handle<Actor>,
}

impl Default for Target {
    fn default() -> Self {
        Self {
            position: Default::default(),
            handle: Default::default(),
        }
    }
}

impl Visit for Target {
    fn visit(&mut self, name: &str, visitor: &mut Visitor) -> VisitResult {
        visitor.enter_region(name)?;

        self.position.visit("Position", visitor)?;
        self.handle.visit("Handle", visitor)?;

        visitor.leave_region()
    }
}

pub struct Bot {
    target: Option<Target>,
    kind: BotKind,
    model: Handle<Node>,
    character: Character,
    pub definition: &'static BotDefinition,
    locomotion_machine: LocomotionMachine,
    combat_machine: CombatMachine,
    dying_machine: DyingMachine,
    last_health: f32,
    restoration_time: f32,
    path: Vec<Vec3>,
    move_target: Vec3,
    current_path_point: usize,
    frustum: Frustum,
    last_poi_update_time: f64,
    point_of_interest: Vec3,
    last_path_rebuild_time: f64,
    last_move_dir: Vec3,
    spine: Handle<Node>,
    yaw: SmoothAngle,
    pitch: SmoothAngle,
}

impl Deref for Bot {
    type Target = Character;

    fn deref(&self) -> &Self::Target {
        &self.character
    }
}

impl DerefMut for Bot {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.character
    }
}

impl Default for Bot {
    fn default() -> Self {
        Self {
            character: Default::default(),
            kind: BotKind::Mutant,
            model: Default::default(),
            target: Default::default(),
            definition: Self::get_definition(BotKind::Mutant),
            locomotion_machine: Default::default(),
            combat_machine: Default::default(),
            dying_machine: Default::default(),
            last_health: 0.0,
            restoration_time: 0.0,
            path: Default::default(),
            move_target: Default::default(),
            current_path_point: 0,
            frustum: Default::default(),
            last_poi_update_time: -10.0,
            point_of_interest: Default::default(),
            last_path_rebuild_time: -10.0,
            last_move_dir: Default::default(),
            spine: Default::default(),
            yaw: SmoothAngle {
                angle: 0.0,
                target: 0.0,
                speed: 260.0f32.to_radians(), // rad/s
            },
            pitch: SmoothAngle {
                angle: 0.0,
                target: 0.0,
                speed: 260.0f32.to_radians(), // rad/s
            },
        }
    }
}

pub struct BotDefinition {
    pub scale: f32,
    pub health: f32,
    pub kind: BotKind,
    pub walk_speed: f32,
    pub weapon_scale: f32,
    pub model: &'static str,
    pub idle_animation: &'static str,
    pub walk_animation: &'static str,
    pub aim_animation: &'static str,
    pub whip_animation: &'static str,
    pub jump_animation: &'static str,
    pub falling_animation: &'static str,
    pub hit_reaction_animation: &'static str,
    pub dying_animation: &'static str,
    pub dead_animation: &'static str,
    pub weapon_hand_name: &'static str,
    pub left_leg_name: &'static str,
    pub right_leg_name: &'static str,
    pub spine: &'static str,
    pub v_aim_angle_hack: f32,
}

fn load_animation<P: AsRef<Path>>(
    resource_manager: &mut ResourceManager,
    path: P,
    model: Handle<Node>,
    scene: &mut Scene,
    spine: Handle<Node>,
) -> Result<Handle<Animation>, ()> {
    let animation = *resource_manager
        .request_model(path)
        .ok_or(())?
        .lock()
        .unwrap()
        .retarget_animations(model, scene)
        .get(0)
        .ok_or(())?;

    // Disable spine animation because it is used to control vertical aim.
    scene
        .animations
        .get_mut(animation)
        .set_node_track_enabled(spine, false);

    Ok(animation)
}

fn disable_leg_tracks(
    animation: &mut Animation,
    root: Handle<Node>,
    leg_name: &str,
    graph: &Graph,
) {
    animation.set_tracks_enabled_from(graph.find_by_name(root, leg_name), false, graph)
}

struct LocomotionMachine {
    machine: Machine,
    walk_animation: Handle<Animation>,
    walk_state: Handle<State>,
}

impl Default for LocomotionMachine {
    fn default() -> Self {
        Self {
            machine: Default::default(),
            walk_animation: Default::default(),
            walk_state: Default::default(),
        }
    }
}

impl Visit for LocomotionMachine {
    fn visit(&mut self, name: &str, visitor: &mut Visitor) -> VisitResult {
        visitor.enter_region(name)?;

        self.machine.visit("Machine", visitor)?;
        self.walk_animation.visit("WalkAnimation", visitor)?;
        self.walk_state.visit("WalkState", visitor)?;

        visitor.leave_region()
    }
}

impl LocomotionMachine {
    pub const STEP_SIGNAL: u64 = 1;

    const WALK_TO_IDLE_PARAM: &'static str = "WalkToIdle";
    const WALK_TO_JUMP_PARAM: &'static str = "WalkToJump";
    const IDLE_TO_WALK_PARAM: &'static str = "IdleToWalk";
    const IDLE_TO_JUMP_PARAM: &'static str = "IdleToJump";
    const JUMP_TO_FALLING_PARAM: &'static str = "JumpToFalling";
    const FALLING_TO_IDLE_PARAM: &'static str = "FallingToIdle";

    fn new(
        resource_manager: &mut ResourceManager,
        definition: &BotDefinition,
        model: Handle<Node>,
        scene: &mut Scene,
        spine: Handle<Node>,
    ) -> Result<Self, ()> {
        let idle_animation = load_animation(
            resource_manager,
            definition.idle_animation,
            model,
            scene,
            spine,
        )?;

        let walk_animation = load_animation(
            resource_manager,
            definition.walk_animation,
            model,
            scene,
            spine,
        )?;
        scene
            .animations
            .get_mut(walk_animation)
            .add_signal(AnimationSignal::new(Self::STEP_SIGNAL, 0.4))
            .add_signal(AnimationSignal::new(Self::STEP_SIGNAL, 0.8));

        let jump_animation = load_animation(
            resource_manager,
            definition.jump_animation,
            model,
            scene,
            spine,
        )?;
        let falling_animation = load_animation(
            resource_manager,
            definition.falling_animation,
            model,
            scene,
            spine,
        )?;

        let mut machine = Machine::new();

        let jump_node = machine.add_node(machine::PoseNode::make_play_animation(jump_animation));
        let jump_state = machine.add_state(State::new("Jump", jump_node));

        let falling_node =
            machine.add_node(machine::PoseNode::make_play_animation(falling_animation));
        let falling_state = machine.add_state(State::new("Falling", falling_node));

        let walk_node = machine.add_node(machine::PoseNode::make_play_animation(walk_animation));
        let walk_state = machine.add_state(State::new("Walk", walk_node));

        let idle_node = machine.add_node(machine::PoseNode::make_play_animation(idle_animation));
        let idle_state = machine.add_state(State::new("Idle", idle_node));

        machine
            .add_transition(machine::Transition::new(
                "Walk->Idle",
                walk_state,
                idle_state,
                0.5,
                Self::WALK_TO_IDLE_PARAM,
            ))
            .add_transition(machine::Transition::new(
                "Walk->Jump",
                walk_state,
                jump_state,
                0.5,
                Self::WALK_TO_JUMP_PARAM,
            ))
            .add_transition(machine::Transition::new(
                "Idle->Walk",
                idle_state,
                walk_state,
                0.5,
                Self::IDLE_TO_WALK_PARAM,
            ))
            .add_transition(machine::Transition::new(
                "Idle->Jump",
                idle_state,
                jump_state,
                0.5,
                Self::IDLE_TO_JUMP_PARAM,
            ))
            .add_transition(machine::Transition::new(
                "Jump->Falling",
                jump_state,
                falling_state,
                0.5,
                Self::JUMP_TO_FALLING_PARAM,
            ))
            .add_transition(machine::Transition::new(
                "Falling->Idle",
                falling_state,
                idle_state,
                0.5,
                Self::FALLING_TO_IDLE_PARAM,
            ));

        machine.set_entry_state(idle_state);

        Ok(Self {
            walk_animation,
            walk_state,
            machine,
        })
    }

    fn is_walking(&self) -> bool {
        let active_transition = self.machine.active_transition();
        self.machine.active_state() == self.walk_state
            || (active_transition.is_some()
                && self.machine.transitions().borrow(active_transition).dest() == self.walk_state)
    }

    fn clean_up(&mut self, scene: &mut Scene) {
        clean_machine(&self.machine, scene);
    }

    fn apply(
        &mut self,
        scene: &mut Scene,
        time: GameTime,
        in_close_combat: bool,
        need_jump: bool,
        has_ground_contact: bool,
    ) {
        self.machine
            .set_parameter(
                Self::IDLE_TO_WALK_PARAM,
                machine::Parameter::Rule(!in_close_combat),
            )
            .set_parameter(
                Self::WALK_TO_IDLE_PARAM,
                machine::Parameter::Rule(in_close_combat),
            )
            .set_parameter(
                Self::WALK_TO_JUMP_PARAM,
                machine::Parameter::Rule(need_jump),
            )
            .set_parameter(
                Self::IDLE_TO_JUMP_PARAM,
                machine::Parameter::Rule(need_jump),
            )
            .set_parameter(
                Self::JUMP_TO_FALLING_PARAM,
                machine::Parameter::Rule(!has_ground_contact),
            )
            .set_parameter(
                Self::FALLING_TO_IDLE_PARAM,
                machine::Parameter::Rule(has_ground_contact),
            )
            .evaluate_pose(&scene.animations, time.delta)
            .apply(&mut scene.graph);
    }
}

struct DyingMachine {
    machine: Machine,
    dead_state: Handle<State>,
    dead_animation: Handle<Animation>,
    dying_animation: Handle<Animation>,
}

impl Default for DyingMachine {
    fn default() -> Self {
        Self {
            machine: Default::default(),
            dead_state: Default::default(),
            dead_animation: Default::default(),
            dying_animation: Default::default(),
        }
    }
}

impl DyingMachine {
    const DYING_TO_DEAD: &'static str = "DyingToDead";

    fn new(
        resource_manager: &mut ResourceManager,
        definition: &BotDefinition,
        model: Handle<Node>,
        scene: &mut Scene,
        spine: Handle<Node>,
    ) -> Result<Self, ()> {
        let dying_animation = load_animation(
            resource_manager,
            definition.dying_animation,
            model,
            scene,
            spine,
        )?;
        scene
            .animations
            .get_mut(dying_animation)
            .set_enabled(false)
            .set_speed(1.5);

        let dead_animation = load_animation(
            resource_manager,
            definition.dead_animation,
            model,
            scene,
            spine,
        )?;
        scene
            .animations
            .get_mut(dead_animation)
            .set_enabled(false)
            .set_loop(false);

        let mut machine = Machine::new();

        let dying_node = machine.add_node(machine::PoseNode::make_play_animation(dying_animation));
        let dying_state = machine.add_state(State::new("Dying", dying_node));

        let dead_node = machine.add_node(machine::PoseNode::make_play_animation(dead_animation));
        let dead_state = machine.add_state(State::new("Dead", dead_node));

        machine.set_entry_state(dying_state);

        machine.add_transition(machine::Transition::new(
            "Dying->Dead",
            dying_state,
            dead_state,
            1.5,
            Self::DYING_TO_DEAD,
        ));

        Ok(Self {
            machine,
            dead_state,
            dead_animation,
            dying_animation,
        })
    }

    fn clean_up(&mut self, scene: &mut Scene) {
        clean_machine(&self.machine, scene);
    }

    fn apply(&mut self, scene: &mut Scene, time: GameTime, is_dead: bool) {
        scene
            .animations
            .get_mut(self.dying_animation)
            .set_enabled(true);
        scene
            .animations
            .get_mut(self.dead_animation)
            .set_enabled(true);

        self.machine
            .set_parameter(Self::DYING_TO_DEAD, machine::Parameter::Rule(is_dead))
            .evaluate_pose(&scene.animations, time.delta)
            .apply(&mut scene.graph);
    }
}

impl Visit for DyingMachine {
    fn visit(&mut self, name: &str, visitor: &mut Visitor) -> VisitResult {
        visitor.enter_region(name)?;

        self.machine.visit("Machine", visitor)?;
        self.dead_state.visit("DeadState", visitor)?;
        self.dying_animation.visit("DyingAnimation", visitor)?;
        self.dead_animation.visit("DeadAnimation", visitor)?;

        visitor.leave_region()
    }
}

struct CombatMachine {
    machine: Machine,
    hit_reaction_animation: Handle<Animation>,
    whip_animation: Handle<Animation>,
    aim_state: Handle<State>,
}

impl Default for CombatMachine {
    fn default() -> Self {
        Self {
            machine: Default::default(),
            hit_reaction_animation: Default::default(),
            whip_animation: Default::default(),
            aim_state: Default::default(),
        }
    }
}

impl CombatMachine {
    pub const HIT_SIGNAL: u64 = 1;

    const AIM_TO_WHIP_PARAM: &'static str = "AimToWhip";
    const WHIP_TO_AIM_PARAM: &'static str = "WhipToAim";
    const HIT_REACTION_TO_AIM_PARAM: &'static str = "HitReactionToAim";
    const AIM_TO_HIT_REACTION_PARAM: &'static str = "AimToHitReaction";
    const WHIP_TO_HIT_REACTION_PARAM: &'static str = "WhipToHitReaction";

    fn new(
        resource_manager: &mut ResourceManager,
        definition: &BotDefinition,
        model: Handle<Node>,
        scene: &mut Scene,
        spine: Handle<Node>,
    ) -> Result<Self, ()> {
        let aim_animation = load_animation(
            resource_manager,
            definition.aim_animation,
            model,
            scene,
            spine,
        )?;

        let whip_animation = load_animation(
            resource_manager,
            definition.whip_animation,
            model,
            scene,
            spine,
        )?;
        scene
            .animations
            .get_mut(whip_animation)
            .add_signal(AnimationSignal::new(Self::HIT_SIGNAL, 0.9));

        let hit_reaction_animation = load_animation(
            resource_manager,
            definition.hit_reaction_animation,
            model,
            scene,
            spine,
        )?;
        scene
            .animations
            .get_mut(hit_reaction_animation)
            .set_loop(false)
            .set_speed(2.0);

        // These animations must *not* affect legs, because legs animated using locomotion machine
        disable_leg_tracks(
            scene.animations.get_mut(aim_animation),
            model,
            definition.left_leg_name,
            &scene.graph,
        );
        disable_leg_tracks(
            scene.animations.get_mut(aim_animation),
            model,
            definition.right_leg_name,
            &scene.graph,
        );

        disable_leg_tracks(
            scene.animations.get_mut(whip_animation),
            model,
            definition.left_leg_name,
            &scene.graph,
        );
        disable_leg_tracks(
            scene.animations.get_mut(whip_animation),
            model,
            definition.right_leg_name,
            &scene.graph,
        );

        disable_leg_tracks(
            scene.animations.get_mut(hit_reaction_animation),
            model,
            definition.left_leg_name,
            &scene.graph,
        );
        disable_leg_tracks(
            scene.animations.get_mut(hit_reaction_animation),
            model,
            definition.right_leg_name,
            &scene.graph,
        );

        let mut machine = Machine::new();

        let hit_reaction_node = machine.add_node(machine::PoseNode::make_play_animation(
            hit_reaction_animation,
        ));
        let hit_reaction_state = machine.add_state(State::new("HitReaction", hit_reaction_node));

        let aim_node = machine.add_node(machine::PoseNode::make_play_animation(aim_animation));
        let aim_state = machine.add_state(State::new("Aim", aim_node));

        let whip_node = machine.add_node(machine::PoseNode::make_play_animation(whip_animation));
        let whip_state = machine.add_state(State::new("Whip", whip_node));

        machine
            .add_transition(machine::Transition::new(
                "Aim->Whip",
                aim_state,
                whip_state,
                0.5,
                Self::AIM_TO_WHIP_PARAM,
            ))
            .add_transition(machine::Transition::new(
                "Whip->Aim",
                whip_state,
                aim_state,
                0.5,
                Self::WHIP_TO_AIM_PARAM,
            ))
            .add_transition(machine::Transition::new(
                "Whip->HitReaction",
                whip_state,
                hit_reaction_state,
                0.2,
                Self::WHIP_TO_HIT_REACTION_PARAM,
            ))
            .add_transition(machine::Transition::new(
                "Aim->HitReaction",
                aim_state,
                hit_reaction_state,
                0.2,
                Self::AIM_TO_HIT_REACTION_PARAM,
            ))
            .add_transition(machine::Transition::new(
                "HitReaction->Aim",
                hit_reaction_state,
                aim_state,
                0.5,
                Self::HIT_REACTION_TO_AIM_PARAM,
            ));

        Ok(Self {
            machine,
            hit_reaction_animation,
            whip_animation,
            aim_state,
        })
    }

    fn clean_up(&mut self, scene: &mut Scene) {
        clean_machine(&self.machine, scene)
    }

    fn apply(
        &mut self,
        scene: &mut Scene,
        time: GameTime,
        in_close_combat: bool,
        was_damaged: bool,
        can_aim: bool,
    ) {
        self.machine
            .set_parameter(
                Self::WHIP_TO_AIM_PARAM,
                machine::Parameter::Rule(!in_close_combat),
            )
            .set_parameter(
                Self::AIM_TO_WHIP_PARAM,
                machine::Parameter::Rule(in_close_combat),
            )
            .set_parameter(
                Self::WHIP_TO_HIT_REACTION_PARAM,
                machine::Parameter::Rule(was_damaged),
            )
            .set_parameter(
                Self::AIM_TO_HIT_REACTION_PARAM,
                machine::Parameter::Rule(was_damaged),
            )
            .set_parameter(
                Self::HIT_REACTION_TO_AIM_PARAM,
                machine::Parameter::Rule(can_aim),
            )
            .evaluate_pose(&scene.animations, time.delta)
            .apply(&mut scene.graph);
    }
}

impl Visit for CombatMachine {
    fn visit(&mut self, name: &str, visitor: &mut Visitor) -> VisitResult {
        visitor.enter_region(name)?;

        self.machine.visit("Machine", visitor)?;
        self.hit_reaction_animation
            .visit("HitReactionAnimation", visitor)?;
        self.whip_animation.visit("WhipAnimation", visitor)?;
        self.aim_state.visit("AimState", visitor)?;

        visitor.leave_region()
    }
}

impl Bot {
    pub fn get_definition(kind: BotKind) -> &'static BotDefinition {
        match kind {
            BotKind::Mutant => {
                static DEFINITION: BotDefinition = BotDefinition {
                    kind: BotKind::Mutant,
                    model: assets::models::characters::MUTANT,
                    idle_animation: assets::animations::mutant::IDLE,
                    walk_animation: assets::animations::mutant::WALK,
                    aim_animation: assets::animations::mutant::AIM,
                    whip_animation: assets::animations::mutant::WHIP,
                    jump_animation: assets::animations::mutant::JUMP,
                    falling_animation: assets::animations::mutant::FALLING,
                    dying_animation: assets::animations::mutant::DYING,
                    dead_animation: assets::animations::mutant::DEAD,
                    hit_reaction_animation: assets::animations::mutant::HIT_REACTION,
                    weapon_hand_name: "Mutant:RightHand",
                    left_leg_name: "Mutant:LeftUpLeg",
                    right_leg_name: "Mutant:RightUpLeg",
                    spine: "Mutant:Spine",
                    walk_speed: 6.0,
                    scale: 0.0085,
                    weapon_scale: 2.6,
                    health: 100.0,
                    v_aim_angle_hack: -2.0,
                };
                &DEFINITION
            }
            BotKind::Parasite => {
                static DEFINITION: BotDefinition = BotDefinition {
                    kind: BotKind::Parasite,
                    model: assets::models::characters::PARASITE,
                    idle_animation: assets::animations::parasite::IDLE,
                    walk_animation: assets::animations::parasite::WALK,
                    aim_animation: assets::animations::parasite::AIM,
                    whip_animation: assets::animations::parasite::WHIP,
                    jump_animation: assets::animations::parasite::JUMP,
                    falling_animation: assets::animations::parasite::FALLING,
                    dying_animation: assets::animations::parasite::DYING,
                    dead_animation: assets::animations::parasite::DEAD,
                    hit_reaction_animation: assets::animations::parasite::HIT_REACTION,
                    weapon_hand_name: "RightHand",
                    left_leg_name: "LeftUpLeg",
                    right_leg_name: "RightUpLeg",
                    spine: "Spine",
                    walk_speed: 6.0,
                    scale: 0.0085,
                    weapon_scale: 2.5,
                    health: 100.0,
                    v_aim_angle_hack: 12.0,
                };
                &DEFINITION
            }
            BotKind::Maw => {
                static DEFINITION: BotDefinition = BotDefinition {
                    kind: BotKind::Maw,
                    model: assets::models::characters::MAW,
                    idle_animation: assets::animations::maw::IDLE,
                    walk_animation: assets::animations::maw::WALK,
                    aim_animation: assets::animations::maw::AIM,
                    whip_animation: assets::animations::maw::WHIP,
                    jump_animation: assets::animations::maw::JUMP,
                    falling_animation: assets::animations::maw::FALLING,
                    dying_animation: assets::animations::maw::DYING,
                    dead_animation: assets::animations::maw::DEAD,
                    hit_reaction_animation: assets::animations::maw::HIT_REACTION,
                    weapon_hand_name: "RightHand",
                    left_leg_name: "LeftUpLeg",
                    right_leg_name: "RightUpLeg",
                    spine: "Spine",
                    walk_speed: 6.0,
                    scale: 0.0085,
                    weapon_scale: 2.5,
                    health: 100.0,
                    v_aim_angle_hack: 16.0,
                };
                &DEFINITION
            }
        }
    }

    pub fn new(
        kind: BotKind,
        resource_manager: &mut ResourceManager,
        scene: &mut Scene,
        position: Vec3,
        sender: Sender<Message>,
    ) -> Result<Self, ()> {
        let definition = Self::get_definition(kind);

        let body_height = 1.25;

        let model = resource_manager
            .request_model(Path::new(definition.model))
            .ok_or(())?
            .lock()
            .unwrap()
            .instantiate_geometry(scene);

        let spine = scene.graph.find_by_name(model, definition.spine);
        if spine.is_none() {
            print!("WARNING: Spine bone not found, bot won't aim vertically!");
        }

        let (pivot, body) = {
            let pivot = scene.graph.add_node(Node::Base(Default::default()));
            scene.graph.link_nodes(model, pivot);
            let transform = scene.graph[model].local_transform_mut();
            transform.set_position(Vec3::new(0.0, -body_height * 0.5, 0.0));
            transform.set_scale(Vec3::new(
                definition.scale,
                definition.scale,
                definition.scale,
            ));

            let capsule_shape = CapsuleShape::new(0.28, body_height, Axis::Y);
            let mut capsule_body = RigidBody::new(ConvexShape::Capsule(capsule_shape));
            capsule_body.set_friction(Vec3::new(0.2, 0.0, 0.2));
            capsule_body.set_position(position);
            let body = scene.physics.add_body(capsule_body);
            scene.physics_binder.bind(pivot, body);

            (pivot, body)
        };

        let hand = scene.graph.find_by_name(model, definition.weapon_hand_name);
        let wpn_scale = definition.weapon_scale * (1.0 / definition.scale);
        let weapon_pivot = Node::Base(
            BaseBuilder::new()
                .with_local_transform(
                    TransformBuilder::new()
                        .with_local_scale(Vec3::new(wpn_scale, wpn_scale, wpn_scale))
                        .with_local_rotation(
                            Quat::from_axis_angle(Vec3::RIGHT, std::f32::consts::FRAC_PI_2)
                                * Quat::from_axis_angle(Vec3::UP, -std::f32::consts::FRAC_PI_2),
                        )
                        .build(),
                )
                .build(),
        );
        let weapon_pivot = scene.graph.add_node(weapon_pivot);
        scene.graph.link_nodes(weapon_pivot, hand);

        let locomotion_machine =
            LocomotionMachine::new(resource_manager, &definition, model, scene, spine)?;
        let combat_machine = CombatMachine::new(resource_manager, definition, model, scene, spine)?;
        let dying_machine = DyingMachine::new(resource_manager, definition, model, scene, spine)?;

        Ok(Self {
            character: Character {
                pivot,
                body,
                weapon_pivot,
                health: definition.health,
                sender: Some(sender),
                name: format!("{:?}", kind),
                ..Default::default()
            },
            spine,
            definition,
            last_health: definition.health,
            model,
            kind,
            locomotion_machine,
            combat_machine,
            dying_machine,
            ..Default::default()
        })
    }

    pub fn can_be_removed(&self) -> bool {
        self.dying_machine.machine.active_state() == self.dying_machine.dead_state
    }

    pub fn can_shoot(&self) -> bool {
        self.combat_machine.machine.active_state() == self.combat_machine.aim_state
    }

    fn select_target(
        &mut self,
        self_handle: Handle<Actor>,
        scene: &Scene,
        targets: &[TargetDescriptor],
    ) {
        self.target = None;
        let position = self.character.position(&scene.physics);
        let mut closest_distance = std::f32::MAX;
        let mut raycast_results = Vec::new();
        'target_loop: for desc in targets {
            if desc.handle != self_handle && self.frustum.is_contains_point(desc.position) {
                if let Some(ray) = Ray::from_two_points(&position, &desc.position) {
                    let options = RayCastOptions {
                        ignore_bodies: false,
                        ignore_static_geometries: false,
                        sort_results: true,
                    };
                    if scene.physics.ray_cast(&ray, options, &mut raycast_results) {
                        'hit_loop: for hit in raycast_results.iter() {
                            match hit.kind {
                                HitKind::StaticTriangle { .. } => {
                                    // Target is behind something.
                                    continue 'target_loop;
                                }
                                HitKind::Body(handle) => {
                                    if self.character.body == handle {
                                        continue 'hit_loop;
                                    }
                                }
                            }
                        }
                    }
                }

                let sqr_d = position.sqr_distance(&desc.position);
                if sqr_d < closest_distance {
                    self.target = Some(Target {
                        position: desc.position,
                        handle: desc.handle,
                    });
                    closest_distance = sqr_d;
                }
            }
        }
    }

    fn select_point_of_interest(&mut self, items: &ItemContainer, scene: &Scene, time: &GameTime) {
        if time.elapsed - self.last_poi_update_time >= 1.25 {
            // Select closest non-despawned item as point of interest.
            let self_position = self.position(&scene.physics);
            let mut closest_distance = std::f32::MAX;
            for item in items.iter() {
                if !item.is_picked_up() {
                    let item_position = item.position(&scene.graph);
                    let sqr_d = item_position.sqr_distance(&self_position);
                    if sqr_d < closest_distance {
                        closest_distance = sqr_d;
                        self.point_of_interest = item_position;
                    }
                }
            }
            self.last_poi_update_time = time.elapsed;
        }
    }

    fn select_weapon(&mut self, weapons: &WeaponContainer) {
        if self.character.current_weapon().is_some()
            && weapons[self.character.current_weapon()].ammo() == 0
        {
            for (i, handle) in self.character.weapons().iter().enumerate() {
                if weapons[*handle].ammo() > 0 {
                    self.character.set_current_weapon(i);
                    break;
                }
            }
        }
    }

    pub fn debug_draw(&self, context: &mut SceneDrawingContext) {
        for pts in self.path.windows(2) {
            let a = pts[0];
            let b = pts[1];
            context.add_line(scene::Line {
                begin: a,
                end: b,
                color: Color::from_rgba(255, 0, 0, 255),
            });
        }

        context.draw_frustum(&self.frustum, Color::from_rgba(0, 200, 0, 255));
    }

    fn update_frustum(&mut self, position: Vec3, graph: &Graph) {
        let head_pos = position + Vec3::new(0.0, 0.8, 0.0);
        let up = graph[self.model].up_vector();
        let look_at = head_pos + graph[self.model].look_vector();
        let view_matrix = Mat4::look_at(head_pos, look_at, up).unwrap_or_default();
        let projection_matrix = Mat4::perspective(60.0f32.to_radians(), 16.0 / 9.0, 0.1, 7.0);
        let view_projection_matrix = projection_matrix * view_matrix;
        self.frustum = Frustum::from(view_projection_matrix).unwrap();
    }

    fn aim_vertically(&mut self, look_dir: Vec3, graph: &mut Graph, time: GameTime) {
        let angle = self.pitch.angle();
        self.pitch
            .set_target(
                look_dir.dot(&Vec3::UP).acos() - std::f32::consts::PI / 2.0
                    + self.definition.v_aim_angle_hack.to_radians(),
            )
            .update(time.delta);

        if self.spine.is_some() {
            graph[self.spine]
                .local_transform_mut()
                .set_rotation(Quat::from_axis_angle(Vec3::RIGHT, angle));
        }
    }

    fn aim_horizontally(&mut self, look_dir: Vec3, graph: &mut Graph, time: GameTime) {
        let angle = self.yaw.angle();
        self.yaw
            .set_target(look_dir.x.atan2(look_dir.z))
            .update(time.delta);

        graph[self.character.pivot]
            .local_transform_mut()
            .set_rotation(Quat::from_axis_angle(Vec3::UP, angle));
    }

    fn rebuild_path(&mut self, position: Vec3, navmesh: &mut Navmesh, time: GameTime) {
        let from = position - Vec3::new(0.0, 1.0, 0.0);
        if let Some(from_index) = navmesh.query_closest(from) {
            if let Some(to_index) = navmesh.query_closest(self.point_of_interest) {
                self.current_path_point = 0;
                // Rebuild path if target path vertex has changed.
                if navmesh
                    .build_path(from_index, to_index, &mut self.path)
                    .is_ok()
                {
                    self.path.reverse();
                    self.last_path_rebuild_time = time.elapsed;
                }
            }
        }
    }

    pub fn update(
        &mut self,
        self_handle: Handle<Actor>,
        context: &mut UpdateContext,
        targets: &[TargetDescriptor],
    ) {
        if self.character.is_dead() {
            self.dying_machine
                .apply(context.scene, context.time, self.character.is_dead());
        } else {
            self.select_target(self_handle, context.scene, targets);
            self.select_weapon(context.weapons);
            self.select_point_of_interest(context.items, context.scene, &context.time);

            let has_ground_contact = self.character.has_ground_contact(&context.scene.physics);
            let body = context.scene.physics.borrow_body_mut(self.character.body);
            let (in_close_combat, look_dir) = match self.target.as_ref() {
                None => (false, self.point_of_interest - body.get_position()),
                Some(target) => {
                    let d = target.position - body.get_position();
                    let close_combat_threshold = 2.0;
                    (d.len() <= close_combat_threshold, d)
                }
            };

            let position = body.get_position();

            if let Some(path_point) = self.path.get(self.current_path_point) {
                self.move_target = *path_point;
                if self.move_target.distance(&position) <= 2.0
                    && self.current_path_point < self.path.len() - 1
                {
                    self.current_path_point += 1;
                }
            }

            self.update_frustum(position, &context.scene.graph);

            if let Some(look_dir) = look_dir.normalized() {
                self.aim_vertically(look_dir, &mut context.scene.graph, context.time);
                self.aim_horizontally(look_dir, &mut context.scene.graph, context.time);

                if !in_close_combat {
                    if has_ground_contact {
                        if let Some(move_dir) = (self.move_target - position).normalized() {
                            let vel =
                                move_dir.scale(self.definition.walk_speed * context.time.delta);
                            body.set_x_velocity(vel.x);
                            body.set_z_velocity(vel.z);
                            self.last_move_dir = move_dir;
                        }
                    } else {
                        // A bit of air control. This helps jump of ledges when there is jump pad below bot.
                        let vel = self
                            .last_move_dir
                            .scale(self.definition.walk_speed * context.time.delta);
                        body.set_x_velocity(vel.x);
                        body.set_z_velocity(vel.z);
                    }
                }
            }

            let need_jump = look_dir.y >= 0.3 && has_ground_contact && in_close_combat;
            if need_jump {
                body.set_y_velocity(0.08);
            }
            let was_damaged = self.character.health < self.last_health;
            if was_damaged {
                let hit_reaction = context
                    .scene
                    .animations
                    .get_mut(self.combat_machine.hit_reaction_animation);
                if hit_reaction.has_ended() {
                    hit_reaction.rewind();
                }
                self.restoration_time = 0.8;
            }
            let can_aim = self.restoration_time <= 0.0;
            self.last_health = self.character.health;

            self.locomotion_machine.apply(
                context.scene,
                context.time,
                in_close_combat,
                need_jump,
                has_ground_contact,
            );
            self.combat_machine.apply(
                context.scene,
                context.time,
                in_close_combat,
                was_damaged,
                can_aim,
            );

            let sender = self.character.sender.as_ref().unwrap();

            if !in_close_combat && can_aim && self.can_shoot() && self.target.is_some() {
                if let Some(weapon) = self
                    .character
                    .weapons
                    .get(self.character.current_weapon as usize)
                {
                    sender
                        .send(Message::ShootWeapon {
                            weapon: *weapon,
                            initial_velocity: Vec3::ZERO,
                            direction: Some(look_dir),
                        })
                        .unwrap();
                }
            }

            // Apply damage to target from melee attack
            if let Some(target) = self.target.as_ref() {
                while let Some(event) = context
                    .scene
                    .animations
                    .get_mut(self.combat_machine.whip_animation)
                    .pop_event()
                {
                    if event.signal_id == CombatMachine::HIT_SIGNAL && in_close_combat {
                        sender
                            .send(Message::DamageActor {
                                actor: target.handle,
                                who: Default::default(),
                                amount: 20.0,
                            })
                            .unwrap();
                    }
                }
            }

            // Emit step sounds from walking animation.
            if self.locomotion_machine.is_walking() {
                while let Some(event) = context
                    .scene
                    .animations
                    .get_mut(self.locomotion_machine.walk_animation)
                    .pop_event()
                {
                    if event.signal_id == LocomotionMachine::STEP_SIGNAL && has_ground_contact {
                        sender
                            .send(Message::PlaySound {
                                path: assets::sounds::footsteps::SHOE_STONE[rand::thread_rng()
                                    .gen_range(0, assets::sounds::footsteps::SHOE_STONE.len())]
                                .into(),
                                position,
                                gain: 1.0,
                                rolloff_factor: 2.0,
                                radius: 3.0,
                            })
                            .unwrap();
                    }
                }
            }

            if context.time.elapsed - self.last_path_rebuild_time >= 1.0 {
                if let Some(navmesh) = context.navmesh.as_mut() {
                    self.rebuild_path(position, navmesh, context.time);
                }
            }
            self.restoration_time -= context.time.delta;
        }
    }

    pub fn clean_up(&mut self, scene: &mut Scene) {
        self.combat_machine.clean_up(scene);
        self.dying_machine.clean_up(scene);
        self.locomotion_machine.clean_up(scene);
        self.character.clean_up(scene);
    }

    pub fn on_actor_removed(&mut self, handle: Handle<Actor>) {
        if let Some(target) = self.target.as_ref() {
            if target.handle == handle {
                self.target = None;
            }
        }
    }

    pub fn set_point_of_interest(&mut self, poi: Vec3, time: GameTime) {
        self.point_of_interest = poi;
        self.last_poi_update_time = time.elapsed;
    }
}

fn clean_machine(machine: &Machine, scene: &mut Scene) {
    for node in machine.nodes() {
        if let PoseNode::PlayAnimation(node) = node {
            scene.animations.remove(node.animation);
        }
    }
}

impl Visit for Bot {
    fn visit(&mut self, name: &str, visitor: &mut Visitor) -> VisitResult {
        visitor.enter_region(name)?;

        let mut kind_id = self.kind.id();
        kind_id.visit("Kind", visitor)?;
        if visitor.is_reading() {
            self.kind = BotKind::from_id(kind_id)?;
        }

        self.definition = Self::get_definition(self.kind);
        self.character.visit("Character", visitor)?;
        self.model.visit("Model", visitor)?;
        self.target.visit("Target", visitor)?;
        self.locomotion_machine
            .visit("LocomotionMachine", visitor)?;
        self.combat_machine.visit("AimMachine", visitor)?;
        self.restoration_time.visit("RestorationTime", visitor)?;
        self.yaw.visit("Yaw", visitor)?;
        self.pitch.visit("Pitch", visitor)?;

        visitor.leave_region()
    }
}
