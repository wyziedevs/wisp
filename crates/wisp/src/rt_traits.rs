//! What the code `wisp-build` generates calls to read a handler's inputs
//! and turn what it returns into an answer. The build writes the same call
//! for every parameter and every return value; rustc picks the impl by the
//! types themselves, so an alias, or a type from another file, is read the
//! way the type it names is. Not a stable API.

use crate::json::check::{Length, max_len, min_len};
use crate::{Cx, Json, Response, Result, input};
use std::any::Any;
use std::fmt::Display;
use std::ops::{Bound, RangeBounds};
use std::str::FromStr;

/// Which impl of [`FromInput`] reads a parameter; rustc infers it.
pub enum One {}
pub enum Maybe {}
pub enum Many {}

/// A handler's parameter other than `cx`, read from the request by its
/// name: a route parameter, then the form a POST, PUT or PATCH sends (or
/// its JSON body), then the query.
#[diagnostic::on_unimplemented(
    message = "a handler cannot take a `{Self}`: its parameters are read from the request by name",
    label = "not read from a request",
    note = "a parameter other than `cx` is `FromStr` (`String`, `u64`, a `bool` checkbox...), an `Option` of one (it may be left out), a `Vec` of one (every value sent), or `body: T` for a JSON body whose `T` is `FromJson`"
)]
pub trait FromInput<M>: Sized {
    fn get(cx: &Cx, name: &str) -> Result<Self>;
}

/// `name: T`: missing, or not a `T`, is a 400 that says which. A `bool` is
/// a checkbox: `true` when sent with any value but `false`, `off` or `0`.
impl<T: FromStr<Err: Display> + 'static> FromInput<One> for T {
    fn get(cx: &Cx, name: &str) -> Result<T> {
        let mut flag: Option<T> = None;
        if let Some(b) = (&mut flag as &mut dyn Any).downcast_mut::<Option<bool>>() {
            *b = Some(input::flag(cx, name));
        }
        match flag {
            Some(v) => Ok(v),
            None => input::required(cx, name),
        }
    }
}

/// `name: Option<T>`: `None` when it is missing or blank.
impl<T: FromStr<Err: Display>> FromInput<Maybe> for Option<T> {
    fn get(cx: &Cx, name: &str) -> Result<Option<T>> {
        input::optional(cx, name)
    }
}

/// `name: Vec<T>`: every value sent under the name.
impl<T: FromStr<Err: Display>> FromInput<Many> for Vec<T> {
    fn get(cx: &Cx, name: &str) -> Result<Vec<T>> {
        input::all(cx, name)
    }
}

/// What an action or `before` hands back: a response to send instead of
/// the page, or none.
#[diagnostic::on_unimplemented(
    message = "an action or `before` returns nothing, a `Response` to send instead, or an `Option<Response>`, not `{Self}`"
)]
pub trait Answer {
    fn answer(self) -> Option<Response>;
}

impl Answer for () {
    fn answer(self) -> Option<Response> {
        None
    }
}

impl Answer for Response {
    fn answer(self) -> Option<Response> {
        Some(self)
    }
}

impl Answer for Option<Response> {
    fn answer(self) -> Option<Response> {
        self
    }
}

/// What a `+server.rs` handler returns, as the response. rustc picks the
/// first that fits, most particular first: `(&&&Ret::new(v)).respond()`.
/// A `Response` is sent as it is; nothing is a 204; `None` is a 404; any
/// other value is its JSON.
pub mod ret {
    use super::*;
    use std::cell::Cell;

    /// The value, taken once by whichever `respond` fits.
    pub struct Ret<T>(Cell<Option<T>>);

    impl<T> Ret<T> {
        pub fn new(value: T) -> Ret<T> {
            Ret(Cell::new(Some(value)))
        }

        fn take(&self) -> T {
            self.0.take().expect("a value is answered once")
        }
    }

    fn not_found() -> crate::Error {
        crate::Error::new(404, "Not Found")
    }

    /// `Response`, `Option<Response>`, `()` and `Option<()>`.
    pub trait Shape {
        fn respond(self) -> Result<Response>;
    }

    impl Shape for &&&Ret<Response> {
        fn respond(self) -> Result<Response> {
            Ok(self.take())
        }
    }

