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
//! Handlers never name their own address either: the dispatcher fails with the
//! address it matched, so a handler and its table row cannot drift into
//! disagreeing about what the command is called.
//!
//! It is the one reader for the wire: the server's handlers and the
//! translator's parses (which the offline renderer shares) both read through
//! it, so an argument of the wrong type is refused the same way everywhere --
//! an optional one included, where saying nothing and saying it wrong are
//! different answers.

use rosc::{OscMessage, OscType};

/// A cursor over one message's arguments.
///
/// Reading advances it, so a handler's sequence of reads *is* its signature;
/// [`Args::rest`] hands back whatever a fixed prefix did not consume, which is
/// how the commands with a trailing list are written.
pub(crate) struct Args<'a> {
    args: &'a [OscType],
    at: usize,
}

/// What a handler returns: `Ok` if it answered, `Err` with the reason if the
/// arguments made no sense. `OscServer::handle_message` turns the `Err` into
/// `/fail` -- the one place in the server that does.
pub(crate) type Answer = Result<(), String>;

impl<'a> Args<'a> {
    pub(crate) fn new(msg: &'a OscMessage) -> Self {
        Args {
            args: &msg.args,
            at: 0,
        }
    }

    /// A cursor past the first `n` arguments -- a command's optional tail,
    /// after a fixed prefix the caller destructured with its own usage line.
    /// It numbers the arguments from the message's first, as a client counts.
    pub(crate) fn after(args: &'a [OscType], n: usize) -> Self {
        Args { args, at: n }
    }

    /// The arguments not yet read.
    pub(crate) fn rest(&self) -> &'a [OscType] {
        &self.args[self.at.min(self.args.len())..]
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.rest().is_empty()
    }

    pub(crate) fn len(&self) -> usize {
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

    /// The next argument, whatever it is: the escape hatch for a reader the
    /// handler has to do itself because it needs context this type has no
    /// business knowing -- resolving a control name against a def, say.
    pub(crate) fn one(&mut self) -> Result<&'a OscType, String> {
        self.next("an argument")
    }

    pub(crate) fn int(&mut self) -> Result<i32, String> {
        match self.next("an integer")? {
            OscType::Int(n) => Ok(*n),
            other => Err(self.wrong("an integer", other)),
        }
    }

    /// A non-negative integer, as the index it is about to be used as. The two
    /// refusals a caller would otherwise write separately -- not an integer,
    /// and negative -- are one read.
    pub(crate) fn index(&mut self) -> Result<usize, String> {
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
    pub(crate) fn float(&mut self) -> Result<f32, String> {
        let arg = self.next("a number")?;
        float_value(arg).ok_or_else(|| self.wrong("a number", arg))
    }

    pub(crate) fn str(&mut self) -> Result<&'a str, String> {
        match self.next("a string")? {
            OscType::String(s) => Ok(s),
            other => Err(self.wrong("a string", other)),
        }
    }

    pub(crate) fn blob(&mut self) -> Result<&'a [u8], String> {
        match self.next("a blob")? {
            OscType::Blob(b) => Ok(b),
            other => Err(self.wrong("a blob", other)),
        }
    }

    /// A 64-bit integer, accepting a 32-bit one: a sample position fits in an
    /// `Int` until it does not, and a client that sends the smaller type means
    /// the number.
    pub(crate) fn long(&mut self) -> Result<i64, String> {
        match self.next("a 64-bit integer")? {
            OscType::Long(n) => Ok(*n),
            OscType::Int(n) => Ok(*n as i64),
            other => Err(self.wrong("a 64-bit integer", other)),
        }
    }

    /// A double, accepting a float, for the same reason [`Args::long`] accepts
    /// an `Int`.
    pub(crate) fn double(&mut self) -> Result<f64, String> {
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
    pub(crate) fn opt_int(&mut self) -> Result<Option<i32>, String> {
        if self.is_empty() {
            return Ok(None);
        }
        self.int().map(Some)
    }

    /// An optional trailing number: absent is `Ok(None)`, present but not a
    /// number is still an error.
    pub(crate) fn opt_float(&mut self) -> Result<Option<f32>, String> {
        if self.is_empty() {
            return Ok(None);
        }
        self.float().map(Some)
    }

    /// An optional trailing string: absent is `Ok(None)`, present but of the
    /// wrong type is still an error.
    pub(crate) fn opt_str(&mut self) -> Result<Option<&'a str>, String> {
        if self.is_empty() {
            return Ok(None);
        }
        self.str().map(Some)
    }

    /// An optional trailing 64-bit integer: absent is `Ok(None)`, present but
    /// of the wrong type is still an error.
    pub(crate) fn opt_long(&mut self) -> Result<Option<i64>, String> {
        if self.is_empty() {
            return Ok(None);
        }
        self.long().map(Some)
    }

    /// An optional trailing double: absent is `Ok(None)`, present but of the
    /// wrong type is still an error.
    pub(crate) fn opt_double(&mut self) -> Result<Option<f64>, String> {
        if self.is_empty() {
            return Ok(None);
        }
        self.double().map(Some)
    }

    /// Requires the remaining arguments to divide into groups of `n`.
    pub(crate) fn expect_groups_of(&self, n: usize, what: &str) -> Answer {
        let left = self.len();
        if left == 0 || !left.is_multiple_of(n) {
            return Err(format!("expected {what}, got {left} arguments"));
        }
        Ok(())
    }
}

/// A def's JSON as the first argument carries it: a blob, or a string a
/// hand-written client finds easier to send.
pub(crate) fn json_payload(args: &[OscType]) -> Result<&[u8], String> {
    match args.first() {
        Some(OscType::Blob(b)) => Ok(b),
        Some(OscType::String(s)) => Ok(s.as_bytes()),
        _ => Err("expected a JSON blob or string".into()),
    }
}

/// A number: `f32`, or an `Int` or a `Double` narrowed.
pub fn float_value(arg: &OscType) -> Option<f32> {
    match arg {
        OscType::Float(f) => Some(*f),
        OscType::Int(i) => Some(*i as f32),
        OscType::Double(d) => Some(*d as f32),
        _ => None,
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
