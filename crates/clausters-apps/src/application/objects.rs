//! Execute work on an object an editor holds out of focus (an audio file
//! parked in the file list) without rebinding its window.

/// The object storage an application already owns. A temporary exchange
/// changes neither the window's binding nor its conversation.
pub(crate) trait Owner {
    type Id: Copy + Eq;
    type State;
    fn current(&self) -> Option<Self::Id>;
    fn take(&mut self, id: Self::Id) -> Option<Self::State>;
    fn replace(&mut self, state: Self::State) -> Self::State;
    fn park(&mut self, state: Self::State);
}

/// Run against one parked object and restore the previously focused state.
pub(crate) fn over<O: Owner, R>(
    owner: &mut O,
    id: O::Id,
    apply: impl FnOnce(&mut O) -> R,
) -> Option<R> {
    if owner.current() == Some(id) {
        return Some(apply(owner));
    }
    let target = owner.take(id)?;
    let focused = owner.replace(target);
    let answer = apply(owner);
    let updated = owner.replace(focused);
    owner.park(updated);
    Some(answer)
}
