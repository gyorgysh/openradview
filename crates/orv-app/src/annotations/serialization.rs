// SPDX-License-Identifier: GPL-3.0-or-later
//! Enforce collection bounds while decoding, before unconstrained vectors can grow.
use super::{MAX_MARKS, SavedMark};
use serde::{
    Deserialize, Deserializer,
    de::{self, SeqAccess, Visitor},
};
use std::{fmt, marker::PhantomData};
fn bounded<'de, D, T, const LIMIT: usize>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Bounded<T, const LIMIT: usize>(PhantomData<T>);
    impl<'de, T: Deserialize<'de>, const LIMIT: usize> Visitor<'de> for Bounded<T, LIMIT> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "at most {LIMIT} bounded measurement values")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
            if sequence.size_hint().is_some_and(|size| size > LIMIT) {
                return Err(de::Error::custom("Too many measurement values"));
            }
            let mut values = Vec::new();
            while let Some(value) = sequence.next_element()? {
                if values.len() == LIMIT {
                    return Err(de::Error::custom("Too many measurement values"));
                }
                values
                    .try_reserve(1)
                    .map_err(|_| de::Error::custom("Not enough memory for measurement values"))?;
                values.push(value);
            }
            Ok(values)
        }
    }
    deserializer.deserialize_seq(Bounded::<T, LIMIT>(PhantomData))
}
pub(super) fn deserialize_points<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<[f64; 2]>, D::Error> {
    bounded::<D, [f64; 2], { orv_core::measurements::MAX_POLYGON_VERTICES }>(deserializer)
}
pub(crate) fn deserialize_marks<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<SavedMark>, D::Error> {
    bounded::<D, SavedMark, MAX_MARKS>(deserializer)
}
