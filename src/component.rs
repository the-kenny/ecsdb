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
    fn to_rusqlite<'a>(component: &'a C) -> Result<rusqlite::types::ToSqlOutput<'a>, StorageError>;
}

pub trait ComponentRead<C> {
    fn from_rusqlite(value: &rusqlite::types::ToSqlOutput<'_>) -> Result<C, StorageError>;
}

impl<C, S> ComponentRead<Self> for C
where
    C: Component<Storage = S>,
    S: ComponentRead<C>,
{
    fn from_rusqlite(value: &rusqlite::types::ToSqlOutput<'_>) -> Result<Self, StorageError> {
        S::from_rusqlite(value)
    }
}

impl<C, S> ComponentWrite<Self> for C
where
    C: Component<Storage = S>,
    S: ComponentWrite<C>,
{
    fn to_rusqlite<'a>(
        component: &'a Self,
    ) -> Result<rusqlite::types::ToSqlOutput<'a>, StorageError> {
        S::to_rusqlite(&component)
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
    fn from_rusqlite(value: &rusqlite::types::ToSqlOutput<'_>) -> Result<C, StorageError> {
        let s = match value {
            rusqlite::types::ToSqlOutput::Borrowed(rusqlite::types::ValueRef::Text(s)) => s,
            rusqlite::types::ToSqlOutput::Owned(rusqlite::types::Value::Text(ref s)) => {
                s.as_bytes()
            }
            other => return Err(StorageError(format!("Unexpected type {other:?}"))),
        };

        serde_json::from_slice(s).map_err(|e| StorageError(e.to_string()))
    }
}

impl<C> ComponentWrite<C> for JsonStorage
where
    C: Component + Serialize,
{
    fn to_rusqlite<'a>(component: &'a C) -> Result<rusqlite::types::ToSqlOutput<'a>, StorageError> {
        let json = serde_json::to_string(&component).map_err(|e| StorageError(e.to_string()))?;
        Ok(rusqlite::types::ToSqlOutput::Owned(
            rusqlite::types::Value::Text(json),
        ))
    }
}

pub struct BlobStorage;

impl<C> ComponentRead<C> for BlobStorage
where
    C: Component + From<Vec<u8>>,
{
    fn from_rusqlite(value: &rusqlite::types::ToSqlOutput<'_>) -> Result<C, StorageError> {
        let b = match value {
            rusqlite::types::ToSqlOutput::Borrowed(rusqlite::types::ValueRef::Blob(b)) => *b,
            rusqlite::types::ToSqlOutput::Owned(rusqlite::types::Value::Blob(b)) => b,
            other => return Err(StorageError(format!("Unexpected type {other:?}"))),
        };

        Ok(C::from(b.to_vec()))
    }
}

impl<C> ComponentWrite<C> for BlobStorage
where
    C: Component + AsRef<[u8]>,
{
    fn to_rusqlite<'a>(component: &'a C) -> Result<rusqlite::types::ToSqlOutput<'a>, StorageError> {
        Ok(rusqlite::types::ToSqlOutput::Borrowed(
            rusqlite::types::ValueRef::Blob(component.as_ref()),
        ))
    }
}

// impl<C> ComponentWrite<C> for BlobStorage
// where
//     C: Component + Into<Vec<u8>>,
// {
//     fn to_rusqlite<'a>(component: &'a C) -> Result<rusqlite::types::ToSqlOutput<'a>, StorageError> {
//         Ok(rusqlite::types::ToSqlOutput::Owned(
//             rusqlite::types::Value::Blob(component.into().as_slice()),
//         ))
//     }
// }

pub struct NullStorage;

impl<C> ComponentRead<C> for NullStorage
where
    C: Component + DeserializeOwned,
{
    fn from_rusqlite(value: &rusqlite::types::ToSqlOutput<'_>) -> Result<C, StorageError> {
        match value {
            rusqlite::types::ToSqlOutput::Borrowed(rusqlite::types::ValueRef::Null)
            | rusqlite::types::ToSqlOutput::Owned(rusqlite::types::Value::Null) => {
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
    fn to_rusqlite<'a>(
        _component: &'a C,
    ) -> Result<rusqlite::types::ToSqlOutput<'a>, StorageError> {
        Ok(rusqlite::types::ToSqlOutput::Owned(
            rusqlite::types::Value::Null,
        ))
    }
}

pub trait Bundle: Sized {
    const COMPONENTS: &'static [&'static str];
    fn component_names() -> &'static [&'static str] {
        Self::COMPONENTS
    }

    fn to_rusqlite<'a>(
        &'a self,
    ) -> Result<Vec<(&'static str, rusqlite::types::ToSqlOutput<'a>)>, StorageError>;

    fn from_rusqlite<'a>(
        components: &[(&'static str, rusqlite::types::ToSqlOutput<'a>)],
    ) -> Result<Option<Self>, StorageError>;
}

impl Bundle for () {
    const COMPONENTS: &'static [&'static str] = &[];

    fn to_rusqlite<'a>(
        &'a self,
    ) -> Result<Vec<(&'static str, rusqlite::types::ToSqlOutput<'a>)>, StorageError> {
        Ok(vec![])
    }

    fn from_rusqlite<'a>(
        _components: &[(&'static str, rusqlite::types::ToSqlOutput<'a>)],
    ) -> Result<Option<Self>, StorageError> {
        Ok(Some(()))
    }
}

impl<C: Component> Bundle for C {
    const COMPONENTS: &'static [&'static str] = &[C::NAME];

    fn to_rusqlite<'a>(
        &'a self,
    ) -> Result<Vec<(&'static str, rusqlite::types::ToSqlOutput<'a>)>, StorageError> {
        Ok(vec![(C::NAME, C::to_rusqlite(&self)?)])
    }

    fn from_rusqlite<'a>(
        components: &[(&'static str, rusqlite::types::ToSqlOutput<'a>)],
    ) -> Result<Option<Self>, StorageError> {
        let Some((_, value)) = components
            .into_iter()
            .find(|(c, _data)| *c == C::component_name())
        else {
            return Ok(None);
        };

        Ok(Some(C::from_rusqlite(value)?))
    }
}

macro_rules! bundle_tuples{
    ($($ts:ident)*) => {
        impl<$($ts,)+> Bundle for ($($ts,)+)
        where
            $($ts: Component,)+
        {
            const COMPONENTS: &'static [&'static str] = &[
                $($ts::NAME,)+
            ];

            fn to_rusqlite<'a>(
                &'a self
            ) -> Result<Vec<(&'static str, rusqlite::types::ToSqlOutput<'a>)>, StorageError> {
                #[allow(non_snake_case)]
                let ($($ts,)+) = self;
                Ok(
                    vec![
                        $(($ts::NAME, $ts::to_rusqlite($ts)?),)+
                    ]
                )
            }

            fn from_rusqlite<'a>(
                components: &[(&'static str, rusqlite::types::ToSqlOutput<'a>)],
            ) -> Result<Option<Self>, StorageError> {
                #[allow(non_snake_case)]
                let ($(Some($ts),)+) = (
                    $(<$ts as Bundle>::from_rusqlite(components)?,)+
                ) else {
                    return Ok(None)
                };

                Ok(Some(($($ts,)+)))
            }
        }

    }
}

crate::tuple_macros::for_each_tuple!(bundle_tuples);
