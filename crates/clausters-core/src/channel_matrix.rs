//! **How channels of one width become channels of another**: a fixed gain
//! for each input channel in each output channel, and the rules that build
//! one.
//!
//! The same question is asked in several places and has one answer here. A
//! part of a take reads a file of another width; a paste lands a block of
//! another width in a take; a track of one width plays into a master of
//! another. Each asks for a matrix by a rule and uses it in its own way: a
//! part keeps the matrix's **routing** form (each output reads at most one
//! input, at unity), which is what lets its picture be composed from its
//! file's peaks one channel at a time; a paste whose matrix is not routing
//! mixes, and is rendered; a player applies any matrix as it goes.
//!
//! A matrix converts a width and nothing else. A pan or a balance is a
//! control that moves, and it stays the strip's (`crate::mixer`): a mono
//! track in a stereo mix is panned under a law, while a mono block pasted
//! into a stereo take is the same signal on both channels at full level.
//! Both are conventions of the field, and they are two acts.

/// **A fixed gain for each input channel in each output channel.** Unity on
/// a diagonal is a copy; a row of several gains is a mix; a row of zeros is
/// silence.
#[derive(Clone, Debug, PartialEq)]
pub struct ChannelMatrix {
    inputs: usize,
    outputs: usize,
    /// One row per output, `inputs` gains each.
    gains: Vec<f32>,
}

impl ChannelMatrix {
    /// Every output silent.
    pub fn silent(inputs: usize, outputs: usize) -> Self {
        Self {
            inputs,
            outputs,
            gains: vec![0.0; inputs * outputs],
        }
    }

    /// **A routing**: output `c` reads input `map[c]`, an entry that is
    /// negative or past the inputs being silence, and an output past the
    /// map too.
    pub fn routing(map: &[i32], inputs: usize, outputs: usize) -> Self {
        let mut m = Self::silent(inputs, outputs);
        for (output, &input) in map.iter().enumerate().take(outputs) {
            if let Ok(input) = usize::try_from(input)
                && input < inputs
            {
                m.set(output, input, 1.0);
            }
        }
        m
    }

    /// **Duplicate**: input 0 on every output, at unity. Mono into any
    /// width.
    pub fn duplicate(inputs: usize, outputs: usize) -> Self {
        Self::routing(&vec![0; outputs], inputs, outputs)
    }

    /// **Mix**: every input into every output at `1 / inputs`, the average,
    /// so a mix of full-scale channels never clips. Any width into mono.
    pub fn mix(inputs: usize, outputs: usize) -> Self {
        let gain = 1.0 / inputs.max(1) as f32;
        Self {
            inputs,
            outputs,
            gains: vec![gain; inputs * outputs],
        }
    }

    /// **Channels as tracks**: input `i` into output `from + i`, the inputs
    /// past the outputs dropped and the outputs outside them silent. Nothing
    /// is duplicated or mixed.
    pub fn as_tracks(inputs: usize, outputs: usize, from: usize) -> Self {
        let map: Vec<i32> = (0..outputs)
            .map(|c| {
                c.checked_sub(from)
                    .filter(|&i| i < inputs)
                    .map_or(-1, |i| i as i32)
            })
            .collect();
        Self::routing(&map, inputs, outputs)
    }

    /// **Folded**: input `i` into output `i % outputs`, at unity -- the
    /// channels past the outputs routed onto the first ones again, so none is
    /// dropped. A track wider than its multitrack plays into the master this
    /// way, its channels past the master's on the master's first channels.
    pub fn folded(inputs: usize, outputs: usize) -> Self {
        let mut m = Self::silent(inputs, outputs);
        if outputs > 0 {
            for input in 0..inputs {
                m.set(input % outputs, input, 1.0);
            }
        }
        m
    }

    /// **The implicit conversion**, what a width becomes when nothing says
    /// otherwise: mono is duplicated, anything wider is read channel by
    /// channel, silence past it.
    pub fn implicit(inputs: usize, outputs: usize) -> Self {
        if inputs == 1 {
            Self::duplicate(inputs, outputs)
        } else {
            Self::as_tracks(inputs, outputs, 0)
        }
    }

    /// **What a paste of `inputs` channels into `outputs` does**, landing
    /// from the channel `from` when one is chosen. Mono is duplicated unless
    /// a channel is chosen; into mono, a wider block is mixed; otherwise the
    /// channels are tracks, from the chosen channel or the first.
    pub fn fit(inputs: usize, outputs: usize, from: Option<usize>) -> Self {
        match from {
            _ if inputs == outputs && from.is_none_or(|f| f == 0) => {
                Self::as_tracks(inputs, outputs, 0)
            }
            None if inputs == 1 => Self::duplicate(inputs, outputs),
            _ if outputs == 1 => Self::mix(inputs, outputs),
            from => Self::as_tracks(inputs, outputs, from.unwrap_or(0)),
        }
    }

