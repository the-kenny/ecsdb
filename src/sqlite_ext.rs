use rusqlite::functions::FunctionFlags;
use rusqlite::types::{Value, ValueRef};
use rusqlite::{Connection, Error, Result};

pub(crate) fn add_regexp_function(db: &Connection) -> Result<()> {
    db.create_scalar_function(
        "velodb_extract_data",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        move |ctx| {
            assert_eq!(ctx.len(), 1, "called with unexpected number of arguments");

            match ctx.get_raw(0) {
                ValueRef::Null => Ok(Value::Null),
                ValueRef::Integer(i) => Ok(Value::Integer(i)),
                ValueRef::Real(r) => Ok(Value::Real(r)),
                // Return NULL for BLOB - no JSON extraction possible
                ValueRef::Blob(_blob) => Ok(Value::Null),
                // JSON
                ValueRef::Text(text) => {
                    let value: serde_json::Value = serde_json::from_slice(text)
                        .map_err(|e| Error::UserFunctionError(Box::new(e)))?;

                    let sqlite_value = match value {
                        serde_json::Value::Null => Value::Null,
                        serde_json::Value::Bool(true) => Value::Integer(1),
                        serde_json::Value::Bool(false) => Value::Integer(0),
                        serde_json::Value::Number(n) => n
                            .as_i64()
                            .map(Value::Integer)
                            .or(n.as_f64().map(Value::Real))
                            .unwrap(),
                        serde_json::Value::String(s) => Value::Text(s),
                        array @ serde_json::Value::Array(_) => Value::Text(array.to_string()),
                        obj @ serde_json::Value::Object(_) => Value::Text(obj.to_string()),
                    };

                    Ok(sqlite_value)
                }
            }
        },
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn custom_fn_test() -> Result<(), anyhow::Error> {
        let db = crate::Ecs::open_in_memory()?;
        let result: bool = db.raw_sql().query_row(
            "select velodb_extract_data(json_quote(10)) > velodb_extract_data(json_quote(2))",
            [],
            |row| row.get(0),
        )?;

        assert_eq!(result, true);

        Ok(())
    }
}
