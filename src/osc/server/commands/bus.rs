//! `/bus_*`: reading and writing control buses.
//!
//! Five commands over one shape -- a value, a range, or a fill -- all of them
//! writing the shared atomics directly rather than going through the engine's
//! command FIFO, because a control bus is exactly the state both threads may
//! touch. The audio-side `/bus_tap` family lives in [`super::super::streams`].

use super::super::*;

impl OscServer {
    /// Control buses are shared atomics: set directly, no engine round-trip.
    pub(in crate::osc::server) fn handle_bus_set(&mut self, msg: &OscMessage, from: ClientId) {
        if msg.args.is_empty() || !msg.args.len().is_multiple_of(2) {
            return self.fail(from, "/bus_set", "expected (busIndex, value) pairs");
        }
        for pair in msg.args.chunks(2) {
            let (OscType::Int(index), Some(value)) = (&pair[0], float_value(&pair[1])) else {
                return self.fail(from, "/bus_set", "expected int bus index and number value");
            };
            if *index < 0 {
                return self.fail(from, "/bus_set", "bus index must be non-negative");
            }
            self.handle.control_buses().set(*index as usize, value);
        }
    }

    /// Replies with a `/bus_get.reply` message carrying (busIndex, value) pairs.
    pub(in crate::osc::server) fn handle_bus_get(&mut self, msg: &OscMessage, from: ClientId) {
        let mut args = Vec::with_capacity(msg.args.len() * 2);
        for arg in &msg.args {
            let OscType::Int(index) = arg else {
                return self.fail(from, "/bus_get", "expected int bus indices");
            };
            if *index < 0 {
                return self.fail(from, "/bus_get", "bus index must be non-negative");
            }
            args.push(OscType::Int(*index));
            args.push(OscType::Float(
                self.handle.control_buses().get(*index as usize),
            ));
        }
        self.reply(from, "/bus_get.reply", args);
    }

    /// `/bus_setRange busIndex numBuses val...`: sets a consecutive range of control
    /// buses (one or more groups). Immediate form writes the shared atomics.
    pub(in crate::osc::server) fn handle_bus_set_range(
        &mut self,
        msg: &OscMessage,
        from: ClientId,
    ) {
        let mut rest = msg.args.as_slice();
        while !rest.is_empty() {
            let [OscType::Int(base), OscType::Int(count), tail @ ..] = rest else {
                return self.fail(
                    from,
                    "/bus_setRange",
                    "expected (busIndex, numBuses, values...) groups",
                );
            };
            let (Ok(base), Ok(count)) = (usize::try_from(*base), usize::try_from(*count)) else {
                return self.fail(from, "/bus_setRange", "bus index and numBuses must be >= 0");
            };
            if tail.len() < count {
                return self.fail(from, "/bus_setRange", "fewer values than numBuses");
            }
            for (offset, value) in tail[..count].iter().enumerate() {
                let Some(value) = float_value(value) else {
                    return self.fail(from, "/bus_setRange", "expected number values");
                };
                self.handle.control_buses().set(base + offset, value);
            }
            rest = &tail[count..];
        }
    }

    /// `/bus_getRange busIndex numBuses ...`: replies `/bus_getRange.reply` with each requested
    /// range expanded to `(busIndex, numBuses, val0, val1, ...)`.
    pub(in crate::osc::server) fn handle_bus_get_range(
        &mut self,
        msg: &OscMessage,
        from: ClientId,
    ) {
        if msg.args.is_empty() || !msg.args.len().is_multiple_of(2) {
            return self.fail(from, "/bus_getRange", "expected (busIndex, numBuses) pairs");
        }
        let mut args = Vec::new();
        for pair in msg.args.chunks(2) {
            let [OscType::Int(base), OscType::Int(count)] = pair else {
                return self.fail(from, "/bus_getRange", "expected int busIndex and numBuses");
            };
            let (Ok(base), Ok(count)) = (usize::try_from(*base), usize::try_from(*count)) else {
                return self.fail(from, "/bus_getRange", "bus index and numBuses must be >= 0");
            };
            args.push(OscType::Int(base as i32));
            args.push(OscType::Int(count as i32));
            for offset in 0..count {
                args.push(OscType::Float(
                    self.handle.control_buses().get(base + offset),
                ));
            }
        }
        self.reply(from, "/bus_getRange.reply", args);
    }

    /// `/bus_fill busIndex numBuses value ...`: fills a consecutive range of
    /// control buses with one value (groups of three).
    pub(in crate::osc::server) fn handle_bus_fill(&mut self, msg: &OscMessage, from: ClientId) {
        if msg.args.is_empty() || !msg.args.len().is_multiple_of(3) {
            return self.fail(
                from,
                "/bus_fill",
                "expected (busIndex, numBuses, value) triples",
            );
        }
        for group in msg.args.chunks(3) {
            let [OscType::Int(base), OscType::Int(count), val] = group else {
                return self.fail(from, "/bus_fill", "expected int busIndex and numBuses");
            };
            let (Ok(base), Ok(count)) = (usize::try_from(*base), usize::try_from(*count)) else {
                return self.fail(from, "/bus_fill", "bus index and numBuses must be >= 0");
            };
            let Some(value) = float_value(val) else {
                return self.fail(from, "/bus_fill", "expected number value");
            };
            for offset in 0..count {
                self.handle.control_buses().set(base + offset, value);
            }
        }
    }
}
