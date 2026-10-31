//! Calls on retained sessions use the same temporary object access as audio.
use super::*;
use crate::application::objects::{self, Owner};
use crate::multitrack::editor::{MultitrackEditor, SessionState};

struct At<'a> {
    editing: &'a mut Editing,
    member: MemberId,
}
impl At<'_> {
    fn editor(&self) -> &MultitrackEditor {
        let Member::Multitrack(editor) = self.editing.member(self.member).unwrap() else {
            unreachable!()
        };
        editor
    }
    fn editor_mut(&mut self) -> &mut MultitrackEditor {
        let Member::Multitrack(editor) = self.editing.member_mut(self.member).unwrap() else {
            unreachable!()
        };
        editor
    }
}
impl Owner for At<'_> {
    type Id = i64;
    type State = (SessionState, bool);
    fn current(&self) -> Option<i64> {
        Owner::current(self.editor())
    }
    fn take(&mut self, id: i64) -> Option<Self::State> {
        Owner::take(self.editor_mut(), id)
    }
    fn replace(&mut self, state: Self::State) -> Self::State {
        Owner::replace(self.editor_mut(), state)
    }
    fn park(&mut self, state: Self::State) {
        Owner::park(self.editor_mut(), state);
    }
}

impl Editing {
    pub(super) fn over_multitrack<R>(
        &mut self,
        member: MemberId,
        object: i64,
        run: impl FnOnce(&mut Editing) -> R,
    ) -> Result<R, String> {
        if !matches!(self.member(member), Some(Member::Multitrack(_))) {
            return Err("the member is not a multitrack".into());
        }
        let scope = self.services.histories.selected();
        let structure = self.seats[member as usize].structure;
        let answer = objects::over(
            &mut At {
                editing: self,
                member,
            },
            object,
            |at| run(at.editing),
        )
        .ok_or_else(|| "the session object is no longer retained".to_string());
        self.services.histories.select(scope);
        self.seats[member as usize].structure = structure;
        answer
    }

    /// Read a retained session without changing this window's active binding.
    pub fn query_multitrack_object(&mut self, member: MemberId, object: i64, query: &str) -> Value {
        self.over_multitrack(member, object, |editing| {
            editing.multitrack_query(member, query)
        })
        .unwrap_or_else(|error| json!({"error":error}))
    }
}
