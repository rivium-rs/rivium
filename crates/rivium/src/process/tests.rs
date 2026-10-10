//! The parts of the process host that need no process of their own; the rest needs a real
//! program to test.

use super::console_event;
use crate::lifecycle::StopReason;

#[test]
fn console_control_events_stand_for_sigint_or_sigterm() {
    for (event, number) in [
        ("CTRL_C", 2),
        ("CTRL_BREAK", 2),
        ("CTRL_CLOSE", 15),
        ("CTRL_LOGOFF", 15),
        ("CTRL_SHUTDOWN", 15),
    ] {
        assert_eq!(
            console_event(event),
            StopReason::Signal {
                name: event,
                number
            }
        );
    }
}
