/// Visibility rule for the wrist panel: a single button press toggles it, so it
/// stays on screen until the user closes it. There is deliberately no idle
/// timeout — one used to close the panel mid-interaction.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct WristVisibilityPolicy {
    opened: bool,
}

impl WristVisibilityPolicy {
    /// Flip open/closed; used by the activation button edge detection.
    pub fn toggle(&mut self) {
        self.opened = !self.opened;
    }

    pub fn open(&mut self) {
        self.opened = true;
    }

    pub fn close(&mut self) {
        self.opened = false;
    }

    pub fn evaluate(&self, device_present: bool) -> bool {
        device_present && self.opened
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_by_default() {
        let policy = WristVisibilityPolicy::default();
        assert!(!policy.evaluate(true));
    }

    #[test]
    fn toggle_opens_and_stays_open() {
        let mut policy = WristVisibilityPolicy::default();
        policy.toggle();
        assert!(policy.evaluate(true));
        // No idle timeout: still visible on any later evaluation.
        assert!(policy.evaluate(true));
        policy.toggle();
        assert!(!policy.evaluate(true));
    }

    #[test]
    fn missing_device_hides_surface_but_keeps_state() {
        let mut policy = WristVisibilityPolicy::default();
        policy.toggle();
        assert!(!policy.evaluate(false));
        // The device coming back restores the panel without another press.
        assert!(policy.evaluate(true));
    }

    #[test]
    fn close_hides_immediately() {
        let mut policy = WristVisibilityPolicy::default();
        policy.open();
        policy.close();
        assert!(!policy.evaluate(true));
    }
}
