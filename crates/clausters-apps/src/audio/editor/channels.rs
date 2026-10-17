//! Which channels of the take are folded: the reader's view of a stack too
//! tall to read whole, held here so a menu, a key and a script ask one state.

use super::*;

impl AudioEditor {
    /// The folded channels, in order.
    pub fn folded(&self) -> Vec<u32> {
        self.folded.iter().copied().collect()
    }

    /// What the take's widget is corrected with: its folded channels.
    fn folded_corrections(&self) -> Vec<Correction> {
        self.widget
            .into_iter()
            .map(|widget| Correction {
                widget: i64::from(widget),
                props: json!({ "folded": self.folded() }),
            })
            .collect()
    }

    /// The channel a request names, or why it names none of the take's.
    fn channel_of(&self, channel: Option<u64>) -> Result<u32, String> {
        match channel {
            Some(c) if c < u64::from(self.channels) => Ok(c as u32),
            Some(c) => Err(format!(
                "channel {c} is not one of the take's {} channels",
                self.channels
            )),
            None => Err("a channel is named by its index, from 0".into()),
        }
    }

    /// **Folds `channel` to a strip, or unfolds it** when it is folded.
    pub(super) fn fold_channel(
        &mut self,
        channel: Option<u64>,
    ) -> (Option<String>, Vec<Correction>) {
        match self.channel_of(channel) {
            Ok(c) => {
                if !self.folded.remove(&c) {
                    self.folded.insert(c);
                }
                (None, self.folded_corrections())
            }
            Err(why) => (Some(why), Vec::new()),
        }
    }

    /// **Unfolds `channel`**, a press on its strip's name.
    pub(super) fn unfold_channel(
        &mut self,
        channel: Option<u64>,
    ) -> (Option<String>, Vec<Correction>) {
        match self.channel_of(channel) {
            Ok(c) => {
                self.folded.remove(&c);
                (None, self.folded_corrections())
            }
            Err(why) => (Some(why), Vec::new()),
        }
    }

    /// **Shows `channel` alone**: every other channel folded.
    pub(super) fn solo_channel(
        &mut self,
        channel: Option<u64>,
    ) -> (Option<String>, Vec<Correction>) {
        match self.channel_of(channel) {
            Ok(c) => {
                self.folded = (0..self.channels).filter(|o| *o != c).collect();
                (None, self.folded_corrections())
            }
            Err(why) => (Some(why), Vec::new()),
        }
    }

    /// **Unfolds every channel.**
    pub(super) fn show_all_channels(&mut self) -> (Option<String>, Vec<Correction>) {
        self.folded.clear();
        (None, self.folded_corrections())
    }
}