    pub fn inputs(&self) -> usize {
        self.inputs
    }

    pub fn outputs(&self) -> usize {
        self.outputs
    }

    /// The gain of `input` in `output`.
    pub fn gain(&self, output: usize, input: usize) -> f32 {
        self.gains[output * self.inputs + input]
    }

    /// Sets the gain of `input` in `output`.
    pub fn set(&mut self, output: usize, input: usize, gain: f32) {
        self.gains[output * self.inputs + input] = gain;
    }

    /// **The routing form**: output -> input, `-1` for silence, or `None`
    /// when an output reads more than one input or one at a gain other than
    /// unity, which is a mix.
    pub fn as_routing(&self) -> Option<Vec<i32>> {
        (0..self.outputs)
            .map(|output| {
                let row = &self.gains[output * self.inputs..][..self.inputs];
                let mut read = -1;
                for (input, &g) in row.iter().enumerate() {
                    if g == 0.0 {
                        continue;
                    }
                    if g != 1.0 || read >= 0 {
                        return None;
                    }
                    read = input as i32;
                }
                Some(read)
            })
            .collect()
    }

    /// **One frame through the matrix**: `input` holds `inputs` samples and
    /// `output` receives `outputs`.
    pub fn apply(&self, input: &[f32], output: &mut [f32]) {
        for (o, out) in output.iter_mut().enumerate().take(self.outputs) {
            let row = &self.gains[o * self.inputs..][..self.inputs];
            *out = row.iter().zip(input).map(|(g, x)| g * x).sum();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wider width folds onto the first outputs, and a narrower one is a
    /// copy of its channels.
    #[test]
    fn folding_routes_the_channels_past_the_outputs_onto_the_first() {
        let m = ChannelMatrix::folded(6, 2);
        let mut out = [0.0; 2];
        m.apply(&[1.0, 2.0, 4.0, 8.0, 16.0, 32.0], &mut out);
        assert_eq!(out, [1.0 + 4.0 + 16.0, 2.0 + 8.0 + 32.0]);
        assert_eq!(
            ChannelMatrix::folded(2, 4).as_routing(),
            Some(vec![0, 1, -1, -1])
        );
    }

    /// Mono is duplicated, a wider width is read channel by channel.
    #[test]
    fn the_implicit_conversion_duplicates_mono_only() {
        assert_eq!(ChannelMatrix::implicit(1, 2).as_routing(), Some(vec![0, 0]));
        assert_eq!(ChannelMatrix::implicit(1, 4).as_routing(), Some(vec![0; 4]));
        assert_eq!(
            ChannelMatrix::implicit(2, 4).as_routing(),
            Some(vec![0, 1, -1, -1])
        );
        assert_eq!(ChannelMatrix::implicit(4, 2).as_routing(), Some(vec![0, 1]));
    }

    /// The three rules of a paste, by the widths and the chosen channel.
    #[test]
    fn a_paste_duplicates_mixes_or_lays_channels_as_tracks() {
        assert_eq!(
            ChannelMatrix::fit(1, 2, None).as_routing(),
            Some(vec![0, 0])
        );
        assert_eq!(
            ChannelMatrix::fit(1, 4, Some(2)).as_routing(),
            Some(vec![-1, -1, 0, -1]),
            "a chosen channel takes a mono block alone"
        );
        let mixed = ChannelMatrix::fit(2, 1, None);
        assert_eq!(mixed.as_routing(), None, "a mix is not a routing");
        let mut out = [0.0];
        mixed.apply(&[1.0, 1.0], &mut out);
        assert_eq!(out, [1.0], "the average of full scale is full scale");
        assert_eq!(
            ChannelMatrix::fit(2, 6, Some(3)).as_routing(),
            Some(vec![-1, -1, -1, 0, 1, -1])
        );
        assert_eq!(
            ChannelMatrix::fit(4, 2, Some(1)).as_routing(),
            Some(vec![-1, 0]),
            "only the channels that fit"
        );
        assert_eq!(
            ChannelMatrix::fit(2, 2, None).as_routing(),
            Some(vec![0, 1])
        );
    }

    /// A routing round-trips, with a map past its inputs silent; any other
    /// gain is not a routing.
    #[test]
    fn a_routing_is_unity_gains_one_per_output() {
        let m = ChannelMatrix::routing(&[1, 5, -1], 2, 3);
        assert_eq!(m.as_routing(), Some(vec![1, -1, -1]));
        let mut half = m.clone();
        half.set(0, 1, 0.5);
        assert_eq!(half.as_routing(), None);
        let mut out = [9.0; 3];
        m.apply(&[0.25, 0.5], &mut out);
        assert_eq!(out, [0.5, 0.0, 0.0]);
    }
}
