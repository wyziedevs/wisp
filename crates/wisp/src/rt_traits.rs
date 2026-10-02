//! What the code `wisp-build` generates calls to read a handler's inputs
//! and turn what it returns into an answer. The build writes the same call
//! for every parameter and every return value; rustc picks the impl by the
//! types themselves, so an alias, or a type from another file, is read the
//! way the type it names is. Not a stable API.

use crate::json::check::{Length, max_len, min_len};
use crate::{Cx, FromJson, Image, Json, Response, Result, input};
use std::any::Any;
use std::fmt::Display;
use std::ops::{Bound, RangeBounds};
use std::str::FromStr;

/// Which impl of [`FromInput`] reads a parameter; rustc infers it.
pub enum One {}
pub enum Maybe {}
pub enum Many {}
pub enum Whole {}

/// A struct with named fields that `#[derive(FromJson)]` (or `Rest`) reads:
/// an action may take it whole (`fn save(post: Post)`), from a form or a
/// JSON body. Only the derive implements it.
pub trait Fields {}
pub enum Upload {}

/// A handler's parameter other than `cx`, read from the request by its
/// name: a route parameter, then the form a POST, PUT or PATCH sends (or
/// its JSON body), then the query.
#[diagnostic::on_unimplemented(
    message = "a handler cannot take a `{Self}`: its parameters are read from the request by name",
    label = "not read from a request",
    note = "a parameter other than `cx` is `FromStr` (`String`, `u64`, a `bool` checkbox...), an `Option` of one (it may be left out), a `Vec` of one (every value sent), an `Image` upload, a struct with `#[derive(FromJson)]` (its fields by name), or `body: T` for a JSON body whose `T` is `FromJson`"
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

/// `post: Post`: the struct's fields by name, from the form or a JSON body.
impl<T: FromJson + Fields> FromInput<Whole> for T {
    fn get(cx: &Cx, _: &str) -> Result<T> {
        input::whole(cx)
    }
}

/// `name: Image`: the file sent in the form's field `name`, if it is an
/// image; none chosen, or another kind of file, is a 422 by the field.
impl FromInput<Upload> for Image {
    fn get(cx: &Cx, name: &str) -> Result<Image> {
        input::image(cx, name)?.ok_or_else(|| crate::Error::invalid(name, "choose an image"))
    }
}

/// `name: Option<Image>`: `None` when no file was chosen.
impl FromInput<Upload> for Option<Image> {
    fn get(cx: &Cx, name: &str) -> Result<Option<Image>> {
        input::image(cx, name)
    }
}

/// What `#[validate(max_size = …)]` measures: an upload.
#[diagnostic::on_unimplemented(message = "`max_size` is for an `Image` parameter, not a `{Self}`")]
pub trait Size {
    fn size(&self) -> usize;
}

impl Size for Image {
    fn size(&self) -> usize {
        self.len()
    }
}

impl<T: Size> Size for Option<T> {
    fn size(&self) -> usize {
        self.as_ref().map_or(0, T::size)
    }
}

/// `#[validate(max_size = 1 * MB)]`: at most that many bytes.
pub fn max_size(v: &impl Size, most: usize) -> Option<String> {
    (v.size() > most).then(|| format!("must be at most {}", crate::image::size_text(most)))
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

/// The body of an action written without `->`: `#[action]` makes it return
/// `Result`, so it may end in `redirect("/")` or `invalid(..)`, use `?`, or
/// end in `;` as a function returning nothing does.
#[diagnostic::on_unimplemented(
    message = "an action without `->` ends in `;`, or in a `Result` such as `redirect(\"/\")`, not in a `{Self}`",
    note = "end its last line with `;`, or write the return type (`-> Response`)"
)]
pub trait Done {
    fn done(self) -> Result;
}

impl Done for () {
    #[inline(always)]
    fn done(self) -> Result {
        Ok(())
    }
}

impl Done for Result {
    #[inline(always)]
    fn done(self) -> Result {
        self
    }
}

/// What `#[action]` wraps such a body in.
#[inline(always)]
pub fn done(body: impl Done) -> Result {
    body.done()
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

    /// An image is itself, not its JSON (see [`crate::Image`]).
    impl Shape for &&&Ret<Image> {
        fn respond(self) -> Result<Response> {
            Ok(self.take().into())
        }
    }

    impl Shape for &&&Ret<Option<Image>> {
        fn respond(self) -> Result<Response> {
            self.take().map(Response::from).ok_or_else(not_found)
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

    #[test]
    #[allow(clippy::needless_borrow)]
    fn images_in_and_out() {
        let post = |file: &[u8]| {
            let mut raw = b"POST /p HTTP/1.1\r\ncontent-type: multipart/form-data; boundary=B\r\n\r\n--B\r\ncontent-disposition: form-data; name=\"pic\"; filename=\"a.png\"\r\ncontent-type: image/png\r\n\r\n".to_vec();
            raw.extend_from_slice(file);
            raw.extend_from_slice(b"\r\n--B--\r\n");
            Cx::for_test(&String::from_utf8_lossy(&raw), &[])
        };
        let gif = b"GIF89a\x01\0 rest";
        let cx = post(gif);
        let pic: Image = FromInput::get(&cx, "pic").unwrap();
        assert_eq!((pic.kind(), pic.bytes()), ("image/gif", &gif[..]));
        let none: Option<Image> = FromInput::get(&cx, "other").unwrap();
        assert!(none.is_none());
        let problem = |r: Result<Image>| r.unwrap_err().fields()[0].1.clone();
        assert_eq!(problem(FromInput::get(&cx, "other")), "choose an image");
        let svg = post(b"<svg onload=\"alert(1)\"/>");
        assert_eq!(
            problem(FromInput::get(&svg, "pic")),
            crate::image::NOT_AN_IMAGE
        );
        let maybe: Result<Option<Image>> = FromInput::get(&svg, "pic");
        assert_eq!(maybe.unwrap_err().status(), 422);

        assert_eq!(max_size(&pic, 13), None);
        assert_eq!(
            max_size(&pic, 12).as_deref(),
            Some("must be at most 12 bytes")
        );
        assert_eq!(max_size(&None::<Image>, 0), None);

        let r = (&&&Ret::new(Some(pic))).respond().unwrap();
        assert_eq!((&*r.content_type, r.body.len()), ("image/gif", 13));
        let gone = (&&&Ret::new(None::<Image>)).respond();
        assert_eq!(gone.err().map(|e| e.status()), Some(404));
    }
}
