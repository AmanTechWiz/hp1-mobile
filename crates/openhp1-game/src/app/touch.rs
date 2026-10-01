//! On-screen controls for touch screens, which have no keyboard or mouse.
//!
//! Touches become the same keys, buttons, and mouse motion that the desktop
//! controls feed into [`InputState`], so gameplay receives the original input
//! axes either way. A floating stick on the left half moves Harry, dragging
//! anywhere else looks around, and the buttons hold their mapped key or mouse
//! button for as long as they are touched.

use egui::{Align2, Color32, FontId, Id, LayerId, Order, Pos2, Rect, Stroke, Vec2};
use winit::{
    event::{ElementState, MouseButton, Touch, TouchPhase},
    keyboard::KeyCode,
};

use super::InputState;

/// Mouse counts produced per logical pixel of finger travel while looking.
const LOOK_SCALE: f32 = 2.0;
/// Fraction of the stick radius a finger must travel before a direction holds.
const STICK_DEAD_ZONE: f32 = 0.35;
/// Logical height of the game area at which controls reach full size.
const FULL_SIZE_HEIGHT: f32 = 400.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Button {
    Menu,
    Jump,
    Cast,
    Boost,
    Brake,
}

impl Button {
    const ALL: [Self; 5] = [Self::Menu, Self::Jump, Self::Cast, Self::Boost, Self::Brake];

    fn label(self) -> &'static str {
        match self {
            Self::Menu => "Menu",
            Self::Jump => "Jump",
            Self::Cast => "Cast",
            Self::Boost => "Boost",
            Self::Brake => "Brake",
        }
    }

    fn set(self, input: &mut InputState, state: ElementState) {
        match self {
            Self::Menu => {}
            Self::Jump => input.set_key(KeyCode::Space, state),
            Self::Cast => input.set_mouse_button(MouseButton::Left, state),
            Self::Boost => input.set_key(KeyCode::KeyZ, state),
            Self::Brake => input.set_key(KeyCode::KeyX, state),
        }
    }
}

/// Where the controls sit, in physical window pixels.
///
/// Controls are laid out over the presented game area rather than the whole
/// window because the overlay is drawn into the game image, which may be
/// letterboxed.
pub(super) struct Layout {
    area: Rect,
    scale_factor: f32,
    unit: f32,
}

impl Layout {
    pub(super) fn new(area: Rect, scale_factor: f32) -> Self {
        let scale_factor = scale_factor.max(0.01);
        let unit = scale_factor * (area.height() / scale_factor / FULL_SIZE_HEIGHT).clamp(0.6, 1.0);
        Self {
            area,
            scale_factor,
            unit,
        }
    }

    fn margin(&self) -> f32 {
        18.0 * self.unit
    }

    fn stick_radius(&self) -> f32 {
        60.0 * self.unit
    }

    fn stick_home(&self) -> Pos2 {
        let inset = self.margin() + self.stick_radius() * 1.2;
        Pos2::new(self.area.min.x + inset, self.area.max.y - inset)
    }

    fn radius(&self, button: Button) -> f32 {
        match button {
            Button::Jump | Button::Cast => 38.0 * self.unit,
            Button::Menu | Button::Boost | Button::Brake => 28.0 * self.unit,
        }
    }

    fn center(&self, button: Button) -> Pos2 {
        let margin = self.margin();
        let large = self.radius(Button::Jump);
        let jump = Pos2::new(
            self.area.max.x - margin - large,
            self.area.max.y - margin - large,
        );
        match button {
            Button::Menu => {
                let inset = margin + self.radius(Button::Menu);
                self.area.min + Vec2::splat(inset)
            }
            Button::Jump => jump,
            Button::Cast => jump - Vec2::new(large * 2.5, 0.0),
            Button::Boost => jump - Vec2::new(0.0, large * 2.5),
            Button::Brake => jump - Vec2::splat(large * 2.5),
        }
    }

    fn button_at(&self, position: Pos2) -> Option<Button> {
        Button::ALL
            .into_iter()
            .find(|button| position.distance(self.center(*button)) <= self.radius(*button) * 1.25)
    }
}

#[derive(Clone, Copy, Debug)]
enum Role {
    Stick { origin: Pos2, position: Pos2 },
    Look { last: Pos2 },
    Button(Button),
}

pub(super) struct TouchControls {
    enabled: bool,
    touches: Vec<(u64, Role)>,
}

