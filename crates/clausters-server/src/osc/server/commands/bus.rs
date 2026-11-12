//! `/bus_*`: reading and writing control buses.
//!
//! Five commands over one shape -- a value, a range, or a fill -- all of them
//! writing the shared atomics directly rather than going through the engine's
//! command FIFO, because a control bus is exactly the state both threads may
//! touch. The audio-side `/bus_tap` family lives in [`super::super::streams`].
//!
//! They differ only in how the arguments are grouped, so what varies is the
//! reading and not the work: each one reads through [`Args`] and every refusal
//! comes back through [`OscServer::attempt`].

use super::super::*;

impl OscServer {
    /// `/bus_set busIndex value ...`: control buses are shared atomics, so this
    /// writes them directly with no engine round-trip.
    pub(in crate::osc::server) fn handle_bus_set(&mut self, mut args: Args) -> Answer {
        args.expect_groups_of(2, "(busIndex, value) pairs")?;
        while !args.is_empty() {
            let (index, value) = (args.index()?, args.float()?);
            self.handle.control_buses().set(index, value);
        }
        Ok(())
    }

    /// `/bus_get busIndex ...`: replies `/bus_get.reply` with (busIndex, value)
    /// pairs.
    pub(in crate::osc::server) fn handle_bus_get(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let mut out = Vec::with_capacity(args.len() * 2);
        while !args.is_empty() {
            let index = args.index()?;
            out.push(OscType::Int(index as i32));
            out.push(OscType::Float(self.handle.control_buses().get(index)));
        }
        self.reply(from, "/bus_get.reply", out);
        Ok(())
    }

    /// `/bus_setRange busIndex numBuses val... ...`: sets consecutive ranges,
    /// one or more groups, each carrying its own values.
    pub(in crate::osc::server) fn handle_bus_set_range(&mut self, mut args: Args) -> Answer {
        if args.is_empty() {
            return Err("expected (busIndex, numBuses, values...) groups".into());
        }
        while !args.is_empty() {
            let (base, count) = (args.index()?, args.index()?);
            if args.len() < count {
                return Err(format!(
                    "numBuses is {count} but only {} values follow",
                    args.len()
                ));
            }
            for offset in 0..count {
                let value = args.float()?;
                self.handle.control_buses().set(base + offset, value);
            }
        }
        Ok(())
    }

    /// `/bus_getRange busIndex numBuses ...`: replies `/bus_getRange.reply` with
    /// each requested range expanded to `(busIndex, numBuses, val0, val1, ...)`.
    pub(in crate::osc::server) fn handle_bus_get_range(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        args.expect_groups_of(2, "(busIndex, numBuses) pairs")?;
        let mut out = Vec::new();
        while !args.is_empty() {
            let (base, count) = (args.index()?, args.index()?);
            out.push(OscType::Int(base as i32));
            out.push(OscType::Int(count as i32));
            for offset in 0..count {
                out.push(OscType::Float(
                    self.handle.control_buses().get(base + offset),
                ));
            }
        }
        self.reply(from, "/bus_getRange.reply", out);
        Ok(())
    }

    /// `/bus_fill busIndex numBuses value ...`: fills consecutive ranges with
    /// one value each, in groups of three.
    pub(in crate::osc::server) fn handle_bus_fill(&mut self, mut args: Args) -> Answer {
        args.expect_groups_of(3, "(busIndex, numBuses, value) triples")?;
        while !args.is_empty() {
            let (base, count, value) = (args.index()?, args.index()?, args.float()?);
            for offset in 0..count {
                self.handle.control_buses().set(base + offset, value);
            }
        }
        Ok(())
    }
}
