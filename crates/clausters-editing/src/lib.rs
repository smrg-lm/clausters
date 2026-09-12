//! **What an editable structure owes its three endpoints**, written once.
//!
//! A structure in this system has three endpoints and an edit may start at any
//! of them: the document owns the model, a host draws it, a server sounds it.
//! Each pair of those needs a *projection* — the structure as props, a gesture
//! as payloads, the structure as what is sounding — and a projection with two
//! implementations is how one curve comes to be drawn two ways and one piece
//! comes to sound two. So there is one of each, here, and every client binds
//! it while the host links it.
//!
//! # Why this is not `clausters-document`
//!
//! That crate's own rule: **the wire stays out of it.** It defines intents and
//! outcomes and does not encode them, and a projection's answer is exactly an
//! encoding — the props of a `/gui_*` message, the arguments of an OSC one. So
//! the projections sit *above* the document rather than in it, in a crate that
//! may know both the model and the shape it is being written into.
//!
//! # Why this is not `clausters-core` either
//!
//! The numeric and drawing *rules* are there and stay there — this crate asks
//! [`clausters_core::envshape::curve_axis`] rather than restating it. What is
//! here is the step after: assembling a rule's answer into the payload an
//! endpoint reads. The core cannot hold that for the projections that come
//! next, because those are over the document's types and the core does not
//! depend on the document.
//!
//! # What a projection is, and what it is not
//!
//! A **function**. It takes the structure and whatever the caller is holding,
//! and hands back the payload. It keeps nothing: where a picture's own state
//! lives — the axis a view has settled on, the zoom, the selection — is the
//! endpoint's question, and the answer is that the *host* owns view state. A
//! projection that kept it would be a fourth place for it to live.

pub mod multitrack;
pub mod points;
