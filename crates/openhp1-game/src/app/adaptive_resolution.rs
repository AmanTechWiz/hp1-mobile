//! Keeps frame pacing smooth on slow GPUs by trading internal resolution for
//! frame time.
//!
//! The scale steps down quickly once frames run clearly below 60 fps and
//! climbs back slowly after a long stable stretch. A step up that fails
//! within [`RETRY_SECONDS`] caps the scale until the cap expires, so the game
//! does not oscillate between two sizes.

/// Fractions of the configured height the scaler may pick, lowest first.
const SCALES: [f32; 5] = [0.5, 0.625, 0.75, 0.875, 1.0];
/// Frame time above which a frame counts as slow, in milliseconds (~45 fps).
const SLOW_MS: f32 = 22.0;
/// Frame time at or below which a frame counts as comfortably fast.
const FAST_MS: f32 = 18.5;
/// Slow frames, net of fast recoveries, that trigger a step down.
const SLOW_FRAMES_TO_DROP: u32 = 36;
/// Consecutive fast frames before trying a larger size (~6 seconds at 60 fps).
const FAST_FRAMES_TO_RISE: u32 = 360;
/// Frames longer than this are loading hitches and are ignored.
const HITCH_MS: f32 = 100.0;
/// A drop this soon after a rise caps the scale at the lower step.
const RETRY_SECONDS: f32 = 30.0;
/// How long such a cap lasts.
const CAP_SECONDS: f32 = 120.0;

#[derive(Clone, Debug)]
pub(super) struct AdaptiveResolution {
    index: usize,
    cap: usize,
    cap_remaining: f32,
    since_rise: f32,
    slow: u32,
    fast: u32,
}

impl Default for AdaptiveResolution {
    fn default() -> Self {
        Self {
            index: SCALES.len() - 1,
            cap: SCALES.len() - 1,
            cap_remaining: 0.0,
            since_rise: f32::INFINITY,
            slow: 0,
            fast: 0,
        }
    }
}

impl AdaptiveResolution {
    /// The height to render at for a configured height.
    pub(super) fn height(&self, configured: u32) -> u32 {
        (((configured as f32 * SCALES[self.index]).round() as u32).max(2) + 1) & !1
    }

    /// Restarts at full size, for when the player changes the resolution.
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    /// Feeds one frame's duration; returns whether the scale changed.
    pub(super) fn update(&mut self, frame_ms: f32) -> bool {
        if !frame_ms.is_finite() || frame_ms > HITCH_MS {
            self.slow = 0;
            self.fast = 0;
            return false;
        }
        let seconds = frame_ms / 1_000.0;
        self.since_rise += seconds;
        if self.cap_remaining > 0.0 {
            self.cap_remaining -= seconds;
            if self.cap_remaining <= 0.0 {
                self.cap = SCALES.len() - 1;
            }
        }
        if frame_ms > SLOW_MS {
            self.fast = 0;
            self.slow += 1;
            if self.slow >= SLOW_FRAMES_TO_DROP && self.index > 0 {
                if self.since_rise < RETRY_SECONDS {
                    self.cap = self.index - 1;
                    self.cap_remaining = CAP_SECONDS;
                }
                self.index -= 1;
                self.slow = 0;
                return true;
            }
        } else {
            self.slow = self.slow.saturating_sub(1);
            if frame_ms <= FAST_MS {
                self.fast += 1;
            } else {
                self.fast = 0;
            }
            if self.fast >= FAST_FRAMES_TO_RISE && self.index < self.cap {
                self.index += 1;
                self.fast = 0;
                self.since_rise = 0.0;
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(scaler: &mut AdaptiveResolution, frames: u32, frame_ms: f32) -> u32 {
        (0..frames).filter(|_| scaler.update(frame_ms)).count() as u32
    }

    #[test]
    fn drops_when_frames_are_slow_and_keeps_even_heights() {
        let mut scaler = AdaptiveResolution::default();
        assert_eq!(scaler.height(720), 720);
        assert_eq!(run(&mut scaler, SLOW_FRAMES_TO_DROP, 30.0), 1);
        assert_eq!(scaler.height(720), 630);
        assert_eq!(scaler.height(719) % 2, 0);
    }

    #[test]
    fn steady_sixty_fps_never_changes_size() {
        let mut scaler = AdaptiveResolution::default();
        assert_eq!(run(&mut scaler, 10_000, 16.7), 0);
        assert_eq!(scaler.height(720), 720);
    }

    #[test]
    fn hitches_are_ignored() {
        let mut scaler = AdaptiveResolution::default();
        assert_eq!(run(&mut scaler, 500, 400.0), 0);
        assert_eq!(scaler.height(720), 720);
    }

    #[test]
    fn rises_after_a_long_stable_stretch_and_a_failed_rise_is_capped() {
        let mut scaler = AdaptiveResolution::default();
        run(&mut scaler, SLOW_FRAMES_TO_DROP, 30.0);
        assert_eq!(run(&mut scaler, FAST_FRAMES_TO_RISE, 16.0), 1);
        assert_eq!(scaler.height(720), 720);

        // The larger size is too slow again right away: drop and cap it.
        run(&mut scaler, SLOW_FRAMES_TO_DROP, 30.0);
        assert_eq!(scaler.height(720), 630);
        assert_eq!(run(&mut scaler, FAST_FRAMES_TO_RISE, 16.0), 0);
        assert_eq!(scaler.height(720), 630);
    }

    #[test]
    fn never_drops_below_the_smallest_step() {
        let mut scaler = AdaptiveResolution::default();
        run(&mut scaler, SLOW_FRAMES_TO_DROP * 20, 40.0);
        assert_eq!(scaler.height(720), 360);
    }
}
