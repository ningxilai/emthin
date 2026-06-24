use smithay::delegate_output;
use smithay::wayland::output::OutputHandler;

use crate::EmthinState;

impl OutputHandler for EmthinState {}
delegate_output!(EmthinState);
