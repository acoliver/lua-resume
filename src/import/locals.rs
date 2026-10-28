//! Giving each local of the normalized function its value from the saved registers.
//!
//! The 5.3 frame holds the source's named locals and the hidden state of `for` loops. The
//! normalized function declares the same source locals, plus generated ones: the site number,
//! expression temporaries and the loop state under generated names.

use std::collections::{HashMap, VecDeque};

use super::ImportError;
use crate::capture::{Local, Value};
use crate::lua53::frame::Binding;

/// The three hidden registers of a loop, in register order.
type LoopState = [Value; 3];

/// A generated local in the normalized function.
enum Generated<'a> {
    Site,
    Temporary,
    /// The function, state or control of generic loop `N` (0, 1 or 2), or its closing value.
    Generic(&'a str, usize),
    /// The limit, step or control of numeric loop `N` (0, 1 or 2).
    Numeric(&'a str, usize),
    Closing,
}

fn digits(rest: &str) -> Option<&str> {
    (!rest.is_empty() && rest.bytes().all(|byte| byte.is_ascii_digit())).then_some(rest)
}

fn classify(name: &str) -> Option<Generated<'_>> {
    if name == crate::rewrite::SITE {
        return Some(Generated::Site);
    }
    let rest = name.strip_prefix("__lr")?;
    if digits(rest).is_some() {
        return Some(Generated::Temporary);
    }
    let suffixed = |prefix: &str| rest.strip_prefix(prefix).and_then(digits);
    for (index, prefix) in ["_f", "_s", "_c"].iter().enumerate() {
        if let Some(id) = suffixed(prefix) {
            return Some(Generated::Generic(id, index));
        }
    }
    for (index, prefix) in ["_nl", "_ns", "_nc"].iter().enumerate() {
        if let Some(id) = suffixed(prefix) {
            return Some(Generated::Numeric(id, index));
        }
    }
    suffixed("_x").map(|_| Generated::Closing)
}

/// The hidden loop registers of a frame, grouped by loop and in loop order.
fn loop_states(
    frame: usize,
    bindings: &[Binding<Value>],
) -> Result<(VecDeque<LoopState>, VecDeque<LoopState>), ImportError> {
    let mut generic = VecDeque::new();
    let mut numeric = VecDeque::new();
    let mut hidden = bindings.iter().filter(|binding| binding.is_loop_state());
    while let Some(first) = hidden.next() {
        let group: Vec<&Binding<Value>> = std::iter::once(first)
            .chain(hidden.by_ref().take(2))
            .collect();
        let names: Vec<&[u8]> = group
            .iter()
            .map(|binding| binding.name.as_slice())
            .collect();
        let values: LoopState = std::array::from_fn(|at| {
            group
                .get(at)
                .map_or(Value::Nil, |binding| binding.value.clone())
        });
        match names.as_slice() {
            [b"(for generator)", b"(for state)", b"(for control)"] => generic.push_back(values),
            [b"(for index)", b"(for limit)", b"(for step)"] => numeric.push_back(values),
            _ => {
                return Err(ImportError::frame(
                    frame,
                    format!(
                        "unsupported hidden registers {:?}",
                        names
                            .iter()
                            .map(|name| String::from_utf8_lossy(name).into_owned())
                            .collect::<Vec<_>>()
                    ),
                ));
            }
        }
    }
    Ok((generic, numeric))
}

struct Binder<'a> {
    frame: usize,
    site: u32,
    source: std::vec::IntoIter<&'a Binding<Value>>,
    generic: VecDeque<LoopState>,
    numeric: VecDeque<LoopState>,
    open_generic: HashMap<String, LoopState>,
    open_numeric: HashMap<String, LoopState>,
}

impl Binder<'_> {
    fn value(&mut self, name: &str) -> Result<Value, ImportError> {
        match classify(name) {
            Some(Generated::Site) => Ok(Value::Integer(i64::from(self.site))),
            Some(Generated::Temporary | Generated::Closing) => Ok(Value::Nil),
            Some(Generated::Generic(id, at)) => {
                let state = self.loop_state(id, at, true)?;
                Ok(state[at].clone())
            }
            Some(Generated::Numeric(id, at)) => {
                // The normalized loop keeps the limit, the step and the control value (the
                // loop index at the start of the iteration); 5.3 keeps index, limit, step.
                let state = self.loop_state(id, at, false)?;
                Ok(state[[1, 2, 0][at]].clone())
            }
            None if name.starts_with("__lr") => Err(ImportError::frame(
                self.frame,
                format!(
                    "the normalized function has the generated local `{name}`, which has no saved counterpart"
                ),
            )),
            None => self.source_local(name),
        }
    }

    fn loop_state(&mut self, id: &str, at: usize, generic: bool) -> Result<LoopState, ImportError> {
        let (queue, open) = if generic {
            (&mut self.generic, &mut self.open_generic)
        } else {
            (&mut self.numeric, &mut self.open_numeric)
        };
        if let Some(state) = open.get(id) {
            return Ok(state.clone());
        }
        // The first generated name of a loop claims the next saved loop in order.
        let (0, Some(state)) = (at, queue.pop_front()) else {
            return Err(ImportError::frame(
                self.frame,
                "a loop of the source has no saved loop registers",
            ));
        };
        open.insert(id.to_owned(), state.clone());
        Ok(state)
    }

    fn source_local(&mut self, name: &str) -> Result<Value, ImportError> {
        match self.source.next() {
            Some(found) if found.name == name.as_bytes() => Ok(found.value.clone()),
            Some(found) => Err(ImportError::frame(
                self.frame,
                format!(
                    "the source declares `{name}` where the save has `{}`",
                    String::from_utf8_lossy(&found.name)
                ),
            )),
            None => Err(ImportError::frame(
                self.frame,
                format!("the source declares `{name}`, which the save has no register for"),
            )),
        }
    }
}

/// The locals of the continuation, in order: `scope` are the names it declares.
pub(super) fn bind(
    frame: usize,
    scope: &[String],
    bindings: &[Binding<Value>],
    site: u32,
) -> Result<Vec<Local>, ImportError> {
    let (generic, numeric) = loop_states(frame, bindings)?;
    let mut binder = Binder {
        frame,
        site,
        source: bindings
            .iter()
            .filter(|binding| !binding.is_loop_state())
            .collect::<Vec<_>>()
            .into_iter(),
        generic,
        numeric,
        open_generic: HashMap::new(),
        open_numeric: HashMap::new(),
    };
    let locals = scope
        .iter()
        .map(|name| {
            Ok(Local {
                name: name.clone(),
                value: binder.value(name)?,
            })
        })
        .collect::<Result<Vec<_>, ImportError>>()?;
    if let Some(left) = binder.source.next() {
        return Err(ImportError::frame(
            frame,
            format!(
                "the save has the live local `{}` that the source does not declare here",
                String::from_utf8_lossy(&left.name)
            ),
        ));
    }
    Ok(locals)
}
