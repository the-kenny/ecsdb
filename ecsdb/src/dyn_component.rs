use tracing::warn;

use crate::{Component, component};

#[derive(Debug, Clone, Copy)]
pub enum Kind {
    Json,
    Blob,
    Null,
    Other(rusqlite::types::Type),
}

#[derive(Debug)]
pub struct DynComponent<'a>(
    pub(crate) &'a str,
    pub(crate) rusqlite::types::ToSqlOutput<'a>,
);

impl<'a> DynComponent<'a> {
    pub fn name(&'a self) -> &'a str {
        self.0
    }

    pub fn into_typed<C: Component>(self) -> Result<C, component::StorageError> {
        C::from_rusqlite(&self.1)
    }

    pub fn as_typed<C: Component>(&self) -> Result<C, component::StorageError> {
        C::from_rusqlite(&self.1)
    }

    pub fn from_typed<C: Component + 'a>(c: &'a C) -> Result<Self, component::StorageError> {
        Ok(Self(C::component_name(), C::to_rusqlite(c)?))
    }

    pub fn kind(&self) -> Kind {
        use rusqlite::types::{ToSqlOutput, Type};

        let t = match self.1 {
            ToSqlOutput::Borrowed(value) => value.data_type(),
            ToSqlOutput::Owned(ref value) => value.data_type(),
            ref other => unreachable!("Unexpected ToSqlOutput {other:?}"),
        };

        match t {
            Type::Text => Kind::Json,
            Type::Blob => Kind::Blob,
            Type::Null => Kind::Null,
            other => Kind::Other(other),
        }
    }

    pub fn as_json(&self) -> Option<serde_json::value::Value> {
        use rusqlite::types::{ToSqlOutput, Value, ValueRef};

        match self.1 {
            ToSqlOutput::Borrowed(ValueRef::Text(s)) => serde_json::from_slice(s).ok(),
            ToSqlOutput::Owned(Value::Text(ref s)) => serde_json::from_slice(s.as_bytes()).ok(),
            ToSqlOutput::Owned(Value::Null) | ToSqlOutput::Borrowed(ValueRef::Null) => {
                Some(serde_json::Value::Null)
            }
            ToSqlOutput::Owned(ref o) => {
                warn!(r#type = ?o.data_type(), "DynComponent::as_json unsupported");
                None
            }
            ToSqlOutput::Borrowed(ref b) => {
                warn!(r#type = ?b.data_type(), "DynComponent::as_json unsupported");
                None
            }
            ref x => {
                warn!(value = ?x, "DynComponent::as_json unsupported");
                None
            }
        }
    }

    pub fn as_blob(&self) -> Option<&[u8]> {
        use rusqlite::types::{ToSqlOutput, Value, ValueRef};

        match self.1 {
            ToSqlOutput::Borrowed(ValueRef::Blob(b)) => Some(b),
            ToSqlOutput::Owned(Value::Blob(ref b)) => Some(b),
            ToSqlOutput::Owned(Value::Null) | ToSqlOutput::Borrowed(ValueRef::Null) => Some(&[]),
            ToSqlOutput::Owned(ref o) => {
                warn!(r#type = ?o.data_type(), "DynComponent::as_blob unsupported");
                None
            }
            ToSqlOutput::Borrowed(ref b) => {
                warn!(r#type = ?b.data_type(), "DynComponent::as_blob unsupported");
                None
            }
            ref x => {
                warn!(value = ?x, "DynComponent::as_blob unsupported");
                None
            }
        }
    }

    pub fn into_blob(self) -> Option<Vec<u8>> {
        use rusqlite::types::{ToSqlOutput, Value, ValueRef};

        match self.1 {
            ToSqlOutput::Borrowed(ValueRef::Blob(b)) => Some(b.to_owned()),
            ToSqlOutput::Owned(Value::Blob(b)) => Some(b),
            ToSqlOutput::Owned(Value::Null) | ToSqlOutput::Borrowed(ValueRef::Null) => Some(vec![]),
            ToSqlOutput::Owned(ref o) => {
                warn!(r#type = ?o.data_type(), "DynComponent::into_blob unsupported");
                None
            }
            ToSqlOutput::Borrowed(ref b) => {
                warn!(r#type = ?b.data_type(), "DynComponent::into_blob unsupported");
                None
            }
            ref x => {
                warn!(value = ?x, "DynComponent::into_blob unsupported");
                None
            }
        }
    }

    pub fn from_json(
        name: &'a str,
        value: &serde_json::Value,
    ) -> Result<Self, component::StorageError> {
        let value = serde_json::to_string(value).expect("Serializable JSON");
        Ok(Self(
            name,
            rusqlite::types::ToSqlOutput::Owned(rusqlite::types::Value::Text(value)),
        ))
    }
}