impl TouchControls {
    pub(super) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            touches: Vec::new(),
        }
    }

    pub(super) fn enabled(&self) -> bool {
        self.enabled
    }

    /// Applies one touch to `input`, returning whether the menu was requested.
    pub(super) fn handle(
        &mut self,
        touch: &Touch,
        layout: &Layout,
        input: &mut InputState,
    ) -> bool {
        let position = Pos2::new(touch.location.x as f32, touch.location.y as f32);
        let existing = self.touches.iter().position(|(id, _)| *id == touch.id);
        match (touch.phase, existing) {
            (TouchPhase::Started, None) => {
                let role = if let Some(button) = layout.button_at(position) {
                    if button == Button::Menu {
                        return true;
                    }
                    button.set(input, ElementState::Pressed);
                    Role::Button(button)
                } else if position.x < layout.area.center().x && !self.has_stick() {
                    Role::Stick {
                        origin: position,
                        position,
                    }
                } else {
                    Role::Look { last: position }
                };
                self.touches.push((touch.id, role));
            }
            (TouchPhase::Moved, Some(index)) => match &mut self.touches[index].1 {
                Role::Stick {
                    origin,
                    position: current,
                } => {
                    let radius = layout.stick_radius();
                    let offset = position - *origin;
                    if offset.length() > radius {
                        *origin = position - offset.normalized() * radius;
                    }
                    *current = position;
                    let direction = (position - *origin) / radius;
                    set_stick_keys(input, direction);
                }
                Role::Look { last } => {
                    let delta = (position - *last) * LOOK_SCALE / layout.scale_factor;
                    input.mouse_delta.0 += f64::from(delta.x);
                    input.mouse_delta.1 += f64::from(delta.y);
                    *last = position;
                }
                Role::Button(_) => {}
            },
            (TouchPhase::Ended | TouchPhase::Cancelled, Some(index)) => {
                match self.touches.remove(index).1 {
                    Role::Stick { .. } => set_stick_keys(input, Vec2::ZERO),
                    Role::Button(button) => button.set(input, ElementState::Released),
                    Role::Look { .. } => {}
                }
            }
            _ => {}
        }
        false
    }

    /// Forgets every active touch after `input` was cleared.
    pub(super) fn release(&mut self) {
        self.touches.clear();
    }

    fn has_stick(&self) -> bool {
        self.touches
            .iter()
            .any(|(_, role)| matches!(role, Role::Stick { .. }))
    }

    fn held(&self, button: Button) -> bool {
        self.touches
            .iter()
            .any(|(_, role)| matches!(role, Role::Button(held) if *held == button))
    }

    pub(super) fn paint(&self, context: &egui::Context, layout: &Layout) {
        let screen = context.content_rect();
        let area = layout.area;
        if area.width() <= 0.0 || area.height() <= 0.0 {
            return;
        }
        let scale = screen.width() / area.width();
        let to_screen = |position: Pos2| screen.min + (position - area.min) * scale;
        let painter =
            context.layer_painter(LayerId::new(Order::Foreground, Id::new("touch_controls")));
        let stroke = Stroke::new(1.5, Color32::from_white_alpha(110));

        let stick = self.touches.iter().find_map(|(_, role)| match role {
            Role::Stick { origin, position } => Some((*origin, *position)),
            _ => None,
        });
        let (origin, knob) = stick.unwrap_or((layout.stick_home(), layout.stick_home()));
        let alpha = if stick.is_some() { 60 } else { 30 };
        let radius = layout.stick_radius() * scale;
        painter.circle(
            to_screen(origin),
            radius,
            Color32::from_black_alpha(alpha),
            stroke,
        );
        painter.circle(
            to_screen(knob),
            radius * 0.45,
            Color32::from_white_alpha(alpha + 20),
            stroke,
        );

        for button in Button::ALL {
            let center = to_screen(layout.center(button));
            let radius = layout.radius(button) * scale;
            let fill = if self.held(button) {
                Color32::from_white_alpha(90)
            } else {
                Color32::from_black_alpha(70)
            };
            painter.circle(center, radius, fill, stroke);
            painter.text(
                center,
                Align2::CENTER_CENTER,
                button.label(),
                FontId::proportional(radius * 0.55),
                Color32::from_white_alpha(220),
            );
        }
    }
}

