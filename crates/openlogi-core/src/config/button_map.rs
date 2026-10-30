//! Deserialization of tables keyed by a typed name, rejecting one key named
//! twice.
//!
//! A key can have more than one spelling. A control's catalog name and its
//! `control:0x010a` form load as the same [`ButtonId`], and `Back+Forward` and
//! `Forward+Back` as the same [`ChordId`]. TOML's own duplicate-key check cannot
//! see that collision, and a plain map's last-wins insert would drop one entry
//! silently, then persist the loss on the next save.

use std::collections::BTreeMap;
use std::fmt;
use std::marker::PhantomData;
use std::str::FromStr;

use serde::de::{self, Deserialize, Deserializer, MapAccess, Visitor};

use crate::binding::{ButtonId, ChordId};

/// A typed table key, and the word for it in a load error.
pub(super) trait TableKey: FromStr<Err: fmt::Display> + Ord + Copy {
    const NOUN: &'static str;
}

impl TableKey for ButtonId {
    const NOUN: &'static str = "control";
}

impl TableKey for ChordId {
    const NOUN: &'static str = "chord";
}

/// Deserialize a keyed table, failing on a key that appears under two
/// spellings.
pub(super) fn deserialize_keyed_map<'de, D, K, V>(
    deserializer: D,
) -> Result<BTreeMap<K, V>, D::Error>
where
    D: Deserializer<'de>,
    K: TableKey,
    V: Deserialize<'de>,
{
    deserializer.deserialize_map(KeyedMapVisitor(PhantomData))
}

/// Deserialize the per-app overlays, a table of keyed tables, with the same
/// guard on every inner table.
pub(super) fn deserialize_keyed_maps_by_app<'de, D, K, V>(
    deserializer: D,
) -> Result<BTreeMap<String, BTreeMap<K, V>>, D::Error>
where
    D: Deserializer<'de>,
    K: TableKey,
    V: Deserialize<'de>,
{
    let maps = BTreeMap::<String, KeyedMap<K, V>>::deserialize(deserializer)?;
    Ok(maps.into_iter().map(|(app, map)| (app, map.0)).collect())
}

struct KeyedMap<K, V>(BTreeMap<K, V>);

impl<'de, K: TableKey, V: Deserialize<'de>> Deserialize<'de> for KeyedMap<K, V> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserialize_keyed_map(deserializer).map(Self)
    }
}

struct KeyedMapVisitor<K, V>(PhantomData<(K, V)>);

impl<'de, K: TableKey, V: Deserialize<'de>> Visitor<'de> for KeyedMapVisitor<K, V> {
    type Value = BTreeMap<K, V>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a table keyed by {} name", K::NOUN)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
        let mut map = BTreeMap::new();
        let mut spellings: BTreeMap<K, String> = BTreeMap::new();
        while let Some(name) = access.next_key::<String>()? {
            let key: K = name.parse().map_err(de::Error::custom)?;
            let value = access.next_value::<V>()?;
            if let Some(earlier) = spellings.insert(key, name.clone()) {
                return Err(de::Error::custom(format_args!(
                    "`{earlier}` and `{name}` name the same {}; keep one",
                    K::NOUN
                )));
            }
            map.insert(key, value);
        }
        Ok(map)
    }
}
