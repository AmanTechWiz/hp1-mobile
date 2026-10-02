//! On-screen controls for touch screens, which have no keyboard or mouse.
//!
//! Touches feed the same [`InputState`] as the desktop controls, so gameplay
//! receives the original input axes either way. The overlay reads like a
//! gamepad HUD and is fully drawn from the first frame: the stick base with
//! its center dot sits bottom left, the action buttons bottom right, and
//! Pause top right, so nothing has to be discovered by touching. A floating
//! analog stick appears wherever a thumb lands on the left half and moves
//! Harry in any direction, scaled by how far it is pushed, like a gamepad
//! stick. Dragging anywhere else looks around, and the buttons hold their
//! mapped key or mouse button for as long as they are touched.
//!
//! The stick behaves like the virtual sticks of console emulators: its base
//! follows the thumb when it is pulled past the rim, so the thumb never runs
//! out of travel; the movement axes ease in over a few milliseconds instead of
//! snapping; and the knob glides with the thumb and back to center on release.

use egui::{Align2, Color32, FontId, Id, LayerId, Order, Pos2, Rect, Stroke, Vec2};
use winit::{
    event::{ElementState, MouseButton, Touch, TouchPhase},
    keyboard::KeyCode,
};

use super::InputState;

/// Mouse counts produced per logical pixel of finger travel while looking.
const LOOK_SCALE: f32 = 2.0;
/// Fraction of the stick radius ignored around its center, so a resting
/// thumb does not drift.
const STICK_DEAD_ZONE: f32 = 0.1;
/// Time constant of the easing applied to the movement axes, in seconds. Short
/// enough to be imperceptible as lag, long enough to smooth thumb jitter and
/// the jump from rest to a full push.
const STICK_RESPONSE_SECONDS: f32 = 0.035;
/// Time constant of the knob's glide, in seconds.
const KNOB_GLIDE_SECONDS: f32 = 0.03;
/// Time constant of the knob's return and fade after release, in seconds.
const KNOB_RELEASE_SECONDS: f32 = 0.07;
/// Touches within this multiple of a button's drawn radius press it.
const BUTTON_HIT_SCALE: f32 = 1.35;
/// Logical height of the game area at which controls reach full size.
const FULL_SIZE_HEIGHT: f32 = 400.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Button {
    Pause,
    Jump,
    Cast,
    Boost,
    Brake,
    Skip,
}

impl Button {
    const ALL: [Self; 6] = [
        Self::Pause,
        Self::Jump,
        Self::Cast,
        Self::Boost,
        Self::Brake,
        Self::Skip,
    ];

    /// Boost and Brake only act on the broom.
    fn broom_only(self) -> bool {
        matches!(self, Self::Boost | Self::Brake)
    }

