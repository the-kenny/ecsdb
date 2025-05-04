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

        let text = match self.1 {
            ToSqlOutput::Borrowed(ValueRef::Text(s)) => s,
            ToSqlOutput::Owned(Value::Text(ref s)) => s.as_bytes(),
            _ => todo!(),
        };

        serde_json::from_slice(text).ok()
    }
}
