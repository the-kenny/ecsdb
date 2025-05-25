use tracing::warn;

use crate::{component, Component};

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
}
