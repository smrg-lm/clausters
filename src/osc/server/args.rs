//! Reading a command's arguments, and saying so when they are wrong.
//!
//! Every handler used to destructure `msg.args` by hand and write its own
//! refusal at each step, which is how 117 `fail` sites came to phrase the same
//! four complaints in a dozen ways: a client asking for the wrong thing twice
//! got two differently-worded answers. [`Args`] is a cursor over the argument
//! list whose readers return [`Err`] instead, so a handler states what it wants
//! and the wording comes from one place.
//!
//! The refusals are prose, not protocol. `/fail` always carries the command's
//! address and one string, and that shape does not change here; what a client
//! must never do is match on the string, which is why the reference documents
//! the arguments and not the sentences.
//!
//! Handlers wired through [`OscServer::attempt`] never name their own address
//! either -- the failure is addressed with the one the client actually sent, so
//! a handler and its dispatch arm cannot drift into disagreeing about what the
//! command is called.

use rosc::{OscMessage, OscType};

use super::float_value;

/// A cursor over one message's arguments.
///
/// Reading advances it, so a handler's sequence of reads *is* its signature;
/// [`Args::rest`] hands back whatever a fixed prefix did not consume, which is
/// how the commands with a trailing list are written.
pub(in crate::osc::server) struct Args<'a> {
    args: &'a [OscType],
    at: usize,
}

/// What a handler returns: `Ok` if it answered, `Err` with the reason if the
/// arguments made no sense. [`OscServer::attempt`] turns the `Err` into `/fail`.
pub(in crate::osc::server) type Answer = Result<(), String>;

impl<'a> Args<'a> {
    pub(in crate::osc::server) fn new(msg: &'a OscMessage) -> Self {
        Args {
            args: &msg.args,
            at: 0,
        }
    }

    /// The arguments not yet read.
    pub(in crate::osc::server) fn rest(&self) -> &'a [OscType] {
        &self.args[self.at.min(self.args.len())..]
    }

    pub(in crate::osc::server) fn is_empty(&self) -> bool {
        self.rest().is_empty()
    }

    pub(in crate::osc::server) fn len(&self) -> usize {
        self.rest().len()
    }

    fn next(&mut self, want: &str) -> Result<&'a OscType, String> {
        let arg = self
            .args
            .get(self.at)
            .ok_or_else(|| format!("expected {want} as argument {}, message ended", self.at + 1))?;
        self.at += 1;
        Ok(arg)
    }

    fn wrong(&self, want: &str, got: &OscType) -> String {
        format!(
            "expected {want} as argument {}, got {}",
            self.at,
            type_name(got)
        )
    }

    pub(in crate::osc::server) fn int(&mut self) -> Result<i32, String> {
        match self.next("an integer")? {
            OscType::Int(n) => Ok(*n),
            other => Err(self.wrong("an integer", other)),
        }
    }

    /// A non-negative integer, as the index it is about to be used as. The two
    /// refusals a caller would otherwise write separately -- not an integer,
    /// and negative -- are one read.
    pub(in crate::osc::server) fn index(&mut self) -> Result<usize, String> {
        let n = self.int()?;
        usize::try_from(n).map_err(|_| {
            format!(
                "argument {} must be zero or greater, got {n}",
                self.at.max(1)
            )
        })
    }

    /// A number: `f32`, or an `Int` widened, since a client that sends `1`
    /// where a float belongs means the number and not a type error.
    pub(in crate::osc::server) fn float(&mut self) -> Result<f32, String> {
        let arg = self.next("a number")?;
        float_value(arg).ok_or_else(|| self.wrong("a number", arg))
    }

    pub(in crate::osc::server) fn str(&mut self) -> Result<&'a str, String> {
        match self.next("a string")? {
            OscType::String(s) => Ok(s),
            other => Err(self.wrong("a string", other)),
        }
    }

    /// A 64-bit integer, accepting a 32-bit one: a sample position fits in an
    /// `Int` until it does not, and a client that sends the smaller type means
    /// the number.
    pub(in crate::osc::server) fn long(&mut self) -> Result<i64, String> {
        match self.next("a 64-bit integer")? {
            OscType::Long(n) => Ok(*n),
            OscType::Int(n) => Ok(*n as i64),
            other => Err(self.wrong("a 64-bit integer", other)),
        }
    }

    /// A double, accepting a float, for the same reason [`Args::long`] accepts
    /// an `Int`.
    pub(in crate::osc::server) fn double(&mut self) -> Result<f64, String> {
        match self.next("a double")? {
            OscType::Double(v) => Ok(*v),
            OscType::Float(v) => Ok(*v as f64),
            other => Err(self.wrong("a double", other)),
        }
    }

    /// An optional trailing integer: absent is `Ok(None)`, present but of the
    /// wrong type is still an error -- the shape of a command whose arguments
    /// all have defaults, where saying nothing and saying it wrong are
    /// different answers.
    pub(in crate::osc::server) fn opt_int(&mut self) -> Result<Option<i32>, String> {
        if self.is_empty() {
            return Ok(None);
        }
        self.int().map(Some)
    }

    /// An optional trailing double: absent is `Ok(None)`, present but of the
    /// wrong type is still an error.
    pub(in crate::osc::server) fn opt_double(&mut self) -> Result<Option<f64>, String> {
        if self.is_empty() {
            return Ok(None);
        }
        self.double().map(Some)
    }

    /// Requires the remaining arguments to divide into groups of `n`.
    pub(in crate::osc::server) fn expect_groups_of(&self, n: usize, what: &str) -> Answer {
        let left = self.len();
        if left == 0 || !left.is_multiple_of(n) {
            return Err(format!("expected {what}, got {left} arguments"));
        }
        Ok(())
    }
}

fn type_name(arg: &OscType) -> &'static str {
    match arg {
        OscType::Int(_) => "an integer",
        OscType::Long(_) => "a 64-bit integer",
        OscType::Float(_) => "a float",
        OscType::Double(_) => "a double",
        OscType::String(_) => "a string",
        OscType::Blob(_) => "a blob",
        OscType::Bool(_) => "a boolean",
        OscType::Time(_) => "a timetag",
        OscType::Nil => "nil",
        _ => "an unsupported type",
    }
}