fn set_stick_keys(input: &mut InputState, direction: Vec2) {
    let state = |held: bool| {
        if held {
            ElementState::Pressed
        } else {
            ElementState::Released
        }
    };
    input.set_key(KeyCode::KeyW, state(direction.y < -STICK_DEAD_ZONE));
    input.set_key(KeyCode::KeyS, state(direction.y > STICK_DEAD_ZONE));
    input.set_key(KeyCode::KeyA, state(direction.x < -STICK_DEAD_ZONE));
    input.set_key(KeyCode::KeyD, state(direction.x > STICK_DEAD_ZONE));
}

#[cfg(test)]
mod tests {
    use winit::{dpi::PhysicalPosition, event::DeviceId};

    use super::*;

    fn layout() -> Layout {
        Layout::new(
            Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 800.0)),
            2.0,
        )
    }

    fn touch(id: u64, phase: TouchPhase, position: Pos2) -> Touch {
        Touch {
            device_id: DeviceId::dummy(),
            phase,
            location: PhysicalPosition::new(f64::from(position.x), f64::from(position.y)),
            force: None,
            id,
        }
    }

    #[test]
    fn left_stick_holds_movement_keys_until_released() {
        let layout = layout();
        let mut controls = TouchControls::new(true);
        let mut input = InputState::default();
        let start = Pos2::new(300.0, 500.0);
        controls.handle(&touch(1, TouchPhase::Started, start), &layout, &mut input);
        controls.handle(
            &touch(1, TouchPhase::Moved, start + Vec2::new(10.0, -100.0)),
            &layout,
            &mut input,
        );
        assert!(input.keys.contains(&KeyCode::KeyW));
        assert!(!input.keys.contains(&KeyCode::KeyD));

        controls.handle(
            &touch(1, TouchPhase::Moved, start + Vec2::new(100.0, 0.0)),
            &layout,
            &mut input,
        );
        assert!(input.keys.contains(&KeyCode::KeyD));
        assert!(!input.keys.contains(&KeyCode::KeyW));

        controls.handle(&touch(1, TouchPhase::Ended, start), &layout, &mut input);
        assert!(input.keys.is_empty());
    }

    #[test]
    fn dragging_elsewhere_produces_logical_mouse_motion() {
        let layout = layout();
        let mut controls = TouchControls::new(true);
        let mut input = InputState::default();
        let start = Pos2::new(1000.0, 300.0);
        controls.handle(&touch(7, TouchPhase::Started, start), &layout, &mut input);
        controls.handle(
            &touch(7, TouchPhase::Moved, start + Vec2::new(40.0, -20.0)),
            &layout,
            &mut input,
        );
        assert_eq!(
            input.mouse_delta,
            (f64::from(20.0 * LOOK_SCALE), f64::from(-10.0 * LOOK_SCALE))
        );
        assert!(input.keys.is_empty());
    }

    #[test]
    fn buttons_hold_their_desktop_inputs_and_menu_is_requested() {
        let layout = layout();
        let mut controls = TouchControls::new(true);
        let mut input = InputState::default();

        let jump = layout.center(Button::Jump);
        controls.handle(&touch(1, TouchPhase::Started, jump), &layout, &mut input);
        let cast = layout.center(Button::Cast);
        controls.handle(&touch(2, TouchPhase::Started, cast), &layout, &mut input);
        assert!(input.keys.contains(&KeyCode::Space));
        assert!(input.space_requested && input.jump_requested);
        assert!(input.cast_mouse && input.cast_requested);

        controls.handle(&touch(2, TouchPhase::Ended, cast), &layout, &mut input);
        assert!(!input.cast_mouse && input.cast_release_requested);
        controls.handle(&touch(1, TouchPhase::Cancelled, jump), &layout, &mut input);
        assert!(!input.keys.contains(&KeyCode::Space) && input.space_release_requested);

        let menu = layout.center(Button::Menu);
        assert!(controls.handle(&touch(3, TouchPhase::Started, menu), &layout, &mut input));
    }

    #[test]
    fn controls_stay_inside_the_presented_game_area() {
        let area = Rect::from_min_size(Pos2::new(300.0, 0.0), Vec2::new(1000.0, 750.0));
        let layout = Layout::new(area, 3.0);
        for button in Button::ALL {
            let radius = layout.radius(button);
            let bounds = Rect::from_center_size(layout.center(button), Vec2::splat(radius * 2.0));
            assert!(
                area.contains_rect(bounds),
                "{button:?} leaves the game area"
            );
        }
        let stick = Rect::from_center_size(
            layout.stick_home(),
            Vec2::splat(layout.stick_radius() * 2.0),
        );
        assert!(area.contains_rect(stick));
    }
}
