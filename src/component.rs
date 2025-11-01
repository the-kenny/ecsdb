use std::any::Any;

use serde::{Serialize, de::DeserializeOwned};

pub use ecsdb_derive::{Bundle, Component};

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
        S::to_rusqlite(component)
    }
}

pub struct JsonStorage;

#[derive(thiserror::Error, Debug)]
#[error("Error reading/writing Component: {0}")]
pub struct StorageError(String);

impl<C> ComponentRead<C> for JsonStorage
where
    C: Component + DeserializeOwned,
{
    fn from_rusqlite(value: &rusqlite::types::ToSqlOutput<'_>) -> Result<C, StorageError> {
        let s = match value {
            rusqlite::types::ToSqlOutput::Borrowed(rusqlite::types::ValueRef::Text(s)) => s,
            rusqlite::types::ToSqlOutput::Owned(rusqlite::types::Value::Text(s)) => s.as_bytes(),
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

pub type BundleData<'a> = Vec<(&'static str, Option<rusqlite::types::ToSqlOutput<'a>>)>;
pub type BundleDataRef<'a> = &'a [(&'static str, Option<rusqlite::types::ToSqlOutput<'a>>)];

pub trait Bundle: Sized {
    const COMPONENTS: &'static [&'static str];

    fn component_names() -> &'static [&'static str] {
        Self::COMPONENTS
    }

    fn to_rusqlite<'a>(&'a self) -> Result<BundleData<'a>, StorageError>;
    // fn from_rusqlite<'a>(components: BundleDataRef<'a>) -> Result<Option<Self>, StorageError>;
}

pub trait BundleComponent {
    const NAME: &'static str;
    fn to_rusqlite<'a>(&'a self) -> Result<Option<rusqlite::types::ToSqlOutput<'a>>, StorageError>;
}

impl Bundle for () {
    const COMPONENTS: &'static [&'static str] = &[];

    fn to_rusqlite<'a>(&'a self) -> Result<BundleData<'a>, StorageError> {
        Ok(vec![])
    }
}

impl<C: Component> BundleComponent for C {
    const NAME: &'static str = C::NAME;

    fn to_rusqlite<'a>(&'a self) -> Result<Option<rusqlite::types::ToSqlOutput<'a>>, StorageError> {
        Ok(Some(C::to_rusqlite(self)?))
    }
}

impl<C: Component> BundleComponent for Option<C> {
    const NAME: &'static str = C::NAME;

    fn to_rusqlite<'a>(&'a self) -> Result<Option<rusqlite::types::ToSqlOutput<'a>>, StorageError> {
        match self {
            Some(c) => <C as BundleComponent>::to_rusqlite(c),
            None => Ok(None),
        }
    }
}

impl<C: Component> Bundle for C {
    const COMPONENTS: &'static [&'static str] = &[C::NAME];

    fn to_rusqlite<'a>(&'a self) -> Result<BundleData<'a>, StorageError> {
        Ok(vec![(C::NAME, Some(C::to_rusqlite(self)?))])
    }
}

impl<C: Component> Bundle for Option<C> {
    const COMPONENTS: &'static [&'static str] = &[C::NAME];

    fn to_rusqlite<'a>(&'a self) -> Result<BundleData<'a>, StorageError> {
        Ok(vec![(
            C::NAME,
            self.as_ref().map(C::to_rusqlite).transpose()?,
        )])
    }
}

macro_rules! bundle_tuples{
    ($($ts:ident)*) => {
        impl<$($ts,)+> Bundle for ($($ts,)+)
        where
            $($ts: BundleComponent,)+
        {
            const COMPONENTS: &'static [&'static str] = &[
                $($ts::NAME,)+
            ];

            fn to_rusqlite<'a>(
                &'a self
            ) -> Result<BundleData<'a>, StorageError> {
                #[allow(non_snake_case)]
                let ($($ts,)+) = self;
                Ok(
                    vec![
                        $(($ts::NAME, $ts::to_rusqlite($ts)?),)+
                    ]
                )
            }
        }
    }
}

crate::tuple_macros::for_each_tuple!(bundle_tuples);