    fn label(self) -> &'static str {
        match self {
            Self::Pause => "",
            Self::Jump => "Jump",
            Self::Cast => "Cast",
            Self::Boost => "Boost",
            Self::Brake => "Brake",
            Self::Skip => "Skip",
        }
    }

    fn set(self, input: &mut InputState, state: ElementState) {
        match self {
            Self::Pause => {}
            Self::Jump => input.set_key(KeyCode::Space, state),
            Self::Cast => input.set_mouse_button(MouseButton::Left, state),
            Self::Boost => input.set_key(KeyCode::KeyZ, state),
            Self::Brake => input.set_key(KeyCode::KeyX, state),
            Self::Skip => {
                // One-shot flag: `InputState::player_input` clears it every
                // frame, so releasing the button needs no key-up.
                if state == ElementState::Pressed {
                    input.skip_requested = true;
                }
            }
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
    /// `area` shrunk by the screen's safe-area insets (notch, rounded corners,
    /// home indicator): controls sit inside it, while touches anywhere in
    /// `area` still look around.
    safe: Rect,
    scale_factor: f32,
    unit: f32,
}

impl Layout {
    /// `insets` are the `[top, right, bottom, left]` safe-area insets in
    /// physical pixels.
    pub(super) fn new(area: Rect, scale_factor: f32, insets: [f32; 4]) -> Self {
        let scale_factor = scale_factor.max(0.01);
        let unit = scale_factor * (area.height() / scale_factor / FULL_SIZE_HEIGHT).clamp(0.6, 1.0);
        let [top, right, bottom, left] = insets.map(|inset| inset.max(0.0));
        let safe = Rect::from_min_max(
            area.min + Vec2::new(left, top),
            area.max - Vec2::new(right, bottom),
        );
        // Insets that swallow the whole area would leave no room for controls.
        let safe = if safe.width() > area.width() * 0.5 && safe.height() > area.height() * 0.5 {
            safe
        } else {
            area
        };
        Self {
            area,
            safe,
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
        Pos2::new(self.safe.min.x + inset, self.safe.max.y - inset)
    }

    /// The drawn circle is also the floor of the hit target: [`Self::button_at`]
    /// accepts touches up to [`BUTTON_HIT_SCALE`] times this, so both sizes
    /// grow together. The cluster spacing in [`Self::center`] is 2.5 radii of
    /// the large buttons, which keeps neighbouring targets from overlapping.
    fn radius(&self, button: Button) -> f32 {
        match button {
            Button::Jump | Button::Cast => 42.0 * self.unit,
            Button::Pause | Button::Boost | Button::Brake | Button::Skip => 30.0 * self.unit,
        }
    }

    fn center(&self, button: Button) -> Pos2 {
        let margin = self.margin();
        let large = self.radius(Button::Jump);
        let jump = Pos2::new(
            self.safe.max.x - margin - large,
            self.safe.max.y - margin - large,
        );
        match button {
            // Top right: the original HUD draws Harry's health in the top left.
            Button::Pause => {
                let inset = margin + self.radius(Button::Pause);
                Pos2::new(self.safe.max.x - inset, self.safe.min.y + inset)
            }
            Button::Jump => jump,
            Button::Cast => jump - Vec2::new(large * 2.5, 0.0),
            Button::Boost => jump - Vec2::new(0.0, large * 2.5),
            Button::Brake => jump - Vec2::splat(large * 2.5),
            // Under Pause: a cutscene covers the whole screen, so Skip shares
            // the top-right corner instead of blocking the center, and stays
            // clear of the HUD, the stick, and the action cluster.
            Button::Skip => {
                let inset = margin + self.radius(Button::Skip);
                let pause_bottom = margin + self.radius(Button::Pause) * 2.0;
                Pos2::new(
                    self.safe.max.x - inset,
                    self.safe.min.y + pause_bottom + inset,
                )
            }
        }
    }

    fn button_at(&self, position: Pos2, broom: bool, cutscene: bool) -> Option<Button> {
        visible_buttons(broom, cutscene).find(|button| {
            position.distance(self.center(*button)) <= self.radius(*button) * BUTTON_HIT_SCALE
        })
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
    broom: bool,
    cutscene: bool,
    touches: Vec<(u64, Role)>,
    /// Where the thumb currently points, as `[right, forward]` axes.
    stick_target: [f32; 2],
    /// `stick_target` eased over [`STICK_RESPONSE_SECONDS`], the value gameplay sees.
    stick_value: [f32; 2],
    /// Knob offset from the stick origin the thumb asks for, in stick radii.
    knob_target: Vec2,
    /// The drawn knob offset, gliding toward `knob_target`.
    knob: Vec2,
    /// Where the floating base was last anchored.
    base: Pos2,
    /// Fades the floating stick in while touched and out after release.
    glow: f32,
}

/// Iterates the buttons currently drawn and hit-testable: broom-only buttons
/// while flying, and Skip only while a cutscene can be skipped.
fn visible_buttons(broom: bool, cutscene: bool) -> impl Iterator<Item = Button> {
    Button::ALL
        .into_iter()
        .filter(move |button| broom || !button.broom_only())
        .filter(move |button| cutscene || *button != Button::Skip)
}

impl TouchControls {
    pub(super) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            broom: false,
            cutscene: false,
            touches: Vec::new(),
            stick_target: [0.0; 2],
            stick_value: [0.0; 2],
            knob_target: Vec2::ZERO,
            knob: Vec2::ZERO,
            base: Pos2::ZERO,
            glow: 0.0,
        }
    }

    pub(super) fn enabled(&self) -> bool {
        self.enabled
    }

    /// Shows the broom-only buttons while Harry flies.
    pub(super) fn set_broom(&mut self, broom: bool) {
        self.broom = broom;
    }

    /// Shows the Skip button while a cutscene is active, dropping any Skip
    /// touch still held when the cutscene ends so the button cannot stick.
    pub(super) fn set_cutscene(&mut self, cutscene: bool) {
        self.cutscene = cutscene;
        if !cutscene {
            self.touches
                .retain(|(_, role)| !matches!(role, Role::Button(Button::Skip)));
        }
    }

    /// Applies one touch to `input`, returning whether pausing was requested.
    pub(super) fn handle(
        &mut self,
        touch: &Touch,
        layout: &Layout,
        input: &mut InputState,
    ) -> bool {
        let position = Pos2::new(touch.location.x as f32, touch.location.y as f32);
        let existing = self.touches.iter().position(|(id, _)| *id == touch.id);
        // `release()` forgets every role when the input state is cleared
        // (menu open, focus lost), but the fingers themselves may still be
        // down. Their later events would find no role and do nothing until
        // lifted, so an untracked finger is adopted again on `Moved` as if it
        // had just landed: the first move only records a position, so nothing
        // jumps. Pause stays edge-triggered on `Started`, because a thumb
        // resuming over it must not bounce straight back into the menu, and
        // an untracked `Ended`/`Cancelled` holds nothing, so it is ignored.
        match (touch.phase, existing) {
            (TouchPhase::Started | TouchPhase::Moved, None) => {
                let role =
                    if let Some(button) = layout.button_at(position, self.broom, self.cutscene) {
                        if button == Button::Pause && touch.phase == TouchPhase::Started {
                            return true;
                        }
                        button.set(input, ElementState::Pressed);
                        Role::Button(button)
                    } else if position.x < layout.area.center().x && !self.has_stick() {
                        self.stick_target = [0.0; 2];
                        self.knob_target = Vec2::ZERO;
                        if self.glow <= 0.01 {
                            self.knob = Vec2::ZERO;
                        }
                        self.base = position;
                        Role::Stick {
                            origin: position,
                            position,
                        }
                    } else {
                        Role::Look { last: position }
                    };
                self.touches.push((touch.id, role));
            }
            (TouchPhase::Moved, Some(index)) => {
                // Only the first look finger steers the camera; a second one
                // would accumulate into the same delta and double the look
                // speed. It keeps tracking its own position anyway, so
                // taking over when the leader lifts starts from where it is.
                let leads_look = self
                    .touches
                    .iter()
                    .position(|(_, role)| matches!(role, Role::Look { .. }))
                    == Some(index);
                let mut stick = None;
                match &mut self.touches[index].1 {
                    Role::Stick {
                        origin,
                        position: current,
                    } => {
                        *current = position;
                        // Pulling past the rim drags the base along, so the
                        // thumb always has the full radius to travel back.
                        let limit = layout.stick_radius();
                        let offset = position - *origin;
                        if offset.length() > limit {
                            *origin = position - offset.normalized() * limit;
                        }
                        stick = Some((*origin, (position - *origin) / limit));
                    }
                    Role::Look { last } => {
                        if leads_look {
                            let delta = (position - *last) * LOOK_SCALE / layout.scale_factor;
                            input.mouse_delta.0 += f64::from(delta.x);
                            input.mouse_delta.1 += f64::from(delta.y);
                        }
                        *last = position;
                    }
                    Role::Button(_) => {}
                }
                if let Some((origin, offset)) = stick {
                    self.base = origin;
                    self.knob_target = offset;
                    self.stick_target = stick_axes(offset);
                }
            }
            (TouchPhase::Ended | TouchPhase::Cancelled, Some(index)) => {
                match self.touches.remove(index).1 {
                    Role::Stick { .. } => self.release_stick(input),
                    Role::Button(button) => button.set(input, ElementState::Released),
                    Role::Look { .. } => {}
                }
            }
            _ => {}
        }
        false
    }

    /// Forgets every active touch after `input` was cleared, so a button
    /// cannot keep asserting an input the state no longer remembers. A finger
    /// that is physically still down is not lost: [`Self::handle`] adopts it
    /// again on its next `Moved` instead of leaving it dead until lifted.
    pub(super) fn release(&mut self) {
        self.touches.clear();
        self.stick_target = [0.0; 2];
        self.stick_value = [0.0; 2];
        self.knob_target = Vec2::ZERO;
    }

    /// Stops movement at once on release, since easing out would keep Harry
    /// walking after the thumb lifted; the knob still glides home visually.
    fn release_stick(&mut self, input: &mut InputState) {
        self.stick_target = [0.0; 2];
        self.stick_value = [0.0; 2];
        self.knob_target = Vec2::ZERO;
        input.stick = [0.0; 2];
    }

    /// Advances the stick's easing by `delta_time` seconds and publishes the
    /// movement axes. Called once per frame before the input is consumed.
    pub(super) fn update(&mut self, delta_time: f32, input: &mut InputState) {
        let delta_time = if delta_time.is_finite() {
            delta_time.clamp(0.0, 0.1)
        } else {
            0.0
        };
        let ease = |seconds: f32| 1.0 - (-delta_time / seconds).exp();
        if self.has_stick() {
            let response = ease(STICK_RESPONSE_SECONDS);
            for (value, target) in self.stick_value.iter_mut().zip(self.stick_target) {
                *value += (target - *value) * response;
                if (target - *value).abs() < 1e-3 {
                    *value = target;
                }
            }
            input.stick = self.stick_value;
            self.knob += (self.knob_target - self.knob) * ease(KNOB_GLIDE_SECONDS);
            self.glow += (1.0 - self.glow) * ease(KNOB_GLIDE_SECONDS);
        } else {
            let release = ease(KNOB_RELEASE_SECONDS);
            self.knob += (Vec2::ZERO - self.knob) * release;
            self.glow -= self.glow * release;
            if self.glow < 0.01 {
                self.glow = 0.0;
                self.knob = Vec2::ZERO;
            }
        }
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
        let stroke = Stroke::new(2.0, Color32::from_white_alpha(150));

        let radius = layout.stick_radius() * scale;
        // The base ring and center dot are always drawn at the stick's home,
        // so a first-time player sees where the stick will come up before
        // touching anything. While a thumb is down the home ring dims and the
        // floating base fades in under it, then fades out again on release.
        let home = to_screen(layout.stick_home());
        let glow = self.glow;
        let base = to_screen(self.base);
        let floating = glow > 0.0 && base.distance(home) > radius * 0.25;
        let home_alpha = if floating { 1.0 - glow * 0.6 } else { 1.0 };
        let faded = |alpha: u8| Color32::from_white_alpha((f32::from(alpha) * home_alpha) as u8);
        painter.circle(
            home,
            radius,
            Color32::from_black_alpha((70.0 * home_alpha) as u8),
            Stroke::new(1.5, faded(90)),
        );
        painter.circle_filled(home, radius * 0.1, faded(170));
        if glow > 0.0 {
            let origin = if floating { base } else { home };
            let alpha = |value: f32| (value * glow) as u8;
            if floating {
                painter.circle(
                    origin,
                    radius,
                    Color32::from_black_alpha(alpha(70.0)),
                    Stroke::new(1.5, Color32::from_white_alpha(alpha(110.0))),
                );
            }
            // The knob stays inside the rim even while the thumb is past it.
            let offset = if self.knob.length() > 1.0 {
                self.knob.normalized()
            } else {
                self.knob
            };
            let knob = origin + offset * radius;
            let knob_radius = radius * 0.42;
            painter.circle_filled(
                knob + Vec2::new(0.0, knob_radius * 0.12),
                knob_radius,
                Color32::from_black_alpha(alpha(60.0)),
            );
            painter.circle(
                knob,
                knob_radius,
                Color32::from_white_alpha(alpha(95.0)),
                Stroke::new(2.0, Color32::from_white_alpha(alpha(190.0))),
            );
            painter.circle_filled(
                knob - Vec2::splat(knob_radius * 0.22),
                knob_radius * 0.32,
                Color32::from_white_alpha(alpha(60.0)),
            );
        }

        for button in visible_buttons(self.broom, self.cutscene) {
            let center = to_screen(layout.center(button));
            let radius = layout.radius(button) * scale;
            let held = self.held(button);
            // A held button fills and outlines brightly, and flips its glyph
            // to dark, so the pressed state reads at a glance.
            let fill = if held {
                Color32::from_white_alpha(120)
            } else {
                Color32::from_black_alpha(90)
            };
            let button_stroke = if held {
                Stroke::new(3.0, Color32::from_white_alpha(240))
            } else {
                stroke
            };
            let glyph = if held {
                Color32::from_black_alpha(230)
            } else {
                Color32::from_white_alpha(230)
            };
            painter.circle(center, radius, fill, button_stroke);
            if button == Button::Pause {
                let bar = Vec2::new(radius * 0.16, radius * 0.5);
                for side in [-1.0, 1.0] {
                    painter.rect_filled(
                        Rect::from_center_size(
                            center + Vec2::new(side * radius * 0.2, 0.0),
                            bar * 2.0,
                        ),
                        radius * 0.05,
                        glyph,
                    );
                }
                continue;
            }
            painter.text(
                center,
                Align2::CENTER_CENTER,
                button.label(),
                FontId::proportional(radius * 0.6),
                glyph,
            );
        }
    }
}

/// Converts a screen-space stick offset, in stick radii, to `[right, forward]`
/// axes with a radial dead zone and a unit-length limit.
fn stick_axes(offset: Vec2) -> [f32; 2] {
    let length = offset.length();
    if length <= STICK_DEAD_ZONE {
        return [0.0; 2];
    }
    let strength = ((length - STICK_DEAD_ZONE) / (1.0 - STICK_DEAD_ZONE)).min(1.0);
    let direction = offset / length * strength;
    [direction.x, -direction.y]
}

#[cfg(test)]
mod tests {
    use winit::{dpi::PhysicalPosition, event::DeviceId};

    use super::*;

    fn layout() -> Layout {
        Layout::new(
            Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 800.0)),
            2.0,
            [0.0; 4],
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

    /// Runs the stick's easing long enough to settle on its target.
    fn settle(controls: &mut TouchControls, input: &mut InputState) {
        for _ in 0..60 {
            controls.update(1.0 / 60.0, input);
        }
    }

    #[test]
    fn left_stick_moves_in_any_direction_by_how_far_it_is_pushed() {
        let layout = layout();
        let radius = layout.stick_radius();
        let mut controls = TouchControls::new(true);
        let mut input = InputState::default();
        let start = Pos2::new(300.0, 500.0);
        controls.handle(&touch(1, TouchPhase::Started, start), &layout, &mut input);
        let mut drag = |offset: Vec2, input: &mut InputState| {
            controls.handle(&touch(1, TouchPhase::Moved, start + offset), &layout, input);
            settle(&mut controls, input);
            input.stick
        };

        assert_eq!(drag(Vec2::new(0.0, -radius * 0.05), &mut input), [0.0; 2]);
        let [right, forward] = drag(Vec2::new(radius, radius).normalized() * radius, &mut input);
        assert!((right - forward.abs()).abs() < 1e-3 && forward < 0.0);
        assert!(((right * right + forward * forward).sqrt() - 1.0).abs() < 1e-3);
        let [right, forward] = drag(Vec2::new(0.0, -radius * 2.0), &mut input);
        assert!(right.abs() < 1e-3 && (forward - 1.0).abs() < 1e-3);
        assert!(input.keys.is_empty());

        controls.handle(&touch(1, TouchPhase::Ended, start), &layout, &mut input);
        // Releasing stops movement at once rather than easing out.
        assert_eq!(input.stick, [0.0; 2]);
    }

    #[test]
    fn stick_axes_ease_in_instead_of_snapping() {
        let layout = layout();
        let radius = layout.stick_radius();
        let mut controls = TouchControls::new(true);
        let mut input = InputState::default();
        let start = Pos2::new(300.0, 500.0);
        controls.handle(&touch(1, TouchPhase::Started, start), &layout, &mut input);
        controls.handle(
            &touch(1, TouchPhase::Moved, start + Vec2::new(0.0, -radius)),
            &layout,
            &mut input,
        );
        controls.update(1.0 / 60.0, &mut input);
        let first = input.stick[1];
        assert!(first > 0.0 && first < 0.9, "first frame was {first}");
        settle(&mut controls, &mut input);
        assert!((input.stick[1] - 1.0).abs() < 1e-3);
    }

    #[test]
    fn the_base_follows_a_thumb_pulled_past_the_rim() {
        let layout = layout();
        let radius = layout.stick_radius();
        let mut controls = TouchControls::new(true);
        let mut input = InputState::default();
        let start = Pos2::new(300.0, 500.0);
        controls.handle(&touch(1, TouchPhase::Started, start), &layout, &mut input);
        let far = start + Vec2::new(radius * 3.0, 0.0);
        controls.handle(&touch(1, TouchPhase::Moved, far), &layout, &mut input);
        settle(&mut controls, &mut input);
        assert!((input.stick[0] - 1.0).abs() < 1e-3);

        // Coming back by one radius is already the center; no travel is lost.
        controls.handle(
            &touch(1, TouchPhase::Moved, far - Vec2::new(radius, 0.0)),
            &layout,
            &mut input,
        );
        settle(&mut controls, &mut input);
        assert!(input.stick[0] < 0.01);
    }

    #[test]
    fn controls_move_inside_the_safe_area() {
        let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(2556.0, 1179.0));
        let insets = [0.0, 150.0, 60.0, 150.0];
        let layout = Layout::new(area, 3.0, insets);
        for button in Button::ALL {
            let radius = layout.radius(button);
            let bounds = Rect::from_center_size(layout.center(button), Vec2::splat(radius * 2.0));
            assert!(
                layout.safe.contains_rect(bounds),
                "{button:?} leaves the safe area"
            );
        }
        assert!(layout.safe.min.x >= 150.0 && layout.safe.max.x <= 2556.0 - 150.0);
        // A touch outside the safe area still looks around.
        assert_eq!(
            layout.button_at(Pos2::new(2540.0, 1170.0), true, true),
            None
        );
    }

    #[test]
    fn stick_axes_scale_the_original_movement_axes() {
        let mut input = InputState {
            stick: [0.5, 0.5],
            ..Default::default()
        };
        let player = input.player_input(1.0 / 60.0);
        assert_eq!(player.base_y, 3_000.0);
        assert_eq!(player.strafe, 3_000.0);
        assert_eq!(player.base_x, 1_500.0);
        assert!(player.broom_pitch_up && !player.broom_pitch_down);

        input.stick = [0.0, -0.4];
        let player = input.player_input(1.0 / 60.0);
        assert_eq!(player.base_y, -1_200.0);
        assert!(!player.broom_pitch_up && !player.broom_pitch_down);
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
    fn only_the_first_look_finger_drives_the_camera() {
        let layout = layout();
        let mut controls = TouchControls::new(true);
        let mut input = InputState::default();
        let leader = Pos2::new(1000.0, 300.0);
        let second = Pos2::new(1000.0, 500.0);
        controls.handle(&touch(7, TouchPhase::Started, leader), &layout, &mut input);
        controls.handle(&touch(8, TouchPhase::Started, second), &layout, &mut input);

        // The second finger would otherwise accumulate into the same delta
        // and double the look speed.
        controls.handle(
            &touch(8, TouchPhase::Moved, second + Vec2::new(30.0, -10.0)),
            &layout,
            &mut input,
        );
        assert_eq!(input.mouse_delta, (0.0, 0.0));

        controls.handle(
            &touch(7, TouchPhase::Moved, leader + Vec2::new(24.0, -12.0)),
            &layout,
            &mut input,
        );
        assert_eq!(
            input.mouse_delta,
            (f64::from(12.0 * LOOK_SCALE), f64::from(-6.0 * LOOK_SCALE))
        );

        // Once the leader lifts, the second finger takes over from its own
        // tracked position, so the camera does not jump.
        input.mouse_delta = (0.0, 0.0);
        controls.handle(&touch(7, TouchPhase::Ended, leader), &layout, &mut input);
        controls.handle(
            &touch(8, TouchPhase::Moved, second + Vec2::new(50.0, -10.0)),
            &layout,
            &mut input,
        );
        assert_eq!(input.mouse_delta.0, f64::from(10.0 * LOOK_SCALE));
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

        let pause = layout.center(Button::Pause);
        assert!(controls.handle(&touch(3, TouchPhase::Started, pause), &layout, &mut input));
    }

    #[test]
    fn touches_still_down_when_the_menu_opens_recover_on_resume() {
        let layout = layout();
        let mut controls = TouchControls::new(true);
        let mut input = InputState::default();
        let jump = layout.center(Button::Jump);
        let look = Pos2::new(1000.0, 300.0);
        controls.handle(&touch(1, TouchPhase::Started, jump), &layout, &mut input);
        controls.handle(&touch(2, TouchPhase::Started, look), &layout, &mut input);

        // Opening the menu clears the input state and forgets the roles,
        // while both fingers are physically still down.
        input.clear();
        controls.release();
        assert!(input.keys.is_empty());

        // Their motion adopts them again instead of leaving them dead until
        // lifted: the button re-asserts its key, the look finger re-anchors
        // on its first move and drives look from the second one on.
        controls.handle(&touch(1, TouchPhase::Moved, jump), &layout, &mut input);
        assert!(controls.held(Button::Jump));
        assert!(input.keys.contains(&KeyCode::Space));
        controls.handle(
            &touch(2, TouchPhase::Moved, look + Vec2::new(4.0, 0.0)),
            &layout,
            &mut input,
        );
        assert_eq!(input.mouse_delta, (0.0, 0.0));
        controls.handle(
            &touch(2, TouchPhase::Moved, look + Vec2::new(24.0, 0.0)),
            &layout,
            &mut input,
        );
        assert_eq!(input.mouse_delta.0, f64::from(10.0 * LOOK_SCALE));

        // A resuming thumb resting over Pause is adopted without reopening
        // the menu; Pause only fires from a fresh tap.
        controls.release();
        let pause = layout.center(Button::Pause);
        assert!(!controls.handle(&touch(1, TouchPhase::Moved, pause), &layout, &mut input));
        controls.handle(&touch(1, TouchPhase::Ended, pause), &layout, &mut input);

        // Lifting a finger that was never tracked holds nothing.
        assert!(!controls.handle(&touch(3, TouchPhase::Ended, look), &layout, &mut input));
    }

    #[test]
    fn broom_buttons_only_respond_while_flying() {
        let layout = layout();
        let mut controls = TouchControls::new(true);
        let mut input = InputState::default();
        let boost = layout.center(Button::Boost);

        controls.handle(&touch(1, TouchPhase::Started, boost), &layout, &mut input);
        assert!(!input.keys.contains(&KeyCode::KeyZ));
        controls.handle(&touch(1, TouchPhase::Ended, boost), &layout, &mut input);

        controls.set_broom(true);
        controls.handle(&touch(2, TouchPhase::Started, boost), &layout, &mut input);
        assert!(input.keys.contains(&KeyCode::KeyZ));
    }

    #[test]
    fn skip_button_only_appears_during_a_cutscene() {
        let layout = layout();
        let mut controls = TouchControls::new(true);
        let mut input = InputState::default();
        let skip = layout.center(Button::Skip);

        assert!(!visible_buttons(false, false).any(|button| button == Button::Skip));
        // Without a cutscene the point is plain look-around, not a button.
        controls.handle(&touch(1, TouchPhase::Started, skip), &layout, &mut input);
        assert!(!input.skip_requested);
        controls.handle(&touch(1, TouchPhase::Ended, skip), &layout, &mut input);

        controls.set_cutscene(true);
        assert!(visible_buttons(false, true).any(|button| button == Button::Skip));
        controls.handle(&touch(2, TouchPhase::Started, skip), &layout, &mut input);
        assert!(input.skip_requested);
    }

    #[test]
    fn pressing_skip_requests_a_one_shot_skip() {
        let layout = layout();
        let mut controls = TouchControls::new(true);
        controls.set_cutscene(true);
        let mut input = InputState::default();
        let skip = layout.center(Button::Skip);

        controls.handle(&touch(1, TouchPhase::Started, skip), &layout, &mut input);
        assert!(input.skip_requested);
        input.player_input(1.0 / 60.0);
        assert!(!input.skip_requested);

        // Releasing, like the desktop Enter key, requests nothing new.
        controls.handle(&touch(1, TouchPhase::Ended, skip), &layout, &mut input);
        assert!(!input.skip_requested);
    }

    #[test]
    fn ending_the_cutscene_releases_a_held_skip_touch() {
        let layout = layout();
        let mut controls = TouchControls::new(true);
        controls.set_cutscene(true);
        let mut input = InputState::default();
        let skip = layout.center(Button::Skip);

        controls.handle(&touch(1, TouchPhase::Started, skip), &layout, &mut input);
        assert!(controls.held(Button::Skip));
        controls.set_cutscene(false);
        assert!(!controls.held(Button::Skip));
        // The stale touch for the vanished button requests nothing on release.
        input.skip_requested = false;
        controls.handle(&touch(1, TouchPhase::Ended, skip), &layout, &mut input);
        assert!(!input.skip_requested);
    }

    #[test]
    fn controls_stay_inside_the_presented_game_area() {
        let area = Rect::from_min_size(Pos2::new(300.0, 0.0), Vec2::new(1000.0, 750.0));
        let layout = Layout::new(area, 3.0, [0.0; 4]);
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
