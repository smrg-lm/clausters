//! A fixed-capacity sample FIFO for the block-based UGens.
//!
//! A UGen that works a hop at a time -- an inverse FFT's overlap-add, a
//! partitioned convolution -- finishes samples in bursts and hands them out
//! one per frame. The buffer is allocated when the UGen is built and never
//! again, so pushing and popping are safe on the audio thread.

/// A ring of `f32` samples with a fixed capacity.
pub struct SampleFifo {
    ring: Vec<f32>,
    head: usize,
    tail: usize,
    len: usize,
}

impl SampleFifo {
    /// A FIFO holding at most `capacity` samples, empty.
    pub fn new(capacity: usize) -> Self {
        SampleFifo {
            ring: vec![0.0; capacity.max(1)],
            head: 0,
            tail: 0,
            len: 0,
        }
    }

    /// Queues `v`; a full FIFO drops it rather than growing.
    #[inline]
    pub fn push(&mut self, v: f32) {
        if self.len < self.ring.len() {
            self.ring[self.tail] = v;
            self.tail = (self.tail + 1) % self.ring.len();
            self.len += 1;
        }
    }

    /// The oldest sample, or silence when the FIFO is empty.
    #[inline]
    pub fn pop(&mut self) -> f32 {
        if self.len == 0 {
            return 0.0;
        }
        let v = self.ring[self.head];
        self.head = (self.head + 1) % self.ring.len();
        self.len -= 1;
        v
    }
}

#[cfg(test)]
mod tests {
    use super::SampleFifo;

    #[test]
    fn it_is_first_in_first_out_and_silent_when_empty() {
        let mut fifo = SampleFifo::new(2);
        fifo.push(1.0);
        fifo.push(2.0);
        fifo.push(3.0);
        assert_eq!([fifo.pop(), fifo.pop(), fifo.pop()], [1.0, 2.0, 0.0]);
    }
}
