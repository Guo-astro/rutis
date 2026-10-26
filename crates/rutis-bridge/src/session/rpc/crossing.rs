//! What a call cannot send (#675): a value the far end cannot take, or a
//! message over the channel's limit, fails only that call, on the side that
//! would send it, and the error says which call, which argument (or the
//! result) and why. The session goes on.

use super::*;

/// What a call is, for messages: `service.method` for an invocation (its
/// target without a `host:` prefix or a scope label), the method alone for
/// a control call (no target).
pub(super) fn invocation(target: &str, method: &str) -> String {
    let service = target.strip_prefix("host:").unwrap_or(target);
    match service.split('\0').next().unwrap_or_default() {
        "" => method.to_owned(),
        service => format!("{service}.{method}"),
    }
}

impl Operation {
    /// The call, for messages.
    pub(super) fn describe(&self) -> String {
        match self {
            Operation::Invoke(target, method, _) => invocation(target, method),
            Operation::Call(_, None, _) => "a function".into(),
            Operation::Call(_, Some(method), _) => format!("the method {method}"),
            Operation::Get(_, property) => format!("the property {property}"),
            Operation::Await(..) => "an awaited result".into(),
        }
    }
}

/// `error` as one of argument `index` (from 0) of `call`: a value error
/// says which; any other error is as it was.
pub(super) fn of_argument(error: Error, index: usize, call: &dyn Fn() -> String) -> Error {
    match error {
        Error::Value(reason) => {
            Error::Value(format!("argument {} of {}: {reason}", index + 1, call()))
        }
        error => error,
    }
}

/// `error` as one of the result of `call` (when known).
pub(super) fn of_result(error: Error, call: Option<&str>) -> Error {
    match error {
        Error::Value(reason) => Error::Value(match call {
            Some(call) => format!("the result of {call}: {reason}"),
            None => format!("the result: {reason}"),
        }),
        error => error,
    }
}

/// Why a message of `size` bytes is not sent over a channel taking at most
/// `limit`.
pub(super) fn over_limit(size: usize, limit: usize) -> String {
    const MIB: usize = 1 << 20;
    let mib = match limit % MIB {
        0 => format!(" ({} MiB)", limit / MIB),
        _ => String::new(),
    };
    format!("the message would be {size} bytes, over the limit of {limit} bytes{mib}")
}

/// How many bytes a value takes as JSON, without keeping them.
pub(super) fn json_size(value: &impl serde::Serialize) -> usize {
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    let _ = serde_json::to_writer(&mut count, value);
    count.0
}

/// The largest of a call's encoded arguments, when they are a list: the
/// one a message over the limit is blamed on.
fn largest(args: &WireValue) -> Option<usize> {
    let sizes: Vec<usize> = match args {
        WireValue::List(items) => items.iter().map(json_size).collect(),
        WireValue::Data(Json::Array(items)) => items.iter().map(json_size).collect(),
        _ => return None,
    };
    (0..sizes.len()).max_by_key(|&index| sizes[index])
}

impl Connection {
    /// The far end, for messages: the endpoint it greeted as, the one the
    /// channel verified or whoever started it named, else `the far end`.
    pub(super) fn far_name(&self) -> String {
        if let Some(greeting) = self.greeting() {
            return greeting.endpoint.to_string();
        }
        if let Some(peer) = &self.0.verified {
            return peer.to_string();
        }
        if let Some(name) = self.0.named.get() {
            return name.clone();
        }
        "the far end".into()
    }

    /// Name the far end of a compat-format session, which does not name
    /// itself: whoever started it knows (a runtime's link).
    pub(crate) fn name_far_end(&self, name: &str) {
        let _ = self.0.named.set(name.to_owned());
    }

    /// An object with methods goes only to a far end that declared
    /// `objects` (a Rust one): any other cannot call its methods, and
    /// would end the session on it (#675).
    pub(super) fn object_refused(&self) -> Error {
        Error::Value(format!(
            "an object with methods cannot cross to {}; pass plain data or a function instead",
            self.far_name()
        ))
    }

    /// Encode a call's arguments (`args`, a list of them as a rule), each
    /// nested at most [`MAX_DEPTH`] deep: a value error says which argument
    /// of `call`. With the references they grant, given back on failure.
    pub(super) fn encode_args(
        &self,
        args: &Value,
        call: &dyn Fn() -> String,
    ) -> Result<(WireValue, Vec<u64>), Error> {
        let mut grants = Vec::new();
        let result = match args {
            Value::List(items) => items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    self.encode_inner(item, MAX_DEPTH, &mut grants)
                        .map_err(|error| of_argument(error, index, call))
                })
                .collect::<Result<Vec<_>, _>>()
                .map(WireValue::List),
            Value::Data(Json::Array(items)) => self
                .encode_inner(args, MAX_DEPTH + 1, &mut grants)
                .map_err(|error| {
                    // Data holds no references: each argument alone says
                    // which one it is.
                    let index = items.iter().position(|item| {
                        self.encode(&Value::Data(item.clone()), MAX_DEPTH).is_err()
                    });
                    match index {
                        Some(index) => of_argument(error, index, call),
                        None => error,
                    }
                }),
            args => {
                self.encode_inner(args, MAX_DEPTH + 1, &mut grants)
                    .map_err(|error| match error {
                        Error::Value(reason) => {
                            Error::Value(format!("the arguments of {}: {reason}", call()))
                        }
                        error => error,
                    })
            }
        };
        match result {
            Ok(wire) => Ok((wire, grants)),
            Err(error) => {
                self.ungrant(grants);
                Err(error)
            }
        }
    }

    /// `frame` as sent, or, over the channel's `limit`, why not: a value
    /// error naming the largest argument of `call` (a call's frame) or the
    /// result.
    pub(super) fn serialized(
        &self,
        frame: &Frame,
        limit: Option<usize>,
        call: &dyn Fn() -> String,
    ) -> Result<Vec<u8>, Error> {
        let bytes = serde_json::to_vec(frame).map_err(transport)?;
        let Some(limit) = limit.filter(|limit| bytes.len() > *limit) else {
            return Ok(bytes);
        };
        let reason = over_limit(bytes.len(), limit);
        let args = match frame {
            Frame::Invoke { args, .. } | Frame::Call { args, .. } => args,
            _ => return Err(Error::Value(format!("{}: {reason}", call()))),
        };
        Err(match largest(args) {
            Some(index) => of_argument(Error::Value(reason), index, call),
            None => Error::Value(format!("the arguments of {}: {reason}", call())),
        })
    }
}
