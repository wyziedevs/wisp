//! `wisp check --types` (the `types` feature): the TypeScript type of each
//! server value a page's or layout's script reads, from the Rust compiler's
//! own inference. The build wraps a block's statements in a closure that is
//! never called; its output type, a list of each value's [`Ts`], is all
//! that is read. No page code runs.

use crate::{Cx, Email, Image, Json, Page, Result, Row, Value};
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::Arc;

/// A type's TypeScript, for values sent to the browser. `#[derive(Json)]`
/// implements it under this feature.
pub trait Ts {
    /// The type without its lifetimes: a value borrowed in a block
    /// (`let name = &user.name;`) has a type that may not leave it.
    type Static: Ts + 'static;
    /// Whether its JSON is an object, which a [`Row`] adds `id` to.
    const OBJECT: bool = false;
    /// Writes this type's TypeScript into `d` (declaring named types there) and returns its name or shape.
    fn ts(d: &mut Decls) -> String;
}

/// The named types (`interface Item {..}`) a set of values refer to.
#[derive(Default)]
pub struct Decls(Vec<(&'static str, String)>);

impl Decls {
    /// `name`, declared once as what `decl` writes (`interface Item {..}`),
    /// which may refer to `name` itself.
    pub fn named(&mut self, name: &'static str, decl: impl FnOnce(&mut Decls) -> String) -> String {
        if !self.0.iter().any(|(n, _)| *n == name) {
            let at = self.0.len();
            self.0.push((name, String::new()));
            self.0[at].1 = decl(self);
        }
        name.to_string()
    }
}

/// A field, by a closure that reads it, as a [`Probe`]: the derive does
/// not need its type written out.
pub fn field<S, T: ?Sized>(_: fn(&S) -> &T) -> Probe<T> {
    Probe(PhantomData)
}

macro_rules! is {
    ($ts:literal: $($t:ty)*) => {$(
        impl Ts for $t {
            type Static = $t;
            fn ts(_: &mut Decls) -> String {
                $ts.into()
            }
        }
    )*};
}

is!("number": u8 u16 u32 u64 usize u128 i8 i16 i32 i64 isize i128 f32 f64);
is!("string": String char Image Email);
is!("boolean": bool);
is!("null": ());
is!("any": Value);

impl Ts for str {
    type Static = String;
    fn ts(_: &mut Decls) -> String {
        "string".into()
    }
}

impl Ts for Cow<'_, str> {
    type Static = String;
    fn ts(_: &mut Decls) -> String {
        "string".into()
    }
}

macro_rules! through {
    ($($w:ty),*) => {$(
        impl<T: Ts + ?Sized> Ts for $w {
            type Static = T::Static;
            const OBJECT: bool = T::OBJECT;
            fn ts(d: &mut Decls) -> String {
                T::ts(d)
            }
        }
    )*};
}

through!(&T, &mut T, Box<T>, Rc<T>, Arc<T>);

impl<T: Ts> Ts for Option<T> {
    type Static = Option<T::Static>;
    fn ts(d: &mut Decls) -> String {
        format!("{} | null", T::ts(d))
    }
}

/// `T[]`, with parentheses around a union or an intersection.
fn array(item: String) -> String {
    match item.contains(" | ") || item.contains(" & ") {
        true => format!("({item})[]"),
        false => format!("{item}[]"),
    }
}

impl<T: Ts> Ts for [T] {
    type Static = Vec<T::Static>;
    fn ts(d: &mut Decls) -> String {
        array(T::ts(d))
    }
}

impl<T: Ts, const N: usize> Ts for [T; N] {
    type Static = Vec<T::Static>;
    fn ts(d: &mut Decls) -> String {
        array(T::ts(d))
    }
}

impl<T: Ts> Ts for Vec<T> {
    type Static = Vec<T::Static>;
    fn ts(d: &mut Decls) -> String {
        array(T::ts(d))
    }
}

macro_rules! tuples {
    ($(($($name:ident),+))*) => {$(
        impl<$($name: Ts),+> Ts for ($($name,)+) {
            type Static = ($($name::Static,)+);
            fn ts(d: &mut Decls) -> String {
                let items = [$($name::ts(d)),+];
                format!("[{}]", items.join(", "))
            }
        }
    )*};
}

tuples! {
    (A)
    (A, B)
    (A, B, C)
    (A, B, C, D)
}

impl<K: AsRef<str>, V: Ts> Ts for BTreeMap<K, V> {
    type Static = BTreeMap<String, V::Static>;
    const OBJECT: bool = true;
    fn ts(d: &mut Decls) -> String {
        format!("Record<string, {}>", V::ts(d))
    }
}

impl<K: AsRef<str>, V: Ts, S> Ts for HashMap<K, V, S> {
    type Static = BTreeMap<String, V::Static>;
    const OBJECT: bool = true;
    fn ts(d: &mut Decls) -> String {
        format!("Record<string, {}>", V::ts(d))
    }
}

/// A row is its value's object with `id`, or `{ id, value }`.
impl<T: Ts> Ts for Row<T> {
    type Static = Row<T::Static>;
    const OBJECT: bool = true;
    fn ts(d: &mut Decls) -> String {
        match T::OBJECT {
            true => format!("{{ id: number }} & {}", T::ts(d)),
            false => format!("{{ id: number; value: {} }}", T::ts(d)),
        }
    }
}

impl<T: Ts> Ts for Page<T> {
    type Static = Page<T::Static>;
    const OBJECT: bool = true;
    fn ts(d: &mut Decls) -> String {
        let row = array(Row::<T>::ts(d));
        format!("{{ rows: {row}; number: number; prev: string | null; next: string | null }}")
    }
}

/// A value, by reference, whose type picks [`Known`] or [`Unknown`]:
/// `(&&probe(&x)).pick()`, with [`ViaTs`] and [`ViaAny`] in scope; or its
/// TypeScript, `(&&probe(&x)).ts(d)`, `unknown` for a type with none.
pub struct Probe<T: ?Sized>(PhantomData<T>);

/// Makes a [`Probe`] of a value's type, to pick [`Known`] or [`Unknown`].
pub fn probe<T: ?Sized>(_: &T) -> Probe<T> {
    Probe(PhantomData)
}

/// A type with TypeScript.
pub struct Known<T>(PhantomData<T>);
/// A type with none: `unknown`.
pub struct Unknown;

/// Picks a type that has TypeScript.
pub trait ViaTs {
    /// The picked type.
    type T;
    /// Its [`Known`] pick.
    fn pick(&self) -> Known<Self::T>;
    /// The type's TypeScript.
    fn ts(&self, d: &mut Decls) -> String;
}

impl<T: Ts + ?Sized> ViaTs for &Probe<T> {
    type T = T::Static;
    fn pick(&self) -> Known<T::Static> {
        Known(PhantomData)
    }
    fn ts(&self, d: &mut Decls) -> String {
        T::ts(d)
    }
}

/// A type with no TypeScript: `unknown`.
pub trait ViaAny {
    /// Its [`Unknown`] pick.
    fn pick(&self) -> Unknown;
    /// `unknown`.
    fn ts(&self, d: &mut Decls) -> String;
}

impl<T: ?Sized> ViaAny for Probe<T> {
    fn pick(&self) -> Unknown {
        Unknown
    }
    fn ts(&self, _: &mut Decls) -> String {
        "unknown".into()
    }
}

/// The picks of a block's values, as `(a, (b, ()))`.
pub trait Fields {
    /// Appends the TypeScript of each value of the list to `out`.
    fn list(d: &mut Decls, out: &mut Vec<String>);
}

impl Fields for () {
    fn list(_: &mut Decls, _: &mut Vec<String>) {}
}

impl<T: Ts, R: Fields> Fields for (Known<T>, R) {
    fn list(d: &mut Decls, out: &mut Vec<String>) {
        out.push(T::ts(d));
        R::list(d, out);
    }
}

impl<R: Fields> Fields for (Unknown, R) {
    fn list(d: &mut Decls, out: &mut Vec<String>) {
        out.push("unknown".into());
        R::list(d, out);
    }
}

/// A page's statements, as the closure the build made of them (never
/// called): `"file":{"values":[[name, type]..],"decls":[[name, decl]..]},`.
pub fn page<C, F, T>(_: &C, rel: &str, names: &[&str], out: &mut String)
where
    C: FnOnce(&'static mut Cx) -> F,
    F: Future<Output = Result<T>>,
    T: Fields,
{
    write::<T>(rel, names, out);
}

/// [`page`] for a layout's statements, which are sync.
pub fn layout<C, T>(_: &C, rel: &str, names: &[&str], out: &mut String)
where
    C: FnOnce(&'static Cx) -> T,
    T: Fields,
{
    write::<T>(rel, names, out);
}

fn write<T: Fields>(rel: &str, names: &[&str], out: &mut String) {
    let mut d = Decls::default();
    let mut types = Vec::new();
    T::list(&mut d, &mut types);
    let values: Vec<(&str, String)> = names.iter().copied().zip(types).collect();
    let decls: Vec<(&str, &str)> = d.0.iter().map(|(n, t)| (*n, t.as_str())).collect();
    if !out.is_empty() {
        out.push(',');
    }
    rel.json(out);
    out.push_str(":{\"values\":");
    values.json(out);
    out.push_str(",\"decls\":");
    decls.json(out);
    out.push('}');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts<T: Ts + ?Sized>() -> String {
        T::ts(&mut Decls::default())
    }

    #[test]
    fn types() {
        assert_eq!(ts::<Vec<Option<u8>>>(), "(number | null)[]");
        assert_eq!(ts::<&[&str]>(), "string[]");
        assert_eq!(ts::<(u8, String)>(), "[number, string]");
        assert_eq!(ts::<HashMap<String, bool>>(), "Record<string, boolean>");
        assert_eq!(ts::<Row<String>>(), "{ id: number; value: string }");
        assert_eq!(
            ts::<Vec<Row<BTreeMap<String, f64>>>>(),
            "({ id: number } & Record<string, number>)[]"
        );
    }
}