    impl Shape for &&&Ret<Option<Response>> {
        fn respond(self) -> Result<Response> {
            self.take().ok_or_else(not_found)
        }
    }

    impl Shape for &&&Ret<()> {
        fn respond(self) -> Result<Response> {
            Ok(Response::empty(204))
        }
    }

    impl Shape for &&&Ret<Option<()>> {
        fn respond(self) -> Result<Response> {
            self.take()
                .map(|()| Response::empty(204))
                .ok_or_else(not_found)
        }
    }

    /// `Option<T>`: its JSON, or a 404.
    pub trait Found {
        fn respond(self) -> Result<Response>;
    }

    impl<T: Json> Found for &&Ret<Option<T>> {
        fn respond(self) -> Result<Response> {
            self.take()
                .map(|v| Response::json_of(&v))
                .ok_or_else(not_found)
        }
    }

    /// Any other value: its JSON.
    pub trait Value {
        fn respond(self) -> Result<Response>;
    }

    impl<T: Json> Value for &Ret<T> {
        fn respond(self) -> Result<Response> {
            Ok(Response::json_of(&self.take()))
        }
    }
}

/// The bounds of `#[validate(len = …)]`.
#[diagnostic::on_unimplemented(
    message = "`len = …` needs a range with a bound, such as `len = 1..=100`"
)]
pub trait LenRange {
    fn bounds(&self) -> (Option<usize>, Option<usize>);
}

macro_rules! len_ranges {
    ($($t:ty)*) => {$(
        impl LenRange for $t {
            fn bounds(&self) -> (Option<usize>, Option<usize>) {
                let lo = match self.start_bound() {
                    Bound::Included(&n) => Some(n),
                    Bound::Excluded(&n) => Some(n + 1),
                    Bound::Unbounded => None,
                };
                let hi = match self.end_bound() {
                    Bound::Included(&n) => Some(n),
                    Bound::Excluded(&n) => Some(n.saturating_sub(1)),
                    Bound::Unbounded => None,
                };
                (lo, hi)
            }
        }
    )*};
}

len_ranges!(
    std::ops::Range<usize>
    std::ops::RangeInclusive<usize>
    std::ops::RangeFrom<usize>
    std::ops::RangeTo<usize>
    std::ops::RangeToInclusive<usize>
);

/// `#[validate(len = 1..=100)]`: characters of a string, items of a list.
pub fn len(v: &impl Length, range: impl LenRange) -> Option<String> {
    let (lo, hi) = range.bounds();
    lo.and_then(|n| min_len(v, n))
        .or_else(|| hi.and_then(|n| max_len(v, n)))
}

#[cfg(test)]
mod tests {
    use super::ret::{Found as _, Ret, Shape as _, Value as _};
    use super::*;

    #[test]
    #[allow(clippy::needless_borrow)] // the form the generated code has
    fn inputs_and_answers_by_type() {
        let get = Cx::for_test("GET /p?n=3&on=on&tag=a&tag=b HTTP/1.1\r\n\r\n", &[]);
        let n: u8 = FromInput::get(&get, "n").unwrap();
        let on: bool = FromInput::get(&get, "on").unwrap();
        let off: bool = FromInput::get(&get, "off").unwrap();
        let none: Option<u8> = FromInput::get(&get, "x").unwrap();
        let tags: Vec<String> = FromInput::get(&get, "tag").unwrap();
        assert_eq!((n, on, off, none), (3, true, false, None));
        assert_eq!(tags, ["a", "b"]);

        let status = |r: Result<Response>| r.map_or_else(|e| e.status(), |r| r.status);
        assert_eq!(status((&&&Ret::new(())).respond()), 204);
        assert_eq!(status((&&&Ret::new(None::<()>)).respond()), 404);
        assert_eq!(status((&&&Ret::new(Some(1u8))).respond()), 200);
        assert_eq!(status((&&&Ret::new(None::<Response>)).respond()), 404);
        let json = (&&&Ret::new(vec![1u8])).respond().unwrap();
        assert_eq!(json.body, b"[1]");
        assert!(().answer().is_none() && Some(Response::empty(204)).answer().is_some());

        let s = String::from("abc");
        assert_eq!(len(&s, 1..=3), None);
        assert!(len(&s, ..3).is_some() && len(&s, 4..).is_some());
    }
}
