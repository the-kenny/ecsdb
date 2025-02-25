use std::any::Any;

use serde::{de::DeserializeOwned, Serialize};

pub use ecsdb_derive::Component;

pub trait Component: Sized + Any + ComponentRead<Self> + ComponentWrite<Self> {
    type Storage;

    const NAME: &'static str;
    fn component_name() -> &'static str {
        Self::NAME
    }
}

pub trait ComponentWrite<C> {
    fn to_rusqlite(component: C) -> Result<rusqlite::types::Value, StorageError>;
}

pub trait ComponentRead<C> {
    fn from_rusqlite(value: rusqlite::types::Value) -> Result<C, StorageError>;
}

impl<C, S> ComponentRead<Self> for C
where
    C: Component<Storage = S>,
    S: ComponentRead<C>,
{
    fn from_rusqlite(value: rusqlite::types::Value) -> Result<Self, StorageError> {
        S::from_rusqlite(value)
    }
}

impl<C, S> ComponentWrite<Self> for C
where
    C: Component<Storage = S>,
    S: ComponentWrite<C>,
{
    fn to_rusqlite(component: Self) -> Result<rusqlite::types::Value, StorageError> {
        S::to_rusqlite(component)
    }
}

pub struct JsonStorage;

#[derive(thiserror::Error, Debug)]
#[error("Error storing Component: {0}")]
pub struct StorageError(String);

impl<C> ComponentRead<C> for JsonStorage
where
    C: Component + DeserializeOwned,
{
    fn from_rusqlite(value: rusqlite::types::Value) -> Result<C, StorageError> {
        match value {
            rusqlite::types::Value::Text(s) => {
                serde_json::from_str(&s).map_err(|e| StorageError(e.to_string()))
            }
            other => Err(StorageError(format!("Unexpected type {other:?}"))),
        }
    }
}

impl<C> ComponentWrite<C> for JsonStorage
where
    C: Component + Serialize,
{
    fn to_rusqlite(component: C) -> Result<rusqlite::types::Value, StorageError> {
        let json = serde_json::to_string(&component).map_err(|e| StorageError(e.to_string()))?;
        Ok(rusqlite::types::Value::Text(json))
    }
}

pub struct BlobStorage;

impl<C> ComponentRead<C> for BlobStorage
where
    C: Component + From<Vec<u8>>,
{
    fn from_rusqlite(value: rusqlite::types::Value) -> Result<C, StorageError> {
        match value {
            rusqlite::types::Value::Blob(b) => Ok(C::from(b)),
            other => Err(StorageError(format!("Unexpected type {other:?}"))),
        }
    }
}

impl<C> ComponentWrite<C> for BlobStorage
where
    C: Component + Into<Vec<u8>>,
{
    fn to_rusqlite(component: C) -> Result<rusqlite::types::Value, StorageError> {
        Ok(rusqlite::types::Value::Blob(component.into()))
    }
}

pub struct NullStorage;

impl<C> ComponentRead<C> for NullStorage
where
    C: Component + DeserializeOwned,
{
    fn from_rusqlite(value: rusqlite::types::Value) -> Result<C, StorageError> {
        match value {
            rusqlite::types::Value::Null => {
                serde_json::from_str("null").map_err(|e| StorageError(e.to_string()))
            }
            other => Err(StorageError(format!("Unexpected type {other:?}"))),
        }
    }
}

impl<C> ComponentWrite<C> for NullStorage
where
    C: Component + Serialize,
{
    fn to_rusqlite(_component: C) -> Result<rusqlite::types::Value, StorageError> {
        Ok(rusqlite::types::Value::Null)
    }
}

pub trait Bundle {
    const COMPONENTS: &'static [&'static str];
    fn component_names() -> &'static [&'static str] {
        Self::COMPONENTS
    }

    fn to_rusqlite(self) -> Result<Vec<(&'static str, rusqlite::types::Value)>, StorageError>;
}

impl<C: Component> Bundle for C {
    const COMPONENTS: &'static [&'static str] = &[C::NAME];

    fn to_rusqlite(self) -> Result<Vec<(&'static str, rusqlite::types::Value)>, StorageError> {
        Ok(vec![(C::NAME, C::to_rusqlite(self)?)])
    }
}

macro_rules! bundle_tuple_impls {
    ($t:tt, $($ts:tt),+) => {
        impl<$t, $($ts,)+> Bundle for ($t, $($ts,)+)
        where
            $t: Component,
            $($ts: Component,)+
        {
            const COMPONENTS: &'static [&'static str] = &[
                $t::NAME,
                $($ts::NAME,)+
            ];

            fn to_rusqlite(
                self
            ) -> Result<Vec<(&'static str, rusqlite::types::Value)>, StorageError> {
                #[allow(non_snake_case)]
                let ($t, $($ts,)+) = self;
                Ok(
                    vec![
                        ($t::NAME, $t::to_rusqlite($t)?),
                        $(($ts::NAME, $ts::to_rusqlite($ts)?),)+
                    ]
                )
            }
        }


        bundle_tuple_impls!($($ts),+);
    };
    ($t:tt) => {
        impl<$t: Component> Bundle for ($t,) {
            const COMPONENTS: &'static [&'static str] = &[ $t::NAME ];

            fn to_rusqlite(
                self
            ) -> Result<Vec<(&'static str, rusqlite::types::Value)>, StorageError> {
                let (t,) = self;
                Ok(
                    vec![($t::NAME, $t::to_rusqlite(t)?)]
                )
            }
        }

    };
}

bundle_tuple_impls!(A, B, C);
