use rg3d::event::VirtualKeyCode;
use serde::{Deserialize, Serialize};

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ControlButton {
    Mouse(u8),
    Key(VirtualKeyCode),
    WheelUp,
    WheelDown,
}

impl ControlButton {
    pub fn name(self) -> &'static str {
        match self {
            ControlButton::Mouse(index) => match index {
                1 => "LMB",
                2 => "RMB",
                3 => "MMB",
                4 => "MB4",
                5 => "MB5",
                _ => "Unknown",
            },
            ControlButton::Key(code) => rg3d::utils::virtual_key_code_name(code),
            ControlButton::WheelUp => "Wheel Up",
            ControlButton::WheelDown => "Wheel Down",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlButtonDefinition {
    pub description: String,
    pub button: ControlButton,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlScheme {
    pub move_forward: ControlButtonDefinition,
    pub move_backward: ControlButtonDefinition,
    pub move_left: ControlButtonDefinition,
    pub move_right: ControlButtonDefinition,
    pub jump: ControlButtonDefinition,
    pub crouch: ControlButtonDefinition,
    pub ads: ControlButtonDefinition,
    pub shoot: ControlButtonDefinition,
    pub next_weapon: ControlButtonDefinition,
    pub prev_weapon: ControlButtonDefinition,
    pub run: ControlButtonDefinition,
    pub mouse_sens: f32,
    pub mouse_y_inverse: bool,
    pub smooth_mouse: bool,
    pub shake_camera: bool,
}

impl Default for ControlScheme {
    fn default() -> Self {
        Self {
            move_forward: ControlButtonDefinition {
                description: "Move Forward".to_string(),
                button: ControlButton::Key(VirtualKeyCode::W),
            },
            move_backward: ControlButtonDefinition {
                description: "Move Backward".to_string(),
                button: ControlButton::Key(VirtualKeyCode::S),
            },
            move_left: ControlButtonDefinition {
                description: "Move Left".to_string(),
                button: ControlButton::Key(VirtualKeyCode::A),
            },
            move_right: ControlButtonDefinition {
                description: "Move Right".to_string(),
                button: ControlButton::Key(VirtualKeyCode::D),
            },
            jump: ControlButtonDefinition {
                description: "Jump".to_string(),
                button: ControlButton::Key(VirtualKeyCode::Space),
            },
            crouch: ControlButtonDefinition {
                description: "Crouch".to_string(),
                button: ControlButton::Key(VirtualKeyCode::C),
            },
            ads: ControlButtonDefinition {
                description: "Aim Down Sights".to_string(),
                button: ControlButton::Mouse(3),
            },
            shoot: ControlButtonDefinition {
                description: "Shoot".to_string(),
                button: ControlButton::Mouse(1),
            },
            next_weapon: ControlButtonDefinition {
                description: "Next Weapon".to_string(),
                button: ControlButton::WheelUp,
            },
            prev_weapon: ControlButtonDefinition {
                description: "Previous Weapon".to_string(),
                button: ControlButton::WheelDown,
            },
            run: ControlButtonDefinition {
                description: "Run".to_string(),
                button: ControlButton::Key(VirtualKeyCode::LShift),
            },
            mouse_sens: 0.2,
            mouse_y_inverse: false,
            smooth_mouse: true,
            shake_camera: true,
        }
    }
}

impl ControlScheme {
    pub fn buttons_mut(&mut self) -> [&mut ControlButtonDefinition; 11] {
        [
            &mut self.move_forward,
            &mut self.move_backward,
            &mut self.move_left,
            &mut self.move_right,
            &mut self.jump,
            &mut self.crouch,
            &mut self.ads,
            &mut self.shoot,
            &mut self.next_weapon,
            &mut self.prev_weapon,
            &mut self.run,
        ]
    }

    pub fn buttons(&self) -> [&ControlButtonDefinition; 11] {
        [
            &self.move_forward,
            &self.move_backward,
            &self.move_left,
            &self.move_right,
            &self.jump,
            &self.crouch,
            &self.ads,
            &self.shoot,
            &self.next_weapon,
            &self.prev_weapon,
            &self.run,
        ]
    }

    pub fn reset(&mut self) {
        *self = Default::default();
    }
}
